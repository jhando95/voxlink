# Design — Milestone 6: Idle Efficiency Audit

**Date:** 2026-04-19
**Status:** Approved (pending spec review)
**Scope:** Measure idle CPU% across five scenarios, audit all periodic work, kill or throttle what's unnecessary, re-measure, commit the improvement. Directly protects Voxlink's "low-overhead" core identity.

## Context

CLAUDE.md names "minimal idle CPU and RAM" as engineering priority #1. The tick loop already has an active/idle split (40 Hz → 10 Hz via `TICK_MS_ACTIVE=25` / `TICK_MS_IDLE=100`), which is good, but it's one optimization in a larger surface. There are numerous `tokio::spawn`, `slint::Timer::default()`, and interval-based background tasks across both the client (`app_desktop`) and server (`signaling_server`) that have never been audited as a whole.

M5 landed microbenchmark baselines and a regression gate; this milestone is the macro-level analog: measure the app at rest, find the waste, kill it.

## Goals

1. Record reproducible idle CPU% baselines for five scenarios.
2. Audit every periodic task in the client and server.
3. Fix at least the top 2–3 waste sources, or conclude with documented evidence that nothing meaningful remains.
4. Re-measure. Commit the new numbers alongside the old ones so the improvement is self-evident.
5. No behavioral regressions.

## Non-goals

- **Startup time.** M7.
- **Audio callback latency.** M8.
- **Memory profiling.** M9.
- **GPU profiling.** The Slint software renderer is already used; GPU isn't the hot path.
- **Server scale testing.** Idle only; "server under load" is a separate concern.
- **Micro-optimizing hot paths during calls.** M5 already gated those. Idle is a distinct regime.

## Architecture

Three phases, each leaves the workspace green.

### Phase 1: Baseline measurement

Add `scripts/measure-idle-cpu.sh` — a reproducible script that:
1. Starts the signaling server in the background on a free loopback port.
2. Samples server CPU% for 30 s at zero peers using `top -pid <pid> -l 30 -stats cpu -s 1`.
3. Starts the client against that server.
4. Samples client CPU% for 30 s in each of three client states:
   - Home screen (no room joined).
   - Joined to a test room, sending silence (mic muted).
   - Window minimized (joined + muted).
5. Simulates a second client connection to the server (to capture "server with 1 peer idle").
6. Reports a results table.

Script uses a combination of the existing `cargo run -p signaling_server` and `cargo run -p app_desktop` (or release binaries if already built). Cleans up child processes on exit via `trap`.

Output goes to stdout in markdown-table form so it can be pasted directly into `docs/PERFORMANCE_TARGETS.md`.

**Scenarios to measure:**

| Label | Target | Description |
|---|---|---|
| `client_home` | client | App launched, sitting on the home view, no server connection |
| `client_joined_silent` | client | Joined to a room, mic muted, no one talking |
| `client_minimized` | client | Same as joined_silent but window minimized |
| `server_zero_peers` | server | Server running, no clients connected |
| `server_one_idle_peer` | server | One client connected to a room, silent |

### Phase 2: Audit

Grep across both crates for periodic work markers:

```
rg '(tokio::spawn|thread::spawn|slint::Timer|set_interval|Duration::from_(secs|millis)|interval\()' crates/
```

For each hit, record in a temporary `/tmp/voxlink-idle-audit.md` table:

| File:line | Task | Frequency | Purpose | Idle-path cost | Action |
|---|---|---|---|---|---|

Categories of action:
- **KEEP** — task is correctly throttled and justified.
- **THROTTLE** — reduce frequency when idle or invisible.
- **GATE** — suspend entirely when a precondition is false (e.g., window hidden).
- **REPLACE** — convert polling to event-driven (atomic notify, channel, etc.).
- **KILL** — task has no reason to exist, delete.

The audit table is checked in as `docs/IDLE_AUDIT.md` so future contributors have the trail.

### Phase 3: Fixes

One commit per fix. Each commit:
1. Applies the fix.
2. Re-runs `scripts/measure-idle-cpu.sh`.
3. Records the new numbers in a "before → after" diff line appended to `docs/PERFORMANCE_TARGETS.md`.
4. Manual smoke test: join a real test room, send audio, leave. Confirm no regression.

Expected fix themes (finalized from the audit):

- **Window-minimized gating.** Pause non-essential UI timers (unread refreshers, presence tickers, anything that updates only the view) when the Slint window isn't visible. Slint exposes a window visibility callback.
- **Tick-loop idle detection tightening.** If the current idle-detector only switches to `TICK_MS_IDLE` when *not in a room*, extend it to detect "in a room but nobody talking for N seconds" and slow down further (e.g., to 5 Hz or 2 Hz).
- **Server idle trimming.** Server likely has timers for scheduled messages, voice-note cleanup, auth-attempt pruning, etc. If any of these fire every second regardless of activity, batch or event-drive them.
- **Tokio runtime size.** If the server uses more worker threads than necessary for its actual concurrency, reduce. (M1 memory note says the runtime was unbounded after a config change; re-check.)
- **UDP keepalive.** 15 s interval is fine when connected; confirm it doesn't fire when no peers have UDP sessions.

**If the audit finds nothing actionable >5 % CPU improvement:** close M6 as "verified efficient" with the baseline table committed. This is a valid outcome — proving the app is already tight is as valuable as tightening it.

### Phase 4: Re-measure + document

Update `docs/PERFORMANCE_TARGETS.md` with a new "Idle CPU baselines" section that shows before/after numbers. Format:

```markdown
## Idle CPU baselines

Measured on Apple M4 Pro, release build, 2026-04-19 / 2026-04-19 (post-M6).

| Scenario              | Before | After | Δ       |
|-----------------------|-------:|------:|--------:|
| client_home           |  X.X % | Y.Y % |  −Z.Z % |
| client_joined_silent  |  X.X % | Y.Y % |  −Z.Z % |
| client_minimized      |  X.X % | Y.Y % |  −Z.Z % |
| server_zero_peers     |  X.X % | Y.Y % |  −Z.Z % |
| server_one_idle_peer  |  X.X % | Y.Y % |  −Z.Z % |
```

## Components

| File | Change |
|---|---|
| `scripts/measure-idle-cpu.sh` *(new)* | Reproducible idle-CPU measurement |
| `docs/IDLE_AUDIT.md` *(new)* | Audit trail of every periodic task |
| `docs/PERFORMANCE_TARGETS.md` | New "Idle CPU baselines" section |
| Client tick loop / main / tray code | Throttle / gate fixes per audit |
| Server background tasks | Same |
| `crates/app_desktop/src/*` (window visibility wiring) | New event hook if Slint callback used |

Total runtime-code footprint: depends on audit findings. Target: ≤ 5 files touched, ≤ 200 LoC changed.

## Testing

- `scripts/measure-idle-cpu.sh` runs end-to-end on the dev machine; produces the expected scenario rows.
- `cargo check --workspace` green at every commit.
- `cargo clippy --workspace --all-targets` ≤ 62 warnings.
- Existing tests pass; known-flaky integration tests remain skipped.
- Manual smoke test per fix: join a room, send audio, leave. Audio plays cleanly on both sides.

## Risks

- **Measurement noise.** `top` CPU% over 30 s can vary 2–3 percentage points between runs on a quiet machine, more on a loaded one. Mitigation: take 3 samples per scenario, report the median. If the script times out, record as "inconclusive" rather than a hard number.
- **Window-visibility event missing or unreliable.** Slint's window visibility API may not fire on macOS when a window goes behind another. Mitigation: fall back to a manual "idle/focused" application state toggle driven by existing activity tracking.
- **Fixes break voice reliability.** Each fix ships as its own commit with manual smoke test. Revert individually if needed.
- **Audit runs long.** The grep produces dozens of hits; triaging each by hand is tedious. Mitigation: focus on the 5–10 most frequent (highest idle-path cost × frequency); defer the rest to follow-ups.

## Commit strategy

Phase 1 (1 commit):
1. `scripts: add measure-idle-cpu.sh with before-audit baseline numbers`

Phase 2 (1 commit):
2. `docs: idle periodic-work audit table`

Phase 3 (N commits, one per fix):
3. `perf(client): pause UI timers when window is minimized` (if that's finding #1)
4. `perf(client): tighten idle-state detection` (if that's finding #2)
5. `perf(server): batch cleanup tasks instead of per-second scans` (if that's finding #3)
… etc.

Phase 4 (1 commit):
6. `docs: record post-M6 idle CPU baselines` (before/after delta)

Total: 3 + N commits, where N is 0–5 depending on what the audit finds.

## Success criteria

1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets` ≤ 62 warnings.
3. Existing tests pass.
4. `scripts/measure-idle-cpu.sh` runs on the dev machine and produces the five scenario numbers.
5. `docs/IDLE_AUDIT.md` exists and lists every periodic task in both crates with a triage decision.
6. `docs/PERFORMANCE_TARGETS.md` has an "Idle CPU baselines" section with before/after numbers.
7. **Either** a measurable improvement (≥ 30 % drop in at least one scenario) **or** documented evidence that the app was already at the efficient floor.
8. Manual smoke test passes after the final commit: join a room, send audio, leave, re-join.

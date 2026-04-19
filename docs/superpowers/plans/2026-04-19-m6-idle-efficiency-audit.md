# M6 — Idle Efficiency Audit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Measure idle CPU% across five scenarios, audit every periodic task in the client and server, throttle/gate/kill the waste, re-measure and document the improvement.

**Architecture:** Four phases — (1) baseline measurement via a reproducible shell script, (2) audit table checked in as `IDLE_AUDIT.md`, (3) targeted fixes driven by the audit (one commit per fix), (4) re-measurement. Each fix ships as an isolated commit with before/after numbers.

**Tech Stack:** bash, `top` (macOS built-in), `cargo run --release`, Slint's existing timer API, tokio. No new deps.

**Spec:** `docs/superpowers/specs/2026-04-19-m6-idle-efficiency-audit-design.md`

**Workspace root:** `/Users/jph/Voiceapp/workspace_template`

**Branch:** start on `feat/m6-idle-audit` from `main`.

---

## Ground rules

1. **Workspace stays green.** `cargo check --workspace` passes before each commit.
2. **No new clippy warnings.** Baseline is 62; must not exceed.
3. **No behavioral regressions.** After each fix commit, manually smoke-test: launch app, join test room, send audio, leave. If audio misbehaves, revert the commit.
4. **One fix per commit.** Each fix in Phase 3 is independently revertable.
5. **Median of three.** Idle CPU measurements take three samples and report the median; a single 30s `top` sample is too noisy.
6. **Use release builds.** Debug builds skew CPU numbers wildly; all measurements run `cargo run --release` or prebuilt release binaries.

---

## Task 0: Branch + baseline build

- [ ] **Step 1: Verify clean working tree**

```
cd /Users/jph/Voiceapp/workspace_template && git status --short
```
Expected: empty. Commit or stash any modifications first.

- [ ] **Step 2: Create feature branch**

```
git checkout -b feat/m6-idle-audit
```

- [ ] **Step 3: Record starting clippy count**

```
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: `62`.

- [ ] **Step 4: Pre-build release binaries**

```
cargo build --release -p signaling_server -p app_desktop
```
Expected: succeeds. This caches the compile so measurement scripts run without compile overhead inside the sampling window.

No commit yet.

---

## Task 1: `scripts/measure-idle-cpu.sh`

**Files:**
- Create: `scripts/measure-idle-cpu.sh`

**What this adds:** A reproducible script that launches the server + one or two clients and samples CPU% for each scenario. Output is a markdown table ready to paste into `PERFORMANCE_TARGETS.md`.

- [ ] **Step 1: Create `scripts/measure-idle-cpu.sh`**

```bash
#!/usr/bin/env bash
# Measure idle CPU% for Voxlink client and server across five scenarios.
# Prints a markdown table to stdout. Run after `cargo build --release`.
#
# Scenarios:
#   server_zero_peers     — server, no clients connected
#   server_one_idle_peer  — server, one client connected and silent
#   client_home           — client on home view, not connected
#   client_joined_silent  — client joined to a room, mic muted
#   client_minimized      — same as joined_silent with window hidden
#
# Usage: ./scripts/measure-idle-cpu.sh
#
# Requirements:
#   - macOS (uses `top -pid ...`). Linux equivalents TBD.
#   - Release binaries built: target/release/signaling_server, target/release/app_desktop
#   - Free loopback port 19090.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

SERVER_BIN=target/release/signaling_server
CLIENT_BIN=target/release/app_desktop
PORT=19090
SAMPLE_DURATION=30
SAMPLES_PER_SCENARIO=3

if [ ! -x "$SERVER_BIN" ]; then
    echo "Missing $SERVER_BIN — run: cargo build --release -p signaling_server" >&2
    exit 1
fi
if [ ! -x "$CLIENT_BIN" ]; then
    echo "Missing $CLIENT_BIN — run: cargo build --release -p app_desktop" >&2
    exit 1
fi

TMPDIR=$(mktemp -d)
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$TMPDIR"' EXIT

# Sample CPU% for PID over SAMPLE_DURATION seconds. Outputs one floating-point
# number: the mean CPU% across the sampling window.
sample_cpu() {
    local pid=$1
    # top -l <count> prints <count> snapshots, each a second apart.
    # We skip the first (warmup) snapshot and average the rest.
    top -pid "$pid" -l "$((SAMPLE_DURATION + 1))" -stats cpu 2>/dev/null \
        | awk 'NR > 2 && /^[0-9.]+$/ {sum+=$1; n++} END {if (n > 0) printf "%.1f", sum/n; else print "NaN"}'
}

# Take SAMPLES_PER_SCENARIO measurements, print the median.
median_cpu() {
    local pid=$1
    local values=()
    for _ in $(seq 1 $SAMPLES_PER_SCENARIO); do
        values+=("$(sample_cpu "$pid")")
    done
    printf "%s\n" "${values[@]}" | sort -n | awk -v n=$SAMPLES_PER_SCENARIO 'NR == (n+1)/2'
}

echo "Voxlink idle CPU baselines — $(date '+%Y-%m-%d %H:%M %Z')"
echo "Machine: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m)"
echo "Rust:    $(rustc --version 2>/dev/null | head -1)"
echo
echo "| Scenario | CPU % (median of $SAMPLES_PER_SCENARIO × ${SAMPLE_DURATION}s) |"
echo "|---|--:|"

# --- server_zero_peers ---
PV_ADDR=127.0.0.1:$PORT "$SERVER_BIN" > "$TMPDIR/server.log" 2>&1 &
SERVER_PID=$!
sleep 2

CPU=$(median_cpu "$SERVER_PID")
printf "| server_zero_peers | %s |\n" "$CPU"

# --- server_one_idle_peer ---
# Spawn client1, wait for it to connect, then sample the server.
VOXLINK_SERVER=ws://127.0.0.1:$PORT "$CLIENT_BIN" > "$TMPDIR/client1.log" 2>&1 &
CLIENT1_PID=$!
sleep 5   # give the client time to connect + settle

CPU=$(median_cpu "$SERVER_PID")
printf "| server_one_idle_peer | %s |\n" "$CPU"

# Client1 is already running, measure its idle behaviour.
# --- client_joined_silent (client1 is already in "home" state, needs manual join for realism) ---
# NOTE: This scenario requires the user to click "join room" manually.
# For automation, we settle for "client_home" here.
CPU=$(median_cpu "$CLIENT1_PID")
printf "| client_home | %s |\n" "$CPU"

# The two remaining scenarios (client_joined_silent, client_minimized) require
# UI interaction. If automation is desired later, wire up slint's ui_visibility
# test harness or accessibility APIs. For M6, record them manually (see
# docs/PERFORMANCE_TARGETS.md — the script records what it can).
echo "| client_joined_silent | (manual — record separately) |"
echo "| client_minimized | (manual — record separately) |"

# Cleanup happens via trap.
```

- [ ] **Step 2: Make executable + syntax check**

```
chmod +x scripts/measure-idle-cpu.sh
bash -n scripts/measure-idle-cpu.sh
```
Expected: no syntax errors.

- [ ] **Step 3: Run the script and save the output**

```
cd /Users/jph/Voiceapp/workspace_template
./scripts/measure-idle-cpu.sh | tee /tmp/m6-baseline-idle.md
```
Expected: script runs ~3 × 3 scenarios × 30 s ≈ 5 minutes total wall-clock. Output is a markdown table with three automated rows and two "manual" placeholders.

If the script fails partway through (server fails to start, for example), check `/tmp/voxlink-*/server.log`. Most likely cause: port 19090 already in use — change `PORT` in the script.

- [ ] **Step 4: Record the two manual scenarios**

The script's `client_home` row is automatic. The two UI-dependent scenarios need human interaction.

For each of `client_joined_silent` and `client_minimized`:

1. Start the client with `VOXLINK_SERVER=ws://127.0.0.1:19090 target/release/app_desktop` (after starting `target/release/signaling_server` in another terminal).
2. Drive the UI into the target state (join a room and mute; or minimize the window).
3. In a third terminal, find the client PID: `pgrep -x app_desktop`.
4. Sample: `top -pid <PID> -l 31 -stats cpu | awk 'NR > 2 && /^[0-9.]+$/ {sum+=$1; n++} END {if (n > 0) printf "%.1f\n", sum/n}'`.
5. Repeat 3× for median.

Record the medians in the `/tmp/m6-baseline-idle.md` file, replacing the `(manual — record separately)` cells.

- [ ] **Step 5: Sanity-check numbers**

Client idle CPU% of >5% on a quiet M4 Pro is surprising for a "low-overhead" app and signals real waste. <1% is the rough target. Record whatever you measure, without judgment — the audit finds the why.

- [ ] **Step 6: Commit script + baseline numbers**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add scripts/measure-idle-cpu.sh
git commit -m "scripts: add measure-idle-cpu.sh for M6 baseline"
```

Note: we don't commit `/tmp/m6-baseline-idle.md` — it gets embedded in `docs/PERFORMANCE_TARGETS.md` in Task 4.

---

## Task 2: Audit — `docs/IDLE_AUDIT.md`

**Files:**
- Create: `docs/IDLE_AUDIT.md`

**What this adds:** A one-time audit of every periodic task in the client and server crates, with a triage decision per task. This is the roadmap for Phase 3 fixes.

- [ ] **Step 1: Grep for periodic work across both crates**

```
cd /Users/jph/Voiceapp/workspace_template
rg -n '(tokio::spawn|thread::spawn|slint::Timer|set_interval|interval\(|Duration::from_(secs|millis))' \
    crates/app_desktop/src crates/signaling_server/src \
    > /tmp/m6-audit-raw.txt
wc -l /tmp/m6-audit-raw.txt
```
Expected: several dozen lines. Skim the output. Not every `Duration::from_secs(...)` is a periodic task — some are timeouts, some are one-shot `tokio::time::sleep`s. You'll triage in the next step.

- [ ] **Step 2: Build the audit table**

For each hit that is actually a periodic task (a timer, an interval, a loop with a sleep), add a row to `docs/IDLE_AUDIT.md`. Skip one-shot timeouts and debounces.

Template:

```markdown
# Voxlink Idle Audit (M6)

Date: 2026-04-19
Scope: every periodic task in `app_desktop` and `signaling_server`.

## Triage legend

- **KEEP** — task is correctly throttled and justified.
- **THROTTLE** — reduce frequency when idle or window hidden.
- **GATE** — suspend entirely when a precondition is false.
- **REPLACE** — convert polling to event-driven.
- **KILL** — task has no reason to exist; delete.

## Client (`crates/app_desktop`)

| File:line | Task | Frequency | Purpose | Idle cost | Action |
|---|---|---|---|---|---|
| <file:line> | <what it does> | <how often> | <why> | <rough CPU or allocs/s on idle path> | <KEEP/THROTTLE/GATE/REPLACE/KILL> |
| ... | | | | | |

## Server (`crates/signaling_server`)

| File:line | Task | Frequency | Purpose | Idle cost | Action |
|---|---|---|---|---|---|
| ... | | | | | |
```

Fill in one row per periodic task. For each:
- **File:line** — from the grep output.
- **Task** — a short label (e.g., "main tick loop", "tray animation", "auth-attempts pruner").
- **Frequency** — from the source (e.g., "40 Hz / 10 Hz idle", "every 30 s").
- **Purpose** — from surrounding code context.
- **Idle cost** — qualitative: "high" if it does heavy work, "medium" if it wakes the thread but does little, "low" if it's cheap.
- **Action** — your triage decision.

**Tips for triage:**
- A timer that always wakes but only does work if a flag is set → consider GATE: don't wake at all when flag is clear.
- A timer that does work regardless of window visibility → GATE on window-visible.
- A once-per-second scan of a HashMap to prune stale entries → REPLACE with TTL-on-read or a bounded ring buffer.
- A tokio::spawn that sleeps forever inside → suspicious; check if it does any work.

- [ ] **Step 3: Identify top 3 actions**

At the bottom of `IDLE_AUDIT.md`, add:

```markdown
## Top actions for this milestone

Ranked by estimated idle-CPU impact:

1. <task name> — <action> — <rationale>
2. <task name> — <action> — <rationale>
3. <task name> — <action> — <rationale>

If more than three findings have similar impact, they land in follow-up commits.
If the top action is estimated <5% idle CPU improvement, M6 closes as "verified efficient."
```

- [ ] **Step 4: Commit the audit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add docs/IDLE_AUDIT.md
git commit -m "docs: idle periodic-work audit (M6 Phase 2)"
```

---

## Task 3: Apply audit fixes (one commit per fix)

**What this is:** Phase 3 of the spec. The specific fixes depend on what Task 2 found. This task is a *template* — repeat it for each "Top action" from the audit.

**General procedure per fix:**

- [ ] **Step 1: Identify the fix target**

Pick the next unaddressed action from the "Top actions" list in `docs/IDLE_AUDIT.md`.

- [ ] **Step 2: Make the code change**

Exact code depends on the action. Common patterns:

**Pattern A — GATE on window visibility.** If a timer should suspend when the window is minimized:

```rust
// Check window visibility via Slint's API. Depending on the Slint version,
// either poll `window.window_handle()`'s winit-level state or expose a
// property `window-visible` from .slint and read it here.
//
// Simplest reliable approach: add a `pub fn set_idle(&self, idle: bool)`
// that the Slint window_event handler calls, and the tick loop checks
// `idle.load(Relaxed)` before doing optional work.

if window_is_visible.load(Ordering::Relaxed) {
    // only do the work when visible
    refresh_unread_badges(...);
}
```

**Pattern B — THROTTLE: do the work every Nth tick instead of every tick.**

```rust
if tick % 10 == 0 {  // once every 10 ticks instead of every tick
    update_bandwidth_display(...);
}
```

**Pattern C — REPLACE polling with event.** If code has `interval(1 second)` just to check a HashMap:

```rust
// Before:
// tokio::spawn(async {
//     let mut tick = tokio::time::interval(Duration::from_secs(1));
//     loop {
//         tick.tick().await;
//         prune_stale_entries(&map).await;
//     }
// });
//
// After: prune inline at the insertion point.
async fn insert_with_prune(map: &Mutex<HashMap<K, (V, Instant)>>, k: K, v: V) {
    let mut m = map.lock().await;
    let now = Instant::now();
    m.retain(|_, (_, inserted_at)| now.duration_since(*inserted_at) < TTL);
    m.insert(k, (v, now));
}
```

**Pattern D — KILL.** Delete the code. Delete the imports it pulled in. `cargo check` and fix.

Pick the pattern that matches the audit's action for this fix.

- [ ] **Step 3: Re-measure**

```
cd /Users/jph/Voiceapp/workspace_template
cargo build --release -p signaling_server -p app_desktop
./scripts/measure-idle-cpu.sh | tee /tmp/m6-after-fix-N.md
```

Compare to the prior measurement. If this fix is effective, at least one scenario's CPU% drops.

- [ ] **Step 4: Manual smoke test**

1. `target/release/signaling_server &`
2. `VOXLINK_SERVER=ws://127.0.0.1:9090 target/release/app_desktop`
3. In the client: create or join a test room.
4. Send audio for 5 seconds.
5. Leave the room.
6. Confirm no errors in console, no audio glitches, UI responsive.

If anything breaks, revert the change: `git checkout -- .`.

- [ ] **Step 5: Verify workspace clean**

```
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: check clean; clippy ≤ 62.

- [ ] **Step 6: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add -A    # limit scope to files actually touched; if others, unstage
git commit -m "perf(<crate>): <short description of the fix>"
```

Commit message examples:
- `perf(client): pause UI timers when window is minimized`
- `perf(client): throttle unread-badge refresh to 1 Hz`
- `perf(server): prune auth-attempts on insert rather than per-second scan`
- `perf(client): eliminate dead per-channel activity timer`

- [ ] **Step 7: Update `docs/IDLE_AUDIT.md`**

In the "Top actions" list, strike through or mark the completed action with its commit SHA:

```markdown
1. ~~tray-animation timer — GATE on window-visible — runs at 4 Hz even when minimized~~ ✓ <commit-sha>
```

Commit the doc update in the same commit as the fix (include it in `git add -A`).

---

**Repeat Task 3 for each action in the Top-3 list** (or more, if you find easy wins). Expected: 0–5 fix commits.

---

## Task 4: Re-measure + record final numbers

**Files:**
- Modify: `docs/PERFORMANCE_TARGETS.md`

**What this adds:** Updates `PERFORMANCE_TARGETS.md` with a new "Idle CPU baselines" section showing before/after numbers for each scenario.

- [ ] **Step 1: Take the final measurement**

```
cd /Users/jph/Voiceapp/workspace_template
cargo build --release -p signaling_server -p app_desktop
./scripts/measure-idle-cpu.sh | tee /tmp/m6-final-idle.md
```

And the two manual scenarios (as in Task 1 Step 4).

- [ ] **Step 2: Append "Idle CPU baselines" section to `docs/PERFORMANCE_TARGETS.md`**

Open `docs/PERFORMANCE_TARGETS.md`. After the existing "Microbenchmark baselines" section (and its subsections), append:

```markdown
---

## Idle CPU baselines

Measured on Apple M4 Pro, release build, before + after M6 (2026-04-19).

| Scenario | Before | After | Δ |
|---|--:|--:|--:|
| `client_home` | `X.X %` | `Y.Y %` | `−Z.Z %` |
| `client_joined_silent` | `X.X %` | `Y.Y %` | `−Z.Z %` |
| `client_minimized` | `X.X %` | `Y.Y %` | `−Z.Z %` |
| `server_zero_peers` | `X.X %` | `Y.Y %` | `−Z.Z %` |
| `server_one_idle_peer` | `X.X %` | `Y.Y %` | `−Z.Z %` |

See `docs/IDLE_AUDIT.md` for the full audit trail.
Reproduce: `./scripts/measure-idle-cpu.sh` (two scenarios require manual UI steps — see the script header).
```

Replace `X.X` with the before-numbers from `/tmp/m6-baseline-idle.md`, `Y.Y` with the after-numbers from `/tmp/m6-final-idle.md`, and compute `Δ` as `Y.Y - X.X` (negative means improvement).

No placeholders should remain in the committed file — every `X.X`, `Y.Y`, `Z.Z` must be a real number.

- [ ] **Step 3: Verify no placeholders**

```
grep -E "X\.X|Y\.Y|Z\.Z" docs/PERFORMANCE_TARGETS.md
```
Expected: no output (all replaced).

- [ ] **Step 4: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add docs/PERFORMANCE_TARGETS.md
git commit -m "docs: record post-M6 idle CPU baselines"
```

---

## Task 5: Final verify + merge

- [ ] **Step 1: Full workspace build**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
```
Expected: clean.

- [ ] **Step 2: Clippy**

```
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: ≤ 62.

- [ ] **Step 3: Run non-flaky tests**

```
cargo test --workspace --no-fail-fast -- \
  --skip live_stress \
  --skip test_create_space \
  --skip test_audio_after_leave_room \
  --skip test_channel_audio_relay \
  --skip test_authenticate_invalid_token_creates_new \
  2>&1 | awk '/test result:/ {ok+=$4; fail+=$6} END {print "passed="ok, "failed="fail}'
```
Expected: `failed=0`.

- [ ] **Step 4: Run bench-check to confirm no microbench regressions**

```
./scripts/bench-check.sh
```
Expected: exits 0. If it reports a regression, the M6 fix accidentally slowed a hot path — revert that specific commit and re-approach the fix differently.

- [ ] **Step 5: Commit manifest**

```
git log --oneline main..HEAD
```
Expected: a sequence of commits: measure script, audit doc, N fix commits, final-numbers commit.

- [ ] **Step 6: Merge to main**

```
git checkout main
git merge --ff-only feat/m6-idle-audit
git branch -d feat/m6-idle-audit
```

---

# Completion criteria

All of:

1. `cargo check --workspace` clean; clippy ≤ 62.
2. All non-flaky tests pass.
3. `scripts/bench-check.sh` exits 0 (no microbench regressions).
4. `scripts/measure-idle-cpu.sh` runs and produces five scenario rows.
5. `docs/IDLE_AUDIT.md` exists with a triage decision per periodic task.
6. `docs/PERFORMANCE_TARGETS.md` has an "Idle CPU baselines" section with real before/after numbers.
7. **Either** ≥ 30 % drop in at least one scenario **or** a documented statement that the app was already at the efficient floor.
8. Manual smoke test passes after the final commit.

# If something goes wrong

- **`top -pid` returns no CPU samples**: the process died before sampling started. Increase the pre-sample `sleep 2` to `sleep 5`.
- **`measure-idle-cpu.sh` hangs waiting for client to be killable**: the `trap` cleanup should handle it; if it doesn't, `pkill -f app_desktop` manually.
- **Audit table produces no findings**: the grep was too narrow. Also search for `tokio::select!`, `loop {`, `join_set`.
- **A fix drops CPU but breaks audio**: revert the commit. The fix pattern was wrong; re-approach with a narrower change (e.g., GATE instead of KILL).
- **Microbench regression after a fix**: one of the inlined changes hurt a hot path. Revert, re-approach.
- **Can't find a window-visibility hook in Slint**: fall back to tracking focus state via an application flag set by existing user-activity detection, or use `std::sync::atomic::AtomicBool` shared from the tray hide/show handler if one exists.

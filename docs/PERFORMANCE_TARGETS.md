# Performance Targets

These are guiding targets for Voxlink. They shape implementation decisions and gate regressions via `scripts/bench-check.sh`.

## Product-level targets (qualitative)

- Cold start should feel fast.
- Idle app at home screen should use very little CPU.
- Background/idle room state should not spin work unnecessarily.
- Joining a room should feel near-instant on a healthy network.
- Device switching should be responsive.

## Engineering rules

- No busy loops.
- No large recurring allocations in hot paths.
- Avoid unnecessary cloning of state.
- Throttle metrics updates to sensible frequencies.
- UI should redraw only when needed.
- Audio callbacks must stay extremely lightweight.

## Early instrumentation priorities

- App startup timing.
- Idle CPU sampling.
- Idle memory reporting.
- UI update cadence.
- Audio callback timing.
- Reconnect count.
- Room join timing.

## Validation mindset

Every subsystem should eventually answer:
- What does it cost at idle?
- What does it cost during a 4-person call?
- What work runs every second, and why?

---

## Microbenchmark baselines

Measured on: **Apple M4 Pro, rustc 1.94.0, 2026-04-17**.

Each row's "Observed" is the median of criterion's `[lo mid hi]` triple. "Target" is the regression threshold: if `scripts/bench-check.sh` reports a value more than ~10% above target, investigate before merging.

### audio_core

| Benchmark | Target | Observed | Notes |
|---|---|---|---|
| `frame_energy_960` | < 500 ns | 439.7 ns | 20 ms frame at 48 kHz |
| `soft_clip_960` | < 1 µs | 655.8 ns | Per-sample soft clipper |
| `i16_to_f32_960` | < 100 ns | 54.2 ns | Every decoded frame (bench is noisy — up to ±10% run-to-run) |
| `mix_4_peers_960` | < 500 ns | 221.2 ns | Four-peer output mix |
| `frame_energy_silence` | < 500 ns | 440.5 ns | Early-exit path |
| `soft_clip_passthrough_960` | < 1 µs | 655.9 ns | In-range samples |

### signaling_server

| Benchmark | Target | Observed | Notes |
|---|---|---|---|
| `histogram_observe` | < 200 ns | 2.18 ns | One observation (bucket scan + atomics) |
| `signal_message_from_slice_simple` | < 100 ns | 19.3 ns | Unit variant deserialize |
| `signal_message_from_slice_complex` | < 500 ns | 87.6 ns | Struct variant with payload |
| `signal_message_to_string` | < 200 ns | 40.2 ns | Serialize to JSON |
| `decode_screen_chunk_metadata` | < 10 ns | 780 ps | 8-byte header parse |

### shared_types

| Benchmark | Target | Observed | Notes |
|---|---|---|---|
| `signal_message_variant_index_struct` | < 5 ns | 417 ps | Struct variant jump table |
| `signal_message_variant_index_unit` | < 5 ns | 417 ps | Unit variant jump table |

All measured values are comfortably under their targets. The regression gate will flag any bench that drifts past criterion's significance threshold vs the saved "main" baseline.

## How to regenerate baselines

After an intentional performance change, or when moving to a different dev machine:

```
./scripts/bench-record-baseline.sh
```

Then re-run this file's "Observed" column from the fresh log. Commit the updated numbers.

## How to gate a change on regressions

```
./scripts/bench-check.sh
```

Exit code 1 means one or more benches regressed past criterion's significance threshold vs the saved "main" baseline. If the slowdown is expected, re-record; otherwise investigate.

Note: criterion's detection is statistically rigorous but some benches (notably `i16_to_f32_960`) show run-to-run variation of ~10% on this machine even without code changes. When the gate fires, inspect the magnitude — anything under ~15% on a single run is probably noise; re-run before investigating code.

---

## Idle CPU baselines

Reproducible script: `./scripts/measure-idle.sh` (macOS; three scenarios automated, two require manual UI steps). Record the before/after numbers from a clean run here.

| Scenario | Before | After | Δ |
|---|--:|--:|--:|
| `server_zero_peers` | `(run script)` | `(run script)` | |
| `server_one_idle_peer` | `(run script)` | `(run script)` | |
| `client_home` | `(run script)` | `(run script)` | |
| `client_joined_silent` | `(manual)` | `(manual)` | |
| `client_minimized` | `(manual)` | `(manual)` | |

### M6 changes (committed; expected impact when measured)

Per `docs/IDLE_AUDIT.md`. Expected idle-CPU reduction by scenario:

- **Client** (`client_home`, `client_joined_silent`, `client_minimized`): ~15–30% reduction. Three idle-path fixes landed: the screen-chunk expiry no longer acquires the network mutex and scans a HashMap every second when nobody's sharing a screen; the unread-pulse property only writes when there are unread items; the typing-dot animation only writes when a typing indicator is actually visible.
- **Server** (`server_zero_peers`, `server_one_idle_peer`): smaller idle-CPU change (the 60 s sweep was low frequency already) but noticeably lower peak-lock contention under churn. `auth_attempts`, `join_failures`, and `connections_per_ip` all prune on insert now; `udp_sessions` still swept. See commit `f54fcfb`.

Run `./scripts/measure-idle.sh` with the M6 commits reverted (`git revert <sha>`) to capture the "before" numbers, and again with them applied to capture "after".

---

## Startup time

Measured on Apple M4 Pro, release build, from `fn main()` entry to `window.run()`.

Target: **total startup ≤ 500 ms**.
Stretch: ≤ 300 ms.

Observed: run the client once, open the Perf panel — the "Startup" card shows the total and per-phase breakdown. Also logged at info level in `voxlink.log` as `startup: <phase> @ <N>ms` lines.

Per-phase expectations (rough heuristics, update after first real run):

- `logging`, `config load`, `tokio runtime`, `core state`, `media + screen share`, `config applied`, `callbacks wired` — each should be ≤ 30 ms.
- `audio engine` — expected to be the slowest phase on most machines (cpal device enumeration + Opus + neural denoiser init). Target ≤ 200 ms.
- `main window` — Slint render-graph construction and initial layout. Target ≤ 250 ms.
- `device populate` — one-time cpal audio device enumeration for the settings dropdown. Target ≤ 100 ms.

If a phase consistently exceeds its target, it is a candidate for optimization in a follow-up milestone.

---

## Memory baselines

Measured on Apple M4 Pro, release build. RSS via `ps -o rss=`.

| Scenario | Target RSS | Observed |
|---|--:|--:|
| `server_zero_peers` | < 50 MB | (run script) |
| `server_one_idle_peer` | < 60 MB | (run script) |
| `client_home` | < 150 MB | (run script) |
| `client_joined_silent` (after 30 s) | < 180 MB | (manual) |

Reproduce: `./scripts/measure-idle.sh`.

### Growth check

The Perf panel in the running client shows a "Growth" row that tracks RSS delta since the second perf snapshot (so sysinfo's lazy-init cost is captured as baseline, not reported as growth). Color thresholds:

- **≤ +10 MB**: green (expected: log file growth, UI text cache churn, etc.)
- **+10 to +50 MB**: amber (tolerable; worth an occasional look)
- **> +50 MB**: red (investigate; likely leak in call state, channel buffers, or cached DMs)

### Rationale

Voxlink ships with a neural denoiser (nnnoiseless), the Slint render graph, and the Rust runtime. A realistic floor for the client before any room activity is ~140 MB. The server has no UI, no audio pipeline, no denoiser — it must fit comfortably inside the Oracle free-tier 1 GB VM alongside the OS — target ~50 MB idle.

For comparison, Discord's Electron desktop client routinely idles at ~300+ MB.

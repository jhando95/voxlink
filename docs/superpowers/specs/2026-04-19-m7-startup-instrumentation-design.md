# Design — Milestone 7: Startup Time Instrumentation

**Date:** 2026-04-19
**Status:** Approved (pending spec review)
**Scope:** Instrumentation only. Measure every major phase of `app_desktop`'s startup sequence, log the per-phase elapsed time, display the breakdown in the Perf panel, document a target in `PERFORMANCE_TARGETS.md`. Optimization of the slowest phase is explicitly a follow-up milestone driven by real observed numbers.

## Context

CLAUDE.md core identity says "fast-launching" is part of the product DNA. There's no measurement infrastructure for this today — `main.rs` already has a single `startup_t0 = Instant::now()` and one log line that reports elapsed time after config load, but nothing more. Users have no way to see where startup time goes, and the project has no target or regression signal.

M6 proved the audit→fix→document pattern works without code sprawl. M7 applies the same discipline: instrument first, commit the data collection, then make an informed decision about whether to optimize at all.

## Goals

1. One visible timing log line per major startup phase.
2. Phase breakdown surfaced in the existing Perf panel so the user sees where their launch time goes without digging in log files.
3. Documented target for total startup time, reproducible from the Perf panel or log.
4. Zero runtime-cost overhead outside startup (no ongoing timer cost after `window.run()`).

## Non-goals

- **Startup optimization.** If the data exposes a slow phase, that's a follow-up milestone. Don't optimize speculatively.
- **Server startup instrumentation.** Server starts in <100 ms, not user-facing, not in the identity statement.
- **Cold-start-from-power-on measurement.** OS-level concern, outside the process.
- **Warm-start / reload timing.** Different instrumentation path; defer.
- **Tracing framework** (`tracing`, `tokio-console`, etc.). Adding a dependency for 11 phase labels is overkill. Plain `Instant::now()` + log is sufficient.

## Architecture

Three pieces, all in the client crate.

### 1. `StartupTimer` helper

New file `crates/app_desktop/src/startup_timer.rs`, ~40 lines:

```rust
use std::time::Instant;

/// Lightweight startup-phase recorder.
///
/// Call `phase()` at each major seam in startup. Each call logs at info
/// level and appends to an internal Vec. The final list is handed to the
/// UI layer for display in the Perf panel, then the timer is dropped.
///
/// Cost at runtime: one `Instant::now()` per phase call (cheap) plus one
/// `log::info!` (gated by the logger). Total overhead is negligible vs the
/// phases themselves.
pub struct StartupTimer {
    start: Instant,
    phases: Vec<(String, u32)>,
}

impl StartupTimer {
    pub fn new() -> Self {
        Self { start: Instant::now(), phases: Vec::with_capacity(16) }
    }

    /// Record the current elapsed time against `name` and log it.
    pub fn phase(&mut self, name: &str) {
        let ms = self.start.elapsed().as_millis() as u32;
        log::info!("startup: {name} @ {ms}ms");
        self.phases.push((name.to_string(), ms));
    }

    /// Consume the timer and return the recorded phases.
    pub fn into_phases(self) -> Vec<(String, u32)> { self.phases }

    pub fn total_ms(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }
}

impl Default for StartupTimer {
    fn default() -> Self { Self::new() }
}
```

### 2. Eleven phase markers in `main.rs`

Insert `timer.phase(...)` calls at the natural seams, roughly in this order:

1. `"logging"` — after `setup_logging()` and crash reporter install.
2. `"config load"` — after `config_store::load_config()`.
3. `"tokio runtime"` — after runtime built.
4. `"core state"` — after perf / voice / app state / network client constructed.
5. `"audio engine"` — after `AudioEngine::new()` returns. Likely the slowest phase.
6. `"audio config applied"` — after the `rt.block_on` that sets noise suppression, loads soundboard clips.
7. `"media + screen share"` — after `MediaSession::new` and `ScreenShareController::new`.
8. `"main window"` — after `MainWindow::new()`. Slint compile + initial render.
9. `"device populate"` — after `populate_devices()`.
10. `"config applied to UI"` — after `apply_config()`.
11. `"callbacks wired"` — right before calling `window.run()`.

Each marker is a single-line `timer.phase("…")` added inline. The final `into_phases()` is consumed right before `window.run()` and pushed into the UI via a new Slint property (below).

### 3. Perf-panel display

The Perf panel already exists (view index 3, in the existing Slint UI). Add two Slint properties on `MainWindow`:

- `startup-total-ms: int` — total time before `window.run()`.
- `startup-phases: [{name: string, ms: int}]` — ordered list of phase markers.

In the Perf view template, render them as a small table:

```
Startup (total 612 ms)
┌─────────────────────┬───────┐
│ logging             │   3 ms│
│ config load         │   8 ms│
│ tokio runtime       │  15 ms│
│ core state          │  19 ms│
│ audio engine        │ 183 ms│
│ audio config        │ 198 ms│
│ media+screenshare   │ 201 ms│
│ main window         │ 446 ms│
│ device populate     │ 520 ms│
│ config → UI         │ 585 ms│
│ callbacks wired     │ 609 ms│
└─────────────────────┴───────┘
```

Column 2 shows cumulative elapsed at each phase. A reader can eyeball the biggest gap between rows to spot the expensive phase.

### 4. Documented target

Add a "Startup time" section to `docs/PERFORMANCE_TARGETS.md`:

```markdown
## Startup time

Measured on Apple M4 Pro, release build.

Target: total startup ≤ 500 ms from `fn main()` entry to `window.run()`.
Stretch: ≤ 300 ms.

Observed on a fresh run: (to be filled from Perf panel after first run)

Per-phase breakdown: see the Perf panel in the running app, or `startup:` lines in the log file.
```

Numbers get filled in by the operator after running the app once.

## Components

| File | Change |
|---|---|
| `crates/app_desktop/src/startup_timer.rs` *(new)* | `StartupTimer` struct + unit tests |
| `crates/app_desktop/src/main.rs` | Declare `mod startup_timer;`, create `timer`, add 11 `timer.phase(...)` calls, push result into window |
| `crates/ui_shell/ui/main.slint` | Add `startup-total-ms: int` and `startup-phases: [{name: string, ms: int}]` properties |
| `crates/ui_shell/ui/views/system_view.slint` *(or wherever perf view renders)* | Render the phases table |
| `docs/PERFORMANCE_TARGETS.md` | Add "Startup time" section |

No changes to runtime behavior. No new deps.

## Risks

- **Slint property limits.** Slint 1.15's model of an array-of-structs may require defining a `struct StartupPhase { name, ms }` in `main.slint`. Mitigation: declare the struct; it's one pattern the codebase already uses (`AutomodWord`, `ScheduledEvent`).
- **Phase markers shift as future changes reorder startup.** Unavoidable and acceptable — the timer design doesn't assume a fixed phase set.
- **Log volume increase.** 11 info-level lines on every startup. Negligible vs the existing startup logging footprint; no action needed.

## Commit strategy

1. `feat(client): add StartupTimer helper + unit tests`
2. `feat(client): instrument startup phases in main.rs`
3. `feat(ui): surface startup phase breakdown in Perf panel`
4. `docs: startup time target in PERFORMANCE_TARGETS.md`

Four commits, each leaves the workspace green.

## Success criteria

1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets` ≤ 62 warnings.
3. All existing tests pass; new `StartupTimer` unit tests pass.
4. Running the client writes `startup: <phase> @ <N>ms` lines for each of the 11 phases to the log.
5. The Perf panel shows a table with all 11 phases and a total.
6. `docs/PERFORMANCE_TARGETS.md` has a "Startup time" section with a numeric target.

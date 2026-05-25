# M7 — Startup Time Instrumentation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Instrument 11 startup phases in `app_desktop`, surface them in the Perf panel, document a target. Zero optimization in this milestone.

**Architecture:** A small `StartupTimer` in `app_desktop` records `(phase_name, cumulative_ms)` at each seam in `main.rs` and logs at info level. Right before `window.run()`, the recorded phases are copied into a new Slint property (`startup-phases: [StartupPhaseData]`) on `MainWindow`. The Perf panel (`views/system_view.slint`) renders them as a table.

**Tech Stack:** Rust 1.94, Slint 1.15. No new deps.

**Spec:** `docs/superpowers/specs/2026-04-19-m7-startup-instrumentation-design.md`

**Workspace root:** `/Users/jph/Voiceapp/workspace_template`

**Branch:** start on `feat/m7-startup-instrumentation` from `main`.

---

## Ground rules

1. **Workspace stays green.** `cargo check --workspace` passes before each commit.
2. **No new clippy warnings.** Baseline is 62; must not exceed.
3. **No runtime-behavior changes.** The `StartupTimer` only records elapsed times and logs; it does not alter existing startup behavior.
4. **Drop-and-forget.** After `window.run()` launches, the `StartupTimer` is consumed; it has zero ongoing cost during the rest of the app's life.
5. **Preserve existing log line.** `main.rs` currently has a `log::info!("Config loaded ({}ms)...")`. Keep it (redundant with the new phase log, but removing it is out of scope).

---

## Task 0: Branch + baseline

- [ ] **Step 1: Verify clean working tree**

```
cd /Users/jph/Voiceapp/workspace_template && git status --short
```
Expected: empty. Commit or stash any modifications first.

- [ ] **Step 2: Create feature branch**

```
git checkout -b feat/m7-startup-instrumentation
```

- [ ] **Step 3: Verify baseline**

```
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: check clean, clippy `62`.

No commit yet.

---

## Task 1: `StartupTimer` helper

**Files:**
- Create: `crates/app_desktop/src/startup_timer.rs`

**What this adds:** A 40-line helper struct that records `(name, elapsed_ms)` at each call, logs at info level, and can be consumed to produce the phase list. Unit tests cover monotonicity and consumption.

- [ ] **Step 1: Create `crates/app_desktop/src/startup_timer.rs`**

```rust
//! Lightweight startup-phase recorder.
//!
//! Call `phase()` at each major seam in startup. Each call logs at info
//! level and appends to an internal `Vec`. The final list is handed to
//! the UI layer for display in the Perf panel, then the timer is dropped.
//!
//! Runtime cost: one `Instant::now()` call + one `log::info!` per phase.
//! Negligible compared to the phase work itself, and zero overhead after
//! the timer is dropped (right before `window.run()`).

use std::time::Instant;

pub struct StartupTimer {
    start: Instant,
    phases: Vec<(String, u32)>,
}

impl StartupTimer {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            phases: Vec::with_capacity(16),
        }
    }

    /// Record cumulative elapsed time with a label and log it.
    pub fn phase(&mut self, name: &str) {
        let ms = self.start.elapsed().as_millis() as u32;
        log::info!("startup: {name} @ {ms}ms");
        self.phases.push((name.to_string(), ms));
    }

    /// Consume the timer and return the recorded phases in order.
    pub fn into_phases(self) -> Vec<(String, u32)> {
        self.phases
    }

    /// Current total elapsed time in milliseconds.
    pub fn total_ms(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }
}

impl Default for StartupTimer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_recorded_in_order() {
        let mut t = StartupTimer::new();
        t.phase("a");
        std::thread::sleep(std::time::Duration::from_millis(5));
        t.phase("b");
        let phases = t.into_phases();
        assert_eq!(phases.len(), 2);
        assert_eq!(phases[0].0, "a");
        assert_eq!(phases[1].0, "b");
        // "b" must be recorded at a time >= "a".
        assert!(phases[1].1 >= phases[0].1);
    }

    #[test]
    fn total_ms_monotonic() {
        let t = StartupTimer::new();
        let m1 = t.total_ms();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let m2 = t.total_ms();
        assert!(m2 >= m1);
    }

    #[test]
    fn default_constructor_works() {
        let t = StartupTimer::default();
        // If this compiles and runs, the Default impl is wired correctly.
        let _ = t.into_phases();
    }
}
```

- [ ] **Step 2: Verify the file compiles on its own by adding it to the module tree**

Open `crates/app_desktop/src/main.rs`. Find the existing `mod` declarations near the top of the file (e.g., `mod automation;`, `mod helpers;`, etc.). Add:

```rust
mod startup_timer;
```

Keep it alphabetically-placed if the existing mods follow that pattern; otherwise put it near the other `mod` declarations.

- [ ] **Step 3: Verify and run the unit tests**

```
cd /Users/jph/Voiceapp/workspace_template
cargo test -p app_desktop startup_timer::
```
Expected: 3 tests pass.

- [ ] **Step 4: Full workspace check**

```
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: clean, warnings ≤ 62.

- [ ] **Step 5: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add crates/app_desktop/src/startup_timer.rs crates/app_desktop/src/main.rs
git commit -m "feat(client): add StartupTimer helper for phase instrumentation"
```

---

## Task 2: `StartupPhaseData` Slint struct + MainWindow properties

**Files:**
- Modify: `crates/ui_shell/ui/theme.slint`
- Modify: `crates/ui_shell/ui/main.slint`

**What this adds:** Declare a Slint struct type for a single (name, ms) pair, add two properties on `MainWindow` — `startup-total-ms` and `startup-phases: [StartupPhaseData]`.

- [ ] **Step 1: Add the Slint struct to `theme.slint`**

Open `crates/ui_shell/ui/theme.slint`. All the other data struct definitions live at the top. Append a new struct below the existing list (after the last `export struct ...` block):

```slint
export struct StartupPhaseData {
    name: string,
    ms: int,
}
```

- [ ] **Step 2: Import the struct in `main.slint`**

Open `crates/ui_shell/ui/main.slint`. Find the imports that pull in theme structs (e.g., `import { VxTheme, ParticipantData, ... } from "theme.slint";` near the top). Add `StartupPhaseData` to that import list:

```slint
import { ..., StartupPhaseData } from "theme.slint";
```

(Use whatever form the existing imports use — there may already be one master import line; just add `StartupPhaseData` to it alphabetically.)

- [ ] **Step 3: Declare the two new properties on `MainWindow`**

Still in `main.slint`, inside the `MainWindow` component's property block (where other `in-out property <...>` declarations live), add:

```slint
in property <int> startup-total-ms: 0;
in property <[StartupPhaseData]> startup-phases: [];
```

Use `in` (not `in-out`) — these are set once by Rust, never written back from Slint.

- [ ] **Step 4: Pass them to SystemView**

Find the `SystemView { ... }` instantiation inside `main.slint` (the line matching `if root.current-view == 3 : SystemView {` near line 1202). Add two property bindings inside its brace block:

```slint
startup-total-ms: root.startup-total-ms;
startup-phases: root.startup-phases;
```

(Place them alongside the other property bindings on that component.)

- [ ] **Step 5: Verify the UI crate compiles**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check -p ui_shell
```
Expected: clean. Slint-compile errors typically report a specific line; read and fix.

- [ ] **Step 6: Commit**

```bash
git add crates/ui_shell/ui/theme.slint crates/ui_shell/ui/main.slint
git commit -m "feat(ui): add StartupPhaseData struct + MainWindow properties for startup metrics"
```

---

## Task 3: Render the startup-phases table in SystemView

**Files:**
- Modify: `crates/ui_shell/ui/views/system_view.slint`

**What this adds:** SystemView receives the two new properties and renders a small table showing total startup time and per-phase elapsed values.

- [ ] **Step 1: Add the two input properties to `SystemView`**

Open `crates/ui_shell/ui/views/system_view.slint`. At the top where `in property <...>` declarations are listed, add:

```slint
in property <int> startup-total-ms: 0;
in property <[StartupPhaseData]> startup-phases: [];
```

Also add `StartupPhaseData` to the existing theme import at the top:

```slint
import { ..., StartupPhaseData } from "../theme.slint";
```

- [ ] **Step 2: Render a startup-phases card inside the existing layout**

Inside the main `VerticalLayout { ... }` in SystemView (look for the existing `VxCard` blocks). After the last existing card, add a new one:

```slint
            VxCard {
                VerticalLayout {
                    padding: 16px;
                    spacing: 8px;

                    Text {
                        text: "Startup  ·  total " + root.startup-total-ms + " ms";
                        font-size: 14px;
                        font-weight: 600;
                        color: VxTheme.text-primary;
                    }

                    if root.startup-phases.length == 0 : Text {
                        text: "(no startup data)";
                        font-size: 12px;
                        color: VxTheme.text-muted;
                    }

                    for phase in root.startup-phases : HorizontalLayout {
                        spacing: 12px;
                        Text {
                            text: phase.name;
                            font-size: 12px;
                            color: VxTheme.text-primary;
                            horizontal-stretch: 1;
                        }
                        Text {
                            text: phase.ms + " ms";
                            font-size: 12px;
                            color: VxTheme.text-muted;
                            horizontal-alignment: right;
                        }
                    }
                }
            }
```

If `VxTheme.text-muted` doesn't exist, substitute the closest color property that the theme defines (grep `text-` in `theme.slint`). Same for `text-primary`.

- [ ] **Step 3: Build the UI crate**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check -p ui_shell
```
Expected: clean. If Slint reports a compile error (e.g., `text-muted` not found), swap to a present property.

- [ ] **Step 4: Run the UI tests**

```
cargo test -p ui_shell --lib
```
Expected: pass. `ui_visibility_layout` and `ui_visibility_snapshots` tests may lightly exercise the new SystemView; if anything fails, inspect — it's likely a snapshot test that captures rendered text and needs its expectation updated with the new `"Startup ..."` line.

If a snapshot test fails solely because the rendered text now contains the new "Startup · total 0 ms" line (with `startup-phases` empty and `startup-total-ms` at 0 default), update the snapshot to include it. Don't skip failing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/ui_shell/ui/views/system_view.slint crates/ui_shell/tests 2>/dev/null || true
git add crates/ui_shell/ui/views/system_view.slint
git commit -m "feat(ui): render startup phase breakdown in Perf panel"
```

(If UI snapshot tests were updated, they'll be included in the `git add` of the tests dir.)

---

## Task 4: Instrument `main.rs` with phase markers

**Files:**
- Modify: `crates/app_desktop/src/main.rs`

**What this adds:** 11 `timer.phase(...)` calls at the natural seams of startup, plus a final `window.set_startup_phases(...)` and `window.set_startup_total_ms(...)` call right before `window.run()`.

- [ ] **Step 1: Read the current startup sequence**

```
cd /Users/jph/Voiceapp/workspace_template
sed -n '28,360p' crates/app_desktop/src/main.rs | head -200
```
Skim — you're looking for the sequence: setup_logging, config load, runtime build, core state, audio engine, media, window, populate devices, apply config, callback wiring, window.run. Each of those is a phase seam.

- [ ] **Step 2: Create the timer near the top of `fn main()`**

Find the existing `let startup_t0 = std::time::Instant::now();` line (around line 35). Replace it with:

```rust
    let mut timer = startup_timer::StartupTimer::new();
```

Keep the `log::info!("Voxlink starting");` line unchanged.

- [ ] **Step 3: Add `timer.phase("logging")` immediately after the crash reporter is installed**

Around the block:
```rust
    let log_path = setup_logging();
    if let Some(crash_dir) = crash_report::install(log_path.clone()) {
        log::info!("Crash reports will be written to {}", crash_dir.display());
    }
```

Add after it:
```rust
    timer.phase("logging");
```

- [ ] **Step 4: Add `timer.phase("config load")` after config load**

Find:
```rust
    config_store::migrate_legacy_auth_token();
    let config = config_store::load_config();
    let has_saved_auth = config_store::has_auth_token();
    log::info!("Config loaded ({}ms)", startup_t0.elapsed().as_millis());
```

The existing `startup_t0.elapsed()` log line refers to a variable that no longer exists (we renamed to `timer`). Fix the log line to use the timer:

```rust
    config_store::migrate_legacy_auth_token();
    let config = config_store::load_config();
    let has_saved_auth = config_store::has_auth_token();
    timer.phase("config load");
```

(The new `timer.phase` already logs a `startup:` line, so the old `log::info!("Config loaded")` is redundant — remove it.)

- [ ] **Step 5: Add `timer.phase("tokio runtime")` after the runtime is built**

Find the `tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()` block. After it (after `let rt_handle = rt.handle().clone();`):

```rust
    timer.phase("tokio runtime");
```

- [ ] **Step 6: Add `timer.phase("core state")` after the five core state allocs**

After the block that creates `perf`, `audio_active_flag`, `network_flag`, `voice`, `state`, `network`:

```rust
    timer.phase("core state");
```

- [ ] **Step 7: Add `timer.phase("audio engine")` after `AudioEngine::new()`**

Find the `Arc::new(TokioMutex::new(match audio_core::AudioEngine::new() { ... }))` block. After it (but before the `rt.block_on(async { aud.set_sensitivity(...)` block that configures the engine):

```rust
    timer.phase("audio engine");
```

- [ ] **Step 8: Add `timer.phase("audio config")` after the block_on that configures audio + loads soundboard**

After the `rt.block_on(async { ... })` block that ends with `for clip in &config.soundboard_clips { ... }`:

```rust
    timer.phase("audio config");
```

- [ ] **Step 9: Add `timer.phase("media + screen share")` after `MediaSession` and `ScreenShareController`**

After:
```rust
    let media = Arc::new(TokioMutex::new(media_transport::MediaSession::new(...)));
    let screen_share = Arc::new(screen_share::ScreenShareController::new());
```

Add:
```rust
    timer.phase("media + screen share");
```

- [ ] **Step 10: Add `timer.phase("main window")` after `MainWindow::new()`**

After the `match MainWindow::new() { ... }` block that binds `window`:

```rust
    timer.phase("main window");
```

- [ ] **Step 11: Add `timer.phase("device populate")` after `populate_devices`**

After:
```rust
    let (saved_input_idx, saved_output_idx) = populate_devices(&window, &audio, &rt, &config);
```

Add:
```rust
    timer.phase("device populate");
```

- [ ] **Step 12: Add `timer.phase("config applied")` after `apply_config`**

After:
```rust
    apply_config(&window, &config, saved_input_idx, saved_output_idx, &voice);
```

Add:
```rust
    timer.phase("config applied");
```

- [ ] **Step 13: Add `timer.phase("callbacks wired")` right before `window.run()`**

Find the existing:
```rust
    if let Err(err) = window.run() {
```

IMMEDIATELY BEFORE it (inside the same function), add:

```rust
    timer.phase("callbacks wired");

    let total = timer.total_ms();
    let phases = timer.into_phases();
    log::info!("startup: complete — {total}ms across {} phases", phases.len());

    // Push the breakdown into the window for the Perf panel.
    let phase_model: Vec<ui_shell::StartupPhaseData> = phases
        .into_iter()
        .map(|(name, ms)| ui_shell::StartupPhaseData {
            name: name.into(),
            ms: ms as i32,
        })
        .collect();
    window.set_startup_total_ms(total as i32);
    window.set_startup_phases(slint::ModelRc::new(slint::VecModel::from(phase_model)));
```

**IMPORTANT:** the `ui_shell::StartupPhaseData` type is re-exported by Slint's code generator when the struct is used in `main.slint`. If the import path differs (e.g., `ui_shell::StartupPhaseData` doesn't resolve), check the generated symbol — Slint emits types under the `ui_shell` crate root. Substitute the actual path if needed. The simplest fallback:

```rust
    // If the named type doesn't resolve, you can build the model with a
    // closure-based wrapper. But try the direct import first.
```

- [ ] **Step 14: Verify build**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
```
Expected: clean. Common failures:
- "`startup_timer` module not found" — confirm `mod startup_timer;` was added to main.rs in Task 1.
- "cannot find `StartupPhaseData` in `ui_shell`" — Slint re-exports structs through the code-generated output. If needed, add `pub use slint_generatedMainWindow::StartupPhaseData;` in `ui_shell/src/lib.rs` so it becomes accessible as `ui_shell::StartupPhaseData`. Grep for how other Slint-generated types are re-exported in that file for the pattern.

- [ ] **Step 15: Clippy**

```
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: ≤ 62.

- [ ] **Step 16: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add crates/app_desktop/src/main.rs crates/ui_shell/src/lib.rs 2>/dev/null || true
git add crates/app_desktop/src/main.rs
git commit -m "feat(client): instrument 11 startup phases in main.rs"
```

(If `ui_shell/src/lib.rs` needed a re-export tweak, include it in the commit.)

---

## Task 5: Document the target

**Files:**
- Modify: `docs/PERFORMANCE_TARGETS.md`

- [ ] **Step 1: Append a "Startup time" section**

Open `docs/PERFORMANCE_TARGETS.md`. At the end of the file (after the existing "Idle CPU baselines" section), append:

```markdown
---

## Startup time

Measured on Apple M4 Pro, release build, from `fn main()` entry to `window.run()`.

Target: **total startup ≤ 500 ms**.
Stretch: ≤ 300 ms.

Observed: run the client once, open the Perf panel — the "Startup" card shows
the total and per-phase breakdown. Also logged at info level in `voxlink.log`
as `startup: <phase> @ <N>ms` lines.

Per-phase expectations (rough heuristics from the spec):
- `logging`, `config load`, `tokio runtime`, `core state`, `media + screen share`, `config applied`, `callbacks wired` — each should be ≤ 30 ms.
- `audio engine` — expected to be the slowest phase on most machines (cpal device enumeration + Opus + neural denoiser init). Target ≤ 200 ms.
- `main window` — Slint render-graph construction and initial layout. Target ≤ 250 ms.
- `device populate` — one-time cpal audio device enumeration for the settings dropdown. Target ≤ 100 ms.

If a phase consistently exceeds its target, it is a candidate for optimization
in a follow-up milestone.
```

- [ ] **Step 2: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add docs/PERFORMANCE_TARGETS.md
git commit -m "docs: startup time target in PERFORMANCE_TARGETS.md"
```

---

## Task 6: Final verify + merge

- [ ] **Step 1: Workspace check**

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

- [ ] **Step 3: Non-flaky tests**

```
cargo test --workspace --no-fail-fast -- \
  --skip live_stress \
  --skip test_create_space \
  --skip test_audio_after_leave_room \
  --skip test_channel_audio_relay \
  --skip test_authenticate_invalid_token_creates_new \
  2>&1 | awk '/test result:/ {ok+=$4; fail+=$6} END {print "passed="ok, "failed="fail}'
```
Expected: `failed=0`. `passed` count should have gone up by 3 (new `StartupTimer` tests).

- [ ] **Step 4: Bench-check**

```
./scripts/bench-check.sh
```
Expected: exits 0. The M7 changes shouldn't affect any microbench.

- [ ] **Step 5: Commit manifest**

```
git log --oneline main..HEAD
```
Expected: five commits (Tasks 1 through 5).

- [ ] **Step 6: Merge to main**

```
git checkout main
git merge --ff-only feat/m7-startup-instrumentation
git branch -d feat/m7-startup-instrumentation
```

---

# Completion criteria

All of:

1. `cargo check --workspace` clean; clippy ≤ 62.
2. All non-flaky tests pass including 3 new `StartupTimer` unit tests.
3. `scripts/bench-check.sh` exits 0.
4. 11 `startup: <phase> @ <N>ms` lines appear in `voxlink.log` after a single client launch.
5. `docs/PERFORMANCE_TARGETS.md` has a "Startup time" section with a numeric target.
6. Opening the Perf panel after launch shows a "Startup · total N ms" card listing all 11 phases.

# If something goes wrong

- **`mod startup_timer;` produces duplicate-module errors**: the `startup_timer.rs` file might have accidentally been created in both `src/` and `src/bin/`. Keep only `crates/app_desktop/src/startup_timer.rs`.
- **Slint compile error on `StartupPhaseData` field access**: Slint struct field names use kebab-case in .slint files; in Rust they're accessed via snake_case. A `StartupPhaseData { name, ms }` in Slint becomes `StartupPhaseData { name: ..., ms: ... }` in Rust.
- **`ui_shell::StartupPhaseData` unresolved**: grep `pub use` in `crates/ui_shell/src/lib.rs` for existing re-exports of Slint-generated types and follow the same pattern.
- **`set_startup_phases` needs a `ModelRc<StartupPhaseData>` not a `VecModel`**: wrap with `slint::ModelRc::new(slint::VecModel::from(vec))` — shown in Task 4 Step 13.
- **UI snapshot test fails** with a diff that shows the new "Startup" card: update the snapshot to match. Don't skip.
- **`startup: complete` log line doesn't appear**: `timer` was moved before `window.run()` — check that `timer.into_phases()` and `window.set_startup_phases(...)` happen BEFORE `window.run()` (which blocks until exit).

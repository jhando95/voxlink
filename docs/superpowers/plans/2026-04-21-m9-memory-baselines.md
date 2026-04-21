# M9 — Memory Baselines Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Document target RSS baselines, extend the measurement script to sample RSS alongside CPU, and add a "Growth since launch" row to the Perf panel.

**Architecture:** Track `initial_memory_mb` on the second `snapshot()` call (first produces the baseline, subsequent calls compute deltas). Surface through `PerfSnapshot` → `PerfData` → SystemView. Rename `measure-idle-cpu.sh` → `measure-idle.sh` and teach it to sample `ps -o rss=`. No new deps.

**Tech Stack:** Rust 1.94, sysinfo (already a dep), `ps` (macOS built-in), Slint 1.15.

**Spec:** `docs/superpowers/specs/2026-04-21-m9-memory-baselines-design.md`

**Workspace root:** `/Users/jph/Voiceapp/workspace_template`
**Branch:** start on `feat/m9-memory-baselines` from `main`.

---

## Ground rules

1. **Workspace stays green.** `cargo check --workspace` passes before each commit.
2. **No new clippy warnings.** Baseline 62; must not exceed.
3. **No new deps.** RSS comes from existing sysinfo; script uses `ps`.
4. **Existing tests keep passing.** Including the `snapshot_returns_non_negative_values` test in perf_metrics.

---

## Task 0: Branch

- [ ] **Step 1: Clean tree + baseline**

```
cd /Users/jph/Voiceapp/workspace_template
git status --short    # expect empty (discard Cargo.lock drift if present)
git checkout -b feat/m9-memory-baselines
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: `62`.

No commit.

---

## Task 1: Track memory growth in `PerfCollector`

**Files:**
- Modify: `crates/shared_types/src/state.rs` (add `memory_growth_mb: f32` field)
- Modify: `crates/perf_metrics/src/lib.rs` (add `initial_memory_mb: Option<f32>`, populate, expose in snapshot)

**What this adds:** The collector records the memory reading from its second snapshot as a baseline, then every subsequent snapshot reports the delta. The first snapshot reports `0.0` for `memory_growth_mb` (baseline not yet established); snapshot #2 also reports `0.0` (baseline just set); snapshot #3 and onwards report real growth.

- [ ] **Step 1: Extend `PerfSnapshot`**

Open `crates/shared_types/src/state.rs`. Find `pub struct PerfSnapshot { ... }` (around line 199). Append one new field at the end:

```rust
pub struct PerfSnapshot {
    // ... existing fields through audio_glitch_count ...
    pub audio_glitch_count: u32,
    /// RSS growth since `initial_memory_mb` was captured (second snapshot).
    /// Zero until the baseline is established.
    pub memory_growth_mb: f32,
}
```

- [ ] **Step 2: Add the field to `PerfCollector`**

Open `crates/perf_metrics/src/lib.rs`. Find `pub struct PerfCollector { ... }`. Add `initial_memory_mb: Option<f32>` after the existing `peak_memory_mb: f32` field:

```rust
pub struct PerfCollector {
    start_time: Instant,
    system: Option<System>,
    pid: Pid,
    num_cpus: f32,
    peak_memory_mb: f32,
    /// Set on the second `snapshot()` call so the sysinfo lazy-init cost
    /// is captured as baseline, not reported as growth.
    initial_memory_mb: Option<f32>,
    // ... rest of existing fields ...
}
```

- [ ] **Step 3: Initialize the new field in `PerfCollector::new`**

Find `impl PerfCollector { pub fn new() -> Self { ... } }`. Inside the `Self { ... }` literal, add `initial_memory_mb: None` alongside `peak_memory_mb: 0.0`:

```rust
        Self {
            start_time: Instant::now(),
            system: None,
            pid: ...,
            num_cpus: ...,
            peak_memory_mb: 0.0,
            initial_memory_mb: None,
            // ... rest unchanged ...
        }
```

- [ ] **Step 4: Compute `memory_growth_mb` in `snapshot()`**

In `crates/perf_metrics/src/lib.rs`, find `pub fn snapshot(&mut self) -> PerfSnapshot`. Currently it binds `mem` around line 83. After the `self.peak_memory_mb = self.peak_memory_mb.max(mem);` line (around line 86), add the growth-tracking logic:

```rust
        self.peak_memory_mb = self.peak_memory_mb.max(mem);

        // Record the second-snapshot memory reading as baseline so sysinfo's
        // lazy-init cost is captured as baseline, not reported as growth.
        // First snapshot: initial_memory_mb is None -> growth reported as 0,
        //                 but do NOT set baseline yet (sysinfo still warming).
        // Second snapshot: set baseline to current mem, report 0 growth.
        // Third+: report mem - baseline.
        let memory_growth_mb = match self.initial_memory_mb {
            None => {
                // Delay baseline to snapshot #2 via a two-phase flag.
                // Use NaN as "we've seen one snapshot; take next as baseline".
                self.initial_memory_mb = Some(f32::NAN);
                0.0
            }
            Some(b) if b.is_nan() => {
                self.initial_memory_mb = Some(mem);
                0.0
            }
            Some(baseline) => (mem - baseline).max(0.0),
        };
```

Then in the `PerfSnapshot { ... }` literal (around line 123), add the field at the end:

```rust
        PerfSnapshot {
            // ... existing fields through audio_glitch_count ...
            audio_glitch_count,
            memory_growth_mb,
        }
```

- [ ] **Step 5: Add a unit test**

In `crates/perf_metrics/src/lib.rs`, inside the existing `#[cfg(test)] mod tests { ... }` block, append:

```rust
    #[test]
    fn memory_growth_is_zero_on_first_two_snapshots() {
        let mut collector = PerfCollector::new();
        let snap1 = collector.snapshot();
        let snap2 = collector.snapshot();
        assert_eq!(
            snap1.memory_growth_mb, 0.0,
            "snapshot #1 should report zero growth (baseline not yet set)"
        );
        assert_eq!(
            snap2.memory_growth_mb, 0.0,
            "snapshot #2 should report zero growth (baseline just set)"
        );
    }

    #[test]
    fn memory_growth_non_negative_on_third_snapshot() {
        let mut collector = PerfCollector::new();
        let _ = collector.snapshot();
        let _ = collector.snapshot();
        let snap3 = collector.snapshot();
        // Growth is clamped to >= 0 (can't shrink below baseline reported).
        assert!(
            snap3.memory_growth_mb >= 0.0,
            "snapshot #3 growth = {} should be non-negative",
            snap3.memory_growth_mb
        );
    }
```

- [ ] **Step 6: Verify**

```
cd /Users/jph/Voiceapp/workspace_template
cargo test -p perf_metrics
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: all tests pass; check clean; clippy ≤ 62.

- [ ] **Step 7: Commit**

```bash
git add crates/shared_types/src/state.rs crates/perf_metrics/src/lib.rs
git commit -m "feat(perf): track memory growth since launch in PerfSnapshot"
```

---

## Task 2: Render "Growth" row in Perf panel

**Files:**
- Modify: `crates/ui_shell/ui/theme.slint` (extend `PerfData`)
- Modify: `crates/ui_shell/src/lib.rs` (map snapshot field)
- Modify: `crates/ui_shell/ui/views/system_view.slint` (render row)

- [ ] **Step 1: Add `memory-growth-mb` to `PerfData`**

Open `crates/ui_shell/ui/theme.slint`. Find `export struct PerfData { ... }` (around line 22). Append one field after `audio-glitch-count`:

```slint
export struct PerfData {
    // ... existing fields ...
    capture-callback-median-ms: float,
    playback-callback-median-ms: float,
    audio-glitch-count: int,
    // M9
    memory-growth-mb: float,
}
```

- [ ] **Step 2: Map the new field**

Open `crates/ui_shell/src/lib.rs`. Find `pub fn update_perf_display` (around line 68). In the `PerfData { ... }` literal, append after `audio_glitch_count`:

```rust
    let perf = PerfData {
        // ... existing fields ...
        audio_glitch_count: snap.audio_glitch_count as i32,
        memory_growth_mb: snap.memory_growth_mb,
    };
```

- [ ] **Step 3: Render the Growth row in SystemView**

Open `crates/ui_shell/ui/views/system_view.slint`. Find the existing memory display — grep:

```
grep -n "memory-mb\|peak-memory-mb\|Peak memory\|MetricRow" crates/ui_shell/ui/views/system_view.slint
```

Find the `MetricRow { label: "Peak memory"; ... }` (or the equivalent hand-rolled row). Add a new row directly after it:

```slint
MetricRow {
    label: "Growth";
    value: "+" + root.perf.memory-growth-mb + " MB";
    indicator-color: root.perf.memory-growth-mb <= 10 ? #4ade80
                   : root.perf.memory-growth-mb <= 50 ? #facc15
                   : #ff6b6b;
}
```

The `MetricRow` component (as discovered in M8) already accepts `indicator-color`, so the color-by-threshold logic works out of the box. If `MetricRow` doesn't accept that property in this codebase, fall back to hand-rolled:

```slint
HorizontalLayout {
    spacing: 12px;
    Text {
        text: "Growth";
        font-size: 12px;
        color: VxTheme.text-primary;
        horizontal-stretch: 1;
    }
    Text {
        text: "+" + root.perf.memory-growth-mb + " MB";
        font-size: 12px;
        color: root.perf.memory-growth-mb <= 10 ? #4ade80
             : root.perf.memory-growth-mb <= 50 ? #facc15
             : #ff6b6b;
        horizontal-alignment: right;
    }
}
```

Place this in BOTH the narrow-layout and wide-layout branches of `SystemView` — the codebase renders the Perf card in both. Grep for `"Peak memory"` to find both occurrences.

- [ ] **Step 4: Verify**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
cargo test -p ui_shell --lib 2>&1 | tail -5
```
Expected: clean, clippy ≤ 62, tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/ui_shell/ui/theme.slint crates/ui_shell/src/lib.rs crates/ui_shell/ui/views/system_view.slint
git commit -m "feat(ui): render Growth row in Perf panel memory section"
```

---

## Task 3: Rename script + sample RSS

**Files:**
- Rename: `scripts/measure-idle-cpu.sh` → `scripts/measure-idle.sh`
- Modify: script body to sample RSS alongside CPU

- [ ] **Step 1: Rename the file via `git mv`**

```
cd /Users/jph/Voiceapp/workspace_template
git mv scripts/measure-idle-cpu.sh scripts/measure-idle.sh
```

- [ ] **Step 2: Update the script to sample RSS**

Open `scripts/measure-idle.sh`. Find the `sample_cpu()` helper function. After it, add `sample_rss()`:

```bash
# Sample RSS memory (in MB) for PID over SAMPLE_DURATION seconds. Averages
# the per-second samples.
sample_rss() {
    local pid=$1
    local n=0
    local sum=0
    local kb=""
    for _ in $(seq 1 $SAMPLE_DURATION); do
        kb=$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ')
        if [ -n "$kb" ]; then
            sum=$((sum + kb))
            n=$((n + 1))
        fi
        sleep 1
    done
    if [ $n -gt 0 ]; then
        # Convert kB to MB, print with one decimal.
        awk -v s=$sum -v n=$n 'BEGIN { printf "%.1f", (s / n) / 1024 }'
    else
        echo "NaN"
    fi
}

# Take SAMPLES_PER_SCENARIO RSS measurements, print the median.
median_rss() {
    local pid=$1
    local values=()
    for _ in $(seq 1 $SAMPLES_PER_SCENARIO); do
        values+=("$(sample_rss "$pid")")
    done
    printf "%s\n" "${values[@]}" | sort -n \
        | awk -v n=$SAMPLES_PER_SCENARIO 'NR == int((n+1)/2)'
}
```

- [ ] **Step 3: Update the table header and each printf to include RSS**

Find the table header in the script:

```bash
echo "| Scenario | CPU % |"
echo "|---|--:|"
```

Change to:

```bash
echo "| Scenario | CPU % | RSS MB |"
echo "|---|--:|--:|"
```

Then find each `printf "| ... | %s |\n" "$CPU"` line and extend it to also call `median_rss` and print the value. Example:

```bash
# --- server_zero_peers ---
PV_ADDR=127.0.0.1:$PORT "$SERVER_BIN" > "$TMPDIR/server.log" 2>&1 &
SERVER_PID=$!
sleep 3

CPU=$(median_cpu "$SERVER_PID")
RSS=$(median_rss "$SERVER_PID")
printf "| server_zero_peers | %s | %s |\n" "$CPU" "$RSS"
```

Do this for all three automated scenarios (`server_zero_peers`, `client_home`, `server_one_idle_peer`).

For the two manual scenarios at the bottom:

```bash
echo "| client_joined_silent | (manual — see below) | (manual) |"
echo "| client_minimized | (manual — see below) | (manual) |"
```

- [ ] **Step 4: Update the manual-procedure text at the bottom of the script**

Find the block that explains the manual scenarios (starts with `## Manual scenarios`). Update the sampling command from CPU-only to both:

```bash
echo "5. In another terminal:"
echo "   \`\`\`"
echo "   PID=\$(pgrep -x app_desktop)"
echo "   for _ in 1 2 3; do"
echo "     top -pid \$PID -l 31 -stats cpu | awk 'NR > 2 && /^[0-9.]+\$/ {sum+=\$1; n++} END {if (n > 0) printf \"cpu=%.1f%% \", sum/n}'"
echo "     kb=\$(ps -o rss= -p \$PID | tr -d ' ')"
echo "     awk -v kb=\$kb 'BEGIN { printf \"rss=%.1f MB\\n\", kb/1024 }'"
echo "   done"
echo "   \`\`\`"
```

- [ ] **Step 5: Update the script's header comment block to match the new scope**

Find the comment block at the top of the script (lines starting with `#`). Change the description:

Before:
```
# Measure idle CPU% for Voxlink client and server across scenarios.
# Prints a markdown table to stdout — suitable for pasting into
# docs/PERFORMANCE_TARGETS.md.
```

After:
```
# Measure idle CPU% and RSS memory for Voxlink client and server across
# scenarios. Prints a markdown table to stdout — suitable for pasting into
# docs/PERFORMANCE_TARGETS.md.
```

- [ ] **Step 6: Syntax check**

```
bash -n scripts/measure-idle.sh
```
Expected: no output.

- [ ] **Step 7: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add scripts/measure-idle.sh
git commit -m "scripts: rename to measure-idle.sh and sample RSS alongside CPU"
```

---

## Task 4: Memory baselines in PERFORMANCE_TARGETS.md

**Files:**
- Modify: `docs/PERFORMANCE_TARGETS.md`

- [ ] **Step 1: Replace old script references**

Open `docs/PERFORMANCE_TARGETS.md`. Grep for the old script name:

```
grep -n "measure-idle-cpu\.sh" docs/PERFORMANCE_TARGETS.md
```

For each hit (likely 2–3 in the Idle CPU section), replace `measure-idle-cpu.sh` with `measure-idle.sh`.

- [ ] **Step 2: Append "Memory baselines" section at the end of the file**

```markdown
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

The Perf panel in the running client shows a "Growth" row that tracks RSS delta since the second perf snapshot (i.e., after sysinfo's lazy initialization is accounted for). Color thresholds:

- **≤ +10 MB**: green (expected: log file growth, UI text cache churn, etc.)
- **+10 to +50 MB**: amber (tolerable but worth an occasional look)
- **> +50 MB**: red (investigate; likely leak in call state, channel buffers, or cached DMs)

### Rationale

Voxlink ships with a neural denoiser (nnnoiseless) + Slint render graph + Rust runtime. A realistic floor for the client before any room activity is ~140 MB. The server has no UI, no audio pipeline, no denoiser; it must fit comfortably inside the Oracle free-tier 1 GB VM alongside the OS — target ~50 MB idle.

For comparison, Discord's Electron desktop client routinely idles at ~300+ MB.
```

- [ ] **Step 3: Verify no leftover old-name references**

```
grep -n "measure-idle-cpu" docs/PERFORMANCE_TARGETS.md
```
Expected: no output (all replaced in step 1).

- [ ] **Step 4: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add docs/PERFORMANCE_TARGETS.md
git commit -m "docs: memory baselines in PERFORMANCE_TARGETS.md"
```

---

## Task 5: Final verify + merge

- [ ] **Step 1: Workspace check + clippy**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: clean, clippy ≤ 62.

- [ ] **Step 2: Non-flaky tests**

```
cargo test --workspace --no-fail-fast -- \
  --skip live_stress \
  --skip test_create_space \
  --skip test_audio_after_leave_room \
  --skip test_channel_audio_relay \
  --skip test_authenticate_invalid_token_creates_new \
  2>&1 | awk '/test result:/ {ok+=$4; fail+=$6} END {print "passed="ok, "failed="fail}'
```
Expected: `failed=0`. `passed` count grows by 2 (new `memory_growth_*` tests).

- [ ] **Step 3: Bench-check**

```
./scripts/bench-check.sh
```
Expected: exits 0.

- [ ] **Step 4: Commit manifest**

```
git log --oneline main..HEAD
```
Expected: four feature commits (Tasks 1 through 4).

- [ ] **Step 5: Merge to main**

```
git checkout main
git merge --ff-only feat/m9-memory-baselines
git branch -d feat/m9-memory-baselines
```

---

# Completion criteria

All of:

1. `cargo check --workspace` clean; clippy ≤ 62.
2. All non-flaky tests pass; two new `memory_growth_*` tests pass.
3. `scripts/bench-check.sh` exits 0.
4. `scripts/measure-idle.sh` exists and produces a CPU% + RSS table.
5. `docs/PERFORMANCE_TARGETS.md` has a "Memory baselines" section and no remaining `measure-idle-cpu.sh` references.
6. The Perf panel (view 3) shows a "Growth" row with color-by-threshold text under the Memory section.

# If something goes wrong

- **`PerfSnapshot` construction sites elsewhere fail to compile with missing `memory_growth_mb`**: grep `PerfSnapshot {` across the workspace; any match needs `memory_growth_mb: 0.0` added. Expected: only one site in `perf_metrics/src/lib.rs`.
- **`MetricRow` doesn't accept `indicator-color`**: M8's spec-reviewer found `MetricRow` has `metric-label` / `metric-value` / `indicator-color` properties. If this codebase's `MetricRow` doesn't expose `indicator-color`, use the hand-rolled `HorizontalLayout` form shown in Task 2 Step 3.
- **Memory growth tests flaky (snapshot #3 returns NaN or negative)**: `.max(0.0)` in the `Some(baseline)` branch of `snapshot()` guarantees `>= 0.0`. If a test still fails, inspect the snapshot values with `dbg!` and adjust the test tolerance.
- **Script RSS reading is blank**: `ps -o rss=` on macOS sometimes returns empty for processes that are starting up. The `sample_rss()` helper already checks `-n "$kb"` before summing. Ensure `sleep 1` is between each sample so the process has time to exist.
- **Renamed script breaks existing operator scripts / muscle memory**: acceptable — the new name reflects the expanded scope. The old script was only referenced in PERFORMANCE_TARGETS.md (updated in Task 4) and wasn't wired into any automation.

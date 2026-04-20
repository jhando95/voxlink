# M8 — Client Audio Callback Histogram Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Instrument client audio capture + playback callbacks with latency histograms, track a 10 ms glitch counter, surface three numbers in the Perf panel (capture median, playback median, glitch count).

**Architecture:** Duplicate `Histogram` from `signaling_server` into `audio_core` with one added method (`median()`). Extend `AudioMetrics` with two `Arc<Histogram>` fields + shared `Arc<AtomicU32>` glitch counter. Wrap both cpal callback bodies in `Instant::now()` + observe. Expose medians + glitch count through `PerfCollector` → `PerfSnapshot` → Slint `PerfData`. Render new rows in SystemView's Perf card.

**Tech Stack:** Rust 1.94, cpal 0.15, Slint 1.15. No new deps.

**Spec:** `docs/superpowers/specs/2026-04-20-m8-audio-callback-histogram-design.md`

**Workspace root:** `/Users/jph/Voiceapp/workspace_template`
**Branch:** start on `feat/m8-audio-callback-histogram` from `main`.

---

## Ground rules

1. **Workspace stays green.** `cargo check --workspace` passes before each commit.
2. **No new clippy warnings.** Baseline 62; must not exceed.
3. **No audio behavior changes.** Instrumentation only. Both callbacks produce identical output.
4. **No microbench regressions.** Run `./scripts/bench-check.sh` at the end.

---

## Task 0: Branch

- [ ] **Step 1: Clean tree + baseline**

```
cd /Users/jph/Voiceapp/workspace_template
git status --short    # expect empty (discard Cargo.lock drift if present)
git checkout -b feat/m8-audio-callback-histogram
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: `62`.

No commit.

---

## Task 1: Duplicate `Histogram` into `audio_core` with `median()` helper

**Files:**
- Create: `crates/audio_core/src/histogram.rs`
- Modify: `crates/audio_core/src/lib.rs` (declare + re-export)

- [ ] **Step 1: Create `crates/audio_core/src/histogram.rs`**

Copy the signaling_server version with two changes: (a) top-level module comment updated for audio context, (b) new `median()` method, (c) no `#![allow(dead_code)]` — all fields will be used.

```rust
//! Lock-free, allocation-free histogram for audio callback latency metrics.
//!
//! Mirrors `signaling_server::histogram::Histogram`. The two copies are kept
//! in sync manually; both use the same log-spaced bucket layout.
//!
//! Observation is four `fetch_add(Relaxed)` in the worst case.

use std::sync::atomic::{AtomicU64, Ordering};

/// Bucket upper bounds, in seconds. Must be sorted ascending.
pub const BOUNDS_SECS: [f64; 11] = [
    0.0005, 0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0,
];

/// Total number of buckets including the implicit `+Inf` final bucket.
pub const BUCKET_COUNT: usize = BOUNDS_SECS.len() + 1;

/// Lock-free histogram.
pub struct Histogram {
    name: &'static str,
    help: &'static str,
    buckets: [AtomicU64; BUCKET_COUNT],
    total_count: AtomicU64,
    total_sum_nanos: AtomicU64,
}

impl Histogram {
    pub const fn new(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            buckets: [
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
            ],
            total_count: AtomicU64::new(0),
            total_sum_nanos: AtomicU64::new(0),
        }
    }

    /// Record one observation.
    pub fn observe(&self, value_secs: f64) {
        let idx = BOUNDS_SECS
            .iter()
            .position(|&b| value_secs <= b)
            .unwrap_or(BUCKET_COUNT - 1);
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
        self.total_count.fetch_add(1, Ordering::Relaxed);
        let nanos = (value_secs.max(0.0) * 1e9) as u64;
        self.total_sum_nanos.fetch_add(nanos, Ordering::Relaxed);
    }

    /// Total number of recorded observations.
    pub fn count(&self) -> u64 {
        self.total_count.load(Ordering::Relaxed)
    }

    /// Approximate median: upper bound of the bucket where the cumulative
    /// count crosses half the total. Returns 0.0 on empty. Granularity
    /// equals the bucket width at that point (acceptable for a UI display).
    pub fn median(&self) -> f64 {
        let total = self.total_count.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let half = total / 2;
        let mut cum: u64 = 0;
        for (i, &bound) in BOUNDS_SECS.iter().enumerate() {
            cum = cum.saturating_add(self.buckets[i].load(Ordering::Relaxed));
            if cum >= half {
                return bound;
            }
        }
        f64::INFINITY
    }

    /// Mean observation time in seconds.
    pub fn mean_secs(&self) -> f64 {
        let total = self.total_count.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let sum_nanos = self.total_sum_nanos.load(Ordering::Relaxed) as f64;
        (sum_nanos / total as f64) / 1e9
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_count_matches_array_literal() {
        assert_eq!(BUCKET_COUNT, BOUNDS_SECS.len() + 1);
    }

    #[test]
    fn observe_buckets_correctly() {
        let h = Histogram::new("test_hist", "test");
        h.observe(0.0001);
        h.observe(0.003);
        h.observe(0.5);
        h.observe(5.0);
        assert_eq!(h.buckets[0].load(Ordering::Relaxed), 1);
        assert_eq!(h.buckets[3].load(Ordering::Relaxed), 1);
        assert_eq!(h.buckets[9].load(Ordering::Relaxed), 1);
        assert_eq!(h.buckets[BUCKET_COUNT - 1].load(Ordering::Relaxed), 1);
        assert_eq!(h.total_count.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn median_empty_is_zero() {
        let h = Histogram::new("t", "t");
        assert_eq!(h.median(), 0.0);
    }

    #[test]
    fn median_single_bucket() {
        let h = Histogram::new("t", "t");
        // 10 observations, all falling in bucket 3 (le=0.005)
        for _ in 0..10 {
            h.observe(0.003);
        }
        assert_eq!(h.median(), 0.005);
    }

    #[test]
    fn median_mixed_distribution() {
        let h = Histogram::new("t", "t");
        // 5 small, 5 large — median should land in the small region
        for _ in 0..5 { h.observe(0.0001); }
        for _ in 0..5 { h.observe(0.5); }
        // cumulative at bucket 0 (le=0.0005) = 5; half of 10 = 5; first bucket
        // whose cumulative ≥ 5 is bucket 0.
        assert_eq!(h.median(), 0.0005);
    }

    #[test]
    fn median_all_in_overflow_is_infinity() {
        let h = Histogram::new("t", "t");
        for _ in 0..5 {
            h.observe(10.0);   // falls into +Inf bucket
        }
        assert!(h.median().is_infinite());
    }

    #[test]
    fn mean_is_approximately_sum_over_count() {
        let h = Histogram::new("t", "t");
        h.observe(0.001);
        h.observe(0.003);
        let m = h.mean_secs();
        assert!((m - 0.002).abs() < 1e-6, "mean = {m}");
    }

    #[test]
    fn count_reflects_observations() {
        let h = Histogram::new("t", "t");
        assert_eq!(h.count(), 0);
        h.observe(0.001);
        h.observe(0.002);
        assert_eq!(h.count(), 2);
    }

    #[test]
    fn observe_does_not_panic_on_negative() {
        let h = Histogram::new("t", "t");
        h.observe(-1.0);
        assert_eq!(h.buckets[0].load(Ordering::Relaxed), 1);
        assert_eq!(h.total_sum_nanos.load(Ordering::Relaxed), 0);
    }
}
```

- [ ] **Step 2: Declare + re-export in `crates/audio_core/src/lib.rs`**

Near the top of `crates/audio_core/src/lib.rs`, among the `pub mod` declarations, add:

```rust
pub mod histogram;
pub use histogram::Histogram;
```

- [ ] **Step 3: Build and test**

```
cd /Users/jph/Voiceapp/workspace_template
cargo test -p audio_core histogram::
```
Expected: 9 tests pass.

```
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: check clean, clippy ≤ 62.

- [ ] **Step 4: Commit**

```bash
git add crates/audio_core/src/histogram.rs crates/audio_core/src/lib.rs
git commit -m "feat(audio_core): Histogram with median() helper for callback metrics"
```

---

## Task 2: Extend `AudioMetrics` + instrument both callbacks

**Files:**
- Modify: `crates/audio_core/src/lib.rs`

- [ ] **Step 1: Add three fields to `AudioMetrics`**

Find the existing `AudioMetrics` struct (around line 32 of `crates/audio_core/src/lib.rs`). Extend it:

```rust
pub struct AudioMetrics {
    pub frames_decoded: Arc<AtomicU32>,
    pub frames_dropped: Arc<AtomicU32>,
    pub current_jitter_ms: Arc<AtomicU32>,
    pub active_peers: Arc<AtomicU32>,
    pub encode_bitrate_kbps: Arc<AtomicU32>,
    /// Capture-callback execution time distribution.
    pub capture_callback_hist: Arc<Histogram>,
    /// Playback-callback execution time distribution.
    pub playback_callback_hist: Arc<Histogram>,
    /// Count of capture OR playback callbacks that took >= 10 ms
    /// (audible-glitch threshold at 48 kHz with typical buffer sizes).
    pub callback_glitch_count: Arc<AtomicU32>,
}
```

- [ ] **Step 2: Initialize the new fields in `AudioMetrics::new`**

Find `impl AudioMetrics { pub fn new() -> Self { ... } }`. Extend the `Self { ... }` literal:

```rust
impl AudioMetrics {
    pub fn new() -> Self {
        Self {
            frames_decoded: Arc::new(AtomicU32::new(0)),
            frames_dropped: Arc::new(AtomicU32::new(0)),
            current_jitter_ms: Arc::new(AtomicU32::new(JITTER_INITIAL as u32 * 20)),
            active_peers: Arc::new(AtomicU32::new(0)),
            encode_bitrate_kbps: Arc::new(AtomicU32::new(64)),
            capture_callback_hist: Arc::new(Histogram::new(
                "audio_capture_callback_seconds",
                "Execution time of cpal input callback (includes DSP chain + Opus encode)",
            )),
            playback_callback_hist: Arc::new(Histogram::new(
                "audio_playback_callback_seconds",
                "Execution time of cpal output callback (includes Opus decode + jitter buffer + mix)",
            )),
            callback_glitch_count: Arc::new(AtomicU32::new(0)),
        }
    }
}
```

- [ ] **Step 3: Instrument the capture callback (input_stream)**

Find the capture callback closure (around `device.build_input_stream`, line ~620). Before the callback is constructed, add `Arc` clones outside the closure:

```rust
let capture_hist = self.metrics.capture_callback_hist.clone();
let glitch_count_capture = self.metrics.callback_glitch_count.clone();
```

(Place these next to the other `Arc` clones that the closure captures. Look for nearby patterns like `let encode_errors = ...` — the same style applies.)

Then wrap the closure body with timing:

```rust
let stream = device.build_input_stream(
    &config,
    move |data: &[f32], _: &cpal::InputCallbackInfo| {
        let _t0 = std::time::Instant::now();
        // ... ALL EXISTING BODY unchanged ...

        // At the very end of the closure body:
        let elapsed = _t0.elapsed().as_secs_f64();
        capture_hist.observe(elapsed);
        if elapsed >= 0.010 {
            glitch_count_capture.fetch_add(1, Ordering::Relaxed);
        }
    },
    err_fn,
    None,
)?;
```

Rename `_t0` to `t0` (we use it, so the underscore isn't needed — was just for clarity in the template).

IMPORTANT: the existing body has early returns / continue statements (e.g., `if is_capturing == 0 { capture_ring.clear(); return; }` or similar). The timing must capture the entire callback wallclock including early-return paths. One way: use a scope guard / drop helper. Simpler: wrap the body in an immediately-invoked closure and measure the closure's execution:

```rust
move |data: &[f32], _info: &cpal::InputCallbackInfo| {
    let t0 = std::time::Instant::now();
    (|| {
        // ... existing body, early-returns are OK inside this closure ...
    })();
    let elapsed = t0.elapsed().as_secs_f64();
    capture_hist.observe(elapsed);
    if elapsed >= 0.010 {
        glitch_count_capture.fetch_add(1, Ordering::Relaxed);
    }
}
```

Use this closure-wrapping pattern. Any `return` inside the inner `(||  { ... })()` only exits the inner closure; the `elapsed` observation always runs.

- [ ] **Step 4: Instrument the playback callback (output_stream)**

Find the playback callback closure (around `device.build_output_stream`, line ~849). Same pattern:

Before the stream construction:

```rust
let playback_hist = self.metrics.playback_callback_hist.clone();
let glitch_count_playback = self.metrics.callback_glitch_count.clone();
```

Wrap the callback body:

```rust
move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
    let t0 = std::time::Instant::now();
    (|| {
        // ... existing body ...
    })();
    let elapsed = t0.elapsed().as_secs_f64();
    playback_hist.observe(elapsed);
    if elapsed >= 0.010 {
        glitch_count_playback.fetch_add(1, Ordering::Relaxed);
    }
}
```

- [ ] **Step 5: Verify workspace**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: check clean, clippy ≤ 62.

Run the audio unit tests to confirm nothing about the streams broke:

```
cargo test -p audio_core --lib 2>&1 | tail -5
```
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/audio_core/src/lib.rs
git commit -m "feat(audio_core): histogram capture + playback callback timings"
```

---

## Task 3: Extend `PerfSnapshot` + `PerfCollector`

**Files:**
- Modify: `crates/shared_types/src/state.rs` (extend `PerfSnapshot`)
- Modify: `crates/perf_metrics/Cargo.toml` (add `audio_core` dep)
- Modify: `crates/perf_metrics/src/lib.rs` (new fields + populate in `snapshot`)

- [ ] **Step 1: Extend `PerfSnapshot` in `shared_types`**

Open `crates/shared_types/src/state.rs`. Find the `pub struct PerfSnapshot { ... }` (around line 199). Append three fields at the end:

```rust
pub struct PerfSnapshot {
    // ... existing fields ...
    pub screen_frames_completed: u32,
    pub screen_frames_dropped: u32,
    pub screen_frames_timed_out: u32,
    // M8: audio callback health
    pub capture_callback_median_ms: f32,
    pub playback_callback_median_ms: f32,
    pub audio_glitch_count: u32,
}
```

- [ ] **Step 2: Update any `PerfSnapshot` literals that list fields explicitly**

Grep for `PerfSnapshot {` across the workspace:
```
grep -rn "PerfSnapshot {" crates/
```

For any `PerfSnapshot { ... }` struct-literal construction that names fields, add defaults for the three new fields:
```rust
    capture_callback_median_ms: 0.0,
    playback_callback_median_ms: 0.0,
    audio_glitch_count: 0,
```

(Most likely there's only one construction site in `crates/perf_metrics/src/lib.rs`.)

`cargo check -p shared_types` first to confirm the struct compiles, then `cargo check --workspace` to see which crates need updating.

- [ ] **Step 3: Add `audio_core` dep to `perf_metrics`**

Open `crates/perf_metrics/Cargo.toml`. Add to `[dependencies]`:

```toml
audio_core = { path = "../audio_core" }
```

(Place alongside `shared_types = { path = "../shared_types" }`.)

- [ ] **Step 4: Extend `PerfCollector`**

Open `crates/perf_metrics/src/lib.rs`. Find the `pub struct PerfCollector { ... }`. Add three fields after the existing `screen_frames_*` fields:

```rust
pub struct PerfCollector {
    // ... existing ...
    pub screen_frames_completed: Arc<AtomicU32>,
    pub screen_frames_dropped: Arc<AtomicU32>,
    pub screen_frames_timed_out: Arc<AtomicU32>,
    // M8: audio callback health
    pub capture_callback_hist: Option<Arc<audio_core::Histogram>>,
    pub playback_callback_hist: Option<Arc<audio_core::Histogram>>,
    pub callback_glitch_count: Arc<AtomicU32>,
    // existing trailing fields:
    last_decoded: u32,
    last_dropped: u32,
}
```

- [ ] **Step 5: Initialize in `PerfCollector::new`**

Find `impl PerfCollector { pub fn new() -> Self { ... } }`. Extend the `Self { ... }` literal:

```rust
    capture_callback_hist: None,
    playback_callback_hist: None,
    callback_glitch_count: Arc::new(AtomicU32::new(0)),
```

(Histograms start as `None` because `PerfCollector::new` runs before `AudioEngine::new`. `main.rs` will wire them after the audio engine exists, matching the existing pattern for `frames_decoded` etc.)

- [ ] **Step 6: Populate the three new fields in `snapshot()`**

Find `pub fn snapshot(&mut self) -> PerfSnapshot` in `perf_metrics/src/lib.rs`. At the field-literal construction, add three computed values:

```rust
        let capture_median_ms = self
            .capture_callback_hist
            .as_ref()
            .map(|h| (h.median() * 1000.0) as f32)
            .unwrap_or(0.0);
        let playback_median_ms = self
            .playback_callback_hist
            .as_ref()
            .map(|h| (h.median() * 1000.0) as f32)
            .unwrap_or(0.0);
        let glitch_count = self.callback_glitch_count.load(Ordering::Relaxed);
```

Then in the `PerfSnapshot { ... }` literal, add:

```rust
    capture_callback_median_ms: capture_median_ms,
    playback_callback_median_ms: playback_median_ms,
    audio_glitch_count: glitch_count,
```

If `median()` returns `f64::INFINITY` (all observations in the +Inf bucket), that cast to f32 produces `f32::INFINITY`. Downstream UI display should handle that gracefully; if not, clamp to e.g. 999.0 here:

```rust
        let capture_median_ms = self
            .capture_callback_hist
            .as_ref()
            .map(|h| {
                let m = h.median() * 1000.0;
                if m.is_finite() { m as f32 } else { 999.0 }
            })
            .unwrap_or(0.0);
```

Use the finite-clamp form; it keeps the UI from showing "inf" to users.

- [ ] **Step 7: Verify workspace**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: check clean, clippy ≤ 62.

- [ ] **Step 8: Commit**

```bash
git add crates/shared_types/src/state.rs crates/perf_metrics/Cargo.toml crates/perf_metrics/src/lib.rs
git commit -m "feat(perf): expose audio callback medians + glitch count in PerfSnapshot"
```

---

## Task 4: Render in SystemView + wire up at startup

**Files:**
- Modify: `crates/ui_shell/ui/theme.slint` (extend `PerfData`)
- Modify: `crates/ui_shell/src/lib.rs` (map snapshot → PerfData)
- Modify: `crates/ui_shell/ui/views/system_view.slint` (new rows)
- Modify: `crates/app_desktop/src/main.rs` (wire `AudioMetrics` → `PerfCollector`)

- [ ] **Step 1: Extend `PerfData` Slint struct**

Open `crates/ui_shell/ui/theme.slint`. Find the `export struct PerfData { ... }` block (line 22). Append three fields:

```slint
export struct PerfData {
    cpu-percent: float,
    memory-mb: float,
    peak-memory-mb: float,
    uptime-secs: int,
    audio-active: bool,
    network-connected: bool,
    dropped-frames: int,
    jitter-buffer-ms: int,
    frame-loss-percent: float,
    encode-bitrate-kbps: int,
    decode-peers: int,
    udp-active: bool,
    ping-ms: int,
    screen-frames-completed: int,
    screen-frames-dropped: int,
    screen-frames-timed-out: int,
    // M8
    capture-callback-median-ms: float,
    playback-callback-median-ms: float,
    audio-glitch-count: int,
}
```

- [ ] **Step 2: Map the new fields in `ui_shell::update_perf_display`**

Open `crates/ui_shell/src/lib.rs`. Find `pub fn update_perf_display(window: &MainWindow, snap: &PerfSnapshot)` (around line 68). Add the three new fields to the `PerfData { ... }` literal:

```rust
    let perf = PerfData {
        cpu_percent: snap.cpu_percent,
        memory_mb: snap.memory_mb,
        peak_memory_mb: snap.peak_memory_mb,
        uptime_secs: snap.uptime_secs as i32,
        audio_active: snap.audio_active,
        network_connected: snap.network_connected,
        dropped_frames: snap.dropped_frames as i32,
        jitter_buffer_ms: snap.jitter_buffer_ms as i32,
        frame_loss_percent: snap.frame_loss_rate * 100.0,
        encode_bitrate_kbps: snap.encode_bitrate_kbps as i32,
        decode_peers: snap.decode_peers as i32,
        udp_active: snap.udp_active,
        ping_ms: snap.ping_ms,
        screen_frames_completed: snap.screen_frames_completed as i32,
        screen_frames_dropped: snap.screen_frames_dropped as i32,
        screen_frames_timed_out: snap.screen_frames_timed_out as i32,
        // M8
        capture_callback_median_ms: snap.capture_callback_median_ms,
        playback_callback_median_ms: snap.playback_callback_median_ms,
        audio_glitch_count: snap.audio_glitch_count as i32,
    };
```

- [ ] **Step 3: Render new rows in SystemView**

Open `crates/ui_shell/ui/views/system_view.slint`. Find the existing Perf metrics card (it renders rows like "CPU: X%", "Jitter: Y ms", etc. — look for `perf.cpu-percent` or `perf.jitter-buffer-ms` usage).

Inside that existing card (or as a new card after it — match the existing style), add a subsection:

```slint
Text {
    text: "Audio callbacks";
    font-size: 13px;
    font-weight: 600;
    color: VxTheme.text-primary;
}
HorizontalLayout {
    spacing: 12px;
    Text {
        text: "Capture median";
        font-size: 12px;
        color: VxTheme.text-primary;
        horizontal-stretch: 1;
    }
    Text {
        text: root.perf.capture-callback-median-ms + " ms";
        font-size: 12px;
        color: VxTheme.text-muted;
        horizontal-alignment: right;
    }
}
HorizontalLayout {
    spacing: 12px;
    Text {
        text: "Playback median";
        font-size: 12px;
        color: VxTheme.text-primary;
        horizontal-stretch: 1;
    }
    Text {
        text: root.perf.playback-callback-median-ms + " ms";
        font-size: 12px;
        color: VxTheme.text-muted;
        horizontal-alignment: right;
    }
}
HorizontalLayout {
    spacing: 12px;
    Text {
        text: "Glitches (≥10 ms)";
        font-size: 12px;
        color: VxTheme.text-primary;
        horizontal-stretch: 1;
    }
    Text {
        text: root.perf.audio-glitch-count;
        font-size: 12px;
        color: root.perf.audio-glitch-count > 0 ? #ff6b6b : VxTheme.text-muted;
        horizontal-alignment: right;
    }
}
```

Adjust to match the existing metrics-row helper component in the file if one exists (e.g., `MetricRow { label: "..."; value: "..."; }`). Prefer reusing the existing component over hand-rolling `HorizontalLayout`s.

Check the file for an existing `MetricRow` component (grep for `MetricRow` in `system_view.slint` and `components.slint`). If present, use it:

```slint
MetricRow { label: "Capture median"; value: root.perf.capture-callback-median-ms + " ms"; }
MetricRow { label: "Playback median"; value: root.perf.playback-callback-median-ms + " ms"; }
MetricRow { label: "Glitches (≥10 ms)"; value: root.perf.audio-glitch-count; }
```

- [ ] **Step 4: Wire audio histograms into `PerfCollector` in main.rs**

Open `crates/app_desktop/src/main.rs`. Find the `rt.block_on` block that sets up audio (around line 86) where existing `p.frames_decoded = aud.metrics.frames_decoded.clone();` etc. lines live. Add three more lines inside the same block:

```rust
        p.capture_callback_hist = Some(aud.metrics.capture_callback_hist.clone());
        p.playback_callback_hist = Some(aud.metrics.playback_callback_hist.clone());
        p.callback_glitch_count = aud.metrics.callback_glitch_count.clone();
```

- [ ] **Step 5: Verify full workspace**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
cargo test --workspace --no-fail-fast -- \
  --skip live_stress \
  --skip test_create_space \
  --skip test_audio_after_leave_room \
  --skip test_channel_audio_relay \
  --skip test_authenticate_invalid_token_creates_new \
  2>&1 | awk '/test result:/ {ok+=$4; fail+=$6} END {print "passed="ok, "failed="fail}'
```
Expected: check clean, clippy ≤ 62, tests `failed=0`.

- [ ] **Step 6: Bench-check**

```
./scripts/bench-check.sh
```
Expected: exits 0 (no microbench regression). The audio_core benches should still run the existing targets; the new `Histogram` in audio_core isn't part of the bench target but its presence shouldn't affect other benches.

- [ ] **Step 7: Commit**

```bash
git add crates/ui_shell/ui/theme.slint crates/ui_shell/src/lib.rs crates/ui_shell/ui/views/system_view.slint crates/app_desktop/src/main.rs
git commit -m "feat(ui): render audio callback metrics in Perf panel"
```

---

## Task 5: Final verify + merge

- [ ] **Step 1: Workspace check**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: clean; ≤ 62.

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
Expected: `failed=0`. Passed count grows by the 9 new `Histogram` tests.

- [ ] **Step 3: Bench-check**

```
./scripts/bench-check.sh
```
Expected: exits 0.

- [ ] **Step 4: Commit manifest**

```
git log --oneline main..HEAD
```
Expected: four commits (Tasks 1 through 4).

- [ ] **Step 5: Merge to main**

```
git checkout main
git merge --ff-only feat/m8-audio-callback-histogram
git branch -d feat/m8-audio-callback-histogram
```

---

# Completion criteria

All of:

1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets` ≤ 62 warnings.
3. All non-flaky tests pass; new `Histogram` tests pass (9 in `audio_core`).
4. `scripts/bench-check.sh` exits 0.
5. `PerfSnapshot` and `PerfData` both contain the three new fields.
6. Running the client produces a Perf panel with "Audio callbacks" subsection showing capture median, playback median, glitch count.

# If something goes wrong

- **Borrow-check error when cloning `self.metrics.*` inside `AudioEngine::start_capture`**: make the clones before consuming `self` — they need to be bound before any `&mut self` operations in the same block.
- **`cpal` callback timing measurement is off**: confirm `Instant::now()` is the first statement in the outer closure (before any early-return branch inside the inner closure).
- **Glitch counter fires spuriously on first callback**: cpal's first few callbacks after `build_*_stream` can take longer due to driver init. Acceptable — documented as noise in the spec. Do not add filter logic.
- **`audio_core::Histogram` namespace collision with `signaling_server::histogram::Histogram`**: they don't collide — they live in different crates. If somehow `use *` pulls both in simultaneously, disambiguate with full paths.
- **`perf_metrics` depends on `audio_core` which depends on `shared_types` which could form a cycle if `shared_types` tried to pull in audio types**: `shared_types` intentionally has no dep on `audio_core`. `PerfSnapshot` uses plain `f32` / `u32` to carry the data.
- **UI snapshot test fails** with the new rows appearing in rendered output: update the snapshot; don't skip.
- **Median returns infinity and renders as "inf ms"**: the clamp to 999.0 in `PerfCollector::snapshot()` prevents this. If still broken, check the `.is_finite()` branch.

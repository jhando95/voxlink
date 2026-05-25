# Design — Milestone 8: Client Audio Callback Histogram

**Date:** 2026-04-20
**Status:** Approved (pending spec review)
**Scope:** Instrument both client audio callbacks (cpal capture + playback) with latency histograms, maintain a glitch counter for callbacks that exceed 10 ms, surface the numbers in the Perf panel. No optimization; pure instrumentation following the M7 pattern.

## Context

Voxlink is voice-first. Audio glitches — the little pops when a callback takes too long — are the most user-visible quality problem a voice app can have. Today the client tracks five audio metrics (`frames_decoded`, `frames_dropped`, `current_jitter_ms`, `active_peers`, `encode_bitrate_kbps`) but has no visibility into the actual callback execution time on either end.

M3 added lock-free `Histogram` latency tracking on the server side (signaling dispatch, UDP relay). The same primitive applied client-side closes the audio quality visibility gap.

## Goals

1. Measure capture and playback callback durations with per-bucket counts.
2. Expose a "glitch counter" — callbacks ≥ 10 ms, which at 48 kHz with typical buffer sizes indicates an audible dropout.
3. Surface three numbers in the Perf panel: capture median, playback median, glitch count.
4. Zero behavioral changes to audio. Pure observability.

## Non-goals

- **Opus encode/decode sub-phase breakdown.** Encode runs inside the capture callback; its cost is already captured in `capture_callback_hist`.
- **Jitter-buffer depth histogram.** Already surfaced via `current_jitter_ms`.
- **Cross-peer round-trip audio latency.** Different milestone; needs protocol support.
- **Histogram persistence across restarts.** Session-scoped, matching existing metrics.
- **Creating a shared utility crate.** Duplicating ~100 lines of `Histogram` is cleaner than a new crate.

## Architecture

### 1. `Histogram` copy in `audio_core`

New file `crates/audio_core/src/histogram.rs`. Contents: a copy of `crates/signaling_server/src/histogram.rs` with `pub(crate)` relaxed to `pub` (so `AudioMetrics` and callers can hold an `Arc<Histogram>`).

One addition versus the server version: a `pub fn median(&self) -> f64` method that walks cumulative bucket counts and returns the upper bound of the bucket where the cumulative count crosses half the total. Approximate (granularity = bucket width), but good enough for a displayed UI number.

```rust
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
    // All observations in the +Inf bucket — return something informative.
    f64::INFINITY
}
```

The two copies (signaling_server and audio_core) drift only if bucket layouts change, which we haven't needed. Accept the duplication.

### 2. `AudioMetrics` extension

Extend the existing struct in `crates/audio_core/src/lib.rs`:

```rust
pub struct AudioMetrics {
    // existing fields:
    pub frames_decoded: Arc<AtomicU32>,
    pub frames_dropped: Arc<AtomicU32>,
    pub current_jitter_ms: Arc<AtomicU32>,
    pub active_peers: Arc<AtomicU32>,
    pub encode_bitrate_kbps: Arc<AtomicU32>,
    // NEW:
    pub capture_callback_hist: Arc<Histogram>,
    pub playback_callback_hist: Arc<Histogram>,
    pub callback_glitch_count: Arc<AtomicU32>,
}
```

`impl AudioMetrics::new` initializes the two histograms with distinct names and helpful text (e.g., `"audio_capture_callback_seconds"`, `"audio_playback_callback_seconds"`).

### 3. Instrumentation sites

Two changes in `crates/audio_core/src/lib.rs`:

**Capture callback** (around `build_input_stream`, ~line 620):

```rust
move |data: &[f32], _info: &cpal::InputCallbackInfo| {
    let t0 = std::time::Instant::now();
    // ... existing body ...
    let elapsed = t0.elapsed().as_secs_f64();
    capture_hist.observe(elapsed);
    if elapsed >= 0.010 {
        glitch_count.fetch_add(1, Ordering::Relaxed);
    }
}
```

**Playback callback** (around `build_output_stream`, ~line 849): same pattern with `playback_hist.observe(elapsed)` and the same glitch increment. Share the glitch counter across capture+playback (simpler; one counter for "any audio callback glitched").

Each callback gets an `Arc<Histogram>` + `Arc<AtomicU32>` clone from `AudioMetrics` before stream construction; the closures move those clones in.

### 4. Exposure through `PerfCollector`

`crates/perf_metrics/src/lib.rs`'s `PerfCollector` already takes a bundle of `Arc` metric refs and snapshots them into `PerfSnapshot`. Extend with three new fields:

```rust
pub capture_callback_hist: Option<Arc<audio_core::Histogram>>,
pub playback_callback_hist: Option<Arc<audio_core::Histogram>>,
pub callback_glitch_count: Arc<AtomicU32>,
```

(`Option` on the histograms because `PerfCollector::new` doesn't know about audio yet — `main.rs` wires them after `AudioEngine::new()`, matching the existing pattern for `frames_decoded`, `current_jitter_ms`, etc.)

Extend `PerfSnapshot` in `shared_types`:

```rust
pub struct PerfSnapshot {
    // ... existing fields ...
    pub capture_callback_median_ms: f32,
    pub playback_callback_median_ms: f32,
    pub audio_glitch_count: u32,
}
```

`PerfCollector::snapshot()` computes the median-ms numbers by calling `.median() * 1000.0` on the histograms (or 0 if `None`).

### 5. UI rendering

Extend the Slint `PerfData` struct in `theme.slint`:

```slint
export struct PerfData {
    // ... existing fields ...
    capture-callback-median-ms: float,
    playback-callback-median-ms: float,
    audio-glitch-count: int,
}
```

In `SystemView`'s Perf card, add a new subsection:

```
Audio callbacks
  capture median    0.8 ms
  playback median   0.9 ms
  glitches          0          (callbacks ≥ 10 ms)
```

Use red text color when glitch count > 0 to make it stand out.

## Components

| File | Change |
|---|---|
| `crates/audio_core/src/histogram.rs` *(new)* | Copy of signaling_server Histogram, add `median()`, `pub` visibility |
| `crates/audio_core/src/lib.rs` | `pub mod histogram;`, `pub use histogram::Histogram;`, extend `AudioMetrics` with 3 fields, instrument both callbacks |
| `crates/audio_core/Cargo.toml` | No change — no new deps |
| `crates/perf_metrics/Cargo.toml` | Add `audio_core = { path = "../audio_core" }` dep (for the `Histogram` type reference) |
| `crates/perf_metrics/src/lib.rs` | Add 3 fields to `PerfCollector`, populate in `snapshot()` |
| `crates/shared_types/src/state.rs` | Add 3 fields to `PerfSnapshot` |
| `crates/ui_shell/ui/theme.slint` | Extend `PerfData` struct with 3 fields |
| `crates/ui_shell/src/lib.rs` | Map new `PerfSnapshot` fields into `PerfData` when pushing to window |
| `crates/ui_shell/ui/views/system_view.slint` | Render new rows |
| `crates/app_desktop/src/main.rs` | Wire audio metrics histograms into `PerfCollector` at startup (alongside existing `p.frames_decoded = ...` lines) |

Total new code: ~120 lines (mostly the copied Histogram).

## Testing

- Unit: `Histogram::median()` returns ~the right bucket for observations at known positions. Returns 0 on empty. Returns infinity when all observations land in +Inf.
- Unit: `AudioMetrics::new()` produces initialized histograms with total_count == 0 and glitch_count == 0.
- Unit: writing to the existing cargo-microbench for histogram observe still runs < 200 ns (no regression from the median method's presence).
- Manual: run the client, confirm Perf panel shows the three new rows. Confirm glitch count stays at 0 during normal operation.
- `cargo bench` to confirm no microbench regression.

## Risks

- **Two copies of `Histogram` drift.** Mitigation: both copies are small, both track the same concern (latency bucketing), and the second copy adds only a `median()` helper. If they drift, a single-line bucket-layout change will need to be mirrored — acceptable maintenance cost.
- **Callback instrumentation overhead.** Each callback gains four `AtomicU64::fetch_add(Relaxed)` calls + an `Instant::elapsed()`. Total overhead: ~100 ns. Negligible vs capture work (typically 500µs–5ms).
- **Glitch counter triggers on legitimate first-callback init.** Possible that the first output callback takes longer than 10ms while cpal initializes. Mitigation: document that "glitches early after join = noise, sustained glitches = real". Don't filter in code — users need to see the raw number.
- **`audio_core` dep on `perf_metrics` would create a cycle.** We avoid this by having `perf_metrics` depend on `audio_core` (one-way), not the reverse.

## Commit strategy

Four commits, workspace green at each:

1. `bench(audio_core): duplicate Histogram type with median() helper + tests`
2. `feat(audio_core): instrument capture + playback callback durations`
3. `feat(perf): expose audio callback medians + glitch count in PerfSnapshot`
4. `feat(ui): render audio callback metrics in Perf panel`

## Success criteria

1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets` ≤ 62 warnings.
3. All existing tests pass; new `Histogram::median` tests pass.
4. Perf panel displays three new rows when the client is running.
5. `scripts/bench-check.sh` exits 0 (microbenches unaffected).
6. Running the app, joining a room, and watching the Perf panel shows realistic callback timings on a healthy machine.

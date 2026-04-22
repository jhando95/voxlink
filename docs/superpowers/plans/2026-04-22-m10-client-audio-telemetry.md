# M10 — Client-Reported Audio Quality Telemetry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clients send `AudioQualityReport` every 10 seconds while in a voice call; server aggregates into five new Prometheus metric series exposed on `/metrics`.

**Architecture:** New `SignalMessage::AudioQualityReport` variant carries five numeric audio-health fields. Client tick loop sends via the existing `rt_handle.spawn(async { network.lock().await.send_signal(&msg).await })` pattern (same as `send_ping`), gated on a new `last_telemetry_update` wall-clock timer. Server's `dispatch.rs` match arm feeds the numbers into three new `Histogram` fields + two new `AtomicU64` counters on `ServerMetrics`. `render_metrics` emits the five series.

**Tech Stack:** Rust 1.94, serde, tokio. No new deps. Uses existing `audio_core::Histogram` (in signaling_server's copy) already added in M3.

**Spec:** `docs/superpowers/specs/2026-04-22-m10-client-audio-telemetry-design.md`

**Workspace root:** `/Users/jph/Voiceapp/workspace_template`
**Branch:** start on `feat/m10-client-audio-telemetry` from `main`.

---

## Ground rules

1. **Workspace stays green.** `cargo check --workspace` passes before each commit.
2. **No new clippy warnings.** Baseline 62; must not exceed.
3. **No new deps.**
4. **No PII in telemetry.** Five numeric fields only.

---

## Task 0: Branch

- [ ] **Step 1: Clean tree + baseline**

```
cd /Users/jph/Voiceapp/workspace_template
git status --short    # expect empty (discard Cargo.lock drift if present)
git checkout -b feat/m10-client-audio-telemetry
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: `62`.

No commit.

---

## Task 1: `AudioQualityReport` variant in `SignalMessage`

**Files:**
- Modify: `crates/shared_types/src/protocol.rs` (enum variant + variant_index + VARIANT_NAMES)
- Modify: `crates/shared_types/src/tests.rs` (serde round-trip test)

- [ ] **Step 1: Add the variant to `SignalMessage`**

Open `crates/shared_types/src/protocol.rs`. The enum is `pub enum SignalMessage { ... }` starting around line 14. Near the bottom of the enum (the last variants look like `MessageReacted { ... }`), ADD a new variant:

```rust
    // Client -> Server: periodic audio quality report for server-side aggregation.
    // All values are client-observed; server only aggregates into /metrics.
    AudioQualityReport {
        /// Client's current capture callback median in milliseconds.
        capture_callback_median_ms: u32,
        /// Client's current playback callback median in milliseconds.
        playback_callback_median_ms: u32,
        /// Delta of audio glitches since the previous report.
        glitches_delta: u32,
        /// Delta of dropped frames since the previous report.
        frames_dropped_delta: u32,
        /// Current jitter-buffer depth in milliseconds.
        jitter_buffer_ms: u32,
    },
```

Place it ALPHABETICALLY at the end of the Client→Server section, or at the very end of the enum — whichever matches the existing enum's convention.

- [ ] **Step 2: Add variant_index arm**

Still in `crates/shared_types/src/protocol.rs`, find `fn variant_index(&self) -> usize` (near the bottom of the impl block, around line 800+). The last match arm currently has the highest index (e.g., `Self::MessageReacted { .. } => 200,`). Append:

```rust
            Self::AudioQualityReport { .. } => 201,
```

(If the current highest index is not 200, use `last_existing_index + 1` — grep the match block to confirm.)

- [ ] **Step 3: Add VARIANT_NAMES entry**

Still in the same file, find `pub const VARIANT_NAMES: &'static [&'static str]`. At the end of the array, before the closing `];`, append:

```rust
        "AudioQualityReport",
```

The order MUST match variant_index — append at the same position (end).

- [ ] **Step 4: Run existing consistency tests**

```
cd /Users/jph/Voiceapp/workspace_template
cargo test -p shared_types signal_message_variant
```

Expected:
- `signal_message_variant_names_match_count` PASSES (VARIANT_NAMES.len() == SIGNAL_MESSAGE_VARIANT_COUNT).
- `signal_message_variant_index_in_bounds` PASSES.

If the names test fails, VARIANT_NAMES length doesn't match the const or the index arm — revisit steps 2-3.

- [ ] **Step 5: Add a serde round-trip test**

Open `crates/shared_types/src/tests.rs`. Append a new test:

```rust
#[test]
fn audio_quality_report_round_trips() {
    use super::SignalMessage;
    let msg = SignalMessage::AudioQualityReport {
        capture_callback_median_ms: 2,
        playback_callback_median_ms: 3,
        glitches_delta: 1,
        frames_dropped_delta: 5,
        jitter_buffer_ms: 40,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: SignalMessage = serde_json::from_str(&json).unwrap();
    match back {
        SignalMessage::AudioQualityReport {
            capture_callback_median_ms,
            playback_callback_median_ms,
            glitches_delta,
            frames_dropped_delta,
            jitter_buffer_ms,
        } => {
            assert_eq!(capture_callback_median_ms, 2);
            assert_eq!(playback_callback_median_ms, 3);
            assert_eq!(glitches_delta, 1);
            assert_eq!(frames_dropped_delta, 5);
            assert_eq!(jitter_buffer_ms, 40);
        }
        other => panic!("wrong variant after round-trip: {other:?}"),
    }
}
```

- [ ] **Step 6: Run the new test**

```
cargo test -p shared_types audio_quality_report_round_trips
```
Expected: PASS.

- [ ] **Step 7: Verify workspace**

```
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: clean; clippy ≤ 62.

- [ ] **Step 8: Commit**

```bash
git add crates/shared_types/src/protocol.rs crates/shared_types/src/tests.rs
git commit -m "feat(shared_types): add AudioQualityReport SignalMessage variant"
```

---

## Task 2: `audio_quality_numbers()` helper on `PerfCollector`

**Files:**
- Modify: `crates/perf_metrics/src/lib.rs`

- [ ] **Step 1: Add the helper method**

Open `crates/perf_metrics/src/lib.rs`. Inside `impl PerfCollector { ... }`, after `snapshot()`, append:

```rust
    /// Return the current audio quality numbers as a tuple:
    /// (capture_callback_median_ms, playback_callback_median_ms,
    ///  cumulative_glitches, cumulative_frames_dropped, jitter_buffer_ms).
    ///
    /// The caller is responsible for computing deltas for the counter fields
    /// (glitches, frames_dropped) against a cached previous value.
    ///
    /// Returns zeros for capture/playback medians if no audio has flowed yet.
    pub fn audio_quality_numbers(&self) -> (u32, u32, u32, u32, u32) {
        let capture_ms = self
            .capture_callback_hist
            .as_ref()
            .map(|h| {
                let m = h.median() * 1000.0;
                if m.is_finite() { m as u32 } else { 999 }
            })
            .unwrap_or(0);
        let playback_ms = self
            .playback_callback_hist
            .as_ref()
            .map(|h| {
                let m = h.median() * 1000.0;
                if m.is_finite() { m as u32 } else { 999 }
            })
            .unwrap_or(0);
        let glitches = self
            .callback_glitch_count
            .load(std::sync::atomic::Ordering::Relaxed);
        let frames_dropped_u64 = self
            .dropped_frames
            .load(std::sync::atomic::Ordering::Relaxed);
        let frames_dropped = u32::try_from(frames_dropped_u64).unwrap_or(u32::MAX);
        let jitter_ms = self
            .current_jitter_ms
            .load(std::sync::atomic::Ordering::Relaxed);
        (capture_ms, playback_ms, glitches, frames_dropped, jitter_ms)
    }
```

- [ ] **Step 2: Add a unit test**

In the existing `#[cfg(test)] mod tests { ... }` block, append:

```rust
    #[test]
    fn audio_quality_numbers_returns_zeros_before_any_audio() {
        let collector = PerfCollector::new();
        let (c, p, g, f, j) = collector.audio_quality_numbers();
        assert_eq!(c, 0, "capture median should be zero before any callbacks");
        assert_eq!(p, 0, "playback median should be zero before any callbacks");
        assert_eq!(g, 0, "glitches should be zero before any callbacks");
        assert_eq!(f, 0, "frames_dropped should be zero before any callbacks");
        // jitter has a non-zero default (JITTER_INITIAL * 20) from AudioMetrics::new;
        // collector on its own has no such default — it's whatever the
        // perf_metrics zero-init yields. Do not assert on this.
        let _ = j;
    }
```

- [ ] **Step 3: Run tests**

```
cd /Users/jph/Voiceapp/workspace_template
cargo test -p perf_metrics
```
Expected: all pass (existing + new test).

- [ ] **Step 4: Verify workspace**

```
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: clean; clippy ≤ 62.

- [ ] **Step 5: Commit**

```bash
git add crates/perf_metrics/src/lib.rs
git commit -m "feat(perf_metrics): add audio_quality_numbers helper"
```

---

## Task 3: Client sends `AudioQualityReport` every 10 s

**Files:**
- Modify: `crates/app_desktop/src/tick_loop/mod.rs`

**Context:** `crates/app_desktop/src/tick_loop/mod.rs` has a tick loop driven by a Slint timer. Existing `last_ping_update` (line ~125) is the pattern for wall-clock timers. Outgoing messages are sent via `rt_handle.spawn(async move { network.lock().await.send_signal(&msg).await })` — the `send_ping` helper shows the pattern.

- [ ] **Step 1: Add state for the telemetry timer and last-reported cumulative values**

In `crates/app_desktop/src/tick_loop/mod.rs`, find the block of `Rc::new(RefCell::new(...))` declarations near the top of `pub fn start(...)` (around lines 120-130, where `last_ping_update`, `last_slow_update`, etc. live). Add:

```rust
    let last_telemetry_update = Rc::new(RefCell::new(Instant::now()));
    // Cached cumulative values as of the last AudioQualityReport send.
    // Deltas reported to the server = current - last_reported_*.
    let last_reported_glitches = Rc::new(RefCell::new(0u32));
    let last_reported_frames_dropped = Rc::new(RefCell::new(0u32));
```

- [ ] **Step 2: Add the send block to the tick body**

Find the existing ping-update block (around line 748):

```rust
            // --- Ping every ~3s (wall-clock) ---
            if last_ping_update.borrow().elapsed() >= Duration::from_secs(3) {
                *last_ping_update.borrow_mut() = Instant::now();
                update_ping(&network, &rt_handle, &w, &perf);
            }
```

Immediately after it, append a similar block for telemetry:

```rust
            // --- Audio quality telemetry every 10s while in a call ---
            if in_call
                && last_telemetry_update.borrow().elapsed() >= Duration::from_secs(10)
            {
                *last_telemetry_update.borrow_mut() = Instant::now();
                send_audio_quality_report(
                    &network,
                    &rt_handle,
                    &perf,
                    &last_reported_glitches,
                    &last_reported_frames_dropped,
                );
            }
```

- [ ] **Step 3: Add the `send_audio_quality_report` helper function**

At the bottom of `crates/app_desktop/src/tick_loop/mod.rs` (after `update_ping`, before the `ListenState` struct around line 766), add:

```rust
/// Build an AudioQualityReport from the current PerfCollector snapshot and
/// ship it to the server. Computes counter deltas against cached previous
/// values; updates the caches on success of packing (not on send completion —
/// the async send may fail, but we don't retry anyway since the next report
/// will include the missed deltas).
fn send_audio_quality_report(
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
    perf: &Rc<RefCell<perf_metrics::PerfCollector>>,
    last_reported_glitches: &Rc<RefCell<u32>>,
    last_reported_frames_dropped: &Rc<RefCell<u32>>,
) {
    let (capture_ms, playback_ms, glitches_cum, frames_dropped_cum, jitter_ms) = {
        let p = perf.borrow();
        p.audio_quality_numbers()
    };

    let prev_glitches = *last_reported_glitches.borrow();
    let prev_frames = *last_reported_frames_dropped.borrow();
    let glitches_delta = glitches_cum.saturating_sub(prev_glitches);
    let frames_dropped_delta = frames_dropped_cum.saturating_sub(prev_frames);

    // Skip reporting idle periods: no audio, no losses, no glitches.
    if capture_ms == 0
        && playback_ms == 0
        && glitches_delta == 0
        && frames_dropped_delta == 0
    {
        return;
    }

    *last_reported_glitches.borrow_mut() = glitches_cum;
    *last_reported_frames_dropped.borrow_mut() = frames_dropped_cum;

    let msg = SignalMessage::AudioQualityReport {
        capture_callback_median_ms: capture_ms,
        playback_callback_median_ms: playback_ms,
        glitches_delta,
        frames_dropped_delta,
        jitter_buffer_ms: jitter_ms,
    };

    let network = network.clone();
    rt_handle.spawn(async move {
        if let Err(e) = network.lock().await.send_signal(&msg).await {
            log::debug!("Failed to send AudioQualityReport: {e}");
        }
    });
}
```

- [ ] **Step 4: Verify imports**

`SignalMessage` is already imported at the top of `tick_loop/mod.rs` (`use shared_types::{MicMode, SignalMessage};`). `Duration` and `Instant` are already imported (`use std::time::{Duration, Instant};`). No new imports needed.

- [ ] **Step 5: Verify**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: clean; clippy ≤ 62.

- [ ] **Step 6: Commit**

```bash
git add crates/app_desktop/src/tick_loop/mod.rs
git commit -m "feat(client): send AudioQualityReport every 10s while in a call"
```

---

## Task 4: Server aggregates into `/metrics`

**Files:**
- Modify: `crates/signaling_server/src/metrics_server.rs` (five new fields + render)
- Modify: `crates/signaling_server/src/dispatch.rs` (match arm for the new variant)

- [ ] **Step 1: Add five fields to `ServerMetrics`**

Open `crates/signaling_server/src/metrics_server.rs`. Find `pub(crate) struct ServerMetrics { ... }`. Append five new fields at the end (after the existing audio-related fields):

```rust
pub(crate) struct ServerMetrics {
    // ... existing fields including udp_rate_limited_total, signaling_dispatch_latency, udp_relay_latency ...
    pub(crate) udp_relay_latency: Histogram,
    // M10: aggregated from client AudioQualityReport messages
    pub(crate) client_audio_capture_callback_seconds: Histogram,
    pub(crate) client_audio_playback_callback_seconds: Histogram,
    pub(crate) client_audio_glitches_total: AtomicU64,
    pub(crate) client_audio_frames_dropped_total: AtomicU64,
    pub(crate) client_jitter_buffer_seconds: Histogram,
}
```

- [ ] **Step 2: Initialize in `Default` impl**

Find `impl Default for ServerMetrics`. In the `Self { ... }` literal, alongside the existing histograms and counters, add:

```rust
        client_audio_capture_callback_seconds: Histogram::new(
            "voxlink_client_audio_capture_callback_seconds",
            "Median capture-callback latency reported by clients (observed once per 10s report)",
        ),
        client_audio_playback_callback_seconds: Histogram::new(
            "voxlink_client_audio_playback_callback_seconds",
            "Median playback-callback latency reported by clients (observed once per 10s report)",
        ),
        client_audio_glitches_total: AtomicU64::new(0),
        client_audio_frames_dropped_total: AtomicU64::new(0),
        client_jitter_buffer_seconds: Histogram::new(
            "voxlink_client_jitter_buffer_seconds",
            "Jitter buffer depth reported by clients",
        ),
```

- [ ] **Step 3: Emit in `render_metrics`**

Find `pub(crate) async fn render_metrics(...)` in the same file. After the existing emission of `udp_rate_limited_total` (and any other UDP counters), emit the two new counters in the same style. The exact pattern depends on whether the existing code uses `writeln!` or `out.push_str(&format!(...))` — match it.

Example (adapt to prevailing style):

```rust
    let _ = writeln!(
        out,
        "# HELP voxlink_client_audio_glitches_total Audio glitches (>=10ms callbacks) reported by clients"
    );
    let _ = writeln!(out, "# TYPE voxlink_client_audio_glitches_total counter");
    let _ = writeln!(
        out,
        "voxlink_client_audio_glitches_total {}",
        metrics
            .client_audio_glitches_total
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "# HELP voxlink_client_audio_frames_dropped_total Audio frames dropped reported by clients"
    );
    let _ = writeln!(out, "# TYPE voxlink_client_audio_frames_dropped_total counter");
    let _ = writeln!(
        out,
        "voxlink_client_audio_frames_dropped_total {}",
        metrics
            .client_audio_frames_dropped_total
            .load(std::sync::atomic::Ordering::Relaxed)
    );
```

After the existing `metrics.udp_relay_latency.render(out);` call, add:

```rust
    metrics.client_audio_capture_callback_seconds.render(out);
    metrics.client_audio_playback_callback_seconds.render(out);
    metrics.client_jitter_buffer_seconds.render(out);
```

- [ ] **Step 4: Add the match arm in `dispatch.rs`**

Open `crates/signaling_server/src/dispatch.rs`. Find `pub(crate) async fn handle_signal(...)`. It contains a large `match signal { ... }` expression. At the END of the match (before the closing `}` of the match, i.e., after the last variant arm), add:

```rust
        SignalMessage::AudioQualityReport {
            capture_callback_median_ms,
            playback_callback_median_ms,
            glitches_delta,
            frames_dropped_delta,
            jitter_buffer_ms,
        } => {
            metrics
                .client_audio_capture_callback_seconds
                .observe(capture_callback_median_ms as f64 / 1000.0);
            metrics
                .client_audio_playback_callback_seconds
                .observe(playback_callback_median_ms as f64 / 1000.0);
            metrics
                .client_audio_glitches_total
                .fetch_add(glitches_delta as u64, std::sync::atomic::Ordering::Relaxed);
            metrics
                .client_audio_frames_dropped_total
                .fetch_add(frames_dropped_delta as u64, std::sync::atomic::Ordering::Relaxed);
            metrics
                .client_jitter_buffer_seconds
                .observe(jitter_buffer_ms as f64 / 1000.0);
        }
```

If the match expression is not at the end of the function but wrapped in something else, place this arm alongside other terminal arms. Grep `grep -n "Self::MessageReacted\|SignalMessage::MessageReacted" crates/signaling_server/src/dispatch.rs` to find the most recently-added variant and place this arm nearby.

- [ ] **Step 5: Verify**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: clean; clippy ≤ 62.

Common failure modes:
- **"non-exhaustive patterns: `AudioQualityReport` not covered"** in dispatch.rs: the match was exhaustive before Task 1 added the variant. The arm in step 4 closes the gap.
- **Histogram type mismatch**: `render_metrics` calls `.render(out)` on `Histogram` instances. Confirm `signaling_server::histogram::Histogram::render` exists with that signature.

- [ ] **Step 6: Run tests**

```
cargo test -p signaling_server --lib 2>&1 | tail -5
cargo test -p shared_types signal_message_variant 2>&1 | tail -5
```
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/signaling_server/src/metrics_server.rs crates/signaling_server/src/dispatch.rs
git commit -m "feat(server): aggregate AudioQualityReport into /metrics"
```

---

## Task 5: Final verify + merge

- [ ] **Step 1: Workspace check + clippy**

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
Expected: `failed=0`. Passed count grows by 2 (new `audio_quality_report_round_trips` + `audio_quality_numbers_returns_zeros_before_any_audio` tests).

- [ ] **Step 3: Bench-check**

```
./scripts/bench-check.sh
```
Expected: exits 0.

- [ ] **Step 4: Commit manifest**

```
git log --oneline main..HEAD
```
Expected: four feature commits (Tasks 1–4).

- [ ] **Step 5: Merge to main**

```
git checkout main
git merge --ff-only feat/m10-client-audio-telemetry
git branch -d feat/m10-client-audio-telemetry
```

---

# Completion criteria

All of:

1. `cargo check --workspace` clean; clippy ≤ 62.
2. All non-flaky tests pass; two new M10 tests pass.
3. `scripts/bench-check.sh` exits 0.
4. `SignalMessage::VARIANT_NAMES.len() == SIGNAL_MESSAGE_VARIANT_COUNT` (enforced by existing M3 test).
5. `AudioQualityReport` serde-round-trips correctly.
6. Running a client against a local server and then `curl http://<metrics-port>/metrics | grep voxlink_client_audio` shows all five series populated (manual verification).

# If something goes wrong

- **`variant_index` number off by one**: the last existing variant's index is whatever it is; append `last + 1`. The M3 consistency tests will catch a mismatch.
- **VARIANT_NAMES.len() mismatch with variant_index**: append the new string at the end of the array, matching the index order. Re-run `cargo test -p shared_types signal_message_variant`.
- **Frames_dropped is `AtomicU64`, not `AtomicU32`**: `audio_quality_numbers` uses `u32::try_from(...).unwrap_or(u32::MAX)` to saturate — documented in the code. If the overflow case is actually hit in practice, the counter saturates at 2^32, which is fine for a 10s-window report.
- **Server binds metrics on a different port than expected**: check `main.rs` for `PV_METRICS_ADDR`. On a fresh `cargo run`, the port is logged to stderr.
- **Signal arm is unreachable due to a catch-all `_ =>` earlier in the match**: unlikely (the dispatch match shouldn't have a catch-all) but if present, add the new arm BEFORE the catch-all.
- **`send_audio_quality_report` captures `perf` by reference but holds the borrow too long**: the helper takes `&Rc<RefCell<PerfCollector>>`, calls `.borrow()` for one tuple read, drops the borrow, then spawns the async send with only the `network` clone and the owned `msg`. This is safe — no borrow crosses an `await`.

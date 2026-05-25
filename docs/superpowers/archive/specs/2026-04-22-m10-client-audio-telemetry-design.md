# Design — Milestone 10: Client-Reported Audio Quality Telemetry

**Date:** 2026-04-22
**Status:** Approved (pending spec review)
**Scope:** Clients send a periodic `AudioQualityReport` message to the server carrying audio-health numbers already collected locally (from M8). Server aggregates into Prometheus-format metrics on `/metrics`. Operators get visibility into field quality across all connected clients, not just the server-side processing latencies from M3.

## Context

M3 added server-side Prometheus metrics (per-variant signaling counters, UDP relay histograms, etc.). M8 added client-side audio callback instrumentation (capture/playback histograms, glitch counter) surfaced in the Perf panel. What's missing: the server has no view into client-observed audio quality. An operator reading `/metrics` sees server-side latencies but cannot tell whether the clients are experiencing audio glitches.

This milestone closes that loop. It was explicitly deferred in the M3 spec:
> Client-reported audio-quality telemetry (jitter, decode errors, packet loss → server). Requires protocol additions; deserves its own milestone.

## Goals

1. Clients send an `AudioQualityReport` to the server every 10 seconds while in a voice call.
2. Server aggregates into histograms + counters exposed on `/metrics`.
3. Five new Prometheus metric series covering callback latency distribution and glitch/drop totals.
4. No new dependencies. No PII in the telemetry.

## Non-goals

- **Per-user telemetry.** Population-level aggregation only. Naming bad actors is not the goal.
- **Opt-out config toggle.** Voxlink is performance-first (per saved feedback memory); numeric anonymous telemetry is fine. A privacy toggle can land in a follow-up if users request it.
- **Client-side alerting on quality drop.** The client already shows glitches in the Perf panel (M8). Aggregate alerting is the operator's concern via `/metrics`.
- **Persisting reports or long-term history.** Process-scoped, matching existing metrics.
- **Measuring client → server → client round-trip audio latency.** Requires explicit probe packets; different milestone.

## Architecture

### 1. New `SignalMessage::AudioQualityReport` variant

```rust
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

Five `u32` fields. Serde JSON representation: ~120 bytes. One new `variant_index` entry + one new `VARIANT_NAMES` string. The existing M3 tests catch drift in either.

### 2. Client emission

In `crates/app_desktop/src/tick_loop/mod.rs`, the existing `last_ping_update` wall-clock `Instant` pattern (from M6's audit notes) is the right model for a 10-second telemetry timer. Add a sibling `last_telemetry_update: Rc<RefCell<Instant>>`.

Every tick, if `last_telemetry_update.elapsed() >= Duration::from_secs(10)` AND the client is connected AND `audio_started.borrow() == true`:

1. Take a PerfCollector snapshot (already happens elsewhere in the tick loop — share the result if it's cheap, otherwise take a fresh one).
2. Compute deltas from a `last_reported: Rc<RefCell<ReportCumulative>>` that caches the cumulative glitch / frame-drop values at the previous report.
3. Construct the `AudioQualityReport` message.
4. Push it through the existing `signal_buf` (the outgoing-message buffer that the tick loop drains).
5. Update `last_telemetry_update` and `last_reported`.

Skip the report if both `glitches_delta == 0` and `frames_dropped_delta == 0` AND capture/playback medians are both 0 — means no audio has flowed since the last report. Reduces noise at idle.

### 3. Server aggregation

Add fields to `ServerMetrics` in `crates/signaling_server/src/metrics_server.rs`:

```rust
pub(crate) client_audio_capture_callback_seconds: Histogram,
pub(crate) client_audio_playback_callback_seconds: Histogram,
pub(crate) client_audio_glitches_total: AtomicU64,
pub(crate) client_audio_frames_dropped_total: AtomicU64,
pub(crate) client_jitter_buffer_seconds: Histogram,
```

In `crates/signaling_server/src/dispatch.rs`, add a match arm for the new variant:

```rust
SignalMessage::AudioQualityReport {
    capture_callback_median_ms,
    playback_callback_median_ms,
    glitches_delta,
    frames_dropped_delta,
    jitter_buffer_ms,
} => {
    metrics.client_audio_capture_callback_seconds
        .observe(capture_callback_median_ms as f64 / 1000.0);
    metrics.client_audio_playback_callback_seconds
        .observe(playback_callback_median_ms as f64 / 1000.0);
    metrics.client_audio_glitches_total
        .fetch_add(glitches_delta as u64, Ordering::Relaxed);
    metrics.client_audio_frames_dropped_total
        .fetch_add(frames_dropped_delta as u64, Ordering::Relaxed);
    metrics.client_jitter_buffer_seconds
        .observe(jitter_buffer_ms as f64 / 1000.0);
}
```

In `render_metrics`, render the two counters alongside existing UDP counters (same style as `voxlink_udp_send_failures_total` etc.) and call `.render()` on the three histograms alongside `signaling_dispatch_latency` / `udp_relay_latency`.

### 4. Small helper on `PerfCollector`

Add a method that packages the current snapshot's audio numbers into a struct matching `AudioQualityReport`'s fields, so the tick loop is a clean one-liner:

```rust
impl PerfCollector {
    pub fn audio_quality_numbers(&self) -> (u32, u32, u32, u32, u32) {
        // (capture_ms, playback_ms, cumulative_glitches, cumulative_frames_dropped, jitter_ms)
    }
}
```

Returns cumulative values; the tick loop handles the delta math against its cached previous values.

## Components

| File | Change |
|---|---|
| `crates/shared_types/src/protocol.rs` | Add `AudioQualityReport` variant, variant_index arm, VARIANT_NAMES entry |
| `crates/perf_metrics/src/lib.rs` | `audio_quality_numbers()` helper method |
| `crates/app_desktop/src/tick_loop/mod.rs` | 10 s wall-clock timer + delta cache + send logic |
| `crates/signaling_server/src/metrics_server.rs` | Five new `ServerMetrics` fields + render |
| `crates/signaling_server/src/dispatch.rs` | Match arm that feeds the metrics |

Total: ~80 LoC of runtime code + 1 unit test (serde round trip of the new variant).

## Testing

- **Unit (shared_types):** `AudioQualityReport` serde-round-trips successfully. `SignalMessage::variant_index()` and `VARIANT_NAMES` consistency (the M3 test `signal_message_variant_names_match_count` fails automatically if either is missed).
- **Unit (perf_metrics):** `audio_quality_numbers()` returns expected zeros on a fresh collector with no audio flowing.
- **Manual verification:** run the client, join a test room, send audio for 60 s, watch the server log for `startup:` and signal-received logs, `curl http://<server>:<metrics-port>/metrics | grep voxlink_client_audio` — should show populated histograms.

## Risks

- **Old clients don't send reports.** Server receives no reports, metrics stay empty — harmless.
- **Old servers receive new reports.** Unknown variant trips the existing `malformed_signaling_messages_total` counter. Not great, but not harmful. Mitigation: we're the only ones deploying both sides; a migration sequence (deploy server first, then clients) avoids this.
- **Report flood under heavy churn.** 10-peer room × 0.1 Hz = 1 report/sec. Server rate-limit is 100/sec/peer. Safe.
- **False zero deltas hide problems.** If a client's audio breaks before the first report, the first report carries glitches since startup — not since "last report." Acceptable semantic.

## Commit strategy

Four commits, workspace green at each:

1. `feat(shared_types): add AudioQualityReport SignalMessage variant`
2. `feat(perf_metrics): add audio_quality_numbers helper`
3. `feat(client): send AudioQualityReport every 10s while in a call`
4. `feat(server): aggregate AudioQualityReport into /metrics`

## Success criteria

1. `cargo check --workspace` clean; clippy ≤ 62.
2. All existing tests pass. New serde round-trip test for `AudioQualityReport` passes. `variant_index` and `VARIANT_NAMES` consistency test still passes (automatic).
3. `scripts/bench-check.sh` exits 0.
4. With the client running and sending audio, server `/metrics` shows populated `voxlink_client_audio_capture_callback_seconds`, `voxlink_client_audio_playback_callback_seconds`, `voxlink_client_audio_glitches_total`, `voxlink_client_audio_frames_dropped_total`, `voxlink_client_jitter_buffer_seconds`.

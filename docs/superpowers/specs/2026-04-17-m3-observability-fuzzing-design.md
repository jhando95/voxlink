# Design — Milestone 3: Production Observability & Fuzz Hardening

**Date:** 2026-04-17
**Status:** Approved (pending spec review)
**Scope:** Add production-grade server observability (per-variant signaling counters, UDP reliability counters, two latency histograms) and a protocol fuzz harness. All additive, no protocol changes.

## Context

Voxlink aims to be a private, efficient Discord competitor for voice chat. Milestones 1 and 2 focused on maintainability (refactor) and deployment security (TLS). This milestone is about **production reliability** — giving an operator enough visibility to diagnose production issues, plus a fuzz harness so hostile-input bugs get found before attackers find them.

The current `/metrics` endpoint has 14 counters (`connection_attempts_total`, `active_connections`, `audio_frames_in/out_total`, etc.) and zero histograms. `signaling_messages_total` lumps all 132 `SignalMessage` variants into a single number — an operator can't tell whether traffic is audio frames, chat messages, friend-list queries, or something exotic. There's no visibility into server-side processing latency. There's no fuzz harness.

## Goals

1. **Per-variant signaling counters** so an ops dashboard can show the mix of traffic (and an attacker flooding a single variant shows up).
2. **Three UDP reliability counters** to distinguish send failures, malformed packets, and rate-limit drops.
3. **Two latency histograms** answering "is the server keeping up?" for both the signaling hot path and the UDP relay hot path.
4. **Fuzz harness** with three targets covering the most-exposed parsing surface.
5. Keep the hand-rolled Prometheus text format — no dep bloat from a Prometheus client lib.

## Non-goals

- **Client-reported audio-quality telemetry** (jitter, decode errors, packet loss → server). Requires protocol additions; deserves its own milestone.
- **Server-observed UDP loss histogram** (would require sequence numbers in audio packets). Protocol change; deferred.
- **Reconnect / unexpected-disconnect counters.** Small but starts to sprawl; deferred.
- **CI fuzz integration.** Repo has no CI config.
- **Grafana dashboard JSON.** Operator's problem.
- **Switching to a Prometheus client library** (`prometheus`, `metrics-exporter-prometheus`). Hand-rolled format is fine and avoids dep bloat.

## Architecture

### 1. Fuzz harness

New `fuzz/` directory at the workspace root, a separate cargo package that cargo-fuzz manages. Three targets:

| Target | Function fuzzed |
|---|---|
| `fuzz_signal_message` | `serde_json::from_slice::<SignalMessage>(&data)` — the primary server parser |
| `fuzz_udp_frame` | server's UDP packet parser (token extraction, packet-type dispatch) — parse arbitrary bytes, assert no panic |
| `fuzz_screen_chunk_metadata` | `decode_screen_chunk_metadata(&data)` from `shared_types::screen` |

Each target is a `fn fuzz_target!(|data: &[u8]| { ... })` that calls the parser and discards the result. The libFuzzer harness handles corpus management, crash minimization, and coverage.

Corpus and artifacts directories are gitignored. Crash reproducers (`fuzz/artifacts/<target>/crash-*`) are checked in IF any are found during development, so they become regression tests.

Documentation in `fuzz/README.md` covers `cargo +nightly fuzz run <target>`, interpreting crash reports, and updating corpus.

### 2. Per-variant signaling counters

Current state: `metrics.signaling_messages_total.fetch_add(1, Relaxed)` fires once per successful decode in `connection.rs`.

Target state: replace that with `metrics.per_message_counters[signal.variant_index()].fetch_add(1, Relaxed)` after successful decode. Keep the existing aggregate counter too, for backwards compatibility with anyone already scraping it.

Implementation:

**`SignalMessage::variant_index`** — a method returning `usize` in the range `[0, N)` where N is the variant count. Implemented as a single `match self { Variant1 => 0, Variant2 => 1, ... }`. The match has no `_` arm, so adding a variant without updating the method is a compile error.

**`SignalMessage::VARIANT_NAMES`** — a `pub const &'static [&'static str]` of length N with the variant names. Used by the metrics renderer to produce `type="..."` labels. A unit test asserts `VARIANT_COUNT == VARIANT_NAMES.len()` and that `variant_index` is unique per-variant (round-trip through a sample of each variant).

Storage in `ServerMetrics`:

```rust
pub(crate) per_message_counters: [AtomicU64; shared_types::SIGNAL_MESSAGE_VARIANT_COUNT],
```

132 × 8 bytes = 1 KB static memory. Trivial.

### 3. Three UDP reliability counters

Added to `ServerMetrics`:

- `udp_send_failures_total: AtomicU64` — incremented when `UdpSocket::send_to` returns `Err` in any of the relay functions
- `udp_invalid_packets_total: AtomicU64` — incremented when a UDP datagram is rejected before reaching the relay (unknown token, packet below minimum size, unknown packet-type byte)
- `udp_rate_limited_total: AtomicU64` — incremented when a peer's per-second fps budget is exhausted and the server drops the frame

Call sites: `crates/signaling_server/src/relay/udp.rs` and `crates/signaling_server/src/relay/{audio,screen}.rs`. Each site is a 1-liner increment next to an existing log message or early-return.

### 4. Two latency histograms

**`voxlink_signaling_dispatch_seconds`** — measures time spent processing one signaling message. Wrap the body of `handle_signal` in `connection.rs`:

```rust
let t0 = Instant::now();
handle_signal(...).await;
metrics.signaling_dispatch_latency.observe(t0.elapsed().as_secs_f64());
```

**`voxlink_udp_relay_seconds`** — measures time from UDP receive to the last `send_to`. Wrap the body of `relay_audio_udp` / `relay_screen_udp` / `run_udp_relay`'s inner handling path.

#### Histogram type

New file `crates/signaling_server/src/histogram.rs` (~80 lines):

```rust
pub(crate) struct Histogram {
    buckets: [AtomicU64; 12],     // counters per bucket (cumulative is computed at render)
    total_count: AtomicU64,
    total_sum_nanos: AtomicU64,   // sum in nanoseconds, then converted at render
    name: &'static str,
    help: &'static str,
}

// Bucket upper bounds in seconds. Last bucket is +Inf by convention.
const BOUNDS_SECS: [f64; 11] = [
    0.0005, 0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0,
];
// buckets[11] catches anything larger than BOUNDS_SECS[10].

impl Histogram {
    pub(crate) const fn new(name: &'static str, help: &'static str) -> Self { ... }
    pub(crate) fn observe(&self, value_secs: f64) { ... }
    pub(crate) fn render(&self, out: &mut String) { ... }
}
```

`observe` finds the first bucket whose upper bound ≥ value, increments that bucket, then increments `total_count` and adds `value_nanos` to `total_sum_nanos`. Lock-free hot path: four `fetch_add`s worst case.

`render` walks buckets in order emitting Prometheus histogram text:

```
# HELP voxlink_signaling_dispatch_seconds Time spent dispatching a single signal message
# TYPE voxlink_signaling_dispatch_seconds histogram
voxlink_signaling_dispatch_seconds_bucket{le="0.0005"} 1234
voxlink_signaling_dispatch_seconds_bucket{le="0.001"} 5678
...
voxlink_signaling_dispatch_seconds_bucket{le="+Inf"} 9999
voxlink_signaling_dispatch_seconds_sum 42.5
voxlink_signaling_dispatch_seconds_count 9999
```

Cumulative bucket counts: when rendering `le="0.001"`, emit the sum of `buckets[0]` + `buckets[1]`, and so on. Prometheus convention.

## Components

| File | Change |
|---|---|
| `fuzz/Cargo.toml` *(new)* | cargo-fuzz package manifest |
| `fuzz/fuzz_targets/fuzz_signal_message.rs` *(new)* | target 1 |
| `fuzz/fuzz_targets/fuzz_udp_frame.rs` *(new)* | target 2 |
| `fuzz/fuzz_targets/fuzz_screen_chunk_metadata.rs` *(new)* | target 3 |
| `fuzz/README.md` *(new)* | how to run |
| `.gitignore` | add `fuzz/corpus/`, `fuzz/target/`, `fuzz/artifacts/` (but NOT `fuzz/artifacts/<target>/crash-*` — we want reproducers checked in) |
| `crates/shared_types/src/protocol.rs` | add `impl SignalMessage { pub fn variant_index(&self) -> usize }` + `pub const VARIANT_NAMES: &[&str]` + `pub const SIGNAL_MESSAGE_VARIANT_COUNT: usize = VARIANT_NAMES.len();` (exported from `shared_types`) |
| `crates/shared_types/src/lib.rs` | re-export `SIGNAL_MESSAGE_VARIANT_COUNT` if needed at crate root |
| `crates/shared_types/src/tests.rs` | unit test: `variant_index` returns a unique value for each variant; `VARIANT_NAMES.len() == SIGNAL_MESSAGE_VARIANT_COUNT` |
| `crates/signaling_server/src/histogram.rs` *(new)* | `Histogram` type + unit tests (bucket math, render format) |
| `crates/signaling_server/src/metrics_server.rs` | add `per_message_counters`, `udp_send_failures_total`, `udp_invalid_packets_total`, `udp_rate_limited_total`, `signaling_dispatch_latency: Histogram`, `udp_relay_latency: Histogram` fields; extend `render_metrics` to emit all of them |
| `crates/signaling_server/src/connection.rs` | after successful deserialize, increment `per_message_counters[signal.variant_index()]`; wrap `handle_signal` call in the histogram timer |
| `crates/signaling_server/src/relay/udp.rs` | increment `udp_invalid_packets_total` at existing rejection sites; wrap the receive-to-forward path in the udp_relay_latency timer |
| `crates/signaling_server/src/relay/audio.rs` | increment `udp_send_failures_total` on send errors; `udp_rate_limited_total` on rate-limit drops |
| `crates/signaling_server/src/relay/screen.rs` | same |
| `crates/signaling_server/src/main.rs` | `Arc<ServerMetrics>` construction already in place; no structural change needed beyond picking up the new fields from Default |
| `crates/signaling_server/Cargo.toml` | no new deps; histogram implementation uses only std |

Total new code: ~300 lines (of which ~80 is the `Histogram` type + its tests, ~150 is the variant-index machinery + tests, ~70 is call-site instrumentation).

## Data flow

### Per-variant counter path

```
client → WebSocket frame → connection.rs deserialize
    → metrics.per_message_counters[signal.variant_index()].fetch_add(1, Relaxed)
    → metrics.signaling_messages_total.fetch_add(1, Relaxed)   // preserved for b/c
    → dispatch::handle_signal(...)
```

### UDP counter path

```
udp socket read → relay/udp.rs
  if token invalid  →  metrics.udp_invalid_packets_total++
  else if rate-limited  →  metrics.udp_rate_limited_total++
  else                   forward
    send_to err  →  metrics.udp_send_failures_total++
```

### Histogram path

```
signaling:
  t0 = Instant::now()
  dispatch::handle_signal(...).await
  metrics.signaling_dispatch_latency.observe(t0.elapsed().as_secs_f64())

udp:
  t0 = Instant::now()
  (relay inner loop / per-packet)
  metrics.udp_relay_latency.observe(t0.elapsed().as_secs_f64())
```

### Render path

`render_metrics` walks its state and emits Prometheus text. Existing counters first, then per-variant counters, then new UDP counters, then histograms.

## Testing

### Unit tests

1. **`Histogram::observe` buckets correctly** — feed values 0.0001, 0.001, 0.01, 1.5 → assert bucket counts.
2. **`Histogram::render` emits valid Prometheus format** — cumulative counts, `_sum`, `_count`, `le="+Inf"` final bucket.
3. **`SignalMessage::variant_index` is unique** — for a curated sample of every discriminator, assert each returns a distinct `usize < SIGNAL_MESSAGE_VARIANT_COUNT`. (A shotgun "construct one of each variant" test is the right shape — if a future variant needs a `Default`-like value, enrich the helper accordingly.)
4. **`VARIANT_NAMES.len() == SIGNAL_MESSAGE_VARIANT_COUNT`** — constant check.
5. **Fuzz targets build** — `cargo +nightly fuzz build` succeeds; gate is "builds on nightly". Not asserted in normal CI.

### Integration

- Existing tests must keep passing.
- Confirm `curl http://127.0.0.1:<metrics-port>/metrics` emits valid Prometheus text (manual verification documented in operator docs — no automated e2e for this).

## Risks & mitigations

- **`SignalMessage::variant_index` / `VARIANT_NAMES` drift when a new variant is added.**
  Mitigation: `variant_index` uses no `_` arm, so missing a variant is a compile error. `VARIANT_NAMES` length mismatch is caught by a unit test. A contributor adding a variant must update three places; compiler + test guide them.

- **Histogram bucket counts go `!Sync` or `!Send`.**
  Mitigation: use `AtomicU64`. Avoid any `Cell`/`RefCell`/`Rc`.

- **Hot path overhead.**
  Per-message counter increment: one `fetch_add(Relaxed)` — negligible vs JSON parse.
  Histogram observe: `Instant::elapsed()` is fast (rdtsc-equivalent on modern platforms); bucket index is a linear scan over 11 `f64`s (~20ns worst case); three `fetch_add(Relaxed)`s; total ~100ns. 50 fps voice relay × this cost is well under 1% CPU. No lock contention.

- **Fuzzing finds crashes we can't immediately fix.**
  Mitigation: check in the crash reproducer, add a commented-out `#[ignore]` unit test pointing at it, track as a regular issue. The fuzz target keeps running past the known crash via `cargo fuzz run --no-cfg-fuzzing`... actually libFuzzer halts at first crash. Workflow: run, find crash, fix, re-run, repeat. Acceptable.

- **Prometheus histogram text format typos.**
  Mitigation: the `Histogram::render` unit test parses the emitted string and asserts structure (bucket lines monotonically increasing, `_sum` and `_count` present, `le="+Inf"` final bucket).

## Commit strategy

Small commits, workspace compileable at every one:

1. `feat(shared_types): add SignalMessage::variant_index and VARIANT_NAMES`
2. `feat(signaling_server): add lock-free Histogram type`
3. `feat(metrics): per-variant signaling counters`
4. `feat(metrics): UDP reliability counters (send_failures, invalid_packets, rate_limited)`
5. `feat(metrics): signaling dispatch latency histogram`
6. `feat(metrics): UDP relay latency histogram`
7. `feat(fuzz): cargo-fuzz harness with SignalMessage, UDP frame, screen chunk targets + README`

## Success criteria

1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets` — warning count ≤ 62 (M2 baseline).
3. All existing tests pass.
4. New tests (Histogram, variant_index, VARIANT_NAMES length) pass.
5. `cargo +nightly fuzz build` succeeds for all three targets on a machine with nightly Rust installed.
6. On a running server, `curl http://localhost:<metrics-port>/metrics` shows:
   - Existing counters unchanged
   - `voxlink_signaling_messages_by_type_total{type="..."} <count>` rows for at least every variant seen since startup
   - `voxlink_udp_send_failures_total`, `voxlink_udp_invalid_packets_total`, `voxlink_udp_rate_limited_total`
   - `voxlink_signaling_dispatch_seconds_bucket{le="..."}` + `_sum` + `_count`
   - `voxlink_udp_relay_seconds_bucket{le="..."}` + `_sum` + `_count`
7. Metrics output is valid Prometheus text format (parseable by `promtool check metrics` if available).

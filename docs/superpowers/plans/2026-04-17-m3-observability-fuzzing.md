# M3 — Production Observability & Fuzz Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-variant signaling counters, three UDP reliability counters, two lock-free latency histograms, and a cargo-fuzz harness with three targets. All additive, no protocol changes.

**Architecture:** Per-variant counters are a `[AtomicU64; N]` array in `ServerMetrics` indexed by a new `SignalMessage::variant_index()` method whose compile-time exhaustiveness guarantees the index and `VARIANT_NAMES` constant stay in sync. Histograms use a small lock-free `Histogram` type (atomic bucket array + sum + count). Fuzzing lives in a sibling `fuzz/` package managed by `cargo-fuzz`.

**Tech Stack:** Rust 1.94, std-only for the metrics/histogram code (no new runtime deps), `libfuzzer-sys` for fuzz targets, nightly toolchain only for `cargo fuzz run`.

**Spec:** `docs/superpowers/specs/2026-04-17-m3-observability-fuzzing-design.md`

**Workspace root:** `/Users/jph/Voiceapp/workspace_template`
**Branch:** `refactor/m1-split` (M1+M2 haven't been merged yet; continue on the same branch until the whole stack lands).

---

## Ground rules for every task

1. **Workspace stays green.** `cargo check --workspace` must succeed before committing.
2. **No new clippy warnings.** Baseline is 62 total warnings (M2 end state). Don't exceed it.
3. **Existing tests keep passing.** Known-flaky integration tests are pre-existing (`live_stress_*`, etc.) and should be skipped via the M2 filter when validating.
4. **No new runtime deps.** `Histogram` is std-only. The fuzz package has its own deps, isolated from the workspace.
5. **Hot-path performance.** Per-message counter increment and histogram observation together must stay well under 1µs at 50fps. Use `fetch_add(Relaxed)` only. No locks. No allocations.
6. **Only touch files listed for each task.**

---

## Task 0: Baseline verification

- [ ] **Step 1: Verify clean build**

Run: `cd /Users/jph/Voiceapp/workspace_template && cargo check --workspace`
Expected: clean.

- [ ] **Step 2: Record starting clippy count**

Run: `cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"`
Expected: `62`. If different, investigate before proceeding.

- [ ] **Step 3: Count current SignalMessage variants**

Run:
```
grep -cE "^    [A-Z][A-Za-z0-9]+,$" crates/shared_types/src/protocol.rs
grep -cE "^    [A-Z][A-Za-z0-9]+ \{$" crates/shared_types/src/protocol.rs
```
Note both numbers. Sum ≈ 200+ variants. This is your target array size later. Record as `VARIANT_COUNT_AT_START`.

No commit.

---

## Task 1: `SignalMessage::variant_index` and `VARIANT_NAMES`

**Files:**
- Modify: `crates/shared_types/src/protocol.rs`
- Modify: `crates/shared_types/src/tests.rs`

**What this adds:** A zero-cost method that returns each variant's stable numeric index, a parallel `VARIANT_NAMES` array, and a compile-time-consistent `SIGNAL_MESSAGE_VARIANT_COUNT` constant. The metrics code (Task 3) will use `variant_index()` as the array index into per-variant counters.

### Critical correctness technique

Rather than hand-maintain two parallel lists (which drift), use the **"const array + exhaustive match" pattern**:

```rust
impl SignalMessage {
    pub const VARIANT_NAMES: &'static [&'static str] = &[
        "CreateRoom",
        "JoinRoom",
        // ... one per variant, IN THE SAME ORDER as the enum definition
    ];

    pub fn variant_index(&self) -> usize {
        match self {
            Self::CreateRoom { .. } => 0,
            Self::JoinRoom { .. } => 1,
            // ... one arm per variant, mapping to its index in VARIANT_NAMES
            // NO `_ =>` arm — missing a variant is a compile error
        }
    }
}

pub const SIGNAL_MESSAGE_VARIANT_COUNT: usize = SignalMessage::VARIANT_NAMES.len();
```

A unit test asserts both lists are the same length and that `variant_index` stays in `[0, COUNT)` for a sample variant of each.

### Steps

- [ ] **Step 1: List every variant name in order**

Run:
```
grep -nE "^    [A-Z][A-Za-z0-9]+(,| \{$)" crates/shared_types/src/protocol.rs | awk -F '[: ,{]+' '{print NR": "$3}'
```
This prints a numbered list of variant names in enum order. Save this list — you'll need it to build `VARIANT_NAMES` and the match arms. Expected output: something like `1: CreateRoom`, `2: JoinRoom`, `3: LeaveRoom`, …

If any variant name contains digits or underscores, the regex above won't catch it — fall back to eyeballing the file with `grep -nE "^    [A-Z]" crates/shared_types/src/protocol.rs`. Record the list.

- [ ] **Step 2: Add the implementation at the bottom of `crates/shared_types/src/protocol.rs`**

After the closing `}` of `pub enum SignalMessage { ... }`, append:

```rust
impl SignalMessage {
    /// Stable numeric index for each variant. Used by the metrics layer to
    /// index a per-variant counter array without allocating a HashMap on
    /// the hot path.
    ///
    /// IMPORTANT: the match below has no `_ =>` arm on purpose — adding a
    /// new variant forces the compiler to flag this function, and once you
    /// give the new variant an index you must also extend `VARIANT_NAMES`.
    pub fn variant_index(&self) -> usize {
        match self {
            // Paste here: "Self::<VariantName> { .. } => <N>," for each variant,
            // in the same order as VARIANT_NAMES below. Use ` { .. }` for struct
            // variants, and bare (no pattern) for unit variants.
            // For example, for a unit variant: `Self::LeaveRoom => 2,`
            Self::CreateRoom { .. } => 0,
            Self::JoinRoom { .. } => 1,
            // ... continue for all variants, one per line
        }
    }

    /// Human-readable variant names. Order must exactly match `variant_index`.
    pub const VARIANT_NAMES: &'static [&'static str] = &[
        "CreateRoom",
        "JoinRoom",
        // ... continue, same order
    ];
}

/// Number of variants in `SignalMessage`. Defined at the module level so it
/// can be used in const contexts (e.g., sizing a `[AtomicU64; N]`).
pub const SIGNAL_MESSAGE_VARIANT_COUNT: usize = SignalMessage::VARIANT_NAMES.len();
```

For the two lists, use your Step 1 output. Write out every variant:
- In `variant_index`: `Self::<Name> { .. } => <index>,` for struct variants, `Self::<Name> => <index>,` for unit variants.
- In `VARIANT_NAMES`: `"<Name>",` in the same order.

Yes, this is ~200 repetitive lines. That's the point — the repetition is what keeps the two representations aligned, enforced by the compiler.

- [ ] **Step 3: Run the build and fix any variant-pattern mistakes**

Run: `cd /Users/jph/Voiceapp/workspace_template && cargo check -p shared_types`

Likely failure modes:
- **"pattern does not mention field X"** on a struct variant: you wrote `Self::Foo { field: _ } => N` but the variant has fields you didn't list. Use `Self::Foo { .. } => N,` to ignore all fields.
- **"expected tuple struct, found struct"**: the variant is a unit variant (no braces). Use `Self::Foo => N,`.
- **"non-exhaustive match"**: you missed a variant. The compiler will name it. Add the arm.
- **Variant name not in VARIANT_NAMES**: length mismatch. Add the missing name to the array.

Keep iterating until `cargo check -p shared_types` is clean.

- [ ] **Step 4: Write the consistency test**

Open `crates/shared_types/src/tests.rs`. Append (inside the existing test module, or as a new test block if the file is a plain `#![cfg(test)]` file):

```rust
#[test]
fn signal_message_variant_names_match_count() {
    use super::{SignalMessage, SIGNAL_MESSAGE_VARIANT_COUNT};
    assert_eq!(
        SignalMessage::VARIANT_NAMES.len(),
        SIGNAL_MESSAGE_VARIANT_COUNT,
        "VARIANT_NAMES and SIGNAL_MESSAGE_VARIANT_COUNT must agree"
    );
    assert!(
        SIGNAL_MESSAGE_VARIANT_COUNT > 0,
        "expected at least one variant"
    );
}

#[test]
fn signal_message_variant_index_in_bounds() {
    use super::SignalMessage;
    // Construct a handful of variants and confirm variant_index is in range.
    // One unit variant, one struct variant is enough — the compiler already
    // guarantees variant_index is total.
    let samples: Vec<SignalMessage> = vec![
        SignalMessage::LeaveRoom,
        SignalMessage::CreateRoom {
            user_name: "test".into(),
            password: None,
        },
    ];
    for msg in samples {
        let idx = msg.variant_index();
        assert!(
            idx < SignalMessage::VARIANT_NAMES.len(),
            "variant_index {idx} out of range"
        );
        // Round-trip: the name at that index should NOT be empty.
        assert!(!SignalMessage::VARIANT_NAMES[idx].is_empty());
    }
}
```

If one of those sample constructions doesn't compile because the variant shape is different (e.g., `CreateRoom` has additional required fields), adjust the construction to match the actual enum. Keep the test's spirit: exercise `variant_index`, confirm it's in range.

- [ ] **Step 5: Run tests**

Run: `cargo test -p shared_types signal_message_variant`
Expected: 2 passed.

- [ ] **Step 6: Full workspace check**

Run: `cargo check --workspace`
Expected: clean.

- [ ] **Step 7: Clippy**

Run: `cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"`
Expected: still 62 (or fewer).

- [ ] **Step 8: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add crates/shared_types/src/protocol.rs crates/shared_types/src/tests.rs
git commit -m "feat(shared_types): add SignalMessage::variant_index + VARIANT_NAMES"
```

---

## Task 2: Lock-free `Histogram` type

**Files:**
- Create: `crates/signaling_server/src/histogram.rs`
- Modify: `crates/signaling_server/src/main.rs` (add `mod histogram;`)

**What this adds:** A reusable atomic-bucket histogram type with log-spaced buckets covering 0.5ms–1s, plus rendering as Prometheus text format. Hot-path observation is lock-free and allocation-free.

- [ ] **Step 1: Create `crates/signaling_server/src/histogram.rs`**

```rust
//! Lock-free, allocation-free histogram for Prometheus-style metrics.
//!
//! Bucket layout is log-spaced and fixed at 11 upper bounds plus a
//! sentinel `+Inf` bucket. Suitable for sub-millisecond-resolution
//! latency measurements up to 1 second.
//!
//! Observation is four `fetch_add(Relaxed)` in the worst case; render
//! walks buckets once and emits Prometheus text.

use std::sync::atomic::{AtomicU64, Ordering};

/// Bucket upper bounds, in seconds. Must be sorted ascending.
pub(crate) const BOUNDS_SECS: [f64; 11] = [
    0.0005, 0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0,
];

/// Total number of buckets including the implicit `+Inf` final bucket.
pub(crate) const BUCKET_COUNT: usize = BOUNDS_SECS.len() + 1;

/// Lock-free histogram.
pub(crate) struct Histogram {
    name: &'static str,
    help: &'static str,
    /// Per-bucket observation counts. `buckets[i]` counts observations in
    /// `(BOUNDS_SECS[i-1], BOUNDS_SECS[i]]`, with `buckets[0]` catching
    /// `(-inf, BOUNDS_SECS[0]]`, and `buckets[BUCKET_COUNT-1]` catching
    /// `(BOUNDS_SECS[last], +inf)`.
    buckets: [AtomicU64; BUCKET_COUNT],
    total_count: AtomicU64,
    total_sum_nanos: AtomicU64,
}

impl Histogram {
    pub(crate) const fn new(name: &'static str, help: &'static str) -> Self {
        // std::array::from_fn isn't `const`, so spell out 12 entries.
        // Keep length in sync with BUCKET_COUNT — unit test catches drift.
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
    pub(crate) fn observe(&self, value_secs: f64) {
        // Pick the first bucket whose upper bound >= value.
        let idx = BOUNDS_SECS
            .iter()
            .position(|&b| value_secs <= b)
            .unwrap_or(BUCKET_COUNT - 1);
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
        self.total_count.fetch_add(1, Ordering::Relaxed);
        // Cap nanos conversion to avoid weirdness on absurd inputs.
        let nanos = (value_secs.max(0.0) * 1e9) as u64;
        self.total_sum_nanos.fetch_add(nanos, Ordering::Relaxed);
    }

    /// Append Prometheus text for this histogram to `out`.
    pub(crate) fn render(&self, out: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(out, "# HELP {} {}", self.name, self.help);
        let _ = writeln!(out, "# TYPE {} histogram", self.name);
        // Prometheus histograms emit cumulative bucket counts.
        let mut cum: u64 = 0;
        for i in 0..BOUNDS_SECS.len() {
            cum = cum.saturating_add(self.buckets[i].load(Ordering::Relaxed));
            let _ = writeln!(
                out,
                "{}_bucket{{le=\"{}\"}} {}",
                self.name,
                format_float(BOUNDS_SECS[i]),
                cum
            );
        }
        // +Inf bucket = total count.
        cum = cum.saturating_add(self.buckets[BUCKET_COUNT - 1].load(Ordering::Relaxed));
        let _ = writeln!(out, "{}_bucket{{le=\"+Inf\"}} {}", self.name, cum);
        // Sum in seconds.
        let sum_secs = self.total_sum_nanos.load(Ordering::Relaxed) as f64 / 1e9;
        let _ = writeln!(out, "{}_sum {}", self.name, sum_secs);
        let _ = writeln!(out, "{}_count {}", self.name, self.total_count.load(Ordering::Relaxed));
    }
}

/// Format a float the way Prometheus expects: no trailing zeros on integers,
/// reasonable precision otherwise.
fn format_float(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.1}")
    } else {
        // Up to 6 significant digits for bucket bounds.
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_count_matches_array_literal() {
        // Keeps the hand-coded `const fn new` array literal aligned with
        // BUCKET_COUNT — if someone changes BOUNDS_SECS without updating
        // the literal, this test fails at const_new() construction time
        // (actually, the compiler would error earlier — but this guards
        // any drift between BOUNDS_SECS and logic that depends on its len).
        assert_eq!(BUCKET_COUNT, BOUNDS_SECS.len() + 1);
    }

    #[test]
    fn observe_buckets_correctly() {
        let h = Histogram::new("test_hist", "test help");
        h.observe(0.0001); // -> bucket 0 (le=0.0005)
        h.observe(0.003);  // -> bucket 3 (le=0.005)
        h.observe(0.5);    // -> bucket 9 (le=0.5)
        h.observe(5.0);    // -> +Inf bucket (index BUCKET_COUNT-1)
        assert_eq!(h.buckets[0].load(Ordering::Relaxed), 1);
        assert_eq!(h.buckets[3].load(Ordering::Relaxed), 1);
        assert_eq!(h.buckets[9].load(Ordering::Relaxed), 1);
        assert_eq!(h.buckets[BUCKET_COUNT - 1].load(Ordering::Relaxed), 1);
        assert_eq!(h.total_count.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn render_emits_valid_prometheus_format() {
        let h = Histogram::new("voxlink_test_seconds", "test latency");
        h.observe(0.001);
        h.observe(0.02);
        let mut out = String::new();
        h.render(&mut out);

        // Sanity checks on the shape.
        assert!(out.contains("# HELP voxlink_test_seconds test latency"));
        assert!(out.contains("# TYPE voxlink_test_seconds histogram"));
        assert!(out.contains("voxlink_test_seconds_bucket{le=\"0.0005\"}"));
        assert!(out.contains("voxlink_test_seconds_bucket{le=\"+Inf\"}"));
        assert!(out.contains("voxlink_test_seconds_sum "));
        assert!(out.contains("voxlink_test_seconds_count 2"));

        // Cumulative: the final (+Inf) bucket count >= any earlier one.
        let inf_line = out.lines().find(|l| l.contains("le=\"+Inf\"")).unwrap();
        let inf_count: u64 = inf_line.rsplit(' ').next().unwrap().parse().unwrap();
        assert_eq!(inf_count, 2);
    }

    #[test]
    fn observe_does_not_panic_on_negative() {
        let h = Histogram::new("t", "t");
        h.observe(-1.0);
        // Goes to bucket 0 (since -1 <= 0.0005), sum clamped to 0.
        assert_eq!(h.buckets[0].load(Ordering::Relaxed), 1);
        assert_eq!(h.total_sum_nanos.load(Ordering::Relaxed), 0);
    }
}
```

- [ ] **Step 2: Wire `mod histogram;` into `main.rs`**

Open `crates/signaling_server/src/main.rs`. Find the block of `mod` declarations near the top (the one that includes `mod tls;`, `mod connection;`, `mod dispatch;`, etc.). Add:

```rust
mod histogram;
```

No re-export needed; the histogram type is only used from `metrics_server.rs` internally.

- [ ] **Step 3: Run tests and full check**

```
cd /Users/jph/Voiceapp/workspace_template
cargo test -p signaling_server histogram::
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: 4 histogram tests pass, workspace clean, warning count ≤ 62.

- [ ] **Step 4: Commit**

```bash
git add crates/signaling_server/src/histogram.rs crates/signaling_server/src/main.rs
git commit -m "feat(signaling_server): add lock-free Histogram type for metrics"
```

---

## Task 3: Per-variant signaling counters

**Files:**
- Modify: `crates/signaling_server/src/metrics_server.rs`
- Modify: `crates/signaling_server/src/connection.rs`

**What this adds:** `ServerMetrics` gains a `per_message_counters: [AtomicU64; SIGNAL_MESSAGE_VARIANT_COUNT]` field. After successful deserialization in `connection.rs`, the counter at `signal.variant_index()` is incremented. The `render_metrics` function emits a `voxlink_signaling_messages_by_type_total{type="<name>"} <count>` line per variant.

- [ ] **Step 1: Modify `ServerMetrics` in `metrics_server.rs`**

Open `crates/signaling_server/src/metrics_server.rs`. Find `pub(crate) struct ServerMetrics { ... }` and `impl Default for ServerMetrics { ... }`.

In the struct, ADD below the existing fields, before `started_at`:

```rust
    /// One counter per SignalMessage variant, indexed by
    /// `SignalMessage::variant_index()`. Array size is
    /// `shared_types::SIGNAL_MESSAGE_VARIANT_COUNT`.
    pub(crate) per_message_counters:
        [AtomicU64; shared_types::SIGNAL_MESSAGE_VARIANT_COUNT],
```

In `Default`, ADD inside the `Self { ... }` literal (before `started_at`):

```rust
            per_message_counters: std::array::from_fn(|_| AtomicU64::new(0)),
```

Add `use shared_types::SIGNAL_MESSAGE_VARIANT_COUNT;` at the top of the file if it isn't already there (otherwise the struct field reference must be fully qualified; either way works).

- [ ] **Step 2: Increment the per-variant counter in `connection.rs`**

Open `crates/signaling_server/src/connection.rs`. Find the site where a successful `serde_json::from_str::<SignalMessage>` (or similar deserialize) result is handled — there's already a `metrics.signaling_messages_total.fetch_add(1, Ordering::Relaxed)` nearby; you want to land right next to it.

Add (immediately after the existing `signaling_messages_total` fetch_add):

```rust
                        metrics
                            .per_message_counters[signal.variant_index()]
                            .fetch_add(1, Ordering::Relaxed);
```

Replace `signal` with whatever the local binding for the deserialized message is named — grep for `signaling_messages_total` in this file to find the spot.

- [ ] **Step 3: Emit the per-variant lines in `render_metrics`**

Back in `metrics_server.rs`, find the `render_metrics` function. Locate where it writes lines for the existing counters (there's a sequence of `writeln!(out, "# HELP ..."); writeln!(out, "# TYPE ..."); writeln!(out, "... {}", metrics.xxx.load(Relaxed))` blocks).

After all existing counters but before histograms/TLS-status blocks (i.e., wherever makes sense in the flow), add:

```rust
    use std::fmt::Write as _;
    let _ = writeln!(
        out,
        "# HELP voxlink_signaling_messages_by_type_total Signaling messages received, broken down by SignalMessage variant"
    );
    let _ = writeln!(
        out,
        "# TYPE voxlink_signaling_messages_by_type_total counter"
    );
    for (i, name) in shared_types::SignalMessage::VARIANT_NAMES.iter().enumerate() {
        let count = metrics.per_message_counters[i].load(std::sync::atomic::Ordering::Relaxed);
        // Skip variants that have never been seen — keeps output small.
        if count > 0 {
            let _ = writeln!(
                out,
                "voxlink_signaling_messages_by_type_total{{type=\"{name}\"}} {count}"
            );
        }
    }
```

(If `render_metrics` builds its output using a different writer pattern — say, direct `push_str` into a `String` — adapt the `writeln!` calls to match existing style.)

- [ ] **Step 4: Verify compilation**

Run: `cd /Users/jph/Voiceapp/workspace_template && cargo check --workspace`
Expected: clean.

Common failure: `SIGNAL_MESSAGE_VARIANT_COUNT` not re-exported from `shared_types::lib`. If you get "cannot find type", open `crates/shared_types/src/lib.rs` and confirm there's a `pub use protocol::*;` or an explicit `pub use protocol::SIGNAL_MESSAGE_VARIANT_COUNT;` — the `pub mod protocol` + `pub use protocol::*;` pattern from M1 should have it covered automatically. If not, add the explicit re-export.

- [ ] **Step 5: Manual smoke test (optional but recommended)**

Start the server on a free port, hit its metrics endpoint, confirm shape:

```
cd /Users/jph/Voiceapp/workspace_template
PV_ADDR=127.0.0.1:19090 PV_METRICS_ADDR=127.0.0.1:19091 cargo run -p signaling_server &
sleep 2
curl -s http://127.0.0.1:19091/metrics | grep -E "^# (HELP|TYPE) voxlink_signaling_messages_by_type_total|^voxlink_signaling_messages_by_type_total"
kill %1
```

Expected: you see the HELP and TYPE lines. No rows yet (no traffic has arrived). If the server doesn't have a `PV_METRICS_ADDR` env, the metrics endpoint may be on a hardcoded port — read `main.rs` to find it, or just skip this smoke test and rely on the unit-test-level gates.

- [ ] **Step 6: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add crates/signaling_server/src/metrics_server.rs crates/signaling_server/src/connection.rs
git commit -m "feat(metrics): per-variant signaling counters"
```

---

## Task 4: UDP reliability counters

**Files:**
- Modify: `crates/signaling_server/src/metrics_server.rs`
- Modify: `crates/signaling_server/src/relay/udp.rs`
- Modify: `crates/signaling_server/src/relay/audio.rs`
- Modify: `crates/signaling_server/src/relay/screen.rs`

**What this adds:** Three new counters:
- `udp_send_failures_total` — bumped when `UdpSocket::send_to` returns `Err`
- `udp_invalid_packets_total` — bumped on parse failure / unknown token / bad header
- `udp_rate_limited_total` — bumped when a peer exceeds fps budget

- [ ] **Step 1: Add the three fields to `ServerMetrics`**

In `crates/signaling_server/src/metrics_server.rs`, inside the struct, add (alongside existing `udp_frames_in_total` and `udp_frames_out_total`):

```rust
    pub(crate) udp_send_failures_total: AtomicU64,
    pub(crate) udp_invalid_packets_total: AtomicU64,
    pub(crate) udp_rate_limited_total: AtomicU64,
```

In `Default`, add their initialization:

```rust
            udp_send_failures_total: AtomicU64::new(0),
            udp_invalid_packets_total: AtomicU64::new(0),
            udp_rate_limited_total: AtomicU64::new(0),
```

- [ ] **Step 2: Emit the three lines in `render_metrics`**

In the same file, in the function that builds the metrics text, next to the `udp_frames_in_total` / `udp_frames_out_total` emission block, add:

```rust
    let _ = writeln!(
        out,
        "# HELP voxlink_udp_send_failures_total UDP datagrams the server failed to send"
    );
    let _ = writeln!(out, "# TYPE voxlink_udp_send_failures_total counter");
    let _ = writeln!(
        out,
        "voxlink_udp_send_failures_total {}",
        metrics.udp_send_failures_total.load(std::sync::atomic::Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "# HELP voxlink_udp_invalid_packets_total UDP datagrams rejected due to invalid headers or unknown tokens"
    );
    let _ = writeln!(out, "# TYPE voxlink_udp_invalid_packets_total counter");
    let _ = writeln!(
        out,
        "voxlink_udp_invalid_packets_total {}",
        metrics.udp_invalid_packets_total.load(std::sync::atomic::Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "# HELP voxlink_udp_rate_limited_total UDP datagrams dropped because the peer exceeded its fps budget"
    );
    let _ = writeln!(out, "# TYPE voxlink_udp_rate_limited_total counter");
    let _ = writeln!(
        out,
        "voxlink_udp_rate_limited_total {}",
        metrics.udp_rate_limited_total.load(std::sync::atomic::Ordering::Relaxed)
    );
```

(Adapt `writeln!` / `push_str` to existing style.)

- [ ] **Step 3: Instrument `relay/udp.rs`**

Open `crates/signaling_server/src/relay/udp.rs`. This file handles the top-level UDP receive loop (`run_udp_relay`).

Find the places where a packet is rejected:
- Bad/unknown UDP session token → `metrics.udp_invalid_packets_total.fetch_add(1, Relaxed)`
- Packet below minimum size (e.g., shorter than `UDP_SESSION_TOKEN_LEN`) → same
- Unknown packet-type byte → same

For each such early-return / early-continue site, add a `metrics.udp_invalid_packets_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);` before the `continue` / `return`.

For any `send_to` call that you can see in this file (if any), wrap in an error branch that bumps `udp_send_failures_total`. Most likely `send_to` calls live in `relay/audio.rs` and `relay/screen.rs` — do those in the next steps.

- [ ] **Step 4: Instrument `relay/audio.rs`**

Open `crates/signaling_server/src/relay/audio.rs`. Find `relay_audio_udp` (and `relay_audio` if it does WS-based sends — ignore the WS one for this task, we're only counting UDP).

For every `send_to(...).await` (or equivalent), wrap the result:

```rust
if let Err(_e) = udp.send_to(&packet, addr).await {
    metrics.udp_send_failures_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}
```

If the code is currently `let _ = udp.send_to(&packet, addr).await;` (ignoring errors), replace the `_` with the error-capture pattern above. Leave the control flow unchanged — we don't want to start propagating these errors, just count them.

If there's a rate-limit check (look for `udp_frames_in_total` / fps budget checks), at the branch that drops the frame, add:

```rust
metrics.udp_rate_limited_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
```

- [ ] **Step 5: Instrument `relay/screen.rs`**

Same pattern as audio: every `send_to` Err path increments `udp_send_failures_total`; every rate-limit drop (e.g., per-peer screen fps) increments `udp_rate_limited_total`.

- [ ] **Step 6: Verify**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: clean; warning count ≤ 62.

- [ ] **Step 7: Commit**

```bash
git add crates/signaling_server/src/metrics_server.rs crates/signaling_server/src/relay
git commit -m "feat(metrics): UDP reliability counters (send_failures, invalid_packets, rate_limited)"
```

---

## Task 5: Signaling dispatch latency histogram

**Files:**
- Modify: `crates/signaling_server/src/metrics_server.rs`
- Modify: `crates/signaling_server/src/connection.rs`

**What this adds:** `ServerMetrics` gains `signaling_dispatch_latency: Histogram`. `connection.rs` wraps the call to `dispatch::handle_signal` with an `Instant::now()` / elapsed measurement.

- [ ] **Step 1: Add the histogram field**

In `crates/signaling_server/src/metrics_server.rs`:

Add `use crate::histogram::Histogram;` at the top if not already present.

In the `ServerMetrics` struct, add:

```rust
    pub(crate) signaling_dispatch_latency: Histogram,
```

In `Default`:

```rust
            signaling_dispatch_latency: Histogram::new(
                "voxlink_signaling_dispatch_seconds",
                "Time spent dispatching a single signal message",
            ),
```

- [ ] **Step 2: Emit in `render_metrics`**

In the same file, at the bottom of `render_metrics` (after all counters), add:

```rust
    metrics.signaling_dispatch_latency.render(out);
```

Replace `out` with whatever the local String/&mut String binding is named in that function.

- [ ] **Step 3: Instrument in `connection.rs`**

Open `crates/signaling_server/src/connection.rs`. Find the call to `dispatch::handle_signal` (or `crate::dispatch::handle_signal`). Wrap it:

Before the call:

```rust
                    let _t0 = std::time::Instant::now();
```

After the call (before any follow-up code):

```rust
                    let _elapsed = _t0.elapsed();
                    metrics
                        .signaling_dispatch_latency
                        .observe(_elapsed.as_secs_f64());
```

If the call is inside a loop or conditional, keep the `t0`/`observe` pairing tight — the measurement should cover only the dispatch itself, not the deserialization or the per-variant counter bump.

Rename `_t0` to `t0` (drop the underscore) since you're using it; the underscore was just to keep the snippet compilable out of context.

- [ ] **Step 4: Verify**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: clean; warning count ≤ 62.

- [ ] **Step 5: Commit**

```bash
git add crates/signaling_server/src/metrics_server.rs crates/signaling_server/src/connection.rs
git commit -m "feat(metrics): signaling dispatch latency histogram"
```

---

## Task 6: UDP relay latency histogram

**Files:**
- Modify: `crates/signaling_server/src/metrics_server.rs`
- Modify: `crates/signaling_server/src/relay/udp.rs`
- Possibly: `crates/signaling_server/src/relay/audio.rs` and `relay/screen.rs` (depends on where the hot path actually sits — pick ONE place to time)

**What this adds:** `ServerMetrics` gains `udp_relay_latency: Histogram`. The UDP relay records receive-to-send elapsed time per packet.

- [ ] **Step 1: Add the field**

In `metrics_server.rs`:

Struct:
```rust
    pub(crate) udp_relay_latency: Histogram,
```

Default:
```rust
            udp_relay_latency: Histogram::new(
                "voxlink_udp_relay_seconds",
                "Time from UDP packet receive to final send_to call",
            ),
```

Render (below the signaling histogram):
```rust
    metrics.udp_relay_latency.render(out);
```

- [ ] **Step 2: Locate the UDP hot path**

In `relay/udp.rs`, find `run_udp_relay` — it's the main receive loop. Inside its `loop`, after each successful `recv_from` (or equivalent), there's a block that (a) validates the token, (b) identifies the packet type, (c) calls into `relay_audio_udp` or `relay_screen_udp`. That dispatch is the span we want to measure: receive → final send.

- [ ] **Step 3: Wrap the relay dispatch**

At the top of the per-packet handling block (just after `recv_from` returns Ok and before token parsing), add:

```rust
            let t0 = std::time::Instant::now();
```

Then, after the relay call returns (wherever the per-packet processing ends — BEFORE the `continue` or loop iteration end), add:

```rust
            metrics.udp_relay_latency.observe(t0.elapsed().as_secs_f64());
```

If the relay dispatch is an `if/else if/else` over packet types, put the `observe` call at the bottom of each arm's successful path, OR refactor so the common exit is at the bottom of the iteration — whichever is cleaner. Prefer a single `observe` call at the iteration bottom if possible.

- [ ] **Step 4: Verify**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: clean; warning count ≤ 62.

- [ ] **Step 5: Commit**

```bash
git add crates/signaling_server/src/metrics_server.rs crates/signaling_server/src/relay
git commit -m "feat(metrics): UDP relay latency histogram"
```

---

## Task 7: cargo-fuzz harness

**Files:**
- Create: `fuzz/Cargo.toml`
- Create: `fuzz/fuzz_targets/fuzz_signal_message.rs`
- Create: `fuzz/fuzz_targets/fuzz_udp_frame.rs`
- Create: `fuzz/fuzz_targets/fuzz_screen_chunk_metadata.rs`
- Create: `fuzz/README.md`
- Modify: `.gitignore`
- Modify: `Cargo.toml` (workspace root — ADD `fuzz` to the `exclude` list so the workspace doesn't try to build it under stable)

**What this adds:** A standalone `fuzz/` cargo package, ignored by the workspace, with three libFuzzer targets.

- [ ] **Step 1: Exclude `fuzz/` from the workspace**

Open `Cargo.toml` at the workspace root. Find the `[workspace]` table. Add (or extend):

```toml
exclude = ["fuzz"]
```

If there's already an `exclude` array, add `"fuzz"` to it. This keeps `cargo build --workspace` from trying to compile the fuzz package (which requires `-Z sanitizer` on nightly).

- [ ] **Step 2: Create `fuzz/Cargo.toml`**

```toml
[package]
name = "voxlink-fuzz"
version = "0.0.0"
edition = "2024"
publish = false

# This crate is intentionally outside the workspace. See ../Cargo.toml.
[package.metadata]
cargo-fuzz = true

[dependencies]
libfuzzer-sys = "0.4"
shared_types = { path = "../crates/shared_types" }

[[bin]]
name = "fuzz_signal_message"
path = "fuzz_targets/fuzz_signal_message.rs"
test = false
doc = false

[[bin]]
name = "fuzz_udp_frame"
path = "fuzz_targets/fuzz_udp_frame.rs"
test = false
doc = false

[[bin]]
name = "fuzz_screen_chunk_metadata"
path = "fuzz_targets/fuzz_screen_chunk_metadata.rs"
test = false
doc = false
```

Note: `edition = "2024"` — match whatever the workspace uses. Check `crates/shared_types/Cargo.toml` for the edition line and copy.

- [ ] **Step 3: Create `fuzz/fuzz_targets/fuzz_signal_message.rs`**

```rust
#![no_main]

use libfuzzer_sys::fuzz_target;
use shared_types::SignalMessage;

fuzz_target!(|data: &[u8]| {
    // The parser should never panic on arbitrary input, only return Err.
    let _ = serde_json::from_slice::<SignalMessage>(data);
});
```

- [ ] **Step 4: Create `fuzz/fuzz_targets/fuzz_udp_frame.rs`**

The server's UDP parser sits inside the signaling_server crate, so this target has to emulate its input-shape validation. Since we can't easily link `signaling_server::relay::udp::run_udp_relay` (it's private and async), the target focuses on the **parsing surface** that lives in `shared_types`:

```rust
#![no_main]

use libfuzzer_sys::fuzz_target;
use shared_types::{MEDIA_PACKET_AUDIO, UDP_KEEPALIVE, UDP_SESSION_TOKEN_LEN};

fuzz_target!(|data: &[u8]| {
    // Model what the server's UDP receive loop does in its input-validation
    // prefix: require a minimum size, pluck a session token, dispatch by
    // packet-type byte.
    if data.len() < UDP_SESSION_TOKEN_LEN + 1 {
        return;
    }
    let _token = &data[..UDP_SESSION_TOKEN_LEN];
    let packet_type = data[UDP_SESSION_TOKEN_LEN];
    let _payload = &data[UDP_SESSION_TOKEN_LEN + 1..];
    // Exhaust the known packet-type branches. The real server code has
    // equivalent logic in relay/udp.rs; this target covers the same
    // decision surface without requiring async runtime setup.
    match packet_type {
        UDP_KEEPALIVE | MEDIA_PACKET_AUDIO => {},
        _ => {},
    }
});
```

This is deliberately shallow — it covers the slicing arithmetic that, if miscalculated, could panic. If a future refactor exposes a pure `parse_udp_frame(data) -> Result<...>` from the server crate, swap this target body for a direct call.

- [ ] **Step 5: Create `fuzz/fuzz_targets/fuzz_screen_chunk_metadata.rs`**

```rust
#![no_main]

use libfuzzer_sys::fuzz_target;
use shared_types::decode_screen_chunk_metadata;

fuzz_target!(|data: &[u8]| {
    // Must never panic on arbitrary input.
    let _ = decode_screen_chunk_metadata(data);
});
```

- [ ] **Step 6: Create `fuzz/README.md`**

```markdown
# Voxlink Fuzz Targets

Protocol-parsing fuzz targets driven by [`cargo-fuzz`](https://rust-fuzz.github.io/book/cargo-fuzz.html) and libFuzzer.

## Prerequisites

- Rust nightly toolchain: `rustup install nightly`
- cargo-fuzz: `cargo install cargo-fuzz`

## Targets

| Target | Fuzzes |
|---|---|
| `fuzz_signal_message` | `serde_json::from_slice::<SignalMessage>` — the server's top-level signaling parser |
| `fuzz_udp_frame` | UDP packet receive path — session token extraction, packet-type dispatch |
| `fuzz_screen_chunk_metadata` | `decode_screen_chunk_metadata` from `shared_types::screen` |

## Running

From the repository root:

```
cd fuzz
cargo +nightly fuzz run fuzz_signal_message
```

To limit runtime (e.g., for CI-style smoke tests):

```
cargo +nightly fuzz run fuzz_signal_message -- -max_total_time=60
```

## If a crash is found

1. libFuzzer stops and writes the input to `fuzz/artifacts/<target>/crash-<hash>`.
2. Check the reproducer into git alongside the fix: `git add fuzz/artifacts/<target>/crash-<hash>`.
3. Fix the bug. Re-run the target to confirm.
4. On any future run, libFuzzer replays checked-in crashes first — so fixed bugs act as regression tests.

## What to do if cargo-fuzz itself fails to build

It needs nightly Rust and `-Z sanitizer` support. On Apple Silicon specifically, AddressSanitizer may need an override — see the cargo-fuzz docs.
```

- [ ] **Step 7: Update `.gitignore`**

Open the repo-root `.gitignore`. Add at the bottom:

```
# cargo-fuzz
fuzz/target/
fuzz/corpus/
# We DO want to keep fuzz/artifacts/ as regression reproducers, so it's NOT listed here.
```

If `.gitignore` doesn't exist, create it with just those lines.

- [ ] **Step 8: Verify the workspace still builds under stable**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
```
Expected: clean. The `fuzz/` package is NOT compiled here because the workspace `exclude` skips it.

- [ ] **Step 9: (Optional) verify the fuzz package builds on nightly**

If you have nightly installed:

```
rustup install nightly 2>/dev/null || true
if rustup toolchain list | grep -q nightly; then
    cd fuzz && cargo +nightly fuzz build
else
    echo "Nightly not installed; skipping fuzz build verification."
fi
```
Expected (if nightly present): all three targets compile. Each is a separate binary.

If you don't have nightly, that's OK — the fuzz harness is opt-in and the stable workspace build is what we gate on.

- [ ] **Step 10: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add fuzz/ .gitignore Cargo.toml
git commit -m "feat(fuzz): cargo-fuzz harness for SignalMessage, UDP frame, screen chunk metadata"
```

---

## Task 8: Final verification

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
Expected: ≤ 62 (the M2 post-M1 baseline).

- [ ] **Step 3: Run new + existing non-flaky tests**

```
cargo test --workspace --no-fail-fast -- \
  --skip live_stress \
  --skip test_create_space \
  --skip test_audio_after_leave_room \
  --skip test_channel_audio_relay \
  --skip test_authenticate_invalid_token_creates_new \
  2>&1 | awk '/test result:/ {ok+=$4; fail+=$6} END {print "passed="ok, "failed="fail}'
```
Expected: `failed=0`. `passed` count = M2 baseline (~388) + new tests from Tasks 1+2 (6 new tests: 2 for `variant_index`, 4 for `Histogram`).

- [ ] **Step 4: Smoke-test metrics output**

Start the server on a free port, send no traffic, hit `/metrics`:

```
cd /Users/jph/Voiceapp/workspace_template
PV_ADDR=127.0.0.1:29090 cargo build -p signaling_server
# Find the metrics endpoint env var / port — check main.rs for something like
# PV_METRICS_ADDR or a derived port. If none, the endpoint may be embedded.
```

Confirm the output contains (grep for):
- `voxlink_signaling_dispatch_seconds_bucket` (histogram)
- `voxlink_signaling_dispatch_seconds_sum`
- `voxlink_udp_relay_seconds_bucket`
- `voxlink_udp_relay_seconds_sum`
- `voxlink_udp_send_failures_total`
- `voxlink_udp_invalid_packets_total`
- `voxlink_udp_rate_limited_total`

Per-variant lines won't appear until traffic flows through — that's expected.

- [ ] **Step 5: (Optional) validate Prometheus format**

If `promtool` is installed (from the Prometheus distribution):

```
curl -s http://127.0.0.1:<metrics-port>/metrics | promtool check metrics
```
Expected: exit 0. If promtool isn't installed, skip.

- [ ] **Step 6: Commit manifest**

```
git log --oneline a805569..HEAD
```
Expected: 7 commits (Tasks 1 through 7).

No commit at this step — it's a review gate.

---

# Completion criteria

All of:

1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets` ≤ 62 warnings.
3. All non-flaky tests pass; new tests (Histogram, variant_index, VARIANT_NAMES) pass.
4. On a machine with nightly Rust + cargo-fuzz: `cd fuzz && cargo +nightly fuzz build` succeeds for all three targets.
5. `/metrics` endpoint emits per-variant counters (for variants that have been seen), three new UDP counters, and two histograms in valid Prometheus text format.
6. Every commit leaves the workspace green.

# If something goes wrong

- **`variant_index` match is incomplete after enum edit:** compiler error will name the missing variant. Add the arm + extend `VARIANT_NAMES`.
- **`SIGNAL_MESSAGE_VARIANT_COUNT` not visible from `signaling_server`:** add `pub use protocol::SIGNAL_MESSAGE_VARIANT_COUNT;` to `crates/shared_types/src/lib.rs`. The existing `pub use protocol::*;` pattern from M1 should cover it, but an explicit re-export is safe.
- **Histogram bucket literal has wrong count:** compiler error on `const fn new` — fix the literal's length to 12.
- **`cargo +nightly fuzz build` fails with "unstable feature":** confirm nightly is current — `rustup update nightly`.
- **Clippy warning count increased:** inspect new warnings. Most likely culprits: `clippy::too_many_arguments` on a histogram constructor (use `#[allow(clippy::too_many_arguments)]` sparingly with a justifying comment), or `clippy::wildcard_imports` if you used `use super::*;` in a test. Fix, don't suppress unless justified.
- **Integration test can't find metrics endpoint:** look in `main.rs` for the metrics binder — there's a specific env var or default port. Update the smoke-test command. Don't change the server behavior.

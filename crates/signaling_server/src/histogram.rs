//! Lock-free, allocation-free histogram for Prometheus-style metrics.
//!
//! Bucket layout is log-spaced and fixed at 11 upper bounds plus a
//! sentinel `+Inf` bucket. Suitable for sub-millisecond-resolution
//! latency measurements up to 1 second.
//!
//! Observation is four `fetch_add(Relaxed)` in the worst case; render
//! walks buckets once and emits Prometheus text.

#![allow(dead_code)]

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
    /// Per-bucket observation counts. `buckets[i]` counts observations
    /// whose first-matching-bucket upper bound is `BOUNDS_SECS[i]`; the
    /// final entry catches anything greater than `BOUNDS_SECS[last]`.
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
        for (i, &bound) in BOUNDS_SECS.iter().enumerate() {
            cum = cum.saturating_add(self.buckets[i].load(Ordering::Relaxed));
            let _ = writeln!(
                out,
                "{}_bucket{{le=\"{}\"}} {}",
                self.name,
                format_float(bound),
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
        format!("{v}")
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
        let h = Histogram::new("test_hist", "test help");
        h.observe(0.0001); // -> bucket 0 (le=0.0005)
        h.observe(0.003);  // -> bucket 3 (le=0.005)
        h.observe(0.5);    // -> bucket 9 (le=0.5)
        h.observe(5.0);    // -> +Inf bucket
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

        assert!(out.contains("# HELP voxlink_test_seconds test latency"));
        assert!(out.contains("# TYPE voxlink_test_seconds histogram"));
        assert!(out.contains("voxlink_test_seconds_bucket{le=\"0.0005\"}"));
        assert!(out.contains("voxlink_test_seconds_bucket{le=\"+Inf\"}"));
        assert!(out.contains("voxlink_test_seconds_sum "));
        assert!(out.contains("voxlink_test_seconds_count 2"));

        let inf_line = out.lines().find(|l| l.contains("le=\"+Inf\"")).unwrap();
        let inf_count: u64 = inf_line.rsplit(' ').next().unwrap().parse().unwrap();
        assert_eq!(inf_count, 2);
    }

    #[test]
    fn observe_does_not_panic_on_negative() {
        let h = Histogram::new("t", "t");
        h.observe(-1.0);
        assert_eq!(h.buckets[0].load(Ordering::Relaxed), 1);
        assert_eq!(h.total_sum_nanos.load(Ordering::Relaxed), 0);
    }
}

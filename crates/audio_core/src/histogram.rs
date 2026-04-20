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
    #[allow(dead_code)]
    name: &'static str,
    #[allow(dead_code)]
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
        for _ in 0..10 {
            h.observe(0.003);
        }
        assert_eq!(h.median(), 0.005);
    }

    #[test]
    fn median_mixed_distribution() {
        let h = Histogram::new("t", "t");
        for _ in 0..5 { h.observe(0.0001); }
        for _ in 0..5 { h.observe(0.5); }
        assert_eq!(h.median(), 0.0005);
    }

    #[test]
    fn median_all_in_overflow_is_infinity() {
        let h = Histogram::new("t", "t");
        for _ in 0..5 {
            h.observe(10.0);
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

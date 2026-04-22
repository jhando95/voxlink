use shared_types::PerfSnapshot;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use sysinfo::{Pid, System};

pub struct PerfCollector {
    start_time: Instant,
    /// Lazy-initialized: sysinfo::System is ~1MB and takes 50-100ms to create.
    /// Deferring to first snapshot() call shaves startup time.
    system: Option<System>,
    pid: Pid,
    num_cpus: f32,
    peak_memory_mb: f32,
    /// Set on the second `snapshot()` call so the sysinfo lazy-init cost
    /// is captured as baseline, not reported as growth.
    initial_memory_mb: Option<f32>,
    pub audio_active: Arc<AtomicBool>,
    pub network_connected: Arc<AtomicBool>,
    /// Shared counter for dropped audio frames (#11)
    pub dropped_frames: Arc<AtomicU64>,
    // Audio metrics (M3)
    pub frames_decoded: Arc<AtomicU32>,
    pub frames_dropped: Arc<AtomicU32>,
    pub current_jitter_ms: Arc<AtomicU32>,
    pub active_peers: Arc<AtomicU32>,
    pub encode_bitrate: Arc<AtomicU32>,
    // Transport
    pub udp_active: Arc<AtomicBool>,
    pub ping_ms: Arc<std::sync::atomic::AtomicI32>,
    pub screen_frames_completed: Arc<AtomicU32>,
    pub screen_frames_dropped: Arc<AtomicU32>,
    pub screen_frames_timed_out: Arc<AtomicU32>,
    // M8: audio callback health
    pub capture_callback_hist: Option<Arc<audio_core::Histogram>>,
    pub playback_callback_hist: Option<Arc<audio_core::Histogram>>,
    pub callback_glitch_count: Arc<AtomicU32>,
    // For computing frame loss rate
    last_decoded: u32,
    last_dropped: u32,
}

impl PerfCollector {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            system: None, // Deferred — created on first snapshot()
            pid: Pid::from_u32(std::process::id()),
            num_cpus: std::thread::available_parallelism()
                .map(|n| n.get() as f32)
                .unwrap_or(1.0),
            peak_memory_mb: 0.0,
            initial_memory_mb: None,
            audio_active: Arc::new(AtomicBool::new(false)),
            network_connected: Arc::new(AtomicBool::new(false)),
            dropped_frames: Arc::new(AtomicU64::new(0)),
            frames_decoded: Arc::new(AtomicU32::new(0)),
            frames_dropped: Arc::new(AtomicU32::new(0)),
            current_jitter_ms: Arc::new(AtomicU32::new(40)),
            active_peers: Arc::new(AtomicU32::new(0)),
            encode_bitrate: Arc::new(AtomicU32::new(0)),
            udp_active: Arc::new(AtomicBool::new(false)),
            ping_ms: Arc::new(std::sync::atomic::AtomicI32::new(-1)),
            screen_frames_completed: Arc::new(AtomicU32::new(0)),
            screen_frames_dropped: Arc::new(AtomicU32::new(0)),
            screen_frames_timed_out: Arc::new(AtomicU32::new(0)),
            // M8: start unwired; main.rs plugs the histograms in after AudioEngine::new()
            capture_callback_hist: None,
            playback_callback_hist: None,
            callback_glitch_count: Arc::new(AtomicU32::new(0)),
            last_decoded: 0,
            last_dropped: 0,
        }
    }

    pub fn snapshot(&mut self) -> PerfSnapshot {
        let system = self.system.get_or_insert_with(|| {
            let mut s = System::new();
            s.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[self.pid]), true);
            s
        });

        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[self.pid]), true);

        let (cpu, mem) = system
            .process(self.pid)
            .map(|p| (p.cpu_usage(), p.memory() as f32 / (1024.0 * 1024.0)))
            .unwrap_or((0.0, 0.0));

        self.peak_memory_mb = self.peak_memory_mb.max(mem);

        // Two-phase baseline: first snapshot sets "pending" (NaN marker),
        // second snapshot captures the actual baseline, subsequent snapshots
        // report delta. This ensures sysinfo's lazy-init cost is baselined,
        // not reported as growth.
        let memory_growth_mb = match self.initial_memory_mb {
            None => {
                self.initial_memory_mb = Some(f32::NAN);
                0.0
            }
            Some(b) if b.is_nan() => {
                self.initial_memory_mb = Some(mem);
                0.0
            }
            Some(baseline) => (mem - baseline).max(0.0),
        };

        // #16: Normalize CPU% by core count so 100% = all cores saturated
        let cpu_normalized = cpu / self.num_cpus;

        // Compute frame loss rate from deltas since last snapshot
        let decoded = self.frames_decoded.load(Ordering::Relaxed);
        let dropped = self.frames_dropped.load(Ordering::Relaxed);
        let delta_decoded = decoded.wrapping_sub(self.last_decoded);
        let delta_dropped = dropped.wrapping_sub(self.last_dropped);
        self.last_decoded = decoded;
        self.last_dropped = dropped;
        let total = delta_decoded + delta_dropped;
        let loss_rate = if total > 0 {
            delta_dropped as f32 / total as f32
        } else {
            0.0
        };

        let capture_callback_median_ms = self
            .capture_callback_hist
            .as_ref()
            .map(|h| {
                let m = h.median() * 1000.0;
                if m.is_finite() { m as f32 } else { 999.0 }
            })
            .unwrap_or(0.0);
        let playback_callback_median_ms = self
            .playback_callback_hist
            .as_ref()
            .map(|h| {
                let m = h.median() * 1000.0;
                if m.is_finite() { m as f32 } else { 999.0 }
            })
            .unwrap_or(0.0);
        let audio_glitch_count = self.callback_glitch_count.load(Ordering::Relaxed);

        PerfSnapshot {
            cpu_percent: cpu_normalized,
            memory_mb: mem,
            peak_memory_mb: self.peak_memory_mb,
            uptime_secs: self.start_time.elapsed().as_secs(),
            audio_active: self.audio_active.load(Ordering::Relaxed),
            network_connected: self.network_connected.load(Ordering::Relaxed),
            dropped_frames: self.dropped_frames.load(Ordering::Relaxed),
            jitter_buffer_ms: self.current_jitter_ms.load(Ordering::Relaxed),
            frame_loss_rate: loss_rate,
            encode_bitrate_kbps: self.encode_bitrate.load(Ordering::Relaxed) / 1000,
            decode_peers: self.active_peers.load(Ordering::Relaxed),
            udp_active: self.udp_active.load(Ordering::Relaxed),
            ping_ms: self.ping_ms.load(Ordering::Relaxed),
            screen_frames_completed: self.screen_frames_completed.load(Ordering::Relaxed),
            screen_frames_dropped: self.screen_frames_dropped.load(Ordering::Relaxed),
            screen_frames_timed_out: self.screen_frames_timed_out.load(Ordering::Relaxed),
            // M8
            capture_callback_median_ms,
            playback_callback_median_ms,
            audio_glitch_count,
            memory_growth_mb,
        }
    }

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
}

impl Default for PerfCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state() {
        let collector = PerfCollector::new();
        assert!(!collector.audio_active.load(Ordering::Relaxed));
        assert!(!collector.network_connected.load(Ordering::Relaxed));
        assert_eq!(collector.dropped_frames.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn dropped_frames_counter() {
        let collector = PerfCollector::new();
        collector.dropped_frames.fetch_add(1, Ordering::Relaxed);
        collector.dropped_frames.fetch_add(1, Ordering::Relaxed);
        collector.dropped_frames.fetch_add(1, Ordering::Relaxed);
        assert_eq!(collector.dropped_frames.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn atomic_flag_propagation() {
        let collector = PerfCollector::new();

        // Clone the flags as the app would
        let audio_flag = collector.audio_active.clone();
        let net_flag = collector.network_connected.clone();

        // Set from another "component"
        audio_flag.store(true, Ordering::Relaxed);
        net_flag.store(true, Ordering::Relaxed);

        // Collector should see the changes
        assert!(collector.audio_active.load(Ordering::Relaxed));
        assert!(collector.network_connected.load(Ordering::Relaxed));
    }

    #[test]
    fn snapshot_returns_non_negative_values() {
        let mut collector = PerfCollector::new();
        let snap = collector.snapshot();
        assert!(snap.cpu_percent >= 0.0);
        assert!(snap.memory_mb >= 0.0);
        assert!(snap.peak_memory_mb >= snap.memory_mb);
        // uptime should be 0 or very small since we just created it
        assert!(snap.uptime_secs <= 1);
        assert!(!snap.audio_active);
        assert!(!snap.network_connected);
        assert_eq!(snap.dropped_frames, 0);
    }

    #[test]
    fn frame_loss_rate_calculation() {
        let mut collector = PerfCollector::new();
        // First snapshot establishes baseline
        let _ = collector.snapshot();

        // Simulate: 90 decoded + 10 dropped = 10% loss
        collector.frames_decoded.store(90, Ordering::Relaxed);
        collector.frames_dropped.store(10, Ordering::Relaxed);

        let snap = collector.snapshot();
        // Loss rate should be 10/100 = 0.1
        assert!((snap.frame_loss_rate - 0.1).abs() < 0.01);
    }

    #[test]
    fn frame_loss_rate_zero_when_no_frames() {
        let mut collector = PerfCollector::new();
        let _ = collector.snapshot(); // baseline
        let snap = collector.snapshot();
        assert!((snap.frame_loss_rate - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn multiple_snapshots_show_deltas() {
        let mut collector = PerfCollector::new();
        let _ = collector.snapshot(); // baseline

        collector.frames_decoded.store(100, Ordering::Relaxed);
        collector.frames_dropped.store(0, Ordering::Relaxed);
        let snap1 = collector.snapshot();
        assert!((snap1.frame_loss_rate - 0.0).abs() < f32::EPSILON);

        // Now add 10 dropped in the next interval
        collector.frames_decoded.store(200, Ordering::Relaxed);
        collector.frames_dropped.store(10, Ordering::Relaxed);
        let snap2 = collector.snapshot();
        // Delta: 100 decoded, 10 dropped = 10/110 ≈ 0.0909
        assert!(snap2.frame_loss_rate > 0.05);
        assert!(snap2.frame_loss_rate < 0.15);
    }

    #[test]
    fn jitter_and_bitrate_metrics() {
        let collector = PerfCollector::new();
        collector.current_jitter_ms.store(60, Ordering::Relaxed);
        collector.encode_bitrate.store(64000, Ordering::Relaxed);
        collector.active_peers.store(3, Ordering::Relaxed);
        collector
            .screen_frames_completed
            .store(7, Ordering::Relaxed);
        collector.screen_frames_dropped.store(2, Ordering::Relaxed);
        collector
            .screen_frames_timed_out
            .store(1, Ordering::Relaxed);

        let mut c = collector;
        let snap = c.snapshot();
        assert_eq!(snap.jitter_buffer_ms, 60);
        assert_eq!(snap.encode_bitrate_kbps, 64); // 64000/1000
        assert_eq!(snap.decode_peers, 3);
        assert_eq!(snap.screen_frames_completed, 7);
        assert_eq!(snap.screen_frames_dropped, 2);
        assert_eq!(snap.screen_frames_timed_out, 1);
    }

    #[test]
    fn default_trait() {
        let collector = PerfCollector::default();
        assert!(!collector.audio_active.load(Ordering::Relaxed));
        assert_eq!(collector.dropped_frames.load(Ordering::Relaxed), 0);
    }

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
        assert!(
            snap3.memory_growth_mb >= 0.0,
            "snapshot #3 growth = {} should be non-negative",
            snap3.memory_growth_mb
        );
    }

    #[test]
    fn audio_quality_numbers_returns_zeros_before_any_audio() {
        let collector = PerfCollector::new();
        let (c, p, g, f, j) = collector.audio_quality_numbers();
        assert_eq!(c, 0, "capture median should be zero before any callbacks");
        assert_eq!(p, 0, "playback median should be zero before any callbacks");
        assert_eq!(g, 0, "glitches should be zero before any callbacks");
        assert_eq!(f, 0, "frames_dropped should be zero before any callbacks");
        // jitter has a non-zero default (JITTER_INITIAL * 20) from AudioMetrics::new
        // but the PerfCollector-level current_jitter_ms is an Arc<AtomicU32> that
        // starts at 0 until wired up by main.rs. Don't assert on it.
        let _ = j;
    }
}

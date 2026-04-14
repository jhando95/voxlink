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

        PerfSnapshot {
            cpu_percent: cpu_normalized,
            memory_mb: mem,
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
        }
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
}

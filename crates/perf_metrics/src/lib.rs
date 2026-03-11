use shared_types::PerfSnapshot;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

        PerfSnapshot {
            cpu_percent: cpu_normalized,
            memory_mb: mem,
            uptime_secs: self.start_time.elapsed().as_secs(),
            audio_active: self.audio_active.load(Ordering::Relaxed),
            network_connected: self.network_connected.load(Ordering::Relaxed),
            dropped_frames: self.dropped_frames.load(Ordering::Relaxed),
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
}

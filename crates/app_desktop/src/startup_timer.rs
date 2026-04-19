//! Lightweight startup-phase recorder.
//!
//! Call `phase()` at each major seam in startup. Each call logs at info
//! level and appends to an internal `Vec`. The final list is handed to
//! the UI layer for display in the Perf panel, then the timer is dropped.
//!
//! Runtime cost: one `Instant::now()` call + one `log::info!` per phase.
//! Negligible compared to the phase work itself, and zero overhead after
//! the timer is dropped (right before `window.run()`).

use std::time::Instant;

// Task 4 wires this into main.rs and removes the allow.
#[allow(dead_code)]
pub struct StartupTimer {
    start: Instant,
    phases: Vec<(String, u32)>,
}

#[allow(dead_code)]
impl StartupTimer {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            phases: Vec::with_capacity(16),
        }
    }

    /// Record cumulative elapsed time with a label and log it.
    pub fn phase(&mut self, name: &str) {
        let ms = self.start.elapsed().as_millis() as u32;
        log::info!("startup: {name} @ {ms}ms");
        self.phases.push((name.to_string(), ms));
    }

    /// Consume the timer and return the recorded phases in order.
    pub fn into_phases(self) -> Vec<(String, u32)> {
        self.phases
    }

    /// Current total elapsed time in milliseconds.
    pub fn total_ms(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }
}

impl Default for StartupTimer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_recorded_in_order() {
        let mut t = StartupTimer::new();
        t.phase("a");
        std::thread::sleep(std::time::Duration::from_millis(5));
        t.phase("b");
        let phases = t.into_phases();
        assert_eq!(phases.len(), 2);
        assert_eq!(phases[0].0, "a");
        assert_eq!(phases[1].0, "b");
        assert!(phases[1].1 >= phases[0].1);
    }

    #[test]
    fn total_ms_monotonic() {
        let t = StartupTimer::new();
        let m1 = t.total_ms();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let m2 = t.total_ms();
        assert!(m2 >= m1);
    }

    #[test]
    fn default_constructor_works() {
        let t = StartupTimer::default();
        let _ = t.into_phases();
    }
}

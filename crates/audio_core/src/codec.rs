use audiopus::coder::Encoder as OpusEncoder;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

// ─── Adaptive Noise Gate with smooth crossfade ───
// Instead of hard on/off, uses a gain multiplier that ramps smoothly.
// This eliminates the click artifacts of a hard gate.

pub(crate) struct NoiseGate {
    noise_floor: f32,
    is_open: bool,
    hold_frames: u32,
    hold_remaining: u32,
    /// Smooth gain (0.0–1.0) — ramps up/down over a few ms to avoid clicks
    gain: f32,
    /// Shared sensitivity value (0.0–1.0 stored as 0–1000).
    /// Higher = more sensitive (lower threshold). Read from config.
    sensitivity: Arc<AtomicU32>,
}

// Gain ramp speed: ~2ms attack, ~5ms release at 48kHz
// Each frame is 960 samples (20ms), so per-sample ramp:
const GATE_ATTACK_PER_SAMPLE: f32 = 1.0 / 96.0; // ~2ms to fully open
const GATE_RELEASE_PER_SAMPLE: f32 = 1.0 / 240.0; // ~5ms to fully close

impl NoiseGate {
    pub fn new(sensitivity: Arc<AtomicU32>) -> Self {
        Self {
            noise_floor: 0.01,
            is_open: false,
            hold_frames: 5,
            hold_remaining: 0,
            gain: 0.0,
            sensitivity,
        }
    }

    /// Determine gate open/closed state based on energy. Call once per frame.
    #[inline(always)]
    pub fn process(&mut self, energy: f32, vad_enabled: bool) -> bool {
        if !vad_enabled {
            self.is_open = true;
            return true;
        }

        // sensitivity: 0.0 = least sensitive (high threshold), 1.0 = most sensitive (low threshold)
        let sens = self.sensitivity.load(Ordering::Relaxed) as f32 / 1000.0;
        // Map sensitivity to threshold multiplier: 0.0 → 6x noise floor, 1.0 → 1.5x noise floor
        let multiplier = 6.0 - sens * 4.5;
        let threshold = (self.noise_floor * multiplier).max(0.001);

        if energy > threshold {
            self.is_open = true;
            self.hold_remaining = self.hold_frames;
            true
        } else {
            self.noise_floor = self.noise_floor * 0.95 + energy * 0.05;
            if self.hold_remaining > 0 {
                self.hold_remaining -= 1;
                true
            } else {
                self.is_open = false;
                false
            }
        }
    }

    /// Apply smooth gain ramp to samples in-place.
    /// Call after `process()` with the gate decision.
    #[inline]
    pub fn apply_gain(&mut self, samples: &mut [f32], gate_open: bool) {
        let target = if gate_open { 1.0 } else { 0.0 };
        let ramp = if gate_open {
            GATE_ATTACK_PER_SAMPLE
        } else {
            GATE_RELEASE_PER_SAMPLE
        };

        for s in samples.iter_mut() {
            if (self.gain - target).abs() > 0.001 {
                if gate_open {
                    self.gain = (self.gain + ramp).min(1.0);
                } else {
                    self.gain = (self.gain - ramp).max(0.0);
                }
            } else {
                self.gain = target;
            }
            *s *= self.gain;
        }
    }

    /// Returns true if gain is effectively zero (fully closed, no need to encode)
    #[inline]
    pub fn is_silent(&self) -> bool {
        self.gain < 0.001
    }
}

// ─── Mute/unmute gain ramp ───
// Eliminates clicks when toggling mute by smoothly fading over ~5ms.

pub(crate) struct MuteRamp {
    gain: f32,
    /// The is_capturing flag: true = unmuted, false = muted
    is_capturing: Arc<AtomicBool>,
}

const MUTE_RAMP_PER_SAMPLE: f32 = 1.0 / 240.0; // ~5ms fade

impl MuteRamp {
    pub fn new(is_capturing: Arc<AtomicBool>) -> Self {
        Self {
            gain: 1.0,
            is_capturing,
        }
    }

    /// Apply mute ramp to samples in-place. Returns true if audio should be sent.
    #[inline]
    pub fn apply(&mut self, samples: &mut [f32]) -> bool {
        // is_capturing=true means unmuted, is_capturing=false means muted
        let target = if self.is_capturing.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        let mut any_nonzero = false;

        for s in samples.iter_mut() {
            if (self.gain - target).abs() > 0.001 {
                if target > 0.5 {
                    self.gain = (self.gain + MUTE_RAMP_PER_SAMPLE).min(1.0);
                } else {
                    self.gain = (self.gain - MUTE_RAMP_PER_SAMPLE).max(0.0);
                }
            } else {
                self.gain = target;
            }
            *s *= self.gain;
            if self.gain > 0.001 {
                any_nonzero = true;
            }
        }
        any_nonzero
    }
}

// ─── Opus Encoder Wrapper (Send-safe) ───

pub(crate) struct SendEncoder(pub OpusEncoder);
unsafe impl Send for SendEncoder {}

impl std::ops::Deref for SendEncoder {
    type Target = OpusEncoder;
    fn deref(&self) -> &OpusEncoder {
        &self.0
    }
}
impl std::ops::DerefMut for SendEncoder {
    fn deref_mut(&mut self) -> &mut OpusEncoder {
        &mut self.0
    }
}

// ─── High-pass filter (DC/rumble removal) ───
// Single-pole IIR high-pass at ~80Hz. Removes mic rumble, plosives, HVAC hum.
// Coefficient: alpha = 1 / (1 + 2π * cutoff / sample_rate)
// At 48kHz with 80Hz cutoff: alpha ≈ 0.9896

pub(crate) struct HighPassFilter {
    prev_input: f32,
    prev_output: f32,
    alpha: f32,
}

impl HighPassFilter {
    pub fn new() -> Self {
        // 80Hz cutoff at 48kHz sample rate
        let cutoff = 80.0f32;
        let sample_rate = shared_types::SAMPLE_RATE as f32;
        let rc = 1.0 / (std::f32::consts::TAU * cutoff);
        let dt = 1.0 / sample_rate;
        let alpha = rc / (rc + dt);
        Self {
            prev_input: 0.0,
            prev_output: 0.0,
            alpha,
        }
    }

    /// Process a buffer of samples in-place.
    #[inline]
    pub fn process(&mut self, samples: &mut [f32]) {
        for s in samples.iter_mut() {
            let input = *s;
            self.prev_output = self.alpha * (self.prev_output + input - self.prev_input);
            self.prev_input = input;
            *s = self.prev_output;
        }
    }
}

// ─── De-esser (simple sibilance reducer) ───
// Reduces harsh sibilant frequencies (4–8kHz) that Opus can exaggerate.
// Uses a one-pole low-pass to detect high-frequency energy and attenuate.

pub(crate) struct DeEsser {
    /// Low-pass state for detecting sibilance
    lp_state: f32,
    /// Gain reduction currently applied
    reduction: f32,
}

const DEESSER_ALPHA: f32 = 0.85; // ~6kHz detection at 48kHz
const DEESSER_THRESHOLD: f32 = 0.15; // energy ratio triggering reduction
const DEESSER_MAX_REDUCTION: f32 = 0.4; // max 60% of sibilant energy kept

impl DeEsser {
    pub fn new() -> Self {
        Self {
            lp_state: 0.0,
            reduction: 0.0,
        }
    }

    /// Process samples in-place, reducing sibilance.
    #[inline]
    pub fn process(&mut self, samples: &mut [f32]) {
        for s in samples.iter_mut() {
            // Extract high-frequency component
            let lp = DEESSER_ALPHA * self.lp_state + (1.0 - DEESSER_ALPHA) * *s;
            let hp = *s - lp;
            self.lp_state = lp;

            // Detect sibilance energy
            let hp_energy = hp.abs();
            let total_energy = s.abs().max(0.001);
            let ratio = hp_energy / total_energy;

            // Smooth gain reduction
            let target = if ratio > DEESSER_THRESHOLD {
                DEESSER_MAX_REDUCTION
                    * ((ratio - DEESSER_THRESHOLD) / (1.0 - DEESSER_THRESHOLD)).min(1.0)
            } else {
                0.0
            };
            self.reduction = self.reduction * 0.95 + target * 0.05;

            // Apply: reduce only the HF component
            *s = lp + hp * (1.0 - self.reduction);
        }
    }
}

// ─── Automatic Gain Control (AGC) ───
// Normalizes mic volume so quiet speakers are boosted and loud speakers are tamed.
// Uses slow-adapting RMS tracking with a fast limiter for sudden peaks.

pub(crate) struct Agc {
    /// Smoothed RMS level estimate
    rms_estimate: f32,
    /// Current gain applied
    gain: f32,
    /// Target RMS level (what we normalize to)
    target_rms: f32,
    /// Max gain to prevent amplifying noise
    max_gain: f32,
    /// Min gain to prevent excessive attenuation
    min_gain: f32,
}

// AGC smoothing: slow adaptation avoids pumping artifacts
const AGC_RMS_ATTACK: f32 = 0.003; // ~300ms to adapt to louder signal
const AGC_RMS_RELEASE: f32 = 0.001; // ~1s to adapt to quieter signal
const AGC_GAIN_SMOOTH: f32 = 0.005; // gain change rate per sample

impl Agc {
    pub fn new() -> Self {
        Self {
            rms_estimate: 0.05,
            gain: 1.0,
            target_rms: 0.12, // comfortable voice level
            max_gain: 10.0,   // +20dB max boost
            min_gain: 0.1,    // -20dB max cut
        }
    }

    /// Process a frame in-place. Call after noise gate (so we don't amplify noise).
    #[inline]
    pub fn process(&mut self, samples: &mut [f32]) {
        // Measure frame RMS
        let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
        let frame_rms = (sum_sq / samples.len().max(1) as f32).sqrt();

        // Update RMS estimate with asymmetric smoothing
        let alpha = if frame_rms > self.rms_estimate {
            AGC_RMS_ATTACK
        } else {
            AGC_RMS_RELEASE
        };
        self.rms_estimate = self.rms_estimate * (1.0 - alpha) + frame_rms * alpha;

        // Compute target gain
        let target_gain = if self.rms_estimate > 0.001 {
            (self.target_rms / self.rms_estimate).clamp(self.min_gain, self.max_gain)
        } else {
            1.0 // near-silence, don't adjust
        };

        // Smoothly ramp gain to target
        for s in samples.iter_mut() {
            self.gain += (target_gain - self.gain) * AGC_GAIN_SMOOTH;
            *s *= self.gain;
            // Fast limiter: prevent clipping from sudden gain application
            if s.abs() > 0.95 {
                *s *= 0.95 / s.abs();
            }
        }
    }
}

// ─── Comfort Noise Generator ───
// Injects very low-level noise when the gate closes to avoid jarring dead silence.
// Uses a simple linear-feedback shift register (LFSR) — zero allocation, no rand crate.

pub(crate) struct ComfortNoise {
    lfsr: u32,
}

const COMFORT_NOISE_LEVEL: f32 = 0.0015; // -56dB — barely perceptible

impl ComfortNoise {
    pub fn new() -> Self {
        Self { lfsr: 0xACE1 }
    }

    /// Fill a buffer with comfort noise. Only call when gate is closed.
    #[inline]
    pub fn fill(&mut self, samples: &mut [f32]) {
        for s in samples.iter_mut() {
            // Galois LFSR — fast pseudo-random
            let bit = self.lfsr & 1;
            self.lfsr >>= 1;
            if bit == 1 {
                self.lfsr ^= 0xB400;
            }
            // Map to [-1, 1] then scale to comfort level
            let noise = (self.lfsr as f32 / 32768.0 - 1.0) * COMFORT_NOISE_LEVEL;
            *s += noise;
        }
    }
}

// ─── DSP Utilities ───

/// Compute RMS energy — O(n), no allocations
#[inline(always)]
pub(crate) fn frame_energy(pcm: &[f32]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = pcm.iter().map(|&s| s * s).sum();
    (sum_sq / pcm.len() as f32).sqrt()
}

/// Fast soft-clip — polynomial approximation, no transcendentals.
#[inline(always)]
pub(crate) fn soft_clip(x: f32) -> f32 {
    if x > 1.0 {
        let d = x - 1.0;
        1.0 + d / (1.0 + d * d)
    } else if x < -1.0 {
        let d = -x - 1.0;
        -(1.0 + d / (1.0 + d * d))
    } else {
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared_types::FRAME_SIZE;

    // ─── frame_energy ───

    #[test]
    fn frame_energy_empty() {
        assert_eq!(frame_energy(&[]), 0.0);
    }

    #[test]
    fn frame_energy_silence() {
        let silence = [0.0f32; 960];
        assert_eq!(frame_energy(&silence), 0.0);
    }

    #[test]
    fn frame_energy_full_scale() {
        let full = [1.0f32; 960];
        let energy = frame_energy(&full);
        assert!((energy - 1.0).abs() < 1e-6);
    }

    #[test]
    fn frame_energy_half_scale() {
        let half = [0.5f32; 100];
        let energy = frame_energy(&half);
        assert!((energy - 0.5).abs() < 1e-6);
    }

    // ─── soft_clip ───

    #[test]
    fn soft_clip_passthrough_in_range() {
        assert_eq!(soft_clip(0.0), 0.0);
        assert_eq!(soft_clip(0.5), 0.5);
        assert_eq!(soft_clip(-0.5), -0.5);
        assert_eq!(soft_clip(1.0), 1.0);
        assert_eq!(soft_clip(-1.0), -1.0);
    }

    #[test]
    fn soft_clip_compression_outside_range() {
        let clipped = soft_clip(2.0);
        assert!(clipped > 1.0);
        assert!(clipped < 2.0);
        let clipped_big = soft_clip(10.0);
        assert!(clipped_big > 1.0);
        assert!(clipped_big < 2.0);
        let clipped_neg = soft_clip(-2.0);
        assert!(clipped_neg < -1.0);
        assert!(clipped_neg > -2.0);
    }

    #[test]
    fn soft_clip_symmetry() {
        for &x in &[0.0, 0.5, 1.0, 1.5, 2.0, 5.0, 10.0] {
            let pos = soft_clip(x);
            let neg = soft_clip(-x);
            assert!(
                (neg + pos).abs() < 1e-6,
                "Symmetry failed for x={x}: soft_clip({x})={pos}, soft_clip(-{x})={neg}"
            );
        }
    }

    // ─── NoiseGate ───

    #[test]
    fn noise_gate_vad_disabled_always_open() {
        let sensitivity = Arc::new(AtomicU32::new(500));
        let mut gate = NoiseGate::new(sensitivity);
        assert!(gate.process(0.0, false));
        assert!(gate.process(0.0001, false));
        assert!(gate.process(1.0, false));
    }

    #[test]
    fn noise_gate_sensitivity_affects_threshold() {
        let low_sens = Arc::new(AtomicU32::new(0));
        let mut gate_low = NoiseGate::new(low_sens);
        let quiet_energy = 0.02;
        for _ in 0..20 {
            gate_low.process(0.001, true);
        }
        let low_result = gate_low.process(quiet_energy, true);

        let high_sens = Arc::new(AtomicU32::new(1000));
        let mut gate_high = NoiseGate::new(high_sens);
        for _ in 0..20 {
            gate_high.process(0.001, true);
        }
        let high_result = gate_high.process(quiet_energy, true);

        assert!(
            high_result || !low_result,
            "High sensitivity should be at least as permissive as low sensitivity"
        );
    }

    #[test]
    fn noise_gate_hold_behavior() {
        let sensitivity = Arc::new(AtomicU32::new(500));
        let mut gate = NoiseGate::new(sensitivity);
        assert!(gate.process(1.0, true));
        let mut hold_count = 0;
        for _ in 0..10 {
            if gate.process(0.0, true) {
                hold_count += 1;
            }
        }
        assert!(
            hold_count >= 1,
            "Gate should hold open after loud signal, got {hold_count} frames"
        );
    }

    #[test]
    fn noise_gate_smooth_gain_ramp() {
        let sensitivity = Arc::new(AtomicU32::new(500));
        let mut gate = NoiseGate::new(sensitivity);

        // Start closed, gain should be 0
        assert!(gate.gain < 0.01);

        // Open the gate
        gate.process(1.0, true);
        let mut samples = [0.5f32; FRAME_SIZE];
        gate.apply_gain(&mut samples, true);

        // Gain should ramp up — first few samples attenuated, last samples near full
        assert!(
            samples[0] < 0.5,
            "First sample should be attenuated during ramp-up"
        );
        assert!(
            samples[FRAME_SIZE - 1] > 0.45,
            "Last sample should be near full after ramp"
        );
    }

    #[test]
    fn noise_gate_smooth_release() {
        let sensitivity = Arc::new(AtomicU32::new(500));
        let mut gate = NoiseGate::new(sensitivity);

        // Open gate and ramp gain to 1.0
        gate.gain = 1.0;
        gate.process(1.0, true);

        // Close gate
        gate.process(0.0, true); // hold starts
        for _ in 0..6 {
            gate.process(0.0, true);
        } // exhaust hold

        let mut samples = [0.5f32; FRAME_SIZE];
        gate.apply_gain(&mut samples, false);

        // First sample should still be near full (smooth ramp-down)
        assert!(samples[0] > 0.3, "Release should be smooth, not instant");
        // After the ramp, last samples should be near zero
        assert!(
            samples[FRAME_SIZE - 1] < 0.1,
            "Should be mostly closed by end of frame"
        );
    }

    // ─── MuteRamp ───

    #[test]
    fn mute_ramp_smooth_fade() {
        // is_capturing=true means unmuted
        let flag = Arc::new(AtomicBool::new(true));
        let mut ramp = MuteRamp::new(flag.clone());

        // Start unmuted
        let mut samples = [1.0f32; 480];
        assert!(ramp.apply(&mut samples));
        assert!(samples[0] > 0.99); // not muted

        // Mute (is_capturing=false)
        flag.store(false, Ordering::Relaxed);
        let mut samples = [1.0f32; 480];
        let has_audio = ramp.apply(&mut samples);
        // First sample should still be near 1.0 (smooth ramp)
        assert!(samples[0] > 0.5, "Mute ramp should be smooth");
        // Last samples should be near zero
        assert!(samples[479] < 0.1 || !has_audio);
    }

    // ─── HighPassFilter ───

    #[test]
    fn highpass_removes_dc() {
        let mut hpf = HighPassFilter::new();
        // DC signal should be removed
        let mut samples = [0.5f32; 960];
        hpf.process(&mut samples);
        // After settling, output should be near zero for DC input
        let last = samples[959];
        assert!(last.abs() < 0.01, "DC should be filtered: got {last}");
    }

    // ─── DeEsser ───

    #[test]
    fn deesser_passes_low_freq() {
        let mut de = DeEsser::new();
        // Low frequency signal should pass through mostly unchanged
        let mut samples = [0.0f32; 960];
        let tau = std::f32::consts::TAU;
        for (i, sample) in samples.iter_mut().enumerate() {
            *sample = (tau * 200.0 * i as f32 / 48000.0).sin() * 0.5;
        }
        let original_energy: f32 = samples.iter().map(|s| s * s).sum();
        de.process(&mut samples);
        let processed_energy: f32 = samples.iter().map(|s| s * s).sum();
        let ratio = processed_energy / original_energy;
        assert!(
            ratio > 0.9,
            "Low freq should pass through mostly unchanged: ratio={ratio}"
        );
    }

    // ─── AGC ───

    #[test]
    fn agc_boosts_quiet_signal() {
        let mut agc = Agc::new();
        // Feed quiet signal for several frames to let AGC adapt
        for _ in 0..50 {
            let mut samples = [0.01f32; FRAME_SIZE];
            agc.process(&mut samples);
        }
        // Now check that output is louder than input
        let mut samples = [0.01f32; FRAME_SIZE];
        agc.process(&mut samples);
        let output_rms: f32 =
            (samples.iter().map(|s| s * s).sum::<f32>() / FRAME_SIZE as f32).sqrt();
        assert!(
            output_rms > 0.01,
            "AGC should boost quiet signal: rms={output_rms}"
        );
    }

    #[test]
    fn agc_attenuates_loud_signal() {
        let mut agc = Agc::new();
        // Feed loud signal for several frames
        for _ in 0..50 {
            let mut samples = [0.8f32; FRAME_SIZE];
            agc.process(&mut samples);
        }
        let mut samples = [0.8f32; FRAME_SIZE];
        agc.process(&mut samples);
        let output_rms: f32 =
            (samples.iter().map(|s| s * s).sum::<f32>() / FRAME_SIZE as f32).sqrt();
        assert!(
            output_rms < 0.8,
            "AGC should attenuate loud signal: rms={output_rms}"
        );
    }

    #[test]
    fn agc_limiter_prevents_clipping() {
        let mut agc = Agc::new();
        // Artificially set high gain
        agc.gain = 10.0;
        let mut samples = [0.5f32; 100];
        agc.process(&mut samples);
        // No sample should exceed 0.95
        let max = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max <= 0.96, "Limiter should prevent clipping: max={max}");
    }

    // ─── Comfort Noise ───

    #[test]
    fn comfort_noise_non_silent() {
        let mut cn = ComfortNoise::new();
        let mut samples = [0.0f32; 960];
        cn.fill(&mut samples);
        let energy: f32 = samples.iter().map(|s| s * s).sum::<f32>() / 960.0;
        assert!(energy > 0.0, "Comfort noise should be non-silent");
        assert!(
            energy < 0.001,
            "Comfort noise should be very quiet: energy={energy}"
        );
    }

    #[test]
    fn comfort_noise_varies() {
        let mut cn = ComfortNoise::new();
        let mut samples = [0.0f32; 100];
        cn.fill(&mut samples);
        // Should not be all the same value
        let distinct: std::collections::HashSet<u32> =
            samples.iter().map(|s| s.to_bits()).collect();
        assert!(distinct.len() > 10, "Comfort noise should have variety");
    }
}

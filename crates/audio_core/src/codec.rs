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
    /// Auto-calibration: accumulate energy during first ~2s to set noise floor.
    /// 100 frames × 20ms = 2 seconds.
    calibration_frames: u16,
    calibration_sum: f32,
}

// Gain ramp speed: ~2ms attack, ~5ms release at 48kHz
// Each frame is 960 samples (20ms), so per-sample ramp:
const GATE_ATTACK_PER_SAMPLE: f32 = 1.0 / 96.0; // ~2ms to fully open
const GATE_RELEASE_PER_SAMPLE: f32 = 1.0 / 240.0; // ~5ms to fully close

impl NoiseGate {
    const CALIBRATION_FRAMES: u16 = 100; // 100 × 20ms = 2s

    pub fn new(sensitivity: Arc<AtomicU32>) -> Self {
        Self {
            noise_floor: 0.01,
            is_open: false,
            hold_frames: 5,
            hold_remaining: 0,
            gain: 0.0,
            sensitivity,
            calibration_frames: 0,
            calibration_sum: 0.0,
        }
    }

    /// Determine gate open/closed state based on energy. Call once per frame.
    /// During the first ~2s, auto-calibrates the noise floor from ambient noise.
    #[inline(always)]
    pub fn process(&mut self, energy: f32, vad_enabled: bool) -> bool {
        if !vad_enabled {
            self.is_open = true;
            return true;
        }

        // Auto-calibration: measure ambient noise during first 2s
        if self.calibration_frames < Self::CALIBRATION_FRAMES {
            self.calibration_frames += 1;
            self.calibration_sum += energy;
            if self.calibration_frames == Self::CALIBRATION_FRAMES {
                let avg = self.calibration_sum / Self::CALIBRATION_FRAMES as f32;
                // Set noise floor from measured ambient, with a minimum to avoid division by tiny values
                self.noise_floor = avg.max(0.001);
                log::debug!(
                    "Noise gate auto-calibrated: noise_floor={:.6}",
                    self.noise_floor
                );
            }
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
    primed: bool,
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
            primed: false,
        }
    }

    /// Process a buffer of samples in-place.
    /// On the very first frame, applies a 2ms fade-in to eliminate the warmup transient.
    #[inline]
    pub fn process(&mut self, samples: &mut [f32]) {
        // Fade-in ramp on first frame to eliminate HPF warmup transient
        // 2ms at 48kHz = 96 samples
        let fade_len = if !self.primed {
            96.min(samples.len())
        } else {
            0
        };

        for (i, s) in samples.iter_mut().enumerate() {
            let input = *s;
            self.prev_output = self.alpha * (self.prev_output + input - self.prev_input);
            self.prev_input = input;
            *s = self.prev_output;

            if i < fade_len {
                *s *= (i + 1) as f32 / fade_len as f32;
            }
        }

        if !self.primed {
            self.primed = true;
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

    /// Process a frame in-place with gate awareness.
    /// When gate is open, normal adaptation. When gate is closed, 10x slower attack
    /// so gain stays calibrated across gate transitions (quiet speech isn't lost on gate open).
    #[inline]
    pub fn process_with_gate(&mut self, samples: &mut [f32], gate_open: bool) {
        // Measure frame RMS
        let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
        let frame_rms = (sum_sq / samples.len().max(1) as f32).sqrt();

        // Use 10x slower adaptation when gate is closed to keep gain calibrated
        let speed = if gate_open { 1.0 } else { 0.1 };
        let alpha = if frame_rms > self.rms_estimate {
            AGC_RMS_ATTACK * speed
        } else {
            AGC_RMS_RELEASE * speed
        };
        self.rms_estimate = self.rms_estimate * (1.0 - alpha) + frame_rms * alpha;

        // Compute target gain
        let target_gain = if self.rms_estimate > 0.001 {
            (self.target_rms / self.rms_estimate).clamp(self.min_gain, self.max_gain)
        } else {
            1.0 // near-silence, don't adjust
        };

        // Smoothly ramp gain to target (slower when gate closed)
        let smooth = AGC_GAIN_SMOOTH * speed;
        for s in samples.iter_mut() {
            self.gain += (target_gain - self.gain) * smooth;
            *s *= self.gain;
            // Fast limiter: prevent clipping from sudden gain application
            if s.abs() > 0.95 {
                *s *= 0.95 / s.abs();
            }
        }
    }
}

// ─── Playback AGC (per-peer volume normalization) ───
// Slower-adapting AGC for playback: normalizes peers to consistent volume
// without affecting the manual volume slider. Prevents loud peers from
// dominating the mix and quiet peers from being inaudible.

pub(crate) struct PlaybackAgc {
    rms_estimate: f32,
    gain: f32,
    target_rms: f32,
    max_gain: f32,
    min_gain: f32,
}

const PLAYBACK_AGC_RMS_ATTACK: f32 = 0.001; // ~1s to adapt to louder signal (slower than capture)
const PLAYBACK_AGC_RMS_RELEASE: f32 = 0.0005; // ~2s to adapt to quieter signal
const PLAYBACK_AGC_GAIN_SMOOTH: f32 = 0.002; // slower gain changes for natural sound

impl PlaybackAgc {
    pub fn new() -> Self {
        Self::with_target_rms(0.15)
    }

    pub fn with_target_rms(target_rms: f32) -> Self {
        Self {
            rms_estimate: 0.05,
            gain: 1.0,
            target_rms,     // default 0.15 — slightly louder target for playback clarity
            max_gain: 6.0,  // +15dB max boost (less aggressive than capture)
            min_gain: 0.15, // -16dB max cut
        }
    }

    /// Process decoded samples in-place before mixing.
    #[inline]
    pub fn process(&mut self, samples: &mut [f32]) {
        let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
        let frame_rms = (sum_sq / samples.len().max(1) as f32).sqrt();

        let alpha = if frame_rms > self.rms_estimate {
            PLAYBACK_AGC_RMS_ATTACK
        } else {
            PLAYBACK_AGC_RMS_RELEASE
        };
        self.rms_estimate = self.rms_estimate * (1.0 - alpha) + frame_rms * alpha;

        let target_gain = if self.rms_estimate > 0.001 {
            (self.target_rms / self.rms_estimate).clamp(self.min_gain, self.max_gain)
        } else {
            1.0
        };

        for s in samples.iter_mut() {
            self.gain += (target_gain - self.gain) * PLAYBACK_AGC_GAIN_SMOOTH;
            *s *= self.gain;
            // Soft limiter
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

// ─── Neural Noise Suppression (RNNoise) ───

/// Spectral noise suppression using nnnoiseless (pure-Rust RNNoise port).
/// Processes 480-sample sub-frames (10ms at 48kHz). Our 960-sample frames
/// are split into two sub-frames automatically.
pub(crate) struct NoiseSuppressor {
    state: Box<nnnoiseless::DenoiseState<'static>>,
    /// Whether suppression is active (can be toggled at runtime).
    enabled: bool,
    /// First frame produces fade-in artifacts — discard it.
    primed: bool,
}

impl NoiseSuppressor {
    const SUB_FRAME: usize = nnnoiseless::DenoiseState::FRAME_SIZE; // 480

    pub fn new() -> Self {
        let state = match std::panic::catch_unwind(nnnoiseless::DenoiseState::new) {
            Ok(s) => s,
            Err(_) => {
                log::error!("NoiseSuppressor: DenoiseState::new() panicked; using fallback");
                // Create a second attempt — if this panics too, it's unrecoverable
                nnnoiseless::DenoiseState::new()
            }
        };
        Self {
            state,
            enabled: false,
            primed: false,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.enabled = enabled;
            if enabled {
                // Reset state when toggling on to avoid stale RNN context
                self.state = nnnoiseless::DenoiseState::new();
                self.primed = false;
            }
        }
    }

    /// Process a 960-sample frame in-place. Samples are in [-1.0, 1.0] range.
    /// nnnoiseless expects [-32768, 32767] (i16 scale), so we scale in/out.
    pub fn process(&mut self, samples: &mut [f32]) {
        if !self.enabled || samples.len() < Self::SUB_FRAME * 2 {
            return;
        }

        let mut sub_in = [0.0f32; Self::SUB_FRAME];
        let mut sub_out = [0.0f32; Self::SUB_FRAME];

        for chunk_idx in 0..2 {
            let offset = chunk_idx * Self::SUB_FRAME;
            // Scale to i16 range for nnnoiseless
            for (i, s) in samples[offset..offset + Self::SUB_FRAME].iter().enumerate() {
                sub_in[i] = s * 32767.0;
            }
            self.state.process_frame(&mut sub_out, &sub_in);

            if !self.primed {
                // Discard first sub-frame output (fade-in artifacts)
                if chunk_idx == 1 {
                    self.primed = true;
                }
                // Still write zeros for the discarded frame
                for s in &mut samples[offset..offset + Self::SUB_FRAME] {
                    *s = 0.0;
                }
            } else {
                // Scale back to [-1.0, 1.0]
                for (i, s) in samples[offset..offset + Self::SUB_FRAME]
                    .iter_mut()
                    .enumerate()
                {
                    *s = (sub_out[i] / 32767.0).clamp(-1.0, 1.0);
                }
            }
        }
    }
}

// ─── Echo Cancellation (lightweight reference-based) ───

/// Shared echo reference buffer — written by playback, read by capture.
/// Uses atomic cursors for lock-free cross-thread access.
pub(crate) struct EchoReference {
    buf: std::cell::UnsafeCell<Vec<f32>>,
    write: std::sync::atomic::AtomicUsize,
    cap: usize,
}

// Safety: buf is only mutated by the single-threaded playback callback (record),
// and read_recent only reads stale-safe samples behind the atomic write cursor.
unsafe impl Send for EchoReference {}
unsafe impl Sync for EchoReference {}

impl EchoReference {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: std::cell::UnsafeCell::new(vec![0.0; capacity]),
            write: std::sync::atomic::AtomicUsize::new(0),
            cap: capacity,
        }
    }

    /// Called from the playback callback to record what was sent to speakers.
    pub fn record(&self, data: &[f32]) {
        let mut w = self.write.load(Ordering::Relaxed);
        // Safety: playback callback is the sole writer; UnsafeCell provides interior mutability.
        let buf = unsafe { &mut *self.buf.get() };
        for &s in data {
            buf[w % self.cap] = s;
            w = w.wrapping_add(1);
        }
        self.write.store(w, Ordering::Release);
    }

    /// Read the most recent `len` samples from the reference buffer.
    /// Returns a freshly filled slice (caller provides the buffer).
    pub fn read_recent(&self, out: &mut [f32]) {
        let w = self.write.load(Ordering::Acquire);
        let len = out.len().min(self.cap);
        // Safety: we only read behind the write cursor with Acquire ordering.
        let buf = unsafe { &*self.buf.get() };
        let start = w.wrapping_sub(len);
        for (i, s) in out[..len].iter_mut().enumerate() {
            *s = buf[(start.wrapping_add(i)) % self.cap];
        }
    }
}

/// Lightweight echo canceller — detects echo via cross-correlation with
/// the playback reference and attenuates the capture signal when detected.
pub(crate) struct EchoCanceller {
    enabled: bool,
    /// Pre-allocated reference buffer for reading playback audio.
    ref_buf: Vec<f32>,
}

#[allow(clippy::needless_range_loop)] // Indexed access clearer for correlation math
impl EchoCanceller {
    pub fn new(frame_size: usize) -> Self {
        Self {
            enabled: false,
            // Extra samples for delay search (up to ~10ms lookaround).
            // +1 headroom to prevent off-by-one at max offset boundary.
            ref_buf: vec![0.0; frame_size + 480 + 1],
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Process a capture frame, suppressing echo based on correlation with playback reference.
    pub fn process(&mut self, samples: &mut [f32], echo_ref: &EchoReference) {
        if !self.enabled {
            return;
        }

        let len = samples.len();
        let ref_len = len + 480; // Search window: frame + 10ms
        if self.ref_buf.len() < ref_len {
            self.ref_buf.resize(ref_len, 0.0);
        }
        echo_ref.read_recent(&mut self.ref_buf[..ref_len]);

        // Compute energy of capture frame
        let cap_energy: f32 = samples.iter().map(|&s| s * s).sum();
        if cap_energy < 1e-8 {
            return; // Silence, nothing to cancel
        }

        // Find best correlation across delay offsets (0 to 480 samples = 0-10ms)
        let mut best_corr: f32 = 0.0;
        let mut best_offset: usize = 0;
        // Check at stride of 48 (1ms steps) for efficiency
        for offset in (0..=480).step_by(48) {
            let mut corr: f32 = 0.0;
            for i in 0..len {
                corr += samples[i] * self.ref_buf[offset + i];
            }
            let abs_corr = corr.abs();
            if abs_corr > best_corr {
                best_corr = abs_corr;
                best_offset = offset;
            }
        }

        // Compute reference energy at best offset
        let ref_slice = &self.ref_buf[best_offset..best_offset + len];
        let ref_energy: f32 = ref_slice.iter().map(|&s| s * s).sum();
        if ref_energy < 1e-8 {
            return; // No playback audio
        }

        // Normalized correlation (0.0 to 1.0)
        let norm_corr = best_corr / (cap_energy.sqrt() * ref_energy.sqrt()).max(1e-8);

        // Only suppress if correlation is high enough (echo detected)
        // Threshold 0.5 = moderate echo, avoids false positives from uncorrelated speech
        if norm_corr > 0.5 {
            // Suppression factor: scale from 0 (no suppression) at 0.5 to 0.85 at 1.0
            let suppress = ((norm_corr - 0.5) * 1.7).min(0.85);
            // Compute optimal scale factor for reference subtraction
            let scale = best_corr / ref_energy.max(1e-8);
            for i in 0..len {
                samples[i] -= ref_slice[i] * scale * suppress;
            }
        }
    }
}

// ─── DSP Utilities ───

/// Compute RMS energy — O(n), no allocations
#[inline(always)]
pub fn frame_energy(pcm: &[f32]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = pcm.iter().map(|&s| s * s).sum();
    (sum_sq / pcm.len() as f32).sqrt()
}

/// Fast soft-clip — polynomial approximation, no transcendentals.
#[inline(always)]
pub fn soft_clip(x: f32) -> f32 {
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
            agc.process_with_gate(&mut samples, true);
        }
        // Now check that output is louder than input
        let mut samples = [0.01f32; FRAME_SIZE];
        agc.process_with_gate(&mut samples, true);
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
            agc.process_with_gate(&mut samples, true);
        }
        let mut samples = [0.8f32; FRAME_SIZE];
        agc.process_with_gate(&mut samples, true);
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
        agc.process_with_gate(&mut samples, true);
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

    // ─── HighPassFilter (extended) ───

    #[test]
    fn highpass_passes_voice_frequencies() {
        let mut hpf = HighPassFilter::new();
        // Prime the filter to avoid fade-in transient
        let mut warmup = [0.0f32; FRAME_SIZE];
        hpf.process(&mut warmup);

        // 300Hz tone (typical voice fundamental) should pass through
        let tau = std::f32::consts::TAU;
        let mut samples = [0.0f32; FRAME_SIZE];
        for (i, s) in samples.iter_mut().enumerate() {
            *s = (tau * 300.0 * i as f32 / 48000.0).sin() * 0.5;
        }
        let original_energy: f32 = samples.iter().map(|s| s * s).sum();
        hpf.process(&mut samples);
        let processed_energy: f32 = samples.iter().map(|s| s * s).sum();
        let ratio = processed_energy / original_energy;
        assert!(ratio > 0.85, "300Hz should pass through HPF: ratio={ratio}");
    }

    #[test]
    fn highpass_attenuates_subsonic() {
        let mut hpf = HighPassFilter::new();
        // Prime
        let mut warmup = [0.0f32; FRAME_SIZE];
        hpf.process(&mut warmup);

        // 20Hz rumble (HVAC/plosive) should be attenuated
        let tau = std::f32::consts::TAU;
        let mut samples = [0.0f32; FRAME_SIZE];
        for (i, s) in samples.iter_mut().enumerate() {
            *s = (tau * 20.0 * i as f32 / 48000.0).sin() * 0.5;
        }
        let original_energy: f32 = samples.iter().map(|s| s * s).sum();
        hpf.process(&mut samples);
        let processed_energy: f32 = samples.iter().map(|s| s * s).sum();
        let ratio = processed_energy / original_energy;
        assert!(
            ratio < 0.5,
            "20Hz should be attenuated by HPF: ratio={ratio}"
        );
    }

    #[test]
    fn highpass_fade_in_prevents_transient() {
        let mut hpf = HighPassFilter::new();
        // First frame should have fade-in (first 96 samples ramp from 0)
        let mut samples = [0.5f32; FRAME_SIZE];
        hpf.process(&mut samples);
        assert!(
            samples[0].abs() < 0.01,
            "First sample of first frame should be near zero (fade-in)"
        );
        assert!(
            samples[95].abs() > 0.0,
            "Sample at end of fade-in ramp should be non-zero"
        );
    }

    // ─── DeEsser (extended) ───

    #[test]
    fn deesser_reduces_sibilance() {
        let mut de = DeEsser::new();
        // High frequency signal (6kHz sibilant range) should be attenuated
        let tau = std::f32::consts::TAU;
        let mut samples = [0.0f32; FRAME_SIZE];
        for (i, s) in samples.iter_mut().enumerate() {
            *s = (tau * 6000.0 * i as f32 / 48000.0).sin() * 0.5;
        }
        let original_energy: f32 = samples.iter().map(|s| s * s).sum();
        de.process(&mut samples);
        let processed_energy: f32 = samples.iter().map(|s| s * s).sum();
        let ratio = processed_energy / original_energy;
        assert!(
            ratio < 0.95,
            "High freq sibilants should be reduced: ratio={ratio}"
        );
    }

    // ─── PlaybackAgc ───

    #[test]
    fn playback_agc_boosts_quiet() {
        let mut agc = PlaybackAgc::new();
        for _ in 0..100 {
            let mut samples = [0.005f32; FRAME_SIZE];
            agc.process(&mut samples);
        }
        let mut samples = [0.005f32; FRAME_SIZE];
        agc.process(&mut samples);
        let rms: f32 = (samples.iter().map(|s| s * s).sum::<f32>() / FRAME_SIZE as f32).sqrt();
        assert!(
            rms > 0.005,
            "PlaybackAgc should boost quiet signal: rms={rms}"
        );
    }

    #[test]
    fn playback_agc_attenuates_loud() {
        let mut agc = PlaybackAgc::new();
        // AGC adapts very slowly (alpha=0.001), so run many frames
        for _ in 0..5000 {
            let mut samples = [0.8f32; FRAME_SIZE];
            agc.process(&mut samples);
        }
        let mut samples = [0.8f32; FRAME_SIZE];
        agc.process(&mut samples);
        let rms: f32 = (samples.iter().map(|s| s * s).sum::<f32>() / FRAME_SIZE as f32).sqrt();
        assert!(
            rms < 0.8,
            "PlaybackAgc should attenuate loud signal: rms={rms}"
        );
    }

    #[test]
    fn playback_agc_custom_target() {
        let agc = PlaybackAgc::with_target_rms(0.3);
        assert!((agc.target_rms - 0.3).abs() < 0.01);
    }

    #[test]
    fn playback_agc_limiter() {
        let mut agc = PlaybackAgc::new();
        agc.gain = 6.0;
        let mut samples = [0.5f32; 100];
        agc.process(&mut samples);
        let max = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max <= 0.96,
            "PlaybackAgc limiter should cap output: max={max}"
        );
    }

    // ─── NoiseGate (extended) ───

    #[test]
    fn noise_gate_auto_calibration() {
        let sensitivity = Arc::new(AtomicU32::new(500));
        let mut gate = NoiseGate::new(sensitivity);

        // During first 100 frames, calibration is active
        for _ in 0..99 {
            gate.process(0.01, true);
        }
        // After 100 frames, noise floor should be calibrated
        assert_eq!(gate.calibration_frames, 99);
        gate.process(0.01, true);
        assert_eq!(gate.calibration_frames, 100);
        // noise_floor should be close to the average of the calibration energy
        assert!(
            (gate.noise_floor - 0.01).abs() < 0.005,
            "Noise floor should calibrate to ~0.01: got {}",
            gate.noise_floor
        );
    }

    #[test]
    fn noise_gate_is_silent_tracks_gain() {
        let sensitivity = Arc::new(AtomicU32::new(500));
        let mut gate = NoiseGate::new(sensitivity);
        // Initially gain is 0, so is_silent should be true
        assert!(gate.is_silent());

        // Open gate and ramp up
        gate.process(1.0, true);
        let mut samples = [0.5f32; FRAME_SIZE];
        gate.apply_gain(&mut samples, true);
        // After ramp-up, should no longer be silent
        assert!(!gate.is_silent());
    }

    // ─── soft_clip (extended) ───

    #[test]
    fn soft_clip_monotonically_increasing() {
        // soft_clip is monotonic in the practical range [-2, 2]
        // (the d/(1+d²) term peaks at d=1 then decreases, so beyond ±2 it's not monotonic)
        let mut prev = soft_clip(-2.0);
        for i in -20..=20 {
            let x = i as f32 / 10.0;
            let y = soft_clip(x);
            assert!(y >= prev, "soft_clip not monotonic at x={x}: {prev} -> {y}");
            prev = y;
        }
    }

    #[test]
    fn soft_clip_bounded() {
        // Output should approach but never reach 2.0 for positive, -2.0 for negative
        for x in [100.0, 1000.0, 1e6] {
            let y = soft_clip(x);
            assert!(y < 2.0, "soft_clip({x}) = {y} should be < 2.0");
            assert!(y > 1.0, "soft_clip({x}) = {y} should be > 1.0");
            let yn = soft_clip(-x);
            assert!(yn > -2.0, "soft_clip(-{x}) = {yn} should be > -2.0");
        }
    }

    // ─── NoiseSuppressor ───

    #[test]
    fn noise_suppressor_disabled_passthrough() {
        let mut ns = NoiseSuppressor::new();
        // When disabled, process should not modify samples
        let original = [0.5f32; FRAME_SIZE];
        let mut samples = original;
        ns.process(&mut samples);
        assert_eq!(
            samples, original,
            "Disabled NoiseSuppressor should not modify audio"
        );
    }

    #[test]
    fn noise_suppressor_enable_disable_toggle() {
        let mut ns = NoiseSuppressor::new();
        assert!(!ns.enabled);
        ns.set_enabled(true);
        assert!(ns.enabled);
        ns.set_enabled(false);
        assert!(!ns.enabled);
    }

    #[test]
    fn noise_suppressor_resets_on_enable() {
        let mut ns = NoiseSuppressor::new();
        ns.set_enabled(true);
        // Process some frames to prime
        let mut samples = [0.1f32; FRAME_SIZE];
        ns.process(&mut samples);
        // After processing a frame, primed state depends on frame energy — just verify it ran
        let _ = ns.primed;
        // Disable and re-enable should reset
        ns.set_enabled(false);
        ns.set_enabled(true);
        assert!(!ns.primed, "Re-enabling should reset primed state");
    }

    #[test]
    fn noise_suppressor_short_buffer_passthrough() {
        let mut ns = NoiseSuppressor::new();
        ns.set_enabled(true);
        // Buffer shorter than 2 sub-frames should be ignored
        let mut short = [0.5f32; 100];
        let original = short;
        ns.process(&mut short);
        assert_eq!(
            short, original,
            "Short buffer should pass through unchanged"
        );
    }

    // ─── EchoCanceller ───

    #[test]
    fn echo_canceller_disabled_passthrough() {
        let mut ec = EchoCanceller::new(FRAME_SIZE);
        let echo_ref = EchoReference::new(FRAME_SIZE * 6);
        let original = [0.3f32; FRAME_SIZE];
        let mut samples = original;
        ec.process(&mut samples, &echo_ref);
        assert_eq!(
            samples, original,
            "Disabled EchoCanceller should not modify audio"
        );
    }

    #[test]
    fn echo_canceller_suppresses_correlated_signal() {
        let mut ec = EchoCanceller::new(FRAME_SIZE);
        ec.set_enabled(true);
        let echo_ref = EchoReference::new(FRAME_SIZE * 6);

        // Generate a signal and record it as playback reference
        let tau = std::f32::consts::TAU;
        let mut signal = [0.0f32; FRAME_SIZE];
        for (i, s) in signal.iter_mut().enumerate() {
            *s = (tau * 440.0 * i as f32 / 48000.0).sin() * 0.5;
        }
        echo_ref.record(&signal);

        // Same signal in capture (simulating echo)
        let mut capture = signal;
        let original_energy: f32 = capture.iter().map(|s| s * s).sum();
        ec.process(&mut capture, &echo_ref);
        let processed_energy: f32 = capture.iter().map(|s| s * s).sum();

        // Echo should be at least partially suppressed
        assert!(
            processed_energy < original_energy,
            "Echo canceller should reduce correlated signal: original={original_energy}, processed={processed_energy}"
        );
    }

    // ─── EchoReference ───

    #[test]
    fn echo_reference_record_and_read() {
        let echo_ref = EchoReference::new(1024);
        let data: Vec<f32> = (0..100).map(|i| i as f32 * 0.01).collect();
        echo_ref.record(&data);

        let mut out = [0.0f32; 50];
        echo_ref.read_recent(&mut out);
        // Should contain the last 50 samples (0.50..0.99)
        assert!(
            (out[0] - 0.50).abs() < 0.01,
            "First read sample should be ~0.50: got {}",
            out[0]
        );
        assert!(
            (out[49] - 0.99).abs() < 0.01,
            "Last read sample should be ~0.99: got {}",
            out[49]
        );
    }

    // ─── AGC (extended) ───

    #[test]
    fn agc_gate_closed_slower_adaptation() {
        let mut agc_open = Agc::new();
        let mut agc_closed = Agc::new();

        // Feed both the same quiet signal, one with gate open, one with gate closed
        for _ in 0..20 {
            let mut samples_open = [0.01f32; FRAME_SIZE];
            let mut samples_closed = [0.01f32; FRAME_SIZE];
            agc_open.process_with_gate(&mut samples_open, true);
            agc_closed.process_with_gate(&mut samples_closed, false);
        }

        // Open-gate AGC should have adapted more aggressively
        let open_gain = agc_open.gain;
        let closed_gain = agc_closed.gain;
        assert!(
            (open_gain - 1.0).abs() >= (closed_gain - 1.0).abs() * 0.5,
            "Open-gate AGC should adapt faster: open_gain={open_gain}, closed_gain={closed_gain}"
        );
    }

    // ─── MuteRamp (extended) ───

    #[test]
    fn mute_ramp_unmute_smooth() {
        let flag = Arc::new(AtomicBool::new(false));
        let mut ramp = MuteRamp::new(flag.clone());

        // Start muted, ramp gain to 0
        let mut samples = [1.0f32; 960];
        ramp.apply(&mut samples);
        // Should be mostly silent
        assert!(samples[959] < 0.01, "Should be muted at end");

        // Now unmute
        flag.store(true, Ordering::Relaxed);
        let mut samples = [1.0f32; 960];
        ramp.apply(&mut samples);
        // First sample should be near 0 (smooth ramp up)
        assert!(samples[0] < 0.1, "Unmute ramp should start low");
        // Last sample should be near 1.0
        assert!(
            samples[959] > 0.9,
            "Unmute ramp should reach near-full by end"
        );
    }

    // ─── ComfortNoise (extended) ───

    #[test]
    fn comfort_noise_deterministic_from_seed() {
        let mut cn1 = ComfortNoise::new();
        let mut cn2 = ComfortNoise::new();
        let mut s1 = [0.0f32; 100];
        let mut s2 = [0.0f32; 100];
        cn1.fill(&mut s1);
        cn2.fill(&mut s2);
        // Same seed should produce same output
        assert_eq!(
            s1, s2,
            "Comfort noise should be deterministic from same seed"
        );
    }

    #[test]
    fn comfort_noise_amplitude_bound() {
        let mut cn = ComfortNoise::new();
        let mut samples = [0.0f32; 10000];
        cn.fill(&mut samples);
        let max = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max < 0.01,
            "Comfort noise amplitude should be very small: max={max}"
        );
    }
}

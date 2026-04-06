//! 3-band biquad equalizer for per-peer audio processing.
//!
//! Uses standard Audio EQ Cookbook biquad formulas (Robert Bristow-Johnson).
//! All state is stack-local in the playback callback — no heap allocation.
//! Coefficient recalculation only happens when gain values change (atomic snapshot).

use std::f32::consts::PI;

const SAMPLE_RATE: f32 = 48000.0;

/// Biquad filter coefficients (transposed direct form II).
#[derive(Clone, Copy)]
struct BiquadCoeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

/// Biquad filter state (2 delay elements per filter).
#[derive(Clone, Copy, Default)]
pub(crate) struct BiquadState {
    z1: f32,
    z2: f32,
}

impl BiquadState {
    /// Process one sample through the biquad (transposed direct form II).
    /// Zero-allocation, branchless hot path.
    #[inline(always)]
    fn process(&mut self, sample: f32, c: &BiquadCoeffs) -> f32 {
        let out = c.b0 * sample + self.z1;
        self.z1 = c.b1 * sample - c.a1 * out + self.z2;
        self.z2 = c.b2 * sample - c.a2 * out;
        out
    }
}

/// Low shelf biquad coefficients.
/// freq: shelf frequency in Hz, gain_db: gain in dB
fn low_shelf(freq: f32, gain_db: f32) -> BiquadCoeffs {
    if gain_db.abs() < 0.01 {
        return BiquadCoeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };
    }
    let a = 10.0f32.powf(gain_db / 40.0); // sqrt(10^(dB/20))
    let w0 = 2.0 * PI * freq / SAMPLE_RATE;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let s = 0.9; // shelf slope (slightly gentle)
    let alpha = sin_w0 / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
    let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

    let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
    let a0_inv = 1.0 / a0;

    BiquadCoeffs {
        b0: (a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha)) * a0_inv,
        b1: (2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0)) * a0_inv,
        b2: (a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha)) * a0_inv,
        a1: (-2.0 * ((a - 1.0) + (a + 1.0) * cos_w0)) * a0_inv,
        a2: ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha) * a0_inv,
    }
}

/// High shelf biquad coefficients.
fn high_shelf(freq: f32, gain_db: f32) -> BiquadCoeffs {
    if gain_db.abs() < 0.01 {
        return BiquadCoeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };
    }
    let a = 10.0f32.powf(gain_db / 40.0);
    let w0 = 2.0 * PI * freq / SAMPLE_RATE;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let s = 0.9;
    let alpha = sin_w0 / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
    let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

    let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
    let a0_inv = 1.0 / a0;

    BiquadCoeffs {
        b0: (a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha)) * a0_inv,
        b1: (-2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0)) * a0_inv,
        b2: (a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha)) * a0_inv,
        a1: (2.0 * ((a - 1.0) - (a + 1.0) * cos_w0)) * a0_inv,
        a2: ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha) * a0_inv,
    }
}

/// Peaking EQ biquad coefficients.
/// freq: center frequency, gain_db: gain in dB, q: quality factor
fn peaking(freq: f32, gain_db: f32, q: f32) -> BiquadCoeffs {
    if gain_db.abs() < 0.01 {
        return BiquadCoeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };
    }
    let a = 10.0f32.powf(gain_db / 40.0);
    let w0 = 2.0 * PI * freq / SAMPLE_RATE;
    let cos_w0 = w0.cos();
    let sin_w0 = w0.sin();
    let alpha = sin_w0 / (2.0 * q);

    let a0 = 1.0 + alpha / a;
    let a0_inv = 1.0 / a0;

    BiquadCoeffs {
        b0: (1.0 + alpha * a) * a0_inv,
        b1: (-2.0 * cos_w0) * a0_inv,
        b2: (1.0 - alpha * a) * a0_inv,
        a1: (-2.0 * cos_w0) * a0_inv,
        a2: (1.0 - alpha / a) * a0_inv,
    }
}

/// Per-peer 3-band EQ state, held per-peer in the playback callback.
/// Recalculates coefficients only when gains change (compared to cached snapshot).
pub(crate) struct PeerEqState {
    low_state: BiquadState,
    mid_state: BiquadState,
    high_state: BiquadState,
    low_coeffs: BiquadCoeffs,
    mid_coeffs: BiquadCoeffs,
    high_coeffs: BiquadCoeffs,
    /// Cached gain snapshot (millibels) to detect changes
    cached_bass_mb: i32,
    cached_mid_mb: i32,
    cached_treble_mb: i32,
}

impl PeerEqState {
    pub fn new() -> Self {
        let bypass = BiquadCoeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };
        Self {
            low_state: BiquadState::default(),
            mid_state: BiquadState::default(),
            high_state: BiquadState::default(),
            low_coeffs: bypass,
            mid_coeffs: bypass,
            high_coeffs: bypass,
            cached_bass_mb: 0,
            cached_mid_mb: 0,
            cached_treble_mb: 0,
        }
    }

    /// Update coefficients if gains have changed, then process the buffer in-place.
    /// `bass_mb`, `mid_mb`, `treble_mb` are in millibels (-600 to +600).
    #[inline]
    pub fn process(&mut self, buf: &mut [f32], bass_mb: i32, mid_mb: i32, treble_mb: i32) {
        // Skip if all bands are flat (common case)
        if bass_mb == 0 && mid_mb == 0 && treble_mb == 0 {
            // Reset cached values so we recalculate on next non-zero setting
            self.cached_bass_mb = 0;
            self.cached_mid_mb = 0;
            self.cached_treble_mb = 0;
            return;
        }

        // Recalculate coefficients only when gain values change
        if bass_mb != self.cached_bass_mb {
            self.cached_bass_mb = bass_mb;
            let db = bass_mb as f32 / 100.0;
            self.low_coeffs = low_shelf(300.0, db);
        }
        if mid_mb != self.cached_mid_mb {
            self.cached_mid_mb = mid_mb;
            let db = mid_mb as f32 / 100.0;
            self.mid_coeffs = peaking(1000.0, db, 0.7);
        }
        if treble_mb != self.cached_treble_mb {
            self.cached_treble_mb = treble_mb;
            let db = treble_mb as f32 / 100.0;
            self.high_coeffs = high_shelf(3000.0, db);
        }

        // Process each sample through all 3 bands in series
        for s in buf.iter_mut() {
            *s = self.low_state.process(*s, &self.low_coeffs);
            *s = self.mid_state.process(*s, &self.mid_coeffs);
            *s = self.high_state.process(*s, &self.high_coeffs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_when_flat() {
        let mut eq = PeerEqState::new();
        let mut buf = [0.5f32; 960];
        let original = buf;
        eq.process(&mut buf, 0, 0, 0);
        // Flat EQ should pass through unchanged
        assert_eq!(buf, original);
    }

    #[test]
    fn bass_boost_increases_low_energy() {
        let mut eq = PeerEqState::new();
        // Generate a 200Hz sine wave (should be in the bass band)
        let mut low_buf: Vec<f32> = (0..960)
            .map(|i| (2.0 * PI * 200.0 * i as f32 / SAMPLE_RATE).sin() * 0.5)
            .collect();
        let original_energy: f32 = low_buf.iter().map(|s| s * s).sum();
        eq.process(&mut low_buf, 600, 0, 0); // +6dB bass
        let boosted_energy: f32 = low_buf.iter().map(|s| s * s).sum();
        assert!(boosted_energy > original_energy * 1.5,
            "Bass boost should increase low-frequency energy: original={original_energy}, boosted={boosted_energy}");
    }

    #[test]
    fn treble_boost_increases_high_energy() {
        let mut eq = PeerEqState::new();
        // Generate a 5kHz sine wave (should be in the treble band)
        let mut high_buf: Vec<f32> = (0..960)
            .map(|i| (2.0 * PI * 5000.0 * i as f32 / SAMPLE_RATE).sin() * 0.5)
            .collect();
        let original_energy: f32 = high_buf.iter().map(|s| s * s).sum();
        eq.process(&mut high_buf, 0, 0, 600); // +6dB treble
        let boosted_energy: f32 = high_buf.iter().map(|s| s * s).sum();
        assert!(boosted_energy > original_energy * 1.5,
            "Treble boost should increase high-frequency energy: original={original_energy}, boosted={boosted_energy}");
    }

    #[test]
    fn coefficients_only_recalculate_on_change() {
        let mut eq = PeerEqState::new();
        let mut buf = [0.1f32; 960];
        eq.process(&mut buf, 300, 0, 0); // Set bass to +3dB
        let cached = eq.cached_bass_mb;
        assert_eq!(cached, 300);
        // Process again with same values — cached should stay the same
        buf = [0.1f32; 960];
        eq.process(&mut buf, 300, 0, 0);
        assert_eq!(eq.cached_bass_mb, 300);
    }
}

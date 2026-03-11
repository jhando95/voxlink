//! Keybind feedback tones — distinct sounds for mute/unmute/deafen/undeafen.
//! Zero-allocation at runtime: tone samples are pre-computed once.
//! Two-tone design: lower pitch = action on (muted/deafened), higher = action off.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use shared_types::SAMPLE_RATE;

/// Duration of the feedback beep in milliseconds.
const TONE_MS: u32 = 50;
/// Volume of the feedback beep (0.0–1.0).
const TONE_VOLUME: f32 = 0.12;
/// Fade envelope length in samples (~1ms)
const FADE_SAMPLES: usize = 48;

const TONE_SAMPLES: usize = (SAMPLE_RATE * TONE_MS / 1000) as usize;

// Frequencies for each action:
const FREQ_MUTE_ON: f32 = 440.0;    // A4 — lower = muted
const FREQ_MUTE_OFF: f32 = 880.0;   // A5 — higher = unmuted
const FREQ_DEAFEN_ON: f32 = 330.0;  // E4 — even lower = deafened
const FREQ_DEAFEN_OFF: f32 = 660.0; // E5 — mid = undeafened

/// Feedback action types
#[derive(Clone, Copy)]
pub(crate) enum FeedbackAction {
    MuteOn,
    MuteOff,
    DeafenOn,
    DeafenOff,
}

fn generate_tone(freq: f32) -> Vec<f32> {
    let mut buf = vec![0.0f32; TONE_SAMPLES];
    let tau = std::f32::consts::TAU;
    for i in 0..TONE_SAMPLES {
        let t = i as f32 / SAMPLE_RATE as f32;
        let envelope = if i < FADE_SAMPLES {
            i as f32 / FADE_SAMPLES as f32
        } else if i > TONE_SAMPLES - FADE_SAMPLES {
            (TONE_SAMPLES - i) as f32 / FADE_SAMPLES as f32
        } else {
            1.0
        };
        buf[i] = (tau * freq * t).sin() * TONE_VOLUME * envelope;
    }
    buf
}

/// Pre-computed feedback tone buffers for all actions.
pub(crate) struct FeedbackTone {
    tones: [Arc<[f32]>; 4], // MuteOn, MuteOff, DeafenOn, DeafenOff
    /// Which tone to play (index*TONE_SAMPLES + position, 0 = idle)
    cursor: Arc<AtomicU32>,
    /// Which tone index is active
    active_tone: Arc<AtomicU32>,
}

impl FeedbackTone {
    pub fn new() -> Self {
        Self {
            tones: [
                Arc::from(generate_tone(FREQ_MUTE_ON)),
                Arc::from(generate_tone(FREQ_MUTE_OFF)),
                Arc::from(generate_tone(FREQ_DEAFEN_ON)),
                Arc::from(generate_tone(FREQ_DEAFEN_OFF)),
            ],
            cursor: Arc::new(AtomicU32::new(0)),
            active_tone: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Get cloneable handles for use in playback callback.
    pub fn playback_state(&self) -> FeedbackPlayback {
        FeedbackPlayback {
            tones: self.tones.clone(),
            cursor: self.cursor.clone(),
            active_tone: self.active_tone.clone(),
        }
    }

    /// Trigger a specific feedback action.
    pub fn trigger(&self, action: FeedbackAction) {
        let idx = action as u32;
        self.active_tone.store(idx, Ordering::Relaxed);
        self.cursor.store(1, Ordering::Relaxed); // 1-indexed, 0 = idle
    }
}

/// Cloneable state for use in the playback callback.
pub(crate) struct FeedbackPlayback {
    tones: [Arc<[f32]>; 4],
    cursor: Arc<AtomicU32>,
    active_tone: Arc<AtomicU32>,
}

impl FeedbackPlayback {
    /// Mix the active tone into the output buffer. Called from playback callback.
    #[inline]
    pub fn mix_into(&self, dest: &mut [f32]) {
        let pos = self.cursor.load(Ordering::Relaxed) as usize;
        if pos == 0 {
            return;
        }
        let tone_idx = self.active_tone.load(Ordering::Relaxed) as usize;
        if tone_idx >= self.tones.len() {
            return;
        }
        let samples = &self.tones[tone_idx];
        let start = pos - 1;
        if start >= samples.len() {
            self.cursor.store(0, Ordering::Relaxed);
            return;
        }
        let remaining = samples.len() - start;
        let count = dest.len().min(remaining);
        for i in 0..count {
            dest[i] += samples[start + i];
        }
        let new_pos = start + count + 1;
        if new_pos > samples.len() {
            self.cursor.store(0, Ordering::Relaxed);
        } else {
            self.cursor.store(new_pos as u32, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_generation_all_actions() {
        let tone = FeedbackTone::new();
        for (i, t) in tone.tones.iter().enumerate() {
            assert_eq!(t.len(), TONE_SAMPLES, "Tone {i} wrong length");
            // Verify non-silent in middle region (single sample may hit a zero crossing)
            let mid = TONE_SAMPLES / 2;
            let peak = t[mid.saturating_sub(25)..=(mid + 25).min(t.len() - 1)]
                .iter().map(|s| s.abs()).fold(0.0f32, f32::max);
            assert!(peak > 0.01, "Tone {i} silent in middle region");
            // Verify fade-in/out
            assert!(t[0].abs() < 0.01, "Tone {i} no fade-in");
            assert!(t[TONE_SAMPLES - 1].abs() < 0.01, "Tone {i} no fade-out");
        }
    }

    #[test]
    fn distinct_frequencies() {
        let tone = FeedbackTone::new();
        // Mute on and off should sound different
        let on_energy: f32 = tone.tones[0].iter().map(|s| s * s).sum();
        let off_energy: f32 = tone.tones[1].iter().map(|s| s * s).sum();
        // Both should have similar total energy but different waveforms
        assert!((on_energy - off_energy).abs() / on_energy < 0.3,
            "Similar energy expected");
        // But waveforms should differ (different frequencies)
        let diff: f32 = tone.tones[0].iter().zip(tone.tones[1].iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(diff > 0.1, "Tones should be distinct");
    }

    #[test]
    fn trigger_and_mix() {
        let tone = FeedbackTone::new();
        let playback = tone.playback_state();

        // Initially idle
        let mut buf = [0.0f32; 100];
        playback.mix_into(&mut buf);
        assert_eq!(buf[0], 0.0);

        // Trigger mute-off (higher pitch)
        tone.trigger(FeedbackAction::MuteOff);
        playback.mix_into(&mut buf);
        assert!(buf[0].abs() > 0.0 || buf[1].abs() > 0.0);
    }

    #[test]
    fn trigger_different_actions() {
        let tone = FeedbackTone::new();
        let playback = tone.playback_state();

        // Trigger mute on
        tone.trigger(FeedbackAction::MuteOn);
        let mut buf1 = [0.0f32; TONE_SAMPLES];
        playback.mix_into(&mut buf1);

        // Trigger deafen on
        tone.trigger(FeedbackAction::DeafenOn);
        let mut buf2 = [0.0f32; TONE_SAMPLES];
        playback.mix_into(&mut buf2);

        // Should produce different waveforms
        let diff: f32 = buf1.iter().zip(buf2.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(diff > 0.1, "Different actions should produce different tones");
    }
}

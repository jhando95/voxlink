//! Keybind feedback tones — distinct sounds for mute/unmute/deafen/undeafen.
//! Zero-allocation at runtime: tone samples are pre-computed once.
//! Lower pitch = action on (muted/deafened), higher = action off.

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;

use shared_types::SAMPLE_RATE;

/// Duration of the feedback beep in milliseconds.
const TONE_MS: u32 = 80;
/// Duration of the speaker preview tone in milliseconds (longer so user can clearly hear it).
const PREVIEW_TONE_MS: u32 = 300;
/// Volume of the feedback beep (0.0–1.0).
const TONE_VOLUME: f32 = 0.25;
/// Volume of the speaker preview tone (louder for clear audibility).
const PREVIEW_VOLUME: f32 = 0.45;
/// Fade envelope length in samples (~2ms)
const FADE_SAMPLES: usize = 96;

const TONE_SAMPLES: usize = (SAMPLE_RATE * TONE_MS / 1000) as usize;

// Frequencies for each action:
const FREQ_MUTE_ON: f32 = 440.0; // A4 — lower = muted
const FREQ_MUTE_OFF: f32 = 880.0; // A5 — higher = unmuted
const FREQ_DEAFEN_ON: f32 = 330.0; // E4 — even lower = deafened
const FREQ_DEAFEN_OFF: f32 = 660.0; // E5 — mid = undeafened
const FREQ_PREVIEW_ON: f32 = 523.25; // C5
const FREQ_PREVIEW_OFF: f32 = 659.25; // E5

/// Feedback action types
#[derive(Clone, Copy)]
pub(crate) enum FeedbackAction {
    MuteOn,
    MuteOff,
    DeafenOn,
    DeafenOff,
    OutputPreview,
    JoinRoom,
    LeaveRoom,
}

fn generate_tone(freq: f32) -> Vec<f32> {
    let mut buf = vec![0.0f32; TONE_SAMPLES];
    let tau = std::f32::consts::TAU;
    for (i, sample) in buf.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        let envelope = if i < FADE_SAMPLES {
            i as f32 / FADE_SAMPLES as f32
        } else if i > TONE_SAMPLES - FADE_SAMPLES {
            (TONE_SAMPLES - i) as f32 / FADE_SAMPLES as f32
        } else {
            1.0
        };
        *sample = (tau * freq * t).sin() * TONE_VOLUME * envelope;
    }
    buf
}

const PREVIEW_SAMPLES: usize = (SAMPLE_RATE * PREVIEW_TONE_MS / 1000) as usize;

const JOIN_LEAVE_MS: u32 = 120;
const JOIN_LEAVE_SAMPLES: usize = (SAMPLE_RATE * JOIN_LEAVE_MS / 1000) as usize;
const FREQ_JOIN_LOW: f32 = 523.25; // C5
const FREQ_JOIN_HIGH: f32 = 659.25; // E5
const FREQ_LEAVE_LOW: f32 = 659.25; // E5
const FREQ_LEAVE_HIGH: f32 = 523.25; // C5
const JOIN_LEAVE_VOLUME: f32 = 0.20;

fn generate_join_leave_tone(freq_first: f32, freq_second: f32) -> Vec<f32> {
    let mut buf = vec![0.0f32; JOIN_LEAVE_SAMPLES];
    let tau = std::f32::consts::TAU;
    let split = JOIN_LEAVE_SAMPLES / 2;
    for (i, sample) in buf.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        let freq = if i < split { freq_first } else { freq_second };
        let envelope = if i < FADE_SAMPLES {
            i as f32 / FADE_SAMPLES as f32
        } else if i > JOIN_LEAVE_SAMPLES - FADE_SAMPLES {
            (JOIN_LEAVE_SAMPLES - i) as f32 / FADE_SAMPLES as f32
        } else {
            1.0
        };
        *sample = (tau * freq * t).sin() * JOIN_LEAVE_VOLUME * envelope;
    }
    buf
}

fn generate_preview_tone() -> Vec<f32> {
    let mut buf = vec![0.0f32; PREVIEW_SAMPLES];
    let tau = std::f32::consts::TAU;
    let split = PREVIEW_SAMPLES / 2;
    for (i, sample) in buf.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        let freq = if i < split {
            FREQ_PREVIEW_ON
        } else {
            FREQ_PREVIEW_OFF
        };
        let envelope = if i < FADE_SAMPLES {
            i as f32 / FADE_SAMPLES as f32
        } else if i > PREVIEW_SAMPLES - FADE_SAMPLES {
            (PREVIEW_SAMPLES - i) as f32 / FADE_SAMPLES as f32
        } else {
            1.0
        };
        *sample = (tau * freq * t).sin() * PREVIEW_VOLUME * envelope;
    }
    buf
}

/// Notification sound style index values.
/// 0 = default, 1 = subtle, 2 = chime, 3 = none
const STYLE_DEFAULT: u8 = 0;
const STYLE_SUBTLE: u8 = 1;
const STYLE_CHIME: u8 = 2;
const STYLE_NONE: u8 = 3;

/// "Subtle" style: quieter, shorter tones (half duration, half volume).
const SUBTLE_TONE_MS: u32 = 40;
const SUBTLE_TONE_SAMPLES: usize = (SAMPLE_RATE * SUBTLE_TONE_MS / 1000) as usize;
const SUBTLE_VOLUME: f32 = 0.12;

fn generate_subtle_tone(freq: f32) -> Vec<f32> {
    let mut buf = vec![0.0f32; SUBTLE_TONE_SAMPLES];
    let tau = std::f32::consts::TAU;
    let fade = FADE_SAMPLES.min(SUBTLE_TONE_SAMPLES / 2);
    for (i, sample) in buf.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        let envelope = if i < fade {
            i as f32 / fade as f32
        } else if i > SUBTLE_TONE_SAMPLES - fade {
            (SUBTLE_TONE_SAMPLES - i) as f32 / fade as f32
        } else {
            1.0
        };
        *sample = (tau * freq * t).sin() * SUBTLE_VOLUME * envelope;
    }
    buf
}

/// "Chime" style: two-note ascending pattern, softer and more musical.
const CHIME_TONE_MS: u32 = 120;
const CHIME_TONE_SAMPLES: usize = (SAMPLE_RATE * CHIME_TONE_MS / 1000) as usize;
const CHIME_VOLUME: f32 = 0.18;

fn generate_chime_tone(freq_low: f32, freq_high: f32) -> Vec<f32> {
    let mut buf = vec![0.0f32; CHIME_TONE_SAMPLES];
    let tau = std::f32::consts::TAU;
    let split = CHIME_TONE_SAMPLES / 2;
    for (i, sample) in buf.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        let freq = if i < split { freq_low } else { freq_high };
        let envelope = if i < FADE_SAMPLES {
            i as f32 / FADE_SAMPLES as f32
        } else if i > CHIME_TONE_SAMPLES - FADE_SAMPLES {
            (CHIME_TONE_SAMPLES - i) as f32 / FADE_SAMPLES as f32
        } else {
            1.0
        };
        *sample = (tau * freq * t).sin() * CHIME_VOLUME * envelope;
    }
    buf
}

/// Pre-computed feedback tone buffers for all actions.
pub(crate) struct FeedbackTone {
    tones: [Arc<[f32]>; 7], // MuteOn, MuteOff, DeafenOn, DeafenOff, OutputPreview, JoinRoom, LeaveRoom
    subtle_tones: [Arc<[f32]>; 7],
    chime_tones: [Arc<[f32]>; 7],
    /// Which tone to play (index*TONE_SAMPLES + position, 0 = idle)
    cursor: Arc<AtomicU32>,
    /// Which tone index is active
    active_tone: Arc<AtomicU32>,
    /// Sound style: 0=default, 1=subtle, 2=chime, 3=none
    sound_style: Arc<AtomicU8>,
}

impl FeedbackTone {
    pub fn new() -> Self {
        Self {
            tones: [
                Arc::from(generate_tone(FREQ_MUTE_ON)),
                Arc::from(generate_tone(FREQ_MUTE_OFF)),
                Arc::from(generate_tone(FREQ_DEAFEN_ON)),
                Arc::from(generate_tone(FREQ_DEAFEN_OFF)),
                Arc::from(generate_preview_tone()),
                Arc::from(generate_join_leave_tone(FREQ_JOIN_LOW, FREQ_JOIN_HIGH)),
                Arc::from(generate_join_leave_tone(FREQ_LEAVE_LOW, FREQ_LEAVE_HIGH)),
            ],
            subtle_tones: [
                Arc::from(generate_subtle_tone(FREQ_MUTE_ON)),
                Arc::from(generate_subtle_tone(FREQ_MUTE_OFF)),
                Arc::from(generate_subtle_tone(FREQ_DEAFEN_ON)),
                Arc::from(generate_subtle_tone(FREQ_DEAFEN_OFF)),
                Arc::from(generate_preview_tone()), // preview always uses default
                Arc::from(generate_subtle_tone(FREQ_JOIN_LOW)),
                Arc::from(generate_subtle_tone(FREQ_LEAVE_LOW)),
            ],
            chime_tones: [
                Arc::from(generate_chime_tone(FREQ_MUTE_ON, FREQ_MUTE_ON * 1.25)),
                Arc::from(generate_chime_tone(FREQ_MUTE_OFF * 0.8, FREQ_MUTE_OFF)),
                Arc::from(generate_chime_tone(FREQ_DEAFEN_ON, FREQ_DEAFEN_ON * 1.25)),
                Arc::from(generate_chime_tone(FREQ_DEAFEN_OFF * 0.8, FREQ_DEAFEN_OFF)),
                Arc::from(generate_preview_tone()), // preview always uses default
                Arc::from(generate_chime_tone(523.25, 783.99)), // C5 -> G5
                Arc::from(generate_chime_tone(783.99, 523.25)), // G5 -> C5
            ],
            cursor: Arc::new(AtomicU32::new(0)),
            active_tone: Arc::new(AtomicU32::new(0)),
            sound_style: Arc::new(AtomicU8::new(STYLE_DEFAULT)),
        }
    }

    /// Set the notification sound style from a config string.
    /// Valid values: "default", "subtle", "chime", "none".
    pub fn set_sound_style(&self, style: &str) {
        let val = match style {
            "subtle" => STYLE_SUBTLE,
            "chime" => STYLE_CHIME,
            "none" => STYLE_NONE,
            _ => STYLE_DEFAULT,
        };
        self.sound_style.store(val, Ordering::Relaxed);
    }

    /// Get cloneable handles for use in playback callback.
    pub fn playback_state(&self) -> FeedbackPlayback {
        FeedbackPlayback {
            tones: self.tones.clone(),
            subtle_tones: self.subtle_tones.clone(),
            chime_tones: self.chime_tones.clone(),
            cursor: self.cursor.clone(),
            active_tone: self.active_tone.clone(),
            sound_style: self.sound_style.clone(),
        }
    }

    /// Trigger a specific feedback action.
    pub fn trigger(&self, action: FeedbackAction) {
        // "none" style: skip all sounds
        if self.sound_style.load(Ordering::Relaxed) == STYLE_NONE {
            return;
        }
        let idx = action as u32;
        self.active_tone.store(idx, Ordering::Relaxed);
        self.cursor.store(1, Ordering::Relaxed); // 1-indexed, 0 = idle
    }
}

/// Cloneable state for use in the playback callback.
pub(crate) struct FeedbackPlayback {
    tones: [Arc<[f32]>; 7],
    subtle_tones: [Arc<[f32]>; 7],
    chime_tones: [Arc<[f32]>; 7],
    cursor: Arc<AtomicU32>,
    active_tone: Arc<AtomicU32>,
    sound_style: Arc<AtomicU8>,
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
        let style = self.sound_style.load(Ordering::Relaxed);
        let samples = match style {
            STYLE_SUBTLE => &self.subtle_tones[tone_idx],
            STYLE_CHIME => &self.chime_tones[tone_idx],
            _ => &self.tones[tone_idx],
        };
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

// ─── Soundboard ───

/// Maximum number of loaded clips to prevent excessive memory use.
const MAX_SOUNDBOARD_CLIPS: usize = 16;
/// Volume multiplier for soundboard clips (0.0–1.0).
const CLIP_VOLUME: f32 = 0.35;

/// A loaded, decoded soundboard clip ready for playback.
struct SoundboardClip {
    samples: Arc<[f32]>,
}

/// Soundboard: load WAV clips and play them into the capture stream.
/// Lock-free playback via atomics (same pattern as FeedbackTone).
pub(crate) struct Soundboard {
    clips: std::sync::Mutex<Vec<SoundboardClip>>,
    /// Active clip index (u32::MAX = idle)
    active_clip: Arc<AtomicU32>,
    /// Playback cursor position (0 = idle)
    cursor: Arc<AtomicU32>,
}

impl Soundboard {
    pub fn new() -> Self {
        Self {
            clips: std::sync::Mutex::new(Vec::new()),
            active_clip: Arc::new(AtomicU32::new(u32::MAX)),
            cursor: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Load a WAV file and add it as a soundboard clip. Returns the clip index.
    /// Resamples to 48kHz mono if needed. Caps at MAX_SOUNDBOARD_CLIPS.
    pub fn load_clip(&self, path: &str) -> Result<usize, String> {
        let reader = hound::WavReader::open(path)
            .map_err(|e| format!("Failed to open WAV {path}: {e}"))?;
        let spec = reader.spec();
        let raw_samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .into_samples::<f32>()
                .filter_map(|s| s.ok())
                .collect(),
            hound::SampleFormat::Int => {
                let bits = spec.bits_per_sample;
                let max_val = (1u32 << (bits - 1)) as f32;
                reader
                    .into_samples::<i32>()
                    .filter_map(|s| s.ok())
                    .map(|s| s as f32 / max_val)
                    .collect()
            }
        };

        // Mix to mono if stereo
        let mono = if spec.channels > 1 {
            let ch = spec.channels as usize;
            raw_samples
                .chunks(ch)
                .map(|frame| frame.iter().sum::<f32>() / ch as f32)
                .collect()
        } else {
            raw_samples
        };

        // Simple resample if not 48kHz (nearest-neighbor, good enough for soundboard)
        let resampled = if spec.sample_rate != SAMPLE_RATE {
            let ratio = spec.sample_rate as f64 / SAMPLE_RATE as f64;
            let out_len = (mono.len() as f64 / ratio) as usize;
            (0..out_len)
                .map(|i| {
                    let src_idx = ((i as f64 * ratio) as usize).min(mono.len() - 1);
                    mono[src_idx] * CLIP_VOLUME
                })
                .collect::<Vec<f32>>()
        } else {
            mono.iter().map(|&s| s * CLIP_VOLUME).collect()
        };

        let mut clips = self.clips.lock().map_err(|_| "Lock poisoned".to_string())?;
        if clips.len() >= MAX_SOUNDBOARD_CLIPS {
            return Err(format!("Maximum {MAX_SOUNDBOARD_CLIPS} clips allowed"));
        }
        let idx = clips.len();
        clips.push(SoundboardClip {
            samples: Arc::from(resampled),
        });
        Ok(idx)
    }

    /// Clear all loaded clips.
    pub fn clear(&self) {
        if let Ok(mut clips) = self.clips.lock() {
            clips.drain(..);
        }
        self.active_clip.store(u32::MAX, Ordering::Relaxed);
        self.cursor.store(0, Ordering::Relaxed);
    }

    /// Trigger playback of clip at the given index.
    pub fn play(&self, index: usize) {
        let count = self.clip_count();
        if index < count {
            self.active_clip.store(index as u32, Ordering::Relaxed);
            self.cursor.store(1, Ordering::Relaxed); // 1-indexed, 0 = idle
        }
    }

    /// Get the number of loaded clips.
    pub fn clip_count(&self) -> usize {
        match self.clips.lock() {
            Ok(clips) => clips.len(),
            Err(_) => 0,
        }
    }

    /// Get cloneable state for the capture callback to mix clips into outgoing audio.
    pub fn capture_state(&self) -> SoundboardPlayback {
        let clip_samples: Vec<Arc<[f32]>> = match self.clips.lock() {
            Ok(clips) => clips.iter().map(|clip| clip.samples.clone()).collect(),
            Err(_) => Vec::new(),
        };
        SoundboardPlayback {
            clips: clip_samples,
            active_clip: self.active_clip.clone(),
            cursor: self.cursor.clone(),
        }
    }
}

/// Lock-free playback state for the capture callback.
pub(crate) struct SoundboardPlayback {
    clips: Vec<Arc<[f32]>>,
    active_clip: Arc<AtomicU32>,
    cursor: Arc<AtomicU32>,
}

impl SoundboardPlayback {
    /// Mix the active soundboard clip into the capture buffer (sent to other peers).
    #[inline]
    pub fn mix_into(&self, dest: &mut [f32]) {
        let pos = self.cursor.load(Ordering::Relaxed) as usize;
        if pos == 0 {
            return;
        }
        let clip_idx = self.active_clip.load(Ordering::Relaxed) as usize;
        if clip_idx >= self.clips.len() {
            self.cursor.store(0, Ordering::Relaxed);
            return;
        }
        let samples = &self.clips[clip_idx];
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
        let expected_lens = [
            TONE_SAMPLES,
            TONE_SAMPLES,
            TONE_SAMPLES,
            TONE_SAMPLES,
            PREVIEW_SAMPLES,
            JOIN_LEAVE_SAMPLES,
            JOIN_LEAVE_SAMPLES,
        ];
        for (i, t) in tone.tones.iter().enumerate() {
            assert_eq!(t.len(), expected_lens[i], "Tone {i} wrong length");
            // Verify non-silent in middle region (single sample may hit a zero crossing)
            let mid = t.len() / 2;
            let peak = t[mid.saturating_sub(25)..=(mid + 25).min(t.len() - 1)]
                .iter()
                .map(|s| s.abs())
                .fold(0.0f32, f32::max);
            assert!(peak > 0.01, "Tone {i} silent in middle region");
            // Verify fade-in/out
            assert!(t[0].abs() < 0.01, "Tone {i} no fade-in");
            assert!(t[t.len() - 1].abs() < 0.01, "Tone {i} no fade-out");
        }
    }

    #[test]
    fn distinct_frequencies() {
        let tone = FeedbackTone::new();
        // Mute on and off should sound different
        let on_energy: f32 = tone.tones[0].iter().map(|s| s * s).sum();
        let off_energy: f32 = tone.tones[1].iter().map(|s| s * s).sum();
        // Both should have similar total energy but different waveforms
        assert!(
            (on_energy - off_energy).abs() / on_energy < 0.3,
            "Similar energy expected"
        );
        // But waveforms should differ (different frequencies)
        let diff: f32 = tone.tones[0]
            .iter()
            .zip(tone.tones[1].iter())
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
        let diff: f32 = buf1
            .iter()
            .zip(buf2.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(
            diff > 0.1,
            "Different actions should produce different tones"
        );
    }

    #[test]
    fn join_leave_tones_exist_and_correct_length() {
        let tone = FeedbackTone::new();
        // JoinRoom is index 5, LeaveRoom is index 6
        let join_tone = &tone.tones[FeedbackAction::JoinRoom as usize];
        let leave_tone = &tone.tones[FeedbackAction::LeaveRoom as usize];

        assert_eq!(join_tone.len(), JOIN_LEAVE_SAMPLES);
        assert_eq!(leave_tone.len(), JOIN_LEAVE_SAMPLES);

        // Both should have non-zero audio content
        let join_energy: f32 = join_tone.iter().map(|s| s * s).sum();
        let leave_energy: f32 = leave_tone.iter().map(|s| s * s).sum();
        assert!(join_energy > 0.01, "Join tone should have energy");
        assert!(leave_energy > 0.01, "Leave tone should have energy");

        // Join and leave tones should differ (different frequency order)
        let diff: f32 = join_tone
            .iter()
            .zip(leave_tone.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(diff > 0.1, "Join and leave tones should be distinct");
    }
}

use shared_types::MicMode;

/// Manages local voice state: mute, deafen, mic mode.
/// Single source of truth for voice controls — UI and audio engine read from here.
pub struct VoiceSession {
    pub mic_mode: MicMode,
    pub is_muted: bool,
    pub is_deafened: bool,
    was_muted_before_deafen: bool,
}

impl VoiceSession {
    pub fn new() -> Self {
        Self {
            mic_mode: MicMode::default(),
            is_muted: false,
            is_deafened: false,
            was_muted_before_deafen: false,
        }
    }

    pub fn toggle_mute(&mut self) {
        self.is_muted = !self.is_muted;
    }

    pub fn toggle_deafen(&mut self) {
        self.is_deafened = !self.is_deafened;
        if self.is_deafened {
            self.was_muted_before_deafen = self.is_muted;
            self.is_muted = true;
        } else {
            self.is_muted = self.was_muted_before_deafen;
        }
    }

    pub fn set_mic_mode(&mut self, mode: MicMode) {
        self.mic_mode = mode;
    }

    /// Reset voice state for leaving a room.
    pub fn reset(&mut self) {
        self.is_muted = false;
        self.is_deafened = false;
    }
}

impl Default for VoiceSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_mute() {
        let mut session = VoiceSession::new();
        assert!(!session.is_muted);
        session.toggle_mute();
        assert!(session.is_muted);
        session.toggle_mute();
        assert!(!session.is_muted);
    }

    #[test]
    fn toggle_deafen_sets_mute() {
        let mut session = VoiceSession::new();
        assert!(!session.is_deafened);
        assert!(!session.is_muted);

        // Deafen should also mute
        session.toggle_deafen();
        assert!(session.is_deafened);
        assert!(session.is_muted);

        // Un-deafen should restore original mute state (was not muted)
        session.toggle_deafen();
        assert!(!session.is_deafened);
        assert!(!session.is_muted);
    }

    #[test]
    fn deafen_remembers_mute_state() {
        let mut session = VoiceSession::new();

        // Mute first, then deafen
        session.toggle_mute();
        assert!(session.is_muted);

        session.toggle_deafen();
        assert!(session.is_deafened);
        assert!(session.is_muted);

        // Un-deafen should restore muted state (was muted before deafen)
        session.toggle_deafen();
        assert!(!session.is_deafened);
        assert!(session.is_muted); // was muted before deafen, so stays muted
    }

    #[test]
    fn reset_clears_state() {
        let mut session = VoiceSession::new();
        session.toggle_mute();
        session.toggle_deafen();
        assert!(session.is_muted);
        assert!(session.is_deafened);

        session.reset();
        assert!(!session.is_muted);
        assert!(!session.is_deafened);
    }

    #[test]
    fn set_mic_mode() {
        let mut session = VoiceSession::new();
        assert_eq!(session.mic_mode, MicMode::OpenMic);

        session.set_mic_mode(MicMode::PushToTalk);
        assert_eq!(session.mic_mode, MicMode::PushToTalk);

        session.set_mic_mode(MicMode::OpenMic);
        assert_eq!(session.mic_mode, MicMode::OpenMic);
    }

    #[test]
    fn double_deafen_is_idempotent() {
        let mut session = VoiceSession::new();
        session.toggle_deafen();
        assert!(session.is_deafened);
        assert!(session.is_muted);

        // Deafen again — should undeafen
        session.toggle_deafen();
        assert!(!session.is_deafened);
        assert!(!session.is_muted);
    }

    #[test]
    fn rapid_mute_toggle_consistency() {
        let mut session = VoiceSession::new();
        // Rapid toggle should always end up at the opposite state
        for _ in 0..100 {
            session.toggle_mute();
        }
        // 100 toggles from false → should be false (even number)
        assert!(!session.is_muted);

        session.toggle_mute();
        assert!(session.is_muted);
    }

    #[test]
    fn deafen_then_mute_then_undeafen_stays_muted() {
        let mut session = VoiceSession::new();
        // Start unmuted
        assert!(!session.is_muted);

        // Deafen (auto-mutes)
        session.toggle_deafen();
        assert!(session.is_deafened);
        assert!(session.is_muted);

        // Undeafen — should restore to NOT muted (was not muted before deafen)
        session.toggle_deafen();
        assert!(!session.is_deafened);
        assert!(!session.is_muted);
    }

    #[test]
    fn reset_after_complex_state() {
        let mut session = VoiceSession::new();
        session.toggle_mute();
        session.toggle_deafen();
        session.set_mic_mode(MicMode::PushToTalk);
        assert!(session.is_muted);
        assert!(session.is_deafened);
        assert_eq!(session.mic_mode, MicMode::PushToTalk);

        session.reset();
        assert!(!session.is_muted);
        assert!(!session.is_deafened);
        // reset doesn't change mic mode — it's a preference, not session state
        assert_eq!(session.mic_mode, MicMode::PushToTalk);
    }

    #[test]
    fn default_trait() {
        let session = VoiceSession::default();
        assert!(!session.is_muted);
        assert!(!session.is_deafened);
        assert_eq!(session.mic_mode, MicMode::OpenMic);
    }
}

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub push_to_talk_key: Option<String>,
    pub open_mic_sensitivity: f32,
    pub mic_mode: String,
    #[serde(default = "default_user_name")]
    pub user_name: String,
    #[serde(default = "default_server_address")]
    pub server_address: String,
    #[serde(default)]
    pub last_room_code: Option<String>,
    #[serde(default)]
    pub window_width: Option<u32>,
    #[serde(default)]
    pub window_height: Option<u32>,
    #[serde(default)]
    pub mute_key: Option<String>,
    #[serde(default)]
    pub deafen_key: Option<String>,
    #[serde(default)]
    pub dark_mode: Option<bool>,
    #[serde(default = "default_theme_preset")]
    pub theme_preset: String,
    #[serde(default)]
    pub saved_spaces: Vec<SavedSpace>,
    #[serde(default)]
    pub last_space_id: Option<String>,
    #[serde(default)]
    pub last_channel_id: Option<String>,
    #[serde(default = "default_feedback_sound")]
    pub feedback_sound: bool,
    #[serde(default = "default_noise_suppression")]
    pub noise_suppression: f32,
    #[serde(default)]
    pub neural_noise_suppression: bool,
    #[serde(default)]
    pub echo_cancellation: bool,
    #[serde(default = "default_volume")]
    pub input_volume: f32,
    #[serde(default = "default_volume")]
    pub output_volume: f32,
    #[serde(default = "default_notifications")]
    pub notifications_enabled: bool,
    #[serde(default)]
    pub auth_token: Option<String>,
    #[serde(default = "default_minimize_to_tray")]
    pub minimize_to_tray: bool,
    #[serde(default)]
    pub member_widget_visible: bool,
    #[serde(default)]
    pub member_widget_x: Option<i32>,
    #[serde(default)]
    pub member_widget_y: Option<i32>,
    #[serde(default)]
    pub favorite_friends: Vec<shared_types::FavoriteFriend>,
    #[serde(default)]
    pub recent_direct_messages: Vec<shared_types::DirectMessageThread>,
    /// Per-peer volume adjustments (peer_name -> volume 0.0-2.0). Persisted across sessions.
    #[serde(default)]
    pub peer_volumes: std::collections::HashMap<String, f32>,
    /// Per-peer 3-band EQ settings (peer_name -> [bass, mid, treble] in millibels).
    #[serde(default)]
    pub peer_eq_settings: std::collections::HashMap<String, [i32; 3]>,
    /// Per-peer stereo pan position (peer_name -> -100..+100).
    #[serde(default)]
    pub peer_pan: std::collections::HashMap<String, i32>,
    /// Private user notes (user_id -> note text). Local only, never sent to server.
    #[serde(default)]
    pub user_notes: std::collections::HashMap<String, String>,
    /// Saved servers for easy switching between multiple voice servers.
    #[serde(default)]
    pub saved_servers: Vec<SavedServer>,
    /// Play notification sounds when peers join or leave the room.
    #[serde(default = "default_true")]
    pub join_leave_sounds: bool,
    /// Show spoiler text by default instead of hiding it.
    #[serde(default)]
    pub show_spoilers: bool,
    /// Use compact chat message layout (less padding, smaller font).
    #[serde(default)]
    pub compact_chat: bool,
    /// Streamer mode: hide IPs, invite codes, room codes, and email from the UI.
    #[serde(default)]
    pub streamer_mode: bool,
    /// List of blocked user IDs. Messages from these users are filtered client-side.
    #[serde(default)]
    pub blocked_users: Vec<String>,
    /// User status preset (online, idle, dnd, invisible).
    #[serde(default = "default_status_preset")]
    pub status_preset: String,
    /// Minutes of inactivity before auto-idle (0 = disabled).
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_mins: u32,
    /// Per-channel notification overrides: channel_id -> "all"/"mentions"/"none".
    #[serde(default)]
    pub channel_notification_overrides: std::collections::HashMap<String, String>,
    /// Volume ducking amount (0.0 = disabled, 0.5 = 50% duck). Applied to non-speaking peers.
    #[serde(default)]
    pub ducking_amount: f32,
    /// Energy threshold above which a peer is considered "speaking" for ducking.
    #[serde(default = "default_ducking_threshold")]
    pub ducking_threshold: f32,
    /// Soundboard clip configurations.
    #[serde(default)]
    pub soundboard_clips: Vec<SoundboardClipConfig>,
    /// Account email (set after login/registration). Used to pre-fill login form.
    #[serde(default)]
    pub account_email: Option<String>,
    /// Category names that are collapsed in the channel list.
    #[serde(default)]
    pub collapsed_categories: Vec<String>,
    /// User IDs whose DM threads are closed/archived (hidden from list, messages preserved).
    #[serde(default)]
    pub closed_dm_user_ids: Vec<String>,
    /// Activity status text (e.g. "Playing Valorant"). Persisted across sessions.
    #[serde(default)]
    pub activity: String,
    /// Notification sound style: "default", "subtle", "chime", "none"
    #[serde(default = "default_notification_sound")]
    pub notification_sound: String,
    /// Show OS-level desktop notifications for mentions and DMs when the window is unfocused.
    #[serde(default = "default_true")]
    pub desktop_notifications: bool,
    /// Last read message ID per channel. Used to show "NEW" separator in chat.
    #[serde(default)]
    pub last_read_messages: std::collections::HashMap<String, String>,
    /// Channel IDs that the user has favorited (shown at top of channel list).
    #[serde(default)]
    pub favorite_channels: Vec<String>,
    /// Recently used reaction emojis (most recent first, max 5). Shown as quick-access in the emoji picker.
    #[serde(default)]
    pub recent_reactions: Vec<String>,
    /// Whether the first-run welcome screen has been dismissed. Once true, the welcome card is hidden.
    #[serde(default)]
    pub first_run_completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSpace {
    pub id: String,
    pub name: String,
    pub invite_code: String,
    pub server_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedServer {
    pub name: String,
    pub address: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundboardClipConfig {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub keybind: Option<String>,
}

fn default_user_name() -> String {
    // Use OS username as default — feels more personal than "User"
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "User".into())
}

fn default_feedback_sound() -> bool {
    true
}

fn default_noise_suppression() -> f32 {
    0.5
}

fn default_volume() -> f32 {
    1.0
}

fn default_notifications() -> bool {
    true
}

fn default_minimize_to_tray() -> bool {
    true
}

fn default_server_address() -> String {
    // Default to the production Oracle Cloud server.
    // Users can add additional servers via the saved servers UI.
    "ws://129.158.231.26:9090".into()
}

fn default_theme_preset() -> String {
    "voxlink".into()
}

fn default_true() -> bool {
    true
}

fn default_status_preset() -> String {
    "online".into()
}

fn default_idle_timeout() -> u32 {
    5
}

fn default_ducking_threshold() -> f32 {
    0.05
}

fn default_notification_sound() -> String {
    "default".into()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            push_to_talk_key: None,
            open_mic_sensitivity: 0.5,
            mic_mode: "open_mic".into(),
            user_name: default_user_name(),
            server_address: default_server_address(),
            last_room_code: None,
            window_width: None,
            window_height: None,
            mute_key: None,
            deafen_key: None,
            dark_mode: None,
            theme_preset: default_theme_preset(),
            saved_spaces: Vec::new(),
            last_space_id: None,
            last_channel_id: None,
            feedback_sound: default_feedback_sound(),
            noise_suppression: default_noise_suppression(),
            neural_noise_suppression: false,
            echo_cancellation: false,
            input_volume: default_volume(),
            output_volume: default_volume(),
            notifications_enabled: default_notifications(),
            auth_token: None,
            minimize_to_tray: default_minimize_to_tray(),
            member_widget_visible: false,
            member_widget_x: None,
            member_widget_y: None,
            favorite_friends: Vec::new(),
            recent_direct_messages: Vec::new(),
            peer_volumes: std::collections::HashMap::new(),
            peer_eq_settings: std::collections::HashMap::new(),
            peer_pan: std::collections::HashMap::new(),
            user_notes: std::collections::HashMap::new(),
            saved_servers: Vec::new(),
            join_leave_sounds: default_true(),
            show_spoilers: false,
            compact_chat: false,
            streamer_mode: false,
            blocked_users: Vec::new(),
            status_preset: default_status_preset(),
            idle_timeout_mins: default_idle_timeout(),
            channel_notification_overrides: std::collections::HashMap::new(),
            ducking_amount: 0.0,
            ducking_threshold: default_ducking_threshold(),
            soundboard_clips: Vec::new(),
            account_email: None,
            collapsed_categories: Vec::new(),
            closed_dm_user_ids: Vec::new(),
            activity: String::new(),
            notification_sound: default_notification_sound(),
            desktop_notifications: default_true(),
            last_read_messages: std::collections::HashMap::new(),
            favorite_channels: Vec::new(),
            recent_reactions: Vec::new(),
            first_run_completed: false,
        }
    }
}

impl AppConfig {
    /// Get the effective server address: first default from saved_servers,
    /// then the legacy server_address field, then the hardcoded default.
    pub fn effective_server_address(&self) -> &str {
        self.saved_servers
            .iter()
            .find(|s| s.is_default)
            .map(|s| s.address.as_str())
            .unwrap_or(&self.server_address)
    }
}

fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("com", "voxlink", "Voxlink").map(|dirs| dirs.config_dir().join("config.json"))
}

/// Return the config directory path for display in the privacy dashboard.
pub fn config_dir_display() -> String {
    ProjectDirs::from("com", "voxlink", "Voxlink")
        .map(|dirs| dirs.config_dir().display().to_string())
        .unwrap_or_else(|| "(unknown)".into())
}

/// Reset config to defaults (preserving auth token so we don't log out).
pub fn reset_to_defaults() -> Result<(), String> {
    let existing = load_config();
    let mut fresh = AppConfig::default();
    // Preserve authentication state so the user stays logged in
    fresh.auth_token = existing.auth_token;
    fresh.account_email = existing.account_email;
    save_config(&fresh)
}

/// Serialize the current config to a pretty-printed JSON string (for export).
pub fn export_config_json() -> String {
    let cfg = load_config();
    serde_json::to_string_pretty(&cfg).unwrap_or_else(|_| "{}".into())
}

pub fn load_config() -> AppConfig {
    let Some(path) = config_path() else {
        log::warn!("Could not determine config directory, using defaults");
        return AppConfig::default();
    };

    match fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|e| {
            log::warn!("Failed to parse config: {e}, using defaults");
            AppConfig::default()
        }),
        Err(_) => {
            log::info!("No config file found at {}, using defaults", path.display());
            AppConfig::default()
        }
    }
}

pub fn save_config(config: &AppConfig) -> Result<(), String> {
    let path = config_path().ok_or("Could not determine config directory")?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {e}"))?;
    }

    let json = serde_json::to_string_pretty(config).map_err(|e| format!("Serialize error: {e}"))?;

    // Atomic write: write to .tmp then rename. Prevents corruption if the process
    // crashes mid-write (rename is atomic on POSIX, near-atomic on Windows).
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, &json).map_err(|e| format!("Failed to write temp config: {e}"))?;
    fs::rename(&tmp_path, &path).map_err(|e| format!("Failed to rename config: {e}"))?;

    log::info!("Config saved to {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let config = AppConfig::default();
        assert!(config.input_device.is_none());
        assert!(config.output_device.is_none());
        assert!(config.push_to_talk_key.is_none());
        assert!((config.open_mic_sensitivity - 0.5).abs() < f32::EPSILON);
        assert_eq!(config.mic_mode, "open_mic");
        assert_eq!(config.server_address, "ws://129.158.231.26:9090");
        assert!(config.last_room_code.is_none());
        assert!(config.window_width.is_none());
        assert!(config.window_height.is_none());
        assert!(config.saved_spaces.is_empty());
        assert!(config.last_space_id.is_none());
        assert!(config.last_channel_id.is_none());
    }

    #[test]
    fn serialization_round_trip() {
        let config = AppConfig {
            input_device: Some("Mic1".into()),
            output_device: Some("Speaker1".into()),
            push_to_talk_key: Some("space".into()),
            open_mic_sensitivity: 0.7,
            mic_mode: "push_to_talk".into(),
            user_name: "TestUser".into(),
            server_address: "ws://localhost:9090".into(),
            last_room_code: Some("123456".into()),
            window_width: Some(800),
            window_height: Some(600),
            mute_key: Some("m".into()),
            deafen_key: Some("d".into()),
            dark_mode: Some(true),
            theme_preset: "space".into(),
            saved_spaces: vec![SavedSpace {
                id: "s1".into(),
                name: "Test Space".into(),
                invite_code: "AbCd1234".into(),
                server_address: "ws://localhost:9090".into(),
            }],
            last_space_id: Some("s1".into()),
            last_channel_id: Some("c1".into()),
            feedback_sound: true,
            noise_suppression: 0.6,
            neural_noise_suppression: true,
            echo_cancellation: false,
            input_volume: 1.5,
            output_volume: 0.8,
            notifications_enabled: true,
            auth_token: None,
            minimize_to_tray: true,
            member_widget_visible: true,
            member_widget_x: Some(120),
            member_widget_y: Some(80),
            favorite_friends: vec![shared_types::FavoriteFriend {
                user_id: "u1".into(),
                name: "Alice".into(),
                is_online: false,
                is_in_voice: false,
                in_private_call: false,
                active_space_name: String::new(),
                active_channel_name: String::new(),
                last_space_name: "Studio".into(),
                last_channel_name: "General".into(),
                last_seen_at: 123,
            }],
            recent_direct_messages: vec![shared_types::DirectMessageThread {
                user_id: "u1".into(),
                user_name: "Alice".into(),
                last_message_id: "m1".into(),
                last_message_preview: "Hello".into(),
                last_message_at: 456,
                unread_count: 2,
                is_online: true,
                is_in_voice: false,
            }],
            peer_volumes: std::collections::HashMap::new(),
            peer_eq_settings: std::collections::HashMap::new(),
            peer_pan: std::collections::HashMap::new(),
            user_notes: std::collections::HashMap::new(),
            saved_servers: vec![SavedServer {
                name: "Primary".into(),
                address: "ws://server1:9090".into(),
                is_default: true,
            }],
            join_leave_sounds: true,
            show_spoilers: false,
            compact_chat: false,
            streamer_mode: false,
            blocked_users: Vec::new(),
            status_preset: "online".into(),
            idle_timeout_mins: 5,
            channel_notification_overrides: std::collections::HashMap::new(),
            ducking_amount: 0.0,
            ducking_threshold: 0.1,
            soundboard_clips: Vec::new(),
            account_email: None,
            collapsed_categories: Vec::new(),
            closed_dm_user_ids: Vec::new(),
            activity: String::new(),
            notification_sound: "chime".into(),
            desktop_notifications: true,
            last_read_messages: std::collections::HashMap::new(),
            favorite_channels: Vec::new(),
            recent_reactions: Vec::new(),
            first_run_completed: false,
        };
        let json = serde_json::to_string(&config).unwrap();
        let decoded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.input_device.as_deref(), Some("Mic1"));
        assert_eq!(decoded.output_device.as_deref(), Some("Speaker1"));
        assert_eq!(decoded.push_to_talk_key.as_deref(), Some("space"));
        assert!((decoded.open_mic_sensitivity - 0.7).abs() < f32::EPSILON);
        assert_eq!(decoded.mic_mode, "push_to_talk");
        assert_eq!(decoded.user_name, "TestUser");
        assert_eq!(decoded.server_address, "ws://localhost:9090");
        assert_eq!(decoded.last_room_code.as_deref(), Some("123456"));
        assert_eq!(decoded.window_width, Some(800));
        assert_eq!(decoded.window_height, Some(600));
        assert_eq!(decoded.saved_spaces.len(), 1);
        assert_eq!(decoded.saved_spaces[0].name, "Test Space");
        assert_eq!(decoded.theme_preset, "space");
        assert_eq!(decoded.last_space_id.as_deref(), Some("s1"));
        assert_eq!(decoded.last_channel_id.as_deref(), Some("c1"));
        assert!(decoded.member_widget_visible);
        assert_eq!(decoded.member_widget_x, Some(120));
        assert_eq!(decoded.member_widget_y, Some(80));
        assert_eq!(decoded.favorite_friends.len(), 1);
        assert_eq!(decoded.favorite_friends[0].user_id, "u1");
        assert_eq!(decoded.recent_direct_messages.len(), 1);
        assert_eq!(decoded.recent_direct_messages[0].user_id, "u1");
        assert_eq!(decoded.saved_servers.len(), 1);
        assert_eq!(decoded.saved_servers[0].name, "Primary");
        assert!(decoded.saved_servers[0].is_default);
    }

    #[test]
    fn backward_compat_missing_new_fields() {
        // JSON from an older version without window_width, window_height, last_room_code
        let json = r#"{
            "input_device": null,
            "output_device": null,
            "push_to_talk_key": null,
            "open_mic_sensitivity": 0.5,
            "mic_mode": "open_mic",
            "user_name": "OldUser",
            "server_address": "ws://localhost:9090"
        }"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.user_name, "OldUser");
        assert!(config.last_room_code.is_none());
        assert!(config.window_width.is_none());
        assert!(config.window_height.is_none());
        assert_eq!(config.theme_preset, "voxlink");
        assert!(config.saved_spaces.is_empty());
        assert!(config.last_space_id.is_none());
        assert!(config.last_channel_id.is_none());
        assert!(!config.member_widget_visible);
        assert!(config.member_widget_x.is_none());
        assert!(config.member_widget_y.is_none());
        assert!(config.favorite_friends.is_empty());
        assert!(config.saved_servers.is_empty());
    }

    #[test]
    fn effective_server_address_uses_default_saved_server() {
        let config = AppConfig {
            server_address: "ws://legacy:9090".into(),
            saved_servers: vec![
                SavedServer {
                    name: "Secondary".into(),
                    address: "ws://second:9090".into(),
                    is_default: false,
                },
                SavedServer {
                    name: "Primary".into(),
                    address: "ws://primary:9090".into(),
                    is_default: true,
                },
            ],
            ..AppConfig::default()
        };
        assert_eq!(config.effective_server_address(), "ws://primary:9090");
    }

    #[test]
    fn effective_server_address_falls_back_to_legacy() {
        let config = AppConfig {
            server_address: "ws://legacy:9090".into(),
            saved_servers: vec![
                SavedServer {
                    name: "NonDefault".into(),
                    address: "ws://other:9090".into(),
                    is_default: false,
                },
            ],
            ..AppConfig::default()
        };
        assert_eq!(config.effective_server_address(), "ws://legacy:9090");
    }

    #[test]
    fn effective_server_address_empty_servers_uses_legacy() {
        let config = AppConfig {
            server_address: "ws://legacy:9090".into(),
            saved_servers: Vec::new(),
            ..AppConfig::default()
        };
        assert_eq!(config.effective_server_address(), "ws://legacy:9090");
    }

    #[test]
    fn saved_server_serialization_round_trip() {
        let servers = vec![
            SavedServer {
                name: "Home".into(),
                address: "ws://192.168.1.1:9090".into(),
                is_default: true,
            },
            SavedServer {
                name: "Cloud".into(),
                address: "wss://voxlink.example.com:443".into(),
                is_default: false,
            },
        ];
        let json = serde_json::to_string(&servers).unwrap();
        let decoded: Vec<SavedServer> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].name, "Home");
        assert!(decoded[0].is_default);
        assert_eq!(decoded[1].address, "wss://voxlink.example.com:443");
        assert!(!decoded[1].is_default);
    }

    #[test]
    fn peer_volumes_round_trip() {
        let mut config = AppConfig::default();
        config.peer_volumes.insert("peer1".into(), 0.5);
        config.peer_volumes.insert("peer2".into(), 1.5);
        let json = serde_json::to_string(&config).unwrap();
        let decoded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.peer_volumes.len(), 2);
        assert!((decoded.peer_volumes["peer1"] - 0.5).abs() < f32::EPSILON);
        assert!((decoded.peer_volumes["peer2"] - 1.5).abs() < f32::EPSILON);
    }

    #[test]
    fn user_notes_round_trip() {
        let mut config = AppConfig::default();
        config.user_notes.insert("u123".into(), "Good moderator".into());
        let json = serde_json::to_string(&config).unwrap();
        let decoded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.user_notes["u123"], "Good moderator");
    }

    #[test]
    fn default_config_has_correct_audio_defaults() {
        let config = AppConfig::default();
        assert!((config.input_volume - 1.0).abs() < f32::EPSILON);
        assert!((config.output_volume - 1.0).abs() < f32::EPSILON);
        assert!((config.noise_suppression - 0.5).abs() < f32::EPSILON);
        assert!(!config.neural_noise_suppression);
        assert!(!config.echo_cancellation);
        assert!(config.feedback_sound);
        assert!(config.notifications_enabled);
        assert!(config.minimize_to_tray);
    }

    // ─── v0.8.0 tests ───

    #[test]
    fn v08_fields_default_from_empty_json() {
        // Simulate a config from a pre-v0.8.0 install — only the required fields
        let json = r#"{
            "input_device": null,
            "output_device": null,
            "push_to_talk_key": null,
            "open_mic_sensitivity": 0.5,
            "mic_mode": "open_mic",
            "user_name": "OldUser",
            "server_address": "ws://localhost:9090"
        }"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        // v0.8.0 fields should get their defaults
        assert!(config.join_leave_sounds); // default true
        assert!(!config.show_spoilers); // default false
        assert!(!config.compact_chat); // default false
        assert!(config.blocked_users.is_empty());
        assert_eq!(config.status_preset, "online");
        assert_eq!(config.idle_timeout_mins, 5);
        assert!(config.channel_notification_overrides.is_empty());
        assert!((config.ducking_amount - 0.0).abs() < f32::EPSILON);
        assert!((config.ducking_threshold - 0.05).abs() < f32::EPSILON);
        assert!(config.soundboard_clips.is_empty());
        assert!(config.desktop_notifications); // default true
    }

    #[test]
    fn soundboard_clip_config_serialization() {
        let clip = SoundboardClipConfig {
            name: "Airhorn".into(),
            path: "/sounds/airhorn.wav".into(),
            keybind: Some("F5".into()),
        };
        let json = serde_json::to_string(&clip).unwrap();
        let decoded: SoundboardClipConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, "Airhorn");
        assert_eq!(decoded.path, "/sounds/airhorn.wav");
        assert_eq!(decoded.keybind.as_deref(), Some("F5"));

        // Without keybind
        let clip2 = SoundboardClipConfig {
            name: "Rimshot".into(),
            path: "/sounds/rimshot.wav".into(),
            keybind: None,
        };
        let json2 = serde_json::to_string(&clip2).unwrap();
        let decoded2: SoundboardClipConfig = serde_json::from_str(&json2).unwrap();
        assert_eq!(decoded2.name, "Rimshot");
        assert!(decoded2.keybind.is_none());
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn v08_fields_round_trip() {
        let mut config = AppConfig::default();
        config.join_leave_sounds = false;
        config.show_spoilers = true;
        config.compact_chat = true;
        config.blocked_users = vec!["u1".into(), "u2".into()];
        config.status_preset = "dnd".into();
        config.idle_timeout_mins = 10;
        config
            .channel_notification_overrides
            .insert("c1".into(), "none".into());
        config.ducking_amount = 0.5;
        config.ducking_threshold = 0.08;
        config.soundboard_clips = vec![SoundboardClipConfig {
            name: "Horn".into(),
            path: "/horn.wav".into(),
            keybind: Some("F1".into()),
        }];

        let json = serde_json::to_string(&config).unwrap();
        let decoded: AppConfig = serde_json::from_str(&json).unwrap();

        assert!(!decoded.join_leave_sounds);
        assert!(decoded.show_spoilers);
        assert!(decoded.compact_chat);
        assert_eq!(decoded.blocked_users, vec!["u1", "u2"]);
        assert_eq!(decoded.status_preset, "dnd");
        assert_eq!(decoded.idle_timeout_mins, 10);
        assert_eq!(decoded.channel_notification_overrides["c1"], "none");
        assert!((decoded.ducking_amount - 0.5).abs() < f32::EPSILON);
        assert!((decoded.ducking_threshold - 0.08).abs() < f32::EPSILON);
        assert_eq!(decoded.soundboard_clips.len(), 1);
        assert_eq!(decoded.soundboard_clips[0].name, "Horn");
        assert_eq!(
            decoded.soundboard_clips[0].keybind.as_deref(),
            Some("F1")
        );
    }
}

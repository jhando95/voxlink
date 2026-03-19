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
    /// Private user notes (user_id -> note text). Local only, never sent to server.
    #[serde(default)]
    pub user_notes: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSpace {
    pub id: String,
    pub name: String,
    pub invite_code: String,
    pub server_address: String,
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
    "ws://129.158.231.26:9090".into()
}

fn default_theme_preset() -> String {
    "voxlink".into()
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
            user_notes: std::collections::HashMap::new(),
        }
    }
}

fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("com", "voxlink", "Voxlink").map(|dirs| dirs.config_dir().join("config.json"))
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
            user_notes: std::collections::HashMap::new(),
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
    }
}

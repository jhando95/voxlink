use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::view::*;

pub(crate) fn default_voice_quality() -> u8 {
    2 // High (64kbps) — current default
}

pub(crate) fn default_search_limit() -> u32 {
    50
}

#[derive(Debug, Clone)]
pub struct Participant {
    pub id: String,
    pub name: String,
    pub is_muted: bool,
    pub is_deafened: bool,
    pub is_speaking: bool,
    pub volume: f32,
    /// Audio level for level meter display (0–100 percentage scale)
    pub audio_level: i32,
    /// Per-peer EQ: bass (0.0=−6dB, 0.5=flat, 1.0=+6dB)
    pub eq_bass: f32,
    /// Per-peer EQ: mid
    pub eq_mid: f32,
    /// Per-peer EQ: treble
    pub eq_treble: f32,
    /// Stereo pan (0.0=full left, 0.5=center, 1.0=full right)
    pub pan: f32,
    /// Whether this participant is a priority speaker
    pub is_priority_speaker: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RoomState {
    pub room_code: String,
    pub participants: Vec<Participant>,
    pub is_muted: bool,
    pub is_deafened: bool,
    pub mic_mode: MicMode,
    pub connection: ConnectionState,
    pub active_screen_share_peer_id: Option<String>,
    pub active_screen_share_peer_name: Option<String>,
    pub is_sharing_screen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub invite_code: String,
    pub member_count: u32,
    pub channel_count: u32,
    #[serde(default)]
    pub is_owner: bool,
    #[serde(default)]
    pub self_role: SpaceRole,
    #[serde(default)]
    pub is_public: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    pub peer_count: u32,
    #[serde(default)]
    pub channel_type: ChannelType,
    #[serde(default)]
    pub topic: String,
    /// Voice quality preset: 0=Low(24kbps), 1=Standard(48kbps), 2=High(64kbps), 3=Ultra(96kbps)
    #[serde(default = "default_voice_quality")]
    pub voice_quality: u8,
    /// Max users allowed in this voice channel (0 = unlimited)
    #[serde(default)]
    pub user_limit: u32,
    /// Category/group name for channel organization
    #[serde(default)]
    pub category: String,
    /// Short status text displayed on voice channels (e.g. "Playing Valorant")
    #[serde(default)]
    pub status: String,
    /// Slow mode cooldown in seconds (0 = disabled)
    #[serde(default)]
    pub slow_mode_secs: u32,
    /// Channel display position (lower = higher in list). Default 0 = insertion order.
    #[serde(default)]
    pub position: u32,
    /// Auto-delete messages after N hours (0 = disabled)
    #[serde(default)]
    pub auto_delete_hours: u32,
    /// Minimum role required to access this channel (empty = member)
    #[serde(default)]
    pub min_role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberInfo {
    pub id: String,
    #[serde(default)]
    pub user_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub role: SpaceRole,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub channel_name: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub bio: String,
    #[serde(default)]
    pub nickname: Option<String>,
    #[serde(default)]
    pub status_preset: UserStatus,
    /// Hex color for the member's role (e.g. "#ff5555"), empty for default
    #[serde(default)]
    pub role_color: String,
    /// Activity status text (e.g. "Playing Valorant"), empty if none
    #[serde(default)]
    pub activity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BanInfo {
    pub user_id: String,
    pub user_name: String,
    pub banned_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpaceAuditEntry {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub actor_name: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub target_name: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub timestamp: u64,
}

#[derive(Debug, Clone, Default)]
pub struct SpaceState {
    pub id: String,
    pub name: String,
    pub description: String,
    pub invite_code: String,
    pub channels: Vec<ChannelInfo>,
    pub members: Vec<MemberInfo>,
    pub audit_log: Vec<SpaceAuditEntry>,
    pub active_channel_id: Option<String>,
    pub selected_text_channel_id: Option<String>,
    pub self_role: SpaceRole,
    pub unread_text_channels: HashMap<String, u32>,
    pub typing_users: HashMap<String, Vec<String>>,
    /// Typing timestamps: (channel_id, user_name) → tick when typing started.
    /// Used for client-side 5-second timeout of stale typing indicators.
    pub typing_ticks: HashMap<(String, String), u64>,
}

#[derive(Debug, Clone)]
pub struct PendingMessage {
    pub channel_id: String,
    pub content: String,
    pub is_direct: bool,
    pub retry_count: u8,
    pub queued_at: u64,
}

#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub current_view: AppView,
    pub room: RoomState,
    pub space: Option<SpaceState>,
    pub self_user_id: Option<String>,
    pub favorite_friends: Vec<FavoriteFriend>,
    pub incoming_friend_requests: Vec<FriendRequest>,
    pub outgoing_friend_requests: Vec<FriendRequest>,
    pub active_direct_message_user_id: Option<String>,
    pub direct_typing_users: HashMap<String, Vec<String>>,
    /// DM typing timestamps: user_id → tick when typing started.
    pub direct_typing_ticks: HashMap<String, u64>,
    pub direct_message_threads: Vec<DirectMessageThread>,
    pub pending_messages: Vec<PendingMessage>,
}

#[derive(Debug, Clone, Default)]
pub struct PerfSnapshot {
    pub cpu_percent: f32,
    pub memory_mb: f32,
    pub peak_memory_mb: f32,
    pub uptime_secs: u64,
    pub audio_active: bool,
    pub network_connected: bool,
    pub dropped_frames: u64,
    // Audio metrics (M3)
    pub jitter_buffer_ms: u32,
    pub frame_loss_rate: f32,
    pub encode_bitrate_kbps: u32,
    pub decode_peers: u32,
    // Transport (v0.6)
    pub udp_active: bool,
    pub ping_ms: i32,
    // Screen-share transport health (1s window on the viewer)
    pub screen_frames_completed: u32,
    pub screen_frames_dropped: u32,
    pub screen_frames_timed_out: u32,
    // M8: audio callback health
    pub capture_callback_median_ms: f32,
    pub playback_callback_median_ms: f32,
    pub audio_glitch_count: u32,
    /// RSS growth since `initial_memory_mb` was captured (second snapshot).
    /// Zero until the baseline is established.
    pub memory_growth_mb: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FavoriteFriend {
    pub user_id: String,
    pub name: String,
    #[serde(default)]
    pub is_online: bool,
    #[serde(default)]
    pub is_in_voice: bool,
    #[serde(default)]
    pub in_private_call: bool,
    #[serde(default)]
    pub active_space_name: String,
    #[serde(default)]
    pub active_channel_name: String,
    #[serde(default)]
    pub last_space_name: String,
    #[serde(default)]
    pub last_channel_name: String,
    #[serde(default)]
    pub last_seen_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FriendPresence {
    pub user_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub is_online: bool,
    #[serde(default)]
    pub is_in_voice: bool,
    #[serde(default)]
    pub in_private_call: bool,
    #[serde(default)]
    pub active_space_name: Option<String>,
    #[serde(default)]
    pub active_channel_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FriendRequest {
    pub user_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub requested_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectMessageThread {
    pub user_id: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub last_message_id: String,
    #[serde(default)]
    pub last_message_preview: String,
    #[serde(default)]
    pub last_message_at: u64,
    #[serde(default)]
    pub unread_count: u32,
    #[serde(default)]
    pub is_online: bool,
    #[serde(default)]
    pub is_in_voice: bool,
}

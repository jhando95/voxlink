use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppView {
    #[default]
    Home,
    Room,
    Settings,
    Performance,
    Space,
    TextChat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicMode {
    #[default]
    OpenMic,
    PushToTalk,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug, Clone)]
pub struct Participant {
    pub id: String,
    pub name: String,
    pub is_muted: bool,
    pub is_deafened: bool,
    pub is_speaking: bool,
    pub volume: f32,
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
    pub invite_code: String,
    pub member_count: u32,
    pub channel_count: u32,
    #[serde(default)]
    pub is_owner: bool,
    #[serde(default)]
    pub self_role: SpaceRole,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelType {
    #[default]
    Voice,
    Text,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpaceRole {
    Owner,
    Admin,
    Moderator,
    #[default]
    Member,
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
}

fn default_voice_quality() -> u8 {
    2 // High (64kbps) — current default
}

fn default_search_limit() -> u32 {
    50
}

/// Map voice quality preset index to Opus bitrate in bps
pub fn voice_quality_bitrate(quality: u8) -> i32 {
    match quality {
        0 => 24000,  // Low — poor network, minimal bandwidth
        1 => 48000,  // Standard — good voice clarity
        3 => 96000,  // Ultra — near-transparent voice
        _ => 64000,  // High (default) — excellent quality
    }
}

/// Display label for voice quality preset
pub fn voice_quality_label(quality: u8) -> &'static str {
    match quality {
        0 => "Low",
        1 => "Standard",
        3 => "Ultra",
        _ => "High",
    }
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

#[derive(Debug, Clone, Default)]
pub struct SpaceState {
    pub id: String,
    pub name: String,
    pub invite_code: String,
    pub channels: Vec<ChannelInfo>,
    pub members: Vec<MemberInfo>,
    pub audit_log: Vec<SpaceAuditEntry>,
    pub active_channel_id: Option<String>,
    pub selected_text_channel_id: Option<String>,
    pub self_role: SpaceRole,
    pub unread_text_channels: HashMap<String, u32>,
    pub typing_users: HashMap<String, Vec<String>>,
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
    pub direct_message_threads: Vec<DirectMessageThread>,
    pub pending_messages: Vec<PendingMessage>,
}

#[derive(Debug, Clone, Default)]
pub struct PerfSnapshot {
    pub cpu_percent: f32,
    pub memory_mb: f32,
    pub uptime_secs: u64,
    pub audio_active: bool,
    pub network_connected: bool,
    pub dropped_frames: u64,
}

/// Messages between client and signaling server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SignalMessage {
    // Client -> Server
    CreateRoom {
        user_name: String,
        #[serde(default)]
        password: Option<String>,
    },
    JoinRoom {
        room_code: String,
        user_name: String,
        #[serde(default)]
        password: Option<String>,
    },
    LeaveRoom,
    MuteChanged {
        is_muted: bool,
    },
    DeafenChanged {
        is_deafened: bool,
    },
    StartScreenShare,
    StopScreenShare,

    // Server -> Client
    RoomCreated {
        room_code: String,
    },
    RoomJoined {
        room_code: String,
        participants: Vec<ParticipantInfo>,
    },
    PeerJoined {
        peer: ParticipantInfo,
    },
    PeerLeft {
        peer_id: String,
    },
    PeerMuteChanged {
        peer_id: String,
        is_muted: bool,
    },
    PeerDeafenChanged {
        peer_id: String,
        is_deafened: bool,
    },
    ScreenShareStarted {
        sharer_id: String,
        sharer_name: String,
        is_self: bool,
    },
    ScreenShareStopped {
        sharer_id: String,
    },
    Error {
        message: String,
    },

    // Client -> Server (Space)
    CreateSpace {
        name: String,
        user_name: String,
    },
    JoinSpace {
        invite_code: String,
        user_name: String,
    },
    LeaveSpace,
    DeleteSpace,
    CreateChannel {
        channel_name: String,
        #[serde(default)]
        channel_type: ChannelType,
        #[serde(default = "default_voice_quality")]
        voice_quality: u8,
    },
    DeleteChannel {
        channel_id: String,
    },
    JoinChannel {
        channel_id: String,
    },
    LeaveChannel,
    SelectTextChannel {
        channel_id: String,
    },
    SetTyping {
        channel_id: String,
        is_typing: bool,
    },
    SendTextMessage {
        channel_id: String,
        content: String,
        #[serde(default)]
        reply_to_message_id: Option<String>,
    },
    PinMessage {
        channel_id: String,
        message_id: String,
        pinned: bool,
    },
    WatchFriendPresence {
        user_ids: Vec<String>,
    },
    SendFriendRequest {
        user_id: String,
    },
    RespondFriendRequest {
        user_id: String,
        accept: bool,
    },
    CancelFriendRequest {
        user_id: String,
    },
    RemoveFriend {
        user_id: String,
    },
    SelectDirectMessage {
        user_id: String,
    },
    SetDirectTyping {
        user_id: String,
        is_typing: bool,
    },
    SendDirectMessage {
        user_id: String,
        content: String,
        #[serde(default)]
        reply_to_message_id: Option<String>,
    },
    EditDirectMessage {
        user_id: String,
        message_id: String,
        new_content: String,
    },
    DeleteDirectMessage {
        user_id: String,
        message_id: String,
    },

    // Server -> Client (Space)
    SpaceCreated {
        space: SpaceInfo,
        channels: Vec<ChannelInfo>,
    },
    SpaceJoined {
        space: SpaceInfo,
        channels: Vec<ChannelInfo>,
        members: Vec<MemberInfo>,
    },
    SpaceDeleted,
    ChannelCreated {
        channel: ChannelInfo,
    },
    ChannelDeleted {
        channel_id: String,
    },
    ChannelJoined {
        channel_id: String,
        channel_name: String,
        participants: Vec<ParticipantInfo>,
        #[serde(default = "default_voice_quality")]
        voice_quality: u8,
    },
    ChannelLeft,
    TextChannelSelected {
        channel_id: String,
        channel_name: String,
        history: Vec<TextMessageData>,
    },
    TextMessage {
        channel_id: String,
        message: TextMessageData,
    },
    TypingState {
        channel_id: String,
        user_name: String,
        is_typing: bool,
    },
    MemberOnline {
        member: MemberInfo,
    },
    MemberOffline {
        member_id: String,
    },
    MemberChannelChanged {
        member_id: String,
        channel_id: Option<String>,
        channel_name: Option<String>,
    },

    // Auth (Milestone 4)
    Authenticate {
        token: Option<String>,
        user_name: String,
    },
    Authenticated {
        token: String,
        user_id: String,
    },
    FriendSnapshot {
        friends: Vec<FavoriteFriend>,
        incoming_requests: Vec<FriendRequest>,
        outgoing_requests: Vec<FriendRequest>,
    },
    DirectMessageSelected {
        user_id: String,
        user_name: String,
        history: Vec<TextMessageData>,
    },
    DirectMessage {
        user_id: String,
        message: TextMessageData,
    },
    DirectTypingState {
        user_id: String,
        user_name: String,
        is_typing: bool,
    },
    DirectMessageEdited {
        user_id: String,
        message_id: String,
        new_content: String,
    },
    DirectMessageDeleted {
        user_id: String,
        message_id: String,
    },
    FriendPresenceSnapshot {
        presences: Vec<FriendPresence>,
    },
    FriendPresenceChanged {
        presence: FriendPresence,
    },

    // Chat improvements (Milestone 5)
    EditTextMessage {
        channel_id: String,
        message_id: String,
        new_content: String,
    },
    DeleteTextMessage {
        channel_id: String,
        message_id: String,
    },
    ReactToMessage {
        channel_id: String,
        message_id: String,
        emoji: String,
    },
    TextMessageEdited {
        channel_id: String,
        message_id: String,
        new_content: String,
    },
    TextMessageDeleted {
        channel_id: String,
        message_id: String,
    },
    MessageReaction {
        channel_id: String,
        message_id: String,
        emoji: String,
        user_name: String,
    },
    MessagePinned {
        channel_id: String,
        message_id: String,
        pinned: bool,
    },

    // User status & Channel topics
    SetUserStatus {
        status: String,
    },
    UserStatusChanged {
        member_id: String,
        status: String,
    },
    SetChannelTopic {
        channel_id: String,
        topic: String,
    },
    ChannelTopicChanged {
        channel_id: String,
        topic: String,
    },

    // Moderation (Milestone 6)
    KickMember {
        member_id: String,
    },
    MuteMember {
        member_id: String,
        muted: bool,
    },
    BanMember {
        member_id: String,
    },
    SetMemberRole {
        user_id: String,
        role: SpaceRole,
    },
    Kicked {
        reason: String,
    },
    MemberMuted {
        member_id: String,
        muted: bool,
    },
    MemberRoleChanged {
        user_id: String,
        role: SpaceRole,
    },
    SpaceAuditLogSnapshot {
        entries: Vec<SpaceAuditEntry>,
    },
    SpaceAuditLogAppended {
        entry: SpaceAuditEntry,
    },

    /// Server is shutting down gracefully
    ServerShutdown,

    // M10: Message Search
    SearchMessages {
        channel_id: String,
        query: String,
        #[serde(default = "default_search_limit")]
        limit: u32,
    },
    SearchResults {
        channel_id: String,
        messages: Vec<TextMessageData>,
    },

    // M11: User Profiles
    SetProfile {
        bio: String,
    },
    ProfileUpdated {
        user_id: String,
        bio: String,
    },
}

/// Maximum audio frame size in bytes (Opus at 24kbps, 20ms = ~60 bytes typical, 256 max)
pub const MAX_AUDIO_FRAME_SIZE: usize = 4096;
pub const MAX_SCREEN_FRAME_SIZE: usize = 512 * 1024;
pub const MEDIA_PACKET_AUDIO: u8 = 1;
pub const MEDIA_PACKET_SCREEN: u8 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticipantInfo {
    pub id: String,
    pub name: String,
    pub is_muted: bool,
    #[serde(default)]
    pub is_deafened: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextMessageData {
    pub sender_id: String,
    pub sender_name: String,
    pub content: String,
    pub timestamp: u64, // unix seconds
    #[serde(default)]
    pub message_id: String,
    #[serde(default)]
    pub edited: bool,
    #[serde(default)]
    pub reactions: Vec<ReactionData>,
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
    #[serde(default)]
    pub reply_to_sender_name: Option<String>,
    #[serde(default)]
    pub reply_preview: Option<String>,
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionData {
    pub emoji: String,
    pub users: Vec<String>,
}

pub const SAMPLE_RATE: u32 = 48000;
pub const CHANNELS: u16 = 1;
pub const FRAME_SIZE: usize = 960; // 20ms at 48kHz

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_message_round_trip_create_room() {
        let msg = SignalMessage::CreateRoom {
            user_name: "Alice".into(),
            password: Some("secret".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::CreateRoom {
                user_name,
                password,
            } => {
                assert_eq!(user_name, "Alice");
                assert_eq!(password.as_deref(), Some("secret"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_join_room() {
        let msg = SignalMessage::JoinRoom {
            room_code: "123456".into(),
            user_name: "Bob".into(),
            password: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::JoinRoom {
                room_code,
                user_name,
                password,
            } => {
                assert_eq!(room_code, "123456");
                assert_eq!(user_name, "Bob");
                assert!(password.is_none());
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_leave_room() {
        let msg = SignalMessage::LeaveRoom;
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, SignalMessage::LeaveRoom));
    }

    #[test]
    fn signal_message_round_trip_mute_changed() {
        let msg = SignalMessage::MuteChanged { is_muted: true };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::MuteChanged { is_muted } => assert!(is_muted),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_deafen_changed() {
        let msg = SignalMessage::DeafenChanged { is_deafened: true };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::DeafenChanged { is_deafened } => assert!(is_deafened),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_room_created() {
        let msg = SignalMessage::RoomCreated {
            room_code: "654321".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::RoomCreated { room_code } => assert_eq!(room_code, "654321"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_room_joined() {
        let msg = SignalMessage::RoomJoined {
            room_code: "111111".into(),
            participants: vec![ParticipantInfo {
                id: "p1".into(),
                name: "Alice".into(),
                is_muted: false,
                is_deafened: true,
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::RoomJoined {
                room_code,
                participants,
            } => {
                assert_eq!(room_code, "111111");
                assert_eq!(participants.len(), 1);
                assert_eq!(participants[0].name, "Alice");
                assert!(participants[0].is_deafened);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_peer_joined() {
        let msg = SignalMessage::PeerJoined {
            peer: ParticipantInfo {
                id: "p2".into(),
                name: "Bob".into(),
                is_muted: true,
                is_deafened: false,
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::PeerJoined { peer } => {
                assert_eq!(peer.id, "p2");
                assert!(peer.is_muted);
                assert!(!peer.is_deafened);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_peer_left() {
        let msg = SignalMessage::PeerLeft {
            peer_id: "p3".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::PeerLeft { peer_id } => assert_eq!(peer_id, "p3"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_peer_mute_changed() {
        let msg = SignalMessage::PeerMuteChanged {
            peer_id: "p4".into(),
            is_muted: false,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::PeerMuteChanged { peer_id, is_muted } => {
                assert_eq!(peer_id, "p4");
                assert!(!is_muted);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_peer_deafen_changed() {
        let msg = SignalMessage::PeerDeafenChanged {
            peer_id: "p5".into(),
            is_deafened: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::PeerDeafenChanged {
                peer_id,
                is_deafened,
            } => {
                assert_eq!(peer_id, "p5");
                assert!(is_deafened);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_error() {
        let msg = SignalMessage::Error {
            message: "something went wrong".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::Error { message } => assert_eq!(message, "something went wrong"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn participant_info_backward_compat_missing_is_deafened() {
        // Old JSON without is_deafened field should default to false
        let json = r#"{"id":"p1","name":"Alice","is_muted":false}"#;
        let info: ParticipantInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.id, "p1");
        assert_eq!(info.name, "Alice");
        assert!(!info.is_muted);
        assert!(!info.is_deafened); // defaults to false
    }

    #[test]
    fn signal_message_round_trip_create_space() {
        let msg = SignalMessage::CreateSpace {
            name: "My Space".into(),
            user_name: "Alice".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::CreateSpace { name, user_name } => {
                assert_eq!(name, "My Space");
                assert_eq!(user_name, "Alice");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_join_space() {
        let msg = SignalMessage::JoinSpace {
            invite_code: "AbCd1234".into(),
            user_name: "Bob".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::JoinSpace {
                invite_code,
                user_name,
            } => {
                assert_eq!(invite_code, "AbCd1234");
                assert_eq!(user_name, "Bob");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_delete_channel() {
        let msg = SignalMessage::DeleteChannel {
            channel_id: "c42".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::DeleteChannel { channel_id } => assert_eq!(channel_id, "c42"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_channel_deleted() {
        let msg = SignalMessage::ChannelDeleted {
            channel_id: "c42".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::ChannelDeleted { channel_id } => assert_eq!(channel_id, "c42"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_space_created() {
        let msg = SignalMessage::SpaceCreated {
            space: SpaceInfo {
                id: "s1".into(),
                name: "Test Space".into(),
                invite_code: "XyZ12345".into(),
                member_count: 1,
                channel_count: 1,
                is_owner: true,
                self_role: SpaceRole::Owner,
            },
            channels: vec![ChannelInfo {
                id: "c1".into(),
                name: "General".into(),
                peer_count: 0,
                channel_type: ChannelType::Voice,
                topic: String::new(),
                voice_quality: 2,
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SpaceCreated { space, channels } => {
                assert_eq!(space.id, "s1");
                assert_eq!(space.name, "Test Space");
                assert_eq!(space.invite_code, "XyZ12345");
                assert_eq!(channels.len(), 1);
                assert_eq!(channels[0].name, "General");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_space_joined() {
        let msg = SignalMessage::SpaceJoined {
            space: SpaceInfo {
                id: "s2".into(),
                name: "Fun Space".into(),
                invite_code: "Abc12345".into(),
                member_count: 2,
                channel_count: 1,
                is_owner: false,
                self_role: SpaceRole::Member,
            },
            channels: vec![ChannelInfo {
                id: "c1".into(),
                name: "General".into(),
                peer_count: 1,
                channel_type: ChannelType::Voice,
                topic: String::new(),
                voice_quality: 2,
            }],
            members: vec![MemberInfo {
                id: "p1".into(),
                user_id: Some("u1".into()),
                name: "Alice".into(),
                role: SpaceRole::Member,
                channel_id: Some("c1".into()),
                channel_name: Some("General".into()),
                status: String::new(),
                bio: String::new(),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SpaceJoined {
                space,
                channels,
                members,
            } => {
                assert_eq!(space.id, "s2");
                assert_eq!(space.member_count, 2);
                assert_eq!(channels.len(), 1);
                assert_eq!(members.len(), 1);
                assert_eq!(members[0].channel_id.as_deref(), Some("c1"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_channel_joined() {
        let msg = SignalMessage::ChannelJoined {
            channel_id: "c1".into(),
            channel_name: "General".into(),
            participants: vec![ParticipantInfo {
                id: "p1".into(),
                name: "Alice".into(),
                is_muted: false,
                is_deafened: false,
            }],
            voice_quality: 2,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::ChannelJoined {
                channel_id,
                channel_name,
                participants,
                voice_quality: _,
            } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(channel_name, "General");
                assert_eq!(participants.len(), 1);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_watch_friend_presence() {
        let msg = SignalMessage::WatchFriendPresence {
            user_ids: vec!["u1".into(), "u2".into()],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::WatchFriendPresence { user_ids } => {
                assert_eq!(user_ids, vec!["u1", "u2"]);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_send_friend_request() {
        let msg = SignalMessage::SendFriendRequest {
            user_id: "u2".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SendFriendRequest { user_id } => assert_eq!(user_id, "u2"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_friend_snapshot() {
        let msg = SignalMessage::FriendSnapshot {
            friends: vec![FavoriteFriend {
                user_id: "u1".into(),
                name: "Alice".into(),
                is_online: true,
                is_in_voice: false,
                in_private_call: false,
                active_space_name: "Studio".into(),
                active_channel_name: String::new(),
                last_space_name: "Studio".into(),
                last_channel_name: "General".into(),
                last_seen_at: 42,
            }],
            incoming_requests: vec![FriendRequest {
                user_id: "u3".into(),
                name: "Charlie".into(),
                requested_at: 99,
            }],
            outgoing_requests: vec![FriendRequest {
                user_id: "u4".into(),
                name: "Dana".into(),
                requested_at: 123,
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::FriendSnapshot {
                friends,
                incoming_requests,
                outgoing_requests,
            } => {
                assert_eq!(friends.len(), 1);
                assert_eq!(friends[0].user_id, "u1");
                assert_eq!(incoming_requests.len(), 1);
                assert_eq!(incoming_requests[0].user_id, "u3");
                assert_eq!(outgoing_requests.len(), 1);
                assert_eq!(outgoing_requests[0].user_id, "u4");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_send_direct_message() {
        let msg = SignalMessage::SendDirectMessage {
            user_id: "u2".into(),
            content: "hey".into(),
            reply_to_message_id: Some("m1".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SendDirectMessage {
                user_id,
                content,
                reply_to_message_id,
            } => {
                assert_eq!(user_id, "u2");
                assert_eq!(content, "hey");
                assert_eq!(reply_to_message_id.as_deref(), Some("m1"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_direct_message_selected() {
        let msg = SignalMessage::DirectMessageSelected {
            user_id: "u2".into(),
            user_name: "Bob".into(),
            history: vec![TextMessageData {
                sender_id: "u2".into(),
                sender_name: "Bob".into(),
                content: "hello".into(),
                timestamp: 42,
                message_id: "m1".into(),
                edited: false,
                reactions: Vec::new(),
                reply_to_message_id: None,
                reply_to_sender_name: None,
                reply_preview: None,
                pinned: false,
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::DirectMessageSelected {
                user_id,
                user_name,
                history,
            } => {
                assert_eq!(user_id, "u2");
                assert_eq!(user_name, "Bob");
                assert_eq!(history.len(), 1);
                assert_eq!(history[0].content, "hello");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_friend_presence_snapshot() {
        let msg = SignalMessage::FriendPresenceSnapshot {
            presences: vec![FriendPresence {
                user_id: "u1".into(),
                name: "Alice".into(),
                is_online: true,
                is_in_voice: true,
                in_private_call: false,
                active_space_name: Some("Studio".into()),
                active_channel_name: Some("General".into()),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::FriendPresenceSnapshot { presences } => {
                assert_eq!(presences.len(), 1);
                assert_eq!(presences[0].user_id, "u1");
                assert!(presences[0].is_online);
                assert_eq!(presences[0].active_space_name.as_deref(), Some("Studio"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_member_channel_changed() {
        let msg = SignalMessage::MemberChannelChanged {
            member_id: "p1".into(),
            channel_id: Some("c2".into()),
            channel_name: Some("Gaming".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::MemberChannelChanged {
                member_id,
                channel_id,
                channel_name,
            } => {
                assert_eq!(member_id, "p1");
                assert_eq!(channel_id.as_deref(), Some("c2"));
                assert_eq!(channel_name.as_deref(), Some("Gaming"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn constants_are_correct() {
        assert_eq!(SAMPLE_RATE, 48000);
        assert_eq!(CHANNELS, 1);
        assert_eq!(FRAME_SIZE, 960); // 20ms at 48kHz
        assert_eq!(MAX_AUDIO_FRAME_SIZE, 4096);
    }
}

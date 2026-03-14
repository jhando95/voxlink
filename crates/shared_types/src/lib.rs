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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceInfo {
    pub id: String,
    pub name: String,
    pub invite_code: String,
    pub member_count: u32,
    pub channel_count: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelType {
    #[default]
    Voice,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    pub peer_count: u32,
    #[serde(default)]
    pub channel_type: ChannelType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub channel_name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SpaceState {
    pub id: String,
    pub name: String,
    pub invite_code: String,
    pub channels: Vec<ChannelInfo>,
    pub members: Vec<MemberInfo>,
    pub active_channel_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub current_view: AppView,
    pub room: RoomState,
    pub space: Option<SpaceState>,
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
    MuteChanged { is_muted: bool },
    DeafenChanged { is_deafened: bool },

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
    Error {
        message: String,
    },

    // Client -> Server (Space)
    CreateSpace { name: String, user_name: String },
    JoinSpace { invite_code: String, user_name: String },
    LeaveSpace,
    DeleteSpace,
    CreateChannel { channel_name: String, #[serde(default)] channel_type: ChannelType },
    JoinChannel { channel_id: String },
    LeaveChannel,
    SelectTextChannel { channel_id: String },
    SendTextMessage { channel_id: String, content: String },

    // Server -> Client (Space)
    SpaceCreated { space: SpaceInfo, channels: Vec<ChannelInfo> },
    SpaceJoined { space: SpaceInfo, channels: Vec<ChannelInfo>, members: Vec<MemberInfo> },
    SpaceDeleted,
    ChannelCreated { channel: ChannelInfo },
    ChannelJoined { channel_id: String, channel_name: String, participants: Vec<ParticipantInfo> },
    ChannelLeft,
    TextChannelSelected { channel_id: String, channel_name: String, history: Vec<TextMessageData> },
    TextMessage { channel_id: String, message: TextMessageData },
    MemberOnline { member: MemberInfo },
    MemberOffline { member_id: String },
    MemberChannelChanged { member_id: String, channel_id: Option<String>, channel_name: Option<String> },

    // Auth (Milestone 4)
    Authenticate { token: Option<String>, user_name: String },
    Authenticated { token: String, user_id: String },

    // Chat improvements (Milestone 5)
    EditTextMessage { channel_id: String, message_id: String, new_content: String },
    DeleteTextMessage { channel_id: String, message_id: String },
    ReactToMessage { channel_id: String, message_id: String, emoji: String },
    TextMessageEdited { channel_id: String, message_id: String, new_content: String },
    TextMessageDeleted { channel_id: String, message_id: String },
    MessageReaction { channel_id: String, message_id: String, emoji: String, user_name: String },

    // Moderation (Milestone 6)
    KickMember { member_id: String },
    MuteMember { member_id: String, muted: bool },
    BanMember { member_id: String },
    Kicked { reason: String },
    MemberMuted { member_id: String, muted: bool },
}

/// Maximum audio frame size in bytes (Opus at 24kbps, 20ms = ~60 bytes typical, 256 max)
pub const MAX_AUDIO_FRAME_SIZE: usize = 4096;

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
            SignalMessage::CreateRoom { user_name, password } => {
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
            SignalMessage::JoinRoom { room_code, user_name, password } => {
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
        let msg = SignalMessage::RoomCreated { room_code: "654321".into() };
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
            participants: vec![
                ParticipantInfo { id: "p1".into(), name: "Alice".into(), is_muted: false, is_deafened: true },
            ],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::RoomJoined { room_code, participants } => {
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
            peer: ParticipantInfo { id: "p2".into(), name: "Bob".into(), is_muted: true, is_deafened: false },
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
        let msg = SignalMessage::PeerLeft { peer_id: "p3".into() };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::PeerLeft { peer_id } => assert_eq!(peer_id, "p3"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_peer_mute_changed() {
        let msg = SignalMessage::PeerMuteChanged { peer_id: "p4".into(), is_muted: false };
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
        let msg = SignalMessage::PeerDeafenChanged { peer_id: "p5".into(), is_deafened: true };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::PeerDeafenChanged { peer_id, is_deafened } => {
                assert_eq!(peer_id, "p5");
                assert!(is_deafened);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_error() {
        let msg = SignalMessage::Error { message: "something went wrong".into() };
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
            SignalMessage::JoinSpace { invite_code, user_name } => {
                assert_eq!(invite_code, "AbCd1234");
                assert_eq!(user_name, "Bob");
            }
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
            },
            channels: vec![ChannelInfo { id: "c1".into(), name: "General".into(), peer_count: 0, channel_type: ChannelType::Voice }],
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
            },
            channels: vec![ChannelInfo { id: "c1".into(), name: "General".into(), peer_count: 1, channel_type: ChannelType::Voice }],
            members: vec![MemberInfo {
                id: "p1".into(),
                name: "Alice".into(),
                channel_id: Some("c1".into()),
                channel_name: Some("General".into()),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SpaceJoined { space, channels, members } => {
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
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::ChannelJoined { channel_id, channel_name, participants } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(channel_name, "General");
                assert_eq!(participants.len(), 1);
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
            SignalMessage::MemberChannelChanged { member_id, channel_id, channel_name } => {
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

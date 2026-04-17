pub mod view;
pub use view::*;

pub mod state;
pub use state::*;

pub mod message_data;
pub use message_data::*;

pub mod protocol;
pub use protocol::*;


/// Maximum audio frame size in bytes (Opus at 24kbps, 20ms = ~60 bytes typical, 256 max)
pub const MAX_AUDIO_FRAME_SIZE: usize = 4096;
pub const MAX_SCREEN_FRAME_SIZE: usize = 512 * 1024;
/// Safe media payload budget for a single UDP datagram.
/// Kept below the protocol maximum to leave room for token and sender headers.
pub const MAX_UDP_MEDIA_PAYLOAD_SIZE: usize = 60 * 1024;
/// Per-chunk metadata for oversized screen-share frames:
/// sequence(u32) + chunk_index(u16) + chunk_count(u16).
pub const SCREEN_CHUNK_METADATA_LEN: usize = 8;
/// Chunked screen-share datagrams intentionally stay well below the protocol
/// ceiling so they avoid `EMSGSIZE` and reduce fragmentation pressure.
pub const MAX_UDP_SCREEN_CHUNK_SIZE: usize = 4 * 1024;
pub const MEDIA_PACKET_AUDIO: u8 = 1;
pub const MEDIA_PACKET_SCREEN: u8 = 2;
pub const MEDIA_PACKET_SCREEN_CHUNK: u8 = 3;

/// UDP session token length in bytes (random, assigned by server on RequestUdp).
pub const UDP_SESSION_TOKEN_LEN: usize = 8;
/// Default UDP relay port (same as WebSocket port + 1).
pub const UDP_DEFAULT_PORT_OFFSET: u16 = 1;
/// UDP keepalive packet type — sent every 15s to keep NAT mappings alive.
pub const UDP_KEEPALIVE: u8 = 0xFE;
/// Interval between UDP keepalive packets.
pub const UDP_KEEPALIVE_INTERVAL_SECS: u64 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenChunkMetadata {
    pub sequence: u32,
    pub chunk_index: u16,
    pub chunk_count: u16,
}

pub fn encode_screen_chunk_metadata(
    sequence: u32,
    chunk_index: u16,
    chunk_count: u16,
) -> [u8; SCREEN_CHUNK_METADATA_LEN] {
    let mut out = [0u8; SCREEN_CHUNK_METADATA_LEN];
    out[..4].copy_from_slice(&sequence.to_be_bytes());
    out[4..6].copy_from_slice(&chunk_index.to_be_bytes());
    out[6..8].copy_from_slice(&chunk_count.to_be_bytes());
    out
}

pub fn decode_screen_chunk_metadata(raw: &[u8]) -> Option<(ScreenChunkMetadata, &[u8])> {
    if raw.len() < SCREEN_CHUNK_METADATA_LEN {
        return None;
    }
    let sequence = u32::from_be_bytes(raw[..4].try_into().ok()?);
    let chunk_index = u16::from_be_bytes(raw[4..6].try_into().ok()?);
    let chunk_count = u16::from_be_bytes(raw[6..8].try_into().ok()?);
    if chunk_count == 0 || chunk_index >= chunk_count {
        return None;
    }
    Some((
        ScreenChunkMetadata {
            sequence,
            chunk_index,
            chunk_count,
        },
        &raw[SCREEN_CHUNK_METADATA_LEN..],
    ))
}

pub const SAMPLE_RATE: u32 = 48000;
pub const CHANNELS: u16 = 1;
pub const FRAME_SIZE: usize = 960; // 20ms at 48kHz

/// Extract the first URL (http:// or https://) from message content.
pub fn extract_first_url(content: &str) -> Option<String> {
    for word in content.split_whitespace() {
        if word.starts_with("http://") || word.starts_with("https://") {
            // Strip trailing punctuation that's likely not part of the URL
            let trimmed = word.trim_end_matches([',', '.', ')', ']', '>', ';']);
            return Some(trimmed.to_string());
        }
    }
    None
}

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
                is_priority_speaker: false,
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
                is_priority_speaker: false,
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
                description: String::new(),
                invite_code: "XyZ12345".into(),
                member_count: 1,
                channel_count: 1,
                is_owner: true,
                self_role: SpaceRole::Owner,
                is_public: false,
            },
            channels: vec![ChannelInfo {
                id: "c1".into(),
                name: "General".into(),
                peer_count: 0,
                channel_type: ChannelType::Voice,
                topic: String::new(),
                voice_quality: 2,
                user_limit: 0,
                category: String::new(),
                status: String::new(),
                slow_mode_secs: 0,
                position: 0,
                auto_delete_hours: 0,
                min_role: String::new(),
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
                description: String::new(),
                invite_code: "Abc12345".into(),
                member_count: 2,
                channel_count: 1,
                is_owner: false,
                self_role: SpaceRole::Member,
                is_public: false,
            },
            channels: vec![ChannelInfo {
                id: "c1".into(),
                name: "General".into(),
                peer_count: 1,
                channel_type: ChannelType::Voice,
                topic: String::new(),
                voice_quality: 2,
                user_limit: 0,
                category: String::new(),
                status: String::new(),
                slow_mode_secs: 0,
                position: 0,
                auto_delete_hours: 0,
                min_role: String::new(),
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
                nickname: None,
                status_preset: UserStatus::Online,
                role_color: String::new(),
                activity: String::new(),
            }],
            welcome_message: Some("Welcome to the space!".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SpaceJoined {
                space,
                channels,
                members,
                welcome_message,
            } => {
                assert_eq!(space.id, "s2");
                assert_eq!(space.member_count, 2);
                assert_eq!(channels.len(), 1);
                assert_eq!(members.len(), 1);
                assert_eq!(members[0].channel_id.as_deref(), Some("c1"));
                assert_eq!(welcome_message.as_deref(), Some("Welcome to the space!"));
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
                is_priority_speaker: false,
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
                forwarded_from: None,
                attachment_name: None,
                attachment_size: None,
                link_url: None,
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
        assert_eq!(MAX_UDP_MEDIA_PAYLOAD_SIZE, 60 * 1024);
        assert_eq!(MAX_UDP_SCREEN_CHUNK_SIZE, 4 * 1024);
        assert_eq!(UDP_SESSION_TOKEN_LEN, 8);
        assert_eq!(UDP_DEFAULT_PORT_OFFSET, 1);
    }

    #[test]
    fn screen_chunk_metadata_round_trip() {
        let encoded = encode_screen_chunk_metadata(42, 2, 7);
        let packet = [encoded.as_slice(), b"tail"].concat();
        let (decoded, payload) = decode_screen_chunk_metadata(&packet).unwrap();
        assert_eq!(
            decoded,
            ScreenChunkMetadata {
                sequence: 42,
                chunk_index: 2,
                chunk_count: 7,
            }
        );
        assert_eq!(payload, b"tail");
    }

    #[test]
    fn screen_chunk_metadata_rejects_invalid_ranges() {
        let encoded = encode_screen_chunk_metadata(7, 4, 4);
        assert!(decode_screen_chunk_metadata(&encoded).is_none());

        let zero_count = encode_screen_chunk_metadata(7, 0, 0);
        assert!(decode_screen_chunk_metadata(&zero_count).is_none());
    }

    #[test]
    fn signal_message_round_trip_request_udp() {
        let msg = SignalMessage::RequestUdp;
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, SignalMessage::RequestUdp));
    }

    #[test]
    fn signal_message_round_trip_udp_ready() {
        let msg = SignalMessage::UdpReady {
            token: "0123456789abcdef".into(),
            port: 9091,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::UdpReady { token, port } => {
                assert_eq!(token, "0123456789abcdef");
                assert_eq!(port, 9091);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_screen_share_transport_feedback() {
        let msg = SignalMessage::ScreenShareTransportFeedback {
            frames_completed: 12,
            frames_dropped: 3,
            frames_timed_out: 1,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::ScreenShareTransportFeedback {
                frames_completed,
                frames_dropped,
                frames_timed_out,
            } => {
                assert_eq!(frames_completed, 12);
                assert_eq!(frames_dropped, 3);
                assert_eq!(frames_timed_out, 1);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_udp_unavailable() {
        let msg = SignalMessage::UdpUnavailable;
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, SignalMessage::UdpUnavailable));
    }

    #[test]
    fn signal_message_round_trip_channel_user_limit() {
        let msg = SignalMessage::SetChannelUserLimit {
            channel_id: "c1".into(),
            user_limit: 5,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetChannelUserLimit {
                channel_id,
                user_limit,
            } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(user_limit, 5);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_priority_speaker() {
        let msg = SignalMessage::SetPrioritySpeaker {
            peer_id: "p1".into(),
            enabled: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetPrioritySpeaker { peer_id, enabled } => {
                assert_eq!(peer_id, "p1");
                assert!(enabled);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_whisper() {
        let msg = SignalMessage::WhisperTo {
            target_peer_ids: vec!["p1".into(), "p2".into()],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::WhisperTo { target_peer_ids } => {
                assert_eq!(target_peer_ids, vec!["p1", "p2"]);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_timeout_member() {
        let msg = SignalMessage::TimeoutMember {
            member_id: "p1".into(),
            duration_secs: 300,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::TimeoutMember {
                member_id,
                duration_secs,
            } => {
                assert_eq!(member_id, "p1");
                assert_eq!(duration_secs, 300);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_slow_mode() {
        let msg = SignalMessage::SetChannelSlowMode {
            channel_id: "c1".into(),
            slow_mode_secs: 10,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetChannelSlowMode {
                channel_id,
                slow_mode_secs,
            } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(slow_mode_secs, 10);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_channel_status() {
        let msg = SignalMessage::SetChannelStatus {
            channel_id: "c1".into(),
            status: "Playing Valorant".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetChannelStatus { channel_id, status } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(status, "Playing Valorant");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn channel_info_new_fields_backward_compat() {
        // Old JSON without new fields should default correctly
        let json = r#"{"id":"c1","name":"General","peer_count":0}"#;
        let info: ChannelInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.user_limit, 0);
        assert_eq!(info.category, "");
        assert_eq!(info.status, "");
        assert_eq!(info.slow_mode_secs, 0);
    }

    #[test]
    fn participant_info_priority_speaker_backward_compat() {
        let json = r#"{"id":"p1","name":"Alice","is_muted":false}"#;
        let info: ParticipantInfo = serde_json::from_str(json).unwrap();
        assert!(!info.is_priority_speaker);
    }

    // ─── v0.8.0 tests ───

    #[test]
    fn user_status_serialization_round_trip() {
        for (variant, expected_str) in [
            (UserStatus::Online, "\"Online\""),
            (UserStatus::Idle, "\"Idle\""),
            (UserStatus::DoNotDisturb, "\"DoNotDisturb\""),
            (UserStatus::Invisible, "\"Invisible\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected_str);
            let decoded: UserStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, variant);
        }
    }

    #[test]
    fn ban_info_serialization() {
        let ban = BanInfo {
            user_id: "u42".into(),
            user_name: "Troll".into(),
            banned_at: 1700000000,
        };
        let json = serde_json::to_string(&ban).unwrap();
        let decoded: BanInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.user_id, "u42");
        assert_eq!(decoded.user_name, "Troll");
        assert_eq!(decoded.banned_at, 1700000000);
    }

    #[test]
    fn text_message_data_with_forwarded_from() {
        let msg = TextMessageData {
            sender_id: "u1".into(),
            sender_name: "Alice".into(),
            content: "forwarded content".into(),
            timestamp: 1000,
            message_id: "m99".into(),
            edited: false,
            reactions: Vec::new(),
            reply_to_message_id: None,
            reply_to_sender_name: None,
            reply_preview: None,
            pinned: false,
            forwarded_from: Some("general".into()),
            attachment_name: None,
            attachment_size: None,
            link_url: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: TextMessageData = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.forwarded_from.as_deref(), Some("general"));

        // Backward compat: missing forwarded_from defaults to None
        let old_json = r#"{"sender_id":"u1","sender_name":"A","content":"hi","timestamp":1}"#;
        let old: TextMessageData = serde_json::from_str(old_json).unwrap();
        assert!(old.forwarded_from.is_none());
    }

    #[test]
    fn member_info_with_nickname_and_status_preset() {
        let member = MemberInfo {
            id: "p1".into(),
            user_id: Some("u1".into()),
            name: "Alice".into(),
            role: SpaceRole::Admin,
            channel_id: None,
            channel_name: None,
            status: "custom status".into(),
            bio: "hello world".into(),
            nickname: Some("Ally".into()),
            status_preset: UserStatus::DoNotDisturb,
            role_color: String::new(),
            activity: String::new(),
        };
        let json = serde_json::to_string(&member).unwrap();
        let decoded: MemberInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.nickname.as_deref(), Some("Ally"));
        assert_eq!(decoded.status_preset, UserStatus::DoNotDisturb);

        // Backward compat: missing nickname/status_preset use defaults
        let old_json = r#"{"id":"p1","name":"Bob"}"#;
        let old: MemberInfo = serde_json::from_str(old_json).unwrap();
        assert!(old.nickname.is_none());
        assert_eq!(old.status_preset, UserStatus::Online);
    }

    #[test]
    fn signal_message_round_trip_set_status_preset() {
        let msg = SignalMessage::SetStatusPreset {
            preset: UserStatus::Idle,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetStatusPreset { preset } => assert_eq!(preset, UserStatus::Idle),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_status_preset_changed() {
        let msg = SignalMessage::StatusPresetChanged {
            member_id: "p1".into(),
            preset: UserStatus::DoNotDisturb,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::StatusPresetChanged { member_id, preset } => {
                assert_eq!(member_id, "p1");
                assert_eq!(preset, UserStatus::DoNotDisturb);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_mention_notification() {
        let msg = SignalMessage::MentionNotification {
            channel_id: "c1".into(),
            channel_name: "General".into(),
            sender_name: "Bob".into(),
            preview: "@Alice check this".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::MentionNotification {
                channel_id,
                channel_name,
                sender_name,
                preview,
            } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(channel_name, "General");
                assert_eq!(sender_name, "Bob");
                assert_eq!(preview, "@Alice check this");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_block_unblock() {
        let msg = SignalMessage::BlockUser {
            user_id: "u5".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::BlockUser { user_id } => assert_eq!(user_id, "u5"),
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::UnblockUser {
            user_id: "u5".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::UnblockUser { user_id } => assert_eq!(user_id, "u5"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_user_blocked_unblocked() {
        let msg = SignalMessage::UserBlocked {
            user_id: "u5".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(matches!(
            serde_json::from_str::<SignalMessage>(&json).unwrap(),
            SignalMessage::UserBlocked { .. }
        ));

        let msg = SignalMessage::UserUnblocked {
            user_id: "u5".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(matches!(
            serde_json::from_str::<SignalMessage>(&json).unwrap(),
            SignalMessage::UserUnblocked { .. }
        ));
    }

    #[test]
    fn signal_message_round_trip_ban_management() {
        let msg = SignalMessage::UnbanMember {
            user_id: "u3".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::UnbanMember { user_id } => assert_eq!(user_id, "u3"),
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::ListBans;
        let json = serde_json::to_string(&msg).unwrap();
        assert!(matches!(
            serde_json::from_str::<SignalMessage>(&json).unwrap(),
            SignalMessage::ListBans
        ));

        let msg = SignalMessage::BanList {
            bans: vec![BanInfo {
                user_id: "u3".into(),
                user_name: "BadUser".into(),
                banned_at: 999,
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::BanList { bans } => {
                assert_eq!(bans.len(), 1);
                assert_eq!(bans[0].user_id, "u3");
                assert_eq!(bans[0].user_name, "BadUser");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_group_dm() {
        let msg = SignalMessage::CreateGroupDM {
            user_ids: vec!["u1".into(), "u2".into(), "u3".into()],
            name: Some("Squad".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::CreateGroupDM { user_ids, name } => {
                assert_eq!(user_ids, vec!["u1", "u2", "u3"]);
                assert_eq!(name.as_deref(), Some("Squad"));
            }
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::GroupDMCreated {
            group_id: "g1".into(),
            name: "Squad".into(),
            members: vec!["u1".into(), "u2".into()],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::GroupDMCreated {
                group_id,
                name,
                members,
            } => {
                assert_eq!(group_id, "g1");
                assert_eq!(name, "Squad");
                assert_eq!(members.len(), 2);
            }
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::SendGroupMessage {
            group_id: "g1".into(),
            content: "hello group".into(),
            reply_to_message_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SendGroupMessage {
                group_id, content, ..
            } => {
                assert_eq!(group_id, "g1");
                assert_eq!(content, "hello group");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_invite_settings() {
        let msg = SignalMessage::SetInviteSettings {
            expires_hours: Some(24),
            max_uses: Some(10),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetInviteSettings {
                expires_hours,
                max_uses,
            } => {
                assert_eq!(expires_hours, Some(24));
                assert_eq!(max_uses, Some(10));
            }
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::InviteSettingsUpdated {
            expires_hours: Some(48),
            max_uses: None,
            uses: 3,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::InviteSettingsUpdated {
                expires_hours,
                max_uses,
                uses,
            } => {
                assert_eq!(expires_hours, Some(48));
                assert!(max_uses.is_none());
                assert_eq!(uses, 3);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_threads() {
        let msg = SignalMessage::GetThread {
            channel_id: "c1".into(),
            message_id: "m1".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::GetThread {
                channel_id,
                message_id,
            } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(message_id, "m1");
            }
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::ThreadMessages {
            channel_id: "c1".into(),
            root_message_id: "m1".into(),
            messages: vec![],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::ThreadMessages {
                channel_id,
                root_message_id,
                messages,
            } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(root_message_id, "m1");
                assert!(messages.is_empty());
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_nickname() {
        let msg = SignalMessage::SetNickname {
            nickname: "Cool Guy".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetNickname { nickname } => assert_eq!(nickname, "Cool Guy"),
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::NicknameChanged {
            user_id: "u1".into(),
            nickname: Some("Ally".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::NicknameChanged { user_id, nickname } => {
                assert_eq!(user_id, "u1");
                assert_eq!(nickname.as_deref(), Some("Ally"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_forward_message() {
        let msg = SignalMessage::ForwardMessage {
            source_channel_id: "c1".into(),
            message_id: "m5".into(),
            target_channel_id: "c2".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::ForwardMessage {
                source_channel_id,
                message_id,
                target_channel_id,
            } => {
                assert_eq!(source_channel_id, "c1");
                assert_eq!(message_id, "m5");
                assert_eq!(target_channel_id, "c2");
            }
            _ => panic!("Wrong variant"),
        }
    }
}

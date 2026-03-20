use rand::rngs::OsRng;
use rand::RngCore;
use shared_types;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};
use tokio_tungstenite::tungstenite::Message;

use crate::{ServerStream, LIMITS};

// ─── Types ───

pub(crate) type Tx =
    futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<ServerStream>, Message>;

pub(crate) struct Peer {
    pub id: String,
    pub name: Mutex<String>,
    /// Persistent user identity (set by auth). Used for ban checks across reconnections.
    pub user_id: Mutex<Option<String>>,
    pub room_code: Mutex<Option<String>>,
    /// Lock-free room code cache for the audio relay hot path.
    /// Avoids acquiring room_code mutex on every audio frame (~50fps per peer).
    /// Updated alongside room_code mutex via set_room_code().
    pub room_code_cache: std::sync::RwLock<Option<String>>,
    pub is_muted: AtomicBool,
    pub is_deafened: AtomicBool,
    pub status: Mutex<String>,
    pub tx: Mutex<Tx>,
    pub space_id: Mutex<Option<String>>,
    pub typing_channel_id: Mutex<Option<String>>,
    pub typing_dm_user_id: Mutex<Option<String>>,
    pub watched_friend_ids: Mutex<HashSet<String>>,
    /// Peer's remote IP address (for brute-force rate limiting)
    pub ip: IpAddr,
    /// UDP address for audio relay (set when client completes UDP handshake)
    pub udp_addr: Mutex<Option<SocketAddr>>,
    /// Priority speaker flag — when active, other peers are ducked
    pub is_priority_speaker: AtomicBool,
    /// Whisper target peer IDs (empty = normal broadcast)
    pub whisper_targets: Mutex<Vec<String>>,
    /// Timeout expiry (unix epoch seconds, 0 = no timeout)
    pub timeout_until: AtomicU64,
    // Rate limiting — atomic timestamps avoid lock contention on audio hot path
    pub msg_count: AtomicU32,
    pub rate_window_ms: AtomicU64,
    // Audio frame rate limiting
    pub audio_frame_count: AtomicU32,
    pub audio_rate_window_ms: AtomicU64,
    pub screen_frame_count: AtomicU32,
    pub screen_rate_window_ms: AtomicU64,
}

impl Peer {
    /// Set the peer's room code, updating both the authoritative mutex and fast cache.
    pub async fn set_room_code(&self, code: Option<String>) {
        *self.room_code.lock().await = code.clone();
        match self.room_code_cache.write() {
            Ok(mut cache) => {
                *cache = code;
            }
            Err(poisoned) => {
                log::warn!("room_code_cache write lock was poisoned; recovering");
                *poisoned.into_inner() = code;
            }
        }
    }

    /// Fast lock-free read of the cached room code (for audio relay hot path).
    pub fn cached_room_code(&self) -> Option<String> {
        match self.room_code_cache.read() {
            Ok(cache) => cache.clone(),
            Err(poisoned) => {
                log::warn!("room_code_cache read lock was poisoned; recovering");
                poisoned.into_inner().clone()
            }
        }
    }
}

pub(crate) struct Room {
    pub peer_ids: Vec<String>,
    pub password: Option<String>,
    pub active_screen_share_peer_id: Option<String>,
    pub created_at: Instant,
}

pub(crate) const MAX_SPACE_AUDIT_ENTRIES: usize = 64;

#[allow(dead_code)]
pub(crate) struct Space {
    pub id: String,
    pub name: String,
    pub invite_code: String,
    pub owner_id: String,
    pub channels: Vec<ChannelMeta>,
    pub member_ids: Vec<String>,
    pub member_roles: HashMap<String, shared_types::SpaceRole>,
    /// Text messages per channel_id, capped at LIMITS.max_channel_messages.
    pub text_messages: HashMap<String, VecDeque<shared_types::TextMessageData>>,
    pub audit_log: VecDeque<shared_types::SpaceAuditEntry>,
    /// Slow mode: (channel_id, peer_id) -> last message epoch seconds
    pub slow_mode_timestamps: HashMap<(String, String), u64>,
    pub created_at: Instant,
}

pub(crate) struct ChannelMeta {
    pub id: String,
    pub name: String,
    pub room_key: String, // internal room code for audio relay, e.g. "sp:s1:ch:c1"
    pub channel_type: shared_types::ChannelType,
    pub topic: String,
    pub voice_quality: u8, // 0=Low, 1=Standard, 2=High, 3=Ultra
    pub user_limit: u32,
    pub category: String,
    pub status: String,
    pub slow_mode_secs: u32,
}

pub(crate) struct ServerState {
    pub peers: HashMap<String, Arc<Peer>>,
    pub rooms: HashMap<String, Room>,
    pub spaces: HashMap<String, Space>,
    /// Reverse index: invite_code -> space_id for O(1) lookup
    pub invite_index: HashMap<String, String>,
    pub next_id: u64,
    pub next_space_id: u64,
    pub next_channel_id: u64,
    pub next_message_id: u64,
    pub next_audit_id: u64,
    pub connections_per_ip: HashMap<IpAddr, u32>,
    /// Rate limit for failed JoinSpace attempts per IP: (failure_count, window_start)
    pub join_failures: HashMap<IpAddr, (u32, Instant)>,
    /// UDP session tokens: token_bytes -> peer_id. Tokens are 8 random bytes.
    pub udp_sessions: HashMap<[u8; 8], String>,
}

impl ServerState {
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
            rooms: HashMap::new(),
            spaces: HashMap::new(),
            invite_index: HashMap::new(),
            next_id: 1,
            next_space_id: 1,
            next_channel_id: 1,
            next_message_id: 1,
            next_audit_id: 1,
            connections_per_ip: HashMap::new(),
            join_failures: HashMap::new(),
            udp_sessions: HashMap::new(),
        }
    }

    pub fn alloc_id(&mut self) -> String {
        let id = format!("p{}", self.next_id);
        self.next_id += 1;
        id
    }

    pub fn generate_room_code(&self) -> String {
        for _ in 0..32 {
            let mut bytes = [0u8; 4];
            OsRng.fill_bytes(&mut bytes);
            let code = format!("{:06}", u32::from_le_bytes(bytes) % 1_000_000);
            if !self.rooms.contains_key(&code) {
                return code;
            }
        }

        format!("{:06}", self.next_id % 1_000_000)
    }

    pub fn generate_invite_code(&self) -> String {
        let chars: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        for _ in 0..32 {
            let mut bytes = [0u8; 8];
            OsRng.fill_bytes(&mut bytes);
            let mut result = String::with_capacity(bytes.len());
            for byte in bytes {
                result.push(chars[(byte as usize) % chars.len()] as char);
            }
            if !self.invite_index.contains_key(&result) {
                return result;
            }
        }

        format!("INV{:05}", self.next_space_id % 100_000)
    }

    pub fn alloc_space_id(&mut self) -> String {
        let id = format!("s{}", self.next_space_id);
        self.next_space_id += 1;
        id
    }

    pub fn alloc_channel_id(&mut self) -> String {
        let id = format!("c{}", self.next_channel_id);
        self.next_channel_id += 1;
        id
    }

    pub fn alloc_message_id(&mut self) -> String {
        let id = format!("m{}", self.next_message_id);
        self.next_message_id += 1;
        id
    }

    pub fn alloc_audit_id(&mut self) -> String {
        let id = format!("a{}", self.next_audit_id);
        self.next_audit_id += 1;
        id
    }
}

pub(crate) type State = Arc<RwLock<ServerState>>;
pub(crate) type Db = Option<Arc<crate::persistence::Database>>;

/// Max text messages stored per channel (in-memory ring buffer).
/// Configurable via VOXLINK_MAX_CHANNEL_MESSAGES env var.
pub(crate) fn max_channel_messages() -> usize {
    LIMITS.max_channel_messages
}

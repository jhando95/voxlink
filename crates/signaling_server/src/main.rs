mod handlers;
pub mod persistence;

use futures_util::{SinkExt, StreamExt};
use shared_types::SignalMessage;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Mutex, RwLock};
use tokio_tungstenite::tungstenite::Message;

// ─── Limits ───

const MAX_NAME_LEN: usize = 32;
const MAX_ROOM_PEERS: usize = 10;
const MAX_CONNECTIONS_PER_IP: u32 = 20;
const RATE_LIMIT_PER_SEC: u32 = 100;
const MAX_PASSWORD_LEN: usize = 64;

// ─── Server stream: either plain TCP or TLS ───

enum ServerStream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl AsyncRead for ServerStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            ServerStream::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            ServerStream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for ServerStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            ServerStream::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            ServerStream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            ServerStream::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            ServerStream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            ServerStream::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            ServerStream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

impl Unpin for ServerStream {}

// ─── Types ───

type Tx =
    futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<ServerStream>, Message>;

struct Peer {
    id: String,
    name: Mutex<String>,
    /// Persistent user identity (set by auth). Used for ban checks across reconnections.
    user_id: Mutex<Option<String>>,
    room_code: Mutex<Option<String>>,
    /// Lock-free room code cache for the audio relay hot path.
    /// Avoids acquiring room_code mutex on every audio frame (~50fps per peer).
    /// Updated alongside room_code mutex via set_room_code().
    room_code_cache: std::sync::RwLock<Option<String>>,
    is_muted: AtomicBool,
    is_deafened: AtomicBool,
    tx: Mutex<Tx>,
    space_id: Mutex<Option<String>>,
    typing_channel_id: Mutex<Option<String>>,
    typing_dm_user_id: Mutex<Option<String>>,
    watched_friend_ids: Mutex<HashSet<String>>,
    // Rate limiting
    msg_count: AtomicU32,
    rate_window: Mutex<Instant>,
    // Audio frame rate limiting (#5)
    audio_frame_count: AtomicU32,
    audio_rate_window: Mutex<Instant>,
    screen_frame_count: AtomicU32,
    screen_rate_window: Mutex<Instant>,
}

impl Peer {
    /// Set the peer's room code, updating both the authoritative mutex and fast cache.
    async fn set_room_code(&self, code: Option<String>) {
        *self.room_code.lock().await = code.clone();
        *self.room_code_cache.write().unwrap() = code;
    }

    /// Fast lock-free read of the cached room code (for audio relay hot path).
    fn cached_room_code(&self) -> Option<String> {
        self.room_code_cache.read().unwrap().clone()
    }
}

struct Room {
    peer_ids: Vec<String>,
    password: Option<String>,
    active_screen_share_peer_id: Option<String>,
    created_at: Instant,
}

/// Max text messages stored per channel (in-memory ring buffer).
const MAX_CHANNEL_MESSAGES: usize = 100;

#[allow(dead_code)]
struct Space {
    id: String,
    name: String,
    invite_code: String,
    owner_id: String,
    channels: Vec<ChannelMeta>,
    member_ids: Vec<String>,
    /// Text messages per channel_id, capped at MAX_CHANNEL_MESSAGES.
    text_messages: HashMap<String, VecDeque<shared_types::TextMessageData>>,
    created_at: Instant,
}

struct ChannelMeta {
    id: String,
    name: String,
    room_key: String, // internal room code for audio relay, e.g. "sp:s1:ch:c1"
    channel_type: shared_types::ChannelType,
}

struct ServerState {
    peers: HashMap<String, Arc<Peer>>,
    rooms: HashMap<String, Room>,
    spaces: HashMap<String, Space>,
    /// Reverse index: invite_code -> space_id for O(1) lookup
    invite_index: HashMap<String, String>,
    next_id: u64,
    next_space_id: u64,
    next_channel_id: u64,
    next_message_id: u64,
    connections_per_ip: HashMap<IpAddr, u32>,
}

impl ServerState {
    fn new() -> Self {
        Self {
            peers: HashMap::new(),
            rooms: HashMap::new(),
            spaces: HashMap::new(),
            invite_index: HashMap::new(),
            next_id: 1,
            next_space_id: 1,
            next_channel_id: 1,
            next_message_id: 1,
            connections_per_ip: HashMap::new(),
        }
    }

    fn alloc_id(&mut self) -> String {
        let id = format!("p{}", self.next_id);
        self.next_id += 1;
        id
    }

    fn generate_room_code(&self) -> String {
        // Use OS-seeded entropy via RandomState (no external crate needed)
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_usize(self.next_id as usize);
        hasher.write_usize(self.rooms.len());
        let hash = hasher.finish();
        format!("{:06}", hash % 1_000_000)
    }

    fn generate_invite_code(&self) -> String {
        use std::hash::{BuildHasher, Hasher};
        let chars: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let mut result = String::with_capacity(8);
        for i in 0..8 {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_usize(self.next_space_id as usize);
            hasher.write_usize(i);
            hasher.write_usize(self.spaces.len());
            let hash = hasher.finish();
            result.push(chars[(hash as usize) % chars.len()] as char);
        }
        result
    }

    fn alloc_space_id(&mut self) -> String {
        let id = format!("s{}", self.next_space_id);
        self.next_space_id += 1;
        id
    }

    fn alloc_channel_id(&mut self) -> String {
        let id = format!("c{}", self.next_channel_id);
        self.next_channel_id += 1;
        id
    }

    fn alloc_message_id(&mut self) -> String {
        let id = format!("m{}", self.next_message_id);
        self.next_message_id += 1;
        id
    }
}

type State = Arc<RwLock<ServerState>>;
type Db = Option<Arc<persistence::Database>>;

// ─── Main ───

#[tokio::main(worker_threads = 2)]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let addr = std::env::var("PV_ADDR").unwrap_or_else(|_| "0.0.0.0:9090".into());
    let listener = TcpListener::bind(&addr).await.expect("Failed to bind");

    // TLS setup (optional)
    let tls_acceptor = match (std::env::var("PV_CERT"), std::env::var("PV_KEY")) {
        (Ok(cert_path), Ok(key_path)) => match load_tls_config(&cert_path, &key_path) {
            Ok(config) => {
                log::info!("TLS enabled (cert: {cert_path}, key: {key_path})");
                Some(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
            }
            Err(e) => {
                log::error!("Failed to load TLS config: {e}");
                log::warn!("Falling back to plain WebSocket (insecure)");
                None
            }
        },
        _ => {
            log::warn!("No TLS configured (set PV_CERT and PV_KEY for secure mode)");
            None
        }
    };

    let proto = if tls_acceptor.is_some() { "wss" } else { "ws" };
    log::info!("Signaling server listening on {proto}://{addr}");

    // Initialize persistence
    let db_path = std::env::var("PV_DB_PATH").unwrap_or_else(|_| "./voxlink.db".into());
    let db = match persistence::Database::open(std::path::Path::new(&db_path)) {
        Ok(db) => {
            log::info!("Database opened at {db_path}");
            Some(Arc::new(db))
        }
        Err(e) => {
            log::error!("Failed to open database: {e} — running without persistence");
            None
        }
    };

    let state: State = Arc::new(RwLock::new(ServerState::new()));

    // Load persisted spaces from DB
    if let Some(ref db) = db {
        if let Ok(space_rows) = db.load_all_spaces() {
            let mut s = state.write().await;
            for sr in &space_rows {
                let channels_rows = db.load_channels_for_space(&sr.id).unwrap_or_default();
                let mut channels = Vec::new();
                for cr in &channels_rows {
                    let ct = if cr.channel_type == "text" {
                        shared_types::ChannelType::Text
                    } else {
                        shared_types::ChannelType::Voice
                    };
                    channels.push(ChannelMeta {
                        id: cr.id.clone(),
                        name: cr.name.clone(),
                        room_key: cr.room_key.clone(),
                        channel_type: ct,
                    });
                    // Create room entries for voice channels
                    if ct == shared_types::ChannelType::Voice {
                        s.rooms.entry(cr.room_key.clone()).or_insert_with(|| Room {
                            peer_ids: Vec::new(),
                            password: None,
                            active_screen_share_peer_id: None,
                            created_at: Instant::now(),
                        });
                    }
                }

                // Load text message history
                let mut text_messages: HashMap<String, VecDeque<shared_types::TextMessageData>> =
                    HashMap::new();
                for cr in &channels_rows {
                    if cr.channel_type == "text" {
                        if let Ok(msgs) = db.load_messages_for_channel(&cr.id, MAX_CHANNEL_MESSAGES)
                        {
                            let dq: VecDeque<_> = msgs
                                .into_iter()
                                .map(|m| shared_types::TextMessageData {
                                    sender_id: m.sender_id,
                                    sender_name: m.sender_name,
                                    content: m.content,
                                    timestamp: m.timestamp as u64,
                                    message_id: m.id,
                                    edited: m.edited,
                                    reactions: Vec::new(),
                                    reply_to_message_id: m.reply_to_message_id,
                                    reply_to_sender_name: m.reply_to_sender_name,
                                    reply_preview: m.reply_preview,
                                    pinned: m.pinned,
                                })
                                .collect();
                            if !dq.is_empty() {
                                text_messages.insert(cr.id.clone(), dq);
                            }
                        }
                    }
                }

                s.invite_index.insert(sr.invite_code.clone(), sr.id.clone());
                s.spaces.insert(
                    sr.id.clone(),
                    Space {
                        id: sr.id.clone(),
                        name: sr.name.clone(),
                        invite_code: sr.invite_code.clone(),
                        owner_id: sr.owner_id.clone(),
                        channels,
                        member_ids: Vec::new(),
                        text_messages,
                        created_at: Instant::now(),
                    },
                );
            }

            // Restore ID allocators past the max persisted IDs
            let max_space = db.max_id_suffix("spaces", "id").unwrap_or(0);
            let max_channel = db.max_id_suffix("channels", "id").unwrap_or(0);
            let max_message = db.max_id_suffix("messages", "id").unwrap_or(0);
            let max_direct_message = db.max_id_suffix("direct_messages", "id").unwrap_or(0);
            s.next_space_id = s.next_space_id.max(max_space + 1);
            s.next_channel_id = s.next_channel_id.max(max_channel + 1);
            s.next_message_id = s
                .next_message_id
                .max(max_message.max(max_direct_message) + 1);

            log::info!("Loaded {} space(s) from database", space_rows.len());
        }
    }

    // Start LAN discovery beacon
    let discover_addr = format!("{proto}://{addr}");
    tokio::spawn(run_discovery(discover_addr));

    // Periodic cleanup of stale empty rooms (every 60s)
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                let mut s = state.write().await;
                let before = s.rooms.len();
                // Remove empty rooms older than 5 minutes
                s.rooms.retain(|code, room| {
                    if room.peer_ids.is_empty()
                        && room.created_at.elapsed() > std::time::Duration::from_secs(300)
                    {
                        log::info!("Cleaning up stale room {code}");
                        false
                    } else {
                        true
                    }
                });
                let removed = before - s.rooms.len();
                if removed > 0 {
                    log::info!("Cleaned up {removed} stale room(s)");
                }
            }
        });
    }

    while let Ok((stream, addr)) = listener.accept().await {
        // Per-IP connection limit
        {
            let mut s = state.write().await;
            let count = s.connections_per_ip.entry(addr.ip()).or_insert(0);
            if *count >= MAX_CONNECTIONS_PER_IP {
                log::warn!("Connection limit reached for {}", addr.ip());
                continue;
            }
            *count += 1;
        }

        let state = state.clone();
        let tls = tls_acceptor.clone();
        let db = db.clone();
        tokio::spawn(async move {
            let server_stream = if let Some(acceptor) = tls {
                match acceptor.accept(stream).await {
                    Ok(tls_stream) => ServerStream::Tls(Box::new(tls_stream)),
                    Err(e) => {
                        log::warn!("TLS handshake failed from {addr}: {e}");
                        decrement_ip(&state, addr.ip()).await;
                        return;
                    }
                }
            } else {
                ServerStream::Plain(stream)
            };

            handle_connection(state.clone(), server_stream, addr, db).await;
            decrement_ip(&state, addr.ip()).await;
        });
    }
}

fn load_tls_config(
    cert_path: &str,
    key_path: &str,
) -> Result<tokio_rustls::rustls::ServerConfig, Box<dyn std::error::Error>> {
    let cert_file = std::fs::File::open(cert_path)?;
    let key_file = std::fs::File::open(key_path)?;

    let certs: Vec<_> = rustls_pemfile::certs(&mut std::io::BufReader::new(cert_file))
        .filter_map(|r| r.ok())
        .collect();

    let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(key_file))?
        .ok_or("No private key found in key file")?;

    let config = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    Ok(config)
}

async fn decrement_ip(state: &State, ip: IpAddr) {
    let mut s = state.write().await;
    if let Some(count) = s.connections_per_ip.get_mut(&ip) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            s.connections_per_ip.remove(&ip);
        }
    }
}

// ─── LAN Discovery ───

async fn run_discovery(server_addr: String) {
    let socket = match UdpSocket::bind("0.0.0.0:9091").await {
        Ok(s) => s,
        Err(e) => {
            log::warn!("LAN discovery unavailable: {e}");
            return;
        }
    };
    if socket.set_broadcast(true).is_err() {
        log::warn!("Could not enable UDP broadcast");
        return;
    }

    log::info!("LAN discovery listening on UDP 9091");
    let mut buf = [0u8; 64];
    loop {
        if let Ok((len, src)) = socket.recv_from(&mut buf).await {
            if len >= 16 && &buf[..16] == b"VOXLINK_DISCOVER" {
                let response = format!("VOXLINK_SERVER:{}", server_addr);
                let _ = socket.send_to(response.as_bytes(), src).await;
                log::info!("Discovery response sent to {src}");
            }
        }
    }
}

// ─── Connection Handler ───

async fn handle_connection(state: State, stream: ServerStream, addr: SocketAddr, db: Db) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log::warn!("WebSocket handshake failed from {addr}: {e}");
            return;
        }
    };

    let (tx, mut rx) = ws.split();

    let peer_id = {
        let mut s = state.write().await;
        let id = s.alloc_id();
        s.peers.insert(
            id.clone(),
            Arc::new(Peer {
                id: id.clone(),
                name: Mutex::new(format!("User-{}", &id)),
                user_id: Mutex::new(None),
                room_code: Mutex::new(None),
                room_code_cache: std::sync::RwLock::new(None),
                is_muted: AtomicBool::new(false),
                is_deafened: AtomicBool::new(false),
                tx: Mutex::new(tx),
                space_id: Mutex::new(None),
                typing_channel_id: Mutex::new(None),
                typing_dm_user_id: Mutex::new(None),
                watched_friend_ids: Mutex::new(HashSet::new()),
                msg_count: AtomicU32::new(0),
                rate_window: Mutex::new(Instant::now()),
                audio_frame_count: AtomicU32::new(0),
                audio_rate_window: Mutex::new(Instant::now()),
                screen_frame_count: AtomicU32::new(0),
                screen_rate_window: Mutex::new(Instant::now()),
            }),
        );
        id
    };

    log::info!("Peer {peer_id} connected from {addr}");

    // Keepalive: send WebSocket pings every 30s to survive NAT/firewall timeouts
    let ping_peer = {
        let s = state.read().await;
        s.peers.get(&peer_id).cloned()
    };
    let ping_task = ping_peer.map(|peer| {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            interval.tick().await; // skip first immediate tick
            loop {
                interval.tick().await;
                let mut tx = peer.tx.lock().await;
                if tx.send(Message::Ping(vec![].into())).await.is_err() {
                    break;
                }
            }
        })
    });

    while let Some(msg) = rx.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                // Rate limit signaling messages
                if !check_rate_limit(&state, &peer_id).await {
                    log::warn!("Peer {peer_id} rate limited");
                    continue;
                }
                if let Ok(signal) = serde_json::from_str::<SignalMessage>(&text) {
                    handle_signal(&state, &peer_id, signal, &db).await;
                }
            }
            Ok(Message::Binary(data)) => {
                if data.is_empty() {
                    continue;
                }
                match data[0] {
                    shared_types::MEDIA_PACKET_AUDIO => {
                        relay_audio(&state, &peer_id, &data[1..]).await;
                    }
                    shared_types::MEDIA_PACKET_SCREEN => {
                        relay_screen(&state, &peer_id, &data[1..]).await;
                    }
                    _ => {}
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Pong(_)) => {} // keepalive response, ignore
            Err(e) => {
                log::warn!("Peer {peer_id} error: {e}");
                break;
            }
            _ => {}
        }
    }

    if let Some(task) = ping_task {
        task.abort();
    }
    let disconnected_user_id = {
        let s = state.read().await;
        match s.peers.get(&peer_id) {
            Some(peer) => peer.user_id.lock().await.clone(),
            None => None,
        }
    };
    handle_disconnect(&state, &peer_id).await;
    state.write().await.peers.remove(&peer_id);
    if let Some(user_id) = disconnected_user_id {
        handlers::presence::notify_watchers_for_user(&state, &user_id).await;
    }
    log::info!("Peer {peer_id} disconnected");
}

async fn check_rate_limit(state: &State, peer_id: &str) -> bool {
    let s = state.read().await;
    let peer = match s.peers.get(peer_id) {
        Some(p) => p.clone(),
        None => return false,
    };
    drop(s);

    let now = Instant::now();
    let mut window = peer.rate_window.lock().await;
    if now.duration_since(*window).as_secs() >= 1 {
        *window = now;
        peer.msg_count.store(1, Ordering::Relaxed);
        true
    } else {
        let count = peer.msg_count.fetch_add(1, Ordering::Relaxed);
        count < RATE_LIMIT_PER_SEC
    }
}

// ─── Input Validation ───

fn validate_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Name cannot be empty".into());
    }
    if trimmed.len() > MAX_NAME_LEN {
        return Err(format!("Name too long (max {} characters)", MAX_NAME_LEN));
    }
    Ok(())
}

fn validate_room_code(code: &str) -> Result<(), String> {
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        return Err("Invalid room code (must be 6 digits)".into());
    }
    Ok(())
}

fn validate_password(pw: &Option<String>) -> Result<(), String> {
    if let Some(ref p) = pw {
        if p.len() > MAX_PASSWORD_LEN {
            return Err(format!(
                "Password too long (max {} characters)",
                MAX_PASSWORD_LEN
            ));
        }
    }
    Ok(())
}

// ─── Signal Handler ───

async fn handle_signal(state: &State, peer_id: &str, msg: SignalMessage, db: &Db) {
    match msg {
        SignalMessage::CreateRoom {
            user_name,
            password,
        } => {
            handlers::room::handle_create_room(state, peer_id, user_name, password).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::JoinRoom {
            room_code,
            user_name,
            password,
        } => {
            handlers::room::handle_join_room(state, peer_id, room_code, user_name, password).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::LeaveRoom => {
            handle_disconnect(state, peer_id).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::MuteChanged { is_muted } => {
            handlers::room::handle_mute_changed(state, peer_id, is_muted).await;
        }
        SignalMessage::DeafenChanged { is_deafened } => {
            handlers::room::handle_deafen_changed(state, peer_id, is_deafened).await;
        }
        SignalMessage::StartScreenShare => {
            handlers::room::handle_start_screen_share(state, peer_id).await;
        }
        SignalMessage::StopScreenShare => {
            handlers::room::handle_stop_screen_share(state, peer_id).await;
        }
        SignalMessage::CreateSpace { name, user_name } => {
            handlers::space::handle_create_space(state, peer_id, name, user_name, db).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::JoinSpace {
            invite_code,
            user_name,
        } => {
            handlers::space::handle_join_space(state, peer_id, invite_code, user_name, db).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::LeaveSpace => {
            handlers::space::handle_leave_space(state, peer_id).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::CreateChannel {
            channel_name,
            channel_type,
        } => {
            handlers::channel::handle_create_channel(
                state,
                peer_id,
                channel_name,
                channel_type,
                db,
            )
            .await;
        }
        SignalMessage::JoinChannel { channel_id } => {
            handlers::channel::handle_join_channel(state, peer_id, channel_id).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::LeaveChannel => {
            handlers::channel::handle_leave_channel(state, peer_id).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::DeleteSpace => {
            handlers::space::handle_delete_space(state, peer_id, db).await;
        }
        SignalMessage::SelectTextChannel { channel_id } => {
            handlers::chat::handle_select_text_channel(state, peer_id, channel_id).await;
        }
        SignalMessage::SelectDirectMessage { user_id } => {
            handlers::chat::handle_select_direct_message(state, peer_id, user_id, db).await;
        }
        SignalMessage::SetTyping {
            channel_id,
            is_typing,
        } => {
            handlers::chat::handle_set_typing(state, peer_id, channel_id, is_typing).await;
        }
        SignalMessage::SetDirectTyping { user_id, is_typing } => {
            handlers::chat::handle_set_direct_typing(state, peer_id, user_id, is_typing, db).await;
        }
        SignalMessage::SendTextMessage {
            channel_id,
            content,
            reply_to_message_id,
        } => {
            handlers::chat::handle_send_text_message(
                state,
                peer_id,
                channel_id,
                content,
                reply_to_message_id,
                db,
            )
            .await;
        }
        SignalMessage::SendDirectMessage {
            user_id,
            content,
            reply_to_message_id,
        } => {
            handlers::chat::handle_send_direct_message(
                state,
                peer_id,
                user_id,
                content,
                reply_to_message_id,
                db,
            )
            .await;
        }
        SignalMessage::PinMessage {
            channel_id,
            message_id,
            pinned,
        } => {
            handlers::chat::handle_pin_message(state, peer_id, channel_id, message_id, pinned, db)
                .await;
        }
        SignalMessage::Authenticate { token, user_name } => {
            handlers::auth::handle_authenticate(state, peer_id, token, user_name, db).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::WatchFriendPresence { user_ids } => {
            handlers::presence::handle_watch_friend_presence(state, peer_id, user_ids).await;
        }
        SignalMessage::SendFriendRequest { user_id } => {
            handlers::friends::handle_send_friend_request(state, peer_id, user_id, db).await;
        }
        SignalMessage::RespondFriendRequest { user_id, accept } => {
            handlers::friends::handle_respond_friend_request(state, peer_id, user_id, accept, db)
                .await;
        }
        SignalMessage::CancelFriendRequest { user_id } => {
            handlers::friends::handle_cancel_friend_request(state, peer_id, user_id, db).await;
        }
        SignalMessage::RemoveFriend { user_id } => {
            handlers::friends::handle_remove_friend(state, peer_id, user_id, db).await;
        }
        SignalMessage::EditTextMessage {
            channel_id,
            message_id,
            new_content,
        } => {
            handlers::chat::handle_edit_text_message(
                state,
                peer_id,
                channel_id,
                message_id,
                new_content,
                db,
            )
            .await;
        }
        SignalMessage::EditDirectMessage {
            user_id,
            message_id,
            new_content,
        } => {
            handlers::chat::handle_edit_direct_message(
                state,
                peer_id,
                user_id,
                message_id,
                new_content,
                db,
            )
            .await;
        }
        SignalMessage::DeleteTextMessage {
            channel_id,
            message_id,
        } => {
            handlers::chat::handle_delete_text_message(state, peer_id, channel_id, message_id, db)
                .await;
        }
        SignalMessage::DeleteDirectMessage {
            user_id,
            message_id,
        } => {
            handlers::chat::handle_delete_direct_message(state, peer_id, user_id, message_id, db)
                .await;
        }
        SignalMessage::ReactToMessage {
            channel_id,
            message_id,
            emoji,
        } => {
            handlers::chat::handle_react_to_message(state, peer_id, channel_id, message_id, emoji)
                .await;
        }
        SignalMessage::KickMember { member_id } => {
            handlers::moderation::handle_kick_member(state, peer_id, member_id).await;
        }
        SignalMessage::MuteMember { member_id, muted } => {
            handlers::moderation::handle_mute_member(state, peer_id, member_id, muted).await;
        }
        SignalMessage::BanMember { member_id } => {
            handlers::moderation::handle_ban_member(state, peer_id, member_id, db).await;
        }
        _ => {}
    }
}

async fn send_error(state: &State, peer_id: &str, message: &str) {
    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(
            &peer,
            &SignalMessage::Error {
                message: message.to_string(),
            },
        )
        .await;
    }
}

// ─── Audio Relay ───

const MAX_AUDIO_FRAMES_PER_SEC: u32 = 100; // 50fps normal, 100 allows burst
const MAX_SCREEN_FRAMES_PER_SEC: u32 = 60;

async fn relay_audio(state: &State, sender_id: &str, data: &[u8]) {
    // #3: Reject oversized audio frames
    if data.len() > shared_types::MAX_AUDIO_FRAME_SIZE {
        return;
    }

    // Fast path: read cached room code without acquiring the global state lock
    // or the per-peer room_code mutex. This is the hottest path in the server
    // (~50 calls/sec per peer).
    let room_code = {
        let s = state.read().await;
        let peer = match s.peers.get(sender_id) {
            Some(p) => p.clone(),
            None => return,
        };

        // #5: Audio frame rate limiting
        let now = Instant::now();
        let mut window = peer.audio_rate_window.lock().await;
        if now.duration_since(*window).as_secs() >= 1 {
            *window = now;
            peer.audio_frame_count.store(1, Ordering::Relaxed);
        } else {
            let count = peer.audio_frame_count.fetch_add(1, Ordering::Relaxed);
            if count >= MAX_AUDIO_FRAMES_PER_SEC {
                return;
            }
        }

        peer.cached_room_code()
    };

    let room_code = match room_code {
        Some(c) => c,
        None => return,
    };

    // Build frame once: [kind, id_len, sender_id_bytes, audio_data]
    let mut frame = Vec::with_capacity(2 + sender_id.len() + data.len());
    frame.push(shared_types::MEDIA_PACKET_AUDIO);
    frame.push(sender_id.len() as u8);
    frame.extend_from_slice(sender_id.as_bytes());
    frame.extend_from_slice(data);

    let others = handlers::collect_room_others(state, &room_code, sender_id).await;

    // Send with timeout to prevent slow peers from blocking the relay.
    // If a peer can't accept within 500ms, drop the frame for them.
    let send_timeout = std::time::Duration::from_millis(500);

    // Single-peer fast path (common case): avoid Arc overhead
    if others.len() == 1 {
        let fut = async {
            let mut tx = others[0].tx.lock().await;
            let _ = tx.send(Message::Binary(frame.into())).await;
        };
        let _ = tokio::time::timeout(send_timeout, fut).await;
        return;
    }

    let frame = Arc::new(frame);
    let futs: Vec<_> = others
        .into_iter()
        .map(|peer| {
            let frame = frame.clone();
            let timeout_dur = send_timeout;
            async move {
                let fut = async {
                    let mut tx = peer.tx.lock().await;
                    let _ = tx.send(Message::Binary((*frame).clone().into())).await;
                };
                let _ = tokio::time::timeout(timeout_dur, fut).await;
            }
        })
        .collect();
    futures_util::future::join_all(futs).await;
}

async fn relay_screen(state: &State, sender_id: &str, data: &[u8]) {
    if data.len() > shared_types::MAX_SCREEN_FRAME_SIZE {
        return;
    }

    let (room_code, allowed) = {
        let s = state.read().await;
        let peer = match s.peers.get(sender_id) {
            Some(p) => p.clone(),
            None => return,
        };

        let now = Instant::now();
        let mut window = peer.screen_rate_window.lock().await;
        if now.duration_since(*window).as_secs() >= 1 {
            *window = now;
            peer.screen_frame_count.store(1, Ordering::Relaxed);
        } else {
            let count = peer.screen_frame_count.fetch_add(1, Ordering::Relaxed);
            if count >= MAX_SCREEN_FRAMES_PER_SEC {
                return;
            }
        }

        let room_code = peer.cached_room_code();
        let allowed = room_code
            .as_ref()
            .and_then(|code| s.rooms.get(code))
            .and_then(|room| room.active_screen_share_peer_id.as_deref())
            == Some(sender_id);
        (room_code, allowed)
    };

    if !allowed {
        return;
    }

    let Some(room_code) = room_code else {
        return;
    };

    let mut frame = Vec::with_capacity(2 + sender_id.len() + data.len());
    frame.push(shared_types::MEDIA_PACKET_SCREEN);
    frame.push(sender_id.len() as u8);
    frame.extend_from_slice(sender_id.as_bytes());
    frame.extend_from_slice(data);

    let others = handlers::collect_room_others(state, &room_code, sender_id).await;
    if others.is_empty() {
        return;
    }

    let send_timeout = std::time::Duration::from_millis(300);
    let frame = Arc::new(frame);
    let futs: Vec<_> = others
        .into_iter()
        .map(|peer| {
            let frame = frame.clone();
            async move {
                let fut = async {
                    let mut tx = peer.tx.lock().await;
                    let _ = tx.send(Message::Binary((*frame).clone().into())).await;
                };
                let _ = tokio::time::timeout(send_timeout, fut).await;
            }
        })
        .collect();
    futures_util::future::join_all(futs).await;
}

// ─── Disconnect ───

async fn handle_disconnect(state: &State, peer_id: &str) {
    // Use cached room code (lock-free) for disconnect path
    let room_code = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => peer.cached_room_code(),
            None => None,
        }
    };

    if let Some(ref code) = room_code {
        handlers::room::stop_screen_share_in_room(state, code, peer_id).await;
        let remaining = handlers::collect_room_others(state, code, peer_id).await;

        {
            let mut s = state.write().await;
            if let Some(room) = s.rooms.get_mut(code) {
                room.peer_ids.retain(|pid| pid != peer_id);
                if room.peer_ids.is_empty() {
                    s.rooms.remove(code);
                    log::info!("Room {code} removed (empty)");
                }
            }
        }

        let notify = SignalMessage::PeerLeft {
            peer_id: peer_id.to_string(),
        };
        for peer in remaining {
            send_to(&peer, &notify).await;
        }
    }

    // Handle space membership cleanup
    let space_id = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => peer.space_id.lock().await.clone(),
            None => None,
        }
    };

    if let Some(ref sid) = space_id {
        handlers::chat::clear_typing_for_peer(state, peer_id).await;
        {
            let mut s = state.write().await;
            if let Some(space) = s.spaces.get_mut(sid) {
                space.member_ids.retain(|id| id != peer_id);
            }
        }

        let notify = SignalMessage::MemberOffline {
            member_id: peer_id.to_string(),
        };
        handlers::broadcast_to_space(state, sid, peer_id, &notify).await;

        if let Some(peer) = state.read().await.peers.get(peer_id) {
            *peer.space_id.lock().await = None;
        }
    }

    handlers::chat::clear_direct_typing_for_peer(state, peer_id).await;

    if let Some(peer) = state.read().await.peers.get(peer_id) {
        peer.set_room_code(None).await;
    }
}

async fn send_to(peer: &Peer, msg: &SignalMessage) {
    if let Ok(json) = serde_json::to_string(msg) {
        let mut tx = peer.tx.lock().await;
        let _ = tx.send(Message::Text(json.into())).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_room_code_is_6_digits() {
        let state = ServerState::new();
        for _ in 0..100 {
            let code = state.generate_room_code();
            assert_eq!(code.len(), 6, "Room code should be 6 characters: {code}");
            assert!(
                code.chars().all(|c| c.is_ascii_digit()),
                "Room code should be all digits: {code}"
            );
        }
    }

    #[test]
    fn validate_name_valid() {
        assert!(validate_name("Alice").is_ok());
        assert!(validate_name("A").is_ok());
        assert!(validate_name("A long but valid name").is_ok());
    }

    #[test]
    fn validate_name_empty() {
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
    }

    #[test]
    fn validate_name_too_long() {
        let long_name = "A".repeat(MAX_NAME_LEN + 1);
        assert!(validate_name(&long_name).is_err());

        // Exactly at limit should be ok
        let exact = "A".repeat(MAX_NAME_LEN);
        assert!(validate_name(&exact).is_ok());
    }

    #[test]
    fn validate_room_code_valid() {
        assert!(validate_room_code("123456").is_ok());
        assert!(validate_room_code("000000").is_ok());
        assert!(validate_room_code("999999").is_ok());
    }

    #[test]
    fn validate_room_code_invalid() {
        assert!(validate_room_code("12345").is_err()); // too short
        assert!(validate_room_code("1234567").is_err()); // too long
        assert!(validate_room_code("12345a").is_err()); // non-digit
        assert!(validate_room_code("").is_err()); // empty
        assert!(validate_room_code("abcdef").is_err()); // letters
    }

    #[test]
    fn validate_password_valid() {
        assert!(validate_password(&None).is_ok());
        assert!(validate_password(&Some("secret".into())).is_ok());
        assert!(validate_password(&Some("".into())).is_ok());
    }

    #[test]
    fn validate_password_too_long() {
        let long_pw = "x".repeat(MAX_PASSWORD_LEN + 1);
        assert!(validate_password(&Some(long_pw)).is_err());

        // Exactly at limit should be ok
        let exact = "x".repeat(MAX_PASSWORD_LEN);
        assert!(validate_password(&Some(exact)).is_ok());
    }

    #[test]
    fn alloc_id_sequential() {
        let mut state = ServerState::new();
        let id1 = state.alloc_id();
        let id2 = state.alloc_id();
        let id3 = state.alloc_id();
        assert_eq!(id1, "p1");
        assert_eq!(id2, "p2");
        assert_eq!(id3, "p3");
    }
}

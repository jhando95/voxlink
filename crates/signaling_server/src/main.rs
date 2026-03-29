mod handlers;
pub mod persistence;
mod types;

pub(crate) use types::{
    ChannelMeta, Db, Peer, Room, ServerState, Space, State,
    max_channel_messages, MAX_SPACE_AUDIT_ENTRIES,
};

use futures_util::{SinkExt, StreamExt};
use rand::rngs::OsRng;
use rand::RngCore;
use shared_types::SignalMessage;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Mutex, RwLock};
use tokio_tungstenite::tungstenite::Message;

// ─── Limits ───

const MAX_NAME_LEN: usize = 32;
const MAX_PASSWORD_LEN: usize = 64;
const DB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Server limits, configurable via environment variables with sensible defaults.
struct ServerLimits {
    max_room_peers: usize,
    max_connections_per_ip: u32,
    max_channel_messages: usize,
    max_audio_fps: u32,
    max_screen_fps: u32,
    rate_limit_per_sec: u32,
}

fn env_or<T: std::str::FromStr>(var: &str, default: T) -> T {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// UDP relay port, set at startup. 0 = disabled.
static UDP_PORT: AtomicU16 = AtomicU16::new(0);

static LIMITS: std::sync::LazyLock<ServerLimits> = std::sync::LazyLock::new(|| ServerLimits {
    max_room_peers: env_or("VOXLINK_MAX_ROOM_PEERS", 10),
    max_connections_per_ip: env_or("VOXLINK_MAX_CONNECTIONS_PER_IP", 20),
    max_channel_messages: env_or("VOXLINK_MAX_CHANNEL_MESSAGES", 100),
    max_audio_fps: env_or("VOXLINK_MAX_AUDIO_FPS", 100),
    max_screen_fps: env_or("VOXLINK_MAX_SCREEN_FPS", 60),
    rate_limit_per_sec: env_or("VOXLINK_RATE_LIMIT_PER_SEC", 100),
});

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

// Types are in types.rs, re-exported via `pub(crate) use types::*` above.
type Metrics = Arc<ServerMetrics>;

struct ServerMetrics {
    connection_attempts_total: AtomicU64,
    active_connections: AtomicU64,
    websocket_handshake_failures_total: AtomicU64,
    signaling_messages_total: AtomicU64,
    malformed_signaling_messages_total: AtomicU64,
    signaling_rate_limited_total: AtomicU64,
    auth_success_total: AtomicU64,
    auth_failure_total: AtomicU64,
    audio_frames_in_total: AtomicU64,
    audio_frames_out_total: AtomicU64,
    screen_frames_in_total: AtomicU64,
    screen_frames_out_total: AtomicU64,
    udp_frames_in_total: AtomicU64,
    udp_frames_out_total: AtomicU64,
    started_at: Instant,
}

impl Default for ServerMetrics {
    fn default() -> Self {
        Self {
            connection_attempts_total: AtomicU64::new(0),
            active_connections: AtomicU64::new(0),
            websocket_handshake_failures_total: AtomicU64::new(0),
            signaling_messages_total: AtomicU64::new(0),
            malformed_signaling_messages_total: AtomicU64::new(0),
            signaling_rate_limited_total: AtomicU64::new(0),
            auth_success_total: AtomicU64::new(0),
            auth_failure_total: AtomicU64::new(0),
            audio_frames_in_total: AtomicU64::new(0),
            audio_frames_out_total: AtomicU64::new(0),
            screen_frames_in_total: AtomicU64::new(0),
            screen_frames_out_total: AtomicU64::new(0),
            udp_frames_in_total: AtomicU64::new(0),
            udp_frames_out_total: AtomicU64::new(0),
            started_at: Instant::now(),
        }
    }
}

fn bind_requires_tls(addr: &str) -> bool {
    match addr.to_socket_addrs() {
        Ok(addrs) => addrs
            .map(|socket_addr| socket_addr.ip())
            .any(|ip| !ip.is_loopback()),
        Err(_) => {
            !addr.starts_with("127.0.0.1:")
                && !addr.starts_with("[::1]:")
                && !addr.starts_with("localhost:")
        }
    }
}

fn allow_insecure_public_bind() -> bool {
    matches!(
        std::env::var("PV_ALLOW_INSECURE").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
    )
}

async fn run_metrics_server(state: State, metrics: Metrics, addr: String, tls_enabled: bool) {
    let listener = match TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(e) => {
            log::error!("Metrics endpoint unavailable on {addr}: {e}");
            return;
        }
    };

    log::info!("Metrics endpoint listening on http://{addr}");
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        let state = state.clone();
        let metrics = metrics.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let read =
                tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut buf))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..read]);
            let is_health = request.starts_with("GET /healthz ");
            let body = if is_health {
                "ok\n".to_string()
            } else {
                render_metrics(&state, &metrics, tls_enabled).await
            };
            let content_type = if is_health {
                "text/plain; charset=utf-8"
            } else {
                "text/plain; version=0.0.4; charset=utf-8"
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}

async fn render_metrics(state: &State, metrics: &ServerMetrics, tls_enabled: bool) -> String {
    let s = state.read().await;
    let active_rooms = s.rooms.len();
    let active_spaces = s.spaces.len();
    let connected_peers = s.peers.len();
    let udp_sessions = s.udp_sessions.len();
    let total_room_peers: usize = s.rooms.values().map(|r| r.peer_ids.len()).sum();
    let max_room_peers: usize = s.rooms.values().map(|r| r.peer_ids.len()).max().unwrap_or(0);
    let total_space_members: usize = s.spaces.values().map(|sp| sp.member_ids.len()).sum();
    drop(s);

    let uptime_secs = metrics.started_at.elapsed().as_secs();

    format!(
        concat!(
            "# TYPE voxlink_connection_attempts_total counter\n",
            "voxlink_connection_attempts_total {}\n",
            "# TYPE voxlink_active_connections gauge\n",
            "voxlink_active_connections {}\n",
            "# TYPE voxlink_websocket_handshake_failures_total counter\n",
            "voxlink_websocket_handshake_failures_total {}\n",
            "# TYPE voxlink_signaling_messages_total counter\n",
            "voxlink_signaling_messages_total {}\n",
            "# TYPE voxlink_malformed_signaling_messages_total counter\n",
            "voxlink_malformed_signaling_messages_total {}\n",
            "# TYPE voxlink_signaling_rate_limited_total counter\n",
            "voxlink_signaling_rate_limited_total {}\n",
            "# TYPE voxlink_auth_success_total counter\n",
            "voxlink_auth_success_total {}\n",
            "# TYPE voxlink_auth_failure_total counter\n",
            "voxlink_auth_failure_total {}\n",
            "# TYPE voxlink_audio_frames_in_total counter\n",
            "voxlink_audio_frames_in_total {}\n",
            "# TYPE voxlink_audio_frames_out_total counter\n",
            "voxlink_audio_frames_out_total {}\n",
            "# TYPE voxlink_screen_frames_in_total counter\n",
            "voxlink_screen_frames_in_total {}\n",
            "# TYPE voxlink_screen_frames_out_total counter\n",
            "voxlink_screen_frames_out_total {}\n",
            "# TYPE voxlink_udp_frames_in_total counter\n",
            "voxlink_udp_frames_in_total {}\n",
            "# TYPE voxlink_udp_frames_out_total counter\n",
            "voxlink_udp_frames_out_total {}\n",
            "# TYPE voxlink_active_rooms gauge\n",
            "voxlink_active_rooms {}\n",
            "# TYPE voxlink_active_spaces gauge\n",
            "voxlink_active_spaces {}\n",
            "# TYPE voxlink_connected_peers gauge\n",
            "voxlink_connected_peers {}\n",
            "# TYPE voxlink_udp_sessions gauge\n",
            "voxlink_udp_sessions {}\n",
            "# TYPE voxlink_total_room_peers gauge\n",
            "voxlink_total_room_peers {}\n",
            "# TYPE voxlink_max_room_peers gauge\n",
            "voxlink_max_room_peers {}\n",
            "# TYPE voxlink_total_space_members gauge\n",
            "voxlink_total_space_members {}\n",
            "# TYPE voxlink_uptime_seconds gauge\n",
            "voxlink_uptime_seconds {}\n",
            "# TYPE voxlink_tls_enabled gauge\n",
            "voxlink_tls_enabled {}\n",
        ),
        metrics.connection_attempts_total.load(Ordering::Relaxed),
        metrics.active_connections.load(Ordering::Relaxed),
        metrics
            .websocket_handshake_failures_total
            .load(Ordering::Relaxed),
        metrics.signaling_messages_total.load(Ordering::Relaxed),
        metrics
            .malformed_signaling_messages_total
            .load(Ordering::Relaxed),
        metrics.signaling_rate_limited_total.load(Ordering::Relaxed),
        metrics.auth_success_total.load(Ordering::Relaxed),
        metrics.auth_failure_total.load(Ordering::Relaxed),
        metrics.audio_frames_in_total.load(Ordering::Relaxed),
        metrics.audio_frames_out_total.load(Ordering::Relaxed),
        metrics.screen_frames_in_total.load(Ordering::Relaxed),
        metrics.screen_frames_out_total.load(Ordering::Relaxed),
        metrics.udp_frames_in_total.load(Ordering::Relaxed),
        metrics.udp_frames_out_total.load(Ordering::Relaxed),
        active_rooms,
        active_spaces,
        connected_peers,
        udp_sessions,
        total_room_peers,
        max_room_peers,
        total_space_members,
        uptime_secs,
        if tls_enabled { 1 } else { 0 },
    )
}

// ─── Main ───

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    // Force-initialize limits from env vars and log them
    log::info!(
        "Server limits: max_room_peers={}, max_connections_per_ip={}, max_channel_messages={}, max_audio_fps={}, max_screen_fps={}, rate_limit_per_sec={}",
        LIMITS.max_room_peers,
        LIMITS.max_connections_per_ip,
        LIMITS.max_channel_messages,
        LIMITS.max_audio_fps,
        LIMITS.max_screen_fps,
        LIMITS.rate_limit_per_sec,
    );

    let addr = std::env::var("PV_ADDR").unwrap_or_else(|_| "0.0.0.0:9090".into());

    // TLS setup (optional)
    let tls_acceptor = match (std::env::var("PV_CERT"), std::env::var("PV_KEY")) {
        (Ok(cert_path), Ok(key_path)) => match load_tls_config(&cert_path, &key_path) {
            Ok(config) => {
                log::info!("TLS enabled (cert: {cert_path}, key: {key_path})");
                Some(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
            }
            Err(e) => {
                log::error!("Failed to load TLS config: {e}");
                None
            }
        },
        _ => None,
    };

    if tls_acceptor.is_none() && bind_requires_tls(&addr) && !allow_insecure_public_bind() {
        log::error!(
            "Refusing insecure public bind on {addr}. Configure PV_CERT and PV_KEY or set PV_ALLOW_INSECURE=1 for local testing only."
        );
        std::process::exit(1);
    }

    if tls_acceptor.is_none() {
        if bind_requires_tls(&addr) {
            log::warn!("Starting insecure public WebSocket server on {addr} because PV_ALLOW_INSECURE is enabled");
        } else {
            log::warn!("No TLS configured; loopback-only server will use plain WebSocket");
        }
    }

    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            log::error!("Failed to bind to {addr}: {e}. Is another server already running on this port?");
            std::process::exit(1);
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
    let metrics: Metrics = Arc::new(ServerMetrics::default());

    // Load persisted spaces from DB
    if let Some(ref db) = db {
        if let Ok(space_rows) = db.load_all_spaces() {
            let mut s = state.write().await;
            for sr in &space_rows {
                let channels_rows = db.load_channels_for_space(&sr.id).unwrap_or_default();
                let role_rows = db.load_space_roles(&sr.id).unwrap_or_default();
                let audit_rows = db
                    .load_audit_log_for_space(&sr.id, MAX_SPACE_AUDIT_ENTRIES)
                    .unwrap_or_default();
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
                        topic: cr.topic.clone().unwrap_or_default(),
                        voice_quality: cr.voice_quality.unwrap_or(2),
                        user_limit: 0,
                        category: String::new(),
                        status: String::new(),
                        slow_mode_secs: 0,
                        min_role: shared_types::SpaceRole::Member,
            position: 0,
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
                        if let Ok(msgs) = db.load_messages_for_channel(&cr.id, max_channel_messages())
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
                                    forwarded_from: None,
                                })
                                .collect();
                            if !dq.is_empty() {
                                text_messages.insert(cr.id.clone(), dq);
                            }
                        }
                    }
                }

                let member_roles = role_rows
                    .into_iter()
                    .filter_map(|row| {
                        let role = match row.role.as_str() {
                            "owner" => shared_types::SpaceRole::Owner,
                            "admin" => shared_types::SpaceRole::Admin,
                            "moderator" => shared_types::SpaceRole::Moderator,
                            "member" => shared_types::SpaceRole::Member,
                            _ => return None,
                        };
                        Some((row.user_id, role))
                    })
                    .collect::<HashMap<_, _>>();
                let audit_log = audit_rows
                    .into_iter()
                    .map(|row| shared_types::SpaceAuditEntry {
                        id: row.id,
                        actor_name: row.actor_name,
                        action: row.action,
                        target_name: row.target_name.unwrap_or_default(),
                        detail: row.detail,
                        timestamp: row.created_at as u64,
                    })
                    .collect::<VecDeque<_>>();

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
                        member_roles,
                        text_messages,
                        audit_log,
                        slow_mode_timestamps: HashMap::new(),
                        created_at: Instant::now(),
                    },
                );
            }

            // Restore ID allocators past the max persisted IDs
            let max_space = db.max_id_suffix("spaces", "id").unwrap_or(0);
            let max_channel = db.max_id_suffix("channels", "id").unwrap_or(0);
            let max_message = db.max_id_suffix("messages", "id").unwrap_or(0);
            let max_direct_message = db.max_id_suffix("direct_messages", "id").unwrap_or(0);
            let max_audit = db.max_id_suffix("space_audit_log", "id").unwrap_or(0);
            s.next_space_id = s.next_space_id.max(max_space + 1);
            s.next_channel_id = s.next_channel_id.max(max_channel + 1);
            s.next_message_id = s
                .next_message_id
                .max(max_message.max(max_direct_message) + 1);
            s.next_audit_id = s.next_audit_id.max(max_audit + 1);

            log::info!("Loaded {} space(s) from database", space_rows.len());
        }
    }

    // Start LAN discovery beacon
    let discover_addr = format!("{proto}://{addr}");
    tokio::spawn(run_discovery(discover_addr));

    // Start UDP audio relay (port = WS port + 1, or PV_UDP_PORT env var)
    {
        let ws_port: u16 = addr
            .split(':')
            .next_back()
            .and_then(|p| p.parse().ok())
            .unwrap_or(9090);
        let udp_port: u16 = env_or("PV_UDP_PORT", ws_port + 1);
        let udp_addr_str = format!("0.0.0.0:{udp_port}");
        match UdpSocket::bind(&udp_addr_str).await {
            Ok(socket) => {
                UDP_PORT.store(udp_port, Ordering::Relaxed);
                log::info!("UDP audio relay listening on {udp_addr_str}");
                let udp_socket = Arc::new(socket);
                tokio::spawn(run_udp_relay(
                    state.clone(),
                    metrics.clone(),
                    udp_socket,
                ));
            }
            Err(e) => {
                log::warn!("UDP audio relay unavailable (failed to bind {udp_addr_str}): {e}");
                // Server still works — all audio goes through WebSocket
            }
        }
    }

    if let Ok(metrics_addr) = std::env::var("PV_METRICS_ADDR") {
        if !metrics_addr.trim().is_empty() {
            tokio::spawn(run_metrics_server(
                state.clone(),
                metrics.clone(),
                metrics_addr,
                tls_acceptor.is_some(),
            ));
        }
    }

    // Periodic cleanup of stale resources (every 60s)
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                let mut s = state.write().await;

                // Remove empty rooms older than 5 minutes
                let before = s.rooms.len();
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

                // Clean stale member_ids that reference disconnected peers
                let connected: HashSet<String> = s.peers.keys().cloned().collect();
                for space in s.spaces.values_mut() {
                    let before_members = space.member_ids.len();
                    space
                        .member_ids
                        .retain(|mid| connected.contains(mid.as_str()));
                    let removed_members = before_members - space.member_ids.len();
                    if removed_members > 0 {
                        log::info!(
                            "Removed {removed_members} stale member(s) from space {}",
                            space.name
                        );
                    }
                }

                // Expire stale auth rate-limit entries (older than 10 minutes)
                let auth_before = s.auth_attempts.len();
                s.auth_attempts
                    .retain(|_, (_, window_start)| window_start.elapsed().as_secs() < 600);
                let auth_removed = auth_before - s.auth_attempts.len();
                if auth_removed > 0 {
                    log::debug!("Cleaned up {auth_removed} stale auth rate-limit entries");
                }

                // Expire stale join-failure rate-limit entries (older than 10 minutes)
                let join_before = s.join_failures.len();
                s.join_failures
                    .retain(|_, (_, window_start)| window_start.elapsed().as_secs() < 600);
                let join_removed = join_before - s.join_failures.len();
                if join_removed > 0 {
                    log::debug!("Cleaned up {join_removed} stale join-failure entries");
                }

                // Expire stale slow-mode timestamps (older than 5 minutes)
                let now_epoch = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                for space in s.spaces.values_mut() {
                    space
                        .slow_mode_timestamps
                        .retain(|_, &mut ts| now_epoch.saturating_sub(ts) < 300);
                }

                // Expire stale UDP session tokens (peers disconnected but tokens remain)
                let udp_before = s.udp_sessions.len();
                // Reuse connected set from above for UDP cleanup
                s.udp_sessions
                    .retain(|_, peer_id| connected.contains(peer_id.as_str()));
                let udp_removed = udp_before - s.udp_sessions.len();
                if udp_removed > 0 {
                    log::debug!("Cleaned up {udp_removed} orphaned UDP session tokens");
                }
            }
        });
    }

    // Graceful shutdown: accept loop races against ctrl_c / SIGTERM
    loop {
        tokio::select! {
            result = listener.accept() => {
                let Ok((stream, addr)) = result else { break };
                // Per-IP connection limit
                {
                    let mut s = state.write().await;
                    let count = s.connections_per_ip.entry(addr.ip()).or_insert(0);
                    if *count >= LIMITS.max_connections_per_ip {
                        log::warn!("Connection limit reached for {}", addr.ip());
                        continue;
                    }
                    *count += 1;
                }

                let state = state.clone();
                let metrics = metrics.clone();
                let tls = tls_acceptor.clone();
                let db = db.clone();
                tokio::spawn(async move {
                    metrics
                        .connection_attempts_total
                        .fetch_add(1, Ordering::Relaxed);
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

                    handle_connection(state.clone(), metrics, server_stream, addr, db).await;
                    decrement_ip(&state, addr.ip()).await;
                });
            }
            _ = tokio::signal::ctrl_c() => {
                log::info!("Shutdown signal received, notifying all peers...");
                break;
            }
        }
    }

    // Broadcast ServerShutdown to all connected peers before exiting
    {
        let s = state.read().await;
        let shutdown_msg = SignalMessage::ServerShutdown;
        for peer in s.peers.values() {
            send_to(peer, &shutdown_msg).await;
        }
    }
    // Give peers a moment to receive the message
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    log::info!("Server shut down gracefully");
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

async fn handle_connection(
    state: State,
    metrics: Metrics,
    stream: ServerStream,
    addr: SocketAddr,
    db: Db,
) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            metrics
                .websocket_handshake_failures_total
                .fetch_add(1, Ordering::Relaxed);
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
                status: Mutex::new(String::new()),
                tx: Mutex::new(tx),
                space_id: Mutex::new(None),
                typing_channel_id: Mutex::new(None),
                typing_dm_user_id: Mutex::new(None),
                watched_friend_ids: Mutex::new(HashSet::new()),
                ip: addr.ip(),
                udp_addr: std::sync::RwLock::new(None),
                is_priority_speaker: AtomicBool::new(false),
                whisper_targets: std::sync::RwLock::new(Vec::new()),
                timeout_until: AtomicU64::new(0),
                msg_count: AtomicU32::new(0),
                rate_window_ms: AtomicU64::new(instant_to_ms()),
                audio_frame_count: AtomicU32::new(0),
                audio_rate_window_ms: AtomicU64::new(instant_to_ms()),
                screen_frame_count: AtomicU32::new(0),
                screen_rate_window_ms: AtomicU64::new(instant_to_ms()),
                blocked_by: std::sync::RwLock::new(HashSet::new()),
            }),
        );
        id
    };

    metrics.active_connections.fetch_add(1, Ordering::Relaxed);

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
                    metrics
                        .signaling_rate_limited_total
                        .fetch_add(1, Ordering::Relaxed);
                    log::warn!("Peer {peer_id} rate limited");
                    continue;
                }
                if let Ok(signal) = serde_json::from_str::<SignalMessage>(&text) {
                    metrics
                        .signaling_messages_total
                        .fetch_add(1, Ordering::Relaxed);
                    handle_signal(&state, &metrics, &peer_id, signal, &db).await;
                } else {
                    metrics
                        .malformed_signaling_messages_total
                        .fetch_add(1, Ordering::Relaxed);
                    log::debug!("Malformed signal from {peer_id}: {}", &text[..text.len().min(200)]);
                }
            }
            Ok(Message::Binary(data)) => {
                if data.is_empty() {
                    continue;
                }
                match data[0] {
                    shared_types::MEDIA_PACKET_AUDIO => {
                        relay_audio(&state, &metrics, &peer_id, &data[1..]).await;
                    }
                    shared_types::MEDIA_PACKET_SCREEN => {
                        relay_screen(&state, &metrics, &peer_id, &data[1..]).await;
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
    {
        let mut s = state.write().await;
        s.peers.remove(&peer_id);
        // Clean up any UDP session token for this peer
        s.udp_sessions.retain(|_, pid| pid != &peer_id);
    }
    if let Some(user_id) = disconnected_user_id {
        handlers::presence::notify_watchers_for_user(&state, &user_id).await;
    }
    metrics.active_connections.fetch_sub(1, Ordering::Relaxed);
    log::info!("Peer {peer_id} disconnected");
}

/// Monotonic millisecond timestamp for lock-free rate limiting.
fn instant_to_ms() -> u64 {
    // Using system uptime-style monotonic clock avoids Instant → u64 issues.
    // We only need relative 1-second windows, so wrapping after ~584 million years is fine.
    static EPOCH: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
    EPOCH.elapsed().as_millis() as u64
}

/// Lock-free rate limit check using atomic timestamp + counter.
fn atomic_rate_check(window_ms: &AtomicU64, counter: &AtomicU32, limit: u32) -> bool {
    let now = instant_to_ms();
    let prev = window_ms.load(Ordering::Relaxed);
    if now.wrapping_sub(prev) >= 1000 {
        // New window — reset counter. CAS to avoid races resetting twice.
        if window_ms.compare_exchange(prev, now, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            counter.store(1, Ordering::Relaxed);
        }
        true
    } else {
        let count = counter.fetch_add(1, Ordering::Relaxed);
        count < limit
    }
}

async fn check_rate_limit(state: &State, peer_id: &str) -> bool {
    let s = state.read().await;
    let peer = match s.peers.get(peer_id) {
        Some(p) => p.clone(),
        None => return false,
    };
    drop(s);

    atomic_rate_check(&peer.rate_window_ms, &peer.msg_count, LIMITS.rate_limit_per_sec)
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

async fn handle_signal(
    state: &State,
    metrics: &Metrics,
    peer_id: &str,
    msg: SignalMessage,
    db: &Db,
) {
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
            voice_quality,
        } => {
            handlers::channel::handle_create_channel(
                state,
                peer_id,
                channel_name,
                channel_type,
                voice_quality,
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
        SignalMessage::DeleteChannel { channel_id } => {
            handlers::channel::handle_delete_channel(state, peer_id, channel_id, db).await;
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
            if handlers::auth::handle_authenticate(state, peer_id, token, user_name, db).await {
                metrics.auth_success_total.fetch_add(1, Ordering::Relaxed);
            } else {
                metrics.auth_failure_total.fetch_add(1, Ordering::Relaxed);
            }
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
        SignalMessage::SetUserStatus { status } => {
            handle_set_user_status(state, peer_id, status, db).await;
        }
        SignalMessage::SetChannelTopic { channel_id, topic } => {
            handle_set_channel_topic(state, peer_id, channel_id, topic, db).await;
        }
        SignalMessage::KickMember { member_id } => {
            handlers::moderation::handle_kick_member(state, peer_id, member_id, db).await;
        }
        SignalMessage::MuteMember { member_id, muted } => {
            handlers::moderation::handle_mute_member(state, peer_id, member_id, muted, db).await;
        }
        SignalMessage::BanMember { member_id } => {
            handlers::moderation::handle_ban_member(state, peer_id, member_id, db).await;
        }
        SignalMessage::SetMemberRole { user_id, role } => {
            handlers::space::handle_set_member_role(state, peer_id, user_id, role, db).await;
        }
        SignalMessage::SearchMessages {
            channel_id,
            query,
            limit,
        } => {
            handlers::chat::handle_search_messages(state, peer_id, channel_id, query, limit, db)
                .await;
        }
        SignalMessage::SetProfile { bio } => {
            handle_set_profile(state, peer_id, bio, db).await;
        }
        SignalMessage::RequestUdp => {
            handle_request_udp(state, peer_id).await;
        }
        SignalMessage::SetChannelUserLimit {
            channel_id,
            user_limit,
        } => {
            handle_channel_setting(state, peer_id, channel_id, ChannelSetting::UserLimit(user_limit))
                .await;
        }
        SignalMessage::SetChannelSlowMode {
            channel_id,
            slow_mode_secs,
        } => {
            handle_channel_setting(
                state,
                peer_id,
                channel_id,
                ChannelSetting::SlowMode(slow_mode_secs),
            )
            .await;
        }
        SignalMessage::SetChannelCategory {
            channel_id,
            category,
        } => {
            handle_channel_setting(state, peer_id, channel_id, ChannelSetting::Category(category))
                .await;
        }
        SignalMessage::SetChannelStatus {
            channel_id,
            status,
        } => {
            handle_channel_setting(state, peer_id, channel_id, ChannelSetting::Status(status))
                .await;
        }
        SignalMessage::SetChannelPermissions { channel_id, min_role } => {
            let role = match min_role.to_lowercase().as_str() {
                "owner" => shared_types::SpaceRole::Owner,
                "admin" => shared_types::SpaceRole::Admin,
                "moderator" | "mod" => shared_types::SpaceRole::Moderator,
                _ => shared_types::SpaceRole::Member,
            };
            handle_channel_setting(state, peer_id, channel_id, ChannelSetting::MinRole(role)).await;
        }
        SignalMessage::ReorderChannels { channel_ids } => {
            handlers::channel::handle_reorder_channels(state, peer_id, channel_ids).await;
        }
        SignalMessage::SetPrioritySpeaker { peer_id: target_id, enabled } => {
            handle_set_priority_speaker(state, peer_id, target_id, enabled).await;
        }
        SignalMessage::WhisperTo { target_peer_ids } => {
            handle_whisper_to(state, peer_id, target_peer_ids).await;
        }
        SignalMessage::WhisperStopped => {
            handle_whisper_stopped(state, peer_id).await;
        }
        SignalMessage::TimeoutMember {
            member_id,
            duration_secs,
        } => {
            handle_timeout_member(state, peer_id, member_id, duration_secs, db).await;
        }
        // v0.8.0: Block/Unblock
        SignalMessage::BlockUser { user_id } => {
            handlers::moderation::handle_block_user(state, peer_id, user_id, db).await;
        }
        SignalMessage::UnblockUser { user_id } => {
            handlers::moderation::handle_unblock_user(state, peer_id, user_id, db).await;
        }
        // v0.8.0: Ban management
        SignalMessage::UnbanMember { user_id } => {
            handlers::moderation::handle_unban_member(state, peer_id, user_id, db).await;
        }
        SignalMessage::ListBans => {
            handlers::moderation::handle_list_bans(state, peer_id, db).await;
        }
        // v0.8.0: Group DMs
        SignalMessage::CreateGroupDM { user_ids, name } => {
            handlers::chat::handle_create_group_dm(state, peer_id, user_ids, name, db).await;
        }
        SignalMessage::SelectGroupDM { group_id } => {
            handlers::chat::handle_select_group_dm(state, peer_id, group_id, db).await;
        }
        SignalMessage::SendGroupMessage { group_id, content, reply_to_message_id } => {
            handlers::chat::handle_send_group_message(state, peer_id, group_id, content, reply_to_message_id, db).await;
        }
        // v0.8.0: Invite settings
        SignalMessage::SetInviteSettings { expires_hours, max_uses } => {
            handlers::space::handle_set_invite_settings(state, peer_id, expires_hours, max_uses, db).await;
        }
        // v0.8.0: Message threads
        SignalMessage::GetThread { channel_id, message_id } => {
            handlers::chat::handle_get_thread(state, peer_id, channel_id, message_id).await;
        }
        // v0.8.0: Nicknames
        SignalMessage::SetNickname { nickname } => {
            handlers::space::handle_set_nickname(state, peer_id, nickname, db).await;
        }
        // v0.8.0: Message forwarding
        SignalMessage::ForwardMessage { source_channel_id, message_id, target_channel_id } => {
            handlers::chat::handle_forward_message(state, peer_id, source_channel_id, message_id, target_channel_id, db).await;
        }
        // v0.8.0: Status presets
        SignalMessage::SetStatusPreset { preset } => {
            handlers::presence::handle_set_status_preset(state, peer_id, preset).await;
        }
        // v0.8.0: Account system
        SignalMessage::CreateAccount { email, password, display_name } => {
            handlers::auth::handle_create_account(state, peer_id, email, password, display_name, db).await;
        }
        SignalMessage::Login { email, password } => {
            handlers::auth::handle_login(state, peer_id, email, password, db).await;
        }
        SignalMessage::Logout => {
            handlers::auth::handle_logout(state, peer_id, db).await;
        }
        SignalMessage::ChangePassword { current_password, new_password } => {
            handlers::auth::handle_change_password(state, peer_id, current_password, new_password, db).await;
        }
        SignalMessage::RevokeAllSessions => {
            handlers::auth::handle_revoke_all_sessions(state, peer_id, db).await;
        }
        other => {
            log::debug!("Unhandled signal from {peer_id}: {:?}", std::mem::discriminant(&other));
        }
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

// Audio/screen frame rate limits now read from LIMITS (env-configurable).

async fn relay_audio(state: &State, metrics: &Metrics, sender_id: &str, data: &[u8]) {
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

        // Server-enforced mute: drop audio from muted peers
        if peer.is_muted.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }

        // #5: Audio frame rate limiting (lock-free)
        if !atomic_rate_check(&peer.audio_rate_window_ms, &peer.audio_frame_count, LIMITS.max_audio_fps) {
            return;
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

    // Collect room peers + apply whisper + block filtering in a single state read
    let others = {
        let s = state.read().await;
        // Get sender's persistent user_id for block checks (lock-free read)
        let sender_user_id: Option<String> = s
            .peers
            .get(sender_id)
            .and_then(|p| p.user_id.try_lock().ok().and_then(|uid| uid.clone()));

        let room_peers: Vec<Arc<Peer>> = s
            .rooms
            .get(&room_code)
            .map(|r| {
                r.peer_ids
                    .iter()
                    .filter(|pid| pid.as_str() != sender_id)
                    .filter_map(|pid| s.peers.get(pid).cloned())
                    // Block filtering: skip recipients who have blocked the sender
                    .filter(|peer| {
                        if let Some(ref uid) = sender_user_id {
                            !peer.blocked_by.read().map(|b| b.contains(uid)).unwrap_or(false)
                        } else {
                            true // No user_id = guest, can't be blocked
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Whisper filtering: read lock-free, no allocation when empty
        let whisper = s
            .peers
            .get(sender_id)
            .and_then(|p| {
                p.whisper_targets
                    .read()
                    .ok()
                    .filter(|t| !t.is_empty())
                    .map(|t| t.clone())
            });
        match whisper {
            Some(targets) => room_peers
                .into_iter()
                .filter(|p| targets.iter().any(|t| t == &p.id))
                .collect(),
            None => room_peers,
        }
    };
    metrics
        .audio_frames_in_total
        .fetch_add(1, Ordering::Relaxed);
    metrics
        .audio_frames_out_total
        .fetch_add(others.len() as u64, Ordering::Relaxed);

    // Send with timeout to prevent slow peers from blocking the relay.
    // If a peer can't accept within 500ms, drop the frame for them.
    let send_timeout = std::time::Duration::from_millis(500);

    // Single-peer fast path (common case): avoid Arc overhead
    if others.len() == 1 {
        let peer_id_dbg = others[0].id.clone();
        let fut = async {
            let mut tx = others[0].tx.lock().await;
            if let Err(e) = tx.send(Message::Binary(frame.into())).await {
                log::debug!("Audio frame send failed for peer {peer_id_dbg}: {e}");
            }
        };
        let _ = tokio::time::timeout(send_timeout, fut).await;
        return;
    }

    // Multi-peer path: clone frame per peer (Arc overhead not worth it for small frames)
    let futs: Vec<_> = others
        .into_iter()
        .map(|peer| {
            let frame_copy = frame.clone();
            let timeout_dur = send_timeout;
            let peer_id_dbg = peer.id.clone();
            async move {
                let fut = async {
                    let mut tx = peer.tx.lock().await;
                    if let Err(e) = tx.send(Message::Binary(frame_copy.into())).await {
                        log::debug!("Audio frame send failed for peer {peer_id_dbg}: {e}");
                    }
                };
                let _ = tokio::time::timeout(timeout_dur, fut).await;
            }
        })
        .collect();
    futures_util::future::join_all(futs).await;
}

async fn relay_screen(state: &State, metrics: &Metrics, sender_id: &str, data: &[u8]) {
    // Bounds-check sender_id length to prevent u8 overflow in frame header
    if sender_id.len() > 255 {
        log::warn!("Screen relay: sender_id too long ({} bytes), dropping frame", sender_id.len());
        return;
    }

    // Validate sender_id is valid UTF-8 (it comes from &str so it is, but guard
    // against future refactors that might pass raw bytes)
    if std::str::from_utf8(sender_id.as_bytes()).is_err() {
        log::warn!("Screen relay: sender_id is not valid UTF-8, dropping frame");
        return;
    }

    if data.len() > shared_types::MAX_SCREEN_FRAME_SIZE {
        // Send error back to sender instead of silently dropping
        let s = state.read().await;
        if let Some(peer) = s.peers.get(sender_id) {
            let msg = SignalMessage::Error {
                message: "Screen share frame too large, reduce quality".into(),
            };
            send_to(peer, &msg).await;
        }
        return;
    }

    let (room_code, allowed) = {
        let s = state.read().await;
        let peer = match s.peers.get(sender_id) {
            Some(p) => p.clone(),
            None => return,
        };

        if !atomic_rate_check(&peer.screen_rate_window_ms, &peer.screen_frame_count, LIMITS.max_screen_fps) {
            return;
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

    let others = {
        let all = handlers::collect_room_others(state, &room_code, sender_id).await;
        // Block filtering for screen share
        let sender_user_id: Option<String> = {
            let s = state.read().await;
            s.peers.get(sender_id)
                .and_then(|p| p.user_id.try_lock().ok().and_then(|uid| uid.clone()))
        };
        if let Some(ref uid) = sender_user_id {
            all.into_iter()
                .filter(|p| !p.blocked_by.read().map(|b| b.contains(uid)).unwrap_or(false))
                .collect()
        } else {
            all
        }
    };
    if others.is_empty() {
        metrics
            .screen_frames_in_total
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    metrics
        .screen_frames_in_total
        .fetch_add(1, Ordering::Relaxed);
    metrics
        .screen_frames_out_total
        .fetch_add(others.len() as u64, Ordering::Relaxed);

    let send_timeout = std::time::Duration::from_millis(300);

    // Single-peer fast path: avoid clone overhead
    if others.len() == 1 {
        let peer_id_dbg = others[0].id.clone();
        let fut = async {
            let mut tx = others[0].tx.lock().await;
            if let Err(e) = tx.send(Message::Binary(frame.into())).await {
                log::debug!("Screen frame send failed for peer {peer_id_dbg}: {e}");
            }
        };
        let _ = tokio::time::timeout(send_timeout, fut).await;
        return;
    }

    let futs: Vec<_> = others
        .into_iter()
        .map(|peer| {
            let frame_copy = frame.clone();
            let peer_id_dbg = peer.id.clone();
            async move {
                let fut = async {
                    let mut tx = peer.tx.lock().await;
                    if let Err(e) = tx.send(Message::Binary(frame_copy.into())).await {
                        log::debug!("Screen frame send failed for peer {peer_id_dbg}: {e}");
                    }
                };
                let _ = tokio::time::timeout(send_timeout, fut).await;
            }
        })
        .collect();
    futures_util::future::join_all(futs).await;
}

// ─── UDP Transport ───

/// Handle a RequestUdp signal: generate a session token and reply with UdpReady.
async fn handle_request_udp(state: &State, peer_id: &str) {
    let udp_port = UDP_PORT.load(Ordering::Relaxed);
    if udp_port == 0 {
        // UDP not enabled on this server
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id).cloned() {
            drop(s);
            send_to(&peer, &SignalMessage::UdpUnavailable).await;
        }
        return;
    }

    // Generate 8 random bytes as session token
    let mut token_bytes = [0u8; 8];
    OsRng.fill_bytes(&mut token_bytes);
    let token_hex = hex_encode(&token_bytes);

    {
        let mut s = state.write().await;
        // Remove any existing token for this peer
        s.udp_sessions.retain(|_, pid| pid != peer_id);
        s.udp_sessions.insert(token_bytes, peer_id.to_string());
    }

    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(
            &peer,
            &SignalMessage::UdpReady {
                token: token_hex,
                port: udp_port,
            },
        )
        .await;
        log::info!("UDP session created for peer {peer_id} on port {udp_port}");
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Run the UDP audio relay socket. Receives UDP packets, maps session tokens to peers,
/// and relays audio to room peers via UDP.
async fn run_udp_relay(state: State, metrics: Metrics, udp_socket: Arc<UdpSocket>) {
    log::info!("UDP audio relay started on {:?}", udp_socket.local_addr());

    // Pre-allocate receive buffer (max: token(8) + type(1) + audio(4096) = 4105)
    let mut buf = vec![0u8; 8 + 1 + shared_types::MAX_AUDIO_FRAME_SIZE];

    loop {
        let (len, src_addr) = match udp_socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => {
                log::warn!("UDP recv error: {e}");
                continue;
            }
        };

        // Minimum packet: 8 (token) + 1 (type byte)
        if len < 9 {
            continue;
        }

        let token: [u8; 8] = match buf[..8].try_into() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let packet_type = buf[8];

        // Look up peer by token
        let peer_id = {
            let s = state.read().await;
            match s.udp_sessions.get(&token) {
                Some(pid) => pid.clone(),
                None => continue, // Unknown token, silently drop
            }
        };

        // Register/update the peer's UDP address on first packet (or address change)
        {
            let s = state.read().await;
            if let Some(peer) = s.peers.get(&peer_id) {
                let current = peer.udp_addr.read().map(|a| *a).unwrap_or(None);
                if current != Some(src_addr) {
                    log::info!("UDP peer {peer_id} registered at {src_addr}");
                    if let Ok(mut addr) = peer.udp_addr.write() {
                        *addr = Some(src_addr);
                    }
                }
            }
        }

        // Keepalive: just refreshes the address mapping above, no relay needed
        if packet_type == shared_types::UDP_KEEPALIVE {
            continue;
        }

        if packet_type == shared_types::MEDIA_PACKET_AUDIO && len >= 10 {
            metrics.udp_frames_in_total.fetch_add(1, Ordering::Relaxed);
            let audio_data = &buf[9..len];
            relay_audio_udp(&state, &metrics, &peer_id, audio_data, &udp_socket).await;
        }
    }
}

/// Relay audio received via UDP to room peers, preferring UDP delivery.
/// Falls back to WebSocket for peers without a registered UDP address.
async fn relay_audio_udp(
    state: &State,
    metrics: &Metrics,
    sender_id: &str,
    data: &[u8],
    udp_socket: &UdpSocket,
) {
    if data.len() > shared_types::MAX_AUDIO_FRAME_SIZE {
        return;
    }

    let room_code = {
        let s = state.read().await;
        let peer = match s.peers.get(sender_id) {
            Some(p) => p.clone(),
            None => return,
        };

        // Server-enforced mute: drop audio from muted peers
        if peer.is_muted.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }

        if !atomic_rate_check(
            &peer.audio_rate_window_ms,
            &peer.audio_frame_count,
            LIMITS.max_audio_fps,
        ) {
            return;
        }

        peer.cached_room_code()
    };

    let room_code = match room_code {
        Some(c) => c,
        None => return,
    };

    // Build frame for both UDP and WS delivery:
    // [MEDIA_PACKET_AUDIO, id_len, sender_id_bytes, audio_data]
    let mut frame = Vec::with_capacity(2 + sender_id.len() + data.len());
    frame.push(shared_types::MEDIA_PACKET_AUDIO);
    frame.push(sender_id.len() as u8);
    frame.extend_from_slice(sender_id.as_bytes());
    frame.extend_from_slice(data);

    // Collect room peers + apply whisper + block filtering in a single state read
    let others = {
        let s = state.read().await;
        let sender_user_id: Option<String> = s
            .peers
            .get(sender_id)
            .and_then(|p| p.user_id.try_lock().ok().and_then(|uid| uid.clone()));

        let room_peers: Vec<Arc<Peer>> = s
            .rooms
            .get(&room_code)
            .map(|r| {
                r.peer_ids
                    .iter()
                    .filter(|pid| pid.as_str() != sender_id)
                    .filter_map(|pid| s.peers.get(pid).cloned())
                    .filter(|peer| {
                        if let Some(ref uid) = sender_user_id {
                            !peer.blocked_by.read().map(|b| b.contains(uid)).unwrap_or(false)
                        } else {
                            true
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let whisper = s
            .peers
            .get(sender_id)
            .and_then(|p| {
                p.whisper_targets
                    .read()
                    .ok()
                    .filter(|t| !t.is_empty())
                    .map(|t| t.clone())
            });
        match whisper {
            Some(targets) => room_peers
                .into_iter()
                .filter(|p| targets.iter().any(|t| t == &p.id))
                .collect(),
            None => room_peers,
        }
    };
    metrics
        .audio_frames_in_total
        .fetch_add(1, Ordering::Relaxed);
    metrics
        .audio_frames_out_total
        .fetch_add(others.len() as u64, Ordering::Relaxed);

    for peer in &others {
        let udp_addr = peer.udp_addr.read().ok().and_then(|a| *a);
        if let Some(addr) = udp_addr {
            // Send via UDP — fire-and-forget (UDP is unreliable by design)
            let _ = udp_socket.send_to(&frame, addr).await;
            metrics.udp_frames_out_total.fetch_add(1, Ordering::Relaxed);
        } else {
            // Fallback: send via WebSocket
            let frame_clone = frame.clone();
            let send_timeout = std::time::Duration::from_millis(500);
            let fut = async {
                let mut tx = peer.tx.lock().await;
                let _ = tx.send(Message::Binary(frame_clone.into())).await;
            };
            let _ = tokio::time::timeout(send_timeout, fut).await;
        }
    }
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
                if room.peer_ids.is_empty() && !code.starts_with("sp:") {
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

        // For space channels, broadcast MemberChannelChanged so space members
        // see the peer left the voice channel (peer counts update correctly)
        if code.starts_with("sp:") {
            if let Some(peer) = state.read().await.peers.get(peer_id) {
                peer.set_room_code(None).await;
                if let Some(sid) = peer.space_id.lock().await.as_ref() {
                    let notify = SignalMessage::MemberChannelChanged {
                        member_id: peer_id.to_string(),
                        channel_id: None,
                        channel_name: None,
                    };
                    handlers::broadcast_to_space(state, sid, peer_id, &notify).await;
                }
            }
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
        // Clear whisper targets so stale whispers don't persist
        if let Ok(mut wt) = peer.whisper_targets.write() {
            wt.clear();
        }
    }
}

async fn send_to(peer: &Peer, msg: &SignalMessage) {
    if let Ok(json) = serde_json::to_string(msg) {
        let mut tx = peer.tx.lock().await;
        if let Err(e) = tx.send(Message::Text(json.into())).await {
            log::debug!("Signaling send failed for peer {}: {e}", peer.id);
        }
    }
}

async fn handle_set_user_status(state: &State, peer_id: &str, status: String, db: &Db) {
    let status = status.chars().take(128).collect::<String>();

    let (space_id, user_id) = {
        let s = state.read().await;
        let Some(peer) = s.peers.get(peer_id) else {
            return;
        };
        *peer.status.lock().await = status.clone();
        let space_id = peer.space_id.lock().await.clone();
        let user_id = peer.user_id.lock().await.clone();
        (space_id, user_id)
    };

    // Persist status to DB if authenticated
    if let (Some(db), Some(uid)) = (db, user_id) {
        let db = db.clone();
        let status_clone = status.clone();
        match tokio::time::timeout(DB_TIMEOUT, tokio::task::spawn_blocking(move || {
            db.set_user_status(&uid, &status_clone);
        })).await {
            Err(_) => log::warn!("DB timeout: set_user_status for peer {peer_id}"),
            Ok(Err(e)) => log::warn!("DB task panicked in set_user_status: {e}"),
            Ok(Ok(())) => {}
        }
    }

    // Broadcast to space members
    if let Some(space_id) = space_id {
        let notify = SignalMessage::UserStatusChanged {
            member_id: peer_id.to_string(),
            status,
        };
        handlers::broadcast_to_space(state, &space_id, peer_id, &notify).await;
    }
}

async fn handle_set_profile(state: &State, peer_id: &str, bio: String, db: &Db) {
    let bio = bio.chars().take(256).collect::<String>();

    let (space_id, user_id) = {
        let s = state.read().await;
        let Some(peer) = s.peers.get(peer_id) else {
            return;
        };
        let space_id = peer.space_id.lock().await.clone();
        let user_id = peer.user_id.lock().await.clone();
        (space_id, user_id)
    };

    // Persist bio to DB
    if let (Some(db), Some(uid)) = (db, &user_id) {
        let db = db.clone();
        let uid = uid.clone();
        let bio_clone = bio.clone();
        match tokio::time::timeout(DB_TIMEOUT, tokio::task::spawn_blocking(move || {
            db.set_user_bio(&uid, &bio_clone);
        })).await {
            Err(_) => log::warn!("DB timeout: set_user_bio for peer {peer_id}"),
            Ok(Err(e)) => log::warn!("DB task panicked in set_user_bio: {e}"),
            Ok(Ok(())) => {}
        }
    }

    // Broadcast to space members
    if let Some(space_id) = space_id {
        let user_id_str = user_id.unwrap_or_else(|| peer_id.to_string());
        let notify = SignalMessage::ProfileUpdated {
            user_id: user_id_str,
            bio,
        };
        handlers::broadcast_to_space(state, &space_id, peer_id, &notify).await;
    }
}

async fn handle_set_channel_topic(
    state: &State,
    peer_id: &str,
    channel_id: String,
    topic: String,
    db: &Db,
) {
    let topic = topic.chars().take(256).collect::<String>();
    let Some((space_id, actor_user_id, actor_role)) =
        handlers::space::peer_space_role(state, peer_id).await
    else {
        return;
    };
    if !handlers::space::can_manage_channels(actor_role) {
        send_error(state, peer_id, "Only admins can change channel topics").await;
        return;
    }
    let actor_name = {
        let peer = {
            let s = state.read().await;
            s.peers.get(peer_id).cloned()
        };
        if let Some(peer) = peer {
            peer.name.lock().await.clone()
        } else {
            "Unknown".into()
        }
    };
    let changed_channel_name = {
        let mut s = state.write().await;
        let Some(space) = s.spaces.get_mut(&space_id) else {
            return;
        };
        let Some(channel) = space.channels.iter_mut().find(|c| c.id == channel_id) else {
            return;
        };
        channel.topic = topic.clone();
        channel.name.clone()
    };

    // Persist to DB
    if let Some(db) = db {
        let db = db.clone();
        let cid = channel_id.clone();
        let t = topic.clone();
        match tokio::time::timeout(DB_TIMEOUT, tokio::task::spawn_blocking(move || {
            db.set_channel_topic(&cid, &t);
        })).await {
            Err(_) => log::warn!("DB timeout: set_channel_topic for channel {channel_id}"),
            Ok(Err(e)) => log::warn!("DB task panicked in set_channel_topic: {e}"),
            Ok(Ok(())) => {}
        }
    }

    let notify = SignalMessage::ChannelTopicChanged {
        channel_id: channel_id.clone(),
        topic: topic.clone(),
    };
    handlers::broadcast_to_space(state, &space_id, peer_id, &notify).await;

    // Also send to the setter
    if let Some(peer) = state.read().await.peers.get(peer_id) {
        send_to(peer, &notify).await;
    }

    let _ = handlers::space::append_audit_entry(
        state,
        db,
        &space_id,
        &actor_user_id,
        &actor_name,
        "topic",
        None,
        Some(changed_channel_name),
        "Updated the channel topic".into(),
    )
    .await;
}

// ─── Channel Settings ───

enum ChannelSetting {
    UserLimit(u32),
    SlowMode(u32),
    Category(String),
    Status(String),
    MinRole(shared_types::SpaceRole),
}

async fn handle_channel_setting(
    state: &State,
    peer_id: &str,
    channel_id: String,
    setting: ChannelSetting,
) {
    let Some((space_id, _, actor_role)) = handlers::space::peer_space_role(state, peer_id).await
    else {
        return;
    };
    if !handlers::space::can_manage_channels(actor_role) {
        send_error(state, peer_id, "Only admins can change channel settings").await;
        return;
    }

    let notify = {
        let mut s = state.write().await;
        let Some(space) = s.spaces.get_mut(&space_id) else {
            return;
        };
        let Some(channel) = space.channels.iter_mut().find(|c| c.id == channel_id) else {
            return;
        };
        match setting {
            ChannelSetting::UserLimit(limit) => {
                channel.user_limit = limit;
                SignalMessage::ChannelUserLimitChanged {
                    channel_id: channel_id.clone(),
                    user_limit: limit,
                }
            }
            ChannelSetting::SlowMode(secs) => {
                channel.slow_mode_secs = secs;
                SignalMessage::ChannelSlowModeChanged {
                    channel_id: channel_id.clone(),
                    slow_mode_secs: secs,
                }
            }
            ChannelSetting::Category(ref cat) => {
                channel.category = cat.chars().take(32).collect();
                SignalMessage::ChannelCategoryChanged {
                    channel_id: channel_id.clone(),
                    category: channel.category.clone(),
                }
            }
            ChannelSetting::Status(ref status) => {
                channel.status = status.chars().take(64).collect();
                SignalMessage::ChannelStatusChanged {
                    channel_id: channel_id.clone(),
                    status: channel.status.clone(),
                }
            }
            ChannelSetting::MinRole(role) => {
                channel.min_role = role;
                let role_str = match role {
                    shared_types::SpaceRole::Owner => "owner",
                    shared_types::SpaceRole::Admin => "admin",
                    shared_types::SpaceRole::Moderator => "moderator",
                    shared_types::SpaceRole::Member => "member",
                };
                SignalMessage::ChannelPermissionsChanged {
                    channel_id: channel_id.clone(),
                    min_role: role_str.to_string(),
                }
            }
        }
    };

    // Broadcast to all space members including self
    let s = state.read().await;
    if let Some(space) = s.spaces.get(&space_id) {
        let members: Vec<Arc<Peer>> = space
            .member_ids
            .iter()
            .filter_map(|id| s.peers.get(id).cloned())
            .collect();
        drop(s);
        for peer in members {
            send_to(&peer, &notify).await;
        }
    }
}

// ─── Priority Speaker ───

async fn handle_set_priority_speaker(
    state: &State,
    peer_id: &str,
    target_id: String,
    enabled: bool,
) {
    // Only admins+ can set priority speaker, or self
    let room_code = {
        let s = state.read().await;
        let Some(peer) = s.peers.get(peer_id) else {
            return;
        };
        peer.cached_room_code()
    };
    let Some(room_code) = room_code else { return };

    // Set the flag on target peer
    {
        let s = state.read().await;
        if let Some(target) = s.peers.get(&target_id) {
            target
                .is_priority_speaker
                .store(enabled, Ordering::Relaxed);
        }
    }

    let notify = SignalMessage::PrioritySpeakerChanged {
        peer_id: target_id,
        enabled,
    };
    // Broadcast to all in room
    let s = state.read().await;
    if let Some(room) = s.rooms.get(&room_code) {
        let peers: Vec<Arc<Peer>> = room
            .peer_ids
            .iter()
            .filter_map(|pid| s.peers.get(pid).cloned())
            .collect();
        drop(s);
        for peer in peers {
            send_to(&peer, &notify).await;
        }
    }
}

// ─── Whisper ───

async fn handle_whisper_to(state: &State, peer_id: &str, target_peer_ids: Vec<String>) {
    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id) {
        if let Ok(mut wt) = peer.whisper_targets.write() {
            *wt = target_peer_ids;
        }
    }
}

async fn handle_whisper_stopped(state: &State, peer_id: &str) {
    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id) {
        if let Ok(mut wt) = peer.whisper_targets.write() {
            wt.clear();
        }
    }
}

// ─── Timeout ───

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn handle_timeout_member(
    state: &State,
    peer_id: &str,
    member_id: String,
    duration_secs: u64,
    db: &Db,
) {
    let Some((space_id, actor_user_id, actor_role)) =
        handlers::space::peer_space_role(state, peer_id).await
    else {
        return;
    };
    if !handlers::space::can_manage_members(actor_role) {
        send_error(state, peer_id, "Insufficient permissions to timeout members").await;
        return;
    }

    // Cap duration at 28 days
    let duration_secs = duration_secs.min(28 * 24 * 3600);
    let until_epoch = now_epoch_secs() + duration_secs;

    let (target_peer, actor_name) = {
        let s = state.read().await;
        let target = s.peers.get(&member_id).cloned();
        let actor_name = if let Some(p) = s.peers.get(peer_id) {
            p.name.lock().await.clone()
        } else {
            "Unknown".into()
        };
        (target, actor_name)
    };

    if let Some(ref target) = target_peer {
        target.timeout_until.store(until_epoch, Ordering::Relaxed);
    }

    let target_name = if let Some(ref target) = target_peer {
        target.name.lock().await.clone()
    } else {
        member_id.clone()
    };

    // Broadcast timeout to space
    let notify = SignalMessage::MemberTimedOut {
        member_id: member_id.clone(),
        until_epoch,
    };
    handlers::broadcast_to_space(state, &space_id, "", &notify).await;

    let duration_str = if duration_secs >= 3600 {
        format!("{}h", duration_secs / 3600)
    } else if duration_secs >= 60 {
        format!("{}m", duration_secs / 60)
    } else {
        format!("{}s", duration_secs)
    };

    let _ = handlers::space::append_audit_entry(
        state,
        db,
        &space_id,
        &actor_user_id,
        &actor_name,
        "timeout",
        Some(member_id),
        Some(target_name),
        format!("Timed out for {duration_str}"),
    )
    .await;
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

    #[test]
    fn bind_requires_tls_for_non_loopback() {
        assert!(!bind_requires_tls("127.0.0.1:9090"));
        assert!(!bind_requires_tls("[::1]:9090"));
        assert!(bind_requires_tls("0.0.0.0:9090"));
        assert!(bind_requires_tls("192.168.1.10:9090"));
    }
}

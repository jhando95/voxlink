mod handlers;
pub mod persistence;
mod types;
mod tls;
mod metrics_server;
mod discovery;
mod validation;
mod relay;
mod connection;
mod dispatch;

pub(crate) use types::{
    max_channel_messages, ChannelMeta, Db, Peer, Room, ServerState, Space, State,
    MAX_SPACE_AUDIT_ENTRIES,
};
pub(crate) use tls::{bind_requires_tls, allow_insecure_public_bind, load_tls_config, ServerStream};
pub(crate) use metrics_server::{ServerMetrics, run_metrics_server};
pub(crate) use validation::{
    validate_name, validate_password, validate_room_code, now_epoch_secs,
};
#[allow(unused_imports)]
pub(crate) use validation::{MAX_NAME_LEN, MAX_PASSWORD_LEN};
pub(crate) use relay::udp::{run_udp_relay, handle_request_udp};
pub(crate) use connection::{handle_connection, handle_disconnect, send_to, send_error, decrement_ip};
pub(crate) use dispatch::handle_signal;

use shared_types::SignalMessage;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::RwLock;

// ─── Limits ───

pub(crate) const DB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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
static UDP_SOCKET: std::sync::OnceLock<Arc<UdpSocket>> = std::sync::OnceLock::new();

static LIMITS: std::sync::LazyLock<ServerLimits> = std::sync::LazyLock::new(|| ServerLimits {
    max_room_peers: env_or("VOXLINK_MAX_ROOM_PEERS", 10),
    max_connections_per_ip: env_or("VOXLINK_MAX_CONNECTIONS_PER_IP", 20),
    max_channel_messages: env_or("VOXLINK_MAX_CHANNEL_MESSAGES", 100),
    max_audio_fps: env_or("VOXLINK_MAX_AUDIO_FPS", 100),
    max_screen_fps: env_or("VOXLINK_MAX_SCREEN_FPS", 60),
    rate_limit_per_sec: env_or("VOXLINK_RATE_LIMIT_PER_SEC", 100),
});


// Types are in types.rs, re-exported via `pub(crate) use types::*` above.
type Metrics = Arc<ServerMetrics>;




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
            log::error!(
                "Failed to bind to {addr}: {e}. Is another server already running on this port?"
            );
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
                        min_role: match cr.min_role.as_deref() {
                            Some("owner") => shared_types::SpaceRole::Owner,
                            Some("admin") => shared_types::SpaceRole::Admin,
                            Some("moderator") => shared_types::SpaceRole::Moderator,
                            _ => shared_types::SpaceRole::Member,
                        },
                        position: cr.position.unwrap_or(0),
                        auto_delete_hours: cr.auto_delete_hours.unwrap_or(0),
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
                        if let Ok(msgs) =
                            db.load_messages_for_channel(&cr.id, max_channel_messages())
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
                                    attachment_name: None,
                                    attachment_size: None,
                                    link_url: m.link_url,
                                })
                                .collect();
                            if !dq.is_empty() {
                                text_messages.insert(cr.id.clone(), dq);
                            }
                        }
                    }
                }

                let mut role_colors: HashMap<String, String> = HashMap::new();
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
                        if !row.role_color.is_empty() {
                            role_colors
                                .entry(row.role.clone())
                                .or_insert(row.role_color);
                        }
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
                let is_public = db.is_space_public(&sr.id).unwrap_or(false);
                s.spaces.insert(
                    sr.id.clone(),
                    Space {
                        id: sr.id.clone(),
                        name: sr.name.clone(),
                        description: String::new(),
                        invite_code: sr.invite_code.clone(),
                        owner_id: sr.owner_id.clone(),
                        channels,
                        member_ids: Vec::new(),
                        member_roles,
                        role_colors,
                        text_messages,
                        audit_log,
                        slow_mode_timestamps: HashMap::new(),
                        created_at: Instant::now(),
                        is_public,
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
    tokio::spawn(discovery::run_discovery(discover_addr));

    // Start UDP media relay (port = WS port + 1, or PV_UDP_PORT env var)
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
                log::info!("UDP media relay listening on {udp_addr_str}");
                let udp_socket = Arc::new(socket);
                let _ = UDP_SOCKET.set(udp_socket.clone());
                tokio::spawn(run_udp_relay(state.clone(), metrics.clone(), udp_socket));
            }
            Err(e) => {
                log::warn!("UDP media relay unavailable (failed to bind {udp_addr_str}): {e}");
                // Server still works — all media goes through WebSocket
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

                // Remove stale connections_per_ip entries where count has reached 0
                let ip_before = s.connections_per_ip.len();
                s.connections_per_ip.retain(|_, count| *count > 0);
                let ip_removed = ip_before - s.connections_per_ip.len();
                if ip_removed > 0 {
                    log::debug!("Cleaned up {ip_removed} stale connections_per_ip entries");
                }
            }
        });
    }

    // Periodic auto-delete cleanup (every 10 minutes)
    if let Some(ref db) = db {
        let db = db.clone();
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(600));
            loop {
                interval.tick().await;
                let db2 = db.clone();
                // Delete expired messages from DB
                let db_deleted =
                    tokio::task::spawn_blocking(move || db2.delete_expired_messages().unwrap_or(0))
                        .await
                        .unwrap_or(0);
                if db_deleted > 0 {
                    log::info!("Auto-delete: removed {db_deleted} expired message(s) from DB");
                }
                // Also purge from in-memory text_messages
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let mut s = state.write().await;
                for space in s.spaces.values_mut() {
                    for ch in &space.channels {
                        if ch.auto_delete_hours > 0 {
                            let cutoff = now.saturating_sub(ch.auto_delete_hours as u64 * 3600);
                            if let Some(msgs) = space.text_messages.get_mut(&ch.id) {
                                let before = msgs.len();
                                msgs.retain(|m| m.timestamp >= cutoff);
                                let removed = before - msgs.len();
                                if removed > 0 {
                                    log::debug!(
                                        "Auto-delete: purged {removed} in-memory message(s) from channel {}",
                                        ch.id
                                    );
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    // Periodic scheduled message delivery (every 30 seconds)
    if let Some(ref db) = db {
        let db = db.clone();
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                let db_clone = db.clone();
                let due = tokio::task::spawn_blocking(move || {
                    db_clone.get_due_scheduled_messages().unwrap_or_default()
                })
                .await
                .unwrap_or_default();
                for (sched_id, space_id, channel_id, sender_id, sender_name, content) in due {
                    let msg_id = {
                        let mut s = state.write().await;
                        s.alloc_message_id()
                    };
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let link_url = shared_types::extract_first_url(&content);
                    let msg_data = shared_types::TextMessageData {
                        message_id: msg_id.clone(),
                        sender_id: sender_id.clone(),
                        sender_name: sender_name.clone(),
                        content: content.clone(),
                        timestamp: now,
                        edited: false,
                        reactions: vec![],
                        reply_to_message_id: None,
                        reply_to_sender_name: None,
                        reply_preview: None,
                        pinned: false,
                        forwarded_from: None,
                        attachment_name: None,
                        attachment_size: None,
                        link_url,
                    };
                    // Store in in-memory buffer and broadcast
                    {
                        let mut s = state.write().await;
                        if let Some(space) = s.spaces.get_mut(&space_id) {
                            let msgs = space
                                .text_messages
                                .entry(channel_id.clone())
                                .or_insert_with(VecDeque::new);
                            msgs.push_back(msg_data.clone());
                            while msgs.len() > max_channel_messages() {
                                msgs.pop_front();
                            }
                        }
                    }
                    // Persist the message
                    {
                        let db_clone = db.clone();
                        let cid = channel_id.clone();
                        let mid = msg_id;
                        let sid = sender_id;
                        let sn = sender_name;
                        let ct = content;
                        let ts = now as i64;
                        tokio::task::spawn_blocking(move || {
                            let _ = db_clone.save_message(&crate::persistence::MessageRow {
                                id: mid,
                                channel_id: cid,
                                sender_id: sid,
                                sender_name: sn,
                                content: ct,
                                timestamp: ts,
                                edited: false,
                                reply_to_message_id: None,
                                reply_to_sender_name: None,
                                reply_preview: None,
                                pinned: false,
                                link_url: None,
                            });
                        });
                    }
                    // Broadcast to space members
                    let notify = SignalMessage::TextMessage {
                        channel_id,
                        message: msg_data,
                    };
                    handlers::broadcast_to_space(&state, &space_id, "", &notify).await;
                    // Delete the scheduled message from DB
                    let db_clone = db.clone();
                    let sid = sched_id;
                    tokio::task::spawn_blocking(move || {
                        let _ = db_clone.delete_scheduled_message(&sid);
                    });
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


// ─── LAN Discovery ───


// ─── DM Voice Call Handlers ───

// ─── Voice Recording ───

async fn handle_start_recording(state: &State, peer_id: &str, channel_id: String) {
    let s = state.read().await;
    let peer = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    let space_id = match peer.space_id.lock().await.clone() {
        Some(id) => id,
        None => return,
    };
    let space = match s.spaces.get(&space_id) {
        Some(sp) => sp,
        None => return,
    };
    let user_id = peer
        .user_id
        .lock()
        .await
        .clone()
        .unwrap_or_else(|| peer_id.to_string());
    let role = handlers::space::role_for_identity(space, &user_id);
    if !role.has_at_least(shared_types::SpaceRole::Admin) {
        drop(s);
        send_error(state, peer_id, "Admin+ required to record").await;
        return;
    }
    let started_by = peer.name.lock().await.clone();
    let room_key = format!("sp:{}:ch:{}", space_id, channel_id);
    let room_peers: Vec<_> = if let Some(room) = s.rooms.get(&room_key) {
        room.peer_ids
            .iter()
            .filter_map(|pid| s.peers.get(pid).cloned())
            .collect()
    } else {
        Vec::new()
    };
    drop(s);
    let msg = SignalMessage::RecordingStarted {
        channel_id,
        started_by,
    };
    for p in &room_peers {
        send_to(p, &msg).await;
    }
}

async fn handle_stop_recording(state: &State, peer_id: &str, channel_id: String) {
    let s = state.read().await;
    let peer = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    let space_id = match peer.space_id.lock().await.clone() {
        Some(id) => id,
        None => return,
    };
    let space = match s.spaces.get(&space_id) {
        Some(sp) => sp,
        None => return,
    };
    let user_id = peer
        .user_id
        .lock()
        .await
        .clone()
        .unwrap_or_else(|| peer_id.to_string());
    let role = handlers::space::role_for_identity(space, &user_id);
    // Only Moderator+ can stop recording (recording initiators are always Admin+)
    if !role.has_at_least(shared_types::SpaceRole::Moderator) {
        drop(s);
        send_error(state, peer_id, "Moderator+ required to stop recording").await;
        return;
    }
    let room_key = format!("sp:{}:ch:{}", space_id, channel_id);
    let room_peers: Vec<_> = if let Some(room) = s.rooms.get(&room_key) {
        room.peer_ids
            .iter()
            .filter_map(|pid| s.peers.get(pid).cloned())
            .collect()
    } else {
        Vec::new()
    };
    drop(s);
    let msg = SignalMessage::RecordingStopped { channel_id };
    for p in &room_peers {
        send_to(p, &msg).await;
    }
}

// ─── Account Management ───

async fn handle_set_display_name(state: &State, peer_id: &str, name: String, db: &Db) {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() || trimmed.len() > 32 {
        send_error(state, peer_id, "Name must be 1-32 characters").await;
        return;
    }
    let s = state.read().await;
    let peer = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    let user_id = match peer.user_id.lock().await.clone() {
        Some(id) => id,
        None => {
            drop(s);
            send_error(state, peer_id, "Not authenticated").await;
            return;
        }
    };
    drop(s);
    *peer.name.lock().await = trimmed.clone();
    if let Some(db) = db {
        let _ = db.update_display_name(&user_id, &trimmed);
    }
    let s = state.read().await;
    if let Some(p) = s.peers.get(peer_id).cloned() {
        send_to(
            &p,
            &SignalMessage::DisplayNameChanged {
                user_id,
                name: trimmed,
            },
        )
        .await;
    }
}

async fn handle_delete_account(state: &State, peer_id: &str, db: &Db) {
    let s = state.read().await;
    let peer = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    let user_id = match peer.user_id.lock().await.clone() {
        Some(id) => id,
        None => {
            drop(s);
            send_error(state, peer_id, "Not authenticated").await;
            return;
        }
    };
    drop(s);
    if let Some(db) = db {
        let _ = db.delete_user(&user_id);
    }
    let s = state.read().await;
    if let Some(p) = s.peers.get(peer_id).cloned() {
        send_to(&p, &SignalMessage::AccountDeleted).await;
    }
}

// ─── Server Discovery ───

async fn handle_set_space_public(state: &State, peer_id: &str, is_public: bool, db: &Db) {
    let (space_id, _user_id, role, member_ids) = {
        let s = state.read().await;
        let peer = match s.peers.get(peer_id).cloned() {
            Some(p) => p,
            None => return,
        };
        let space_id = match peer.space_id.lock().await.clone() {
            Some(id) => id,
            None => return,
        };
        let space = match s.spaces.get(&space_id) {
            Some(sp) => sp,
            None => return,
        };
        let user_id = peer
            .user_id
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| peer_id.to_string());
        let role = handlers::space::role_for_identity(space, &user_id);
        let member_ids = space.member_ids.clone();
        (space_id, user_id, role, member_ids)
    };

    if !matches!(
        role,
        shared_types::SpaceRole::Owner | shared_types::SpaceRole::Admin
    ) {
        send_error(state, peer_id, "Only admins can change space visibility").await;
        return;
    }
    // Update in-memory state
    {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            space.is_public = is_public;
        }
    }
    if let Some(db) = db {
        let _ = db.set_space_public(&space_id, is_public);
    }
    // Broadcast to space members
    let s = state.read().await;
    for mid in &member_ids {
        for (_, p) in s.peers.iter() {
            let uid = p.user_id.lock().await.clone().unwrap_or_default();
            if uid == *mid {
                send_to(p, &SignalMessage::SpacePublicChanged { is_public }).await;
            }
        }
    }
}

async fn handle_browse_public_spaces(state: &State, peer_id: &str, db: &Db) {
    let mut spaces = Vec::new();
    if let Some(db) = db {
        if let Ok(public_rows) = db.load_public_spaces() {
            let s = state.read().await;
            for (id, name, desc, invite) in public_rows {
                let (member_count, channel_count, online_count) =
                    if let Some(sp) = s.spaces.get(&id) {
                        let online = s
                            .peers
                            .values()
                            .filter(|p| {
                                p.space_id
                                    .try_lock()
                                    .map(|sid| sid.as_deref() == Some(id.as_str()))
                                    .unwrap_or(false)
                            })
                            .count() as u32;
                        (sp.member_ids.len() as u32, sp.channels.len() as u32, online)
                    } else {
                        (0, 0, 0)
                    };
                spaces.push(shared_types::PublicSpaceInfo {
                    id,
                    name,
                    description: desc,
                    invite_code: invite,
                    member_count,
                    channel_count,
                    online_count,
                });
            }
        }
    }
    let s = state.read().await;
    if let Some(p) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(&p, &SignalMessage::PublicSpaceList { spaces }).await;
    }
}

async fn handle_send_voice_note(
    state: &State,
    peer_id: &str,
    channel_id: String,
    duration_secs: u32,
    data: Vec<u8>,
    db: &Db,
) {
    // Voice note = special text message with voice note attachment
    if data.len() > 512_000 {
        // 500KB max
        send_error(state, peer_id, "Voice note too large (max 500KB)").await;
        return;
    }

    let space_id = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => peer.space_id.lock().await.clone(),
            None => None,
        }
    };
    let Some(space_id) = space_id else { return };

    // Check if peer is timed out
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            let until = peer
                .timeout_until
                .load(std::sync::atomic::Ordering::Relaxed);
            if until > 0 && now_epoch_secs() < until {
                let peer = peer.clone();
                drop(s);
                send_to(
                    &peer,
                    &SignalMessage::Error {
                        message: "You are timed out and cannot send messages".into(),
                    },
                )
                .await;
                return;
            }
        }
    }

    // Check channel permissions (min_role)
    {
        let s = state.read().await;
        let min_role = s
            .spaces
            .get(&space_id)
            .and_then(|sp| sp.channels.iter().find(|ch| ch.id == channel_id))
            .map(|ch| ch.min_role)
            .unwrap_or(shared_types::SpaceRole::Member);
        if min_role != shared_types::SpaceRole::Member {
            let user_role = if let Some(peer) = s.peers.get(peer_id) {
                if let Some(uid) = peer.user_id.lock().await.as_deref() {
                    s.spaces
                        .get(&space_id)
                        .and_then(|sp| sp.member_roles.get(uid).copied())
                        .unwrap_or(shared_types::SpaceRole::Member)
                } else {
                    shared_types::SpaceRole::Member
                }
            } else {
                shared_types::SpaceRole::Member
            };
            if !user_role.has_at_least(min_role) {
                if let Some(peer) = s.peers.get(peer_id).cloned() {
                    drop(s);
                    send_to(
                        &peer,
                        &SignalMessage::Error {
                            message: "You don't have permission to use this channel".into(),
                        },
                    )
                    .await;
                }
                return;
            }
        }
    }

    // Slow mode check
    {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            let slow_mode_secs = space
                .channels
                .iter()
                .find(|ch| ch.id == channel_id)
                .map(|ch| ch.slow_mode_secs)
                .unwrap_or(0);
            if slow_mode_secs > 0 {
                let now = now_epoch_secs();
                let key = (channel_id.clone(), peer_id.to_string());
                if let Some(&last) = space.slow_mode_timestamps.get(&key) {
                    if now < last + slow_mode_secs as u64 {
                        let remaining = (last + slow_mode_secs as u64) - now;
                        if let Some(peer) = s.peers.get(peer_id).cloned() {
                            drop(s);
                            send_to(&peer, &SignalMessage::Error {
                                message: format!("Slow mode: wait {remaining}s before sending another message"),
                            }).await;
                        }
                        return;
                    }
                }
                space.slow_mode_timestamps.insert(key, now);
            }
        }
    }

    // Auto-moderation filter check (on the voice note description text)
    let content_text = format!("\u{1F3A4} Voice note ({duration_secs}s)");
    if let Some((matched_word, action)) =
        handlers::moderation::check_automod(db, &space_id, &content_text).await
    {
        if action == "block" {
            send_error(
                state,
                peer_id,
                &format!("Message blocked by auto-moderation (matched: {matched_word})"),
            )
            .await;
            return;
        }
    }

    let s = state.read().await;
    let peer = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    let user_id = peer
        .user_id
        .lock()
        .await
        .clone()
        .unwrap_or_else(|| peer_id.to_string());
    let name = peer.name.lock().await.clone();
    let name = if name.is_empty() {
        "Anonymous".to_string()
    } else {
        name
    };
    drop(s);

    let msg_id = {
        let mut s = state.write().await;
        s.alloc_message_id()
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let msg = shared_types::TextMessageData {
        message_id: msg_id,
        sender_name: name,
        sender_id: user_id.clone(),
        content: content_text,
        timestamp: now,
        reply_to_message_id: None,
        reply_to_sender_name: None,
        reply_preview: None,
        edited: false,
        pinned: false,
        reactions: vec![],
        forwarded_from: None,
        attachment_name: Some(format!("voice_note_{duration_secs}s.opus")),
        attachment_size: Some(data.len() as u32),
        link_url: None,
    };

    // Broadcast to all peers in the same space (they filter by selected channel client-side)
    let s = state.read().await;
    if let Some(space) = s.spaces.get(&space_id) {
        for mid in &space.member_ids {
            for (_, p) in s.peers.iter() {
                let uid = p.user_id.lock().await.clone().unwrap_or_default();
                if uid == *mid {
                    // Block check: skip recipients who have blocked the sender
                    if p.blocked_by
                        .read()
                        .map(|b| b.contains(&user_id))
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    send_to(
                        p,
                        &SignalMessage::TextMessage {
                            channel_id: channel_id.clone(),
                            message: msg.clone(),
                        },
                    )
                    .await;
                }
            }
        }
    }
}

// ─── UDP Transport ───

/// Handle a RequestUdp signal: generate a session token and reply with UdpReady.

// ─── Disconnect ───

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
        match tokio::time::timeout(
            DB_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                db.set_user_status(&uid, &status_clone);
            }),
        )
        .await
        {
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
        match tokio::time::timeout(
            DB_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                db.set_user_bio(&uid, &bio_clone);
            }),
        )
        .await
        {
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
        match tokio::time::timeout(
            DB_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                db.set_channel_topic(&cid, &t);
            }),
        )
        .await
        {
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

pub(crate) enum ChannelSetting {
    UserLimit(u32),
    SlowMode(u32),
    Category(String),
    Status(String),
    MinRole(shared_types::SpaceRole),
    AutoDelete(u32),
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
            ChannelSetting::AutoDelete(hours) => {
                channel.auto_delete_hours = hours;
                SignalMessage::ChannelAutoDeleteChanged {
                    channel_id: channel_id.clone(),
                    auto_delete_hours: hours,
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
    // Only Moderator+ can set priority speaker on others; anyone can set it on themselves
    let is_self = peer_id == target_id;
    if !is_self {
        if let Some((_space_id, _user_id, role)) =
            handlers::space::peer_space_role(state, peer_id).await
        {
            if !role.has_at_least(shared_types::SpaceRole::Moderator) {
                send_error(
                    state,
                    peer_id,
                    "Moderator+ required to set priority speaker on others",
                )
                .await;
                return;
            }
        } else {
            return;
        }
    }

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
            target.is_priority_speaker.store(enabled, Ordering::Relaxed);
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
    // Cap whisper targets to 20 to prevent abuse
    if target_peer_ids.len() > 20 {
        send_error(state, peer_id, "Too many whisper targets (max 20)").await;
        return;
    }
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
        send_error(
            state,
            peer_id,
            "Insufficient permissions to timeout members",
        )
        .await;
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

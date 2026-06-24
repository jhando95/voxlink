use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;
use futures_util::StreamExt;
use shared_types::SignalMessage;
use tokio::sync::{mpsc, Mutex, Notify};
use tokio_tungstenite::tungstenite::Message;
use crate::types::{Peer, State, Db};
use crate::tls::ServerStream;
use crate::metrics_server::ServerMetrics;
use crate::outbound;
use crate::validation::instant_to_ms;
use crate::validation::check_rate_limit;
use crate::relay::audio::relay_audio;
use crate::relay::screen::{relay_screen, relay_screen_chunk};

type Metrics = Arc<ServerMetrics>;

pub(crate) async fn decrement_ip(state: &State, ip: IpAddr) {
    let mut s = state.write().await;
    if let Some(count) = s.connections_per_ip.get_mut(&ip) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            s.connections_per_ip.remove(&ip);
        }
    }
}

pub(crate) async fn handle_connection(
    state: State,
    metrics: Metrics,
    stream: ServerStream,
    addr: SocketAddr,
    db: Db,
) {
    // Cap per-message size (default is 64 MiB). 16 MiB comfortably covers an
    // 8 MiB attachment after base64 (~11 MB) while limiting per-connection memory
    // on small hosts. Covers both plain and TLS connections via ServerStream.
    let ws_config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(16 * 1024 * 1024))
        .max_frame_size(Some(16 * 1024 * 1024));
    let ws = match tokio_tungstenite::accept_async_with_config(stream, Some(ws_config)).await {
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

    // Per-peer outbound lanes. The drain task is the sole owner of the sink;
    // senders never lock anything (see outbound.rs).
    let (signaling_tx, signaling_rx) =
        mpsc::channel(outbound::SIGNALING_LANE_CAPACITY);
    let (media_tx, media_rx) = mpsc::channel(outbound::MEDIA_LANE_CAPACITY);
    let disconnect = Arc::new(Notify::new());

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
                is_server_deafened: AtomicBool::new(false),
                status: Mutex::new(String::new()),
                activity: Mutex::new(String::new()),
                signaling_tx,
                media_tx,
                disconnect: disconnect.clone(),
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
                last_screen_chunk_sequence: AtomicU32::new(0),
                blocked_by: std::sync::RwLock::new(HashSet::new()),
                status_preset: Mutex::new(shared_types::UserStatus::default()),
            }),
        );
        id
    };

    // Spawn the per-peer drain task that owns the WS sink.
    let drain_handle = outbound::spawn_peer_drain(
        peer_id.clone(),
        tx,
        signaling_rx,
        media_rx,
        disconnect.clone(),
        metrics.clone(),
    );

    metrics.active_connections.fetch_add(1, Ordering::Relaxed);

    log::info!("Peer {peer_id} connected from {addr}");

    // Keepalive: send WebSocket pings every 30s to survive NAT/firewall timeouts.
    // Uses the per-peer signaling lane so we never lock a sink directly.
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
                if !outbound::try_send_ping(&peer) {
                    break; // peer torn down
                }
            }
        })
    });

    // Per-connection reusable buffers for audio relay (avoids alloc per frame)
    let mut relay_buf: Vec<u8> = Vec::with_capacity(512);
    let mut room_peers_buf: Vec<Arc<Peer>> = Vec::with_capacity(20);

    loop {
        let msg = tokio::select! {
            biased;
            // Drain task or an overflowing sender asked us to bail.
            _ = disconnect.notified() => break,
            next = rx.next() => match next {
                Some(m) => m,
                None => break,
            },
        };
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
                    metrics
                        .per_message_counters[signal.variant_index()]
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let t0 = Instant::now();
                    crate::handle_signal(&state, &metrics, &peer_id, signal, &db).await;
                    metrics
                        .signaling_dispatch_latency
                        .observe(t0.elapsed().as_secs_f64());
                } else {
                    metrics
                        .malformed_signaling_messages_total
                        .fetch_add(1, Ordering::Relaxed);
                    log::debug!(
                        "Malformed signal from {peer_id}: {}",
                        &text[..text.len().min(200)]
                    );
                }
            }
            Ok(Message::Binary(data)) => {
                if data.is_empty() {
                    continue;
                }
                match data[0] {
                    shared_types::MEDIA_PACKET_AUDIO => {
                        relay_audio(
                            &state,
                            &metrics,
                            &peer_id,
                            &data[1..],
                            &mut relay_buf,
                            &mut room_peers_buf,
                        )
                        .await;
                    }
                    shared_types::MEDIA_PACKET_SCREEN => {
                        relay_screen(&state, &metrics, &peer_id, &data[1..]).await;
                    }
                    shared_types::MEDIA_PACKET_SCREEN_CHUNK => {
                        relay_screen_chunk(&state, &metrics, &peer_id, &data[1..]).await;
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
    // Both Senders drop when the peer is removed; that closes the receivers
    // and lets the drain task observe `else => break` and exit cleanly.
    // Wait briefly so the WebSocket close handshake actually flushes.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), drain_handle).await;
    if let Some(ref user_id) = disconnected_user_id {
        // Persist last-seen timestamp so offline friends see when this user was last online
        if let Some(ref db) = db {
            let uid = user_id.clone();
            let db = db.clone();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            tokio::task::spawn_blocking(move || {
                if let Err(e) = db.update_last_seen(&uid, now) {
                    log::warn!("Failed to update last_seen for {}: {e}", uid);
                }
            });
        }
        crate::handlers::presence::notify_watchers_for_user(&state, user_id).await;
    }
    metrics.active_connections.fetch_sub(1, Ordering::Relaxed);
    log::info!("Peer {peer_id} disconnected");
}

pub(crate) async fn send_error(state: &State, peer_id: &str, message: &str) {
    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(
            &peer,
            &SignalMessage::Error {
                message: message.to_string(),
            },
        );
    }
}

pub(crate) async fn handle_disconnect(state: &State, peer_id: &str) {
    // Use cached room code (lock-free) for disconnect path
    let room_code = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => peer.cached_room_code(),
            None => None,
        }
    };

    if let Some(ref code) = room_code {
        crate::handlers::room::stop_screen_share_in_room(state, code, peer_id).await;
        let remaining = crate::handlers::collect_room_others(state, code, peer_id).await;

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
            send_to(&peer, &notify);
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
                    crate::handlers::broadcast_to_space(state, sid, peer_id, &notify).await;
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
        crate::handlers::chat::clear_typing_for_peer(state, peer_id).await;
        {
            let mut s = state.write().await;
            if let Some(space) = s.spaces.get_mut(sid) {
                space.member_ids.retain(|id| id != peer_id);
            }
        }

        let notify = SignalMessage::MemberOffline {
            member_id: peer_id.to_string(),
        };
        crate::handlers::broadcast_to_space(state, sid, peer_id, &notify).await;

        if let Some(peer) = state.read().await.peers.get(peer_id) {
            *peer.space_id.lock().await = None;
        }
    }

    crate::handlers::chat::clear_direct_typing_for_peer(state, peer_id).await;

    if let Some(peer) = state.read().await.peers.get(peer_id) {
        peer.set_room_code(None).await;
        // Clear whisper targets so stale whispers don't persist
        if let Ok(mut wt) = peer.whisper_targets.write() {
            wt.clear();
        }
    }
}

/// Push a signaling message onto the peer's reliable outbound lane.
///
/// Non-blocking: serialize → `try_send` on the bounded signaling channel. On
/// Full, increment overflow counters and notify the disconnect watcher; the
/// rx loop will then break out and `handle_disconnect` will clean up.
///
/// Returns immediately. All existing callers that previously `.await`ed have
/// been migrated to non-await calls; the sink write is done asynchronously by
/// the per-peer drain task.
pub(crate) fn send_to(peer: &Peer, msg: &SignalMessage) {
    // We don't have access to the metrics struct here; the metrics-aware
    // path lives in outbound::try_send_signaling and is called from
    // contexts that DO have metrics. For ergonomics inside handlers, route
    // through the no-metrics shim: peer.signaling_tx::try_send directly.
    let json = match serde_json::to_string(msg) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Failed to serialize signal for peer {}: {e}", peer.id);
            return;
        }
    };
    match peer
        .signaling_tx
        .try_send(crate::outbound::OutboundFrame::Signaling(json))
    {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            log::warn!(
                "Signaling lane full for peer {}; triggering disconnect",
                peer.id
            );
            peer.disconnect.notify_one();
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            // Peer is already torn down; no-op.
        }
    }
}

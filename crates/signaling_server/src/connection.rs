use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use futures_util::{SinkExt, StreamExt};
use shared_types::SignalMessage;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use crate::types::{Peer, State, Db};
use crate::tls::ServerStream;
use crate::metrics_server::ServerMetrics;
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
                is_server_deafened: AtomicBool::new(false),
                status: Mutex::new(String::new()),
                activity: Mutex::new(String::new()),
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
                last_screen_chunk_sequence: AtomicU32::new(0),
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

    // Per-connection reusable buffers for audio relay (avoids alloc per frame)
    let mut relay_buf: Vec<u8> = Vec::with_capacity(512);
    let mut room_peers_buf: Vec<Arc<Peer>> = Vec::with_capacity(20);

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
                    crate::handle_signal(&state, &metrics, &peer_id, signal, &db).await;
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
        )
        .await;
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

pub(crate) async fn send_to(peer: &Peer, msg: &SignalMessage) {
    if let Ok(json) = serde_json::to_string(msg) {
        let mut tx = peer.tx.lock().await;
        if let Err(e) = tx.send(Message::Text(json.into())).await {
            log::debug!("Signaling send failed for peer {}: {e}", peer.id);
        }
    }
}

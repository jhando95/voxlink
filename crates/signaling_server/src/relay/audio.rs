use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::net::UdpSocket;
use crate::types::{Peer, State};
use crate::validation::atomic_rate_check;
use crate::metrics_server::ServerMetrics;
use crate::LIMITS;
use shared_types;

type Metrics = Arc<ServerMetrics>;

// ─── Audio Relay ───

// Audio/screen frame rate limits now read from LIMITS (env-configurable).

pub(crate) async fn relay_audio(
    state: &State,
    metrics: &Metrics,
    sender_id: &str,
    data: &[u8],
    relay_buf: &mut Vec<u8>,
    room_peers_buf: &mut Vec<Arc<Peer>>,
) {
    // #3: Reject oversized audio frames
    if data.len() > shared_types::MAX_AUDIO_FRAME_SIZE {
        return;
    }

    // Single state read: get room code, sender info, room peers, and whisper targets
    // in one lock acquisition. This is the hottest path (~50 calls/sec per peer).
    let s = state.read().await;

    let peer = match s.peers.get(sender_id) {
        Some(p) => p.clone(),
        None => return,
    };

    // Server-enforced mute: drop audio from muted peers
    if peer.is_muted.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    // Server-enforced timeout: drop audio while the peer is under moderation timeout.
    // Without this, a moderation timeout only suppresses chat sends; voice was still
    // relayed, which made timeouts feel half-applied.
    {
        let now = crate::now_epoch_secs() as u64;
        let until = peer.timeout_until.load(std::sync::atomic::Ordering::Relaxed);
        if until > now {
            return;
        }
    }

    // #5: Audio frame rate limiting (lock-free)
    if !atomic_rate_check(
        &peer.audio_rate_window_ms,
        &peer.audio_frame_count,
        LIMITS.max_audio_fps,
    ) {
        return;
    }

    let room_code = match peer.cached_room_code() {
        Some(c) => c,
        None => return,
    };

    // Build frame into reusable buffer: [kind, id_len, sender_id_bytes, audio_data]
    relay_buf.clear();
    relay_buf.push(shared_types::MEDIA_PACKET_AUDIO);
    relay_buf.push(sender_id.len() as u8);
    relay_buf.extend_from_slice(sender_id.as_bytes());
    relay_buf.extend_from_slice(data);

    // Get sender's persistent user_id for block checks (lock-free read)
    let sender_user_id: Option<String> = s
        .peers
        .get(sender_id)
        .and_then(|p| p.user_id.try_lock().ok().and_then(|uid| uid.clone()));

    // Collect room peers into reusable buffer with block + deafen filtering
    room_peers_buf.clear();
    if let Some(r) = s.rooms.get(&room_code) {
        for pid in &r.peer_ids {
            if pid.as_str() == sender_id {
                continue;
            }
            if let Some(p) = s.peers.get(pid) {
                // Block filtering: skip recipients who have blocked the sender
                if let Some(ref uid) = sender_user_id {
                    if p.blocked_by
                        .read()
                        .map(|b| b.contains(uid))
                        .unwrap_or(false)
                    {
                        continue;
                    }
                }
                // Server-deafen: skip recipients who are server-deafened
                if p.is_server_deafened
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    continue;
                }
                room_peers_buf.push(p.clone());
            }
        }
    }

    // Whisper filtering: read lock-free, no allocation when empty
    let whisper = s.peers.get(sender_id).and_then(|p| {
        p.whisper_targets
            .read()
            .ok()
            .filter(|t| !t.is_empty())
            .map(|t| t.clone())
    });

    // Drop the state read lock before sending frames
    drop(s);

    if let Some(targets) = whisper {
        room_peers_buf.retain(|p| targets.iter().any(|t| t == &p.id));
    }

    metrics
        .audio_frames_in_total
        .fetch_add(1, Ordering::Relaxed);
    metrics
        .audio_frames_out_total
        .fetch_add(room_peers_buf.len() as u64, Ordering::Relaxed);

    // Hand each recipient a cheap refcount-bumped clone of the shared frame
    // and push it onto their per-peer media lane. try_send_media is
    // non-blocking; on Full it drops the frame and bumps the lossy counter.
    let shared: tokio_tungstenite::tungstenite::Bytes =
        tokio_tungstenite::tungstenite::Bytes::copy_from_slice(&relay_buf[..]);
    for peer in room_peers_buf.iter() {
        crate::outbound::try_send_media(peer, shared.clone(), metrics);
    }
}

/// Relay audio received via UDP to room peers, preferring UDP delivery.
/// Falls back to WebSocket for peers without a registered UDP address.
pub(crate) async fn relay_audio_udp(
    state: &State,
    metrics: &Metrics,
    sender_id: &str,
    data: &[u8],
    udp_socket: &UdpSocket,
    relay_buf: &mut Vec<u8>,
    room_peers_buf: &mut Vec<Arc<Peer>>,
) {
    if data.len() > shared_types::MAX_AUDIO_FRAME_SIZE {
        return;
    }

    // Single state read: get room code, sender info, room peers, and whisper targets
    let s = state.read().await;

    let peer = match s.peers.get(sender_id) {
        Some(p) => p.clone(),
        None => return,
    };

    // Server-enforced mute: drop audio from muted peers
    if peer.is_muted.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    // Server-enforced timeout: drop audio while under moderation timeout (mirrors
    // the WebSocket relay above so UDP can't bypass moderation).
    {
        let now = crate::now_epoch_secs() as u64;
        let until = peer.timeout_until.load(std::sync::atomic::Ordering::Relaxed);
        if until > now {
            return;
        }
    }

    if !atomic_rate_check(
        &peer.audio_rate_window_ms,
        &peer.audio_frame_count,
        LIMITS.max_audio_fps,
    ) {
        metrics.udp_rate_limited_total.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let room_code = match peer.cached_room_code() {
        Some(c) => c,
        None => return,
    };

    // Build frame into reusable buffer for both UDP and WS delivery:
    // [MEDIA_PACKET_AUDIO, id_len, sender_id_bytes, audio_data]
    relay_buf.clear();
    relay_buf.push(shared_types::MEDIA_PACKET_AUDIO);
    relay_buf.push(sender_id.len() as u8);
    relay_buf.extend_from_slice(sender_id.as_bytes());
    relay_buf.extend_from_slice(data);

    // Get sender's persistent user_id for block checks (lock-free read)
    let sender_user_id: Option<String> = s
        .peers
        .get(sender_id)
        .and_then(|p| p.user_id.try_lock().ok().and_then(|uid| uid.clone()));

    // Collect room peers into reusable buffer with block + deafen filtering
    room_peers_buf.clear();
    if let Some(r) = s.rooms.get(&room_code) {
        for pid in &r.peer_ids {
            if pid.as_str() == sender_id {
                continue;
            }
            if let Some(p) = s.peers.get(pid) {
                // Block filtering: skip recipients who have blocked the sender
                if let Some(ref uid) = sender_user_id {
                    if p.blocked_by
                        .read()
                        .map(|b| b.contains(uid))
                        .unwrap_or(false)
                    {
                        continue;
                    }
                }
                // Server-deafen: skip recipients who are server-deafened
                if p.is_server_deafened
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    continue;
                }
                room_peers_buf.push(p.clone());
            }
        }
    }

    // Whisper filtering: read lock-free, no allocation when empty
    let whisper = s.peers.get(sender_id).and_then(|p| {
        p.whisper_targets
            .read()
            .ok()
            .filter(|t| !t.is_empty())
            .map(|t| t.clone())
    });

    // Drop the state read lock before sending frames
    drop(s);

    if let Some(targets) = whisper {
        room_peers_buf.retain(|p| targets.iter().any(|t| t == &p.id));
    }

    let frame = &*relay_buf;
    metrics
        .audio_frames_in_total
        .fetch_add(1, Ordering::Relaxed);
    metrics
        .audio_frames_out_total
        .fetch_add(room_peers_buf.len() as u64, Ordering::Relaxed);

    // Share one Bytes across every WS-fallback recipient (UDP recipients send
    // straight off the &[u8] slice; only WS recipients need the ref-counted form).
    let ws_shared: tokio_tungstenite::tungstenite::Bytes =
        tokio_tungstenite::tungstenite::Bytes::copy_from_slice(frame);
    for peer in room_peers_buf.iter() {
        let udp_addr = peer.udp_addr.read().ok().and_then(|a| *a);
        if let Some(addr) = udp_addr {
            // Send via UDP — fire-and-forget (UDP is unreliable by design)
            if let Err(_e) = udp_socket.send_to(frame, addr).await {
                metrics.udp_send_failures_total.fetch_add(1, Ordering::Relaxed);
            }
            metrics.udp_frames_out_total.fetch_add(1, Ordering::Relaxed);
        } else {
            // Fallback: push onto the WS media lane (drop on Full).
            crate::outbound::try_send_media(peer, ws_shared.clone(), metrics);
        }
    }
}

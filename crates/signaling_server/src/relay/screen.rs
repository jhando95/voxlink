use std::sync::Arc;
use std::sync::atomic::Ordering;
use futures_util::SinkExt;
use tokio::net::UdpSocket;
use tokio_tungstenite::tungstenite::Message;
use shared_types::{
    MAX_SCREEN_FRAME_SIZE, MAX_UDP_MEDIA_PAYLOAD_SIZE, MAX_UDP_SCREEN_CHUNK_SIZE,
    MEDIA_PACKET_SCREEN, MEDIA_PACKET_SCREEN_CHUNK, SCREEN_CHUNK_METADATA_LEN,
    ScreenChunkMetadata, decode_screen_chunk_metadata, SignalMessage,
};
use crate::types::{Peer, State};
use crate::metrics_server::ServerMetrics;
use crate::{LIMITS, UDP_SOCKET, send_to};
use crate::validation::{atomic_rate_check, ChunkedScreenSequenceState, chunked_screen_sequence_state};

type Metrics = Arc<ServerMetrics>;

pub(crate) async fn prepare_screen_relay(
    state: &State,
    sender_id: &str,
    chunk_sequence: Option<u32>,
) -> Option<(Vec<Arc<Peer>>, bool)> {
    let s = state.read().await;
    let peer = s.peers.get(sender_id)?.clone();

    let count_frame = match chunk_sequence {
        Some(sequence) => {
            match chunked_screen_sequence_state(&peer.last_screen_chunk_sequence, sequence) {
                ChunkedScreenSequenceState::NewFrame => {
                    if !atomic_rate_check(
                        &peer.screen_rate_window_ms,
                        &peer.screen_frame_count,
                        LIMITS.max_screen_fps,
                    ) {
                        return None;
                    }
                    true
                }
                ChunkedScreenSequenceState::ExistingFrame => false,
                ChunkedScreenSequenceState::StaleFrame => return None,
            }
        }
        None => {
            if !atomic_rate_check(
                &peer.screen_rate_window_ms,
                &peer.screen_frame_count,
                LIMITS.max_screen_fps,
            ) {
                return None;
            }
            true
        }
    };

    let room_code = peer.cached_room_code()?;
    let allowed = s
        .rooms
        .get(&room_code)
        .and_then(|room| room.active_screen_share_peer_id.as_deref())
        == Some(sender_id);
    if !allowed {
        return None;
    }

    let sender_user_id: Option<String> = s
        .peers
        .get(sender_id)
        .and_then(|p| p.user_id.try_lock().ok().and_then(|uid| uid.clone()));

    let mut recipients = Vec::new();
    if let Some(room) = s.rooms.get(&room_code) {
        for pid in &room.peer_ids {
            if pid.as_str() == sender_id {
                continue;
            }
            if let Some(candidate) = s.peers.get(pid) {
                if let Some(ref uid) = sender_user_id {
                    if candidate
                        .blocked_by
                        .read()
                        .map(|blocked| blocked.contains(uid))
                        .unwrap_or(false)
                    {
                        continue;
                    }
                }
                recipients.push(candidate.clone());
            }
        }
    }

    Some((recipients, count_frame))
}

pub(crate) fn screen_chunk_is_plausible(
    metadata: ScreenChunkMetadata,
    chunk_len: usize,
) -> bool {
    if chunk_len > MAX_UDP_SCREEN_CHUNK_SIZE {
        return false;
    }
    let max_chunks = MAX_SCREEN_FRAME_SIZE.div_ceil(MAX_UDP_SCREEN_CHUNK_SIZE);
    metadata.chunk_count as usize <= max_chunks
}

pub(crate) async fn send_screen_frame_to_peers(
    metrics: &Metrics,
    peers: Vec<Arc<Peer>>,
    frame: &[u8],
    udp_frame_ok: bool,
) {
    let send_timeout = std::time::Duration::from_millis(300);
    for peer in peers {
        if udp_frame_ok {
            let udp_addr = peer.udp_addr.read().ok().and_then(|addr| *addr);
            if let Some(addr) = udp_addr {
                if let Some(socket) = UDP_SOCKET.get() {
                    if let Err(_e) = socket.send_to(frame, addr).await {
                        metrics.udp_send_failures_total.fetch_add(1, Ordering::Relaxed);
                    }
                    metrics.udp_frames_out_total.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            }
        }

        let frame_clone = frame.to_vec();
        let peer_id_dbg = peer.id.clone();
        let fut = async {
            let mut tx = peer.tx.lock().await;
            if let Err(e) = tx.send(Message::Binary(frame_clone.into())).await {
                log::debug!("Screen frame send failed for peer {peer_id_dbg}: {e}");
            }
        };
        let _ = tokio::time::timeout(send_timeout, fut).await;
    }
}

pub(crate) async fn relay_screen(state: &State, metrics: &Metrics, sender_id: &str, data: &[u8]) {
    if sender_id.len() > u8::MAX as usize {
        log::warn!(
            "Screen relay: sender_id too long ({} bytes), dropping frame",
            sender_id.len()
        );
        return;
    }

    if data.len() > MAX_SCREEN_FRAME_SIZE {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(sender_id) {
            let msg = SignalMessage::Error {
                message: "Screen share frame too large, reduce quality".into(),
            };
            send_to(peer, &msg).await;
        }
        return;
    }

    let Some((others, count_frame)) = prepare_screen_relay(state, sender_id, None).await else {
        return;
    };

    if count_frame {
        metrics
            .screen_frames_in_total
            .fetch_add(1, Ordering::Relaxed);
        metrics
            .screen_frames_out_total
            .fetch_add(others.len() as u64, Ordering::Relaxed);
    }
    if others.is_empty() {
        return;
    }

    let mut frame = Vec::with_capacity(2 + sender_id.len() + data.len());
    frame.push(MEDIA_PACKET_SCREEN);
    frame.push(sender_id.len() as u8);
    frame.extend_from_slice(sender_id.as_bytes());
    frame.extend_from_slice(data);

    send_screen_frame_to_peers(
        metrics,
        others,
        &frame,
        data.len() <= MAX_UDP_SCREEN_CHUNK_SIZE,
    )
    .await;
}

pub(crate) async fn relay_screen_chunk(state: &State, metrics: &Metrics, sender_id: &str, data: &[u8]) {
    if sender_id.len() > u8::MAX as usize {
        return;
    }

    let Some((metadata, chunk_data)) = decode_screen_chunk_metadata(data) else {
        return;
    };
    if !screen_chunk_is_plausible(metadata, chunk_data.len()) {
        return;
    }

    let Some((others, count_frame)) =
        prepare_screen_relay(state, sender_id, Some(metadata.sequence)).await
    else {
        return;
    };

    if count_frame {
        metrics
            .screen_frames_in_total
            .fetch_add(1, Ordering::Relaxed);
        metrics
            .screen_frames_out_total
            .fetch_add(others.len() as u64, Ordering::Relaxed);
    }
    if others.is_empty() {
        return;
    }

    let mut frame = Vec::with_capacity(
        2 + sender_id.len() + SCREEN_CHUNK_METADATA_LEN + chunk_data.len(),
    );
    frame.push(MEDIA_PACKET_SCREEN_CHUNK);
    frame.push(sender_id.len() as u8);
    frame.extend_from_slice(sender_id.as_bytes());
    frame.extend_from_slice(data);

    send_screen_frame_to_peers(metrics, others, &frame, true).await;
}

pub(crate) async fn relay_screen_udp(
    state: &State,
    metrics: &Metrics,
    sender_id: &str,
    data: &[u8],
    udp_socket: &UdpSocket,
    relay_buf: &mut Vec<u8>,
    room_peers_buf: &mut Vec<Arc<Peer>>,
) {
    if data.len() > MAX_UDP_MEDIA_PAYLOAD_SIZE || sender_id.len() > 255 {
        return;
    }

    let s = state.read().await;

    let peer = match s.peers.get(sender_id) {
        Some(p) => p.clone(),
        None => return,
    };

    if !atomic_rate_check(
        &peer.screen_rate_window_ms,
        &peer.screen_frame_count,
        LIMITS.max_screen_fps,
    ) {
        metrics.udp_rate_limited_total.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let room_code = match peer.cached_room_code() {
        Some(code) => code,
        None => return,
    };

    let allowed = s
        .rooms
        .get(&room_code)
        .and_then(|room| room.active_screen_share_peer_id.as_deref())
        == Some(sender_id);
    if !allowed {
        return;
    }

    relay_buf.clear();
    relay_buf.push(MEDIA_PACKET_SCREEN);
    relay_buf.push(sender_id.len() as u8);
    relay_buf.extend_from_slice(sender_id.as_bytes());
    relay_buf.extend_from_slice(data);

    let sender_user_id: Option<String> = s
        .peers
        .get(sender_id)
        .and_then(|p| p.user_id.try_lock().ok().and_then(|uid| uid.clone()));

    room_peers_buf.clear();
    if let Some(room) = s.rooms.get(&room_code) {
        for pid in &room.peer_ids {
            if pid.as_str() == sender_id {
                continue;
            }
            if let Some(candidate) = s.peers.get(pid) {
                if let Some(ref uid) = sender_user_id {
                    if candidate
                        .blocked_by
                        .read()
                        .map(|blocked| blocked.contains(uid))
                        .unwrap_or(false)
                    {
                        continue;
                    }
                }
                room_peers_buf.push(candidate.clone());
            }
        }
    }

    drop(s);

    if room_peers_buf.is_empty() {
        metrics
            .screen_frames_in_total
            .fetch_add(1, Ordering::Relaxed);
        return;
    }

    let frame = &*relay_buf;
    metrics
        .screen_frames_in_total
        .fetch_add(1, Ordering::Relaxed);
    metrics
        .screen_frames_out_total
        .fetch_add(room_peers_buf.len() as u64, Ordering::Relaxed);

    let send_timeout = std::time::Duration::from_millis(300);
    let udp_frame_ok = data.len() <= MAX_UDP_SCREEN_CHUNK_SIZE;
    for peer in room_peers_buf.iter() {
        let udp_addr = if udp_frame_ok {
            peer.udp_addr.read().ok().and_then(|addr| *addr)
        } else {
            None
        };
        if let Some(addr) = udp_addr {
            if let Err(_e) = udp_socket.send_to(frame, addr).await {
                metrics.udp_send_failures_total.fetch_add(1, Ordering::Relaxed);
            }
            metrics.udp_frames_out_total.fetch_add(1, Ordering::Relaxed);
        } else {
            let frame_clone = frame.to_vec();
            let fut = async {
                let mut tx = peer.tx.lock().await;
                let _ = tx.send(Message::Binary(frame_clone.into())).await;
            };
            let _ = tokio::time::timeout(send_timeout, fut).await;
        }
    }
}

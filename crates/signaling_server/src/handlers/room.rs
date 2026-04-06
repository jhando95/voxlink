use crate::{send_error, send_to, validate_name, validate_password, validate_room_code};
use crate::{Peer, Room, State, LIMITS};
use shared_types::{ParticipantInfo, SignalMessage};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

/// Collect Arc<Peer> references for all peers in a room except `exclude_id`.
pub async fn collect_room_others(
    state: &State,
    room_code: &str,
    exclude_id: &str,
) -> Vec<Arc<Peer>> {
    let s = state.read().await;
    s.rooms
        .get(room_code)
        .map(|r| {
            r.peer_ids
                .iter()
                .filter(|pid| pid.as_str() != exclude_id)
                .filter_map(|pid| s.peers.get(pid).cloned())
                .collect()
        })
        .unwrap_or_default()
}

/// Broadcast a signal message to all peers in a room except `exclude_id`.
pub async fn broadcast_to_room(
    state: &State,
    room_code: &str,
    exclude_id: &str,
    msg: &SignalMessage,
) {
    let others = collect_room_others(state, room_code, exclude_id).await;
    for peer in others {
        send_to(&peer, msg).await;
    }
}

pub async fn handle_create_room(
    state: &State,
    peer_id: &str,
    user_name: String,
    password: Option<String>,
) {
    if let Err(e) = validate_name(&user_name) {
        send_error(state, peer_id, &e).await;
        return;
    }
    if let Err(e) = validate_password(&password) {
        send_error(state, peer_id, &e).await;
        return;
    }

    let mut s = state.write().await;
    let code = s.generate_room_code();

    if let Some(peer) = s.peers.get(peer_id) {
        *peer.name.lock().await = user_name.trim().to_string();
        peer.set_room_code(Some(code.clone())).await;
    }

    let has_pw = password.is_some();
    s.rooms.insert(
        code.clone(),
        Room {
            peer_ids: vec![peer_id.to_string()],
            password,
            active_screen_share_peer_id: None,
            created_at: Instant::now(),
        },
    );

    log::info!(
        "Room {code} created by {peer_id}{}",
        if has_pw { " (password protected)" } else { "" }
    );

    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(&peer, &SignalMessage::RoomCreated { room_code: code }).await;
    }
}

pub async fn handle_join_room(
    state: &State,
    peer_id: &str,
    room_code: String,
    user_name: String,
    password: Option<String>,
) {
    if let Err(e) = validate_name(&user_name) {
        send_error(state, peer_id, &e).await;
        return;
    }
    if let Err(e) = validate_room_code(&room_code) {
        send_error(state, peer_id, &e).await;
        return;
    }

    // Validate first, BEFORE mutating any peer state
    {
        let s = state.read().await;
        match s.rooms.get(&room_code) {
            None => {
                if let Some(peer) = s.peers.get(peer_id).cloned() {
                    drop(s);
                    send_to(
                        &peer,
                        &SignalMessage::Error {
                            message: format!("Room {room_code} not found"),
                        },
                    )
                    .await;
                }
                return;
            }
            Some(room) => {
                if let Some(ref room_pw) = room.password {
                    let provided = password.as_deref().unwrap_or("");
                    if provided != room_pw {
                        if let Some(peer) = s.peers.get(peer_id).cloned() {
                            drop(s);
                            send_to(
                                &peer,
                                &SignalMessage::Error {
                                    message: "Incorrect room password".into(),
                                },
                            )
                            .await;
                        }
                        return;
                    }
                }

                if room.peer_ids.len() >= LIMITS.max_room_peers {
                    if let Some(peer) = s.peers.get(peer_id).cloned() {
                        drop(s);
                        send_to(
                            &peer,
                            &SignalMessage::Error {
                                message: format!(
                                    "Room is full (max {} participants)",
                                    LIMITS.max_room_peers
                                ),
                            },
                        )
                        .await;
                    }
                    return;
                }
            }
        }
    }

    // Validation passed — now mutate state
    let mut s = state.write().await;

    // Update peer info
    if let Some(peer) = s.peers.get(peer_id) {
        *peer.name.lock().await = user_name.trim().to_string();
        peer.set_room_code(Some(room_code.clone())).await;
    }

    // Build participant list from existing peers
    let mut participants = Vec::new();
    if let Some(room) = s.rooms.get(&room_code) {
        for pid in &room.peer_ids {
            if let Some(p) = s.peers.get(pid) {
                participants.push(ParticipantInfo {
                    id: p.id.clone(),
                    name: p.name.lock().await.clone(),
                    is_muted: p.is_muted.load(Ordering::Relaxed),
                    is_deafened: p.is_deafened.load(Ordering::Relaxed),
                    is_priority_speaker: p.is_priority_speaker.load(Ordering::Relaxed),
                });
            }
        }
    }

    // Add peer to room
    if let Some(room) = s.rooms.get_mut(&room_code) {
        room.peer_ids.push(peer_id.to_string());
    }

    // Notify the joiner
    if let Some(peer) = s.peers.get(peer_id).cloned() {
        send_to(
            &peer,
            &SignalMessage::RoomJoined {
                room_code: room_code.clone(),
                participants,
            },
        )
        .await;
    }

    // Build joiner info for broadcasting
    let joiner_info = if let Some(p) = s.peers.get(peer_id) {
        Some(ParticipantInfo {
            id: p.id.clone(),
            name: p.name.lock().await.clone(),
            is_muted: p.is_muted.load(Ordering::Relaxed),
            is_deafened: p.is_deafened.load(Ordering::Relaxed),
            is_priority_speaker: p.is_priority_speaker.load(Ordering::Relaxed),
        })
    } else {
        None
    };

    // Notify other peers
    if let Some(info) = joiner_info {
        let notify = SignalMessage::PeerJoined { peer: info };
        let others: Vec<Arc<Peer>> = s
            .rooms
            .get(&room_code)
            .map(|r| {
                r.peer_ids
                    .iter()
                    .filter(|pid| pid.as_str() != peer_id)
                    .filter_map(|pid| s.peers.get(pid).cloned())
                    .collect()
            })
            .unwrap_or_default();
        drop(s);

        for peer in others {
            send_to(&peer, &notify).await;
        }
    }

    log::info!("Peer {peer_id} joined room {room_code}");
}

pub async fn handle_mute_changed(state: &State, peer_id: &str, is_muted: bool) {
    let room_code = {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            peer.is_muted.store(is_muted, Ordering::Relaxed);
            peer.cached_room_code()
        } else {
            None
        }
    };

    if let Some(code) = room_code {
        let notify = SignalMessage::PeerMuteChanged {
            peer_id: peer_id.to_string(),
            is_muted,
        };
        broadcast_to_room(state, &code, peer_id, &notify).await;
    }
}

pub async fn handle_deafen_changed(state: &State, peer_id: &str, is_deafened: bool) {
    let room_code = {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            peer.is_deafened.store(is_deafened, Ordering::Relaxed);
            peer.cached_room_code()
        } else {
            None
        }
    };

    if let Some(code) = room_code {
        let notify = SignalMessage::PeerDeafenChanged {
            peer_id: peer_id.to_string(),
            is_deafened,
        };
        broadcast_to_room(state, &code, peer_id, &notify).await;
    }
}

pub async fn handle_start_screen_share(state: &State, peer_id: &str) {
    let (sharer_name, peers) = {
        let mut s = state.write().await;
        let Some(peer) = s.peers.get(peer_id).cloned() else {
            return;
        };
        let Some(room_code) = peer.cached_room_code() else {
            drop(s);
            send_error(state, peer_id, "Join a room before starting screen share").await;
            return;
        };
        let Some(room) = s.rooms.get_mut(&room_code) else {
            drop(s);
            send_error(state, peer_id, "Room not found").await;
            return;
        };
        if room
            .active_screen_share_peer_id
            .as_deref()
            .is_some_and(|active| active != peer_id)
        {
            drop(s);
            send_error(state, peer_id, "Another screen share is already active").await;
            return;
        }

        room.active_screen_share_peer_id = Some(peer_id.to_string());
        let peer_ids = room.peer_ids.clone();
        let sharer_name = peer.name.lock().await.clone();
        let peers = peer_ids
            .iter()
            .filter_map(|pid| s.peers.get(pid).cloned())
            .collect::<Vec<_>>();
        (sharer_name, peers)
    };

    for peer in peers {
        send_to(
            &peer,
            &SignalMessage::ScreenShareStarted {
                sharer_id: peer_id.to_string(),
                sharer_name: sharer_name.clone(),
                is_self: peer.id == peer_id,
            },
        )
        .await;
    }
}

pub async fn handle_stop_screen_share(state: &State, peer_id: &str) {
    let room_code = {
        let s = state.read().await;
        s.peers
            .get(peer_id)
            .and_then(|peer| peer.cached_room_code())
    };
    if let Some(room_code) = room_code {
        stop_screen_share_in_room(state, &room_code, peer_id).await;
    }
}

pub async fn stop_screen_share_in_room(state: &State, room_code: &str, sharer_id: &str) {
    let peers = {
        let mut s = state.write().await;
        let Some(room) = s.rooms.get_mut(room_code) else {
            return;
        };
        if room.active_screen_share_peer_id.as_deref() != Some(sharer_id) {
            return;
        }
        room.active_screen_share_peer_id = None;
        let peer_ids = room.peer_ids.clone();
        peer_ids
            .iter()
            .filter_map(|pid| s.peers.get(pid).cloned())
            .collect::<Vec<_>>()
    };

    for peer in peers {
        send_to(
            &peer,
            &SignalMessage::ScreenShareStopped {
                sharer_id: sharer_id.to_string(),
            },
        )
        .await;
    }
}

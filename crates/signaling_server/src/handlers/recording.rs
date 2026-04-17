use std::sync::atomic::Ordering;
use shared_types::SignalMessage;
use crate::types::{State, Db};
use crate::connection::{send_to, send_error};
use crate::validation::now_epoch_secs;

pub(crate) async fn handle_start_recording(state: &State, peer_id: &str, channel_id: String) {
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
    let role = crate::handlers::space::role_for_identity(space, &user_id);
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

pub(crate) async fn handle_stop_recording(state: &State, peer_id: &str, channel_id: String) {
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
    let role = crate::handlers::space::role_for_identity(space, &user_id);
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

pub(crate) async fn handle_send_voice_note(
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
                .load(Ordering::Relaxed);
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
        crate::handlers::moderation::check_automod(db, &space_id, &content_text).await
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

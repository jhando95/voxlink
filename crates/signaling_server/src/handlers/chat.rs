use crate::{send_error, send_to};
use crate::{Db, Peer, State};
use shared_types::{ChannelType, ReactionData, SignalMessage};
use std::sync::Arc;

const MAX_DIRECT_MESSAGES: usize = 500;

pub async fn clear_typing_for_peer(state: &State, peer_id: &str) {
    let Some(peer) = peer_for_id(state, peer_id).await else {
        return;
    };
    let previous_channel = {
        let mut active = peer.typing_channel_id.lock().await;
        active.take()
    };
    let Some(channel_id) = previous_channel else {
        return;
    };

    let Some(space_id) = peer.space_id.lock().await.clone() else {
        return;
    };
    let user_name = peer.name.lock().await.clone();
    let notify = SignalMessage::TypingState {
        channel_id,
        user_name,
        is_typing: false,
    };
    crate::handlers::broadcast_to_space(state, &space_id, peer_id, &notify).await;
}

pub async fn clear_direct_typing_for_peer(state: &State, peer_id: &str) {
    let Some(peer) = peer_for_id(state, peer_id).await else {
        return;
    };
    let previous_target = {
        let mut active = peer.typing_dm_user_id.lock().await;
        active.take()
    };
    let Some(target_user_id) = previous_target else {
        return;
    };
    let Some(sender_user_id) = peer.user_id.lock().await.clone() else {
        return;
    };
    let notify = SignalMessage::DirectTypingState {
        user_id: sender_user_id,
        user_name: peer.name.lock().await.clone(),
        is_typing: false,
    };
    for recipient in peers_for_user(state, &target_user_id).await {
        send_to(&recipient, &notify).await;
    }
}

pub async fn handle_select_text_channel(state: &State, peer_id: &str, channel_id: String) {
    clear_typing_for_peer(state, peer_id).await;
    clear_direct_typing_for_peer(state, peer_id).await;

    let space_id = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => peer.space_id.lock().await.clone(),
            None => None,
        }
    };

    let Some(space_id) = space_id else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };

    let s = state.read().await;
    let Some(space) = s.spaces.get(&space_id) else {
        return;
    };

    let channel = space.channels.iter().find(|ch| ch.id == channel_id);
    let Some(ch) = channel else {
        drop(s);
        send_error(state, peer_id, "Channel not found").await;
        return;
    };

    if ch.channel_type != ChannelType::Text {
        drop(s);
        send_error(state, peer_id, "Not a text channel").await;
        return;
    }

    let channel_name = ch.name.clone();
    let history: Vec<_> = space
        .text_messages
        .get(&channel_id)
        .map(|dq| dq.iter().cloned().collect())
        .unwrap_or_default();

    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(
            &peer,
            &SignalMessage::TextChannelSelected {
                channel_id,
                channel_name,
                history,
            },
        )
        .await;
    }
}

pub async fn handle_select_direct_message(state: &State, peer_id: &str, user_id: String, db: &Db) {
    clear_typing_for_peer(state, peer_id).await;
    clear_direct_typing_for_peer(state, peer_id).await;

    let Some(db) = db.as_ref().cloned() else {
        send_error(state, peer_id, "Direct messages require persistence").await;
        return;
    };
    let Some(current_user_id) = authenticated_user_id(state, peer_id).await else {
        send_error(
            state,
            peer_id,
            "Authenticate before opening direct messages",
        )
        .await;
        return;
    };

    let target_user_id = user_id.trim().to_string();
    if target_user_id.is_empty() {
        send_error(state, peer_id, "Conversation target is missing").await;
        return;
    }
    if target_user_id == current_user_id {
        send_error(state, peer_id, "Use spaces for notes to yourself").await;
        return;
    }

    let current_for_db = current_user_id.clone();
    let target_for_db = target_user_id.clone();
    let loaded = tokio::task::spawn_blocking(
        move || -> Result<(String, Vec<shared_types::TextMessageData>), String> {
            if !db.friendship_exists(&current_for_db, &target_for_db)? {
                return Err("Direct messages are available for friends only".into());
            }
            let target_name = db
                .find_user_by_id(&target_for_db)?
                .map(|user| user.display_name)
                .unwrap_or_else(|| "Friend".into());
            let history = db
                .load_direct_messages_between(&current_for_db, &target_for_db, MAX_DIRECT_MESSAGES)?
                .into_iter()
                .map(|row| shared_types::TextMessageData {
                    sender_id: row.sender_user_id,
                    sender_name: row.sender_name,
                    content: row.content,
                    timestamp: row.timestamp as u64,
                    message_id: row.id,
                    edited: row.edited,
                    reactions: Vec::new(),
                    reply_to_message_id: row.reply_to_message_id,
                    reply_to_sender_name: row.reply_to_sender_name,
                    reply_preview: row.reply_preview,
                    pinned: false,
                })
                .collect();
            Ok((target_name, history))
        },
    )
    .await
    .unwrap_or_else(|_| Err("Direct message lookup failed".into()));

    match loaded {
        Ok((target_name, history)) => {
            if let Some(peer) = peer_for_id(state, peer_id).await {
                send_to(
                    &peer,
                    &SignalMessage::DirectMessageSelected {
                        user_id: target_user_id,
                        user_name: target_name,
                        history,
                    },
                )
                .await;
            }
        }
        Err(message) => {
            send_error(state, peer_id, &message).await;
        }
    }
}

pub async fn handle_set_typing(state: &State, peer_id: &str, channel_id: String, is_typing: bool) {
    if !is_typing {
        clear_typing_for_peer(state, peer_id).await;
        return;
    }

    let Some(peer) = peer_for_id(state, peer_id).await else {
        return;
    };
    let Some(space_id) = peer.space_id.lock().await.clone() else {
        return;
    };
    if !is_text_channel(state, &space_id, &channel_id).await {
        return;
    }
    clear_direct_typing_for_peer(state, peer_id).await;

    let previous_channel = {
        let mut active = peer.typing_channel_id.lock().await;
        if active.as_deref() == Some(channel_id.as_str()) {
            return;
        }
        active.replace(channel_id.clone())
    };

    let user_name = peer.name.lock().await.clone();

    if let Some(previous_channel) = previous_channel {
        let notify = SignalMessage::TypingState {
            channel_id: previous_channel,
            user_name: user_name.clone(),
            is_typing: false,
        };
        crate::handlers::broadcast_to_space(state, &space_id, peer_id, &notify).await;
    }

    let notify = SignalMessage::TypingState {
        channel_id,
        user_name,
        is_typing: true,
    };
    crate::handlers::broadcast_to_space(state, &space_id, peer_id, &notify).await;
}

pub async fn handle_set_direct_typing(
    state: &State,
    peer_id: &str,
    user_id: String,
    is_typing: bool,
    db: &Db,
) {
    if !is_typing {
        clear_direct_typing_for_peer(state, peer_id).await;
        return;
    }

    let Some(db) = db.as_ref().cloned() else {
        return;
    };
    let Some(peer) = peer_for_id(state, peer_id).await else {
        return;
    };
    let Some(current_user_id) = peer.user_id.lock().await.clone() else {
        return;
    };

    let target_user_id = user_id.trim().to_string();
    if target_user_id.is_empty() || target_user_id == current_user_id {
        return;
    }

    let current_for_db = current_user_id.clone();
    let target_for_db = target_user_id.clone();
    let allowed =
        tokio::task::spawn_blocking(move || db.friendship_exists(&current_for_db, &target_for_db))
            .await
            .unwrap_or(Ok(false))
            .unwrap_or(false);
    if !allowed {
        return;
    }

    clear_typing_for_peer(state, peer_id).await;

    let previous_target = {
        let mut active = peer.typing_dm_user_id.lock().await;
        if active.as_deref() == Some(target_user_id.as_str()) {
            return;
        }
        active.replace(target_user_id.clone())
    };
    let user_name = peer.name.lock().await.clone();

    if let Some(previous_target) = previous_target {
        let notify = SignalMessage::DirectTypingState {
            user_id: current_user_id.clone(),
            user_name: user_name.clone(),
            is_typing: false,
        };
        for recipient in peers_for_user(state, &previous_target).await {
            send_to(&recipient, &notify).await;
        }
    }

    let notify = SignalMessage::DirectTypingState {
        user_id: current_user_id,
        user_name,
        is_typing: true,
    };
    for recipient in peers_for_user(state, &target_user_id).await {
        send_to(&recipient, &notify).await;
    }
}

pub async fn handle_send_text_message(
    state: &State,
    peer_id: &str,
    channel_id: String,
    content: String,
    reply_to_message_id: Option<String>,
    db: &Db,
) {
    let content = content.trim().to_string();
    if content.is_empty() || content.len() > 2000 {
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
            let until = peer.timeout_until.load(std::sync::atomic::Ordering::Relaxed);
            if until > 0 && crate::now_epoch_secs() < until {
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

    clear_typing_for_peer(state, peer_id).await;

    // Get sender name and verify channel is text type
    let sender_name = {
        let s = state.read().await;
        let name = s
            .peers
            .get(peer_id)
            .map(|p| p.name.try_lock().map(|n| n.clone()).unwrap_or_default());
        let is_text = s
            .spaces
            .get(&space_id)
            .and_then(|sp| sp.channels.iter().find(|ch| ch.id == channel_id))
            .map(|ch| ch.channel_type == ChannelType::Text)
            .unwrap_or(false);
        if !is_text {
            return;
        }
        name
    };

    let Some(sender_name) = sender_name else {
        return;
    };

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
                let now = crate::now_epoch_secs();
                let key = (channel_id.clone(), peer_id.to_string());
                if let Some(&last) = space.slow_mode_timestamps.get(&key) {
                    if now < last + slow_mode_secs as u64 {
                        let remaining = (last + slow_mode_secs as u64) - now;
                        if let Some(peer) = s.peers.get(peer_id).cloned() {
                            drop(s);
                            send_to(
                                &peer,
                                &SignalMessage::Error {
                                    message: format!(
                                        "Slow mode: wait {remaining}s before sending another message"
                                    ),
                                },
                            )
                            .await;
                        }
                        return;
                    }
                }
                space.slow_mode_timestamps.insert(key, now);
            }
        }
    }

    let reply_metadata =
        match resolve_reply_metadata(state, &space_id, &channel_id, reply_to_message_id).await {
            Ok(metadata) => metadata,
            Err(message) => {
                send_error(state, peer_id, &message).await;
                return;
            }
        };

    let message_id = {
        let mut s = state.write().await;
        s.alloc_message_id()
    };

    // Use stable identity so the sender can edit/delete after reconnecting
    let stable_sender = super::space::stable_peer_id(state, peer_id).await;

    let msg_data = shared_types::TextMessageData {
        sender_id: stable_sender,
        sender_name: sender_name.clone(),
        content: content.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        message_id: message_id.clone(),
        edited: false,
        reactions: Vec::new(),
        reply_to_message_id: reply_metadata
            .as_ref()
            .map(|(message_id, _, _)| message_id.clone()),
        reply_to_sender_name: reply_metadata
            .as_ref()
            .map(|(_, sender_name, _)| sender_name.clone()),
        reply_preview: reply_metadata
            .as_ref()
            .map(|(_, _, preview)| preview.clone()),
        pinned: false,
    };

    // Store message in memory
    {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            let msgs = space.text_messages.entry(channel_id.clone()).or_default();
            msgs.push_back(msg_data.clone());
            if msgs.len() > crate::max_channel_messages() {
                msgs.pop_front();
            }
        }
    }

    // Persist to DB
    if let Some(ref db) = db {
        let db = db.clone();
        let mid = message_id.clone();
        let cid = channel_id.clone();
        let sid = msg_data.sender_id.clone();
        let sname = sender_name;
        let ts = msg_data.timestamp;
        let cont = content;
        let reply_to_message_id = msg_data.reply_to_message_id.clone();
        let reply_to_sender_name = msg_data.reply_to_sender_name.clone();
        let reply_preview = msg_data.reply_preview.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = db.save_message(&crate::persistence::MessageRow {
                id: mid,
                channel_id: cid,
                sender_id: sid,
                sender_name: sname,
                content: cont,
                timestamp: ts as i64,
                edited: false,
                reply_to_message_id,
                reply_to_sender_name,
                reply_preview,
                pinned: false,
            }) {
                log::error!("Failed to persist message: {e}");
            }
        });
    }

    // Broadcast to all space members
    let notify = SignalMessage::TextMessage {
        channel_id,
        message: msg_data,
    };
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

pub async fn handle_send_direct_message(
    state: &State,
    peer_id: &str,
    user_id: String,
    content: String,
    reply_to_message_id: Option<String>,
    db: &Db,
) {
    let content = content.trim().to_string();
    if content.is_empty() || content.len() > 2000 {
        return;
    }

    let Some(db) = db.as_ref().cloned() else {
        send_error(state, peer_id, "Direct messages require persistence").await;
        return;
    };
    let Some(peer) = peer_for_id(state, peer_id).await else {
        return;
    };
    let Some(current_user_id) = peer.user_id.lock().await.clone() else {
        send_error(
            state,
            peer_id,
            "Authenticate before sending direct messages",
        )
        .await;
        return;
    };

    let target_user_id = user_id.trim().to_string();
    if target_user_id.is_empty() || target_user_id == current_user_id {
        return;
    }

    clear_direct_typing_for_peer(state, peer_id).await;

    let sender_name = peer.name.lock().await.clone();
    let message_id = {
        let mut s = state.write().await;
        s.alloc_message_id()
    };
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let current_for_db = current_user_id.clone();
    let target_for_db = target_user_id.clone();
    let message_id_for_db = message_id.clone();
    let sender_name_for_db = sender_name.clone();
    let content_for_db = content.clone();
    let stored =
        tokio::task::spawn_blocking(move || -> Result<shared_types::TextMessageData, String> {
            if !db.friendship_exists(&current_for_db, &target_for_db)? {
                return Err("Direct messages are available for friends only".into());
            }
            let reply_metadata = resolve_direct_reply_metadata(
                &db,
                &current_for_db,
                &target_for_db,
                reply_to_message_id,
            )?;
            let (user_low_id, user_high_id) = ordered_user_pair(&current_for_db, &target_for_db);
            let row = crate::persistence::DirectMessageRow {
                id: message_id_for_db.clone(),
                user_low_id,
                user_high_id,
                sender_user_id: current_for_db.clone(),
                sender_name: sender_name_for_db.clone(),
                content: content_for_db.clone(),
                timestamp,
                edited: false,
                reply_to_message_id: reply_metadata
                    .as_ref()
                    .map(|(message_id, _, _)| message_id.clone()),
                reply_to_sender_name: reply_metadata
                    .as_ref()
                    .map(|(_, sender_name, _)| sender_name.clone()),
                reply_preview: reply_metadata
                    .as_ref()
                    .map(|(_, _, preview)| preview.clone()),
            };
            db.save_direct_message(&row)?;
            Ok(shared_types::TextMessageData {
                sender_id: current_for_db,
                sender_name: sender_name_for_db,
                content: content_for_db,
                timestamp: timestamp as u64,
                message_id: message_id_for_db,
                edited: false,
                reactions: Vec::new(),
                reply_to_message_id: row.reply_to_message_id,
                reply_to_sender_name: row.reply_to_sender_name,
                reply_preview: row.reply_preview,
                pinned: false,
            })
        })
        .await
        .unwrap_or_else(|_| Err("Direct message send failed".into()));

    match stored {
        Ok(message) => {
            let sender_notify = SignalMessage::DirectMessage {
                user_id: target_user_id.clone(),
                message: message.clone(),
            };
            for recipient in peers_for_user(state, &current_user_id).await {
                send_to(&recipient, &sender_notify).await;
            }

            let target_notify = SignalMessage::DirectMessage {
                user_id: current_user_id,
                message,
            };
            for recipient in peers_for_user(state, &target_user_id).await {
                send_to(&recipient, &target_notify).await;
            }
        }
        Err(message) => {
            send_error(state, peer_id, &message).await;
        }
    }
}

// ─── Edit Message (Milestone 5) ───

pub async fn handle_edit_text_message(
    state: &State,
    peer_id: &str,
    channel_id: String,
    message_id: String,
    new_content: String,
    db: &Db,
) {
    let new_content = new_content.trim().to_string();
    if new_content.is_empty() || new_content.len() > 2000 {
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

    // Use stable identity for ownership check (survives reconnection)
    let editor_identity = super::space::stable_peer_id(state, peer_id).await;

    // Verify ownership and update in-memory
    let ok = {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            if let Some(msgs) = space.text_messages.get_mut(&channel_id) {
                if let Some(msg) = msgs.iter_mut().find(|m| m.message_id == message_id) {
                    if msg.sender_id == editor_identity {
                        msg.content = new_content.clone();
                        msg.edited = true;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    };

    if !ok {
        send_error(state, peer_id, "Cannot edit this message").await;
        return;
    }

    // Persist
    if let Some(ref db) = db {
        let db = db.clone();
        let mid = message_id.clone();
        let content = new_content.clone();
        tokio::task::spawn_blocking(move || {
            let _ = db.update_message(&mid, &content);
        });
    }

    // Broadcast edit
    let notify = SignalMessage::TextMessageEdited {
        channel_id,
        message_id,
        new_content,
    };
    crate::handlers::broadcast_to_space(state, &space_id, "", &notify).await;
}

pub async fn handle_edit_direct_message(
    state: &State,
    peer_id: &str,
    user_id: String,
    message_id: String,
    new_content: String,
    db: &Db,
) {
    let new_content = new_content.trim().to_string();
    if new_content.is_empty() || new_content.len() > 2000 {
        return;
    }

    let Some(db) = db.as_ref().cloned() else {
        send_error(state, peer_id, "Direct messages require persistence").await;
        return;
    };
    let Some(current_user_id) = authenticated_user_id(state, peer_id).await else {
        send_error(
            state,
            peer_id,
            "Authenticate before editing direct messages",
        )
        .await;
        return;
    };
    let target_user_id = user_id.trim().to_string();
    if target_user_id.is_empty() || target_user_id == current_user_id {
        return;
    }

    let current_for_db = current_user_id.clone();
    let target_for_db = target_user_id.clone();
    let message_id_for_db = message_id.clone();
    let new_content_for_db = new_content.clone();
    let updated = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let Some(message) = db.get_direct_message(&message_id_for_db)? else {
            return Ok(false);
        };
        let (low, high) = ordered_user_pair(&current_for_db, &target_for_db);
        if message.user_low_id != low
            || message.user_high_id != high
            || message.sender_user_id != current_for_db
        {
            return Ok(false);
        }
        db.update_direct_message(&message_id_for_db, &new_content_for_db)
    })
    .await
    .unwrap_or_else(|_| Err("Direct message edit failed".into()));

    match updated {
        Ok(true) => {
            let sender_notify = SignalMessage::DirectMessageEdited {
                user_id: target_user_id.clone(),
                message_id: message_id.clone(),
                new_content: new_content.clone(),
            };
            for recipient in peers_for_user(state, &current_user_id).await {
                send_to(&recipient, &sender_notify).await;
            }

            let target_notify = SignalMessage::DirectMessageEdited {
                user_id: current_user_id,
                message_id,
                new_content,
            };
            for recipient in peers_for_user(state, &target_user_id).await {
                send_to(&recipient, &target_notify).await;
            }
        }
        Ok(false) => {
            send_error(state, peer_id, "Cannot edit this direct message").await;
        }
        Err(message) => {
            send_error(state, peer_id, &message).await;
        }
    }
}

pub async fn handle_pin_message(
    state: &State,
    peer_id: &str,
    channel_id: String,
    message_id: String,
    pinned: bool,
    db: &Db,
) {
    let owner_identity = super::space::stable_peer_id(state, peer_id).await;
    let space_id = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => peer.space_id.lock().await.clone(),
            None => None,
        }
    };
    let Some(space_id) = space_id else { return };

    let ok = {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            let is_owner = space.owner_id == owner_identity;
            if let Some(messages) = space.text_messages.get_mut(&channel_id) {
                if let Some(message) = messages.iter_mut().find(|msg| msg.message_id == message_id)
                {
                    if message.sender_id == owner_identity || is_owner {
                        message.pinned = pinned;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    };

    if !ok {
        send_error(state, peer_id, "Cannot change pin state for this message").await;
        return;
    }

    if let Some(ref db) = db {
        let db = db.clone();
        let message_id = message_id.clone();
        tokio::task::spawn_blocking(move || {
            let _ = db.set_message_pinned(&message_id, pinned);
        });
    }

    let notify = SignalMessage::MessagePinned {
        channel_id,
        message_id,
        pinned,
    };
    crate::handlers::broadcast_to_space(state, &space_id, "", &notify).await;
}

// ─── Delete Message (Milestone 5) ───

pub async fn handle_delete_text_message(
    state: &State,
    peer_id: &str,
    channel_id: String,
    message_id: String,
    db: &Db,
) {
    let owner_identity = super::space::stable_peer_id(state, peer_id).await;
    let space_id = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => peer.space_id.lock().await.clone(),
            None => None,
        }
    };
    let Some(space_id) = space_id else { return };

    // Check ownership: sender can delete own, space owner can delete any
    let ok = {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            let is_owner = space.owner_id == owner_identity;
            if let Some(msgs) = space.text_messages.get_mut(&channel_id) {
                if let Some(pos) = msgs.iter().position(|m| m.message_id == message_id) {
                    if msgs[pos].sender_id == owner_identity || is_owner {
                        msgs.remove(pos);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    };

    if !ok {
        send_error(state, peer_id, "Cannot delete this message").await;
        return;
    }

    // Persist
    if let Some(ref db) = db {
        let db = db.clone();
        let mid = message_id.clone();
        tokio::task::spawn_blocking(move || {
            let _ = db.delete_message(&mid);
        });
    }

    let notify = SignalMessage::TextMessageDeleted {
        channel_id,
        message_id,
    };
    crate::handlers::broadcast_to_space(state, &space_id, "", &notify).await;
}

pub async fn handle_delete_direct_message(
    state: &State,
    peer_id: &str,
    user_id: String,
    message_id: String,
    db: &Db,
) {
    let Some(db) = db.as_ref().cloned() else {
        send_error(state, peer_id, "Direct messages require persistence").await;
        return;
    };
    let Some(current_user_id) = authenticated_user_id(state, peer_id).await else {
        send_error(
            state,
            peer_id,
            "Authenticate before deleting direct messages",
        )
        .await;
        return;
    };
    let target_user_id = user_id.trim().to_string();
    if target_user_id.is_empty() || target_user_id == current_user_id {
        return;
    }

    let current_for_db = current_user_id.clone();
    let target_for_db = target_user_id.clone();
    let message_id_for_db = message_id.clone();
    let deleted = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let Some(message) = db.get_direct_message(&message_id_for_db)? else {
            return Ok(false);
        };
        let (low, high) = ordered_user_pair(&current_for_db, &target_for_db);
        if message.user_low_id != low
            || message.user_high_id != high
            || message.sender_user_id != current_for_db
        {
            return Ok(false);
        }
        db.delete_direct_message(&message_id_for_db)
    })
    .await
    .unwrap_or_else(|_| Err("Direct message delete failed".into()));

    match deleted {
        Ok(true) => {
            let sender_notify = SignalMessage::DirectMessageDeleted {
                user_id: target_user_id.clone(),
                message_id: message_id.clone(),
            };
            for recipient in peers_for_user(state, &current_user_id).await {
                send_to(&recipient, &sender_notify).await;
            }

            let target_notify = SignalMessage::DirectMessageDeleted {
                user_id: current_user_id,
                message_id,
            };
            for recipient in peers_for_user(state, &target_user_id).await {
                send_to(&recipient, &target_notify).await;
            }
        }
        Ok(false) => {
            send_error(state, peer_id, "Cannot delete this direct message").await;
        }
        Err(message) => {
            send_error(state, peer_id, &message).await;
        }
    }
}

// ─── React to Message (Milestone 5) ───

pub async fn handle_react_to_message(
    state: &State,
    peer_id: &str,
    channel_id: String,
    message_id: String,
    emoji: String,
) {
    let emoji = emoji.trim().to_string();
    if emoji.is_empty() || emoji.len() > 16 {
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

    let user_name = {
        let s = state.read().await;
        s.peers
            .get(peer_id)
            .map(|p| p.name.try_lock().map(|n| n.clone()).unwrap_or_default())
    };
    let Some(user_name) = user_name else { return };

    // Update in-memory
    {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            if let Some(msgs) = space.text_messages.get_mut(&channel_id) {
                if let Some(msg) = msgs.iter_mut().find(|m| m.message_id == message_id) {
                    // Toggle: add if not present, remove if already reacted
                    if let Some(reaction) = msg.reactions.iter_mut().find(|r| r.emoji == emoji) {
                        if let Some(pos) = reaction.users.iter().position(|u| u == &user_name) {
                            reaction.users.remove(pos);
                            if reaction.users.is_empty() {
                                msg.reactions.retain(|r| r.emoji != emoji);
                            }
                        } else {
                            reaction.users.push(user_name.clone());
                        }
                    } else {
                        msg.reactions.push(ReactionData {
                            emoji: emoji.clone(),
                            users: vec![user_name.clone()],
                        });
                    }
                }
            }
        }
    }

    let notify = SignalMessage::MessageReaction {
        channel_id,
        message_id,
        emoji,
        user_name,
    };
    crate::handlers::broadcast_to_space(state, &space_id, "", &notify).await;
}

async fn peer_for_id(state: &State, peer_id: &str) -> Option<Arc<Peer>> {
    let s = state.read().await;
    s.peers.get(peer_id).cloned()
}

async fn authenticated_user_id(state: &State, peer_id: &str) -> Option<String> {
    let peer = peer_for_id(state, peer_id).await?;
    let user_id = peer.user_id.lock().await.clone();
    user_id
}

async fn peers_for_user(state: &State, user_id: &str) -> Vec<Arc<Peer>> {
    let peers = {
        let s = state.read().await;
        s.peers.values().cloned().collect::<Vec<_>>()
    };

    let mut matching = Vec::new();
    for peer in peers {
        if peer.user_id.lock().await.as_deref() == Some(user_id) {
            matching.push(peer);
        }
    }
    matching
}

async fn is_text_channel(state: &State, space_id: &str, channel_id: &str) -> bool {
    let s = state.read().await;
    s.spaces
        .get(space_id)
        .and_then(|space| {
            space
                .channels
                .iter()
                .find(|channel| channel.id == channel_id)
        })
        .map(|channel| channel.channel_type == ChannelType::Text)
        .unwrap_or(false)
}

async fn resolve_reply_metadata(
    state: &State,
    space_id: &str,
    channel_id: &str,
    reply_to_message_id: Option<String>,
) -> Result<Option<(String, String, String)>, String> {
    let Some(reply_to_message_id) = reply_to_message_id else {
        return Ok(None);
    };
    let s = state.read().await;
    let Some(space) = s.spaces.get(space_id) else {
        return Err("Space not found".into());
    };
    let Some(messages) = space.text_messages.get(channel_id) else {
        return Err("Reply target not found".into());
    };
    let Some(target) = messages
        .iter()
        .find(|message| message.message_id == reply_to_message_id)
    else {
        return Err("Reply target not found".into());
    };

    Ok(Some((
        target.message_id.clone(),
        target.sender_name.clone(),
        reply_preview_for(&target.content),
    )))
}

fn reply_preview_for(content: &str) -> String {
    let single_line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let preview: String = single_line.chars().take(72).collect();
    if single_line.chars().count() > 72 {
        format!("{preview}...")
    } else {
        preview
    }
}

fn resolve_direct_reply_metadata(
    db: &crate::persistence::Database,
    current_user_id: &str,
    target_user_id: &str,
    reply_to_message_id: Option<String>,
) -> Result<Option<(String, String, String)>, String> {
    let Some(reply_to_message_id) = reply_to_message_id else {
        return Ok(None);
    };
    let Some(target) = db.get_direct_message(&reply_to_message_id)? else {
        return Err("Reply target not found".into());
    };
    let (low, high) = ordered_user_pair(current_user_id, target_user_id);
    if target.user_low_id != low || target.user_high_id != high {
        return Err("Reply target not found".into());
    }

    Ok(Some((
        target.id,
        target.sender_name,
        reply_preview_for(&target.content),
    )))
}

pub async fn handle_search_messages(
    state: &State,
    peer_id: &str,
    channel_id: String,
    query: String,
    limit: u32,
    db: &Db,
) {
    let query = query.trim().to_string();
    if query.is_empty() || query.len() > 200 {
        send_error(state, peer_id, "Invalid search query").await;
        return;
    }
    let limit = limit.min(100);

    let Some(db) = db else {
        send_error(state, peer_id, "Search unavailable").await;
        return;
    };

    // Verify the peer is in a space that owns this channel
    {
        let s = state.read().await;
        let Some(peer) = s.peers.get(peer_id) else {
            return;
        };
        if peer.space_id.lock().await.is_none() {
            send_error(state, peer_id, "Not in a space").await;
            return;
        }
    }

    let db = db.clone();
    let channel_id_for_db = channel_id.clone();
    let query_for_db = query.clone();
    let result = tokio::task::spawn_blocking(move || {
        db.search_messages(&channel_id_for_db, &query_for_db, limit)
    })
    .await;

    let peer = {
        let s = state.read().await;
        s.peers.get(peer_id).cloned()
    };
    let Some(peer) = peer else { return };

    match result {
        Ok(Ok(rows)) => {
            let messages: Vec<shared_types::TextMessageData> = rows
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
            let resp = SignalMessage::SearchResults {
                channel_id,
                messages,
            };
            send_to(&peer, &resp).await;
        }
        _ => {
            send_error(state, peer_id, "Search failed").await;
        }
    }
}

fn ordered_user_pair(user_a: &str, user_b: &str) -> (String, String) {
    if user_a <= user_b {
        (user_a.to_string(), user_b.to_string())
    } else {
        (user_b.to_string(), user_a.to_string())
    }
}

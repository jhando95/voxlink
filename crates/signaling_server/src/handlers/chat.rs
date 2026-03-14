use crate::{send_error, send_to};
use crate::{Db, Peer, State};
use shared_types::{ChannelType, ReactionData, SignalMessage};
use std::sync::Arc;

pub async fn handle_select_text_channel(state: &State, peer_id: &str, channel_id: String) {
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
    let Some(space) = s.spaces.get(&space_id) else { return };

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
    let history: Vec<_> = space.text_messages
        .get(&channel_id)
        .map(|dq| dq.iter().cloned().collect())
        .unwrap_or_default();

    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(&peer, &SignalMessage::TextChannelSelected {
            channel_id,
            channel_name,
            history,
        }).await;
    }
}

pub async fn handle_send_text_message(state: &State, peer_id: &str, channel_id: String, content: String, db: &Db) {
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

    // Get sender name and verify channel is text type
    let sender_name = {
        let s = state.read().await;
        let name = s.peers.get(peer_id)
            .map(|p| p.name.try_lock().map(|n| n.clone()).unwrap_or_default());
        let is_text = s.spaces.get(&space_id)
            .and_then(|sp| sp.channels.iter().find(|ch| ch.id == channel_id))
            .map(|ch| ch.channel_type == ChannelType::Text)
            .unwrap_or(false);
        if !is_text {
            return;
        }
        name
    };

    let Some(sender_name) = sender_name else { return };

    let message_id = {
        let mut s = state.write().await;
        s.alloc_message_id()
    };

    let msg_data = shared_types::TextMessageData {
        sender_id: peer_id.to_string(),
        sender_name: sender_name.clone(),
        content: content.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        message_id: message_id.clone(),
        edited: false,
        reactions: Vec::new(),
    };

    // Store message in memory
    {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            let msgs = space.text_messages.entry(channel_id.clone()).or_default();
            msgs.push_back(msg_data.clone());
            if msgs.len() > crate::MAX_CHANNEL_MESSAGES {
                msgs.pop_front();
            }
        }
    }

    // Persist to DB
    if let Some(ref db) = db {
        let db = db.clone();
        let mid = message_id.clone();
        let cid = channel_id.clone();
        let sid = peer_id.to_string();
        let sname = sender_name;
        let ts = msg_data.timestamp;
        let cont = content;
        tokio::task::spawn_blocking(move || {
            if let Err(e) = db.save_message(&crate::persistence::MessageRow {
                id: mid,
                channel_id: cid,
                sender_id: sid,
                sender_name: sname,
                content: cont,
                timestamp: ts as i64,
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
        let members: Vec<Arc<Peer>> = space.member_ids.iter()
            .filter_map(|id| s.peers.get(id).cloned())
            .collect();
        drop(s);
        for peer in members {
            send_to(&peer, &notify).await;
        }
    }
}

// ─── Edit Message (Milestone 5) ───

pub async fn handle_edit_text_message(
    state: &State, peer_id: &str, channel_id: String, message_id: String, new_content: String, db: &Db,
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

    // Verify ownership and update in-memory
    let ok = {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            if let Some(msgs) = space.text_messages.get_mut(&channel_id) {
                if let Some(msg) = msgs.iter_mut().find(|m| m.message_id == message_id) {
                    if msg.sender_id == peer_id {
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

// ─── Delete Message (Milestone 5) ───

pub async fn handle_delete_text_message(
    state: &State, peer_id: &str, channel_id: String, message_id: String, db: &Db,
) {
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
            let is_owner = space.owner_id == peer_id;
            if let Some(msgs) = space.text_messages.get_mut(&channel_id) {
                if let Some(pos) = msgs.iter().position(|m| m.message_id == message_id) {
                    if msgs[pos].sender_id == peer_id || is_owner {
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

// ─── React to Message (Milestone 5) ───

pub async fn handle_react_to_message(
    state: &State, peer_id: &str, channel_id: String, message_id: String, emoji: String,
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
        s.peers.get(peer_id)
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

use crate::{send_error, send_to};
use crate::{Peer, State};
use shared_types::{ChannelType, SignalMessage};
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

pub async fn handle_send_text_message(state: &State, peer_id: &str, channel_id: String, content: String) {
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

    let msg_data = shared_types::TextMessageData {
        sender_id: peer_id.to_string(),
        sender_name,
        content,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };

    // Store message
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

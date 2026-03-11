use crate::{send_error, send_to, validate_name};
use crate::{ChannelMeta, Peer, Room, Space, State};
use shared_types::{ChannelInfo, ChannelType, MemberInfo, SignalMessage, SpaceInfo};
use std::sync::Arc;
use std::time::Instant;

use super::channel::handle_leave_channel;

pub async fn broadcast_to_space(state: &State, space_id: &str, exclude_id: &str, msg: &SignalMessage) {
    let s = state.read().await;
    if let Some(space) = s.spaces.get(space_id) {
        let members: Vec<Arc<Peer>> = space.member_ids.iter()
            .filter(|id| id.as_str() != exclude_id)
            .filter_map(|id| s.peers.get(id).cloned())
            .collect();
        drop(s);
        for peer in members {
            send_to(&peer, msg).await;
        }
    }
}

pub async fn handle_create_space(state: &State, peer_id: &str, name: String, user_name: String) {
    if let Err(e) = validate_name(&name) {
        send_error(state, peer_id, &e).await;
        return;
    }
    if let Err(e) = validate_name(&user_name) {
        send_error(state, peer_id, &e).await;
        return;
    }

    let mut s = state.write().await;

    let space_id = s.alloc_space_id();
    let invite_code = s.generate_invite_code();
    let channel_id = s.alloc_channel_id();
    let room_key = format!("sp:{}:ch:{}", space_id, channel_id);

    // Set peer name
    if let Some(peer) = s.peers.get(peer_id) {
        *peer.name.lock().await = user_name.trim().to_string();
        *peer.space_id.lock().await = Some(space_id.clone());
    }

    // Create the default "General" channel room
    s.rooms.insert(
        room_key.clone(),
        Room {
            peer_ids: Vec::new(),
            password: None,
            created_at: Instant::now(),
        },
    );

    let channel_meta = ChannelMeta {
        id: channel_id.clone(),
        name: "General".into(),
        room_key,
        channel_type: ChannelType::Voice,
    };

    let space = Space {
        id: space_id.clone(),
        name: name.clone(),
        invite_code: invite_code.clone(),
        owner_id: peer_id.to_string(),
        channels: vec![channel_meta],
        member_ids: vec![peer_id.to_string()],
        text_messages: std::collections::HashMap::new(),
        created_at: Instant::now(),
    };

    s.invite_index.insert(invite_code.clone(), space_id.clone());
    s.spaces.insert(space_id.clone(), space);

    let space_info = SpaceInfo {
        id: space_id.clone(),
        name,
        invite_code,
        member_count: 1,
        channel_count: 1,
    };

    let channels = vec![ChannelInfo {
        id: channel_id,
        name: "General".into(),
        peer_count: 0,
        channel_type: ChannelType::Voice,
    }];

    log::info!("Space {} created by {peer_id}", space_id);

    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(&peer, &SignalMessage::SpaceCreated { space: space_info, channels }).await;
    }
}

pub async fn handle_join_space(state: &State, peer_id: &str, invite_code: String, user_name: String) {
    if let Err(e) = validate_name(&user_name) {
        send_error(state, peer_id, &e).await;
        return;
    }

    let mut s = state.write().await;

    // O(1) lookup via invite_index
    let space_id = s.invite_index.get(&invite_code).cloned();

    let space_id = match space_id {
        Some(id) => id,
        None => {
            if let Some(peer) = s.peers.get(peer_id).cloned() {
                drop(s);
                send_to(&peer, &SignalMessage::Error { message: "Invalid invite code".into() }).await;
            }
            return;
        }
    };

    // Set peer name and space_id
    if let Some(peer) = s.peers.get(peer_id) {
        *peer.name.lock().await = user_name.trim().to_string();
        *peer.space_id.lock().await = Some(space_id.clone());
    }

    // Add peer to space
    if let Some(space) = s.spaces.get_mut(&space_id) {
        if !space.member_ids.contains(&peer_id.to_string()) {
            space.member_ids.push(peer_id.to_string());
        }
    }

    // Build response data
    let (space_info, channels, members) = {
        let space = s.spaces.get(&space_id).unwrap();
        let space_info = SpaceInfo {
            id: space.id.clone(),
            name: space.name.clone(),
            invite_code: space.invite_code.clone(),
            member_count: space.member_ids.len() as u32,
            channel_count: space.channels.len() as u32,
        };

        let channels: Vec<ChannelInfo> = space.channels.iter().map(|ch| {
            let peer_count = s.rooms.get(&ch.room_key)
                .map(|r| r.peer_ids.len() as u32)
                .unwrap_or(0);
            ChannelInfo {
                id: ch.id.clone(),
                name: ch.name.clone(),
                peer_count,
                channel_type: ch.channel_type,
            }
        }).collect();

        let mut members = Vec::new();
        for mid in &space.member_ids {
            if let Some(p) = s.peers.get(mid) {
                let name = p.name.lock().await.clone();
                // Find if this member is in a channel
                let (ch_id, ch_name) = space.channels.iter()
                    .find(|ch| {
                        s.rooms.get(&ch.room_key)
                            .map(|r| r.peer_ids.contains(mid))
                            .unwrap_or(false)
                    })
                    .map(|ch| (Some(ch.id.clone()), Some(ch.name.clone())))
                    .unwrap_or((None, None));
                members.push(MemberInfo {
                    id: mid.clone(),
                    name,
                    channel_id: ch_id,
                    channel_name: ch_name,
                });
            }
        }

        (space_info, channels, members)
    };

    // Send SpaceJoined to joiner
    if let Some(peer) = s.peers.get(peer_id).cloned() {
        send_to(&peer, &SignalMessage::SpaceJoined {
            space: space_info,
            channels,
            members,
        }).await;
    }

    // Build MemberOnline for broadcasting
    let member_info = if let Some(p) = s.peers.get(peer_id) {
        Some(MemberInfo {
            id: peer_id.to_string(),
            name: p.name.lock().await.clone(),
            channel_id: None,
            channel_name: None,
        })
    } else {
        None
    };

    drop(s);

    // Broadcast MemberOnline to other space members
    if let Some(member) = member_info {
        broadcast_to_space(state, &space_id, peer_id, &SignalMessage::MemberOnline { member }).await;
    }

    log::info!("Peer {peer_id} joined space {space_id}");
}

pub async fn handle_leave_space(state: &State, peer_id: &str) {
    let space_id = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => peer.space_id.lock().await.clone(),
            None => None,
        }
    };

    let Some(space_id) = space_id else { return };

    // If peer is in a voice channel within the space, leave it first
    handle_leave_channel(state, peer_id).await;

    // Remove peer from space
    {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            space.member_ids.retain(|id| id != peer_id);
        }
    }

    // Clear peer's space_id
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            *peer.space_id.lock().await = None;
        }
    }

    // Broadcast MemberOffline
    let notify = SignalMessage::MemberOffline { member_id: peer_id.to_string() };
    broadcast_to_space(state, &space_id, peer_id, &notify).await;

    log::info!("Peer {peer_id} left space {space_id}");
}

pub async fn handle_delete_space(state: &State, peer_id: &str) {
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

    // Check ownership
    {
        let s = state.read().await;
        let is_owner = s.spaces.get(&space_id)
            .map(|space| space.owner_id == peer_id)
            .unwrap_or(false);
        if !is_owner {
            drop(s);
            send_error(state, peer_id, "Only the space creator can delete it").await;
            return;
        }
    }

    // Collect all members to notify
    let member_peers: Vec<Arc<Peer>> = {
        let s = state.read().await;
        s.spaces.get(&space_id)
            .map(|space| {
                space.member_ids.iter()
                    .filter_map(|id| s.peers.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    };

    // Remove space, its rooms, and invite index entry
    {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.remove(&space_id) {
            s.invite_index.remove(&space.invite_code);
            // Remove all channel rooms
            for ch in &space.channels {
                s.rooms.remove(&ch.room_key);
            }
        }
    }

    // Clear space_id and room_code for all members, notify them
    for peer in &member_peers {
        *peer.space_id.lock().await = None;
        peer.set_room_code(None).await;
        send_to(peer, &SignalMessage::SpaceDeleted).await;
    }

    log::info!("Space {space_id} deleted by {peer_id}");
}

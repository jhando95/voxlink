use crate::{send_error, send_to, validate_name};
use crate::{ChannelMeta, Db, Peer, Room, Space, State};
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

pub async fn handle_create_space(state: &State, peer_id: &str, name: String, user_name: String, db: &Db) {
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
        room_key: room_key.clone(),
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
        name: name.clone(),
        invite_code: invite_code.clone(),
        member_count: 1,
        channel_count: 1,
        is_owner: true,
    };

    let channels = vec![ChannelInfo {
        id: channel_id.clone(),
        name: "General".into(),
        peer_count: 0,
        channel_type: ChannelType::Voice,
    }];

    log::info!("Space {} created by {peer_id}", space_id);

    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(&peer, &SignalMessage::SpaceCreated { space: space_info, channels }).await;
    } else {
        drop(s);
    }

    // Persist space and channel to DB
    if let Some(ref db) = db {
        let db = db.clone();
        let sid = space_id;
        let sname = name;
        let sinvite = invite_code;
        let sowner = peer_id.to_string();
        let cid = channel_id;
        let rk = room_key;
        tokio::task::spawn_blocking(move || {
            if let Err(e) = db.save_space(&crate::persistence::SpaceRow {
                id: sid.clone(),
                name: sname,
                invite_code: sinvite,
                owner_id: sowner,
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            }) {
                log::error!("Failed to persist space: {e}");
            }
            if let Err(e) = db.save_channel(&crate::persistence::ChannelRow {
                id: cid,
                space_id: sid,
                name: "General".into(),
                room_key: rk,
                channel_type: "voice".into(),
            }) {
                log::error!("Failed to persist channel: {e}");
            }
        });
    }
}

pub async fn handle_join_space(state: &State, peer_id: &str, invite_code: String, user_name: String, db: &Db) {
    if let Err(e) = validate_name(&user_name) {
        send_error(state, peer_id, &e).await;
        return;
    }

    // Resolve space_id and check ban before taking write lock
    let (space_id, check_id) = {
        let s = state.read().await;
        let space_id = match s.invite_index.get(&invite_code).cloned() {
            Some(id) => id,
            None => {
                if let Some(peer) = s.peers.get(peer_id).cloned() {
                    drop(s);
                    send_to(&peer, &SignalMessage::Error { message: "Invalid invite code".into() }).await;
                }
                return;
            }
        };
        let check_id = if let Some(peer) = s.peers.get(peer_id) {
            peer.user_id.lock().await.clone().unwrap_or_else(|| peer_id.to_string())
        } else {
            peer_id.to_string()
        };
        (space_id, check_id)
    };

    // Check ban (outside state lock to avoid blocking)
    if let Some(ref db) = db {
        let db_clone = db.clone();
        let sid = space_id.clone();
        let cid = check_id;
        let banned = tokio::task::spawn_blocking(move || {
            db_clone.is_banned(&sid, &cid).unwrap_or(false)
        }).await.unwrap_or(false);
        if banned {
            send_error(state, peer_id, "You are banned from this space").await;
            return;
        }
    }

    let mut s = state.write().await;

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
            is_owner: space.owner_id == peer_id,
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

pub async fn handle_delete_space(state: &State, peer_id: &str, db: &Db) {
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

    // Persist deletion
    if let Some(ref db) = db {
        let db = db.clone();
        let sid = space_id.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = db.delete_space(&sid) {
                log::error!("Failed to delete space from DB: {e}");
            }
        });
    }

    log::info!("Space {space_id} deleted by {peer_id}");
}

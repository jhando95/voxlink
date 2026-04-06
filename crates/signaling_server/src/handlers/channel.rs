use crate::{send_error, send_to, validate_name};
use crate::{ChannelMeta, Db, Peer, Room, State};
use shared_types::{ChannelInfo, ChannelType, ParticipantInfo, SignalMessage};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use super::room::collect_room_others;
use super::space::broadcast_to_space;

pub async fn handle_create_channel(
    state: &State,
    peer_id: &str,
    channel_name: String,
    channel_type: ChannelType,
    voice_quality: u8,
    db: &Db,
) {
    if let Err(e) = validate_name(&channel_name) {
        send_error(state, peer_id, &e).await;
        return;
    }

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

    let Some((_, actor_user_id, actor_role)) = super::space::peer_space_role(state, peer_id).await
    else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };
    if !super::space::can_manage_channels(actor_role) {
        send_error(state, peer_id, "Only admins can create channels").await;
        return;
    }

    let actor_name = {
        let peer = {
            let s = state.read().await;
            s.peers.get(peer_id).cloned()
        };
        if let Some(peer) = peer {
            peer.name.lock().await.clone()
        } else {
            "Unknown".into()
        }
    };

    let channel_info = {
        let mut s = state.write().await;
        let channel_id = s.alloc_channel_id();
        let room_key = format!("sp:{}:ch:{}", space_id, channel_id);

        // Create room for the channel
        s.rooms.insert(
            room_key.clone(),
            Room {
                peer_ids: Vec::new(),
                password: None,
                active_screen_share_peer_id: None,
                created_at: Instant::now(),
            },
        );

        let quality = voice_quality.min(3);
        let meta = ChannelMeta {
            id: channel_id.clone(),
            name: channel_name.clone(),
            room_key,
            channel_type,
            topic: String::new(),
            voice_quality: quality,
            user_limit: 0,
            category: String::new(),
            status: String::new(),
            slow_mode_secs: 0,
            min_role: shared_types::SpaceRole::Member,
            position: 0,
            auto_delete_hours: 0,
        };

        if let Some(space) = s.spaces.get_mut(&space_id) {
            space.channels.push(meta);
        }

        ChannelInfo {
            id: channel_id,
            name: channel_name,
            peer_count: 0,
            channel_type,
            topic: String::new(),
            voice_quality: quality,
            user_limit: 0,
            category: String::new(),
            status: String::new(),
            slow_mode_secs: 0,
            position: 0,
            auto_delete_hours: 0,
            min_role: String::new(),
        }
    };

    log::info!(
        "Channel {} created in space {space_id} by {peer_id}",
        channel_info.id
    );

    // Persist channel to DB
    let vq = channel_info.voice_quality;
    if let Some(ref db) = db {
        let db = db.clone();
        let cid = channel_info.id.clone();
        let sid = space_id.clone();
        let cname = channel_info.name.clone();
        // Retrieve room_key from state
        let rk = {
            let s = state.read().await;
            s.spaces
                .get(&space_id)
                .and_then(|sp| sp.channels.iter().find(|c| c.id == cid))
                .map(|c| c.room_key.clone())
                .unwrap_or_default()
        };
        let ct = if channel_type == ChannelType::Text {
            "text"
        } else {
            "voice"
        };
        tokio::task::spawn_blocking(move || {
            if let Err(e) = db.save_channel(&crate::persistence::ChannelRow {
                id: cid,
                space_id: sid,
                name: cname,
                room_key: rk,
                channel_type: ct.into(),
                topic: None,
                voice_quality: Some(vq),
                min_role: None,
                position: None,
                auto_delete_hours: None,
            }) {
                log::error!("Failed to persist channel: {e}");
            }
        });
    }

    let created_channel_name = channel_info.name.clone();
    let notify = SignalMessage::ChannelCreated {
        channel: channel_info,
    };
    // Broadcast to all space members including the creator
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

    let channel_type_label = if channel_type == ChannelType::Text {
        "text"
    } else {
        "voice"
    };
    let _ = super::space::append_audit_entry(
        state,
        db,
        &space_id,
        &actor_user_id,
        &actor_name,
        "channel",
        None,
        Some(created_channel_name),
        format!("Created a {channel_type_label} channel"),
    )
    .await;
}

pub async fn handle_delete_channel(state: &State, peer_id: &str, channel_id: String, db: &Db) {
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

    let Some((_, actor_user_id, actor_role)) = super::space::peer_space_role(state, peer_id).await
    else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };
    if !super::space::can_manage_channels(actor_role) {
        send_error(state, peer_id, "Only admins can delete channels").await;
        return;
    }

    let actor_name = {
        let peer = {
            let s = state.read().await;
            s.peers.get(peer_id).cloned()
        };
        if let Some(peer) = peer {
            peer.name.lock().await.clone()
        } else {
            "Unknown".into()
        }
    };

    let (deleted_channel, member_ids, affected_voice_ids) = {
        let mut s = state.write().await;
        let Some((member_ids, channel_count)) = s
            .spaces
            .get(&space_id)
            .map(|space| (space.member_ids.clone(), space.channels.len()))
        else {
            send_error(state, peer_id, "Space not found").await;
            return;
        };

        if channel_count <= 1 {
            drop(s);
            send_error(state, peer_id, "A space must keep at least one channel").await;
            return;
        }

        let deleted_channel = {
            let Some(space) = s.spaces.get_mut(&space_id) else {
                drop(s);
                send_error(state, peer_id, "Space no longer exists").await;
                return;
            };
            let Some(index) = space
                .channels
                .iter()
                .position(|channel| channel.id == channel_id)
            else {
                drop(s);
                send_error(state, peer_id, "Channel not found").await;
                return;
            };
            space.text_messages.remove(&channel_id);
            space.channels.remove(index)
        };
        let affected_voice_ids = s
            .rooms
            .remove(&deleted_channel.room_key)
            .map(|room| room.peer_ids)
            .unwrap_or_default();
        (deleted_channel, member_ids, affected_voice_ids)
    };

    let (member_peers, affected_voice_peers): (Vec<Arc<Peer>>, Vec<Arc<Peer>>) = {
        let s = state.read().await;
        (
            member_ids
                .iter()
                .filter_map(|id| s.peers.get(id).cloned())
                .collect(),
            affected_voice_ids
                .iter()
                .filter_map(|id| s.peers.get(id).cloned())
                .collect(),
        )
    };

    let deleted_notify = SignalMessage::ChannelDeleted {
        channel_id: channel_id.clone(),
    };
    for peer in &member_peers {
        send_to(peer, &deleted_notify).await;
    }

    for peer in &affected_voice_peers {
        peer.set_room_code(None).await;
        send_to(peer, &SignalMessage::ChannelLeft).await;
    }

    for member_id in affected_voice_ids {
        let notify = SignalMessage::MemberChannelChanged {
            member_id,
            channel_id: None,
            channel_name: None,
        };
        for peer in &member_peers {
            send_to(peer, &notify).await;
        }
    }

    if let Some(ref db) = db {
        let db = db.clone();
        let channel_id = channel_id.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = db.delete_channel(&channel_id) {
                log::error!("Failed to delete channel: {e}");
            }
        });
    }

    log::info!(
        "Channel {} deleted in space {space_id} by {peer_id}",
        deleted_channel.id
    );

    let _ = super::space::append_audit_entry(
        state,
        db,
        &space_id,
        &actor_user_id,
        &actor_name,
        "channel",
        None,
        Some(deleted_channel.name.clone()),
        "Deleted the channel".into(),
    )
    .await;
}

pub async fn handle_join_channel(state: &State, peer_id: &str, channel_id: String) {
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

    // Leave current channel first (if any)
    handle_leave_channel(state, peer_id).await;

    let mut s = state.write().await;

    // Find channel in space
    let channel_data = s.spaces.get(&space_id).and_then(|space| {
        space
            .channels
            .iter()
            .find(|ch| ch.id == channel_id)
            .map(|ch| (ch.room_key.clone(), ch.name.clone(), ch.channel_type, ch.voice_quality, ch.min_role))
    });

    let Some((room_key, channel_name, ch_type, voice_quality, min_role)) = channel_data else {
        if let Some(peer) = s.peers.get(peer_id).cloned() {
            drop(s);
            send_to(
                &peer,
                &SignalMessage::Error {
                    message: "Channel not found".into(),
                },
            )
            .await;
        }
        return;
    };

    // Check channel permission (min_role)
    if min_role != shared_types::SpaceRole::Member {
        let user_role = {
            let peer = s.peers.get(peer_id);
            if let Some(peer) = peer {
                let user_id = peer.user_id.lock().await.clone();
                if let Some(uid) = &user_id {
                    s.spaces.get(&space_id)
                        .and_then(|sp| sp.member_roles.get(uid).copied())
                        .unwrap_or(shared_types::SpaceRole::Member)
                } else {
                    shared_types::SpaceRole::Member
                }
            } else {
                shared_types::SpaceRole::Member
            }
        };
        if !user_role.has_at_least(min_role) {
            if let Some(peer) = s.peers.get(peer_id).cloned() {
                drop(s);
                send_to(
                    &peer,
                    &SignalMessage::Error {
                        message: "You don't have permission to access this channel".into(),
                    },
                )
                .await;
            }
            return;
        }
    }

    // Check user limit
    if ch_type == ChannelType::Voice {
        let user_limit = s.spaces.get(&space_id).and_then(|sp| {
            sp.channels.iter().find(|c| c.id == channel_id).map(|c| c.user_limit)
        }).unwrap_or(0);
        if user_limit > 0 {
            let current = s.rooms.get(&room_key).map(|r| r.peer_ids.len() as u32).unwrap_or(0);
            if current >= user_limit {
                if let Some(peer) = s.peers.get(peer_id).cloned() {
                    drop(s);
                    send_to(
                        &peer,
                        &SignalMessage::Error {
                            message: format!("Channel is full ({user_limit}/{user_limit})"),
                        },
                    )
                    .await;
                }
                return;
            }
        }
    }

    // Text channels cannot be joined for voice
    if ch_type == ChannelType::Text {
        if let Some(peer) = s.peers.get(peer_id).cloned() {
            drop(s);
            send_to(
                &peer,
                &SignalMessage::Error {
                    message: "Cannot join a text channel for voice".into(),
                },
            )
            .await;
        }
        return;
    }

    // Set peer's room_code to the channel's room_key (integrates with audio relay)
    if let Some(peer) = s.peers.get(peer_id) {
        peer.set_room_code(Some(room_key.clone())).await;
    }

    // Build participant list of existing peers in channel
    let mut participants = Vec::new();
    if let Some(room) = s.rooms.get(&room_key) {
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
    if let Some(room) = s.rooms.get_mut(&room_key) {
        room.peer_ids.push(peer_id.to_string());
    }

    let joiner_peer = s.peers.get(peer_id).cloned();

    // Send PeerJoined to others in the channel
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

    let (joiner_info, others) = if let Some(info) = joiner_info {
        let others: Vec<Arc<Peer>> = s
            .rooms
            .get(&room_key)
            .map(|r| {
                r.peer_ids
                    .iter()
                    .filter(|pid| pid.as_str() != peer_id)
                    .filter_map(|pid| s.peers.get(pid).cloned())
                    .collect()
            })
            .unwrap_or_default();
        (Some(info), others)
    } else {
        (None, Vec::new())
    };

    drop(s);

    // Notify the joiner after the room membership update has completed.
    if let Some(peer) = joiner_peer {
        send_to(
            &peer,
            &SignalMessage::ChannelJoined {
                channel_id: channel_id.clone(),
                channel_name: channel_name.clone(),
                participants,
                voice_quality,
            },
        )
        .await;
    }

    if let Some(info) = joiner_info {
        let notify = SignalMessage::PeerJoined { peer: info };
        for peer in &others {
            send_to(peer, &notify).await;
        }
    }

    // Broadcast MemberChannelChanged to all space members
    let notify = SignalMessage::MemberChannelChanged {
        member_id: peer_id.to_string(),
        channel_id: Some(channel_id.clone()),
        channel_name: Some(channel_name),
    };
    broadcast_to_space(state, &space_id, peer_id, &notify).await;
    // Also send to self so they see their own state update
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id).cloned() {
            drop(s);
            send_to(&peer, &notify).await;
        }
    }

    log::info!("Peer {peer_id} joined channel {channel_id} in space {space_id}");
}

pub async fn handle_leave_channel(state: &State, peer_id: &str) {
    let (room_code, space_id) = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => {
                let rc = peer.cached_room_code();
                let sid = peer.space_id.lock().await.clone();
                (rc, sid)
            }
            None => (None, None),
        }
    };

    let Some(ref code) = room_code else { return };
    // Only handle space channel rooms (prefixed with "sp:")
    if !code.starts_with("sp:") {
        return;
    }

    super::room::stop_screen_share_in_room(state, code, peer_id).await;

    let remaining = collect_room_others(state, code, peer_id).await;

    {
        let mut s = state.write().await;
        if let Some(room) = s.rooms.get_mut(code) {
            room.peer_ids.retain(|pid| pid != peer_id);
            // Don't remove space channel rooms when empty (they're persistent)
        }
    }

    // Notify remaining channel peers
    let notify = SignalMessage::PeerLeft {
        peer_id: peer_id.to_string(),
    };
    for peer in remaining {
        send_to(&peer, &notify).await;
    }

    // Clear peer's room code and send ChannelLeft (single lock acquisition)
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            peer.set_room_code(None).await;
            let peer = peer.clone();
            drop(s);
            send_to(&peer, &SignalMessage::ChannelLeft).await;
        }
    }

    // Broadcast MemberChannelChanged to space members (including self for peer count update)
    if let Some(ref sid) = space_id {
        let notify = SignalMessage::MemberChannelChanged {
            member_id: peer_id.to_string(),
            channel_id: None,
            channel_name: None,
        };
        broadcast_to_space(state, sid, peer_id, &notify).await;
        // Also send to self so their channel list updates peer counts
        {
            let s = state.read().await;
            if let Some(peer) = s.peers.get(peer_id).cloned() {
                drop(s);
                send_to(&peer, &notify).await;
            }
        }
    }

    log::info!("Peer {peer_id} left channel");
}

/// Reorder channels in a space. Only admins+ can reorder.
pub async fn handle_reorder_channels(
    state: &State,
    peer_id: &str,
    channel_ids: Vec<String>,
) {
    let space_id = {
        let s = state.read().await;
        let Some(peer) = s.peers.get(peer_id) else { return };
        let sid = peer.space_id.lock().await.clone();
        drop(s);
        sid
    };
    let Some(space_id) = space_id else { return };

    // Check permission using user_id (not peer_id)
    {
        let Some((_sid, _uid, role)) = super::space::peer_space_role(state, peer_id).await else {
            crate::send_error(state, peer_id, "Not in a space").await;
            return;
        };
        if crate::handlers::space::role_rank(role) < crate::handlers::space::role_rank(shared_types::SpaceRole::Admin) {
            crate::send_error(state, peer_id, "Only admins can reorder channels").await;
            return;
        }
    }

    // Apply new order (write lock)
    {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            for (pos, cid) in channel_ids.iter().enumerate() {
                if let Some(ch) = space.channels.iter_mut().find(|c| c.id == *cid) {
                    ch.position = pos as u32;
                }
            }
            space.channels.sort_by_key(|c| c.position);
        }
    }

    // Broadcast to all in space
    let notify = shared_types::SignalMessage::ChannelsReordered { channel_ids };
    crate::handlers::broadcast_to_space(state, &space_id, "", &notify).await;
    log::info!("Channels reordered in space {space_id} by {peer_id}");
}

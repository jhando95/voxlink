use std::sync::Arc;
use std::sync::atomic::Ordering;
use shared_types::SignalMessage;
use crate::types::{Peer, State, Db};
use crate::connection::{send_to, send_error};
use crate::DB_TIMEOUT;

pub(crate) enum ChannelSetting {
    UserLimit(u32),
    SlowMode(u32),
    Category(String),
    Status(String),
    MinRole(shared_types::SpaceRole),
    AutoDelete(u32),
}

pub(crate) async fn handle_set_space_public(state: &State, peer_id: &str, is_public: bool, db: &Db) {
    let (space_id, _user_id, role, member_ids) = {
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
        let member_ids = space.member_ids.clone();
        (space_id, user_id, role, member_ids)
    };

    if !matches!(
        role,
        shared_types::SpaceRole::Owner | shared_types::SpaceRole::Admin
    ) {
        send_error(state, peer_id, "Only admins can change space visibility").await;
        return;
    }
    // Update in-memory state
    {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            space.is_public = is_public;
        }
    }
    if let Some(db) = db {
        let _ = db.set_space_public(&space_id, is_public);
    }
    // Broadcast to space members
    let s = state.read().await;
    for mid in &member_ids {
        for (_, p) in s.peers.iter() {
            let uid = p.user_id.lock().await.clone().unwrap_or_default();
            if uid == *mid {
                send_to(p, &SignalMessage::SpacePublicChanged { is_public }).await;
            }
        }
    }
}

pub(crate) async fn handle_browse_public_spaces(state: &State, peer_id: &str, db: &Db) {
    let mut spaces = Vec::new();
    if let Some(db) = db {
        if let Ok(public_rows) = db.load_public_spaces() {
            let s = state.read().await;
            for (id, name, desc, invite) in public_rows {
                let (member_count, channel_count, online_count) =
                    if let Some(sp) = s.spaces.get(&id) {
                        let online = s
                            .peers
                            .values()
                            .filter(|p| {
                                p.space_id
                                    .try_lock()
                                    .map(|sid| sid.as_deref() == Some(id.as_str()))
                                    .unwrap_or(false)
                            })
                            .count() as u32;
                        (sp.member_ids.len() as u32, sp.channels.len() as u32, online)
                    } else {
                        (0, 0, 0)
                    };
                spaces.push(shared_types::PublicSpaceInfo {
                    id,
                    name,
                    description: desc,
                    invite_code: invite,
                    member_count,
                    channel_count,
                    online_count,
                });
            }
        }
    }
    let s = state.read().await;
    if let Some(p) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(&p, &SignalMessage::PublicSpaceList { spaces }).await;
    }
}

pub(crate) async fn handle_set_channel_topic(
    state: &State,
    peer_id: &str,
    channel_id: String,
    topic: String,
    db: &Db,
) {
    let topic = topic.chars().take(256).collect::<String>();
    let Some((space_id, actor_user_id, actor_role)) =
        crate::handlers::space::peer_space_role(state, peer_id).await
    else {
        return;
    };
    if !crate::handlers::space::can_manage_channels(actor_role) {
        send_error(state, peer_id, "Only admins can change channel topics").await;
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
    let changed_channel_name = {
        let mut s = state.write().await;
        let Some(space) = s.spaces.get_mut(&space_id) else {
            return;
        };
        let Some(channel) = space.channels.iter_mut().find(|c| c.id == channel_id) else {
            return;
        };
        channel.topic = topic.clone();
        channel.name.clone()
    };

    // Persist to DB
    if let Some(db) = db {
        let db = db.clone();
        let cid = channel_id.clone();
        let t = topic.clone();
        match tokio::time::timeout(
            DB_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                db.set_channel_topic(&cid, &t);
            }),
        )
        .await
        {
            Err(_) => log::warn!("DB timeout: set_channel_topic for channel {channel_id}"),
            Ok(Err(e)) => log::warn!("DB task panicked in set_channel_topic: {e}"),
            Ok(Ok(())) => {}
        }
    }

    let notify = SignalMessage::ChannelTopicChanged {
        channel_id: channel_id.clone(),
        topic: topic.clone(),
    };
    crate::handlers::broadcast_to_space(state, &space_id, peer_id, &notify).await;

    // Also send to the setter
    if let Some(peer) = state.read().await.peers.get(peer_id) {
        send_to(peer, &notify).await;
    }

    let _ = crate::handlers::space::append_audit_entry(
        state,
        db,
        &space_id,
        &actor_user_id,
        &actor_name,
        "topic",
        None,
        Some(changed_channel_name),
        "Updated the channel topic".into(),
    )
    .await;
}

pub(crate) async fn handle_channel_setting(
    state: &State,
    peer_id: &str,
    channel_id: String,
    setting: ChannelSetting,
) {
    let Some((space_id, _, actor_role)) = crate::handlers::space::peer_space_role(state, peer_id).await
    else {
        return;
    };
    if !crate::handlers::space::can_manage_channels(actor_role) {
        send_error(state, peer_id, "Only admins can change channel settings").await;
        return;
    }

    let notify = {
        let mut s = state.write().await;
        let Some(space) = s.spaces.get_mut(&space_id) else {
            return;
        };
        let Some(channel) = space.channels.iter_mut().find(|c| c.id == channel_id) else {
            return;
        };
        match setting {
            ChannelSetting::UserLimit(limit) => {
                channel.user_limit = limit;
                SignalMessage::ChannelUserLimitChanged {
                    channel_id: channel_id.clone(),
                    user_limit: limit,
                }
            }
            ChannelSetting::SlowMode(secs) => {
                channel.slow_mode_secs = secs;
                SignalMessage::ChannelSlowModeChanged {
                    channel_id: channel_id.clone(),
                    slow_mode_secs: secs,
                }
            }
            ChannelSetting::Category(ref cat) => {
                channel.category = cat.chars().take(32).collect();
                SignalMessage::ChannelCategoryChanged {
                    channel_id: channel_id.clone(),
                    category: channel.category.clone(),
                }
            }
            ChannelSetting::Status(ref status) => {
                channel.status = status.chars().take(64).collect();
                SignalMessage::ChannelStatusChanged {
                    channel_id: channel_id.clone(),
                    status: channel.status.clone(),
                }
            }
            ChannelSetting::MinRole(role) => {
                channel.min_role = role;
                let role_str = match role {
                    shared_types::SpaceRole::Owner => "owner",
                    shared_types::SpaceRole::Admin => "admin",
                    shared_types::SpaceRole::Moderator => "moderator",
                    shared_types::SpaceRole::Member => "member",
                };
                SignalMessage::ChannelPermissionsChanged {
                    channel_id: channel_id.clone(),
                    min_role: role_str.to_string(),
                }
            }
            ChannelSetting::AutoDelete(hours) => {
                channel.auto_delete_hours = hours;
                SignalMessage::ChannelAutoDeleteChanged {
                    channel_id: channel_id.clone(),
                    auto_delete_hours: hours,
                }
            }
        }
    };

    // Broadcast to all space members including self
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

pub(crate) async fn handle_set_priority_speaker(
    state: &State,
    peer_id: &str,
    target_id: String,
    enabled: bool,
) {
    // Only Moderator+ can set priority speaker on others; anyone can set it on themselves
    let is_self = peer_id == target_id;
    if !is_self {
        if let Some((_space_id, _user_id, role)) =
            crate::handlers::space::peer_space_role(state, peer_id).await
        {
            if !role.has_at_least(shared_types::SpaceRole::Moderator) {
                send_error(
                    state,
                    peer_id,
                    "Moderator+ required to set priority speaker on others",
                )
                .await;
                return;
            }
        } else {
            return;
        }
    }

    let room_code = {
        let s = state.read().await;
        let Some(peer) = s.peers.get(peer_id) else {
            return;
        };
        peer.cached_room_code()
    };
    let Some(room_code) = room_code else { return };

    // Set the flag on target peer
    {
        let s = state.read().await;
        if let Some(target) = s.peers.get(&target_id) {
            target.is_priority_speaker.store(enabled, Ordering::Relaxed);
        }
    }

    let notify = SignalMessage::PrioritySpeakerChanged {
        peer_id: target_id,
        enabled,
    };
    // Broadcast to all in room
    let s = state.read().await;
    if let Some(room) = s.rooms.get(&room_code) {
        let peers: Vec<Arc<Peer>> = room
            .peer_ids
            .iter()
            .filter_map(|pid| s.peers.get(pid).cloned())
            .collect();
        drop(s);
        for peer in peers {
            send_to(&peer, &notify).await;
        }
    }
}

use crate::{send_error, send_to, validate_name};
use crate::{ChannelMeta, Db, Peer, Room, Space, State};
use shared_types::{
    ChannelInfo, ChannelType, MemberInfo, SignalMessage, SpaceAuditEntry, SpaceInfo, SpaceRole,
};
use std::sync::Arc;
use std::time::Instant;

use super::channel::handle_leave_channel;
use super::presence::notify_watchers_for_user;

pub async fn stable_peer_id(state: &State, peer_id: &str) -> String {
    let peer = {
        let s = state.read().await;
        s.peers.get(peer_id).cloned()
    };
    match peer {
        Some(peer) => peer
            .user_id
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| peer_id.to_string()),
        None => peer_id.to_string(),
    }
}

pub fn role_rank(role: SpaceRole) -> u8 {
    match role {
        SpaceRole::Owner => 3,
        SpaceRole::Admin => 2,
        SpaceRole::Moderator => 1,
        SpaceRole::Member => 0,
    }
}

pub fn can_manage_channels(role: SpaceRole) -> bool {
    matches!(role, SpaceRole::Owner | SpaceRole::Admin)
}

pub fn can_manage_members(role: SpaceRole) -> bool {
    matches!(
        role,
        SpaceRole::Owner | SpaceRole::Admin | SpaceRole::Moderator
    )
}

pub fn can_manage_roles(role: SpaceRole) -> bool {
    matches!(role, SpaceRole::Owner | SpaceRole::Admin)
}

pub fn can_view_audit(_role: SpaceRole) -> bool {
    true
}

pub fn role_for_identity(space: &Space, user_id: &str) -> SpaceRole {
    if space.owner_id == user_id {
        SpaceRole::Owner
    } else {
        space
            .member_roles
            .get(user_id)
            .copied()
            .unwrap_or(SpaceRole::Member)
    }
}

pub fn space_info_for_identity(space: &Space, user_id: &str) -> SpaceInfo {
    let self_role = role_for_identity(space, user_id);
    SpaceInfo {
        id: space.id.clone(),
        name: space.name.clone(),
        invite_code: space.invite_code.clone(),
        member_count: space.member_ids.len() as u32,
        channel_count: space.channels.len() as u32,
        is_owner: self_role == SpaceRole::Owner,
        self_role,
    }
}

pub async fn peer_space_role(state: &State, peer_id: &str) -> Option<(String, String, SpaceRole)> {
    let user_id = stable_peer_id(state, peer_id).await;
    let s = state.read().await;
    let peer = s.peers.get(peer_id)?;
    let space_id = peer.space_id.lock().await.clone()?;
    let space = s.spaces.get(&space_id)?;
    Some((
        space_id,
        user_id.clone(),
        role_for_identity(space, &user_id),
    ))
}

pub async fn resolve_space_member(
    state: &State,
    space_id: &str,
    member_ref: &str,
) -> Option<(String, String, String, Arc<Peer>)> {
    let candidates = {
        let s = state.read().await;
        let space = s.spaces.get(space_id)?;
        space
            .member_ids
            .iter()
            .filter_map(|member_id| s.peers.get(member_id).cloned())
            .collect::<Vec<_>>()
    };

    for peer in candidates {
        let user_id = peer
            .user_id
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| peer.id.clone());
        if peer.id == member_ref || user_id == member_ref {
            let name = peer.name.lock().await.clone();
            return Some((peer.id.clone(), user_id, name, peer));
        }
    }

    None
}

fn role_storage_key(role: SpaceRole) -> &'static str {
    match role {
        SpaceRole::Owner => "owner",
        SpaceRole::Admin => "admin",
        SpaceRole::Moderator => "moderator",
        SpaceRole::Member => "member",
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub async fn append_audit_entry(
    state: &State,
    db: &Db,
    space_id: &str,
    actor_user_id: &str,
    actor_name: &str,
    action: &str,
    target_user_id: Option<String>,
    target_name: Option<String>,
    detail: String,
) -> Option<SpaceAuditEntry> {
    let (entry, recipients) = {
        let mut s = state.write().await;
        let entry_id = s.alloc_audit_id();
        let entry = SpaceAuditEntry {
            id: entry_id.clone(),
            actor_name: actor_name.to_string(),
            action: action.to_string(),
            target_name: target_name.clone().unwrap_or_default(),
            detail: detail.clone(),
            timestamp: now_secs() as u64,
        };

        let member_ids = {
            let space = s.spaces.get_mut(space_id)?;
            space.audit_log.push_front(entry.clone());
            while space.audit_log.len() > crate::MAX_SPACE_AUDIT_ENTRIES {
                space.audit_log.pop_back();
            }
            space.member_ids.clone()
        };

        let recipients = member_ids
            .iter()
            .filter_map(|member_id| s.peers.get(member_id).cloned())
            .collect::<Vec<_>>();
        (entry, recipients)
    };

    if let Some(db) = db {
        let db = db.clone();
        let row = crate::persistence::AuditLogRow {
            id: entry.id.clone(),
            space_id: space_id.to_string(),
            actor_user_id: actor_user_id.to_string(),
            actor_name: actor_name.to_string(),
            action: action.to_string(),
            target_user_id,
            target_name,
            detail,
            created_at: entry.timestamp as i64,
        };
        tokio::task::spawn_blocking(move || {
            if let Err(err) = db.save_audit_log_entry(&row) {
                log::error!("Failed to persist audit log entry: {err}");
            }
        });
    }

    let notify = SignalMessage::SpaceAuditLogAppended {
        entry: entry.clone(),
    };
    for peer in recipients {
        send_to(&peer, &notify).await;
    }

    Some(entry)
}

pub async fn broadcast_to_space(
    state: &State,
    space_id: &str,
    exclude_id: &str,
    msg: &SignalMessage,
) {
    let s = state.read().await;
    if let Some(space) = s.spaces.get(space_id) {
        let members: Vec<Arc<Peer>> = space
            .member_ids
            .iter()
            .filter(|id| id.as_str() != exclude_id)
            .filter_map(|id| s.peers.get(id).cloned())
            .collect();
        drop(s);
        for peer in members {
            send_to(&peer, msg).await;
        }
    }
}

pub async fn handle_create_space(
    state: &State,
    peer_id: &str,
    name: String,
    user_name: String,
    db: &Db,
) {
    if let Err(e) = validate_name(&name) {
        send_error(state, peer_id, &e).await;
        return;
    }
    if let Err(e) = validate_name(&user_name) {
        send_error(state, peer_id, &e).await;
        return;
    }

    let owner_id = stable_peer_id(state, peer_id).await;
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
            active_screen_share_peer_id: None,
            created_at: Instant::now(),
        },
    );

    let channel_meta = ChannelMeta {
        id: channel_id.clone(),
        name: "General".into(),
        room_key: room_key.clone(),
        channel_type: ChannelType::Voice,
        topic: String::new(),
        voice_quality: 2, // High (64kbps) default
        user_limit: 0,
        category: String::new(),
        status: String::new(),
        slow_mode_secs: 0,
    };

    let space = Space {
        id: space_id.clone(),
        name: name.clone(),
        invite_code: invite_code.clone(),
        owner_id: owner_id.clone(),
        channels: vec![channel_meta],
        member_ids: vec![peer_id.to_string()],
        member_roles: std::collections::HashMap::from([(owner_id.clone(), SpaceRole::Owner)]),
        text_messages: std::collections::HashMap::new(),
        audit_log: std::collections::VecDeque::new(),
        slow_mode_timestamps: std::collections::HashMap::new(),
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
        self_role: SpaceRole::Owner,
    };

    let channels = vec![ChannelInfo {
        id: channel_id.clone(),
        name: "General".into(),
        peer_count: 0,
        channel_type: ChannelType::Voice,
        topic: String::new(),
        voice_quality: 2,
        user_limit: 0,
        category: String::new(),
        status: String::new(),
        slow_mode_secs: 0,
    }];

    log::info!("Space {} created by {peer_id}", space_id);

    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(
            &peer,
            &SignalMessage::SpaceCreated {
                space: space_info,
                channels,
            },
        )
        .await;
    } else {
        drop(s);
    }

    // Persist space and channel to DB
    if let Some(ref db) = db {
        let db = db.clone();
        let sid = space_id.clone();
        let sname = name;
        let sinvite = invite_code;
        let sowner = owner_id.clone();
        let cid = channel_id;
        let rk = room_key;
        let assigned_at = now_secs();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = db.save_space(&crate::persistence::SpaceRow {
                id: sid.clone(),
                name: sname,
                invite_code: sinvite,
                owner_id: sowner.clone(),
                created_at: assigned_at,
            }) {
                log::error!("Failed to persist space: {e}");
            }
            if let Err(e) = db.save_channel(&crate::persistence::ChannelRow {
                id: cid,
                space_id: sid.clone(),
                name: "General".into(),
                room_key: rk,
                channel_type: "voice".into(),
                topic: None,
                voice_quality: Some(2),
            }) {
                log::error!("Failed to persist channel: {e}");
            }
            if let Err(e) = db.save_space_role(&crate::persistence::SpaceRoleRow {
                space_id: sid,
                user_id: sowner,
                role: role_storage_key(SpaceRole::Owner).into(),
                assigned_at,
            }) {
                log::error!("Failed to persist owner role: {e}");
            }
        });
    }

    let _ = append_audit_entry(
        state,
        db,
        &space_id,
        &owner_id,
        user_name.trim(),
        "space",
        None,
        None,
        "Created the space".into(),
    )
    .await;
}

pub async fn handle_join_space(
    state: &State,
    peer_id: &str,
    invite_code: String,
    user_name: String,
    db: &Db,
) {
    if let Err(e) = validate_name(&user_name) {
        send_error(state, peer_id, &e).await;
        return;
    }

    // Brute-force protection: check if this IP has too many recent failed JoinSpace attempts
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            let ip = peer.ip;
            if let Some(&(count, window_start)) = s.join_failures.get(&ip) {
                if window_start.elapsed().as_secs() < 60 && count >= 5 {
                    drop(s);
                    send_error(state, peer_id, "Too many failed join attempts, try again later").await;
                    return;
                }
            }
        }
    }

    // Resolve space_id and check ban before taking write lock
    let (space_id, check_id) = {
        let s = state.read().await;
        let space_id = match s.invite_index.get(&invite_code).cloned() {
            Some(id) => id,
            None => {
                // Record failed attempt for brute-force protection
                if let Some(peer) = s.peers.get(peer_id) {
                    let ip = peer.ip;
                    drop(s);
                    {
                        let mut sw = state.write().await;
                        let entry = sw.join_failures.entry(ip).or_insert((0, Instant::now()));
                        if entry.1.elapsed().as_secs() >= 60 {
                            *entry = (1, Instant::now());
                        } else {
                            entry.0 += 1;
                        }
                    }
                    send_error(state, peer_id, "Invalid invite code").await;
                } else {
                    drop(s);
                    send_error(state, peer_id, "Invalid invite code").await;
                }
                return;
            }
        };
        let check_id = if let Some(peer) = s.peers.get(peer_id) {
            peer.user_id
                .lock()
                .await
                .clone()
                .unwrap_or_else(|| peer_id.to_string())
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
        let banned =
            tokio::task::spawn_blocking(move || db_clone.is_banned(&sid, &cid).unwrap_or(false))
                .await
                .unwrap_or(false);
        if banned {
            send_error(state, peer_id, "You are banned from this space").await;
            return;
        }
    }

    // Reset join failure counter on successful invite code match
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            let ip = peer.ip;
            drop(s);
            let mut sw = state.write().await;
            sw.join_failures.remove(&ip);
        }
    }

    let joiner_identity = stable_peer_id(state, peer_id).await;
    {
        let mut s = state.write().await;

        if let Some(peer) = s.peers.get(peer_id) {
            *peer.name.lock().await = user_name.trim().to_string();
            *peer.space_id.lock().await = Some(space_id.clone());
        }

        let new_user_id = if let Some(peer) = s.peers.get(peer_id) {
            peer.user_id.lock().await.clone()
        } else {
            None
        };
        let mut stale_ids = Vec::new();
        if let (Some(ref uid), Some(space)) = (&new_user_id, s.spaces.get(&space_id)) {
            for mid in &space.member_ids {
                if mid == peer_id {
                    continue;
                }
                if let Some(p) = s.peers.get(mid.as_str()) {
                    if p.user_id.lock().await.as_deref() == Some(uid.as_str()) {
                        stale_ids.push(mid.clone());
                    }
                } else {
                    stale_ids.push(mid.clone());
                }
            }
        }

        if let Some(space) = s.spaces.get_mut(&space_id) {
            for stale in &stale_ids {
                space.member_ids.retain(|id| id != stale);
            }
            if !space.member_ids.contains(&peer_id.to_string()) {
                space.member_ids.push(peer_id.to_string());
            }
        }
    }

    let (space_info, channels, members, joiner_peer, audit_log, member_info) = {
        let s = state.read().await;
        let Some(space) = s.spaces.get(&space_id) else {
            log::warn!("Space {space_id} disappeared before building response");
            return;
        };
        let space_info = space_info_for_identity(space, &joiner_identity);

        let channels: Vec<ChannelInfo> = space
            .channels
            .iter()
            .map(|ch| {
                let peer_count = s
                    .rooms
                    .get(&ch.room_key)
                    .map(|r| r.peer_ids.len() as u32)
                    .unwrap_or(0);
                ChannelInfo {
                    id: ch.id.clone(),
                    name: ch.name.clone(),
                    peer_count,
                    channel_type: ch.channel_type,
                    topic: ch.topic.clone(),
                    voice_quality: ch.voice_quality,
                    user_limit: ch.user_limit,
                    category: ch.category.clone(),
                    status: ch.status.clone(),
                    slow_mode_secs: ch.slow_mode_secs,
                }
            })
            .collect();

        let mut members = Vec::new();
        for mid in &space.member_ids {
            if let Some(p) = s.peers.get(mid) {
                let name = p.name.lock().await.clone();
                let stable_id = p
                    .user_id
                    .lock()
                    .await
                    .clone()
                    .unwrap_or_else(|| mid.clone());
                let (ch_id, ch_name) = space
                    .channels
                    .iter()
                    .find(|ch| {
                        s.rooms
                            .get(&ch.room_key)
                            .map(|r| r.peer_ids.contains(mid))
                            .unwrap_or(false)
                    })
                    .map(|ch| (Some(ch.id.clone()), Some(ch.name.clone())))
                    .unwrap_or((None, None));
                members.push(MemberInfo {
                    id: mid.clone(),
                    user_id: Some(stable_id.clone()),
                    name,
                    role: role_for_identity(space, &stable_id),
                    channel_id: ch_id,
                    channel_name: ch_name,
                    status: p.status.lock().await.clone(),
                    bio: String::new(),
                });
            }
        }

        let joiner_peer = s.peers.get(peer_id).cloned();
        let audit_log = if can_view_audit(space_info.self_role) {
            space.audit_log.iter().cloned().collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let member_info = if let Some(p) = s.peers.get(peer_id) {
            let user_id = p
                .user_id
                .lock()
                .await
                .clone()
                .unwrap_or_else(|| peer_id.to_string());
            Some(MemberInfo {
                id: peer_id.to_string(),
                user_id: Some(user_id.clone()),
                name: p.name.lock().await.clone(),
                role: role_for_identity(space, &user_id),
                channel_id: None,
                channel_name: None,
                status: p.status.lock().await.clone(),
                bio: String::new(),
            })
        } else {
            None
        };

        (
            space_info,
            channels,
            members,
            joiner_peer,
            audit_log,
            member_info,
        )
    };

    if let Some(peer) = joiner_peer {
        send_to(
            &peer,
            &SignalMessage::SpaceJoined {
                space: space_info,
                channels,
                members,
            },
        )
        .await;
        send_to(
            &peer,
            &SignalMessage::SpaceAuditLogSnapshot { entries: audit_log },
        )
        .await;
    }

    // Broadcast MemberOnline to other space members
    if let Some(member) = member_info {
        broadcast_to_space(
            state,
            &space_id,
            peer_id,
            &SignalMessage::MemberOnline { member },
        )
        .await;
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

    crate::handlers::chat::clear_typing_for_peer(state, peer_id).await;

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
    let notify = SignalMessage::MemberOffline {
        member_id: peer_id.to_string(),
    };
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

    let owner_identity = stable_peer_id(state, peer_id).await;

    // Check ownership and remove space atomically under write lock to prevent TOCTOU
    let member_peers: Vec<Arc<Peer>> = {
        let mut s = state.write().await;
        let is_owner = s
            .spaces
            .get(&space_id)
            .map(|space| space.owner_id == peer_id || space.owner_id == owner_identity)
            .unwrap_or(false);
        if !is_owner {
            drop(s);
            send_error(state, peer_id, "Only the space creator can delete it").await;
            return;
        }

        // Collect members to notify while we hold the lock
        let members: Vec<Arc<Peer>> = s
            .spaces
            .get(&space_id)
            .map(|space| {
                space
                    .member_ids
                    .iter()
                    .filter_map(|id| s.peers.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default();

        // Remove space, its rooms, and invite index entry
        if let Some(space) = s.spaces.remove(&space_id) {
            s.invite_index.remove(&space.invite_code);
            for ch in &space.channels {
                s.rooms.remove(&ch.room_key);
            }
        }

        members
    };

    // Clear space_id and room_code for all members, notify them
    let mut affected_user_ids = Vec::new();
    for peer in &member_peers {
        if let Some(user_id) = peer.user_id.lock().await.clone() {
            affected_user_ids.push(user_id);
        }
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

    for user_id in affected_user_ids {
        notify_watchers_for_user(state, &user_id).await;
    }
}

pub async fn handle_set_member_role(
    state: &State,
    peer_id: &str,
    target_user_id: String,
    role: SpaceRole,
    db: &Db,
) {
    let Some((space_id, actor_user_id, actor_role)) = peer_space_role(state, peer_id).await else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };

    if !can_manage_roles(actor_role) {
        send_error(state, peer_id, "You do not have permission to manage roles").await;
        return;
    }
    if actor_user_id == target_user_id {
        send_error(state, peer_id, "You cannot change your own role").await;
        return;
    }
    if role == SpaceRole::Owner {
        send_error(state, peer_id, "Owner role cannot be reassigned").await;
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

    let (changed, recipients) = {
        let mut s = state.write().await;
        let Some(space) = s.spaces.get_mut(&space_id) else {
            send_error(state, peer_id, "Space not found").await;
            return;
        };
        let current_role = role_for_identity(space, &target_user_id);
        if current_role == SpaceRole::Owner {
            drop(s);
            send_error(state, peer_id, "Owner role cannot be changed").await;
            return;
        }

        let allowed = match actor_role {
            SpaceRole::Owner => true,
            SpaceRole::Admin => {
                current_role != SpaceRole::Admin
                    && role_rank(role) <= role_rank(SpaceRole::Moderator)
            }
            _ => false,
        };
        if !allowed {
            drop(s);
            send_error(state, peer_id, "You cannot assign that role").await;
            return;
        }

        let changed = current_role != role;
        if changed {
            if role == SpaceRole::Member {
                space.member_roles.remove(&target_user_id);
            } else {
                space.member_roles.insert(target_user_id.clone(), role);
            }
        }

        let member_ids = space.member_ids.clone();
        let recipients = member_ids
            .iter()
            .filter_map(|member_id| s.peers.get(member_id).cloned())
            .collect::<Vec<_>>();
        (changed, recipients)
    };

    if !changed {
        return;
    }

    let target_name = resolve_space_member(state, &space_id, &target_user_id)
        .await
        .map(|(_, _, name, _)| name)
        .unwrap_or_else(|| target_user_id.clone());

    if let Some(db) = db {
        let db = db.clone();
        let space_id = space_id.clone();
        let target_user_id = target_user_id.clone();
        tokio::task::spawn_blocking(move || {
            let result = if role == SpaceRole::Member {
                db.delete_space_role(&space_id, &target_user_id).map(|_| ())
            } else {
                db.save_space_role(&crate::persistence::SpaceRoleRow {
                    space_id,
                    user_id: target_user_id,
                    role: role_storage_key(role).into(),
                    assigned_at: now_secs(),
                })
            };
            if let Err(err) = result {
                log::error!("Failed to persist role change: {err}");
            }
        });
    }

    let notify = SignalMessage::MemberRoleChanged {
        user_id: target_user_id.clone(),
        role,
    };
    for peer in recipients {
        send_to(&peer, &notify).await;
    }

    let detail = format!(
        "{} is now {}",
        target_name,
        match role {
            SpaceRole::Owner => "owner",
            SpaceRole::Admin => "admin",
            SpaceRole::Moderator => "moderator",
            SpaceRole::Member => "member",
        }
    );
    let _ = append_audit_entry(
        state,
        db,
        &space_id,
        &actor_user_id,
        &actor_name,
        "role",
        Some(target_user_id),
        Some(target_name),
        detail,
    )
    .await;
}

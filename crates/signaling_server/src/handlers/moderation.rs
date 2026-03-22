use crate::{send_error, send_to, Db, Peer, State};
use shared_types::{SignalMessage, SpaceRole};
use std::sync::Arc;

use super::presence::notify_watchers_for_user;
use super::space::{
    append_audit_entry, broadcast_to_space, can_manage_members, peer_space_role,
    resolve_space_member, role_for_identity, role_rank,
};

async fn actor_context(
    state: &State,
    peer_id: &str,
) -> Option<(String, String, SpaceRole, String)> {
    let (space_id, actor_user_id, actor_role) = peer_space_role(state, peer_id).await?;
    let peer = {
        let s = state.read().await;
        s.peers.get(peer_id).cloned()
    }?;
    let actor_name = peer.name.lock().await.clone();
    Some((space_id, actor_user_id, actor_role, actor_name))
}

fn can_moderate_target(actor_role: SpaceRole, target_role: SpaceRole) -> bool {
    can_manage_members(actor_role) && role_rank(actor_role) > role_rank(target_role)
}

async fn remove_member_from_space(
    state: &State,
    space_id: &str,
    member_peer: &Arc<Peer>,
    actual_member_id: &str,
    reason: &str,
) {
    {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(space_id) {
            space.member_ids.retain(|id| id != actual_member_id);
        }
    }

    member_peer.set_room_code(None).await;
    *member_peer.space_id.lock().await = None;

    send_to(
        member_peer,
        &SignalMessage::Kicked {
            reason: reason.to_string(),
        },
    )
    .await;

    broadcast_to_space(
        state,
        space_id,
        actual_member_id,
        &SignalMessage::MemberOffline {
            member_id: actual_member_id.to_string(),
        },
    )
    .await;
}

pub async fn handle_kick_member(state: &State, peer_id: &str, member_id: String, db: &Db) {
    let Some((space_id, actor_user_id, actor_role, actor_name)) =
        actor_context(state, peer_id).await
    else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };
    if !can_manage_members(actor_role) {
        send_error(state, peer_id, "You do not have permission to kick members").await;
        return;
    }

    let Some((actual_member_id, target_user_id, target_name, member_peer)) =
        resolve_space_member(state, &space_id, &member_id).await
    else {
        send_error(state, peer_id, "Member not found").await;
        return;
    };

    if target_user_id == actor_user_id {
        send_error(state, peer_id, "Cannot kick yourself").await;
        return;
    }

    let target_role = {
        let s = state.read().await;
        s.spaces
            .get(&space_id)
            .map(|space| role_for_identity(space, &target_user_id))
            .unwrap_or(SpaceRole::Member)
    };
    if !can_moderate_target(actor_role, target_role) {
        send_error(state, peer_id, "You cannot kick that member").await;
        return;
    }

    remove_member_from_space(
        state,
        &space_id,
        &member_peer,
        &actual_member_id,
        "You have been kicked from the space",
    )
    .await;

    log::info!("Peer {actual_member_id} kicked from space {space_id} by {peer_id}");
    notify_watchers_for_user(state, &target_user_id).await;
    let _ = append_audit_entry(
        state,
        db,
        &space_id,
        &actor_user_id,
        &actor_name,
        "kick",
        Some(target_user_id),
        Some(target_name),
        "Removed the member from the space".into(),
    )
    .await;
}

pub async fn handle_mute_member(
    state: &State,
    peer_id: &str,
    member_id: String,
    muted: bool,
    db: &Db,
) {
    let Some((space_id, actor_user_id, actor_role, actor_name)) =
        actor_context(state, peer_id).await
    else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };
    if !can_manage_members(actor_role) {
        send_error(state, peer_id, "You do not have permission to mute members").await;
        return;
    }

    let Some((actual_member_id, target_user_id, target_name, member_peer)) =
        resolve_space_member(state, &space_id, &member_id).await
    else {
        send_error(state, peer_id, "Member not found").await;
        return;
    };

    if target_user_id == actor_user_id {
        send_error(state, peer_id, "Cannot mute yourself").await;
        return;
    }

    let target_role = {
        let s = state.read().await;
        s.spaces
            .get(&space_id)
            .map(|space| role_for_identity(space, &target_user_id))
            .unwrap_or(SpaceRole::Member)
    };
    if !can_moderate_target(actor_role, target_role) {
        send_error(state, peer_id, "You cannot mute that member").await;
        return;
    }

    member_peer
        .is_muted
        .store(muted, std::sync::atomic::Ordering::Relaxed);

    let notify = SignalMessage::MemberMuted {
        member_id: actual_member_id.clone(),
        muted,
    };
    broadcast_to_space(state, &space_id, "", &notify).await;

    log::info!(
        "Peer {actual_member_id} {} in space {space_id} by {peer_id}",
        if muted {
            "server-muted"
        } else {
            "server-unmuted"
        }
    );

    let detail = if muted {
        "Muted the member in the space"
    } else {
        "Unmuted the member in the space"
    };
    let _ = append_audit_entry(
        state,
        db,
        &space_id,
        &actor_user_id,
        &actor_name,
        "mute",
        Some(target_user_id),
        Some(target_name),
        detail.into(),
    )
    .await;
}

pub async fn handle_ban_member(state: &State, peer_id: &str, member_id: String, db: &Db) {
    let Some((space_id, actor_user_id, actor_role, actor_name)) =
        actor_context(state, peer_id).await
    else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };
    if !can_manage_members(actor_role) {
        send_error(state, peer_id, "You do not have permission to ban members").await;
        return;
    }

    let Some((actual_member_id, target_user_id, target_name, member_peer)) =
        resolve_space_member(state, &space_id, &member_id).await
    else {
        send_error(state, peer_id, "Member not found").await;
        return;
    };

    if target_user_id == actor_user_id {
        send_error(state, peer_id, "Cannot ban yourself").await;
        return;
    }

    let target_role = {
        let s = state.read().await;
        s.spaces
            .get(&space_id)
            .map(|space| role_for_identity(space, &target_user_id))
            .unwrap_or(SpaceRole::Member)
    };
    if !can_moderate_target(actor_role, target_role) {
        send_error(state, peer_id, "You cannot ban that member").await;
        return;
    }

    if let Some(db) = db {
        let db = db.clone();
        let sid = space_id.clone();
        let banned_user_id = target_user_id.clone();
        let persist_result = tokio::task::spawn_blocking(move || {
            db.save_ban(&crate::persistence::BanRow {
                space_id: sid,
                user_id: banned_user_id,
                banned_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            })
        })
        .await;

        match persist_result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                log::error!("Failed to persist ban: {err}");
                send_error(state, peer_id, "Failed to ban member").await;
                return;
            }
            Err(err) => {
                log::error!("Failed to join ban task: {err}");
                send_error(state, peer_id, "Failed to ban member").await;
                return;
            }
        }
    }

    remove_member_from_space(
        state,
        &space_id,
        &member_peer,
        &actual_member_id,
        "You have been banned from the space",
    )
    .await;

    log::info!("Peer {actual_member_id} banned from space {space_id} by {peer_id}");
    notify_watchers_for_user(state, &target_user_id).await;
    let _ = append_audit_entry(
        state,
        db,
        &space_id,
        &actor_user_id,
        &actor_name,
        "ban",
        Some(target_user_id),
        Some(target_name),
        "Banned the member from the space".into(),
    )
    .await;
}

pub async fn handle_unban_member(state: &State, peer_id: &str, user_id: String, db: &Db) {
    let Some((space_id, actor_user_id, actor_role, actor_name)) =
        actor_context(state, peer_id).await
    else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };
    if !can_manage_members(actor_role) {
        send_error(state, peer_id, "You do not have permission to unban members").await;
        return;
    }

    let Some(db_arc) = db else {
        send_error(state, peer_id, "Persistence required for ban management").await;
        return;
    };

    let db_clone = db_arc.clone();
    let sid = space_id.clone();
    let uid = user_id.clone();
    let result = tokio::task::spawn_blocking(move || db_clone.delete_ban(&sid, &uid))
        .await
        .unwrap_or_else(|_| Err("Unban task failed".into()));

    match result {
        Ok(true) => {
            log::info!("User {user_id} unbanned from space {space_id} by {peer_id}");
            let _ = append_audit_entry(
                state,
                db,
                &space_id,
                &actor_user_id,
                &actor_name,
                "unban",
                Some(user_id),
                None,
                "Unbanned the user from the space".into(),
            )
            .await;
        }
        Ok(false) => {
            send_error(state, peer_id, "User is not banned").await;
        }
        Err(msg) => {
            send_error(state, peer_id, &msg).await;
        }
    }
}

pub async fn handle_list_bans(state: &State, peer_id: &str, db: &Db) {
    let Some((space_id, _actor_user_id, actor_role, _actor_name)) =
        actor_context(state, peer_id).await
    else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };
    if !can_manage_members(actor_role) {
        send_error(state, peer_id, "You do not have permission to view bans").await;
        return;
    }

    let Some(db) = db else {
        send_error(state, peer_id, "Persistence required for ban management").await;
        return;
    };

    let db_clone = db.clone();
    let sid = space_id;
    let result = tokio::task::spawn_blocking(move || db_clone.load_bans(&sid))
        .await
        .unwrap_or_else(|_| Err("Ban list task failed".into()));

    match result {
        Ok(bans) => {
            let ban_infos: Vec<shared_types::BanInfo> = bans
                .into_iter()
                .map(|b| shared_types::BanInfo {
                    user_id: b.user_id,
                    user_name: String::new(),
                    banned_at: b.banned_at as u64,
                })
                .collect();
            let peer = {
                let s = state.read().await;
                s.peers.get(peer_id).cloned()
            };
            if let Some(peer) = peer {
                send_to(&peer, &SignalMessage::BanList { bans: ban_infos }).await;
            }
        }
        Err(msg) => {
            send_error(state, peer_id, &msg).await;
        }
    }
}

pub async fn handle_block_user(state: &State, peer_id: &str, user_id: String, db: &Db) {
    let Some(db) = db else {
        send_error(state, peer_id, "Persistence required").await;
        return;
    };
    let Some(current_user_id) = super::chat::authenticated_user_id_pub(state, peer_id).await
    else {
        send_error(state, peer_id, "Authenticate first").await;
        return;
    };

    let db_clone = db.clone();
    let blocker = current_user_id.clone();
    let blocked = user_id.clone();
    let result =
        tokio::task::spawn_blocking(move || db_clone.save_user_block(&blocker, &blocked))
            .await
            .unwrap_or_else(|_| Err("Block task failed".into()));

    match result {
        Ok(()) => {
            let peer = {
                let s = state.read().await;
                s.peers.get(peer_id).cloned()
            };
            if let Some(peer) = peer {
                send_to(&peer, &SignalMessage::UserBlocked { user_id }).await;
            }
        }
        Err(msg) => {
            send_error(state, peer_id, &msg).await;
        }
    }
}

pub async fn handle_unblock_user(state: &State, peer_id: &str, user_id: String, db: &Db) {
    let Some(db) = db else {
        send_error(state, peer_id, "Persistence required").await;
        return;
    };
    let Some(current_user_id) = super::chat::authenticated_user_id_pub(state, peer_id).await
    else {
        send_error(state, peer_id, "Authenticate first").await;
        return;
    };

    let db_clone = db.clone();
    let blocker = current_user_id;
    let blocked = user_id.clone();
    let result =
        tokio::task::spawn_blocking(move || db_clone.delete_user_block(&blocker, &blocked))
            .await
            .unwrap_or_else(|_| Err("Unblock task failed".into()));

    match result {
        Ok(()) => {
            let peer = {
                let s = state.read().await;
                s.peers.get(peer_id).cloned()
            };
            if let Some(peer) = peer {
                send_to(&peer, &SignalMessage::UserUnblocked { user_id }).await;
            }
        }
        Err(msg) => {
            send_error(state, peer_id, &msg).await;
        }
    }
}

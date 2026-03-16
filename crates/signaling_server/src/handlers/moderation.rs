use crate::{send_error, send_to, Db, Peer, State};
use shared_types::SignalMessage;
use std::sync::Arc;

use super::presence::notify_watchers_for_user;
use super::space::broadcast_to_space;

/// Get the space_id of a peer and verify they are the space owner.
async fn verify_owner(state: &State, peer_id: &str) -> Option<String> {
    let s = state.read().await;
    let peer = s.peers.get(peer_id)?;
    let space_id = peer.space_id.lock().await.clone()?;
    let space = s.spaces.get(&space_id)?;
    if space.owner_id == peer_id {
        Some(space_id)
    } else {
        None
    }
}

pub async fn handle_kick_member(state: &State, peer_id: &str, member_id: String) {
    let Some(space_id) = verify_owner(state, peer_id).await else {
        send_error(state, peer_id, "Only the space owner can kick members").await;
        return;
    };

    if member_id == peer_id {
        send_error(state, peer_id, "Cannot kick yourself").await;
        return;
    }

    // Get the member's peer
    let member_peer: Option<Arc<Peer>> = {
        let s = state.read().await;
        s.peers.get(&member_id).cloned()
    };

    let Some(member_peer) = member_peer else {
        return;
    };
    let member_user_id = member_peer.user_id.lock().await.clone();

    // Remove from space
    {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            space.member_ids.retain(|id| id != &member_id);
        }
    }

    // Clear their room/space state
    member_peer.set_room_code(None).await;
    *member_peer.space_id.lock().await = None;

    // Notify kicked member
    send_to(
        &member_peer,
        &SignalMessage::Kicked {
            reason: "You have been kicked from the space".into(),
        },
    )
    .await;

    // Broadcast to remaining members
    broadcast_to_space(
        state,
        &space_id,
        &member_id,
        &SignalMessage::MemberOffline {
            member_id: member_id.clone(),
        },
    )
    .await;

    log::info!("Peer {member_id} kicked from space {space_id} by {peer_id}");
    if let Some(user_id) = member_user_id {
        notify_watchers_for_user(state, &user_id).await;
    }
}

pub async fn handle_mute_member(state: &State, peer_id: &str, member_id: String, muted: bool) {
    let Some(space_id) = verify_owner(state, peer_id).await else {
        send_error(state, peer_id, "Only the space owner can mute members").await;
        return;
    };

    // Set the member's mute state
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(&member_id) {
            peer.is_muted
                .store(muted, std::sync::atomic::Ordering::Relaxed);
        }
    }

    // Broadcast to all space members
    let notify = SignalMessage::MemberMuted {
        member_id: member_id.clone(),
        muted,
    };
    broadcast_to_space(state, &space_id, "", &notify).await;

    log::info!(
        "Peer {member_id} {} in space {space_id} by {peer_id}",
        if muted {
            "server-muted"
        } else {
            "server-unmuted"
        }
    );
}

pub async fn handle_ban_member(state: &State, peer_id: &str, member_id: String, db: &Db) {
    let Some(space_id) = verify_owner(state, peer_id).await else {
        send_error(state, peer_id, "Only the space owner can ban members").await;
        return;
    };

    if member_id == peer_id {
        send_error(state, peer_id, "Cannot ban yourself").await;
        return;
    }

    // Persist ban
    if let Some(ref db) = db {
        let db = db.clone();
        let sid = space_id.clone();
        let mid = member_id.clone();
        tokio::task::spawn_blocking(move || {
            let _ = db.save_ban(&crate::persistence::BanRow {
                space_id: sid,
                user_id: mid,
                banned_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            });
        });
    }

    // Kick the member (reuse kick logic)
    handle_kick_member(state, peer_id, member_id).await;
}

use crate::connection::send_error;
use crate::handlers::space::{
    can_manage_members, peer_space_role, resolve_space_member, role_for_identity, role_rank,
};
use crate::types::{Db, State};
use crate::validation::now_epoch_secs;
use shared_types::{SignalMessage, SpaceRole};
use std::sync::atomic::Ordering;

pub(crate) async fn handle_timeout_member(
    state: &State,
    peer_id: &str,
    member_id: String,
    duration_secs: u64,
    db: &Db,
) {
    let Some((space_id, actor_user_id, actor_role)) = peer_space_role(state, peer_id).await else {
        return;
    };
    if !can_manage_members(actor_role) {
        send_error(
            state,
            peer_id,
            "Insufficient permissions to timeout members",
        )
        .await;
        return;
    }

    // Resolve the target within the actor's own space. Using the global peer map
    // directly would let a moderator time out any connected user in any space.
    let Some((_actual_member_id, target_uid, target_name, target_peer)) =
        resolve_space_member(state, &space_id, &member_id).await
    else {
        send_error(state, peer_id, "Member not found").await;
        return;
    };

    if target_uid == actor_user_id {
        send_error(state, peer_id, "Cannot time out yourself").await;
        return;
    }

    // Rank check: a moderator may not time out an equal or higher-ranked member
    // (e.g. an admin or the owner).
    let target_role = {
        let s = state.read().await;
        s.spaces
            .get(&space_id)
            .map(|space| role_for_identity(space, &target_uid))
            .unwrap_or(SpaceRole::Member)
    };
    if !(can_manage_members(actor_role) && role_rank(actor_role) > role_rank(target_role)) {
        send_error(state, peer_id, "You cannot time out that member").await;
        return;
    }

    // Cap duration at 28 days
    let duration_secs = duration_secs.min(28 * 24 * 3600);
    let until_epoch = now_epoch_secs() + duration_secs;

    target_peer
        .timeout_until
        .store(until_epoch, Ordering::Relaxed);

    let actor_name = {
        let s = state.read().await;
        if let Some(p) = s.peers.get(peer_id) {
            p.name.lock().await.clone()
        } else {
            "Unknown".into()
        }
    };

    // Persist the timeout so it survives reconnects and server restarts.
    if let Some(db) = db {
        let db = db.clone();
        let sid = space_id.clone();
        let actor_uid = actor_user_id.clone();
        let target_uid = target_uid.clone();
        let until_signed = until_epoch as i64;
        let now_signed = now_epoch_secs() as i64;
        tokio::task::spawn_blocking(move || {
            db.upsert_timeout(&sid, &target_uid, until_signed, &actor_uid, now_signed);
        });
    }

    // Broadcast timeout to space
    let notify = SignalMessage::MemberTimedOut {
        member_id: member_id.clone(),
        until_epoch,
    };
    crate::handlers::broadcast_to_space(state, &space_id, "", &notify).await;

    let duration_str = if duration_secs >= 3600 {
        format!("{}h", duration_secs / 3600)
    } else if duration_secs >= 60 {
        format!("{}m", duration_secs / 60)
    } else {
        format!("{}s", duration_secs)
    };

    let _ = crate::handlers::space::append_audit_entry(
        state,
        db,
        &space_id,
        &actor_user_id,
        &actor_name,
        "timeout",
        Some(member_id),
        Some(target_name),
        format!("Timed out for {duration_str}"),
    )
    .await;
}

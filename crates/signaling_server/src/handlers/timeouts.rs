use std::sync::atomic::Ordering;
use shared_types::SignalMessage;
use crate::types::{State, Db};
use crate::connection::send_error;
use crate::validation::now_epoch_secs;

pub(crate) async fn handle_timeout_member(
    state: &State,
    peer_id: &str,
    member_id: String,
    duration_secs: u64,
    db: &Db,
) {
    let Some((space_id, actor_user_id, actor_role)) =
        crate::handlers::space::peer_space_role(state, peer_id).await
    else {
        return;
    };
    if !crate::handlers::space::can_manage_members(actor_role) {
        send_error(
            state,
            peer_id,
            "Insufficient permissions to timeout members",
        )
        .await;
        return;
    }

    // Cap duration at 28 days
    let duration_secs = duration_secs.min(28 * 24 * 3600);
    let until_epoch = now_epoch_secs() + duration_secs;

    let (target_peer, actor_name) = {
        let s = state.read().await;
        let target = s.peers.get(&member_id).cloned();
        let actor_name = if let Some(p) = s.peers.get(peer_id) {
            p.name.lock().await.clone()
        } else {
            "Unknown".into()
        };
        (target, actor_name)
    };

    if let Some(ref target) = target_peer {
        target.timeout_until.store(until_epoch, Ordering::Relaxed);
    }

    let target_name = if let Some(ref target) = target_peer {
        target.name.lock().await.clone()
    } else {
        member_id.clone()
    };

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

use shared_types::SignalMessage;
use crate::types::{State, Db};
use crate::connection::{send_to, send_error};

pub(crate) async fn handle_schedule_message(
    state: &State,
    peer_id: &str,
    channel_id: String,
    content: String,
    send_at: i64,
    db: &Db,
) {
    if content.len() > 2000 {
        send_error(
            state,
            peer_id,
            "Message content too long (max 2000 characters)",
        )
        .await;
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    if send_at <= now {
        send_error(state, peer_id, "Scheduled time must be in the future").await;
        return;
    }
    let s = state.read().await;
    let peer = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    let space_id = peer.space_id.lock().await.clone().unwrap_or_default();
    let sender_name = peer.name.lock().await.clone();
    let user_id = peer
        .user_id
        .lock()
        .await
        .clone()
        .unwrap_or_else(|| peer_id.to_string());
    let db = match db {
        Some(db) => db,
        None => return,
    };
    let schedule_id = {
        use rand::RngCore;
        let mut buf = [0u8; 4];
        rand::rngs::OsRng.fill_bytes(&mut buf);
        format!("sched_{:08x}", u32::from_le_bytes(buf))
    };
    drop(s);
    let _ = db.schedule_message(
        &schedule_id,
        &space_id,
        &channel_id,
        &user_id,
        &sender_name,
        &content,
        send_at,
    );
    let s = state.read().await;
    if let Some(p) = s.peers.get(peer_id).cloned() {
        send_to(
            &p,
            &SignalMessage::MessageScheduled {
                schedule_id,
                channel_id,
                content,
                send_at,
            },
        )
        .await;
    }
}

pub(crate) async fn handle_cancel_scheduled_message(
    state: &State,
    peer_id: &str,
    schedule_id: String,
    db: &Db,
) {
    let db = match db {
        Some(db) => db,
        None => return,
    };
    // Verify the caller owns this scheduled message
    let s = state.read().await;
    let peer = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    let user_id_opt = peer.user_id.lock().await.clone();
    drop(s);
    let user_id = match user_id_opt {
        Some(id) => id,
        None => {
            send_error(state, peer_id, "Not authenticated").await;
            return;
        }
    };
    let db_clone = db.clone();
    let sid = schedule_id.clone();
    let owner = tokio::task::spawn_blocking(move || db_clone.get_scheduled_message_sender(&sid))
        .await
        .unwrap_or(Err("task failed".into()));
    match owner {
        Ok(Some(sender_id)) if sender_id == user_id => {}
        Ok(Some(_)) => {
            send_error(
                state,
                peer_id,
                "You can only cancel your own scheduled messages",
            )
            .await;
            return;
        }
        Ok(None) => {
            send_error(state, peer_id, "Scheduled message not found").await;
            return;
        }
        Err(_) => return,
    }
    let _ = db.cancel_scheduled_message(&schedule_id);
    let s = state.read().await;
    if let Some(p) = s.peers.get(peer_id).cloned() {
        send_to(
            &p,
            &SignalMessage::ScheduledMessageCancelled { schedule_id },
        )
        .await;
    }
}

pub(crate) async fn handle_set_welcome_message(state: &State, peer_id: &str, message: String, db: &Db) {
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
    if !role.has_at_least(shared_types::SpaceRole::Admin) {
        drop(s);
        send_error(state, peer_id, "Admin+ required").await;
        return;
    }
    let members: Vec<_> = space.member_ids.iter().cloned().collect();
    let peers_map: Vec<_> = members
        .iter()
        .filter_map(|mid| s.peers.get(mid).cloned())
        .collect();
    drop(s);
    if let Some(db) = db {
        let _ = db.set_welcome_message(&space_id, &message);
    }
    let msg = SignalMessage::WelcomeMessageChanged { message };
    for p in &peers_map {
        send_to(p, &msg).await;
    }
}

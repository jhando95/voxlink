use crate::connection::{send_error, send_to};
use crate::types::{Db, State};
use shared_types::{Permissions, SignalMessage};

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
    let _ = db.schedule_message(&crate::persistence::NewScheduledMessage {
        id: &schedule_id,
        space_id: &space_id,
        channel_id: &channel_id,
        sender_id: &user_id,
        sender_name: &sender_name,
        content: &content,
        send_at,
    });
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
        );
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
        );
    }
}

pub(crate) async fn handle_set_welcome_message(
    state: &State,
    peer_id: &str,
    message: String,
    db: &Db,
) {
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
    let perms = Permissions::from_bits(peer.space_perms.load(std::sync::atomic::Ordering::Relaxed));
    if !perms.has(Permissions::MANAGE_SPACE) {
        drop(s);
        send_error(
            state,
            peer_id,
            "You do not have permission to set the welcome message",
        )
        .await;
        return;
    }
    let members: Vec<_> = space.member_ids.to_vec();
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
        send_to(p, &msg);
    }
}

use shared_types::SignalMessage;
use crate::types::{State, Db};
use crate::connection::{send_to, send_error};
use crate::DB_TIMEOUT;

pub(crate) async fn handle_set_display_name(state: &State, peer_id: &str, name: String, db: &Db) {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() || trimmed.len() > 32 {
        send_error(state, peer_id, "Name must be 1-32 characters").await;
        return;
    }
    let s = state.read().await;
    let peer = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    let user_id = match peer.user_id.lock().await.clone() {
        Some(id) => id,
        None => {
            drop(s);
            send_error(state, peer_id, "Not authenticated").await;
            return;
        }
    };
    drop(s);
    *peer.name.lock().await = trimmed.clone();
    if let Some(db) = db {
        let _ = db.update_display_name(&user_id, &trimmed);
    }
    let s = state.read().await;
    if let Some(p) = s.peers.get(peer_id).cloned() {
        send_to(
            &p,
            &SignalMessage::DisplayNameChanged {
                user_id,
                name: trimmed,
            },
        )
        .await;
    }
}

pub(crate) async fn handle_delete_account(state: &State, peer_id: &str, db: &Db) {
    let s = state.read().await;
    let peer = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    let user_id = match peer.user_id.lock().await.clone() {
        Some(id) => id,
        None => {
            drop(s);
            send_error(state, peer_id, "Not authenticated").await;
            return;
        }
    };
    drop(s);
    if let Some(db) = db {
        let _ = db.delete_user(&user_id);
    }
    let s = state.read().await;
    if let Some(p) = s.peers.get(peer_id).cloned() {
        send_to(&p, &SignalMessage::AccountDeleted).await;
    }
}

pub(crate) async fn handle_set_user_status(state: &State, peer_id: &str, status: String, db: &Db) {
    let status = status.chars().take(128).collect::<String>();

    let (space_id, user_id) = {
        let s = state.read().await;
        let Some(peer) = s.peers.get(peer_id) else {
            return;
        };
        *peer.status.lock().await = status.clone();
        let space_id = peer.space_id.lock().await.clone();
        let user_id = peer.user_id.lock().await.clone();
        (space_id, user_id)
    };

    // Persist status to DB if authenticated
    if let (Some(db), Some(uid)) = (db, user_id) {
        let db = db.clone();
        let status_clone = status.clone();
        match tokio::time::timeout(
            DB_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                db.set_user_status(&uid, &status_clone);
            }),
        )
        .await
        {
            Err(_) => log::warn!("DB timeout: set_user_status for peer {peer_id}"),
            Ok(Err(e)) => log::warn!("DB task panicked in set_user_status: {e}"),
            Ok(Ok(())) => {}
        }
    }

    // Broadcast to space members
    if let Some(space_id) = space_id {
        let notify = SignalMessage::UserStatusChanged {
            member_id: peer_id.to_string(),
            status,
        };
        crate::handlers::broadcast_to_space(state, &space_id, peer_id, &notify).await;
    }
}

pub(crate) async fn handle_set_profile(state: &State, peer_id: &str, bio: String, db: &Db) {
    let bio = bio.chars().take(256).collect::<String>();

    let (space_id, user_id) = {
        let s = state.read().await;
        let Some(peer) = s.peers.get(peer_id) else {
            return;
        };
        let space_id = peer.space_id.lock().await.clone();
        let user_id = peer.user_id.lock().await.clone();
        (space_id, user_id)
    };

    // Persist bio to DB
    if let (Some(db), Some(uid)) = (db, &user_id) {
        let db = db.clone();
        let uid = uid.clone();
        let bio_clone = bio.clone();
        match tokio::time::timeout(
            DB_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                db.set_user_bio(&uid, &bio_clone);
            }),
        )
        .await
        {
            Err(_) => log::warn!("DB timeout: set_user_bio for peer {peer_id}"),
            Ok(Err(e)) => log::warn!("DB task panicked in set_user_bio: {e}"),
            Ok(Ok(())) => {}
        }
    }

    // Broadcast to space members
    if let Some(space_id) = space_id {
        let user_id_str = user_id.unwrap_or_else(|| peer_id.to_string());
        let notify = SignalMessage::ProfileUpdated {
            user_id: user_id_str,
            bio,
        };
        crate::handlers::broadcast_to_space(state, &space_id, peer_id, &notify).await;
    }
}

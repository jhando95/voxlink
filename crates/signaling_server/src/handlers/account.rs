use crate::connection::{send_error, send_to};
use crate::types::{Db, State};
use crate::DB_TIMEOUT;
use shared_types::SignalMessage;

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
    let space_id = peer.space_id.lock().await.clone();
    let room_code = peer.cached_room_code();
    drop(s);
    *peer.name.lock().await = trimmed.clone();
    if let Some(db) = db {
        let _ = db.update_display_name(&user_id, &trimmed);
    }

    let notify = SignalMessage::DisplayNameChanged {
        user_id: user_id.clone(),
        name: trimmed.clone(),
    };

    // Self acknowledgement so the local UI updates immediately.
    {
        let s = state.read().await;
        if let Some(p) = s.peers.get(peer_id).cloned() {
            drop(s);
            send_to(&p, &notify);
        }
    }

    // Broadcast to the user's current space (so the member list updates) and
    // current voice room (so room peers see the new label).
    if let Some(sid) = space_id.as_deref() {
        crate::handlers::broadcast_to_space(state, sid, peer_id, &notify).await;
    }
    if let Some(rc) = room_code.as_deref() {
        let recipients: Vec<std::sync::Arc<crate::Peer>> = {
            let s = state.read().await;
            s.rooms
                .get(rc)
                .map(|r| {
                    r.peer_ids
                        .iter()
                        .filter(|pid| pid.as_str() != peer_id)
                        .filter_map(|pid| s.peers.get(pid).cloned())
                        .collect()
                })
                .unwrap_or_default()
        };
        for p in recipients {
            send_to(&p, &notify);
        }
    }
    // And any friends watching this user's presence.
    super::presence::notify_watchers_for_user(state, &user_id).await;
}

pub(crate) async fn handle_delete_account(
    state: &State,
    peer_id: &str,
    current_password: String,
    db: &Db,
) {
    // Rate-limit deletion attempts the same way we rate-limit auth — a stolen token
    // alone must not be enough to wipe an account, and brute-forcing the password
    // through this handler should be no faster than against login.
    if !super::auth::check_auth_rate_limit(state, peer_id).await {
        send_error(state, peer_id, "Too many attempts. Try again in a minute.").await;
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

    // Require current password — protects against a stolen session token being
    // enough to destroy the account.
    let Some(db_ref) = db.as_ref() else {
        send_error(state, peer_id, "Account system unavailable").await;
        return;
    };
    let db_clone = db_ref.clone();
    let uid_for_hash = user_id.clone();
    let stored_hash = match tokio::time::timeout(
        crate::DB_TIMEOUT,
        tokio::task::spawn_blocking(move || db_clone.get_password_hash(&uid_for_hash)),
    )
    .await
    {
        Ok(Ok(Ok(Some(h)))) => h,
        _ => {
            send_error(state, peer_id, "Could not verify password").await;
            return;
        }
    };
    if !super::auth::verify_password(&current_password, &stored_hash) {
        send_error(state, peer_id, "Current password is incorrect").await;
        return;
    }

    if let Some(db) = db {
        let _ = db.delete_user(&user_id);
    }
    let s = state.read().await;
    if let Some(p) = s.peers.get(peer_id).cloned() {
        send_to(&p, &SignalMessage::AccountDeleted);
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

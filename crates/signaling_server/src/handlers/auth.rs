use crate::{send_error, send_to, Db, State};
use rand::rngs::OsRng;
use rand::RngCore;
use shared_types::SignalMessage;

use super::friends::send_friend_snapshot_to_peer;

pub async fn handle_authenticate(
    state: &State,
    peer_id: &str,
    token: Option<String>,
    user_name: String,
    db: &Db,
) -> bool {
    let user_name = user_name.trim().to_string();
    if user_name.is_empty() || user_name.len() > 32 {
        send_error(
            state,
            peer_id,
            "Display name must be between 1 and 32 characters",
        )
        .await;
        return false;
    }

    // Set the peer's display name
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            *peer.name.lock().await = user_name.clone();
        }
    }

    let Some(ref db_ref) = db else {
        // No DB — just acknowledge with a transient token
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id).cloned() {
            drop(s);
            send_to(
                &peer,
                &SignalMessage::Authenticated {
                    token: String::new(),
                    user_id: peer_id.to_string(),
                },
            )
            .await;
        }
        return true;
    };

    // Try to restore identity from existing token
    if let Some(ref tok) = token {
        if !tok.is_empty() {
            let db_clone = db_ref.clone();
            let tok_clone = tok.clone();
            let found = match tokio::time::timeout(crate::DB_TIMEOUT, tokio::task::spawn_blocking(move || {
                db_clone.find_user_by_token(&tok_clone).unwrap_or(None)
            })).await {
                Ok(result) => result.unwrap_or(None),
                Err(_) => {
                    // DB timeout — return error instead of creating a new identity,
                    // which would cause the user to lose their previous identity.
                    log::warn!("DB timeout: find_user_by_token for peer {peer_id}");
                    send_error(state, peer_id, "Authentication is temporarily unavailable (DB timeout)").await;
                    return false;
                }
            };

            if let Some(user) = found {
                let rotated_token = generate_token();
                let rotated_name = user_name.clone();
                let user_id = user.user_id.clone();
                let now = unix_now_secs();
                let db_clone = db_ref.clone();
                let uid = user_id.clone();
                let tok = rotated_token.clone();
                let name = rotated_name.clone();
                let rotate_result = tokio::time::timeout(crate::DB_TIMEOUT, tokio::task::spawn_blocking(move || {
                    db_clone.rotate_user_session(&uid, &tok, &name, now, now)
                }))
                .await;

                let token_to_send = match rotate_result {
                    Ok(Ok(Ok(()))) => rotated_token,
                    Ok(Ok(Err(e))) => {
                        log::error!("Failed to rotate session token for {user_id}: {e}");
                        user.token
                    }
                    Ok(Err(e)) => {
                        log::error!("Failed to join token rotation task for {user_id}: {e}");
                        user.token
                    }
                    Err(_) => {
                        log::warn!("DB timeout: rotate_user_session for {user_id}");
                        user.token
                    }
                };

                // Store persistent user_id on peer for ban checks
                let s = state.read().await;
                if let Some(peer) = s.peers.get(peer_id) {
                    *peer.user_id.lock().await = Some(user_id.clone());
                }
                if let Some(peer) = s.peers.get(peer_id).cloned() {
                    drop(s);
                    send_to(
                        &peer,
                        &SignalMessage::Authenticated {
                            token: token_to_send,
                            user_id,
                        },
                    )
                    .await;
                    send_friend_snapshot_to_peer(state, peer_id, db).await;
                }
                log::info!("Peer {peer_id} authenticated (restored identity)");
                return true;
            }
        }
    }

    // Generate a fresh persistent identity for newly authenticated users.
    let new_token = generate_token();
    let user_id = generate_user_id();
    let now = unix_now_secs();

    // Store persistent user_id on peer for ban checks
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            *peer.user_id.lock().await = Some(user_id.clone());
        }
    }

    // Persist new user
    let db_clone = db_ref.clone();
    let uid = user_id.clone();
    let tok = new_token.clone();
    let name = user_name.clone();
    let save_result = tokio::time::timeout(crate::DB_TIMEOUT, tokio::task::spawn_blocking(move || {
        db_clone.save_user(&crate::persistence::UserRow {
            user_id: uid,
            token: tok,
            display_name: name,
            created_at: now,
            issued_at: now,
            last_seen_at: now,
        })
    }))
    .await;

    match save_result {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => {
            log::error!("Failed to persist user: {e}");
            let s = state.read().await;
            if let Some(peer) = s.peers.get(peer_id) {
                *peer.user_id.lock().await = None;
            }
            send_error(state, peer_id, "Authentication is temporarily unavailable").await;
            return false;
        }
        Ok(Err(e)) => {
            log::error!("Failed to join auth persistence task: {e}");
            let s = state.read().await;
            if let Some(peer) = s.peers.get(peer_id) {
                *peer.user_id.lock().await = None;
            }
            send_error(state, peer_id, "Authentication is temporarily unavailable").await;
            return false;
        }
        Err(_) => {
            log::warn!("DB timeout: save_user for peer {peer_id}");
            send_error(state, peer_id, "Authentication is temporarily unavailable").await;
            return false;
        }
    }

    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(
            &peer,
            &SignalMessage::Authenticated {
                token: new_token,
                user_id,
            },
        )
        .await;
        send_friend_snapshot_to_peer(state, peer_id, db).await;
    }

    log::info!("Peer {peer_id} authenticated (new identity)");
    true
}

fn generate_token() -> String {
    random_hex(32)
}

fn generate_user_id() -> String {
    format!("u{}", random_hex(12))
}

fn random_hex(num_bytes: usize) -> String {
    let mut bytes = vec![0u8; num_bytes];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

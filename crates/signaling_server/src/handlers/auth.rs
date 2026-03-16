use crate::{send_to, Db, State};
use shared_types::SignalMessage;

use super::friends::send_friend_snapshot_to_peer;

pub async fn handle_authenticate(
    state: &State,
    peer_id: &str,
    token: Option<String>,
    user_name: String,
    db: &Db,
) {
    let user_name = user_name.trim().to_string();
    if user_name.is_empty() || user_name.len() > 32 {
        return;
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
        return;
    };

    // Try to restore identity from existing token
    if let Some(ref tok) = token {
        if !tok.is_empty() {
            let db_clone = db_ref.clone();
            let tok_clone = tok.clone();
            let found = tokio::task::spawn_blocking(move || {
                db_clone.find_user_by_token(&tok_clone).unwrap_or(None)
            })
            .await
            .unwrap_or(None);

            if let Some(user) = found {
                // Restore identity — update name if changed
                if user.display_name != user_name {
                    let db_clone = db_ref.clone();
                    let uid = user.user_id.clone();
                    let name = user_name.clone();
                    tokio::task::spawn_blocking(move || {
                        let _ = db_clone.update_user_name(&uid, &name);
                    });
                }

                // Store persistent user_id on peer for ban checks
                let s = state.read().await;
                if let Some(peer) = s.peers.get(peer_id) {
                    *peer.user_id.lock().await = Some(user.user_id.clone());
                }
                if let Some(peer) = s.peers.get(peer_id).cloned() {
                    drop(s);
                    send_to(
                        &peer,
                        &SignalMessage::Authenticated {
                            token: user.token,
                            user_id: user.user_id,
                        },
                    )
                    .await;
                    send_friend_snapshot_to_peer(state, peer_id, db).await;
                }
                log::info!("Peer {peer_id} authenticated (restored identity)");
                return;
            }
        }
    }

    // Generate new token (64 hex chars from OS entropy)
    let new_token = generate_token();
    let user_id = peer_id.to_string();

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
    let name = user_name;
    tokio::task::spawn_blocking(move || {
        if let Err(e) = db_clone.save_user(&crate::persistence::UserRow {
            user_id: uid,
            token: tok,
            display_name: name,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        }) {
            log::error!("Failed to persist user: {e}");
        }
    });

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
}

fn generate_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut result = String::with_capacity(64);
    for i in 0..8 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_usize(i);
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        result.push_str(&format!("{:016x}", hasher.finish()));
    }
    result
}

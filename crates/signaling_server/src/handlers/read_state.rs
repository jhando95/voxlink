use shared_types::{ReadStateEntry, SignalMessage};

use crate::types::{Db, State};
use crate::{now_epoch_secs, send_to};

/// Persist a (user, channel) → last_read_message_id mark. Idempotent: re-running
/// with the same arguments is a no-op aside from a clock-skew-tolerant
/// `last_read_at` bump. Anonymous (no user_id) peers are silently ignored — read
/// state is only meaningful for authenticated identities.
pub(crate) async fn handle_mark_channel_read(
    state: &State,
    peer_id: &str,
    channel_id: String,
    message_id: String,
    db: &Db,
) {
    if channel_id.is_empty() || message_id.is_empty() {
        return;
    }
    let user_id = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => peer.user_id.lock().await.clone(),
            None => return,
        }
    };
    let Some(user_id) = user_id else {
        return;
    };
    let Some(db) = db.as_ref() else {
        return;
    };
    let db = db.clone();
    let now = now_epoch_secs() as i64;
    tokio::task::spawn_blocking(move || {
        db.upsert_last_read(&user_id, &channel_id, &message_id, now);
    });
}

/// Build a fresh ReadStateSnapshot for a user and push it down a specific peer.
/// Called from the auth handlers as the final step before returning so a new
/// device starts perfectly in sync with whatever the user has read elsewhere.
pub(crate) async fn send_snapshot_to_peer(state: &State, peer_id: &str, db: &Db) {
    let user_id = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => peer.user_id.lock().await.clone(),
            None => None,
        }
    };
    let Some(user_id) = user_id else {
        return;
    };
    let Some(db_ref) = db.as_ref() else {
        return;
    };
    let db_clone = db_ref.clone();
    let entries = tokio::task::spawn_blocking(move || {
        db_clone
            .load_read_state_for_user(&user_id)
            .unwrap_or_default()
            .into_iter()
            .map(|(channel_id, last_read_message_id)| ReadStateEntry {
                channel_id,
                last_read_message_id,
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    if entries.is_empty() {
        return;
    }
    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(&peer, &SignalMessage::ReadStateSnapshot { entries });
    }
}

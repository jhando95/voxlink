use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use shared_types::{FavoriteFriend, FriendRequest, SignalMessage};

use crate::handlers::presence::describe_user_presence;
use crate::{send_error, send_to, Db, Peer, State};

pub async fn send_friend_snapshot_to_peer(state: &State, peer_id: &str, db: &Db) {
    let Some(user_id) = authenticated_user_id(state, peer_id).await else {
        return;
    };
    let peer = {
        let s = state.read().await;
        s.peers.get(peer_id).cloned()
    };
    let Some(peer) = peer else {
        return;
    };

    let (friends, incoming_requests, outgoing_requests) =
        build_friend_snapshot(state, &user_id, db).await;
    send_to(
        &peer,
        &SignalMessage::FriendSnapshot {
            friends,
            incoming_requests,
            outgoing_requests,
        },
    )
    .await;
}

pub async fn notify_friend_snapshot_for_user(state: &State, user_id: &str, db: &Db) {
    let peers = peers_for_user(state, user_id).await;
    if peers.is_empty() {
        return;
    }

    let (friends, incoming_requests, outgoing_requests) =
        build_friend_snapshot(state, user_id, db).await;
    for peer in peers {
        send_to(
            &peer,
            &SignalMessage::FriendSnapshot {
                friends: friends.clone(),
                incoming_requests: incoming_requests.clone(),
                outgoing_requests: outgoing_requests.clone(),
            },
        )
        .await;
    }
}

pub async fn handle_send_friend_request(state: &State, peer_id: &str, user_id: String, db: &Db) {
    let Some(db_arc) = db.as_ref().cloned() else {
        send_error(state, peer_id, "Friend requests require persistence").await;
        return;
    };
    let Some(requester_id) = authenticated_user_id(state, peer_id).await else {
        send_error(
            state,
            peer_id,
            "Authenticate before sending friend requests",
        )
        .await;
        return;
    };

    let target_user_id = user_id.trim().to_string();
    if target_user_id.is_empty() {
        send_error(state, peer_id, "Friend request target is missing").await;
        return;
    }
    if target_user_id == requester_id {
        send_error(state, peer_id, "You cannot add yourself as a friend").await;
        return;
    }

    let requester_id_for_db = requester_id.clone();
    let target_for_db = target_user_id.clone();
    let now = unix_now_secs() as i64;
    let outcome = tokio::task::spawn_blocking(move || -> Result<FriendRequestOutcome, String> {
        if db_arc.find_user_by_id(&target_for_db)?.is_none() {
            return Ok(FriendRequestOutcome::Error(
                "That account is not available".into(),
            ));
        }
        if db_arc.friendship_exists(&requester_id_for_db, &target_for_db)? {
            return Ok(FriendRequestOutcome::Error(
                "You are already friends".into(),
            ));
        }
        if db_arc.friend_request_exists(&requester_id_for_db, &target_for_db)? {
            return Ok(FriendRequestOutcome::Error(
                "Friend request already pending".into(),
            ));
        }

        if db_arc.friend_request_exists(&target_for_db, &requester_id_for_db)? {
            db_arc.delete_friend_request(&target_for_db, &requester_id_for_db)?;
            let (user_low_id, user_high_id) =
                ordered_pair_owned(&requester_id_for_db, &target_for_db);
            db_arc.save_friendship(&crate::persistence::FriendshipRow {
                user_low_id,
                user_high_id,
                created_at: now,
            })?;
            return Ok(FriendRequestOutcome::Accepted);
        }

        db_arc.save_friend_request(&crate::persistence::FriendRequestRow {
            requester_id: requester_id_for_db,
            addressee_id: target_for_db,
            created_at: now,
        })?;
        Ok(FriendRequestOutcome::Sent)
    })
    .await
    .unwrap_or_else(|_| Err("Friend request task failed".into()));

    match outcome {
        Ok(FriendRequestOutcome::Error(message)) => {
            send_error(state, peer_id, &message).await;
        }
        Ok(FriendRequestOutcome::Accepted) | Ok(FriendRequestOutcome::Sent) => {
            notify_friend_snapshot_for_user(state, &requester_id, db).await;
            notify_friend_snapshot_for_user(state, &target_user_id, db).await;
        }
        Err(message) => {
            send_error(state, peer_id, &message).await;
        }
    }
}

pub async fn handle_respond_friend_request(
    state: &State,
    peer_id: &str,
    user_id: String,
    accept: bool,
    db: &Db,
) {
    let Some(db_arc) = db.as_ref().cloned() else {
        send_error(state, peer_id, "Friend requests require persistence").await;
        return;
    };
    let Some(current_user_id) = authenticated_user_id(state, peer_id).await else {
        send_error(
            state,
            peer_id,
            "Authenticate before managing friend requests",
        )
        .await;
        return;
    };

    let source_user_id = user_id.trim().to_string();
    if source_user_id.is_empty() {
        send_error(state, peer_id, "Friend request target is missing").await;
        return;
    }

    let current_for_db = current_user_id.clone();
    let source_for_db = source_user_id.clone();
    let now = unix_now_secs() as i64;
    let outcome = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        if !db_arc.friend_request_exists(&source_for_db, &current_for_db)? {
            return Ok(false);
        }
        db_arc.delete_friend_request(&source_for_db, &current_for_db)?;
        if accept {
            let (user_low_id, user_high_id) = ordered_pair_owned(&current_for_db, &source_for_db);
            db_arc.save_friendship(&crate::persistence::FriendshipRow {
                user_low_id,
                user_high_id,
                created_at: now,
            })?;
        }
        Ok(true)
    })
    .await
    .unwrap_or_else(|_| Err("Friend request response task failed".into()));

    match outcome {
        Ok(true) => {
            notify_friend_snapshot_for_user(state, &current_user_id, db).await;
            notify_friend_snapshot_for_user(state, &source_user_id, db).await;
        }
        Ok(false) => {
            send_error(state, peer_id, "That friend request is no longer pending").await;
        }
        Err(message) => {
            send_error(state, peer_id, &message).await;
        }
    }
}

pub async fn handle_cancel_friend_request(state: &State, peer_id: &str, user_id: String, db: &Db) {
    let Some(db_arc) = db.as_ref().cloned() else {
        send_error(state, peer_id, "Friend requests require persistence").await;
        return;
    };
    let Some(current_user_id) = authenticated_user_id(state, peer_id).await else {
        send_error(
            state,
            peer_id,
            "Authenticate before managing friend requests",
        )
        .await;
        return;
    };

    let target_user_id = user_id.trim().to_string();
    if target_user_id.is_empty() {
        send_error(state, peer_id, "Friend request target is missing").await;
        return;
    }

    let current_for_db = current_user_id.clone();
    let target_for_db = target_user_id.clone();
    let removed = tokio::task::spawn_blocking(move || {
        db_arc.delete_friend_request(&current_for_db, &target_for_db)
    })
    .await
    .unwrap_or_else(|_| Err("Cancel friend request task failed".into()));

    match removed {
        Ok(true) => {
            notify_friend_snapshot_for_user(state, &current_user_id, db).await;
            notify_friend_snapshot_for_user(state, &target_user_id, db).await;
        }
        Ok(false) => {
            send_error(
                state,
                peer_id,
                "That outgoing friend request is already gone",
            )
            .await;
        }
        Err(message) => {
            send_error(state, peer_id, &message).await;
        }
    }
}

pub async fn handle_remove_friend(state: &State, peer_id: &str, user_id: String, db: &Db) {
    let Some(db_arc) = db.as_ref().cloned() else {
        send_error(state, peer_id, "Friend requests require persistence").await;
        return;
    };
    let Some(current_user_id) = authenticated_user_id(state, peer_id).await else {
        send_error(state, peer_id, "Authenticate before managing friends").await;
        return;
    };

    let target_user_id = user_id.trim().to_string();
    if target_user_id.is_empty() {
        send_error(state, peer_id, "Friend target is missing").await;
        return;
    }

    let current_for_db = current_user_id.clone();
    let target_for_db = target_user_id.clone();
    let removed = tokio::task::spawn_blocking(move || {
        db_arc.delete_friendship(&current_for_db, &target_for_db)
    })
    .await
    .unwrap_or_else(|_| Err("Remove friend task failed".into()));

    match removed {
        Ok(true) => {
            notify_friend_snapshot_for_user(state, &current_user_id, db).await;
            notify_friend_snapshot_for_user(state, &target_user_id, db).await;
        }
        Ok(false) => {
            send_error(state, peer_id, "You are not friends with that account").await;
        }
        Err(message) => {
            send_error(state, peer_id, &message).await;
        }
    }
}

async fn build_friend_snapshot(
    state: &State,
    user_id: &str,
    db: &Db,
) -> (Vec<FavoriteFriend>, Vec<FriendRequest>, Vec<FriendRequest>) {
    let Some(db) = db.as_ref().cloned() else {
        return (Vec::new(), Vec::new(), Vec::new());
    };

    let user_id_owned = user_id.to_string();
    let loaded = tokio::task::spawn_blocking(move || load_snapshot_rows(&db, &user_id_owned))
        .await
        .unwrap_or_else(|_| Ok((Vec::new(), Vec::new(), Vec::new())));

    let Ok((mut friends, mut incoming_requests, mut outgoing_requests)) = loaded else {
        return (Vec::new(), Vec::new(), Vec::new());
    };

    for friend in &mut friends {
        let presence = describe_user_presence(state, &friend.user_id).await;
        apply_presence(friend, &presence);
    }

    friends.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    incoming_requests.sort_by(|left, right| right.requested_at.cmp(&left.requested_at));
    outgoing_requests.sort_by(|left, right| right.requested_at.cmp(&left.requested_at));
    (friends, incoming_requests, outgoing_requests)
}

fn load_snapshot_rows(
    db: &crate::persistence::Database,
    user_id: &str,
) -> Result<(Vec<FavoriteFriend>, Vec<FriendRequest>, Vec<FriendRequest>), String> {
    let friendships = db.load_friendships_for_user(user_id)?;
    let incoming_rows = db.load_incoming_friend_requests(user_id)?;
    let outgoing_rows = db.load_outgoing_friend_requests(user_id)?;

    let mut friends = Vec::with_capacity(friendships.len());
    for friendship in friendships {
        let other_user_id = if friendship.user_low_id == user_id {
            friendship.user_high_id
        } else {
            friendship.user_low_id
        };
        let name = db
            .find_user_by_id(&other_user_id)?
            .map(|user| user.display_name)
            .unwrap_or_else(|| "Unknown user".into());
        friends.push(FavoriteFriend {
            user_id: other_user_id,
            name,
            ..FavoriteFriend::default()
        });
    }

    let mut incoming_requests = Vec::with_capacity(incoming_rows.len());
    for request in incoming_rows {
        let name = db
            .find_user_by_id(&request.requester_id)?
            .map(|user| user.display_name)
            .unwrap_or_else(|| "Unknown user".into());
        incoming_requests.push(FriendRequest {
            user_id: request.requester_id,
            name,
            requested_at: request.created_at as u64,
        });
    }

    let mut outgoing_requests = Vec::with_capacity(outgoing_rows.len());
    for request in outgoing_rows {
        let name = db
            .find_user_by_id(&request.addressee_id)?
            .map(|user| user.display_name)
            .unwrap_or_else(|| "Unknown user".into());
        outgoing_requests.push(FriendRequest {
            user_id: request.addressee_id,
            name,
            requested_at: request.created_at as u64,
        });
    }

    Ok((friends, incoming_requests, outgoing_requests))
}

fn apply_presence(friend: &mut FavoriteFriend, presence: &shared_types::FriendPresence) {
    if !presence.name.is_empty() {
        friend.name = presence.name.clone();
    }
    friend.is_online = presence.is_online;
    friend.is_in_voice = presence.is_in_voice;
    friend.in_private_call = presence.in_private_call;
    friend.active_space_name = presence.active_space_name.clone().unwrap_or_default();
    friend.active_channel_name = presence.active_channel_name.clone().unwrap_or_default();
    if friend.is_online {
        friend.last_seen_at = unix_now_secs();
        if !friend.active_space_name.is_empty() {
            friend.last_space_name = friend.active_space_name.clone();
        }
        if !friend.active_channel_name.is_empty() {
            friend.last_channel_name = friend.active_channel_name.clone();
        } else if friend.in_private_call {
            friend.last_channel_name = "Private call".into();
        }
    }
}

async fn authenticated_user_id(state: &State, peer_id: &str) -> Option<String> {
    let peer = {
        let s = state.read().await;
        s.peers.get(peer_id).cloned()
    }?;
    let user_id = peer.user_id.lock().await.clone();
    user_id
}

async fn peers_for_user(state: &State, user_id: &str) -> Vec<Arc<Peer>> {
    let peers = {
        let s = state.read().await;
        s.peers.values().cloned().collect::<Vec<_>>()
    };

    let mut matching = Vec::new();
    for peer in peers {
        if peer.user_id.lock().await.as_deref() == Some(user_id) {
            matching.push(peer);
        }
    }
    matching
}

fn ordered_pair_owned(user_a: &str, user_b: &str) -> (String, String) {
    if user_a <= user_b {
        (user_a.to_string(), user_b.to_string())
    } else {
        (user_b.to_string(), user_a.to_string())
    }
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

enum FriendRequestOutcome {
    Sent,
    Accepted,
    Error(String),
}

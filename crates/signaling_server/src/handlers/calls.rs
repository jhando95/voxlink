use shared_types::SignalMessage;
use crate::types::State;
use crate::connection::{send_to, send_error};

pub(crate) async fn handle_call_user(state: &State, caller_peer_id: &str, target_user_id: String) {
    let s = state.read().await;
    let caller_peer = match s.peers.get(caller_peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    let caller_user_id = match caller_peer.user_id.lock().await.clone() {
        Some(id) => id,
        None => {
            drop(s);
            send_to(
                &caller_peer,
                &SignalMessage::CallEnded {
                    room_key: String::new(),
                    reason: "not_authenticated".into(),
                },
            )
            .await;
            return;
        }
    };
    let caller_name = caller_peer.name.lock().await.clone();

    // Find the target peer by user_id
    let mut target_peer = None;
    for peer in s.peers.values() {
        let uid = peer.user_id.lock().await;
        if uid.as_deref() == Some(&target_user_id) {
            target_peer = Some(peer.clone());
            break;
        }
    }
    drop(s);

    let room_key = format!("dm_call:{}:{}", caller_user_id, target_user_id);

    match target_peer {
        Some(target) => {
            send_to(
                &target,
                &SignalMessage::IncomingCall {
                    caller_id: caller_user_id,
                    caller_name,
                    room_key,
                },
            )
            .await;
        }
        None => {
            send_to(
                &caller_peer,
                &SignalMessage::CallEnded {
                    room_key,
                    reason: "offline".into(),
                },
            )
            .await;
        }
    }
}

pub(crate) async fn handle_accept_call(state: &State, peer_id: &str, room_key: String) {
    // Parse the room key to find both user IDs: "dm_call:{caller_id}:{target_id}"
    let parts: Vec<&str> = room_key.splitn(3, ':').collect();
    if parts.len() < 3 || parts[0] != "dm_call" {
        send_error(state, peer_id, "Invalid call room key").await;
        return;
    }
    let caller_user_id = parts[1];
    let _target_user_id = parts[2];

    let s = state.read().await;
    let accepter = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    let accepter_name = accepter.name.lock().await.clone();

    // Find the caller peer by user_id
    let mut caller_peer = None;
    for peer in s.peers.values() {
        let uid = peer.user_id.lock().await;
        if uid.as_deref() == Some(caller_user_id) {
            caller_peer = Some(peer.clone());
            break;
        }
    }
    drop(s);

    if let Some(caller) = caller_peer {
        let caller_name = caller.name.lock().await.clone();
        // Join both peers to the DM call room using existing room join logic
        crate::handlers::room::handle_join_room(state, &caller.id, room_key.clone(), caller_name, None)
            .await;
        crate::handlers::room::handle_join_room(state, peer_id, room_key, accepter_name, None).await;
    } else {
        send_to(
            &accepter,
            &SignalMessage::CallEnded {
                room_key,
                reason: "caller_disconnected".into(),
            },
        )
        .await;
    }
}

pub(crate) async fn handle_decline_call(state: &State, peer_id: &str, room_key: String) {
    let parts: Vec<&str> = room_key.splitn(3, ':').collect();
    if parts.len() < 3 || parts[0] != "dm_call" {
        return;
    }
    let caller_user_id = parts[1];
    let target_user_id = parts[2];

    let s = state.read().await;
    let decliner = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    let decliner_user_id = decliner.user_id.lock().await.clone();

    // Determine who the other party is
    let other_user_id = if decliner_user_id.as_deref() == Some(caller_user_id) {
        target_user_id
    } else {
        caller_user_id
    };

    // Find the other party's peer
    let mut other_peer = None;
    for peer in s.peers.values() {
        let uid = peer.user_id.lock().await;
        if uid.as_deref() == Some(other_user_id) {
            other_peer = Some(peer.clone());
            break;
        }
    }
    drop(s);

    if let Some(other) = other_peer {
        send_to(
            &other,
            &SignalMessage::CallEnded {
                room_key,
                reason: "declined".into(),
            },
        )
        .await;
    }
}

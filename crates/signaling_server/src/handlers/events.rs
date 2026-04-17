use shared_types::SignalMessage;
use crate::types::{State, Db};
use crate::connection::{send_to, send_error};

pub(crate) async fn handle_create_event(
    state: &State,
    peer_id: &str,
    title: String,
    description: String,
    start_time: i64,
    end_time: i64,
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
    let user_id = peer
        .user_id
        .lock()
        .await
        .clone()
        .unwrap_or_else(|| peer_id.to_string());
    let role = crate::handlers::space::role_for_identity(space, &user_id);
    if !role.has_at_least(shared_types::SpaceRole::Moderator) {
        drop(s);
        send_error(state, peer_id, "Moderator+ required to create events").await;
        return;
    }
    let creator_name = peer.name.lock().await.clone();
    let event_id = {
        use rand::RngCore;
        let mut buf = [0u8; 4];
        rand::rngs::OsRng.fill_bytes(&mut buf);
        format!("evt_{:08x}", u32::from_le_bytes(buf))
    };
    let members: Vec<_> = space.member_ids.iter().cloned().collect();
    let peers_map: Vec<_> = members
        .iter()
        .filter_map(|mid| s.peers.get(mid).cloned())
        .collect();
    drop(s);
    if let Some(db) = db {
        let _ = db.create_scheduled_event(
            &event_id,
            &space_id,
            &title,
            &description,
            start_time,
            end_time,
            &user_id,
            &creator_name,
        );
    }
    let event = shared_types::ScheduledEvent {
        id: event_id,
        title,
        description,
        start_time,
        end_time,
        creator_name,
        interested_count: 0,
        is_interested: false,
    };
    let msg = SignalMessage::ScheduledEventCreated { event };
    for p in &peers_map {
        send_to(p, &msg).await;
    }
}

pub(crate) async fn handle_delete_event(state: &State, peer_id: &str, event_id: String, db: &Db) {
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
    if !role.has_at_least(shared_types::SpaceRole::Moderator) {
        drop(s);
        send_error(state, peer_id, "Moderator+ required").await;
        return;
    }
    let members: Vec<_> = space.member_ids.iter().cloned().collect();
    let peers_map: Vec<_> = members
        .iter()
        .filter_map(|mid| s.peers.get(mid).cloned())
        .collect();
    drop(s);
    if let Some(db) = db {
        let _ = db.delete_scheduled_event(&event_id);
    }
    let msg = SignalMessage::ScheduledEventDeleted { event_id };
    for p in &peers_map {
        send_to(p, &msg).await;
    }
}

pub(crate) async fn handle_toggle_event_interest(state: &State, peer_id: &str, event_id: String, db: &Db) {
    let s = state.read().await;
    let peer = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
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
    drop(s);
    let is_interested = match db.toggle_event_interest(&event_id, &user_id) {
        Ok(b) => b,
        Err(_) => return,
    };
    let count = db.get_event_interest_count(&event_id).unwrap_or(0);
    let s = state.read().await;
    if let Some(p) = s.peers.get(peer_id).cloned() {
        send_to(
            &p,
            &SignalMessage::EventInterestUpdated {
                event_id,
                interested_count: count,
                is_interested,
            },
        )
        .await;
    }
}

pub(crate) async fn handle_list_events(state: &State, peer_id: &str, db: &Db) {
    let s = state.read().await;
    let peer = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    let space_id = match peer.space_id.lock().await.clone() {
        Some(id) => id,
        None => return,
    };
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
    drop(s);
    let events = db
        .load_scheduled_events(&space_id, &user_id)
        .unwrap_or_default();
    let s = state.read().await;
    if let Some(p) = s.peers.get(peer_id).cloned() {
        send_to(&p, &SignalMessage::ScheduledEventList { events }).await;
    }
}

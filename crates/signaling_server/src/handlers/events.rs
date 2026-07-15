use crate::connection::{send_error, send_to};
use crate::types::{Db, State};
use shared_types::SignalMessage;

pub(crate) async fn handle_create_event(
    state: &State,
    peer_id: &str,
    title: String,
    description: String,
    start_time: i64,
    end_time: i64,
    db: &Db,
) {
    // Validate inputs before doing anything else. Without these guards the
    // handler would happily persist empty titles, multi-megabyte descriptions,
    // past-dated start times, and end_time<start_time intervals.
    let title = title.trim().to_string();
    if title.is_empty() {
        send_error(state, peer_id, "Event title is required").await;
        return;
    }
    if title.chars().count() > 128 {
        send_error(state, peer_id, "Event title is too long (max 128 chars)").await;
        return;
    }
    if description.chars().count() > 2000 {
        send_error(
            state,
            peer_id,
            "Event description is too long (max 2000 chars)",
        )
        .await;
        return;
    }
    let now = crate::now_epoch_secs() as i64;
    // Allow a small backwards grace (5 min) for clock skew.
    if start_time + 300 < now {
        send_error(state, peer_id, "Event start time must be in the future").await;
        return;
    }
    if end_time != 0 && end_time <= start_time {
        send_error(state, peer_id, "Event end time must be after start time").await;
        return;
    }

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
    let members: Vec<_> = space.member_ids.to_vec();
    let peers_map: Vec<_> = members
        .iter()
        .filter_map(|mid| s.peers.get(mid).cloned())
        .collect();
    drop(s);
    if let Some(db) = db {
        let _ = db.create_scheduled_event(&crate::persistence::NewScheduledEvent {
            id: &event_id,
            space_id: &space_id,
            title: &title,
            description: &description,
            start_time,
            end_time,
            creator_id: &user_id,
            creator_name: &creator_name,
        });
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
        send_to(p, &msg);
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
    let members: Vec<_> = space.member_ids.to_vec();
    let peers_map: Vec<_> = members
        .iter()
        .filter_map(|mid| s.peers.get(mid).cloned())
        .collect();
    drop(s);
    if let Some(db) = db {
        // Scope the delete to the actor's space; if nothing was removed the
        // event_id did not belong here, so don't broadcast a phantom deletion.
        match db.delete_scheduled_event(&event_id, &space_id) {
            Ok(false) => {
                send_error(state, peer_id, "Event not found in your space").await;
                return;
            }
            Err(_) => {
                send_error(state, peer_id, "Failed to delete event").await;
                return;
            }
            Ok(true) => {}
        }
    }
    let msg = SignalMessage::ScheduledEventDeleted { event_id };
    for p in &peers_map {
        send_to(p, &msg);
    }
}

pub(crate) async fn handle_toggle_event_interest(
    state: &State,
    peer_id: &str,
    event_id: String,
    db: &Db,
) {
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
        );
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
        send_to(&p, &SignalMessage::ScheduledEventList { events });
    }
}

use std::collections::HashSet;
use std::sync::Arc;

use shared_types::{FriendPresence, SignalMessage};

use crate::{send_to, Peer, State};

const MAX_WATCHED_FRIENDS: usize = 256;

pub async fn handle_watch_friend_presence(state: &State, peer_id: &str, user_ids: Vec<String>) {
    let peer = {
        let s = state.read().await;
        s.peers.get(peer_id).cloned()
    };
    let Some(peer) = peer else {
        return;
    };

    let mut seen = HashSet::new();
    let watched: Vec<String> = user_ids
        .into_iter()
        .map(|user_id| user_id.trim().to_string())
        .filter(|user_id| !user_id.is_empty())
        .filter(|user_id| seen.insert(user_id.clone()))
        .take(MAX_WATCHED_FRIENDS)
        .collect();

    {
        let mut current = peer.watched_friend_ids.lock().await;
        current.clear();
        current.extend(watched.iter().cloned());
    }

    let mut presences = Vec::with_capacity(watched.len());
    for user_id in watched {
        presences.push(describe_user_presence(state, &user_id).await);
    }
    send_to(&peer, &SignalMessage::FriendPresenceSnapshot { presences }).await;
}

pub async fn notify_watchers_for_peer(state: &State, peer_id: &str) {
    let user_id = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => peer.user_id.lock().await.clone(),
            None => None,
        }
    };
    if let Some(user_id) = user_id {
        notify_watchers_for_user(state, &user_id).await;
    }
}

pub async fn notify_watchers_for_user(state: &State, user_id: &str) {
    let watchers = watchers_for_user(state, user_id).await;
    if watchers.is_empty() {
        return;
    }

    let presence = describe_user_presence(state, user_id).await;
    for watcher in watchers {
        send_to(
            &watcher,
            &SignalMessage::FriendPresenceChanged {
                presence: presence.clone(),
            },
        )
        .await;
    }
}

async fn watchers_for_user(state: &State, user_id: &str) -> Vec<Arc<Peer>> {
    let s = state.read().await;
    let mut watchers = Vec::new();
    for peer in s.peers.values() {
        // Use try_lock to avoid blocking: if the lock is held, skip this peer
        // (they'll get the next presence update). This prevents O(n) awaits.
        match peer.watched_friend_ids.try_lock() {
            Ok(watched) => {
                if watched.contains(user_id) {
                    watchers.push(peer.clone());
                }
            }
            Err(_) => {
                // Lock contended — skip this peer for this update cycle
            }
        }
    }
    watchers
}

fn better_presence_rank(presence: &FriendPresence) -> i32 {
    if presence.active_space_name.is_some() && presence.is_in_voice {
        return 4;
    }
    if presence.in_private_call {
        return 3;
    }
    if presence.active_space_name.is_some() {
        return 2;
    }
    if presence.is_online {
        return 1;
    }
    0
}

pub async fn describe_user_presence(state: &State, user_id: &str) -> FriendPresence {
    // Collect matching peers first to minimize time holding state read lock
    let matching_peers: Vec<(Arc<Peer>, String, Option<String>, Option<String>)> = {
        let s = state.read().await;
        let mut matches = Vec::new();
        for peer in s.peers.values() {
            // Use try_lock to avoid blocking state read for all other peers
            let uid_match = match peer.user_id.try_lock() {
                Ok(uid) => uid.as_deref() == Some(user_id),
                Err(_) => false,
            };
            if !uid_match {
                continue;
            }
            let name = peer
                .name
                .try_lock()
                .map(|n| n.clone())
                .unwrap_or_default();
            let space_id = peer
                .space_id
                .try_lock()
                .ok()
                .and_then(|s| s.clone());
            let room_code = peer.cached_room_code();
            matches.push((peer.clone(), name, space_id, room_code));
        }
        matches
    };

    let mut best = FriendPresence {
        user_id: user_id.to_string(),
        ..FriendPresence::default()
    };

    // Now resolve space/channel info with a fresh read lock per candidate
    let s = state.read().await;
    for (_, name, space_id, room_code) in &matching_peers {
        let mut candidate = FriendPresence {
            user_id: user_id.to_string(),
            name: name.clone(),
            is_online: true,
            ..FriendPresence::default()
        };

        if let Some(space_id) = space_id {
            if let Some(space) = s.spaces.get(space_id) {
                candidate.active_space_name = Some(space.name.clone());
                if let Some(room_key) = room_code.as_deref() {
                    if let Some(channel) = space
                        .channels
                        .iter()
                        .find(|channel| channel.room_key == room_key)
                    {
                        candidate.active_channel_name = Some(channel.name.clone());
                        candidate.is_in_voice =
                            channel.channel_type == shared_types::ChannelType::Voice;
                    }
                }
            }
        } else if room_code.is_some() {
            candidate.is_in_voice = true;
            candidate.in_private_call = true;
        }

        if better_presence_rank(&candidate) > better_presence_rank(&best)
            || (best.name.is_empty() && !candidate.name.is_empty())
        {
            best = candidate;
        }
    }

    best
}

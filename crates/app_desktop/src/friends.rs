use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use shared_types::{
    AppState, FavoriteFriend, FriendPresence, FriendRequest, MemberInfo, SignalMessage,
};
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

pub fn load_from_config(
    window: &MainWindow,
    state: &Rc<RefCell<AppState>>,
    favorites: Vec<FavoriteFriend>,
) {
    {
        let mut app = state.borrow_mut();
        app.favorite_friends = favorites;
        app.incoming_friend_requests.clear();
        app.outgoing_friend_requests.clear();
        for friend in &mut app.favorite_friends {
            clear_live_presence(friend);
        }
        refresh_metadata_in_place(&mut app);
    }
    sync_ui(window, state);
}

pub fn sync_ui(window: &MainWindow, state: &Rc<RefCell<AppState>>) {
    let search_query = window.get_space_search_query().to_string();
    let threads_changed = {
        let mut app = state.borrow_mut();
        crate::direct_messages::sync_threads_with_friends(&mut app)
    };
    if threads_changed {
        crate::direct_messages::persist(state);
    }

    let app = state.borrow();
    if let Some(ref space) = app.space {
        ui_shell::render_space(
            window,
            space,
            &search_query,
            &app.favorite_friends,
            &app.incoming_friend_requests,
            &app.outgoing_friend_requests,
            app.self_user_id.as_deref(),
        );
    }
    ui_shell::set_friend_counts(window, &app.favorite_friends);
    ui_shell::set_friend_list(window, &app.favorite_friends);
    ui_shell::set_friend_requests(
        window,
        &app.incoming_friend_requests,
        &app.outgoing_friend_requests,
    );
    ui_shell::set_direct_message_threads(window, &app.direct_message_threads);
    window.set_unread_direct_messages_count(
        app.direct_message_threads
            .iter()
            .map(|thread| thread.unread_count)
            .sum::<u32>() as i32,
    );
    ui_shell::sync_member_widget(app.space.as_ref(), &app.favorite_friends);
}

pub fn persist(state: &Rc<RefCell<AppState>>) {
    crate::helpers::save_favorite_friends_async(state.borrow().favorite_friends.clone());
}

pub fn sync_presence_subscription(
    state: &Rc<RefCell<AppState>>,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let watched_user_ids = {
        let app = state.borrow();
        let self_user_id = app.self_user_id.as_deref();
        let mut watched = BTreeSet::new();
        for friend in &app.favorite_friends {
            if self_user_id == Some(friend.user_id.as_str()) {
                continue;
            }
            watched.insert(friend.user_id.clone());
        }
        watched.into_iter().collect::<Vec<_>>()
    };

    let network = network.clone();
    rt_handle.spawn(async move {
        let net = network.lock().await;
        let _ = net
            .send_signal(&SignalMessage::WatchFriendPresence {
                user_ids: watched_user_ids,
            })
            .await;
    });
}

pub fn handle_friend_snapshot(
    window: &MainWindow,
    state: &Rc<RefCell<AppState>>,
    friends: &[FavoriteFriend],
    incoming_requests: &[FriendRequest],
    outgoing_requests: &[FriendRequest],
) {
    {
        let mut app = state.borrow_mut();
        merge_cached_friends_in_place(&mut app.favorite_friends, friends);
        app.incoming_friend_requests = incoming_requests.to_vec();
        app.outgoing_friend_requests = outgoing_requests.to_vec();
        refresh_metadata_in_place(&mut app);
        crate::direct_messages::sync_threads_with_friends(&mut app);
    }
    persist(state);
    crate::direct_messages::persist(state);
    sync_ui(window, state);
}

pub fn handle_presence_snapshot(
    window: &MainWindow,
    state: &Rc<RefCell<AppState>>,
    presences: &[FriendPresence],
) {
    let now = unix_now_secs();
    {
        let mut app = state.borrow_mut();
        for friend in &mut app.favorite_friends {
            clear_live_presence(friend);
        }
        for presence in presences {
            if let Some(friend) = app
                .favorite_friends
                .iter_mut()
                .find(|friend| friend.user_id == presence.user_id)
            {
                apply_presence(friend, presence, now);
            }
        }
        crate::direct_messages::sync_threads_with_friends(&mut app);
    }
    sync_ui(window, state);
}

pub fn handle_presence_changed(
    window: &MainWindow,
    state: &Rc<RefCell<AppState>>,
    presence: &FriendPresence,
) {
    let now = unix_now_secs();
    {
        let mut app = state.borrow_mut();
        if let Some(friend) = app
            .favorite_friends
            .iter_mut()
            .find(|friend| friend.user_id == presence.user_id)
        {
            apply_presence(friend, presence, now);
        }
        crate::direct_messages::sync_threads_with_friends(&mut app);
    }
    sync_ui(window, state);
}

pub fn refresh_metadata_in_place(app: &mut AppState) -> bool {
    let Some(space) = app.space.as_ref() else {
        return false;
    };

    let mut changed = false;
    let now = unix_now_secs();
    for friend in &mut app.favorite_friends {
        if let Some(member) = space
            .members
            .iter()
            .find(|member| stable_member_id(member) == friend.user_id)
        {
            if friend.name != member.name {
                friend.name = member.name.clone();
                changed = true;
            }
            if friend.last_space_name != space.name {
                friend.last_space_name = space.name.clone();
                changed = true;
            }
            let channel_name = member.channel_name.clone().unwrap_or_default();
            if !channel_name.is_empty() && friend.last_channel_name != channel_name {
                friend.last_channel_name = channel_name;
                changed = true;
            }
            if friend.last_seen_at != now {
                friend.last_seen_at = now;
                changed = true;
            }
        }
    }
    changed
}

pub fn stable_member_id(member: &MemberInfo) -> String {
    member
        .user_id
        .clone()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| member.id.clone())
}

/// Merge incoming friend list into the existing list in place, preserving
/// cached offline metadata without cloning the entire Vec.
fn merge_cached_friends_in_place(existing: &mut Vec<FavoriteFriend>, incoming: &[FavoriteFriend]) {
    // Extract cached offline metadata into owned HashMap before mutating
    let cached_meta: HashMap<String, (String, String, String, u64)> = existing
        .drain(..)
        .map(|f| {
            (
                f.user_id,
                (
                    f.name,
                    f.last_space_name,
                    f.last_channel_name,
                    f.last_seen_at,
                ),
            )
        })
        .collect();

    existing.reserve(incoming.len());
    for friend in incoming {
        let mut next = friend.clone();
        if let Some((name, space, channel, seen_at)) = cached_meta.get(&friend.user_id) {
            if next.name.is_empty() {
                next.name = name.clone();
            }
            if !next.is_online {
                next.last_space_name = space.clone();
                next.last_channel_name = channel.clone();
                next.last_seen_at = *seen_at;
            }
        }
        existing.push(next);
    }
    existing.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
}

fn apply_presence(friend: &mut FavoriteFriend, presence: &FriendPresence, now: u64) {
    let was_online = friend.is_online;
    if !presence.name.is_empty() {
        friend.name = presence.name.clone();
    }

    friend.is_online = presence.is_online;
    friend.is_in_voice = presence.is_in_voice;
    friend.in_private_call = presence.in_private_call;
    friend.active_space_name = presence.active_space_name.clone().unwrap_or_default();
    friend.active_channel_name = presence.active_channel_name.clone().unwrap_or_default();

    if friend.is_online {
        if !friend.active_space_name.is_empty() {
            friend.last_space_name = friend.active_space_name.clone();
        }
        if !friend.active_channel_name.is_empty() {
            friend.last_channel_name = friend.active_channel_name.clone();
        } else if friend.in_private_call {
            friend.last_channel_name = "Private call".into();
        }
        friend.last_seen_at = now;
    } else if was_online {
        friend.last_seen_at = now;
    }
}

fn clear_live_presence(friend: &mut FavoriteFriend) {
    friend.is_online = false;
    friend.is_in_voice = false;
    friend.in_private_call = false;
    friend.active_space_name.clear();
    friend.active_channel_name.clear();
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use shared_types::{AppState, DirectMessageThread, FavoriteFriend, TextMessageData};
use ui_shell::MainWindow;

pub fn load_from_config(
    window: &MainWindow,
    state: &Rc<RefCell<AppState>>,
    threads: Vec<DirectMessageThread>,
) {
    {
        let mut app = state.borrow_mut();
        app.direct_message_threads = threads;
        sync_threads_with_friends(&mut app);
    }
    sync_ui(window, state);
}

pub fn sync_ui(window: &MainWindow, state: &Rc<RefCell<AppState>>) {
    let app = state.borrow();
    ui_shell::set_direct_message_threads(window, &app.direct_message_threads);
    window.set_unread_direct_messages_count(
        app.direct_message_threads
            .iter()
            .map(|thread| thread.unread_count)
            .sum::<u32>() as i32,
    );
}

pub fn persist(state: &Rc<RefCell<AppState>>) {
    crate::helpers::save_recent_direct_messages_async(
        state.borrow().direct_message_threads.clone(),
    );
}

pub fn sync_threads_with_friends(app: &mut AppState) -> bool {
    let friend_ids: std::collections::HashSet<&str> = app
        .favorite_friends
        .iter()
        .map(|friend| friend.user_id.as_str())
        .collect();
    let friend_map: HashMap<&str, &FavoriteFriend> = app
        .favorite_friends
        .iter()
        .map(|friend| (friend.user_id.as_str(), friend))
        .collect();
    let original_len = app.direct_message_threads.len();
    app.direct_message_threads
        .retain(|thread| friend_ids.contains(thread.user_id.as_str()));

    let mut changed = original_len != app.direct_message_threads.len();
    for thread in &mut app.direct_message_threads {
        if let Some(friend) = friend_map.get(thread.user_id.as_str()) {
            changed |= apply_friend_metadata(thread, friend);
        }
    }

    sort_threads(&mut app.direct_message_threads);
    changed
}

pub fn record_selected_conversation(
    app: &mut AppState,
    user_id: &str,
    user_name: &str,
    history: &[TextMessageData],
) -> bool {
    let friend = app
        .favorite_friends
        .iter()
        .find(|friend| friend.user_id == user_id)
        .cloned();
    let thread = thread_mut(app, user_id, friend.as_ref(), user_name);
    let mut changed = false;

    if thread.unread_count != 0 {
        thread.unread_count = 0;
        changed = true;
    }
    if thread.user_name != user_name {
        thread.user_name = user_name.to_string();
        changed = true;
    }

    if let Some(last_message) = history.last() {
        changed |= update_last_message(thread, last_message);
    }

    app.active_direct_message_user_id = Some(user_id.to_string());
    sort_threads(&mut app.direct_message_threads);
    changed
}

pub fn record_message(
    app: &mut AppState,
    user_id: &str,
    message: &TextMessageData,
    conversation_open: bool,
) -> bool {
    let self_user_id = app.self_user_id.clone();
    let friend = app
        .favorite_friends
        .iter()
        .find(|friend| friend.user_id == user_id)
        .cloned();
    let fallback_name = friend
        .as_ref()
        .map(|friend| friend.name.as_str())
        .unwrap_or(user_id);
    let thread = thread_mut(app, user_id, friend.as_ref(), fallback_name);

    let mut changed = update_last_message(thread, message);
    let is_self_message = self_user_id
        .as_deref()
        .map(|self_user_id| self_user_id == message.sender_id)
        .unwrap_or(false);

    if conversation_open {
        if thread.unread_count != 0 {
            thread.unread_count = 0;
            changed = true;
        }
    } else if !is_self_message {
        let next = thread.unread_count.saturating_add(1).min(99);
        if next != thread.unread_count {
            thread.unread_count = next;
            changed = true;
        }
    }

    sort_threads(&mut app.direct_message_threads);
    changed
}

pub fn record_message_edit(
    app: &mut AppState,
    user_id: &str,
    message_id: &str,
    new_content: &str,
) -> bool {
    let Some(thread) = app
        .direct_message_threads
        .iter_mut()
        .find(|thread| thread.user_id == user_id)
    else {
        return false;
    };
    if thread.last_message_id != message_id {
        return false;
    }

    let preview = preview_for(new_content);
    if thread.last_message_preview == preview {
        return false;
    }
    thread.last_message_preview = preview;
    true
}

pub fn record_message_delete(app: &mut AppState, user_id: &str, message_id: &str) -> bool {
    let Some(thread) = app
        .direct_message_threads
        .iter_mut()
        .find(|thread| thread.user_id == user_id)
    else {
        return false;
    };
    if thread.last_message_id != message_id {
        return false;
    }

    if thread.last_message_preview == "Message removed" {
        return false;
    }
    thread.last_message_preview = "Message removed".into();
    true
}

fn thread_mut<'a>(
    app: &'a mut AppState,
    user_id: &str,
    friend: Option<&FavoriteFriend>,
    fallback_name: &str,
) -> &'a mut DirectMessageThread {
    if let Some(index) = app
        .direct_message_threads
        .iter()
        .position(|thread| thread.user_id == user_id)
    {
        let thread = &mut app.direct_message_threads[index];
        if let Some(friend) = friend {
            apply_friend_metadata(thread, friend);
        } else if thread.user_name.is_empty() {
            thread.user_name = fallback_name.to_string();
        }
        return thread;
    }

    app.direct_message_threads.push(DirectMessageThread {
        user_id: user_id.to_string(),
        user_name: friend
            .map(|friend| friend.name.clone())
            .unwrap_or_else(|| fallback_name.to_string()),
        last_message_id: String::new(),
        last_message_preview: String::new(),
        last_message_at: 0,
        unread_count: 0,
        is_online: friend.map(|friend| friend.is_online).unwrap_or(false),
        is_in_voice: friend
            .map(|friend| friend.is_in_voice || friend.in_private_call)
            .unwrap_or(false),
    });
    app.direct_message_threads
        .last_mut()
        .expect("direct message thread should exist after insert")
}

fn update_last_message(thread: &mut DirectMessageThread, message: &TextMessageData) -> bool {
    let mut changed = false;
    if thread.last_message_id != message.message_id {
        thread.last_message_id = message.message_id.clone();
        changed = true;
    }

    let preview = preview_for(&message.content);
    if thread.last_message_preview != preview {
        thread.last_message_preview = preview;
        changed = true;
    }

    if thread.last_message_at != message.timestamp {
        thread.last_message_at = message.timestamp;
        changed = true;
    }

    changed
}

fn apply_friend_metadata(thread: &mut DirectMessageThread, friend: &FavoriteFriend) -> bool {
    let mut changed = false;
    if thread.user_name != friend.name {
        thread.user_name = friend.name.clone();
        changed = true;
    }
    if thread.is_online != friend.is_online {
        thread.is_online = friend.is_online;
        changed = true;
    }
    let is_in_voice = friend.is_in_voice || friend.in_private_call;
    if thread.is_in_voice != is_in_voice {
        thread.is_in_voice = is_in_voice;
        changed = true;
    }
    changed
}

const MAX_DM_THREADS: usize = 100;

fn sort_threads(threads: &mut Vec<DirectMessageThread>) {
    threads.sort_by(|left, right| {
        right
            .last_message_at
            .cmp(&left.last_message_at)
            .then_with(|| right.unread_count.cmp(&left.unread_count))
            .then_with(|| {
                left.user_name
                    .to_lowercase()
                    .cmp(&right.user_name.to_lowercase())
            })
    });
    // Cap to prevent unbounded memory growth
    threads.truncate(MAX_DM_THREADS);
}

fn preview_for(content: &str) -> String {
    let single_line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let preview: String = single_line.chars().take(72).collect();
    if single_line.chars().count() > 72 {
        format!("{preview}...")
    } else {
        preview
    }
}

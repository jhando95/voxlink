use std::cell::RefCell;
use std::rc::Rc;

use shared_types::AppView;
use slint::Model;
use ui_shell::MainWindow;

const MAX_CHAT_MESSAGES_IN_VIEW: usize = 250;

pub fn handle_text_channel_selected(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    channel_id: &str,
    channel_name: &str,
    history: &[shared_types::TextMessageData],
) {
    log::info!("Selected text channel: {channel_name} ({channel_id})");
    w.set_chat_channel_id(channel_id.into());
    w.set_chat_channel_name(channel_name.into());
    w.set_chat_is_direct_message(false);

    // Show channel topic in subtitle if available, else fall back to space name
    let (topic, slow_mode_secs) = {
        let s = state.borrow();
        let ch = s
            .space
            .as_ref()
            .and_then(|sp| sp.channels.iter().find(|ch| ch.id == channel_id));
        let topic = ch.and_then(|ch| {
            if ch.topic.is_empty() {
                None
            } else {
                Some(ch.topic.clone())
            }
        });
        let slow = ch.map(|ch| ch.slow_mode_secs).unwrap_or(0);
        (topic, slow)
    };
    let subtitle = match topic {
        Some(t) => format!("{} — {}", w.get_current_space_name(), t),
        None => w.get_current_space_name().to_string(),
    };
    w.set_chat_context_subtitle(subtitle.into());

    // Set slow mode duration for countdown timer; reset any active countdown
    w.set_slow_mode_secs(slow_mode_secs as i32);
    w.set_slow_mode_remaining(0);
    w.set_chat_back_view(ui_shell::view_to_index(AppView::Space));
    w.set_chat_input(slint::SharedString::default());
    w.set_editing_message_id(slint::SharedString::default());
    w.set_editing_original_content(slint::SharedString::default());
    w.set_reply_target_message_id(slint::SharedString::default());
    w.set_reply_target_sender_name(slint::SharedString::default());
    w.set_reply_target_preview(slint::SharedString::default());

    // Use user_name for self-detection (server uses peer IDs, not "self")
    let my_name = w.get_user_name().to_string();
    ui_shell::set_chat_messages(w, history, &my_name);
    sync_pinned_messages(w);

    {
        let mut s = state.borrow_mut();
        if let Some(ref mut space) = s.space {
            space.selected_text_channel_id = Some(channel_id.to_string());
            space.unread_text_channels.remove(channel_id);
            crate::helpers::save_last_text_channel_async(space.id.clone(), channel_id.to_string());
        }
        s.active_direct_message_user_id = None;
        s.current_view = AppView::TextChat;
    }

    crate::friends::sync_ui(w, state);

    sync_typing_text(w, state, channel_id);
    w.set_current_view(ui_shell::view_to_index(AppView::TextChat));
}

pub fn handle_direct_message_selected(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    user_id: &str,
    user_name: &str,
    history: &[shared_types::TextMessageData],
) {
    log::info!("Selected direct message: {user_name} ({user_id})");
    w.set_chat_channel_id(user_id.into());
    w.set_chat_channel_name(user_name.into());
    w.set_chat_is_direct_message(true);
    w.set_chat_context_subtitle("Direct message".into());
    w.set_slow_mode_secs(0);
    w.set_slow_mode_remaining(0);
    w.set_chat_input(slint::SharedString::default());
    w.set_editing_message_id(slint::SharedString::default());
    w.set_editing_original_content(slint::SharedString::default());
    w.set_chat_pinned_messages(
        std::rc::Rc::new(slint::VecModel::<ui_shell::ChatMessage>::from(Vec::new())).into(),
    );
    w.set_reply_target_message_id(slint::SharedString::default());
    w.set_reply_target_sender_name(slint::SharedString::default());
    w.set_reply_target_preview(slint::SharedString::default());

    let my_name = w.get_user_name().to_string();
    ui_shell::set_chat_messages(w, history, &my_name);

    let changed = {
        let mut s = state.borrow_mut();
        let changed = crate::direct_messages::record_selected_conversation(
            &mut s, user_id, user_name, history,
        );
        s.current_view = AppView::TextChat;
        changed
    };
    if changed {
        crate::direct_messages::persist(state);
    }
    crate::direct_messages::sync_ui(w, state);

    sync_direct_typing_text(w, state, user_id);
    w.set_current_view(ui_shell::view_to_index(AppView::TextChat));
    w.set_status_text("Conversation ready".into());
}

pub fn handle_text_message(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    channel_id: &str,
    message: &shared_types::TextMessageData,
) {
    let current_view = w.get_current_view();
    let current_channel = w.get_chat_channel_id().to_string();
    let chat_open = current_view == ui_shell::view_to_index(AppView::TextChat)
        && !w.get_chat_is_direct_message()
        && current_channel == channel_id;
    let my_name = w.get_user_name().to_string();
    let is_self_message = is_self_message(state, w, message);

    {
        let mut s = state.borrow_mut();
        if let Some(ref mut space) = s.space {
            if let Some(users) = space.typing_users.get_mut(channel_id) {
                users.retain(|name| name != &message.sender_name);
                if users.is_empty() {
                    space.typing_users.remove(channel_id);
                }
            }
        }
    }

    if !chat_open {
        let mut s = state.borrow_mut();
        if let Some(ref mut space) = s.space {
            let is_known_text_channel = space.channels.iter().any(|channel| {
                channel.id == channel_id && channel.channel_type == shared_types::ChannelType::Text
            });
            if is_known_text_channel && !is_self_message {
                let unread = space
                    .unread_text_channels
                    .entry(channel_id.to_string())
                    .or_insert(0);
                *unread = unread.saturating_add(1).min(99);
            }
        }
        drop(s);
        crate::friends::sync_ui(w, state);

        // Send notification for messages in other channels; suppress in DND mode.
        // Respect per-channel notification overrides: "none" = silent, "mentions" = @-only.
        if w.get_notifications_enabled() && !is_self_message && w.get_status_preset() != 2 {
            let cfg = config_store::load_config();
            let override_setting = cfg
                .channel_notification_overrides
                .get(channel_id)
                .map(|s| s.as_str())
                .unwrap_or("all");
            let my_name_ref = w.get_user_name();
            let mentions_self = ui_shell::message_mentions_user(&message.content, &my_name_ref);
            let should_notify = match override_setting {
                "none" => false,
                "mentions" => mentions_self,
                _ => true, // "all" or unset
            };
            if should_notify {
                let sender = message.sender_name.clone();
                let content = truncate_for_notification(&message.content);
                crate::helpers::send_notification(&sender, &content);
            }
        }
        return;
    }

    let chat_msg = ui_shell::text_msg_to_chat_msg(message, &my_name);
    push_chat_message(w, chat_msg);
    sync_typing_text(w, state, channel_id);
}

pub fn handle_direct_message(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    user_id: &str,
    message: &shared_types::TextMessageData,
) {
    let active_direct = {
        let mut s = state.borrow_mut();
        if let Some(users) = s.direct_typing_users.get_mut(user_id) {
            users.retain(|name| name != &message.sender_name);
            if users.is_empty() {
                s.direct_typing_users.remove(user_id);
            }
        }
        s.active_direct_message_user_id.clone()
    };
    let chat_open = w.get_current_view() == ui_shell::view_to_index(AppView::TextChat)
        && w.get_chat_is_direct_message()
        && active_direct.as_deref() == Some(user_id);
    let thread_changed = {
        let mut s = state.borrow_mut();
        crate::direct_messages::record_message(&mut s, user_id, message, chat_open)
    };
    if thread_changed {
        crate::direct_messages::persist(state);
    }
    crate::direct_messages::sync_ui(w, state);
    let my_name = w.get_user_name().to_string();
    let is_self_message = is_self_message(state, w, message);

    if !chat_open {
        // Suppress desktop notifications in DND mode
        if w.get_notifications_enabled() && !is_self_message && w.get_status_preset() != 2 {
            let sender = message.sender_name.clone();
            let content = truncate_for_notification(&message.content);
            crate::helpers::send_notification(&sender, &content);
        }
        return;
    }

    let chat_msg = ui_shell::text_msg_to_chat_msg(message, &my_name);
    push_chat_message(w, chat_msg);
    sync_direct_typing_text(w, state, user_id);
}

pub fn handle_text_message_edited(
    w: &MainWindow,
    channel_id: &str,
    message_id: &str,
    new_content: &str,
) {
    if w.get_chat_is_direct_message() {
        return;
    }
    let current_channel = w.get_chat_channel_id().to_string();
    if current_channel != channel_id {
        return;
    }

    let messages: slint::ModelRc<ui_shell::ChatMessage> = w.get_chat_messages();
    if let Some(model) = messages
        .as_any()
        .downcast_ref::<slint::VecModel<ui_shell::ChatMessage>>()
    {
        for i in 0..model.row_count() {
            if let Some(msg) = model.row_data(i) {
                if msg.message_id.as_str() == message_id {
                    let mut updated = msg;
                    updated.content = new_content.into();
                    updated.edited = true;
                    updated.mentions_self =
                        ui_shell::message_mentions_user(new_content, &w.get_user_name());
                    model.set_row_data(i, updated);
                    break;
                }
            }
        }
    }
    sync_pinned_messages(w);
}

pub fn handle_direct_message_edited(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    user_id: &str,
    message_id: &str,
    new_content: &str,
) {
    let changed = {
        let mut s = state.borrow_mut();
        crate::direct_messages::record_message_edit(&mut s, user_id, message_id, new_content)
    };
    if changed {
        crate::direct_messages::persist(state);
    }
    crate::direct_messages::sync_ui(w, state);
    if !w.get_chat_is_direct_message() || w.get_chat_channel_id() != user_id {
        return;
    }
    update_message_content(w, message_id, new_content);
}

pub fn handle_text_message_deleted(w: &MainWindow, channel_id: &str, message_id: &str) {
    if w.get_chat_is_direct_message() {
        return;
    }
    let current_channel = w.get_chat_channel_id().to_string();
    if current_channel != channel_id {
        return;
    }

    let messages: slint::ModelRc<ui_shell::ChatMessage> = w.get_chat_messages();
    if let Some(model) = messages
        .as_any()
        .downcast_ref::<slint::VecModel<ui_shell::ChatMessage>>()
    {
        for i in 0..model.row_count() {
            if let Some(msg) = model.row_data(i) {
                if msg.message_id.as_str() == message_id {
                    model.remove(i);
                    break;
                }
            }
        }
    }
    if w.get_reply_target_message_id() == message_id {
        w.set_reply_target_message_id(slint::SharedString::default());
        w.set_reply_target_sender_name(slint::SharedString::default());
        w.set_reply_target_preview(slint::SharedString::default());
    }
    sync_pinned_messages(w);
}

pub fn handle_direct_message_deleted(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    user_id: &str,
    message_id: &str,
) {
    let changed = {
        let mut s = state.borrow_mut();
        crate::direct_messages::record_message_delete(&mut s, user_id, message_id)
    };
    if changed {
        crate::direct_messages::persist(state);
    }
    crate::direct_messages::sync_ui(w, state);
    if !w.get_chat_is_direct_message() || w.get_chat_channel_id() != user_id {
        return;
    }
    remove_message(w, message_id);
}

pub fn handle_message_reaction(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    channel_id: &str,
    message_id: &str,
    emoji: &str,
    user_name: &str,
) {
    if w.get_chat_is_direct_message() {
        return;
    }
    let current_channel = w.get_chat_channel_id().to_string();
    if current_channel != channel_id {
        return;
    }

    let _ = state; // state reserved for future client-side message tracking

    // Toggle the reaction in the UI model, mirroring the server's toggle logic:
    // If user already reacted with this emoji, remove them; otherwise add them.
    let messages: slint::ModelRc<ui_shell::ChatMessage> = w.get_chat_messages();
    if let Some(model) = messages
        .as_any()
        .downcast_ref::<slint::VecModel<ui_shell::ChatMessage>>()
    {
        for i in 0..model.row_count() {
            if let Some(msg) = model.row_data(i) {
                if msg.message_id.as_str() == message_id {
                    let mut updated = msg;
                    // Parse existing reactions from the display string back to structured data,
                    // apply the toggle, then re-render. We track per-emoji user lists in a
                    // local vec since the UI only stores a formatted string.
                    let mut reaction_map: Vec<(String, Vec<String>)> =
                        parse_reaction_pills(&updated.reactions);

                    if let Some(entry) = reaction_map.iter_mut().find(|(e, _)| e == emoji) {
                        if let Some(pos) = entry.1.iter().position(|u| u == user_name) {
                            entry.1.remove(pos);
                        } else {
                            entry.1.push(user_name.to_string());
                        }
                    } else {
                        reaction_map.push((emoji.to_string(), vec![user_name.to_string()]));
                    }
                    // Remove empty reactions
                    reaction_map.retain(|(_, users)| !users.is_empty());

                    // Re-render as "emoji count" pills
                    let display = reaction_map
                        .iter()
                        .map(|(e, users)| format!("{} {}", e, users.len()))
                        .collect::<Vec<_>>()
                        .join("  ");
                    updated.reactions = display.into();
                    model.set_row_data(i, updated);
                    break;
                }
            }
        }
    }
    sync_pinned_messages(w);
}

pub fn handle_direct_message_reaction(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    user_id: &str,
    message_id: &str,
    emoji: &str,
    user_name: &str,
) {
    if !w.get_chat_is_direct_message() {
        return;
    }
    // For DMs, chat_channel_id holds the other user's ID
    let current_dm_user = w.get_chat_channel_id().to_string();
    if current_dm_user != user_id {
        return;
    }

    let _ = state;

    let messages: slint::ModelRc<ui_shell::ChatMessage> = w.get_chat_messages();
    if let Some(model) = messages
        .as_any()
        .downcast_ref::<slint::VecModel<ui_shell::ChatMessage>>()
    {
        for i in 0..model.row_count() {
            if let Some(msg) = model.row_data(i) {
                if msg.message_id.as_str() == message_id {
                    let mut updated = msg;
                    let mut reaction_map: Vec<(String, Vec<String>)> =
                        parse_reaction_pills(&updated.reactions);

                    if let Some(entry) = reaction_map.iter_mut().find(|(e, _)| e == emoji) {
                        if let Some(pos) = entry.1.iter().position(|u| u == user_name) {
                            entry.1.remove(pos);
                        } else {
                            entry.1.push(user_name.to_string());
                        }
                    } else {
                        reaction_map.push((emoji.to_string(), vec![user_name.to_string()]));
                    }
                    reaction_map.retain(|(_, users)| !users.is_empty());

                    let display = reaction_map
                        .iter()
                        .map(|(e, users)| format!("{} {}", e, users.len()))
                        .collect::<Vec<_>>()
                        .join("  ");
                    updated.reactions = display.into();
                    model.set_row_data(i, updated);
                    break;
                }
            }
        }
    }
}

/// Parse reaction display string "👍 2  ❤ 1" back into structured data.
/// Since we don't store user lists in the UI, we create placeholder user entries
/// matching the count. This is only used for toggle logic within a session —
/// full state is restored from server on channel re-select.
fn parse_reaction_pills(display: &slint::SharedString) -> Vec<(String, Vec<String>)> {
    let s = display.to_string();
    if s.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::new();
    // Split on double-space which separates pills
    for pill in s.split("  ") {
        let pill = pill.trim();
        if pill.is_empty() {
            continue;
        }
        // Last token is count, everything before is the emoji
        if let Some(space_pos) = pill.rfind(' ') {
            let emoji_part = &pill[..space_pos];
            let count_part = &pill[space_pos + 1..];
            if let Ok(count) = count_part.parse::<usize>() {
                let users: Vec<String> = (0..count).map(|i| format!("user_{i}")).collect();
                result.push((emoji_part.to_string(), users));
            }
        }
    }
    result
}

pub fn handle_typing_state(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    channel_id: &str,
    user_name: &str,
    is_typing: bool,
    tick: u64,
) {
    {
        let mut s = state.borrow_mut();
        if let Some(ref mut space) = s.space {
            let users = space
                .typing_users
                .entry(channel_id.to_string())
                .or_default();
            let key = (channel_id.to_string(), user_name.to_string());
            if is_typing {
                if !users.iter().any(|name| name == user_name) {
                    users.push(user_name.to_string());
                    users.sort_by_key(|name| name.to_lowercase());
                }
                space.typing_ticks.insert(key, tick);
            } else {
                users.retain(|name| name != user_name);
                space.typing_ticks.remove(&key);
            }

            if users.is_empty() {
                space.typing_users.remove(channel_id);
            }
        }
    }

    if w.get_chat_channel_id() == channel_id {
        sync_typing_text(w, state, channel_id);
    }
}

pub fn handle_direct_typing_state(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    user_id: &str,
    user_name: &str,
    is_typing: bool,
    tick: u64,
) {
    {
        let mut s = state.borrow_mut();
        if is_typing {
            let users = s
                .direct_typing_users
                .entry(user_id.to_string())
                .or_default();
            if !users.iter().any(|name| name == user_name) {
                users.push(user_name.to_string());
                users.sort_by_key(|name| name.to_lowercase());
            }
            s.direct_typing_ticks.insert(user_id.to_string(), tick);
        } else {
            if let Some(users) = s.direct_typing_users.get_mut(user_id) {
                users.retain(|name| name != user_name);
                if users.is_empty() {
                    s.direct_typing_users.remove(user_id);
                }
            }
            s.direct_typing_ticks.remove(user_id);
        }
    }

    if w.get_chat_is_direct_message() && w.get_chat_channel_id() == user_id {
        sync_direct_typing_text(w, state, user_id);
    }
}

/// Expire typing indicators older than 5 seconds (200 ticks at 40Hz).
/// Call from the tick loop periodically (e.g. every 40 ticks / 1 second).
pub fn expire_stale_typing(w: &MainWindow, state: &Rc<RefCell<shared_types::AppState>>, tick: u64) {
    const TYPING_TIMEOUT_TICKS: u64 = 200; // 5 seconds at 40Hz

    // Early exit: skip work when no typing indicators are active
    {
        let s = state.borrow();
        let has_channel = s
            .space
            .as_ref()
            .is_some_and(|sp| !sp.typing_ticks.is_empty());
        let has_dm = !s.direct_typing_ticks.is_empty();
        if !has_channel && !has_dm {
            return;
        }
    }

    let mut changed_channels: Vec<String> = Vec::new();
    let mut changed_dm = false;

    {
        let mut s = state.borrow_mut();

        // Expire channel typing
        if let Some(ref mut space) = s.space {
            let expired: Vec<(String, String)> = space
                .typing_ticks
                .iter()
                .filter(|(_, &t)| tick.saturating_sub(t) >= TYPING_TIMEOUT_TICKS)
                .map(|(k, _)| k.clone())
                .collect();

            for (channel_id, user_name) in &expired {
                space
                    .typing_ticks
                    .remove(&(channel_id.clone(), user_name.clone()));
                if let Some(users) = space.typing_users.get_mut(channel_id) {
                    users.retain(|name| name != user_name);
                    if users.is_empty() {
                        space.typing_users.remove(channel_id);
                    }
                    if !changed_channels.contains(channel_id) {
                        changed_channels.push(channel_id.clone());
                    }
                }
            }
        }

        // Expire DM typing
        let expired_dm: Vec<String> = s
            .direct_typing_ticks
            .iter()
            .filter(|(_, &t)| tick.saturating_sub(t) >= TYPING_TIMEOUT_TICKS)
            .map(|(k, _)| k.clone())
            .collect();

        for user_id in &expired_dm {
            s.direct_typing_ticks.remove(user_id);
            s.direct_typing_users.remove(user_id);
            changed_dm = true;
        }
    }

    // Update UI for affected channels
    let current_channel = w.get_chat_channel_id().to_string();
    for ch in &changed_channels {
        if *ch == current_channel {
            sync_typing_text(w, state, ch);
        }
    }
    if changed_dm && w.get_chat_is_direct_message() {
        sync_direct_typing_text(w, state, &current_channel);
    }
}

pub fn handle_message_pinned(w: &MainWindow, channel_id: &str, message_id: &str, pinned: bool) {
    if w.get_chat_is_direct_message() {
        return;
    }
    if w.get_chat_channel_id() != channel_id {
        return;
    }

    let messages: slint::ModelRc<ui_shell::ChatMessage> = w.get_chat_messages();
    if let Some(model) = messages
        .as_any()
        .downcast_ref::<slint::VecModel<ui_shell::ChatMessage>>()
    {
        for i in 0..model.row_count() {
            if let Some(msg) = model.row_data(i) {
                if msg.message_id.as_str() == message_id {
                    let mut updated = msg;
                    updated.is_pinned = pinned;
                    model.set_row_data(i, updated);
                    break;
                }
            }
        }
    }

    sync_pinned_messages(w);
}

fn sync_typing_text(w: &MainWindow, state: &Rc<RefCell<shared_types::AppState>>, channel_id: &str) {
    let typing_text = {
        let s = state.borrow();
        s.space
            .as_ref()
            .and_then(|space| space.typing_users.get(channel_id))
            .map(|users| format_typing_text(users))
            .unwrap_or_default()
    };
    w.set_chat_typing_text(typing_text.into());
}

fn sync_direct_typing_text(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    user_id: &str,
) {
    let typing_text = {
        let s = state.borrow();
        s.direct_typing_users
            .get(user_id)
            .map(|users| format_typing_text(users))
            .unwrap_or_default()
    };
    w.set_chat_typing_text(typing_text.into());
}

fn format_typing_text(users: &[String]) -> String {
    match users.len() {
        0 => String::new(),
        1 => format!("{} is typing", users[0]),
        2 => format!("{} and {} are typing", users[0], users[1]),
        3 => format!("{}, {}, and {} are typing", users[0], users[1], users[2]),
        n => format!("{n} people are typing"),
    }
}

fn sync_pinned_messages(w: &MainWindow) {
    let messages: slint::ModelRc<ui_shell::ChatMessage> = w.get_chat_messages();
    let mut pinned = Vec::new();
    if let Some(model) = messages
        .as_any()
        .downcast_ref::<slint::VecModel<ui_shell::ChatMessage>>()
    {
        for i in 0..model.row_count() {
            if let Some(msg) = model.row_data(i) {
                if msg.is_pinned {
                    pinned.push(msg);
                }
            }
        }
    }
    w.set_chat_pinned_messages(std::rc::Rc::new(slint::VecModel::from(pinned)).into());
}

fn push_chat_message(w: &MainWindow, chat_msg: ui_shell::ChatMessage) {
    let messages: slint::ModelRc<ui_shell::ChatMessage> = w.get_chat_messages();
    if let Some(model) = messages
        .as_any()
        .downcast_ref::<slint::VecModel<ui_shell::ChatMessage>>()
    {
        while model.row_count() >= MAX_CHAT_MESSAGES_IN_VIEW {
            model.remove(0);
        }
        model.push(chat_msg);
    }
}

fn update_message_content(w: &MainWindow, message_id: &str, new_content: &str) {
    let messages: slint::ModelRc<ui_shell::ChatMessage> = w.get_chat_messages();
    if let Some(model) = messages
        .as_any()
        .downcast_ref::<slint::VecModel<ui_shell::ChatMessage>>()
    {
        for i in 0..model.row_count() {
            if let Some(msg) = model.row_data(i) {
                if msg.message_id.as_str() == message_id {
                    let mut updated = msg;
                    updated.content = new_content.into();
                    updated.edited = true;
                    updated.mentions_self =
                        ui_shell::message_mentions_user(new_content, &w.get_user_name());
                    model.set_row_data(i, updated);
                    break;
                }
            }
        }
    }
    sync_pinned_messages(w);
}

fn remove_message(w: &MainWindow, message_id: &str) {
    let messages: slint::ModelRc<ui_shell::ChatMessage> = w.get_chat_messages();
    if let Some(model) = messages
        .as_any()
        .downcast_ref::<slint::VecModel<ui_shell::ChatMessage>>()
    {
        for i in 0..model.row_count() {
            if let Some(msg) = model.row_data(i) {
                if msg.message_id.as_str() == message_id {
                    model.remove(i);
                    break;
                }
            }
        }
    }
    if w.get_reply_target_message_id() == message_id {
        w.set_reply_target_message_id(slint::SharedString::default());
        w.set_reply_target_sender_name(slint::SharedString::default());
        w.set_reply_target_preview(slint::SharedString::default());
    }
    sync_pinned_messages(w);
}

/// Truncate message content to ~50 chars for notification preview,
/// safe for multi-byte UTF-8 (never splits a char boundary).
fn truncate_for_notification(content: &str) -> String {
    let mut end = 0;
    for (i, ch) in content.char_indices() {
        if i + ch.len_utf8() > 50 {
            break;
        }
        end = i + ch.len_utf8();
    }
    if end < content.len() {
        format!("{}...", &content[..end])
    } else {
        content.to_string()
    }
}

pub fn handle_search_results(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    channel_id: &str,
    messages: &[shared_types::TextMessageData],
) {
    // Only show results if we're still viewing this channel
    let current_channel = w.get_chat_channel_id().to_string();
    if current_channel != channel_id {
        return;
    }

    let user_name = w.get_user_name().to_string();
    let _ = state; // reserved for future use
    let items: Vec<ui_shell::ChatMessage> = messages
        .iter()
        .map(|m| ui_shell::text_msg_to_chat_msg(m, &user_name))
        .collect();

    let model = std::rc::Rc::new(slint::VecModel::from(items));
    w.set_chat_search_results(model.into());
}

fn is_self_message(
    state: &Rc<RefCell<shared_types::AppState>>,
    w: &MainWindow,
    message: &shared_types::TextMessageData,
) -> bool {
    let self_user_id = state.borrow().self_user_id.clone();
    self_user_id
        .as_deref()
        .map(|user_id| user_id == message.sender_id)
        .unwrap_or_else(|| w.get_user_name() == message.sender_name)
}

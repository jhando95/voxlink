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
    w.set_chat_context_subtitle(w.get_current_space_name());
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

        // Send notification for messages in other channels
        if w.get_notifications_enabled() && !is_self_message {
            let sender = message.sender_name.clone();
            let content = truncate_for_notification(&message.content);
            crate::helpers::send_notification(&sender, &content);
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
        if w.get_notifications_enabled() && !is_self_message {
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
                    updated.mentions_self = new_content.to_lowercase().contains(&format!(
                        "@{}",
                        w.get_user_name().to_string().to_lowercase()
                    ));
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

    // For simplicity, just append the reaction indicator to the existing reactions string.
    // A full implementation would track individual reaction state, but this is sufficient
    // for the display since the server sends the full reaction state on TextChannelSelected.
    let messages: slint::ModelRc<ui_shell::ChatMessage> = w.get_chat_messages();
    if let Some(model) = messages
        .as_any()
        .downcast_ref::<slint::VecModel<ui_shell::ChatMessage>>()
    {
        for i in 0..model.row_count() {
            if let Some(msg) = model.row_data(i) {
                if msg.message_id.as_str() == message_id {
                    let mut updated = msg;
                    // Simple toggle: if user's reaction emoji already in string, it was toggled
                    let current = updated.reactions.to_string();
                    if current.contains(emoji) {
                        // Re-render will happen on next channel select; for now just mark it
                        updated.reactions = format!("{current} (+{user_name})").into();
                    } else if current.is_empty() {
                        updated.reactions = format!("{emoji} 1").into();
                    } else {
                        updated.reactions = format!("{current}  {emoji} 1").into();
                    }
                    model.set_row_data(i, updated);
                    break;
                }
            }
        }
    }
    sync_pinned_messages(w);
}

pub fn handle_typing_state(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    channel_id: &str,
    user_name: &str,
    is_typing: bool,
) {
    {
        let mut s = state.borrow_mut();
        if let Some(ref mut space) = s.space {
            let users = space
                .typing_users
                .entry(channel_id.to_string())
                .or_default();
            if is_typing {
                if !users.iter().any(|name| name == user_name) {
                    users.push(user_name.to_string());
                    users.sort_by_key(|name| name.to_lowercase());
                }
            } else {
                users.retain(|name| name != user_name);
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
) {
    {
        let mut s = state.borrow_mut();
        let users = s
            .direct_typing_users
            .entry(user_id.to_string())
            .or_default();
        if is_typing {
            if !users.iter().any(|name| name == user_name) {
                users.push(user_name.to_string());
                users.sort_by_key(|name| name.to_lowercase());
            }
        } else {
            users.retain(|name| name != user_name);
        }

        if users.is_empty() {
            s.direct_typing_users.remove(user_id);
        }
    }

    if w.get_chat_is_direct_message() && w.get_chat_channel_id() == user_id {
        sync_direct_typing_text(w, state, user_id);
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
    match users {
        [] => String::new(),
        [one] => format!("{one} is typing..."),
        [one, two] => format!("{one} and {two} are typing..."),
        [one, two, ..] => format!("{one}, {two}, and others are typing..."),
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
                    updated.mentions_self = new_content.to_lowercase().contains(&format!(
                        "@{}",
                        w.get_user_name().to_string().to_lowercase()
                    ));
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

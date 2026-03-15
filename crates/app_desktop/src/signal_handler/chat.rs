use std::cell::RefCell;
use std::rc::Rc;

use shared_types::AppView;
use slint::Model;
use ui_shell::MainWindow;

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
    w.set_chat_input(slint::SharedString::default());

    // Use user_name for self-detection (server uses peer IDs, not "self")
    let my_name = w.get_user_name().to_string();
    ui_shell::set_chat_messages(w, history, &my_name);

    {
        let mut s = state.borrow_mut();
        if let Some(ref mut space) = s.space {
            space.selected_text_channel_id = Some(channel_id.to_string());
            space.unread_text_channels.remove(channel_id);
            let search_query = w.get_space_search_query().to_string();
            ui_shell::render_space(w, space, &search_query);
            crate::helpers::save_last_text_channel_async(space.id.clone(), channel_id.to_string());
        }
        s.current_view = AppView::TextChat;
    }

    w.set_current_view(ui_shell::view_to_index(AppView::TextChat));
}

pub fn handle_text_message(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    channel_id: &str,
    message: &shared_types::TextMessageData,
) {
    let current_view = w.get_current_view();
    let current_channel = w.get_chat_channel_id().to_string();
    let chat_open =
        current_view == ui_shell::view_to_index(AppView::TextChat) && current_channel == channel_id;
    let my_name = w.get_user_name().to_string();
    let is_self_message = message.sender_name == my_name;

    if !chat_open {
        let search_query = w.get_space_search_query().to_string();
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
                ui_shell::render_space(w, space, &search_query);
            }
        }

        // Send notification for messages in other channels
        if w.get_notifications_enabled() && !is_self_message {
            let sender = message.sender_name.clone();
            let content = if message.content.len() > 50 {
                format!("{}...", &message.content[..50])
            } else {
                message.content.clone()
            };
            crate::helpers::send_notification(&sender, &content);
        }
        return;
    }

    let chat_msg = ui_shell::text_msg_to_chat_msg(message, &my_name);

    let messages: slint::ModelRc<ui_shell::ChatMessage> = w.get_chat_messages();
    if let Some(model) = messages
        .as_any()
        .downcast_ref::<slint::VecModel<ui_shell::ChatMessage>>()
    {
        // Cap at 500 messages to bound memory usage
        while model.row_count() >= 500 {
            model.remove(0);
        }
        model.push(chat_msg);
    }
}

pub fn handle_text_message_edited(
    w: &MainWindow,
    channel_id: &str,
    message_id: &str,
    new_content: &str,
) {
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
                    model.set_row_data(i, updated);
                    break;
                }
            }
        }
    }
}

pub fn handle_text_message_deleted(w: &MainWindow, channel_id: &str, message_id: &str) {
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
}

pub fn handle_message_reaction(
    w: &MainWindow,
    channel_id: &str,
    message_id: &str,
    emoji: &str,
    user_name: &str,
) {
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
}

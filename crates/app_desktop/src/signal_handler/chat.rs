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

    state.borrow_mut().current_view = AppView::TextChat;
    w.set_current_view(ui_shell::view_to_index(AppView::TextChat));
}

pub fn handle_text_message(
    w: &MainWindow,
    _state: &Rc<RefCell<shared_types::AppState>>,
    channel_id: &str,
    message: &shared_types::TextMessageData,
) {
    // Only append if we're viewing this channel
    let current_channel = w.get_chat_channel_id().to_string();
    if current_channel != channel_id {
        return;
    }

    // Append to model
    let color_index = message.sender_name.bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32)) % 8;
    let my_name = w.get_user_name().to_string();
    let chat_msg = ui_shell::ChatMessage {
        sender_name: message.sender_name.clone().into(),
        content: message.content.clone().into(),
        timestamp: ui_shell::format_timestamp(message.timestamp).into(),
        is_self: message.sender_name == my_name,
        color_index: color_index as i32,
    };

    let messages: slint::ModelRc<ui_shell::ChatMessage> = w.get_chat_messages();
    if let Some(model) = messages.as_any().downcast_ref::<slint::VecModel<ui_shell::ChatMessage>>() {
        model.push(chat_msg);
    }
}

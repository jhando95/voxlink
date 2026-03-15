use std::sync::Arc;

use shared_types::SignalMessage;
use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

pub fn setup_select_text_channel(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_select_text_channel(move |channel_id| {
        let channel_id_str = channel_id.to_string();
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        w.set_status_text("Opening channel...".into());
        let network = network.clone();
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            if let Err(e) = net
                .send_signal(&SignalMessage::SelectTextChannel {
                    channel_id: channel_id_str,
                })
                .await
            {
                log::error!("Failed to select text channel: {e}");
                if let Some(w) = window_weak.upgrade() {
                    w.set_status_text("Failed to open channel".into());
                }
            }
        });
    });
}

pub fn setup_send_text_message(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_send_text_message(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let channel_id = w.get_chat_channel_id().to_string();
        let content = w.get_chat_input().to_string().trim().to_string();
        if content.is_empty() || channel_id.is_empty() {
            return;
        }
        w.set_chat_input(slint::SharedString::default());
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::SendTextMessage {
                    channel_id,
                    content,
                })
                .await;
        });
    });
}

pub fn setup_edit_text_message(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_edit_text_message(move |message_id, new_content| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let channel_id = w.get_chat_channel_id().to_string();
        let message_id = message_id.to_string();
        let new_content = new_content.to_string().trim().to_string();
        if new_content.is_empty() || channel_id.is_empty() {
            return;
        }
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::EditTextMessage {
                    channel_id,
                    message_id,
                    new_content,
                })
                .await;
        });
    });
}

pub fn setup_delete_text_message(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let window_weak = window.as_weak();
    let rt_handle = rt_handle.clone();
    window.on_delete_text_message(move |message_id| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let channel_id = w.get_chat_channel_id().to_string();
        let message_id = message_id.to_string();
        if channel_id.is_empty() {
            return;
        }
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::DeleteTextMessage {
                    channel_id,
                    message_id,
                })
                .await;
        });
    });
}

pub fn setup_react_to_message(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_react_to_message(move |message_id, emoji| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let channel_id = w.get_chat_channel_id().to_string();
        let message_id = message_id.to_string();
        let emoji = emoji.to_string();
        if channel_id.is_empty() {
            return;
        }
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::ReactToMessage {
                    channel_id,
                    message_id,
                    emoji,
                })
                .await;
        });
    });
}

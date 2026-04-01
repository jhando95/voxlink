use std::sync::Arc;

use shared_types::{AppView, SignalMessage};
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

pub fn setup_open_direct_message(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_open_direct_message(move |user_id| {
        let user_id = user_id.trim().to_string();
        if user_id.is_empty() {
            return;
        }
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let current_view = w.get_current_view();
        let back_view = if current_view == ui_shell::view_to_index(AppView::TextChat) {
            w.get_chat_back_view()
        } else {
            current_view
        };
        w.set_previous_view(back_view);
        w.set_chat_back_view(back_view);
        w.set_status_text("Opening conversation...".into());
        let network = network.clone();
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            if let Err(e) = net
                .send_signal(&SignalMessage::SelectDirectMessage { user_id })
                .await
            {
                log::error!("Failed to open direct message: {e}");
                if let Some(w) = window_weak.upgrade() {
                    w.set_status_text("Failed to open conversation".into());
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
        let target_id = w.get_chat_channel_id().to_string();
        let content = w.get_chat_input().to_string().trim().to_string();
        let reply_to_message_id = match w.get_reply_target_message_id().to_string() {
            value if value.is_empty() => None,
            value => Some(value),
        };
        let is_direct_message = w.get_chat_is_direct_message();
        if content.is_empty() || target_id.is_empty() {
            return;
        }
        w.set_chat_input(slint::SharedString::default());
        let network = network.clone();
        let window_weak2 = window_weak.clone();
        rt_handle.spawn(async move {
            let ok = match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                network.lock(),
            )
            .await
            {
                Ok(net) => {
                    let res = if is_direct_message {
                        net.send_signal(&SignalMessage::SendDirectMessage {
                            user_id: target_id,
                            content: content.clone(),
                            reply_to_message_id,
                        })
                        .await
                    } else {
                        net.send_signal(&SignalMessage::SendTextMessage {
                            channel_id: target_id,
                            content: content.clone(),
                            reply_to_message_id,
                        })
                        .await
                    };
                    if let Err(e) = &res {
                        log::error!("Failed to send message: {e}");
                    }
                    res.is_ok()
                }
                Err(_) => {
                    log::error!("Network lock timed out sending message");
                    false
                }
            };
            if !ok {
                // Restore the message so the user doesn't lose it
                if let Some(w) = window_weak2.upgrade() {
                    w.set_chat_input(content.into());
                }
            }
        });
    });
}

pub fn setup_chat_typing_activity(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_chat_typing_activity(move |is_typing| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let target_id = w.get_chat_channel_id().to_string();
        let is_direct_message = w.get_chat_is_direct_message();
        if target_id.is_empty() {
            return;
        }
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = if is_direct_message {
                net.send_signal(&SignalMessage::SetDirectTyping {
                    user_id: target_id,
                    is_typing,
                })
                .await
            } else {
                net.send_signal(&SignalMessage::SetTyping {
                    channel_id: target_id,
                    is_typing,
                })
                .await
            };
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
        let target_id = w.get_chat_channel_id().to_string();
        let message_id = message_id.to_string();
        let new_content = new_content.to_string().trim().to_string();
        let is_direct_message = w.get_chat_is_direct_message();
        if new_content.is_empty() || target_id.is_empty() {
            return;
        }
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = if is_direct_message {
                net.send_signal(&SignalMessage::EditDirectMessage {
                    user_id: target_id,
                    message_id,
                    new_content,
                })
                .await
            } else {
                net.send_signal(&SignalMessage::EditTextMessage {
                    channel_id: target_id,
                    message_id,
                    new_content,
                })
                .await
            };
        });
    });
}

pub fn setup_toggle_pin_message(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_toggle_pin_message(move |message_id, pinned| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        if w.get_chat_is_direct_message() {
            return;
        }
        let channel_id = w.get_chat_channel_id().to_string();
        if channel_id.is_empty() {
            return;
        }
        let message_id = message_id.to_string();
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::PinMessage {
                    channel_id,
                    message_id,
                    pinned,
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
        let target_id = w.get_chat_channel_id().to_string();
        let message_id = message_id.to_string();
        let is_direct_message = w.get_chat_is_direct_message();
        if target_id.is_empty() {
            return;
        }
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = if is_direct_message {
                net.send_signal(&SignalMessage::DeleteDirectMessage {
                    user_id: target_id,
                    message_id,
                })
                .await
            } else {
                net.send_signal(&SignalMessage::DeleteTextMessage {
                    channel_id: target_id,
                    message_id,
                })
                .await
            };
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
        let is_dm = w.get_chat_is_direct_message();
        let message_id = message_id.to_string();
        let emoji = emoji.to_string();
        let network = network.clone();
        if is_dm {
            // For DMs, chat_channel_id holds the target user_id
            let user_id = w.get_chat_channel_id().to_string();
            if user_id.is_empty() {
                return;
            }
            rt_handle.spawn(async move {
                let net = network.lock().await;
                let _ = net
                    .send_signal(&SignalMessage::ReactToDirectMessage {
                        user_id,
                        message_id,
                        emoji,
                    })
                    .await;
            });
        } else {
            let channel_id = w.get_chat_channel_id().to_string();
            if channel_id.is_empty() {
                return;
            }
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
        }
    });
}

pub fn setup_open_thread(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    let weak = window.as_weak();
    window.on_open_thread(move |channel_id, message_id| {
        let Some(w) = weak.upgrade() else { return };
        let channel_id = if channel_id.is_empty() {
            w.get_chat_channel_id().to_string()
        } else {
            channel_id.to_string()
        };
        let message_id = message_id.to_string();
        if channel_id.is_empty() || message_id.is_empty() {
            return;
        }
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::GetThread {
                    channel_id,
                    message_id,
                })
                .await;
        });
    });
}

pub fn setup_close_thread(window: &MainWindow) {
    window.on_close_thread(move || {});
}

pub fn setup_search_messages(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    let weak = window.as_weak();
    window.on_search_messages(move |query| {
        let Some(w) = weak.upgrade() else { return };
        let channel_id = w.get_chat_channel_id().to_string();
        let query = query.to_string();
        if channel_id.is_empty() || query.is_empty() {
            return;
        }
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::SearchMessages {
                    channel_id,
                    query,
                    limit: 50,
                })
                .await;
        });
    });
}

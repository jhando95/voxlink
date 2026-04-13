use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use shared_types::{AppView, SignalMessage};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

use crate::helpers::CONFIG_LOCK;

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
                    crate::helpers::show_toast(&w, "Failed to open channel", 3);
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
        // Reopen if previously closed — ensures DM reappears in the thread list
        crate::helpers::reopen_dm_async(user_id.clone());
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
                    crate::helpers::show_toast(&w, "Failed to open conversation", 3);
                }
            }
        });
    });
}

pub fn setup_close_direct_message(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
) {
    let state = state.clone();
    let window_weak = window.as_weak();
    window.on_close_direct_message(move |user_id| {
        let user_id = user_id.trim().to_string();
        if user_id.is_empty() {
            return;
        }
        // Remove from in-memory threads
        {
            let mut app = state.borrow_mut();
            app.direct_message_threads
                .retain(|thread| thread.user_id != user_id);
        }
        // Persist the close
        crate::helpers::close_dm_async(user_id);
        // Persist remaining threads
        crate::direct_messages::persist(&state);
        // Re-render
        if let Some(w) = window_weak.upgrade() {
            crate::direct_messages::sync_ui(&w, &state);
        }
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
        let raw_content = w.get_chat_input().to_string().trim().to_string();
        let content = ui_shell::resolve_emoji_shortcodes(&raw_content);

        // ── Slash command handling ──
        if content.starts_with('/') {
            let parts: Vec<&str> = content.splitn(2, ' ').collect();
            let cmd = parts[0].to_lowercase();
            let arg = parts.get(1).unwrap_or(&"").to_string();
            match cmd.as_str() {
                "/shrug" => {
                    let suffix = " \u{00AF}\\_(\u{30C4})_/\u{00AF}";
                    let new_content = if arg.is_empty() { suffix.trim_start().to_string() } else { format!("{arg}{suffix}") };
                    w.set_chat_input(new_content.into());
                    // Don't return — let it send with the modified content below
                }
                "/tableflip" => {
                    let suffix = " (\u{256F}\u{00B0}\u{25A1}\u{00B0})\u{256F}\u{FE35} \u{253B}\u{2501}\u{253B}";
                    let new_content = if arg.is_empty() { suffix.trim_start().to_string() } else { format!("{arg}{suffix}") };
                    w.set_chat_input(new_content.into());
                }
                "/unflip" => {
                    let suffix = " \u{252C}\u{2500}\u{252C} \u{30CE}( \u{309C}-\u{309C}\u{30CE})";
                    let new_content = if arg.is_empty() { suffix.trim_start().to_string() } else { format!("{arg}{suffix}") };
                    w.set_chat_input(new_content.into());
                }
                "/nick" => {
                    if !arg.is_empty() {
                        let network2 = network.clone();
                        let name = arg.clone();
                        rt_handle.spawn(async move {
                            let net = network2.lock().await;
                            let _ = net.send_signal(&SignalMessage::SetNickname { nickname: name }).await;
                        });
                    }
                    w.set_chat_input(slint::SharedString::default());
                    return;
                }
                "/status" => {
                    let preset = match arg.to_lowercase().as_str() {
                        "online" => 0i32,
                        "idle" => 1,
                        "dnd" => 2,
                        "invisible" => 3,
                        _ => -1,
                    };
                    if preset >= 0 {
                        w.set_status_preset(preset);
                        let network2 = network.clone();
                        let preset_name = arg.to_lowercase();
                        rt_handle.spawn(async move {
                            let net = network2.lock().await;
                            let _ = net.send_signal(&SignalMessage::SetUserStatus { status: preset_name }).await;
                        });
                    }
                    w.set_chat_input(slint::SharedString::default());
                    return;
                }
                "/activity" => {
                    let network2 = network.clone();
                    let text = arg.clone();
                    rt_handle.spawn(async move {
                        let net = network2.lock().await;
                        let _ = net.send_signal(&SignalMessage::SetActivity { activity: text }).await;
                    });
                    w.set_chat_input(slint::SharedString::default());
                    return;
                }
                "/mute" => {
                    let current = w.get_is_muted();
                    w.set_is_muted(!current);
                    w.set_chat_input(slint::SharedString::default());
                    return;
                }
                "/deafen" => {
                    let current = w.get_is_deafened();
                    w.set_is_deafened(!current);
                    w.set_chat_input(slint::SharedString::default());
                    return;
                }
                _ => {} // Unknown command — send as regular message
            }
            // For /shrug, /tableflip, /unflip — re-read the modified input
            let modified = w.get_chat_input().to_string();
            if !modified.is_empty() && modified != content {
                // Will be sent as the content below
                // fall through with modified content
            }
        }

        // Re-read in case slash commands modified it
        let content = {
            let current = w.get_chat_input().to_string().trim().to_string();
            if current.is_empty() { content } else { ui_shell::resolve_emoji_shortcodes(&current) }
        };

        // Enforce message length limit
        if content.len() > 2000 {
            crate::helpers::show_toast(&w, "Message too long (2000 char limit)", 2);
            return;
        }

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
                    crate::helpers::show_toast(&w, "Failed to send message", 3);
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

        // Track recent reactions: push to front, deduplicate, trim to 5
        {
            let _lock = CONFIG_LOCK.lock().ok();
            let mut cfg = config_store::load_config();
            cfg.recent_reactions.retain(|e| e != &emoji);
            cfg.recent_reactions.insert(0, emoji.clone());
            cfg.recent_reactions.truncate(5);
            ui_shell::set_recent_reactions(&w, &cfg.recent_reactions);
            let _ = config_store::save_config(&cfg);
        }

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

pub fn setup_forward_message(
    window: &MainWindow,
    state: &std::rc::Rc<std::cell::RefCell<shared_types::AppState>>,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    let weak = window.as_weak();
    let state = state.clone();
    window.on_forward_message(move |message_id| {
        let Some(w) = weak.upgrade() else { return };
        let message_id = message_id.to_string();
        let source_channel_id = w.get_chat_channel_id().to_string();
        if message_id.is_empty() || source_channel_id.is_empty() {
            return;
        }

        // Find the first other text channel in the space
        let target = {
            let s = state.borrow();
            s.space.as_ref().and_then(|space| {
                space
                    .channels
                    .iter()
                    .find(|ch| {
                        ch.channel_type == shared_types::ChannelType::Text
                            && ch.id != source_channel_id
                    })
                    .map(|ch| (ch.id.clone(), ch.name.clone()))
            })
        };

        let Some((target_channel_id, target_name)) = target else {
            crate::helpers::show_toast(&w, "No other text channel to forward to", 2);
            return;
        };

        crate::helpers::show_toast(&w, &format!("Forwarded to #{target_name}"), 1);
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::ForwardMessage {
                    source_channel_id,
                    message_id,
                    target_channel_id,
                })
                .await;
        });
    });
}

pub fn setup_copy_message_text(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_copy_message_text(move |content| {
        let text = content.to_string();
        if text.is_empty() {
            return;
        }
        if crate::helpers::copy_to_clipboard(&text) {
            if let Some(w) = window_weak.upgrade() {
                crate::helpers::show_toast(&w, "Copied to clipboard", 1);
            }
        } else {
            log::warn!("Failed to copy message text to clipboard");
            if let Some(w) = window_weak.upgrade() {
                crate::helpers::show_toast(&w, "Failed to copy to clipboard", 3);
            }
        }
    });
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

/// Extract the `@partial` mention query from chat input text.
/// Returns `Some(partial)` if the text contains an in-progress @mention
/// (the portion after the last `@` that has no spaces), or `None` otherwise.
fn extract_mention_query(text: &str) -> Option<&str> {
    // Find the last '@' in the text
    let at_pos = text.rfind('@')?;
    let after_at = &text[at_pos + 1..];
    // If there's a space after the @, the mention is complete/cancelled
    if after_at.contains(' ') {
        return None;
    }
    Some(after_at)
}

pub fn setup_mention_input_changed(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
) {
    let state = state.clone();
    let window_weak = window.as_weak();
    window.on_mention_input_changed(move |text| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let text_str = text.to_string();
        let query = extract_mention_query(&text_str);
        match query {
            Some(partial) => {
                let lower = partial.to_lowercase();
                // Gather member names from space state
                let members: Vec<String> = {
                    let app = state.borrow();
                    if let Some(space) = &app.space {
                        space
                            .members
                            .iter()
                            .filter(|m| {
                                let name_lower = m.name.to_lowercase();
                                lower.is_empty() || name_lower.starts_with(&lower)
                            })
                            .take(8)
                            .map(|m| m.name.clone())
                            .collect()
                    } else {
                        Vec::new()
                    }
                };
                if members.is_empty() {
                    w.set_mention_popup_visible(false);
                    return;
                }
                let model: Vec<SharedString> = members
                    .iter()
                    .map(|n| SharedString::from(n.as_str()))
                    .collect();
                let rc = Rc::new(VecModel::from(model));
                w.set_mention_suggestions(ModelRc::from(rc));
                w.set_mention_selected_index(0);
                w.set_mention_popup_visible(true);
            }
            None => {
                w.set_mention_popup_visible(false);
            }
        }
    });
}

pub fn setup_call_user(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    let window_weak = window.as_weak();
    window.on_call_user(move |user_id| {
        let user_id = user_id.trim().to_string();
        if user_id.is_empty() {
            return;
        }
        if let Some(w) = window_weak.upgrade() {
            crate::helpers::show_toast(&w, "Calling...", 0);
        }
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::CallUser {
                    target_user_id: user_id,
                })
                .await;
        });
    });
}

pub fn setup_accept_call(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    let window_weak = window.as_weak();
    window.on_accept_call(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let room_key = w.get_incoming_call_room().to_string();
        if room_key.is_empty() {
            return;
        }
        w.set_incoming_call_visible(false);
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::AcceptCall { room_key })
                .await;
        });
    });
}

pub fn setup_decline_call(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    let window_weak = window.as_weak();
    window.on_decline_call(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let room_key = w.get_incoming_call_room().to_string();
        if room_key.is_empty() {
            return;
        }
        w.set_incoming_call_visible(false);
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::DeclineCall { room_key })
                .await;
        });
    });
}

pub fn setup_mention_selected(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_mention_selected(move |idx| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let suggestions = w.get_mention_suggestions();
        let idx = idx as usize;
        if idx >= suggestions.row_count() {
            return;
        }
        let name = suggestions.row_data(idx).unwrap_or_default();
        let input = w.get_chat_input().to_string();
        // Find the last '@' and replace @partial with @name + space
        if let Some(at_pos) = input.rfind('@') {
            let mut result = String::with_capacity(input.len() + name.len());
            result.push_str(&input[..at_pos]);
            result.push('@');
            result.push_str(name.as_str());
            result.push(' ');
            w.set_chat_input(SharedString::from(result));
        }
        w.set_mention_popup_visible(false);
    });
}

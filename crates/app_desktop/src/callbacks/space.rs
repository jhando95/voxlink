use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use shared_types::{AppView, SignalMessage, SpaceRole};
use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

use crate::helpers;

pub fn setup_create_space(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_create_space(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let name = w.get_space_name().to_string().trim().to_string();
        let user_name = w.get_user_name().to_string().trim().to_string();
        if name.is_empty() {
            w.set_status_text("Enter a space name".into());
            return;
        }
        if user_name.is_empty() {
            w.set_status_text("Enter your name first".into());
            return;
        }
        let network = network.clone();
        w.set_status_text("Creating space...".into());
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            if let Err(e) = net
                .send_signal(&SignalMessage::CreateSpace { name, user_name })
                .await
            {
                log::error!("Failed to create space: {e}");
                if let Some(w) = window_weak.upgrade() {
                    w.set_status_text("Failed: Not connected".into());
                }
            }
        });
    });
}

pub fn setup_join_space(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_join_space(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let invite_code = w.get_space_invite_code().to_string().trim().to_string();
        let user_name = w.get_user_name().to_string().trim().to_string();
        if invite_code.is_empty() {
            w.set_status_text("Enter an invite code".into());
            return;
        }
        if user_name.is_empty() {
            w.set_status_text("Enter your name first".into());
            return;
        }
        let network = network.clone();
        w.set_status_text("Joining space...".into());
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            if let Err(e) = net
                .send_signal(&SignalMessage::JoinSpace {
                    invite_code,
                    user_name,
                })
                .await
            {
                log::error!("Failed to join space: {e}");
                if let Some(w) = window_weak.upgrade() {
                    w.set_status_text("Failed: Not connected".into());
                }
            }
        });
    });
}

pub fn setup_select_space(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let state = state.clone();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_select_space(move |space_id| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let space_id_str = space_id.to_string();
        w.set_current_space_id(space_id.clone());
        w.set_space_search_query(slint::SharedString::default());

        // Show cached space data immediately for responsiveness
        let (invite_code, had_cached_space) = {
            let s = state.borrow();
            let mut invite = None;
            let mut had_cached = false;
            if let Some(ref space) = s.space {
                if space.id == space_id_str {
                    w.set_current_space_name(space.name.clone().into());
                    w.set_current_space_invite(space.invite_code.clone().into());
                    invite = Some(space.invite_code.clone());
                    had_cached = true;
                }
            }
            (invite, had_cached)
        };
        if had_cached_space {
            crate::friends::sync_ui(&w, &state);
        }

        w.set_current_view(ui_shell::view_to_index(AppView::Space));
        state.borrow_mut().current_view = AppView::Space;

        // Re-join the space on the server so the peer is registered
        let invite_code = invite_code.or_else(|| {
            let cfg = config_store::load_config();
            cfg.saved_spaces
                .iter()
                .find(|s| s.id == space_id_str)
                .map(|s| s.invite_code.clone())
        });
        if let Some(code) = invite_code {
            let user_name = w.get_user_name().to_string();
            let network = network.clone();
            rt_handle.spawn(async move {
                let net = network.lock().await;
                let _ = net
                    .send_signal(&SignalMessage::JoinSpace {
                        invite_code: code,
                        user_name,
                    })
                    .await;
            });
        }
    });
}

pub fn setup_filter_space(window: &MainWindow, state: &Rc<RefCell<shared_types::AppState>>) {
    let window_weak = window.as_weak();
    let state = state.clone();
    window.on_space_search_changed(move |query| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let s = state.borrow();
        if let Some(ref space) = s.space {
            if w.get_current_space_id() == space.id {
                ui_shell::render_space(
                    &w,
                    space,
                    query.as_str(),
                    &s.favorite_friends,
                    &s.incoming_friend_requests,
                    &s.outgoing_friend_requests,
                    s.self_user_id.as_deref(),
                );
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub fn setup_leave_space(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    voice: &Rc<RefCell<voice_engine::VoiceSession>>,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    audio_started: &Rc<RefCell<bool>>,
    audio_active_flag: &Arc<AtomicBool>,
    screen_share: &Arc<crate::screen_share::ScreenShareController>,
    speaking_ticks: &Rc<RefCell<std::collections::HashMap<String, u64>>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let state = state.clone();
    let network = network.clone();
    let audio = audio.clone();
    let rt_handle = rt_handle.clone();
    let audio_active_flag = audio_active_flag.clone();
    let audio_started = audio_started.clone();
    let screen_share = screen_share.clone();
    let voice = voice.clone();
    let speaking_ticks = speaking_ticks.clone();
    window.on_leave_space(move || {
        log::info!("Leaving space");
        screen_share.stop_capture();

        // If in a channel, clean up audio
        if *audio_started.borrow() {
            voice.borrow_mut().reset();
            *audio_started.borrow_mut() = false;
            speaking_ticks.borrow_mut().clear();
        }

        {
            let mut s = state.borrow_mut();
            s.room = Default::default();
            s.space = None;
            s.active_direct_message_user_id = None;
            s.direct_typing_users.clear();
            s.current_view = AppView::Home;
        }

        let Some(w) = window_weak.upgrade() else {
            return;
        };
        w.set_current_view(ui_shell::view_to_index(AppView::Home));
        w.set_current_space_id(slint::SharedString::default());
        w.set_current_space_name(slint::SharedString::default());
        w.set_current_space_invite(slint::SharedString::default());
        w.set_space_search_query(slint::SharedString::default());
        w.set_confirm_delete_channel_id(slint::SharedString::default());
        w.set_visible_text_channels(0);
        w.set_visible_voice_channels(0);
        w.set_visible_members(0);
        w.set_is_space_owner(false);
        crate::signal_handler::apply_space_permissions(&w, SpaceRole::Member);
        w.set_in_space_channel(false);
        w.set_room_code(slint::SharedString::default());
        w.set_is_muted(false);
        w.set_is_deafened(false);
        w.set_chat_channel_id(slint::SharedString::default());
        w.set_chat_channel_name(slint::SharedString::default());
        w.set_chat_is_direct_message(false);
        w.set_chat_context_subtitle(slint::SharedString::default());
        w.set_chat_back_view(ui_shell::view_to_index(AppView::Space));
        w.set_chat_input(slint::SharedString::default());
        w.set_chat_pinned_messages(
            std::rc::Rc::new(slint::VecModel::<ui_shell::ChatMessage>::from(Vec::new())).into(),
        );
        w.set_chat_typing_text(slint::SharedString::default());
        w.set_editing_message_id(slint::SharedString::default());
        w.set_editing_original_content(slint::SharedString::default());
        w.set_reply_target_message_id(slint::SharedString::default());
        w.set_reply_target_sender_name(slint::SharedString::default());
        w.set_reply_target_preview(slint::SharedString::default());
        w.set_status_text("Connected".into());
        w.set_window_title("Voxlink".into());
        w.set_has_screen_share(false);
        w.set_is_sharing_screen(false);
        w.set_screen_share_owner_name(slint::SharedString::default());
        w.set_screen_share_owner_id(slint::SharedString::default());
        w.set_screen_share_image(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
            slint::Rgba8Pixel,
        >::new(1, 1)));
        ui_shell::set_channels(&w, &[]);
        ui_shell::set_members(&w, &[]);
        ui_shell::set_space_audit_log(&w, &[]);
        crate::friends::sync_ui(&w, &state);

        crate::helpers::sync_saved_spaces_ui(&w, None);

        let network = network.clone();
        let audio = audio.clone();
        let flag = audio_active_flag.clone();
        rt_handle.spawn(async move {
            {
                let net = network.lock().await;
                let _ = net.send_signal(&SignalMessage::LeaveSpace).await;
            }
            let mut aud = audio.lock().await;
            aud.stop_capture();
            aud.stop_playback();
            flag.store(false, Ordering::Relaxed);
        });
    });
}

pub fn setup_copy_invite_code(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_copy_invite_code(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let code = w.get_current_space_invite().to_string();
        if !code.is_empty() {
            if helpers::copy_to_clipboard(&code) {
                w.set_status_text("Invite code copied".into());
            } else {
                w.set_status_text("Failed to copy to clipboard".into());
            }
        }
    });
}

pub fn setup_copy_share_message(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_copy_share_message(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let invite = w.get_current_space_invite().to_string();
        if invite.is_empty() {
            return;
        }

        let name = w.get_current_space_name().to_string();
        let share_message = if name.is_empty() {
            format!(
                "Join me on Voxlink.\nInvite code: {invite}\nOpen Voxlink and paste the invite code to jump in."
            )
        } else {
            format!(
                "Join {name} on Voxlink.\nInvite code: {invite}\nOpen Voxlink and paste the invite code to jump in."
            )
        };

        if helpers::copy_to_clipboard(&share_message) {
            w.set_status_text("Share message copied".into());
        } else {
            w.set_status_text("Failed to copy to clipboard".into());
        }
    });
}

pub fn setup_delete_space(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_delete_space(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        w.set_status_text("Deleting space...".into());
        let network = network.clone();
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            if let Err(e) = net.send_signal(&SignalMessage::DeleteSpace).await {
                log::error!("Failed to delete space: {e}");
                if let Some(w) = window_weak.upgrade() {
                    w.set_status_text("Failed to delete space".into());
                }
            }
        });
    });
}

pub fn setup_kick_member(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_kick_member(move |member_id| {
        let member_id = member_id.to_string();
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::KickMember { member_id })
                .await;
        });
    });
}

pub fn setup_ban_member(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_ban_member(move |member_id| {
        let member_id = member_id.to_string();
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::BanMember { member_id })
                .await;
        });
    });
}

pub fn setup_server_mute_member(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_server_mute_member(move |member_id, muted| {
        let member_id = member_id.to_string();
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::MuteMember { member_id, muted })
                .await;
        });
    });
}

pub fn setup_set_member_role(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_set_member_role(move |user_id, role_index| {
        let user_id = user_id.to_string();
        if user_id.is_empty() {
            return;
        }
        let role = match role_index {
            2 => SpaceRole::Admin,
            1 => SpaceRole::Moderator,
            _ => SpaceRole::Member,
        };
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::SetMemberRole { user_id, role })
                .await;
        });
    });
}

pub fn setup_set_user_status(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_set_user_status(move |status| {
        let status = status.to_string().trim().to_string();
        if status.is_empty() {
            return;
        }
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::SetUserStatus { status })
                .await;
        });
    });
}

pub fn setup_set_channel_topic(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_set_channel_topic(move |channel_id, topic| {
        let channel_id = channel_id.to_string();
        let topic = topic.to_string().trim().to_string();
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::SetChannelTopic { channel_id, topic })
                .await;
        });
    });
}

pub fn setup_set_profile(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_set_profile(move |bio| {
        let bio = bio.to_string().trim().to_string();
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::SetProfile { bio })
                .await;
        });
    });
}

pub fn setup_set_channel_user_limit(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_set_channel_user_limit(move |channel_id, user_limit| {
        let channel_id = channel_id.to_string();
        let user_limit = user_limit.max(0) as u32;
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::SetChannelUserLimit { channel_id, user_limit })
                .await;
        });
    });
}

pub fn setup_set_channel_slow_mode(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_set_channel_slow_mode(move |channel_id, slow_mode_secs| {
        let channel_id = channel_id.to_string();
        let slow_mode_secs = slow_mode_secs.max(0) as u32;
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::SetChannelSlowMode { channel_id, slow_mode_secs })
                .await;
        });
    });
}

pub fn setup_set_channel_category(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_set_channel_category(move |channel_id, category| {
        let channel_id = channel_id.to_string();
        let category = category.to_string().trim().to_string();
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::SetChannelCategory { channel_id, category })
                .await;
        });
    });
}

pub fn setup_set_channel_status(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_set_channel_status(move |channel_id, status| {
        let channel_id = channel_id.to_string();
        let status = status.to_string().trim().to_string();
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::SetChannelStatus { channel_id, status })
                .await;
        });
    });
}

pub fn setup_timeout_member(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_timeout_member(move |member_id, duration_secs| {
        let member_id = member_id.to_string();
        let duration_secs = duration_secs.max(0) as u64;
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::TimeoutMember { member_id, duration_secs })
                .await;
        });
    });
}

pub fn setup_set_priority_speaker(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_set_priority_speaker(move |peer_id, enabled| {
        let peer_id = peer_id.to_string();
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::SetPrioritySpeaker { peer_id, enabled })
                .await;
        });
    });
}

pub fn setup_whisper(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    // WhisperTo
    {
        let network = network.clone();
        let rt_handle = rt_handle.clone();
        window.on_whisper_to(move |target_ids_csv| {
            let csv: &str = target_ids_csv.as_str();
            let target_peer_ids: Vec<String> = csv
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if target_peer_ids.is_empty() {
                return;
            }
            let network = network.clone();
            rt_handle.spawn(async move {
                let net = network.lock().await;
                let _ = net
                    .send_signal(&SignalMessage::WhisperTo { target_peer_ids })
                    .await;
            });
        });
    }
    // WhisperStopped
    {
        let network = network.clone();
        let rt_handle = rt_handle.clone();
        window.on_stop_whisper(move || {
            let network = network.clone();
            rt_handle.spawn(async move {
                let net = network.lock().await;
                let _ = net
                    .send_signal(&SignalMessage::WhisperStopped)
                    .await;
            });
        });
    }
}

pub fn setup_save_user_note(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_save_user_note(move |user_id, note| {
        let user_id = user_id.to_string();
        let note = note.to_string().trim().to_string();
        // Save locally — never sent to server
        let mut cfg = config_store::load_config();
        if note.is_empty() {
            cfg.user_notes.remove(&user_id);
        } else {
            cfg.user_notes.insert(user_id, note);
        }
        let _ = config_store::save_config(&cfg);
        if let Some(w) = window_weak.upgrade() {
            w.set_status_text("Note saved".into());
        }
    });
}

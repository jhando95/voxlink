mod channel;
pub(crate) mod chat;
pub mod connection;
mod member;
mod room;
mod space;

pub use space::apply_space_permissions;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use shared_types::{AppView, SignalMessage};
use slint::Model;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

/// Shared context for audio setup — avoids passing 5+ params through every handler.
pub struct AudioContext {
    pub audio_started: Rc<RefCell<bool>>,
    pub audio: Arc<TokioMutex<audio_core::AudioEngine>>,
    pub media: Arc<TokioMutex<media_transport::MediaSession>>,
    pub network: Arc<TokioMutex<net_control::NetworkClient>>,
    pub audio_active_flag: Arc<AtomicBool>,
    pub screen_share: Arc<crate::screen_share::ScreenShareController>,
    pub rt_handle: tokio::runtime::Handle,
    pub saved_input_device: Rc<RefCell<Option<String>>>,
    pub saved_output_device: Rc<RefCell<Option<String>>>,
}

/// Process a batch of signal messages from the server, updating app state and UI.
pub fn process_signals(
    signals: &[SignalMessage],
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    ctx: &AudioContext,
    tick: u64,
) {
    for signal in signals.iter() {
        match signal {
            SignalMessage::RoomCreated { room_code } => {
                room::handle_room_entered(w, state, room_code, &[], ctx);
            }
            SignalMessage::RoomJoined {
                room_code,
                participants,
            } => {
                room::handle_room_entered(w, state, room_code, participants, ctx);
            }
            SignalMessage::PeerJoined { peer } => {
                room::handle_peer_joined(w, state, peer, &ctx.audio);
                // Notify only when not viewing the room; suppress in DND mode
                let in_room_view = w.get_current_view() == ui_shell::view_to_index(shared_types::AppView::Room);
                let is_dnd = w.get_status_preset() == 2;
                if w.get_notifications_enabled() && !in_room_view && !is_dnd {
                    crate::helpers::send_notification("Voxlink", &format!("{} joined", peer.name));
                }
            }
            SignalMessage::PeerLeft { peer_id } => {
                room::handle_peer_left(w, state, peer_id, &ctx.audio);
            }
            SignalMessage::PeerMuteChanged { peer_id, is_muted } => {
                room::handle_peer_mute_changed(w, state, peer_id, *is_muted);
            }
            SignalMessage::PeerDeafenChanged {
                peer_id,
                is_deafened,
            } => {
                room::handle_peer_deafen_changed(w, state, peer_id, *is_deafened);
            }
            SignalMessage::ScreenShareStarted {
                sharer_id,
                sharer_name,
                is_self,
            } => {
                room::handle_screen_share_started(w, state, sharer_id, sharer_name, *is_self, ctx);
            }
            SignalMessage::ScreenShareStopped { sharer_id } => {
                room::handle_screen_share_stopped(w, state, sharer_id, ctx);
            }
            SignalMessage::Error { message } => {
                let stale_saved_space = message.contains("Invalid invite code")
                    && w.get_current_view() == ui_shell::view_to_index(AppView::Space)
                    && state.borrow().space.is_none()
                    && !w.get_current_space_id().is_empty();
                if stale_saved_space {
                    let space_id = w.get_current_space_id().to_string();
                    crate::helpers::remove_saved_space_async(space_id.clone());
                    crate::helpers::sync_saved_spaces_ui(w, Some(&space_id));
                    state.borrow_mut().current_view = AppView::Home;
                    w.set_current_view(ui_shell::view_to_index(AppView::Home));
                    w.set_current_space_id(slint::SharedString::default());
                    w.set_current_space_name(slint::SharedString::default());
                    w.set_current_space_invite(slint::SharedString::default());
                    w.set_space_search_query(slint::SharedString::default());
                    w.set_confirm_delete_channel_id(slint::SharedString::default());
                    w.set_is_space_owner(false);
                    space::apply_space_permissions(w, shared_types::SpaceRole::Member);
                    ui_shell::set_channels(w, &[]);
                    ui_shell::set_members(w, &[]);
                    ui_shell::set_space_audit_log(w, &[]);
                    w.set_status_text("Saved space no longer exists on the server".into());
                    crate::helpers::show_toast(w, "Space no longer exists on the server", 2);
                    // Stop audio if it was running (user may have been in a channel)
                    if *ctx.audio_started.borrow() {
                        *ctx.audio_started.borrow_mut() = false;
                        ctx.audio_active_flag.store(false, std::sync::atomic::Ordering::Relaxed);
                        let audio = ctx.audio.clone();
                        ctx.rt_handle.spawn(async move {
                            let mut aud = audio.lock().await;
                            aud.stop_capture();
                            aud.stop_playback();
                        });
                    }
                    continue;
                }
                log::error!("Server error: {message}");
                crate::helpers::show_toast(w, &format!("Error: {message}"), 3);
            }
            SignalMessage::SpaceCreated { space, channels } => {
                space::handle_space_created(w, state, space, channels);
            }
            SignalMessage::SpaceJoined {
                space,
                channels,
                members,
                welcome_message,
            } => {
                space::handle_space_joined(w, state, space, channels, members, welcome_message);
            }
            SignalMessage::ChannelCreated { channel } => {
                channel::handle_channel_created(w, state, channel);
            }
            SignalMessage::ChannelDeleted { channel_id } => {
                channel::handle_channel_deleted(w, state, channel_id);
            }
            SignalMessage::ChannelJoined {
                channel_id,
                channel_name,
                participants,
                voice_quality,
            } => {
                channel::handle_channel_joined(
                    w,
                    state,
                    channel_id,
                    channel_name,
                    participants,
                    *voice_quality,
                    ctx,
                );
            }
            SignalMessage::ChannelLeft => {
                channel::handle_channel_left(w, state, ctx);
            }
            SignalMessage::MemberOnline { member } => {
                member::handle_member_online(w, state, member);
            }
            SignalMessage::MemberOffline { member_id } => {
                member::handle_member_offline(w, state, member_id);
            }
            SignalMessage::MemberChannelChanged {
                member_id,
                channel_id,
                channel_name,
            } => {
                member::handle_member_channel_changed(
                    w,
                    state,
                    member_id,
                    channel_id,
                    channel_name,
                );
            }
            SignalMessage::UserStatusChanged { member_id, status } => {
                member::handle_user_status_changed(w, state, member_id, status);
            }
            SignalMessage::MemberRoleChanged { user_id, role } => {
                member::handle_member_role_changed(w, state, user_id, *role);
            }
            SignalMessage::ChannelTopicChanged { channel_id, topic } => {
                channel::handle_channel_topic_changed(w, state, channel_id, topic);
            }
            SignalMessage::SpaceAuditLogSnapshot { entries } => {
                member::handle_space_audit_snapshot(w, state, entries);
            }
            SignalMessage::SpaceAuditLogAppended { entry } => {
                member::handle_space_audit_appended(w, state, entry);
            }
            SignalMessage::SpaceDeleted => {
                space::handle_space_deleted(w, state, ctx);
            }
            SignalMessage::SpaceRenamed { name } => {
                let mut s = state.borrow_mut();
                if let Some(ref mut space) = s.space {
                    space.name = name.clone();
                }
                w.set_current_space_name(name.into());
            }
            SignalMessage::SpaceDescriptionChanged { description } => {
                let mut s = state.borrow_mut();
                if let Some(ref mut space) = s.space {
                    space.description = description.clone();
                }
                w.set_space_description(description.into());
            }
            SignalMessage::TextChannelSelected {
                channel_id,
                channel_name,
                history,
            } => {
                chat::handle_text_channel_selected(w, state, channel_id, channel_name, history);
            }
            SignalMessage::DirectMessageSelected {
                user_id,
                user_name,
                history,
            } => {
                chat::handle_direct_message_selected(w, state, user_id, user_name, history);
            }
            SignalMessage::TextMessage {
                channel_id,
                message,
            } => {
                chat::handle_text_message(w, state, channel_id, message);
            }
            SignalMessage::DirectMessage { user_id, message } => {
                chat::handle_direct_message(w, state, user_id, message);
            }
            SignalMessage::TypingState {
                channel_id,
                user_name,
                is_typing,
            } => {
                chat::handle_typing_state(w, state, channel_id, user_name, *is_typing, tick);
            }
            SignalMessage::DirectTypingState {
                user_id,
                user_name,
                is_typing,
            } => {
                chat::handle_direct_typing_state(w, state, user_id, user_name, *is_typing, tick);
            }
            // Auth (Milestone 4)
            SignalMessage::Authenticated { token, user_id } => {
                log::info!("Authenticated as {user_id}");
                state.borrow_mut().self_user_id = Some(user_id.clone());
                w.set_first_run(false);
                if !token.is_empty() {
                    crate::helpers::save_auth_token_async(token.clone());
                }
                crate::friends::sync_presence_subscription(state, &ctx.network, &ctx.rt_handle);
                // Request UDP transport after auth succeeds
                let net = ctx.network.clone();
                ctx.rt_handle.spawn(async move {
                    let net = net.lock().await;
                    if let Err(e) = net.request_udp().await {
                        log::debug!("Failed to request UDP: {e}");
                    }
                });
            }
            SignalMessage::FriendSnapshot {
                friends,
                incoming_requests,
                outgoing_requests,
            } => {
                crate::friends::handle_friend_snapshot(
                    w,
                    state,
                    friends,
                    incoming_requests,
                    outgoing_requests,
                );
                crate::friends::sync_presence_subscription(state, &ctx.network, &ctx.rt_handle);
            }
            SignalMessage::FriendPresenceSnapshot { presences } => {
                crate::friends::handle_presence_snapshot(w, state, presences);
            }
            SignalMessage::FriendPresenceChanged { presence } => {
                crate::friends::handle_presence_changed(w, state, presence);
            }
            // Chat improvements (Milestone 5)
            SignalMessage::TextMessageEdited {
                channel_id,
                message_id,
                new_content,
            } => {
                chat::handle_text_message_edited(w, channel_id, message_id, new_content);
            }
            SignalMessage::DirectMessageEdited {
                user_id,
                message_id,
                new_content,
            } => {
                chat::handle_direct_message_edited(w, state, user_id, message_id, new_content);
            }
            SignalMessage::TextMessageDeleted {
                channel_id,
                message_id,
            } => {
                chat::handle_text_message_deleted(w, channel_id, message_id);
            }
            SignalMessage::DirectMessageDeleted {
                user_id,
                message_id,
            } => {
                chat::handle_direct_message_deleted(w, state, user_id, message_id);
            }
            SignalMessage::MessageReaction {
                channel_id,
                message_id,
                emoji,
                user_name,
            } => {
                chat::handle_message_reaction(w, state, channel_id, message_id, emoji, user_name);
            }
            SignalMessage::DirectMessageReaction {
                user_id,
                message_id,
                emoji,
                user_name,
            } => {
                chat::handle_direct_message_reaction(
                    w, state, user_id, message_id, emoji, user_name,
                );
            }
            SignalMessage::MessagePinned {
                channel_id,
                message_id,
                pinned,
            } => {
                chat::handle_message_pinned(w, channel_id, message_id, *pinned);
            }
            // Moderation (Milestone 6)
            SignalMessage::Kicked { reason } => {
                log::warn!("Kicked: {reason}");
                crate::helpers::show_toast(w, &format!("Kicked: {reason}"), 3);
                // Clean up space/room state
                {
                    let mut s = state.borrow_mut();
                    s.room = Default::default();
                    s.space = None;
                    s.active_direct_message_user_id = None;
                    s.direct_typing_users.clear();
                    s.current_view = shared_types::AppView::Home;
                }
                w.set_current_view(0);
                w.set_current_space_id(slint::SharedString::default());
                w.set_current_space_name(slint::SharedString::default());
                w.set_current_space_invite(slint::SharedString::default());
                w.set_space_search_query(slint::SharedString::default());
                w.set_visible_text_channels(0);
                w.set_visible_voice_channels(0);
                w.set_visible_members(0);
                w.set_chat_channel_id(slint::SharedString::default());
                w.set_chat_channel_name(slint::SharedString::default());
                w.set_chat_is_direct_message(false);
                w.set_chat_context_subtitle(slint::SharedString::default());
                w.set_chat_back_view(ui_shell::view_to_index(shared_types::AppView::Space));
                w.set_chat_input(slint::SharedString::default());
                w.set_chat_pinned_messages(
                    std::rc::Rc::new(slint::VecModel::<ui_shell::ChatMessage>::from(Vec::new()))
                        .into(),
                );
                w.set_chat_typing_text(slint::SharedString::default());
                w.set_editing_message_id(slint::SharedString::default());
                w.set_editing_original_content(slint::SharedString::default());
                w.set_reply_target_message_id(slint::SharedString::default());
                w.set_reply_target_sender_name(slint::SharedString::default());
                w.set_reply_target_preview(slint::SharedString::default());
                w.set_is_space_owner(false);
                space::apply_space_permissions(w, shared_types::SpaceRole::Member);
                w.set_in_space_channel(false);
                w.set_room_code(slint::SharedString::default());
                w.set_is_muted(false);
                w.set_is_deafened(false);
                w.set_status_text(reason.clone().into());
                w.set_has_screen_share(false);
                w.set_is_sharing_screen(false);
                w.set_screen_share_owner_name(slint::SharedString::default());
                w.set_screen_share_owner_id(slint::SharedString::default());
                w.set_screen_share_image(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
                    slint::Rgba8Pixel,
                >::new(1, 1)));
                w.set_recording_active(false);
                w.set_recording_user(slint::SharedString::default());
                ui_shell::set_channels(w, &[]);
                ui_shell::set_members(w, &[]);
                ui_shell::set_space_audit_log(w, &[]);
                crate::friends::sync_ui(w, state);
                ctx.screen_share.stop_capture();
                // Stop audio streams — prevents mic/playback leaking after kick
                *ctx.audio_started.borrow_mut() = false;
                ctx.audio_active_flag.store(false, std::sync::atomic::Ordering::Relaxed);
                let audio = ctx.audio.clone();
                ctx.rt_handle.spawn(async move {
                    let mut aud = audio.lock().await;
                    aud.stop_capture();
                    aud.stop_playback();
                });
                if w.get_notifications_enabled() && w.get_status_preset() != 2 {
                    crate::helpers::send_notification("Voxlink", reason);
                }
            }
            SignalMessage::MemberMuted { member_id, muted } => {
                room::handle_peer_mute_changed(w, state, member_id, *muted);
            }
            SignalMessage::MemberServerDeafened { member_id, deafened } => {
                log::info!("Member {member_id} server-deafened: {deafened}");
                w.set_status_text(
                    if *deafened {
                        format!("Member server-deafened")
                    } else {
                        format!("Member server-undeafened")
                    }
                    .into(),
                );
            }
            SignalMessage::ServerShutdown => {
                log::info!("Server is shutting down");
                w.set_status_text("Server restarting, reconnecting...".into());
                w.set_room_status("Server restarting...".into());
                crate::helpers::show_toast(w, "Server restarting, reconnecting...", 2);
            }
            SignalMessage::SearchResults {
                channel_id,
                messages,
            } => {
                chat::handle_search_results(w, state, channel_id, messages);
            }
            SignalMessage::ProfileUpdated { user_id, bio } => {
                member::handle_profile_updated(w, state, user_id, bio);
            }
            // UDP transport negotiation
            SignalMessage::UdpReady { token, port } => {
                log::info!("Server offered UDP transport on port {port}");
                let net = ctx.network.clone();
                let token = token.clone();
                let port = *port;
                ctx.rt_handle.spawn(async move {
                    let net = net.lock().await;
                    match net.setup_udp(&token, port).await {
                        Ok(()) => log::info!("UDP audio transport activated"),
                        Err(e) => log::warn!("Failed to set up UDP transport: {e}"),
                    }
                });
            }
            SignalMessage::UdpUnavailable => {
                log::info!("Server does not support UDP transport, using WebSocket");
            }
            // Channel settings (v0.7)
            SignalMessage::ChannelUserLimitChanged {
                channel_id,
                user_limit,
            } => {
                channel::handle_channel_setting_changed(w, state, channel_id, |ch| {
                    ch.user_limit = *user_limit;
                });
            }
            SignalMessage::ChannelSlowModeChanged {
                channel_id,
                slow_mode_secs,
            } => {
                channel::handle_channel_setting_changed(w, state, channel_id, |ch| {
                    ch.slow_mode_secs = *slow_mode_secs;
                });
                // Update live countdown if viewing this channel
                if w.get_chat_channel_id() == channel_id.as_str() && !w.get_chat_is_direct_message() {
                    w.set_slow_mode_secs(*slow_mode_secs as i32);
                }
            }
            SignalMessage::ChannelCategoryChanged {
                channel_id,
                category,
            } => {
                channel::handle_channel_setting_changed(w, state, channel_id, |ch| {
                    ch.category = category.clone();
                });
            }
            SignalMessage::ChannelStatusChanged {
                channel_id,
                status,
            } => {
                channel::handle_channel_setting_changed(w, state, channel_id, |ch| {
                    ch.status = status.clone();
                });
            }
            SignalMessage::ChannelAutoDeleteChanged {
                channel_id,
                auto_delete_hours,
            } => {
                channel::handle_channel_setting_changed(w, state, channel_id, |ch| {
                    ch.auto_delete_hours = *auto_delete_hours;
                });
            }
            SignalMessage::PrioritySpeakerChanged { peer_id, enabled } => {
                room::handle_priority_speaker_changed(w, state, peer_id, *enabled);
            }
            SignalMessage::MemberTimedOut {
                member_id,
                until_epoch,
            } => {
                log::info!("Member {member_id} timed out until {until_epoch}");
                w.set_status_text(
                    format!("Member timed out until {}", until_epoch).into(),
                );
            }
            SignalMessage::MemberTimeoutExpired { member_id } => {
                log::info!("Timeout expired for {member_id}");
            }
            // v0.8.0: Block/Unblock acknowledgments
            SignalMessage::UserBlocked { user_id } => {
                log::info!("Blocked user: {user_id}");
                w.set_status_text("Blocked user".to_string().into());
                crate::helpers::show_toast(w, "User blocked", 1);
            }
            SignalMessage::UserUnblocked { user_id } => {
                log::info!("Unblocked user: {user_id}");
                w.set_status_text("Unblocked user".to_string().into());
                crate::helpers::show_toast(w, "User unblocked", 1);
            }
            // v0.8.0: Ban list
            SignalMessage::BanList { bans } => {
                log::info!("Received ban list: {} entries", bans.len());
                // Ban list UI would be handled here
            }
            // v0.8.0: Status presets
            SignalMessage::StatusPresetChanged { member_id, preset } => {
                log::trace!("Status preset changed for {member_id}: {preset:?}");
            }
            // v0.8.0: Mention notifications
            SignalMessage::MentionNotification {
                channel_id: _,
                channel_name,
                sender_name,
                preview,
            } => {
                if w.get_notifications_enabled() && w.get_status_preset() != 2 {
                    crate::helpers::send_notification(
                        &format!("{sender_name} in #{channel_name}"),
                        preview,
                    );
                }
            }
            // v0.8.0: Group DMs
            SignalMessage::GroupDMCreated {
                group_id,
                name,
                members: _,
            } => {
                log::info!("Group DM created: {name} ({group_id})");
                w.set_status_text(format!("Group created: {name}").into());
            }
            SignalMessage::GroupDMSelected {
                group_id: _,
                name,
                members: _,
                history,
            } => {
                let my_name = w.get_user_name().to_string();
                w.set_chat_channel_name(name.into());
                w.set_chat_is_direct_message(true);
                w.set_chat_context_subtitle("Group message".into());
                ui_shell::set_chat_messages(w, history, &my_name);
                w.set_current_view(ui_shell::view_to_index(AppView::TextChat));
            }
            SignalMessage::GroupMessage {
                group_id: _,
                message,
            } => {
                let my_name = w.get_user_name().to_string();
                let chat_msg = ui_shell::text_msg_to_chat_msg(message, &my_name);
                let messages: slint::ModelRc<ui_shell::ChatMessage> = w.get_chat_messages();
                if let Some(model) = messages
                    .as_any()
                    .downcast_ref::<slint::VecModel<ui_shell::ChatMessage>>()
                {
                    model.push(chat_msg);
                }
            }
            // v0.8.0: Invite settings
            SignalMessage::InviteSettingsUpdated {
                expires_hours,
                max_uses,
                uses,
            } => {
                log::info!("Invite settings updated: expires={expires_hours:?} max={max_uses:?} uses={uses}");
            }
            // v0.8.0: Threads
            SignalMessage::ThreadMessages {
                channel_id: _,
                root_message_id: _,
                messages,
            } => {
                log::info!("Thread messages received ({} replies)", messages.len());
                let self_name = w.get_user_name().to_string();
                if let Some(parent) = messages.first() {
                    w.set_thread_parent_sender(parent.sender_name.clone().into());
                    w.set_thread_parent_content(
                        ui_shell::render_markdown(&parent.content).0.into(),
                    );
                    w.set_thread_parent_timestamp(
                        ui_shell::format_timestamp(parent.timestamp).into(),
                    );
                }
                let replies: Vec<ui_shell::ChatMessage> = messages
                    .iter()
                    .skip(1)
                    .map(|m| ui_shell::text_msg_to_chat_msg(m, &self_name))
                    .collect();
                let model = std::rc::Rc::new(slint::VecModel::from(replies));
                w.set_thread_messages(model.into());
                w.set_thread_panel_visible(true);
            }
            // v0.8.0: Nicknames
            SignalMessage::NicknameChanged {
                user_id,
                nickname,
            } => {
                log::trace!("Nickname changed for {user_id}: {nickname:?}");
                // Update member list display name if in space view
                let mut s = state.borrow_mut();
                if let Some(ref mut space) = s.space {
                    if let Some(member) = space.members.iter_mut().find(|m| {
                        m.user_id.as_deref() == Some(user_id.as_str())
                    }) {
                        member.nickname = nickname.clone();
                    }
                }
            }
            // v0.8.0: Account system
            SignalMessage::AccountCreated { token, user_id } => {
                log::info!("Account created: {user_id}");
                state.borrow_mut().self_user_id = Some(user_id.clone());
                w.set_first_run(false);
                w.set_is_logged_in(true);
                w.set_show_login_view(false);
                w.set_auth_error(slint::SharedString::default());
                crate::helpers::show_toast(w, "Account created", 1);
                if !token.is_empty() {
                    crate::helpers::save_auth_token_async(token.clone());
                }
                // Also authenticate to get full session features
                let net = ctx.network.clone();
                let name = w.get_user_name().to_string();
                let tok = token.clone();
                ctx.rt_handle.spawn(async move {
                    let net = net.lock().await;
                    let _ = net.send_signal(&SignalMessage::Authenticate {
                        token: Some(tok),
                        user_name: name,
                    }).await;
                });
            }
            SignalMessage::LoginSuccess { token, user_id, display_name } => {
                log::info!("Login success: {user_id} ({display_name})");
                state.borrow_mut().self_user_id = Some(user_id.clone());
                w.set_first_run(false);
                w.set_is_logged_in(true);
                w.set_show_login_view(false);
                w.set_auth_error(slint::SharedString::default());
                w.set_user_name(display_name.as_str().into());
                crate::helpers::show_toast(w, &format!("Welcome back, {display_name}"), 1);
                if !token.is_empty() {
                    crate::helpers::save_auth_token_async(token.clone());
                }
                // Save email to config
                let email = w.get_auth_email().to_string();
                w.set_account_email(email.as_str().into());
                crate::helpers::save_account_email_async(email);
                // Request UDP after login
                let net = ctx.network.clone();
                ctx.rt_handle.spawn(async move {
                    let net = net.lock().await;
                    if let Err(e) = net.request_udp().await {
                        log::debug!("Failed to request UDP: {e}");
                    }
                });
                crate::friends::sync_presence_subscription(state, &ctx.network, &ctx.rt_handle);
            }
            SignalMessage::AuthError { message } => {
                log::warn!("Auth error: {message}");
                w.set_auth_error(message.as_str().into());
                crate::helpers::show_toast(w, &format!("Auth error: {message}"), 3);
            }
            SignalMessage::LoggedOut => {
                log::info!("Logged out");
                state.borrow_mut().self_user_id = None;
                w.set_is_logged_in(false);
                w.set_account_email(slint::SharedString::default());
                crate::helpers::clear_auth_token_async();
                crate::helpers::show_toast(w, "Logged out", 0);
            }
            SignalMessage::PasswordChanged => {
                log::info!("Password changed successfully");
                crate::helpers::show_toast(w, "Password changed", 1);
            }
            SignalMessage::AllSessionsRevoked => {
                log::info!("All sessions revoked — current session re-authenticated");
                crate::helpers::show_toast(w, "All other sessions revoked", 1);
            }
            SignalMessage::ChannelsReordered { channel_ids } => {
                let mut s = state.borrow_mut();
                if let Some(ref mut space) = s.space {
                    // Reorder channels to match server's new order
                    let mut reordered = Vec::with_capacity(space.channels.len());
                    for cid in channel_ids {
                        if let Some(pos) = space.channels.iter().position(|c| c.id == *cid) {
                            reordered.push(space.channels[pos].clone());
                        }
                    }
                    // Append any channels not in the reorder list (shouldn't happen, but safe)
                    for ch in &space.channels {
                        if !reordered.iter().any(|r| r.id == ch.id) {
                            reordered.push(ch.clone());
                        }
                    }
                    space.channels = reordered;
                    drop(s);
                    // Refresh UI
                    let s = state.borrow();
                    if let Some(ref space) = s.space {
                        ui_shell::set_channels(w, &space.channels);
                    }
                }
            }
            // v0.10.0: Role colors
            SignalMessage::RoleColorChanged { role, color } => {
                member::handle_role_color_changed(w, state, *role, color);
            }
            // v0.10.0: Activity status
            SignalMessage::ActivityChanged { member_id, activity } => {
                member::handle_activity_changed(w, state, member_id, activity);
            }
            // v0.10.0: Display name / account management
            SignalMessage::DisplayNameChanged { user_id, name } => {
                log::info!("Display name changed for {user_id}: {name}");
                // Show toast if it's our own display name change
                if state.borrow().self_user_id.as_deref() == Some(user_id.as_str()) {
                    w.set_user_name(name.as_str().into());
                    crate::helpers::show_toast(w, "Display name updated", 1);
                }
            }
            SignalMessage::AccountDeleted => {
                log::info!("Account deleted");
                state.borrow_mut().self_user_id = None;
                w.set_is_logged_in(false);
                crate::helpers::clear_auth_token_async();
                crate::helpers::show_toast(w, "Account deleted", 1);
            }
            // v0.10.0: Scheduled events
            SignalMessage::ScheduledEventCreated { event } => {
                log::info!("Scheduled event created: {} ({})", event.title, event.id);
                let new_data = ui_shell::scheduled_event_to_data(event);
                let model: slint::ModelRc<ui_shell::ScheduledEventData> =
                    w.get_scheduled_events();
                if let Some(vec_model) = model
                    .as_any()
                    .downcast_ref::<slint::VecModel<ui_shell::ScheduledEventData>>()
                {
                    vec_model.push(new_data);
                } else {
                    // Model is empty default — replace with a new VecModel
                    let vm = slint::VecModel::from(vec![new_data]);
                    w.set_scheduled_events(
                        std::rc::Rc::new(vm).into(),
                    );
                }
            }
            SignalMessage::ScheduledEventDeleted { event_id } => {
                log::info!("Scheduled event deleted: {event_id}");
                let model: slint::ModelRc<ui_shell::ScheduledEventData> =
                    w.get_scheduled_events();
                if let Some(vec_model) = model
                    .as_any()
                    .downcast_ref::<slint::VecModel<ui_shell::ScheduledEventData>>()
                {
                    for i in 0..vec_model.row_count() {
                        if vec_model.row_data(i).map_or(false, |e| e.id == event_id.as_str()) {
                            vec_model.remove(i);
                            break;
                        }
                    }
                }
            }
            SignalMessage::EventInterestUpdated {
                event_id,
                interested_count,
                is_interested,
            } => {
                log::info!(
                    "Event interest updated: {event_id} — count={interested_count}, self={is_interested}"
                );
                let model: slint::ModelRc<ui_shell::ScheduledEventData> =
                    w.get_scheduled_events();
                if let Some(vec_model) = model
                    .as_any()
                    .downcast_ref::<slint::VecModel<ui_shell::ScheduledEventData>>()
                {
                    for i in 0..vec_model.row_count() {
                        if let Some(mut evt) = vec_model.row_data(i) {
                            if evt.id == event_id.as_str() {
                                evt.interested_count = *interested_count as i32;
                                evt.is_interested = *is_interested;
                                vec_model.set_row_data(i, evt);
                                break;
                            }
                        }
                    }
                }
            }
            SignalMessage::ScheduledEventList { events } => {
                log::info!("Received scheduled event list: {} events", events.len());
                ui_shell::set_scheduled_events(w, events);
            }
            // v0.10.0: Recording
            SignalMessage::RecordingStarted {
                channel_id,
                started_by,
            } => {
                log::info!("Recording started in channel {channel_id} by {started_by}");
                w.set_recording_active(true);
                w.set_recording_user(started_by.as_str().into());
            }
            SignalMessage::RecordingStopped { channel_id } => {
                log::info!("Recording stopped in channel {channel_id}");
                w.set_recording_active(false);
                w.set_recording_user(slint::SharedString::default());
            }
            // v0.10.0: Welcome message
            SignalMessage::WelcomeMessageChanged { message } => {
                log::info!("Welcome message updated: {message}");
                w.set_welcome_message(message.as_str().into());
            }
            // v0.10.0: Message scheduling
            SignalMessage::MessageScheduled {
                schedule_id,
                channel_id,
                content,
                send_at,
            } => {
                log::info!(
                    "Message scheduled: {schedule_id} in {channel_id} at {send_at} — {content}"
                );
            }
            SignalMessage::ScheduledMessageCancelled { schedule_id } => {
                log::info!("Scheduled message cancelled: {schedule_id}");
            }
            // Server discovery
            SignalMessage::PublicSpaceList { spaces } => {
                log::info!("Received {} public spaces", spaces.len());
                let items: Vec<ui_shell::PublicSpaceData> = spaces.iter().map(|s| {
                    let initial = s.name.chars().next().unwrap_or('?').to_uppercase().to_string();
                    let ci = s.name.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32)) % 8;
                    ui_shell::PublicSpaceData {
                        id: s.id.clone().into(),
                        name: s.name.clone().into(),
                        description: s.description.clone().into(),
                        invite_code: s.invite_code.clone().into(),
                        member_count: s.member_count as i32,
                        channel_count: s.channel_count as i32,
                        online_count: s.online_count as i32,
                        initial: initial.into(),
                        color_index: ci as i32,
                    }
                }).collect();
                let model = Rc::new(slint::VecModel::from(items));
                w.set_public_spaces(slint::ModelRc::from(model));
            }
            SignalMessage::SpacePublicChanged { is_public } => {
                log::info!("Space public status changed: {is_public}");
                w.set_is_space_public(*is_public);
            }
            SignalMessage::MessageReacted { channel_id, message_id, emoji, reactor_name, count } => {
                log::info!("Reaction {emoji} on {message_id} in {channel_id} by {reactor_name} (count: {count})");
            }
            // Auto-moderation word filter
            SignalMessage::AutomodWordList { words } => {
                log::info!("Received automod word list: {} entries", words.len());
                ui_shell::set_automod_words(w, words);
            }
            SignalMessage::AutomodWordAdded { word, action } => {
                log::info!("Automod word added: {word} (action: {action})");
                let model: slint::ModelRc<ui_shell::AutomodWordData> = w.get_automod_words();
                let new_item = ui_shell::AutomodWordData {
                    word: word.as_str().into(),
                    action: action.as_str().into(),
                };
                if let Some(vec_model) = model
                    .as_any()
                    .downcast_ref::<slint::VecModel<ui_shell::AutomodWordData>>()
                {
                    vec_model.push(new_item);
                } else {
                    let vm = slint::VecModel::from(vec![new_item]);
                    w.set_automod_words(Rc::new(vm).into());
                }
            }
            SignalMessage::AutomodWordRemoved { word } => {
                log::info!("Automod word removed: {word}");
                let model: slint::ModelRc<ui_shell::AutomodWordData> = w.get_automod_words();
                if let Some(vec_model) = model
                    .as_any()
                    .downcast_ref::<slint::VecModel<ui_shell::AutomodWordData>>()
                {
                    for i in 0..vec_model.row_count() {
                        if vec_model.row_data(i).map_or(false, |e| e.word == word.as_str()) {
                            vec_model.remove(i);
                            break;
                        }
                    }
                }
            }
            other => {
                log::trace!("Unhandled signal (client-to-server variant): {:?}",
                    std::mem::discriminant(other));
            }
        }
    }
}

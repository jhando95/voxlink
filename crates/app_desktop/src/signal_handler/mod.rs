mod channel;
mod chat;
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
                // Desktop notification for peer join
                if w.get_notifications_enabled() {
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
                    continue;
                }
                log::error!("Server error: {message}");
                w.set_status_text(format!("Error: {message}").into());
            }
            SignalMessage::SpaceCreated { space, channels } => {
                space::handle_space_created(w, state, space, channels);
            }
            SignalMessage::SpaceJoined {
                space,
                channels,
                members,
            } => {
                space::handle_space_joined(w, state, space, channels, members);
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
                chat::handle_typing_state(w, state, channel_id, user_name, *is_typing);
            }
            SignalMessage::DirectTypingState {
                user_id,
                user_name,
                is_typing,
            } => {
                chat::handle_direct_typing_state(w, state, user_id, user_name, *is_typing);
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
                ui_shell::set_channels(w, &[]);
                ui_shell::set_members(w, &[]);
                ui_shell::set_space_audit_log(w, &[]);
                crate::friends::sync_ui(w, state);
                ctx.screen_share.stop_capture();
                if w.get_notifications_enabled() {
                    crate::helpers::send_notification("Voxlink", reason);
                }
            }
            SignalMessage::MemberMuted { member_id, muted } => {
                room::handle_peer_mute_changed(w, state, member_id, *muted);
            }
            SignalMessage::ServerShutdown => {
                log::info!("Server is shutting down");
                w.set_status_text("Server restarting, reconnecting...".into());
                w.set_room_status("Server restarting...".into());
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
            _ => {}
        }
    }
}

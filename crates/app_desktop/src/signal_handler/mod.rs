mod channel;
mod chat;
pub mod connection;
mod member;
mod room;
mod space;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use shared_types::SignalMessage;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

/// Shared context for audio setup — avoids passing 5+ params through every handler.
pub struct AudioContext {
    pub audio_started: Rc<RefCell<bool>>,
    pub audio: Arc<TokioMutex<audio_core::AudioEngine>>,
    pub media: Arc<TokioMutex<media_transport::MediaSession>>,
    pub audio_active_flag: Arc<AtomicBool>,
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
            SignalMessage::RoomJoined { room_code, participants } => {
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
            SignalMessage::PeerDeafenChanged { peer_id, is_deafened } => {
                room::handle_peer_deafen_changed(w, state, peer_id, *is_deafened);
            }
            SignalMessage::Error { message } => {
                log::error!("Server error: {message}");
                w.set_status_text(format!("Error: {message}").into());
            }
            SignalMessage::SpaceCreated { space, channels } => {
                space::handle_space_created(w, state, space, channels);
            }
            SignalMessage::SpaceJoined { space, channels, members } => {
                space::handle_space_joined(w, state, space, channels, members);
            }
            SignalMessage::ChannelCreated { channel } => {
                channel::handle_channel_created(w, state, channel);
            }
            SignalMessage::ChannelJoined { channel_id, channel_name, participants } => {
                channel::handle_channel_joined(w, state, channel_id, channel_name, participants, ctx);
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
            SignalMessage::MemberChannelChanged { member_id, channel_id, channel_name } => {
                member::handle_member_channel_changed(w, state, member_id, channel_id, channel_name);
            }
            SignalMessage::SpaceDeleted => {
                space::handle_space_deleted(w, state, ctx);
            }
            SignalMessage::TextChannelSelected { channel_id, channel_name, history } => {
                chat::handle_text_channel_selected(w, state, channel_id, channel_name, history);
            }
            SignalMessage::TextMessage { channel_id, message } => {
                chat::handle_text_message(w, state, channel_id, message);
            }
            // Auth (Milestone 4)
            SignalMessage::Authenticated { token, user_id } => {
                log::info!("Authenticated as {user_id}");
                if !token.is_empty() {
                    crate::helpers::save_auth_token_async(token.clone());
                }
            }
            // Chat improvements (Milestone 5)
            SignalMessage::TextMessageEdited { channel_id, message_id, new_content } => {
                chat::handle_text_message_edited(w, channel_id, message_id, new_content);
            }
            SignalMessage::TextMessageDeleted { channel_id, message_id } => {
                chat::handle_text_message_deleted(w, channel_id, message_id);
            }
            SignalMessage::MessageReaction { channel_id, message_id, emoji, user_name } => {
                chat::handle_message_reaction(w, channel_id, message_id, emoji, user_name);
            }
            // Moderation (Milestone 6)
            SignalMessage::Kicked { reason } => {
                log::warn!("Kicked: {reason}");
                // Clean up space/room state
                {
                    let mut s = state.borrow_mut();
                    s.room = Default::default();
                    s.space = None;
                    s.current_view = shared_types::AppView::Home;
                }
                w.set_current_view(0);
                w.set_current_space_id(slint::SharedString::default());
                w.set_current_space_name(slint::SharedString::default());
                w.set_current_space_invite(slint::SharedString::default());
                w.set_is_space_owner(false);
                w.set_in_space_channel(false);
                w.set_room_code(slint::SharedString::default());
                w.set_is_muted(false);
                w.set_is_deafened(false);
                w.set_status_text(reason.clone().into());
                if w.get_notifications_enabled() {
                    crate::helpers::send_notification("Voxlink", reason);
                }
            }
            SignalMessage::MemberMuted { member_id, muted } => {
                room::handle_peer_mute_changed(w, state, member_id, *muted);
            }
            _ => {}
        }
    }
}

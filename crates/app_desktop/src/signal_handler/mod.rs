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
                room::handle_peer_joined(w, state, peer);
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
            _ => {}
        }
    }
}

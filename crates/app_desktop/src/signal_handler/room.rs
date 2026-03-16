use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use shared_types::{AppView, Participant};
use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

use super::AudioContext;

pub fn handle_room_entered(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    room_code: &str,
    existing_participants: &[shared_types::ParticipantInfo],
    ctx: &AudioContext,
) {
    let is_create = existing_participants.is_empty();
    if is_create {
        log::info!("Room created: {room_code}");
    } else {
        log::info!("Joined room: {room_code}");
    }

    crate::helpers::save_room_code_async(room_code.to_string());

    let mut s = state.borrow_mut();
    s.room.room_code = room_code.to_string();

    s.room.participants = existing_participants
        .iter()
        .map(|p| Participant {
            id: p.id.clone(),
            name: p.name.clone(),
            is_muted: p.is_muted,
            is_deafened: false,
            is_speaking: false,
            volume: 1.0,
        })
        .collect();
    s.room.participants.push(Participant {
        id: "self".into(),
        name: w.get_user_name().to_string(),
        is_muted: false,
        is_deafened: false,
        is_speaking: false,
        volume: 1.0,
    });

    s.room.connection = shared_types::ConnectionState::Connected;
    s.room.active_screen_share_peer_id = None;
    s.room.active_screen_share_peer_name = None;
    s.room.is_sharing_screen = false;
    s.current_view = AppView::Room;

    w.set_room_code(room_code.into());
    w.set_reconnect_attempts(0);
    w.set_dropped_frames_baseline(w.get_dropped_frames_total());
    w.set_dropped_frames(0);
    w.set_current_view(ui_shell::view_to_index(AppView::Room));
    let count = s.room.participants.len();
    w.set_window_title(format!("Voxlink — {room_code} ({count})").into());
    w.set_room_password(slint::SharedString::default());
    w.set_room_status(slint::SharedString::default());
    w.set_has_screen_share(false);
    w.set_is_sharing_screen(false);
    w.set_screen_share_owner_name(slint::SharedString::default());
    w.set_screen_share_owner_id(slint::SharedString::default());
    w.set_screen_share_image(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
        slint::Rgba8Pixel,
    >::new(1, 1)));
    if let Err(message) = ctx.screen_share.refresh_sources() {
        log::warn!("Failed to refresh screen share sources: {message}");
    }
    ctx.screen_share.apply_to_window(w);
    if !is_create {
        w.set_join_code(slint::SharedString::default());
    }
    ui_shell::set_participants(w, &s.room.participants);

    crate::helpers::start_audio_if_needed(
        &ctx.audio_started,
        &ctx.audio,
        &ctx.media,
        &ctx.audio_active_flag,
        &ctx.rt_handle,
        ctx.saved_input_device.borrow().clone(),
        ctx.saved_output_device.borrow().clone(),
        Some(w.as_weak()),
    );
}

pub fn handle_peer_joined(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    peer: &shared_types::ParticipantInfo,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
) {
    log::info!("Peer joined: {} ({})", peer.name, peer.id);
    let mut s = state.borrow_mut();
    s.room.participants.push(Participant {
        id: peer.id.clone(),
        name: peer.name.clone(),
        is_muted: peer.is_muted,
        is_deafened: false,
        is_speaking: false,
        volume: 1.0,
    });
    ui_shell::set_participants(w, &s.room.participants);
    let count = s.room.participants.len();
    let code = &s.room.room_code;
    w.set_window_title(format!("Voxlink — {code} ({count})").into());

    // Play join notification sound
    if w.get_feedback_sound() {
        if let Ok(aud) = audio.try_lock() {
            aud.play_notification(true);
        }
    }
}

pub fn handle_peer_left(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    peer_id: &str,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
) {
    log::info!("Peer left: {peer_id}");
    let mut s = state.borrow_mut();
    s.room.participants.retain(|p| p.id != peer_id);
    ui_shell::set_participants(w, &s.room.participants);
    let count = s.room.participants.len();
    let code = &s.room.room_code;
    w.set_window_title(format!("Voxlink — {code} ({count})").into());
    if let Ok(aud) = audio.try_lock() {
        // Play leave notification sound
        if w.get_feedback_sound() {
            aud.play_notification(false);
        }
        aud.remove_peer(peer_id);
    }
}

pub fn handle_peer_mute_changed(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    peer_id: &str,
    is_muted: bool,
) {
    let mut s = state.borrow_mut();
    if let Some(p) = s.room.participants.iter_mut().find(|p| p.id == peer_id) {
        p.is_muted = is_muted;
    }
    ui_shell::set_participants(w, &s.room.participants);
}

pub fn handle_peer_deafen_changed(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    peer_id: &str,
    is_deafened: bool,
) {
    let mut s = state.borrow_mut();
    if let Some(p) = s.room.participants.iter_mut().find(|p| p.id == peer_id) {
        p.is_deafened = is_deafened;
    }
    ui_shell::set_participants(w, &s.room.participants);
}

pub fn handle_screen_share_started(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    sharer_id: &str,
    sharer_name: &str,
    is_self: bool,
    ctx: &AudioContext,
) {
    {
        let mut s = state.borrow_mut();
        s.room.active_screen_share_peer_id = Some(sharer_id.to_string());
        s.room.active_screen_share_peer_name = Some(sharer_name.to_string());
        s.room.is_sharing_screen = is_self;
    }

    w.set_has_screen_share(true);
    w.set_is_sharing_screen(is_self);
    w.set_screen_share_owner_id(sharer_id.into());
    w.set_screen_share_owner_name(sharer_name.into());
    w.set_room_status(
        (if is_self {
            "Screen share starting..."
        } else {
            "Screen share live"
        })
        .into(),
    );

    if is_self {
        if let Err(message) = ctx
            .screen_share
            .start_capture(ctx.network.clone(), ctx.rt_handle.clone())
        {
            log::error!("Failed to start local screen share capture: {message}");
            ctx.screen_share.stop_capture();
            let network = ctx.network.clone();
            ctx.rt_handle.spawn(async move {
                let net = network.lock().await;
                let _ = net
                    .send_signal(&shared_types::SignalMessage::StopScreenShare)
                    .await;
            });
            {
                let mut s = state.borrow_mut();
                s.room.active_screen_share_peer_id = None;
                s.room.active_screen_share_peer_name = None;
                s.room.is_sharing_screen = false;
            }
            w.set_has_screen_share(false);
            w.set_is_sharing_screen(false);
            w.set_screen_share_owner_id(slint::SharedString::default());
            w.set_screen_share_owner_name(slint::SharedString::default());
            w.set_screen_share_image(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
                slint::Rgba8Pixel,
            >::new(1, 1)));
            w.set_room_status("Screen share could not start".into());
        }
    }
    ctx.screen_share.apply_to_window(w);
}

pub fn handle_screen_share_stopped(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    sharer_id: &str,
    ctx: &AudioContext,
) {
    let was_self = {
        let mut s = state.borrow_mut();
        let was_self = s.room.is_sharing_screen
            && s.room.active_screen_share_peer_id.as_deref() == Some(sharer_id);
        s.room.active_screen_share_peer_id = None;
        s.room.active_screen_share_peer_name = None;
        s.room.is_sharing_screen = false;
        was_self
    };

    if was_self {
        ctx.screen_share.stop_capture();
    }

    w.set_has_screen_share(false);
    w.set_is_sharing_screen(false);
    w.set_screen_share_owner_id(slint::SharedString::default());
    w.set_screen_share_owner_name(slint::SharedString::default());
    w.set_screen_share_image(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
        slint::Rgba8Pixel,
    >::new(1, 1)));
    if w.get_room_status().as_str().contains("Screen share") {
        w.set_room_status(slint::SharedString::default());
    }
    ctx.screen_share.apply_to_window(w);
}

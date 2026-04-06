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

    let cfg = config_store::load_config();
    let saved_volumes = &cfg.peer_volumes;
    let saved_eq = &cfg.peer_eq_settings;
    let saved_pan = &cfg.peer_pan;
    s.room.participants = existing_participants
        .iter()
        .map(|p| {
            let eq = saved_eq.get(&p.name).copied().unwrap_or([0, 0, 0]);
            let pan_raw = saved_pan.get(&p.name).copied().unwrap_or(0);
            Participant {
                id: p.id.clone(),
                name: p.name.clone(),
                is_muted: p.is_muted,
                is_deafened: false,
                is_speaking: false,
                volume: saved_volumes.get(&p.name).copied().unwrap_or(1.0),
                audio_level: 0,
                eq_bass: eq[0] as f32 / 1200.0 + 0.5,
                eq_mid: eq[1] as f32 / 1200.0 + 0.5,
                eq_treble: eq[2] as f32 / 1200.0 + 0.5,
                pan: pan_raw as f32 / 200.0 + 0.5,
                is_priority_speaker: p.is_priority_speaker,
            }
        })
        .collect();
    // Restore mute/deafen state from the UI (survives reconnects)
    let self_muted = w.get_is_muted();
    let self_deafened = w.get_is_deafened();
    s.room.participants.push(Participant {
        id: "self".into(),
        name: w.get_user_name().to_string(),
        is_muted: self_muted,
        is_deafened: self_deafened,
        is_speaking: false,
        volume: 1.0,
        audio_level: 0,
        eq_bass: 0.5,
        eq_mid: 0.5,
        eq_treble: 0.5,
        pan: 0.5,
        is_priority_speaker: false,
    });
    s.room.is_muted = self_muted;
    s.room.is_deafened = self_deafened;

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
    let cfg = config_store::load_config();
    let saved_vol = cfg.peer_volumes
        .get(&peer.name).copied().unwrap_or(1.0);
    let saved_eq = cfg.peer_eq_settings
        .get(&peer.name).copied().unwrap_or([0, 0, 0]);
    let saved_pan = cfg.peer_pan
        .get(&peer.name).copied().unwrap_or(0);
    let mut s = state.borrow_mut();
    s.room.participants.push(Participant {
        id: peer.id.clone(),
        name: peer.name.clone(),
        is_muted: peer.is_muted,
        is_deafened: false,
        is_speaking: false,
        volume: saved_vol,
        audio_level: 0,
        eq_bass: saved_eq[0] as f32 / 1200.0 + 0.5,
        eq_mid: saved_eq[1] as f32 / 1200.0 + 0.5,
        eq_treble: saved_eq[2] as f32 / 1200.0 + 0.5,
        pan: saved_pan as f32 / 200.0 + 0.5,
        is_priority_speaker: peer.is_priority_speaker,
    });
    ui_shell::set_participants(w, &s.room.participants);
    let count = s.room.participants.len();
    let code = &s.room.room_code;
    w.set_window_title(format!("Voxlink — {code} ({count})").into());

    // Apply saved volume to audio engine
    if (saved_vol - 1.0).abs() > 0.01 {
        if let Ok(aud) = audio.try_lock() {
            aud.set_peer_volume(&peer.id, saved_vol);
        }
    }
    // Apply saved EQ to audio engine
    if saved_eq != [0, 0, 0] {
        if let Ok(aud) = audio.try_lock() {
            aud.set_peer_eq(&peer.id, saved_eq[0], saved_eq[1], saved_eq[2]);
        }
    }
    // Apply saved pan to audio engine
    if saved_pan != 0 {
        if let Ok(aud) = audio.try_lock() {
            aud.set_peer_pan(&peer.id, saved_pan);
        }
    }

    // Play join notification sound; suppress in DND mode
    if w.get_join_leave_sounds() && w.get_status_preset() != 2 {
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
        // Play leave notification sound; suppress in DND mode
        if w.get_join_leave_sounds() && w.get_status_preset() != 2 {
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

pub fn handle_priority_speaker_changed(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    peer_id: &str,
    enabled: bool,
) {
    log::info!(
        "Priority speaker {}: {peer_id}",
        if enabled { "enabled" } else { "disabled" }
    );
    let mut s = state.borrow_mut();
    let peer_name = if let Some(p) = s
        .room
        .participants
        .iter_mut()
        .find(|p| p.id == peer_id)
    {
        p.is_priority_speaker = enabled;
        Some(p.name.clone())
    } else {
        None
    };
    ui_shell::set_participants(w, &s.room.participants);
    drop(s);
    if let Some(name) = peer_name {
        if enabled {
            w.set_status_text(format!("{name} is now priority speaker").into());
        }
    }
}

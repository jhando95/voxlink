use std::cell::RefCell;
use std::rc::Rc;

use shared_types::{AppView, Participant};
use slint::ComponentHandle;
use ui_shell::MainWindow;

use super::AudioContext;

pub fn handle_channel_created(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    channel: &shared_types::ChannelInfo,
) {
    log::info!("Channel created: {} ({})", channel.name, channel.id);
    let mut s = state.borrow_mut();
    if let Some(ref mut space) = s.space {
        space.channels.push(channel.clone());
    }
    drop(s);
    crate::friends::sync_ui(w, state);
    w.set_new_channel_name(slint::SharedString::default());
    w.set_new_channel_is_voice(true);
}

pub fn handle_channel_joined(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    channel_id: &str,
    channel_name: &str,
    participants: &[shared_types::ParticipantInfo],
    ctx: &AudioContext,
) {
    log::info!("Joined channel: {channel_name} ({channel_id})");

    let mut s = state.borrow_mut();

    s.room.room_code = channel_id.to_string();
    s.room.participants = participants
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
    s.current_view = AppView::Room;

    if let Some(ref mut space) = s.space {
        space.active_channel_id = Some(channel_id.to_string());
    }

    w.set_room_code(channel_name.into());
    w.set_in_space_channel(true);
    w.set_reconnect_attempts(0);
    w.set_dropped_frames_baseline(w.get_dropped_frames_total());
    w.set_dropped_frames(0);
    w.set_current_view(ui_shell::view_to_index(AppView::Room));
    let count = s.room.participants.len();
    w.set_window_title(format!("Voxlink \u{2014} {channel_name} ({count})").into());
    w.set_room_status(slint::SharedString::default());
    ui_shell::set_participants(w, &s.room.participants);
    drop(s);
    crate::friends::sync_ui(w, state);

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

pub fn handle_channel_left(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    ctx: &AudioContext,
) {
    {
        let s = state.borrow();
        if s.current_view != AppView::Room {
            log::debug!(
                "Ignoring ChannelLeft — already left (view={:?})",
                s.current_view
            );
            return;
        }
    }

    log::info!("Left channel");
    {
        let mut s = state.borrow_mut();
        s.room = Default::default();
        if let Some(ref mut space) = s.space {
            space.active_channel_id = None;
        }
        s.current_view = AppView::Space;
    }
    crate::friends::sync_ui(w, state);
    *ctx.audio_started.borrow_mut() = false;

    w.set_current_view(ui_shell::view_to_index(AppView::Space));
    w.set_room_code(slint::SharedString::default());
    w.set_is_muted(false);
    w.set_is_deafened(false);
    w.set_in_space_channel(false);
    w.set_mic_level(0.0);
    w.set_reconnect_attempts(0);
    w.set_dropped_frames_baseline(w.get_dropped_frames_total());
    w.set_dropped_frames(0);
    w.set_window_title("Voxlink".into());

    let audio = ctx.audio.clone();
    let flag = ctx.audio_active_flag.clone();
    ctx.rt_handle.spawn(async move {
        let mut aud = audio.lock().await;
        aud.stop_capture();
        aud.stop_playback();
        flag.store(false, std::sync::atomic::Ordering::Relaxed);
    });
}

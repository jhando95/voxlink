use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use shared_types::{AppView, SignalMessage};
use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

pub fn setup_create_channel(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_create_channel(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let channel_name = w.get_new_channel_name().to_string().trim().to_string();
        if channel_name.is_empty() {
            w.set_status_text("Enter a channel name".into());
            return;
        }
        let is_voice = w.get_new_channel_is_voice();
        let channel_type = if is_voice {
            shared_types::ChannelType::Voice
        } else {
            shared_types::ChannelType::Text
        };
        let voice_quality = w.get_new_channel_voice_quality() as u8;
        let network = network.clone();
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            if let Err(e) = net
                .send_signal(&SignalMessage::CreateChannel {
                    channel_name,
                    channel_type,
                    voice_quality,
                })
                .await
            {
                log::error!("Failed to create channel: {e}");
                if let Some(w) = window_weak.upgrade() {
                    w.set_status_text("Failed to create channel".into());
                }
            }
        });
    });
}

pub fn setup_join_channel(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_join_channel(move |channel_id| {
        let channel_id_str = channel_id.to_string();
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        w.set_status_text("Joining channel...".into());
        let network = network.clone();
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            if let Err(e) = net
                .send_signal(&SignalMessage::JoinChannel {
                    channel_id: channel_id_str,
                })
                .await
            {
                log::error!("Failed to join channel: {e}");
                if let Some(w) = window_weak.upgrade() {
                    w.set_status_text("Failed to join channel".into());
                }
            }
        });
    });
}

pub fn setup_delete_channel(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_delete_channel(move |channel_id| {
        let channel_id = channel_id.trim().to_string();
        if channel_id.is_empty() {
            return;
        }
        if let Some(w) = window_weak.upgrade() {
            w.set_status_text("Deleting channel...".into());
        }
        let network = network.clone();
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            if let Err(e) = net
                .send_signal(&SignalMessage::DeleteChannel { channel_id })
                .await
            {
                log::error!("Failed to delete channel: {e}");
                if let Some(w) = window_weak.upgrade() {
                    w.set_status_text("Failed to delete channel".into());
                }
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
pub fn setup_leave_channel(
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
    window.on_leave_channel(move || {
        log::info!("Leaving channel");
        screen_share.stop_capture();
        voice.borrow_mut().reset();
        {
            let mut s = state.borrow_mut();
            s.room = Default::default();
            if let Some(ref mut space) = s.space {
                space.active_channel_id = None;
            }
            // Only navigate to Space if currently viewing the room;
            // otherwise stay on the current view (e.g. Settings, System)
            if s.current_view == AppView::Room {
                s.current_view = AppView::Space;
            }
        }
        *audio_started.borrow_mut() = false;
        speaking_ticks.borrow_mut().clear();

        let Some(w) = window_weak.upgrade() else {
            return;
        };

        // Restore channel/member list before switching view
        crate::friends::sync_ui(&w, &state);

        if w.get_current_view() == ui_shell::view_to_index(AppView::Room) {
            w.set_current_view(ui_shell::view_to_index(AppView::Space));
        }
        ui_shell::set_participants(&w, &[]);
        w.set_room_code(slint::SharedString::default());
        w.set_active_channel_id(slint::SharedString::default());
        w.set_is_muted(false);
        w.set_is_deafened(false);
        w.set_in_space_channel(false);
        w.set_status_text("Connected".into());
        w.set_window_title("Voxlink".into());
        w.set_room_status(slint::SharedString::default());
        w.set_mic_level(0.0);
        w.set_reconnect_attempts(0);
        w.set_dropped_frames_baseline(w.get_dropped_frames_total());
        w.set_dropped_frames(0);
        w.set_has_screen_share(false);
        w.set_is_sharing_screen(false);
        w.set_screen_share_owner_name(slint::SharedString::default());
        w.set_screen_share_owner_id(slint::SharedString::default());
        w.set_screen_share_image(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
            slint::Rgba8Pixel,
        >::new(1, 1)));

        let network = network.clone();
        let audio = audio.clone();
        let flag = audio_active_flag.clone();
        rt_handle.spawn(async move {
            // Send leave signal, then release network lock BEFORE stopping audio.
            {
                let net = network.lock().await;
                let _ = net.send_signal(&SignalMessage::LeaveChannel).await;
            }
            let mut aud = audio.lock().await;
            aud.stop_capture();
            aud.stop_playback();
            flag.store(false, Ordering::Relaxed);
        });
    });
}

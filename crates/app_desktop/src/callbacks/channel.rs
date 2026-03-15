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
        let network = network.clone();
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            if let Err(e) = net
                .send_signal(&SignalMessage::CreateChannel {
                    channel_name,
                    channel_type,
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

#[allow(clippy::too_many_arguments)]
pub fn setup_leave_channel(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    voice: &Rc<RefCell<voice_engine::VoiceSession>>,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    audio_started: &Rc<RefCell<bool>>,
    audio_active_flag: &Arc<AtomicBool>,
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
    let voice = voice.clone();
    let speaking_ticks = speaking_ticks.clone();
    window.on_leave_channel(move || {
        log::info!("Leaving channel");
        voice.borrow_mut().reset();
        {
            let mut s = state.borrow_mut();
            s.room = Default::default();
            if let Some(ref mut space) = s.space {
                space.active_channel_id = None;
            }
            s.current_view = AppView::Space;
        }
        *audio_started.borrow_mut() = false;
        speaking_ticks.borrow_mut().clear();

        let Some(w) = window_weak.upgrade() else {
            return;
        };

        // Restore channel/member list before switching view
        {
            let s = state.borrow();
            if let Some(ref space) = s.space {
                ui_shell::render_space(&w, space, &w.get_space_search_query().to_string());
            }
        }

        w.set_current_view(ui_shell::view_to_index(AppView::Space));
        w.set_room_code(slint::SharedString::default());
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

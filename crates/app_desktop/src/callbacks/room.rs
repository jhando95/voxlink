use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use shared_types::{AppView, SignalMessage};
use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

use crate::helpers;

fn refresh_share_sources_ui(
    window: &MainWindow,
    screen_share: &crate::screen_share::ScreenShareController,
) -> Result<(), String> {
    screen_share.refresh_sources()?;
    screen_share.apply_to_window(window);
    Ok(())
}

pub fn setup_create_room(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_create_room(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let user_name = w.get_user_name().to_string().trim().to_string();
        if user_name.is_empty() {
            w.set_status_text("Enter your name first".into());
            return;
        }
        let pw = w.get_room_password().to_string();
        let password = if pw.is_empty() { None } else { Some(pw) };
        let network = network.clone();
        w.set_status_text("Starting call...".into());
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            if let Err(e) = net
                .send_signal(&SignalMessage::CreateRoom {
                    user_name,
                    password,
                })
                .await
            {
                log::error!("Failed to create room: {e}");
                if let Some(w) = window_weak.upgrade() {
                    w.set_status_text("Failed: Not connected".into());
                }
            }
        });
    });
}

pub fn setup_join_room(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_join_room(move |code| {
        let code = code.to_string().trim().to_uppercase();
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        if code.is_empty() {
            w.set_status_text("Enter a call code".into());
            return;
        }
        let user_name = w.get_user_name().to_string().trim().to_string();
        if user_name.is_empty() {
            w.set_status_text("Enter your name first".into());
            return;
        }
        let pw = w.get_room_password().to_string();
        let password = if pw.is_empty() { None } else { Some(pw) };
        let network = network.clone();
        w.set_status_text("Joining call...".into());
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            if let Err(e) = net
                .send_signal(&SignalMessage::JoinRoom {
                    room_code: code,
                    user_name,
                    password,
                })
                .await
            {
                log::error!("Failed to join room: {e}");
                if let Some(w) = window_weak.upgrade() {
                    w.set_status_text("Failed: Not connected".into());
                }
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
pub fn setup_leave_room(
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
    window.on_leave_room(move || {
        log::info!("Leaving room");
        screen_share.stop_capture();
        voice.borrow_mut().reset();
        {
            let mut s = state.borrow_mut();
            s.room = Default::default();
            s.current_view = AppView::Home;
        }
        *audio_started.borrow_mut() = false;
        speaking_ticks.borrow_mut().clear();

        let Some(w) = window_weak.upgrade() else {
            return;
        };
        w.set_current_view(ui_shell::view_to_index(AppView::Home));
        w.set_room_code(slint::SharedString::default());
        w.set_is_muted(false);
        w.set_is_deafened(false);
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
        w.set_screen_share_sources(
            std::rc::Rc::new(slint::VecModel::<ui_shell::ShareSourceData>::from(
                Vec::new(),
            ))
            .into(),
        );
        w.set_selected_screen_share_source(-1);
        w.set_selected_screen_share_profile(0);
        w.set_screen_share_source_label(slint::SharedString::default());
        w.set_screen_share_source_detail(slint::SharedString::default());
        w.set_screen_share_quality_label(slint::SharedString::default());
        w.set_screen_share_quality_detail(slint::SharedString::default());

        helpers::clear_room_code_async();

        let network = network.clone();
        let audio = audio.clone();
        let flag = audio_active_flag.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net.send_signal(&SignalMessage::LeaveRoom).await;
            let mut aud = audio.lock().await;
            aud.stop_capture();
            aud.stop_playback();
            flag.store(false, Ordering::Relaxed);
        });
    });
}

pub fn setup_toggle_screen_share(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    screen_share: &Arc<crate::screen_share::ScreenShareController>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let screen_share = screen_share.clone();
    let rt_handle = rt_handle.clone();
    window.on_toggle_screen_share(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let is_sharing = w.get_is_sharing_screen();
        if w.get_has_screen_share() && !is_sharing {
            w.set_room_status("Another share is already live".into());
            return;
        }
        if !is_sharing {
            if let Err(message) = refresh_share_sources_ui(&w, &screen_share) {
                w.set_room_status(message.into());
                return;
            }
        }

        let network = network.clone();
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let result = if is_sharing {
                net.send_signal(&SignalMessage::StopScreenShare).await
            } else {
                net.send_signal(&SignalMessage::StartScreenShare).await
            };

            if let Err(e) = result {
                log::error!("Failed to toggle screen share: {e}");
                if let Some(w) = window_weak.upgrade() {
                    w.set_room_status("Screen share request failed".into());
                }
            }
        });
    });
}

pub fn setup_refresh_screen_share_sources(
    window: &MainWindow,
    screen_share: &Arc<crate::screen_share::ScreenShareController>,
) {
    let window_weak = window.as_weak();
    let screen_share = screen_share.clone();
    window.on_refresh_screen_share_sources(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        match refresh_share_sources_ui(&w, &screen_share) {
            Ok(()) => {
                if w.get_room_status().as_str().contains("source") {
                    w.set_room_status(slint::SharedString::default());
                }
            }
            Err(message) => w.set_room_status(message.into()),
        }
    });
}

pub fn setup_select_screen_share_source(
    window: &MainWindow,
    screen_share: &Arc<crate::screen_share::ScreenShareController>,
) {
    let window_weak = window.as_weak();
    let screen_share = screen_share.clone();
    window.on_select_screen_share_source(move |index| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        if let Err(message) = screen_share.select_source_index(index as usize) {
            w.set_room_status(message.into());
            return;
        }
        screen_share.apply_to_window(&w);
        if w.get_is_sharing_screen() {
            w.set_room_status("Source updated for your next share".into());
        }
    });
}

pub fn setup_select_screen_share_profile(
    window: &MainWindow,
    screen_share: &Arc<crate::screen_share::ScreenShareController>,
) {
    let window_weak = window.as_weak();
    let screen_share = screen_share.clone();
    window.on_select_screen_share_profile(move |index| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        if let Err(message) = screen_share.select_profile_index(index as usize) {
            w.set_room_status(message.into());
            return;
        }
        screen_share.apply_to_window(&w);
        if w.get_is_sharing_screen() {
            w.set_room_status("Profile updated for your next share".into());
        }
    });
}

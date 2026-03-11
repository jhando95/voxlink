use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use shared_types::{AppView, SignalMessage};
use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

use crate::helpers;

pub fn setup_create_room(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_create_room(move || {
        let Some(w) = window_weak.upgrade() else { return; };
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
        let Some(w) = window_weak.upgrade() else { return; };
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
    window.on_leave_room(move || {
        log::info!("Leaving room");
        voice.borrow_mut().reset();
        {
            let mut s = state.borrow_mut();
            s.room = Default::default();
            s.current_view = AppView::Home;
        }
        *audio_started.borrow_mut() = false;
        speaking_ticks.borrow_mut().clear();

        let Some(w) = window_weak.upgrade() else { return; };
        w.set_current_view(ui_shell::view_to_index(AppView::Home));
        w.set_room_code(slint::SharedString::default());
        w.set_is_muted(false);
        w.set_is_deafened(false);
        w.set_status_text("Connected".into());
        w.set_window_title("Voxlink".into());
        w.set_room_status(slint::SharedString::default());
        w.set_mic_level(0.0);

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

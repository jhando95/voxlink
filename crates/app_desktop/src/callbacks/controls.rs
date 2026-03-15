use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use shared_types::{MicMode, SignalMessage};
use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

pub fn setup_toggle_mute(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    voice: &Rc<RefCell<voice_engine::VoiceSession>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let voice = voice.clone();
    let state = state.clone();
    let audio = audio.clone();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_toggle_mute(move || {
        voice.borrow_mut().toggle_mute();
        let muted = voice.borrow().is_muted;
        {
            let mut s = state.borrow_mut();
            s.room.is_muted = muted;
            if let Some(me) = s.room.participants.iter_mut().find(|p| p.id == "self") {
                me.is_muted = muted;
            }
        }

        let Some(w) = window_weak.upgrade() else {
            return;
        };
        w.set_is_muted(muted);
        ui_shell::set_participants(&w, &state.borrow().room.participants);

        let audio = audio.clone();
        let network = network.clone();
        let feedback = w.get_feedback_sound();
        rt_handle.spawn(async move {
            let aud = audio.lock().await;
            aud.set_muted(muted);
            if feedback {
                aud.play_feedback_mute(muted);
            }
            drop(aud);
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::MuteChanged { is_muted: muted })
                .await;
        });
    });
}

pub fn setup_toggle_deafen(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    voice: &Rc<RefCell<voice_engine::VoiceSession>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let voice = voice.clone();
    let state = state.clone();
    let audio = audio.clone();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_toggle_deafen(move || {
        voice.borrow_mut().toggle_deafen();
        let v = voice.borrow();
        {
            let mut s = state.borrow_mut();
            s.room.is_deafened = v.is_deafened;
            s.room.is_muted = v.is_muted;
            if let Some(me) = s.room.participants.iter_mut().find(|p| p.id == "self") {
                me.is_muted = v.is_muted;
            }
        }

        let Some(w) = window_weak.upgrade() else {
            return;
        };
        w.set_is_deafened(v.is_deafened);
        w.set_is_muted(v.is_muted);
        ui_shell::set_participants(&w, &state.borrow().room.participants);

        let muted = v.is_muted;
        let deafened = v.is_deafened;
        let audio = audio.clone();
        let network = network.clone();
        let feedback = w.get_feedback_sound();
        rt_handle.spawn(async move {
            let aud = audio.lock().await;
            aud.set_muted(muted);
            aud.set_deafened(deafened);
            if feedback {
                aud.play_feedback_deafen(deafened);
            }
            drop(aud);
            let net = network.lock().await;
            let _ = net
                .send_signal(&SignalMessage::MuteChanged { is_muted: muted })
                .await;
            let _ = net
                .send_signal(&SignalMessage::DeafenChanged {
                    is_deafened: deafened,
                })
                .await;
        });
    });
}

pub fn setup_toggle_mic_mode(
    window: &MainWindow,
    voice: &Rc<RefCell<voice_engine::VoiceSession>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let voice = voice.clone();
    let audio = audio.clone();
    let rt_handle = rt_handle.clone();
    window.on_toggle_mic_mode(move || {
        let mut v = voice.borrow_mut();
        let new_mode = match v.mic_mode {
            MicMode::OpenMic => MicMode::PushToTalk,
            MicMode::PushToTalk => MicMode::OpenMic,
        };
        v.set_mic_mode(new_mode);
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        w.set_is_open_mic(new_mode == MicMode::OpenMic);

        let audio = audio.clone();
        let is_ptt = new_mode == MicMode::PushToTalk;
        rt_handle.spawn(async move {
            let aud = audio.lock().await;
            aud.set_vad_enabled(!is_ptt);
            if is_ptt {
                aud.set_muted(true);
            } else {
                aud.set_muted(false);
            }
        });
    });
}

pub fn setup_volume_changed(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let audio = audio.clone();
    let rt_handle = rt_handle.clone();
    let state = state.clone();
    window.on_volume_changed(move |peer_id, volume| {
        let peer_id_str = peer_id.to_string();
        {
            let mut s = state.borrow_mut();
            if let Some(p) = s.room.participants.iter_mut().find(|p| p.id == peer_id_str) {
                p.volume = volume;
            }
        }
        let audio = audio.clone();
        rt_handle.spawn(async move {
            let aud = audio.lock().await;
            aud.set_peer_volume(&peer_id_str, volume);
        });
    });
}

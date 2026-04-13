use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use shared_types::{MicMode, SignalMessage};
use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

use crate::helpers::CONFIG_LOCK;

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
        let peer_name = {
            let mut s = state.borrow_mut();
            let name = s
                .room
                .participants
                .iter()
                .find(|p| p.id == peer_id_str)
                .map(|p| p.name.clone());
            if let Some(p) = s.room.participants.iter_mut().find(|p| p.id == peer_id_str) {
                p.volume = volume;
            }
            name
        };
        // Persist per-peer volume by name (survives reconnects)
        if let Some(name) = peer_name {
            if (volume - 1.0).abs() > 0.01 {
                let _lock = CONFIG_LOCK.lock().ok();
                let mut cfg = config_store::load_config();
                cfg.peer_volumes.insert(name, volume);
                let _ = config_store::save_config(&cfg);
            }
        }
        let audio = audio.clone();
        rt_handle.spawn(async move {
            let aud = audio.lock().await;
            aud.set_peer_volume(&peer_id_str, volume);
        });
    });
}

/// Wire per-peer 3-band EQ slider changes.
/// UI sends bass/mid/treble as 0.0–1.0 (where 0.5 = flat).
/// We convert to millibels: (val - 0.5) * 12.0 * 100 = millibels (-600 to +600).
pub fn setup_eq_changed(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let audio = audio.clone();
    let rt_handle = rt_handle.clone();
    let state = state.clone();
    window.on_eq_changed(move |peer_id, bass, mid, treble| {
        let peer_id_str = peer_id.to_string();
        let bass_mb = ((bass - 0.5) * 1200.0) as i32;
        let mid_mb = ((mid - 0.5) * 1200.0) as i32;
        let treble_mb = ((treble - 0.5) * 1200.0) as i32;

        // Persist by peer name
        let peer_name = {
            let s = state.borrow();
            s.room
                .participants
                .iter()
                .find(|p| p.id == peer_id_str)
                .map(|p| p.name.clone())
        };
        if let Some(name) = peer_name {
            let _lock = CONFIG_LOCK.lock().ok();
            if bass_mb != 0 || mid_mb != 0 || treble_mb != 0 {
                let mut cfg = config_store::load_config();
                cfg.peer_eq_settings
                    .insert(name, [bass_mb, mid_mb, treble_mb]);
                let _ = config_store::save_config(&cfg);
            } else {
                // Remove entry when all flat
                let mut cfg = config_store::load_config();
                cfg.peer_eq_settings.remove(&name);
                let _ = config_store::save_config(&cfg);
            }
        }

        let audio = audio.clone();
        rt_handle.spawn(async move {
            let aud = audio.lock().await;
            aud.set_peer_eq(&peer_id_str, bass_mb, mid_mb, treble_mb);
        });
    });
}

/// Wire per-peer stereo pan slider changes.
/// UI sends pan as 0.0–1.0 (where 0.5 = center).
/// We convert to -100..+100.
pub fn setup_pan_changed(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let audio = audio.clone();
    let rt_handle = rt_handle.clone();
    let state = state.clone();
    window.on_pan_changed(move |peer_id, pan| {
        let peer_id_str = peer_id.to_string();
        let pan_val = ((pan - 0.5) * 200.0) as i32;

        // Persist by peer name
        let peer_name = {
            let s = state.borrow();
            s.room
                .participants
                .iter()
                .find(|p| p.id == peer_id_str)
                .map(|p| p.name.clone())
        };
        if let Some(name) = peer_name {
            let _lock = CONFIG_LOCK.lock().ok();
            if pan_val != 0 {
                let mut cfg = config_store::load_config();
                cfg.peer_pan.insert(name, pan_val);
                let _ = config_store::save_config(&cfg);
            } else {
                let mut cfg = config_store::load_config();
                cfg.peer_pan.remove(&name);
                let _ = config_store::save_config(&cfg);
            }
        }

        let audio = audio.clone();
        rt_handle.spawn(async move {
            let aud = audio.lock().await;
            aud.set_peer_pan(&peer_id_str, pan_val);
        });
    });
}

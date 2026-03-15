pub mod keys;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use device_query::{DeviceQuery, DeviceState, Keycode};
use shared_types::{MicMode, SignalMessage};
use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

use crate::signal_handler;
use keys::{combo_held, combo_to_config, combo_to_display, keycode_sort_order};

const TICK_MS: u64 = 25; // 40Hz — smooth for audio, low overhead

// ─── Event Loop ───

/// Start the 25ms event loop timer that drives the app.
#[allow(clippy::too_many_arguments)]
pub fn start(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    voice: &Rc<RefCell<voice_engine::VoiceSession>>,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    media: &Arc<TokioMutex<media_transport::MediaSession>>,
    perf: &Rc<RefCell<perf_metrics::PerfCollector>>,
    audio_started: &Rc<RefCell<bool>>,
    audio_active_flag: &Arc<AtomicBool>,
    network_flag: &Arc<AtomicBool>,
    speaking_ticks: &Rc<RefCell<HashMap<String, u64>>>,
    saved_input_device: Rc<RefCell<Option<String>>>,
    saved_output_device: Rc<RefCell<Option<String>>>,
    rt_handle: &tokio::runtime::Handle,
    ptt_key: Rc<RefCell<Vec<Keycode>>>,
    mute_key: Rc<RefCell<Vec<Keycode>>>,
    deafen_key: Rc<RefCell<Vec<Keycode>>>,
) {
    let window_weak = window.as_weak();
    let state = state.clone();
    let voice = voice.clone();
    let network = network.clone();
    let audio = audio.clone();
    let perf = perf.clone();
    let network_flag = network_flag.clone();
    let rt_handle = rt_handle.clone();
    let timer = slint::Timer::default();
    let tick_count = Rc::new(RefCell::new(0u64));

    let audio_ctx = signal_handler::AudioContext {
        audio_started: audio_started.clone(),
        audio: audio.clone(),
        media: media.clone(),
        audio_active_flag: audio_active_flag.clone(),
        rt_handle: rt_handle.clone(),
        saved_input_device,
        saved_output_device,
    };

    let was_connected = Rc::new(RefCell::new(false));
    let reconnect_cooldown = Rc::new(RefCell::new(0u64));
    let reconnect_interval = Rc::new(RefCell::new(3u64));
    let speaking_ticks = speaking_ticks.clone();
    let copied_at_tick: Rc<RefCell<Option<u64>>> = Rc::new(RefCell::new(None));
    let error_at_tick: Rc<RefCell<Option<u64>>> = Rc::new(RefCell::new(None));
    let notification_at_tick: Rc<RefCell<Option<u64>>> = Rc::new(RefCell::new(None));
    let signal_buf: Rc<RefCell<Vec<SignalMessage>>> = Rc::new(RefCell::new(Vec::with_capacity(8)));
    let signal_process: Rc<RefCell<Vec<SignalMessage>>> =
        Rc::new(RefCell::new(Vec::with_capacity(8)));
    let device_state = DeviceState::new();
    let ptt_was_held = Rc::new(RefCell::new(false));
    let prev_m_held = Rc::new(RefCell::new(false));
    let prev_d_held = Rc::new(RefCell::new(false));
    let prev_esc_held = Rc::new(RefCell::new(false));
    let mute_cooldown = Rc::new(RefCell::new(0u64));
    let deafen_cooldown = Rc::new(RefCell::new(0u64));
    let listen_state: Rc<RefCell<Option<ListenState>>> = Rc::new(RefCell::new(None));

    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(TICK_MS),
        move || {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            let tick = {
                let mut t = tick_count.borrow_mut();
                *t += 1;
                *t
            };

            let in_room = w.get_current_view() == 1;

            // --- Drain and process signal messages ---
            drain_signals(&network, &signal_buf);
            std::mem::swap(
                &mut *signal_buf.borrow_mut(),
                &mut *signal_process.borrow_mut(),
            );
            process_signals(
                &signal_process,
                &w,
                &state,
                &audio_ctx,
                &notification_at_tick,
                tick,
            );

            // --- Keyboard input ---
            let current_view = w.get_current_view();
            let keys = device_state.get_keys();

            // --- Keybind listening mode ---
            let listening = w.get_listening_keybind().to_string();
            if !listening.is_empty() {
                handle_keybind_listening(
                    &listening,
                    &keys,
                    &listen_state,
                    &w,
                    &ptt_key,
                    &mute_key,
                    &deafen_key,
                );
            } else {
                *listen_state.borrow_mut() = None;

                handle_escape(&keys, current_view, &w, &prev_esc_held);

                if !in_room {
                    *ptt_was_held.borrow_mut() = false;
                }

                if in_room {
                    handle_room_hotkeys(
                        &keys,
                        &voice,
                        &state,
                        &audio,
                        &network,
                        &rt_handle,
                        &w,
                        &ptt_was_held,
                        &prev_m_held,
                        &prev_d_held,
                        &mute_cooldown,
                        &deafen_cooldown,
                        &ptt_key,
                        &mute_key,
                        &deafen_key,
                        w.get_feedback_sound(),
                    );

                    signal_handler::connection::drain_audio_and_update_speaking(
                        &network,
                        &audio,
                        &state,
                        &speaking_ticks,
                        tick,
                        &w,
                    );

                    update_mic_level(tick, &audio, &state, &w);
                } else if current_view == 2 && w.get_mic_preview_active() {
                    update_preview_mic_level(tick, &audio, &w);
                }
            }

            // --- Timed auto-hides ---
            auto_hide_notification(&notification_at_tick, tick, &w);
            auto_clear_errors(&error_at_tick, tick, &w);
            auto_hide_copied(&copied_at_tick, tick, &w);

            // --- Slow updates every ~1s ---
            if tick.is_multiple_of(40) {
                let total_dropped_frames =
                    perf.borrow()
                        .dropped_frames
                        .load(std::sync::atomic::Ordering::Relaxed) as i32;
                w.set_dropped_frames_total(total_dropped_frames);
                w.set_dropped_frames(
                    (total_dropped_frames - w.get_dropped_frames_baseline()).max(0),
                );

                signal_handler::connection::check_connection(
                    &network,
                    &w,
                    &was_connected,
                    &reconnect_cooldown,
                    &reconnect_interval,
                    &network_flag,
                    &rt_handle,
                    &perf,
                );

                if in_room {
                    check_audio_recovery(&audio, &rt_handle, &w);
                }
            }

            // --- Ping every ~3s ---
            if tick.is_multiple_of(120) {
                update_ping(&network, &rt_handle, &w);
            }
        },
    );

    std::mem::forget(timer);
}

// ─── Keybind Listening ───

struct ListenState {
    baseline: Vec<Keycode>,
    candidate: Vec<Keycode>,
    stable_ticks: u8,
    total_ticks: u16,
}

const KEYBIND_LISTEN_TIMEOUT_TICKS: u16 = 400;

fn handle_keybind_listening(
    listening: &str,
    keys: &[Keycode],
    listen_state: &Rc<RefCell<Option<ListenState>>>,
    w: &MainWindow,
    ptt_key: &Rc<RefCell<Vec<Keycode>>>,
    mute_key: &Rc<RefCell<Vec<Keycode>>>,
    deafen_key: &Rc<RefCell<Vec<Keycode>>>,
) {
    let mut ls = listen_state.borrow_mut();

    if ls.is_none() {
        *ls = Some(ListenState {
            baseline: keys.to_vec(),
            candidate: Vec::new(),
            stable_ticks: 0,
            total_ticks: 0,
        });
        return;
    }

    let state = ls.as_mut().unwrap();

    state.total_ticks += 1;
    if state.total_ticks >= KEYBIND_LISTEN_TIMEOUT_TICKS {
        log::info!("Keybind listening timed out after 10s");
        w.set_listening_keybind("".into());
        *ls = None;
        return;
    }

    if keys.contains(&Keycode::Escape) && !state.baseline.contains(&Keycode::Escape) {
        w.set_listening_keybind("".into());
        *ls = None;
        return;
    }

    let mut new_keys: Vec<Keycode> = keys
        .iter()
        .filter(|k| !state.baseline.contains(k))
        .copied()
        .collect();
    new_keys.sort_by_key(|k| keycode_sort_order(*k));
    new_keys.dedup();

    if new_keys.is_empty() {
        state.candidate.clear();
        state.stable_ticks = 0;
        return;
    }

    if new_keys == state.candidate {
        state.stable_ticks += 1;
    } else {
        state.candidate = new_keys;
        state.stable_ticks = 1;
    }

    if state.stable_ticks < 3 {
        return;
    }

    let combo = state.candidate.clone();
    let display = combo_to_display(&combo);
    let config_str = combo_to_config(&combo);

    match listening {
        "ptt" => {
            w.set_ptt_key_display(display.into());
            *ptt_key.borrow_mut() = combo;
        }
        "mute" => {
            w.set_mute_key_display(display.into());
            *mute_key.borrow_mut() = combo;
        }
        "deafen" => {
            w.set_deafen_key_display(display.into());
            *deafen_key.borrow_mut() = combo;
        }
        _ => {}
    }

    w.set_listening_keybind("".into());
    *ls = None;

    let slot = listening.to_string();
    std::thread::spawn(move || {
        let mut cfg = config_store::load_config();
        match slot.as_str() {
            "ptt" => cfg.push_to_talk_key = Some(config_str),
            "mute" => cfg.mute_key = Some(config_str),
            "deafen" => cfg.deafen_key = Some(config_str),
            _ => {}
        }
        let _ = config_store::save_config(&cfg);
    });
}

// ─── Signal + Audio ───

fn drain_signals(
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    signal_buf: &Rc<RefCell<Vec<SignalMessage>>>,
) {
    let mut buf = signal_buf.borrow_mut();
    buf.clear();
    if let Ok(mut net) = network.try_lock() {
        while let Some(msg) = net.try_recv_signal() {
            buf.push(msg);
        }
    }
}

fn process_signals(
    signal_process: &Rc<RefCell<Vec<SignalMessage>>>,
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    audio_ctx: &signal_handler::AudioContext,
    notification_at_tick: &Rc<RefCell<Option<u64>>>,
    tick: u64,
) {
    let signals = signal_process.borrow();
    if signals.is_empty() {
        return;
    }

    for sig in signals.iter() {
        match sig {
            SignalMessage::PeerJoined { peer } => {
                w.set_room_status(format!("{} joined", peer.name).into());
                *notification_at_tick.borrow_mut() = Some(tick);
            }
            SignalMessage::PeerLeft { peer_id } => {
                let name = state
                    .borrow()
                    .room
                    .participants
                    .iter()
                    .find(|p| p.id == *peer_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Someone".into());
                w.set_room_status(format!("{name} left").into());
                *notification_at_tick.borrow_mut() = Some(tick);
            }
            SignalMessage::MemberOnline { member } => {
                w.set_status_text(format!("{} came online", member.name).into());
                *notification_at_tick.borrow_mut() = Some(tick);
            }
            _ => {}
        }
    }
    signal_handler::process_signals(&signals, w, state, audio_ctx);
}

// ─── Keyboard Handling ───

fn handle_escape(
    keys: &[Keycode],
    current_view: i32,
    w: &MainWindow,
    prev_esc_held: &Rc<RefCell<bool>>,
) {
    let esc_held = keys.contains(&Keycode::Escape);
    let was_esc = *prev_esc_held.borrow();
    if esc_held && !was_esc {
        match current_view {
            2 | 3 => w.invoke_navigate(w.get_previous_view()),
            4 => w.invoke_navigate(0),
            _ => {}
        }
    }
    *prev_esc_held.borrow_mut() = esc_held;
}

#[allow(clippy::too_many_arguments)]
fn handle_room_hotkeys(
    keys: &[Keycode],
    voice: &Rc<RefCell<voice_engine::VoiceSession>>,
    state: &Rc<RefCell<shared_types::AppState>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
    w: &MainWindow,
    ptt_was_held: &Rc<RefCell<bool>>,
    prev_m_held: &Rc<RefCell<bool>>,
    prev_d_held: &Rc<RefCell<bool>>,
    mute_cooldown: &Rc<RefCell<u64>>,
    deafen_cooldown: &Rc<RefCell<u64>>,
    ptt_key: &Rc<RefCell<Vec<Keycode>>>,
    mute_key_cell: &Rc<RefCell<Vec<Keycode>>>,
    deafen_key_cell: &Rc<RefCell<Vec<Keycode>>>,
    feedback_sound: bool,
) {
    let is_ptt = voice.borrow().mic_mode == MicMode::PushToTalk;

    if is_ptt {
        let ptt_combo = ptt_key.borrow();
        if !ptt_combo.is_empty() {
            let held = combo_held(&ptt_combo, keys);
            let was_held = *ptt_was_held.borrow();

            if held != was_held {
                *ptt_was_held.borrow_mut() = held;
                let muted = !held;
                voice.borrow_mut().is_muted = muted;
                {
                    let mut s = state.borrow_mut();
                    s.room.is_muted = muted;
                    if let Some(me) = s.room.participants.iter_mut().find(|p| p.id == "self") {
                        me.is_muted = muted;
                    }
                }
                w.set_is_muted(muted);
                ui_shell::set_participants(w, &state.borrow().room.participants);

                let audio = audio.clone();
                let network = network.clone();
                rt_handle.spawn(async move {
                    audio.lock().await.set_muted(muted);
                    let net = network.lock().await;
                    let _ = net
                        .send_signal(&SignalMessage::MuteChanged { is_muted: muted })
                        .await;
                });
            }
        }
    }

    {
        let mut m_cd = mute_cooldown.borrow_mut();
        if *m_cd > 0 {
            *m_cd -= 1;
        }
        let mut d_cd = deafen_cooldown.borrow_mut();
        if *d_cd > 0 {
            *d_cd -= 1;
        }

        let mute_combo = mute_key_cell.borrow();
        if !mute_combo.is_empty() {
            let m_held = combo_held(&mute_combo, keys);
            let was_m = *prev_m_held.borrow();
            if m_held && !was_m && !is_ptt && *m_cd == 0 {
                w.invoke_toggle_mute();
                *m_cd = 4;
                if feedback_sound {
                    let will_be_muted = w.get_is_muted();
                    if let Ok(aud) = audio.try_lock() {
                        aud.play_feedback_mute(will_be_muted);
                    }
                }
            }
            *prev_m_held.borrow_mut() = m_held;
        }

        let deafen_combo = deafen_key_cell.borrow();
        if !deafen_combo.is_empty() {
            let d_held = combo_held(&deafen_combo, keys);
            let was_d = *prev_d_held.borrow();
            if d_held && !was_d && *d_cd == 0 {
                w.invoke_toggle_deafen();
                *d_cd = 4;
                if feedback_sound {
                    let will_be_deafened = w.get_is_deafened();
                    if let Ok(aud) = audio.try_lock() {
                        aud.play_feedback_deafen(will_be_deafened);
                    }
                }
            }
            *prev_d_held.borrow_mut() = d_held;
        }
    }
}

// ─── Mic Level ───

fn update_mic_level(
    tick: u64,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    state: &Rc<RefCell<shared_types::AppState>>,
    w: &MainWindow,
) {
    if !tick.is_multiple_of(2) {
        return;
    }
    if let Ok(aud) = audio.try_lock() {
        let level = aud.mic_level();
        w.set_mic_level(level);

        let self_speaking = level > 0.02 && !w.get_is_muted();
        let mut s = state.borrow_mut();
        if let Some(me) = s.room.participants.iter_mut().find(|p| p.id == "self") {
            if me.is_speaking != self_speaking {
                me.is_speaking = self_speaking;
                ui_shell::set_participants(w, &s.room.participants);
            }
        }
    }
}

fn update_preview_mic_level(
    tick: u64,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    w: &MainWindow,
) {
    if !tick.is_multiple_of(2) {
        return;
    }
    if let Ok(aud) = audio.try_lock() {
        w.set_mic_level(aud.mic_level());
    }
}

// ─── Timed Auto-Hides ───

fn auto_hide_notification(
    notification_at_tick: &Rc<RefCell<Option<u64>>>,
    tick: u64,
    w: &MainWindow,
) {
    if let Some(t) = *notification_at_tick.borrow() {
        if tick.saturating_sub(t) >= 120 {
            w.set_room_status(slint::SharedString::default());
            *notification_at_tick.borrow_mut() = None;
        }
    }
}

fn auto_clear_errors(error_at_tick: &Rc<RefCell<Option<u64>>>, tick: u64, w: &MainWindow) {
    let status = w.get_status_text();
    let is_error = status.starts_with("Failed:")
        || status.starts_with("Error:")
        || status.starts_with("Enter ")
        || status == "No server found on LAN"
        || status == "Reconnect failed";
    if is_error {
        let mut eat = error_at_tick.borrow_mut();
        if eat.is_none() {
            *eat = Some(tick);
        }
        if let Some(t) = *eat {
            if tick.saturating_sub(t) >= 320 {
                let fallback = if w.get_is_connected() {
                    "Connected"
                } else {
                    "Tap Connect"
                };
                w.set_status_text(fallback.into());
                *eat = None;
            }
        }
    } else {
        *error_at_tick.borrow_mut() = None;
    }
}

fn auto_hide_copied(copied_at_tick: &Rc<RefCell<Option<u64>>>, tick: u64, w: &MainWindow) {
    if w.get_show_copied() {
        let mut cat = copied_at_tick.borrow_mut();
        if cat.is_none() {
            *cat = Some(tick);
        }
        if let Some(t) = *cat {
            if tick.saturating_sub(t) >= 80 {
                w.set_show_copied(false);
                *cat = None;
            }
        }
    } else {
        *copied_at_tick.borrow_mut() = None;
    }
}

// ─── Ping ───

fn update_ping(
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
    w: &MainWindow,
) {
    if let Ok(net) = network.try_lock() {
        // Read last ping result
        let ms = net.ping_ms();
        if ms >= 0 {
            w.set_ping_ms(ms);
        }
        // Send next ping
        let network = network.clone();
        rt_handle.spawn(async move {
            network.lock().await.send_ping().await;
        });
    }
}

// ─── Audio Recovery ───

fn check_audio_recovery(
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    rt_handle: &tokio::runtime::Handle,
    w: &MainWindow,
) {
    if let Ok(aud) = audio.try_lock() {
        let capture_err = aud.capture_error.load(std::sync::atomic::Ordering::Relaxed);
        let playback_err = aud
            .playback_error
            .load(std::sync::atomic::Ordering::Relaxed);
        if !capture_err && !playback_err {
            return;
        }
        drop(aud);

        log::warn!("Audio device error detected, attempting recovery");
        w.set_room_status("Audio device lost, recovering...".into());
        let audio = audio.clone();
        let window_weak = w.as_weak();
        rt_handle.spawn(async move {
            let mut aud = audio.lock().await;
            if aud.capture_error.load(std::sync::atomic::Ordering::Relaxed) {
                log::info!("Restarting capture on default device");
                if let Err(e) = aud.restart_capture(None) {
                    log::error!("Capture recovery failed: {e}");
                } else {
                    aud.capture_error
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
            }
            if aud
                .playback_error
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                log::info!("Restarting playback on default device");
                if let Err(e) = aud.restart_playback(None) {
                    log::error!("Playback recovery failed: {e}");
                } else {
                    aud.playback_error
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
            }
            if let Some(w) = window_weak.upgrade() {
                w.set_room_status("Audio recovered".into());
            }
        });
    }
}

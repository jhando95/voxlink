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
    screen_share: &Arc<crate::screen_share::ScreenShareController>,
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
    let screen_share = screen_share.clone();
    let timer = slint::Timer::default();
    let screen_frame_timer = Rc::new(slint::Timer::default());
    let tick_count = Rc::new(RefCell::new(0u64));

    let audio_ctx = signal_handler::AudioContext {
        audio_started: audio_started.clone(),
        audio: audio.clone(),
        media: media.clone(),
        network: network.clone(),
        audio_active_flag: audio_active_flag.clone(),
        screen_share: screen_share.clone(),
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
    let screen_frame_timer_tick = screen_frame_timer.clone();
    let audio_started_conn = audio_started.clone();
    let audio_conn = audio.clone();
    let audio_flag_conn = audio_active_flag.clone();
    let audio_recovery = Rc::new(RefCell::new(AudioRecoveryState::default()));
    let was_in_call = Rc::new(RefCell::new(false));

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

            let in_call = *audio_started_conn.borrow();
            let current_view = w.get_current_view();
            let viewing_room = current_view == 1;

            // Reset audio recovery state when entering a new call
            {
                let prev = *was_in_call.borrow();
                *was_in_call.borrow_mut() = in_call;
                if in_call && !prev {
                    audio_recovery.borrow_mut().reset();
                }
            }

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
            let keys = device_state.get_keys();

            // --- Keybind listening mode ---
            // Check emptiness on SharedString first (O(1)) to avoid heap allocation every tick
            let listening_shared = w.get_listening_keybind();
            if !listening_shared.is_empty() {
                let listening = listening_shared.to_string();
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

                if !in_call {
                    *ptt_was_held.borrow_mut() = false;
                }

                if in_call {
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

                    // Screen share only processes frames when viewing the room
                    if viewing_room {
                        if w.get_has_screen_share() {
                            if !screen_frame_timer_tick.running() {
                                screen_frame_timer_tick.start(
                                    slint::TimerMode::Repeated,
                                    std::time::Duration::from_millis(16),
                                    {
                                        let screen_timer_window = window_weak.clone();
                                        let screen_timer_network = network.clone();
                                        move || {
                                            let Some(w) = screen_timer_window.upgrade() else {
                                                return;
                                            };
                                            if !w.get_has_screen_share() {
                                                return;
                                            }
                                            signal_handler::connection::drain_screen_share_frame(
                                                &screen_timer_network,
                                                &w,
                                            );
                                        }
                                    },
                                );
                            }
                        } else if screen_frame_timer_tick.running() {
                            screen_frame_timer_tick.stop();
                        }

                        if tick.is_multiple_of(40) {
                            screen_share.apply_to_window(&w);
                        }
                    }

                    update_mic_level(tick, &audio, &state, &w);
                } else {
                    // Not in a call — stop screen share timer if it's still running (cleanup)
                    if screen_frame_timer_tick.running() {
                        screen_frame_timer_tick.stop();
                    }
                    if current_view == 2 && w.get_mic_preview_active() {
                        update_preview_mic_level(tick, &audio, &w);
                    }
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
                    &screen_share,
                    &rt_handle,
                    &perf,
                    &audio_started_conn,
                    &audio_conn,
                    &audio_flag_conn,
                );

                if in_call {
                    check_audio_recovery(
                        &audio,
                        &rt_handle,
                        &w,
                        &audio_recovery,
                        &audio_ctx.saved_input_device,
                        &audio_ctx.saved_output_device,
                    );
                }
            }

            // --- Retry pending messages every ~2s ---
            if tick.is_multiple_of(80) {
                retry_pending_messages(&state, &network, &rt_handle);
            }

            // --- Ping every ~3s ---
            if tick.is_multiple_of(120) {
                update_ping(&network, &rt_handle, &w, &perf);
            }

            // --- Adaptive bitrate every ~5s ---
            if tick.is_multiple_of(200) && in_call {
                adapt_bitrate(&audio, w.get_ping_ms());
            }
        },
    );

    std::mem::forget(timer);
    std::mem::forget(screen_frame_timer);
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

    let Some(state) = ls.as_mut() else {
        return;
    };

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

        let is_muted = w.get_is_muted();
        let self_speaking = level > 0.02 && !is_muted;

        // Detect talking while muted
        let talking_while_muted = level > 0.02 && is_muted;
        if w.get_talking_while_muted() != talking_while_muted {
            w.set_talking_while_muted(talking_while_muted);
        }

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
    let should_clear = notification_at_tick
        .borrow()
        .map_or(false, |t| tick.saturating_sub(t) >= 120);
    if should_clear {
        w.set_room_status(slint::SharedString::default());
        *notification_at_tick.borrow_mut() = None;
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
    perf: &Rc<RefCell<perf_metrics::PerfCollector>>,
) {
    if let Ok(net) = network.try_lock() {
        // Read last ping result
        let ms = net.ping_ms();
        if ms >= 0 {
            w.set_ping_ms(ms);
        }
        // Update perf collector atomics for the perf panel
        let udp = net.is_udp_active();
        let p = perf.borrow();
        p.ping_ms.store(ms, std::sync::atomic::Ordering::Relaxed);
        p.udp_active
            .store(udp, std::sync::atomic::Ordering::Relaxed);
        drop(p);
        w.set_udp_active(udp);
        // Send next ping
        let network = network.clone();
        rt_handle.spawn(async move {
            network.lock().await.send_ping().await;
        });
    }
}

// ─── Message Retry Queue ───

fn retry_pending_messages(
    state: &Rc<RefCell<shared_types::AppState>>,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let mut to_send: Vec<shared_types::PendingMessage> = Vec::new();
    {
        let mut s = state.borrow_mut();
        for mut msg in s.pending_messages.drain(..) {
            if msg.retry_count >= 3 {
                log::warn!("Dropping message after 3 retries: {}", msg.content.chars().take(50).collect::<String>());
                continue;
            }
            msg.retry_count += 1;
            to_send.push(msg);
        }
    }

    if to_send.is_empty() {
        return;
    }

    let network = network.clone();
    rt_handle.spawn(async move {
        let net = network.lock().await;
        let mut failed = Vec::new();
        for msg in to_send {
            let signal = if msg.is_direct {
                SignalMessage::SendDirectMessage {
                    user_id: msg.channel_id.clone(),
                    content: msg.content.clone(),
                    reply_to_message_id: None,
                }
            } else {
                SignalMessage::SendTextMessage {
                    channel_id: msg.channel_id.clone(),
                    content: msg.content.clone(),
                    reply_to_message_id: None,
                }
            };
            if net.send_signal(&signal).await.is_err() {
                failed.push(msg);
            } else {
                log::debug!("Retry succeeded for pending message");
            }
        }
        if !failed.is_empty() {
            // Can't easily borrow RefCell from async — just log the failures
            log::warn!("{} message(s) still failing after retry", failed.len());
        }
    });
}

// ─── Adaptive Bitrate ───

fn adapt_bitrate(audio: &Arc<TokioMutex<audio_core::AudioEngine>>, ping_ms: i32) {
    let Some(aud) = audio.try_lock().ok() else {
        return;
    };
    let target = aud.target_bitrate();
    if target <= 0 {
        return;
    }

    // Use packet loss ratio as primary signal, RTT as secondary
    let loss = aud.packet_loss_ratio();
    let new_bitrate = if loss > 0.15 {
        // Heavy loss (>15%): aggressive reduction
        (target as f32 * 0.5) as i32
    } else if loss > 0.05 {
        // Moderate loss (>5%): reduce to 70%
        (target as f32 * 0.7) as i32
    } else if loss > 0.01 {
        // Light loss (>1%): reduce to 85%
        (target as f32 * 0.85) as i32
    } else if ping_ms > 200 {
        // No loss but high RTT: reduce to 75%
        target * 3 / 4
    } else if ping_ms > 100 {
        // No loss, moderate RTT: reduce to 90%
        target * 9 / 10
    } else {
        // Good conditions: full target
        target
    }
    .max(16_000); // Floor: 16kbps minimum

    // Reset loss counters each adaptation window
    aud.reset_loss_counters();

    let current = aud.current_bitrate();
    // Update metrics with current bitrate
    aud.metrics.encode_bitrate_kbps.store(
        (current as u32) / 1000,
        std::sync::atomic::Ordering::Relaxed,
    );
    if new_bitrate != current {
        aud.set_bitrate(new_bitrate);
        log::debug!(
            "Adaptive bitrate: {}bps → {}bps (loss={:.1}%, ping={}ms, target={}bps)",
            current,
            new_bitrate,
            loss * 100.0,
            ping_ms,
            target
        );
    }
}

// ─── Audio Recovery with Backoff & Device Hotplug ───

struct AudioRecoveryState {
    retry_count: u8,
    backoff_ticks_remaining: u64,
    backoff_ticks: u64,
    persistent_error: bool,
    /// Cached device lists for hotplug detection
    cached_input_devices: Vec<String>,
    cached_output_devices: Vec<String>,
    /// Tick counter for device polling (~1s interval = 40 ticks)
    device_poll_tick: u64,
}

impl Default for AudioRecoveryState {
    fn default() -> Self {
        Self {
            retry_count: 0,
            backoff_ticks_remaining: 0,
            backoff_ticks: 40, // 1s initial backoff
            persistent_error: false,
            cached_input_devices: Vec::new(),
            cached_output_devices: Vec::new(),
            device_poll_tick: 0,
        }
    }
}

impl AudioRecoveryState {
    fn reset(&mut self) {
        self.retry_count = 0;
        self.backoff_ticks_remaining = 0;
        self.backoff_ticks = 40;
        self.persistent_error = false;
    }

    fn schedule_retry(&mut self) {
        self.retry_count += 1;
        if self.retry_count >= 5 {
            self.persistent_error = true;
            return;
        }
        // Exponential backoff: 1s → 2s → 4s → 8s → 12s (cap)
        self.backoff_ticks = (self.backoff_ticks * 2).min(480); // 12s cap
        self.backoff_ticks_remaining = self.backoff_ticks;
    }
}

fn check_audio_recovery(
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    rt_handle: &tokio::runtime::Handle,
    w: &MainWindow,
    recovery: &Rc<RefCell<AudioRecoveryState>>,
    saved_input_device: &Rc<RefCell<Option<String>>>,
    saved_output_device: &Rc<RefCell<Option<String>>>,
) {
    let mut rec = recovery.borrow_mut();

    if rec.persistent_error {
        // Already gave up — show persistent error
        return;
    }

    // Backoff countdown
    if rec.backoff_ticks_remaining > 0 {
        rec.backoff_ticks_remaining -= 1;
        return;
    }

    if let Ok(aud) = audio.try_lock() {
        let capture_err = aud.capture_error.load(std::sync::atomic::Ordering::Relaxed);
        let playback_err = aud.playback_error.load(std::sync::atomic::Ordering::Relaxed);

        // Device hotplug detection (~1s polling)
        rec.device_poll_tick += 1;
        if rec.device_poll_tick >= 40 {
            rec.device_poll_tick = 0;

            let current_inputs = aud.input_device_names();
            let current_outputs = aud.output_device_names();

            // Check if saved device disappeared
            let saved_in = saved_input_device.borrow().clone();
            let saved_out = saved_output_device.borrow().clone();

            let input_disappeared = saved_in
                .as_ref()
                .map(|name| !current_inputs.contains(name) && rec.cached_input_devices.contains(name))
                .unwrap_or(false);
            let output_disappeared = saved_out
                .as_ref()
                .map(|name| !current_outputs.contains(name) && rec.cached_output_devices.contains(name))
                .unwrap_or(false);

            // Check if saved device reappeared
            let input_reappeared = saved_in
                .as_ref()
                .map(|name| current_inputs.contains(name) && !rec.cached_input_devices.contains(name))
                .unwrap_or(false);
            let output_reappeared = saved_out
                .as_ref()
                .map(|name| current_outputs.contains(name) && !rec.cached_output_devices.contains(name))
                .unwrap_or(false);

            rec.cached_input_devices = current_inputs;
            rec.cached_output_devices = current_outputs;

            // Auto-fallback on disappearance
            if input_disappeared && !capture_err {
                log::warn!("Saved input device disappeared, switching to default");
                w.set_room_status("Input device disconnected, switching to default...".into());
                let audio = audio.clone();
                let window_weak = w.as_weak();
                rt_handle.spawn(async move {
                    let mut aud = audio.lock().await;
                    if let Err(e) = aud.restart_capture(None) {
                        log::error!("Fallback capture restart failed: {e}");
                    } else if let Some(w) = window_weak.upgrade() {
                        w.set_room_status("Switched to default microphone".into());
                    }
                });
                drop(rec);
                return;
            }
            if output_disappeared && !playback_err {
                log::warn!("Saved output device disappeared, switching to default");
                w.set_room_status("Output device disconnected, switching to default...".into());
                let audio = audio.clone();
                let window_weak = w.as_weak();
                rt_handle.spawn(async move {
                    let mut aud = audio.lock().await;
                    if let Err(e) = aud.restart_playback(None) {
                        log::error!("Fallback playback restart failed: {e}");
                    } else if let Some(w) = window_weak.upgrade() {
                        w.set_room_status("Switched to default speakers".into());
                    }
                });
                drop(rec);
                return;
            }

            // Auto-switch back when saved device reappears
            if input_reappeared {
                log::info!("Saved input device reappeared, switching back");
                let dev = saved_in.clone();
                let audio = audio.clone();
                let window_weak = w.as_weak();
                rt_handle.spawn(async move {
                    let mut aud = audio.lock().await;
                    if let Err(e) = aud.restart_capture(dev.as_deref()) {
                        log::error!("Capture switch-back failed: {e}");
                    } else if let Some(w) = window_weak.upgrade() {
                        w.set_room_status("Reconnected to saved microphone".into());
                    }
                });
            }
            if output_reappeared {
                log::info!("Saved output device reappeared, switching back");
                let dev = saved_out.clone();
                let audio = audio.clone();
                let window_weak = w.as_weak();
                rt_handle.spawn(async move {
                    let mut aud = audio.lock().await;
                    if let Err(e) = aud.restart_playback(dev.as_deref()) {
                        log::error!("Playback switch-back failed: {e}");
                    } else if let Some(w) = window_weak.upgrade() {
                        w.set_room_status("Reconnected to saved speakers".into());
                    }
                });
            }
        }

        if !capture_err && !playback_err {
            // No errors — reset recovery state if we were recovering
            if rec.retry_count > 0 {
                rec.reset();
            }
            return;
        }
        drop(aud);

        log::warn!(
            "Audio device error detected, attempt {}/5",
            rec.retry_count + 1
        );
        w.set_room_status(
            format!(
                "Audio device lost, recovering (attempt {}/5)...",
                rec.retry_count + 1
            )
            .into(),
        );
        let audio = audio.clone();
        let window_weak = w.as_weak();
        let retry = rec.retry_count;
        rt_handle.spawn(async move {
            let mut aud = audio.lock().await;
            let mut recovered = true;
            if aud.capture_error.load(std::sync::atomic::Ordering::Relaxed) {
                log::info!("Restarting capture on default device");
                if let Err(e) = aud.restart_capture(None) {
                    log::error!("Capture recovery failed: {e}");
                    recovered = false;
                } else {
                    aud.capture_error.store(false, std::sync::atomic::Ordering::Relaxed);
                }
            }
            if aud.playback_error.load(std::sync::atomic::Ordering::Relaxed) {
                log::info!("Restarting playback on default device");
                if let Err(e) = aud.restart_playback(None) {
                    log::error!("Playback recovery failed: {e}");
                    recovered = false;
                } else {
                    aud.playback_error.store(false, std::sync::atomic::Ordering::Relaxed);
                }
            }
            if let Some(w) = window_weak.upgrade() {
                if recovered {
                    w.set_room_status("Audio recovered".into());
                } else if retry >= 4 {
                    w.set_room_status("Audio recovery failed — please check your devices".into());
                }
            }
        });
        rec.schedule_retry();
    }
}

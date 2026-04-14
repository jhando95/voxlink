pub mod keys;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use device_query::{DeviceQuery, DeviceState, Keycode};
use shared_types::{MicMode, SignalMessage};
use slint::{ComponentHandle, Model};
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

use crate::signal_handler;
use keys::{combo_held, combo_to_config, combo_to_display, keycode_sort_order};

const TICK_MS_ACTIVE: u64 = 25; // 40Hz — smooth for audio during voice calls
const TICK_MS_IDLE: u64 = 100; // 10Hz — low overhead when not in a voice call
const TICKS_PER_IDLE_FIRE: u64 = TICK_MS_IDLE / TICK_MS_ACTIVE; // tick increment per idle fire

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
    soundboard_combos: Rc<RefCell<Vec<(usize, Vec<Keycode>)>>>,
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
    let timer = Rc::new(slint::Timer::default());
    let timer_handle = timer.clone(); // cloned into the closure for set_interval
    let screen_frame_timer = Rc::new(slint::Timer::default());
    let tick_count = Rc::new(RefCell::new(0u64));
    let timer_is_active_rate = Rc::new(RefCell::new(true)); // tracks current timer rate

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
    let toast_at_tick: Rc<RefCell<Option<Instant>>> = Rc::new(RefCell::new(None));
    let signal_buf: Rc<RefCell<Vec<SignalMessage>>> = Rc::new(RefCell::new(Vec::with_capacity(8)));
    let signal_process: Rc<RefCell<Vec<SignalMessage>>> =
        Rc::new(RefCell::new(Vec::with_capacity(8)));
    let device_state = DeviceState::new();
    let ptt_was_held = Rc::new(RefCell::new(false));
    let prev_m_held = Rc::new(RefCell::new(false));
    let prev_d_held = Rc::new(RefCell::new(false));
    let prev_esc_held = Rc::new(RefCell::new(false));
    let prev_alt_arrow_held: Rc<RefCell<(bool, u64)>> = Rc::new(RefCell::new((false, 0)));
    let prev_ctrl_k_held = Rc::new(RefCell::new(false));
    let prev_qs_arrow_held = Rc::new(RefCell::new(false));
    let prev_ch_arrow_held = Rc::new(RefCell::new(false));
    let prev_ch_enter_held = Rc::new(RefCell::new(false));
    let mute_cooldown = Rc::new(RefCell::new(0u64));
    let deafen_cooldown = Rc::new(RefCell::new(0u64));
    let listen_state: Rc<RefCell<Option<ListenState>>> = Rc::new(RefCell::new(None));
    let screen_frame_timer_tick = screen_frame_timer.clone();
    let audio_started_conn = audio_started.clone();
    let audio_conn = audio.clone();
    let audio_flag_conn = audio_active_flag.clone();
    let audio_recovery = Rc::new(RefCell::new(AudioRecoveryState::default()));
    let was_in_call = Rc::new(RefCell::new(false));
    let smoothed_levels: Rc<RefCell<HashMap<String, f32>>> = Rc::new(RefCell::new(HashMap::new()));
    // Reusable cache for per-peer RMS levels — avoids Vec+String alloc every tick
    let peer_level_cache: Rc<RefCell<HashMap<String, f32>>> = Rc::new(RefCell::new(HashMap::new()));
    let last_input_time = Rc::new(RefCell::new(Instant::now()));
    let prev_keys_for_idle: Rc<RefCell<Vec<Keycode>>> = Rc::new(RefCell::new(Vec::new()));
    let is_idle = Rc::new(RefCell::new(false));
    let soundboard_was_held: Rc<RefCell<Vec<bool>>> = {
        let count = soundboard_combos.borrow().len();
        Rc::new(RefCell::new(vec![false; count]))
    };
    // Bandwidth tracking — accumulates session total bytes
    let session_bytes_total = Rc::new(RefCell::new(0u64));
    // Cached bandwidth display values — skip format! when unchanged
    let prev_session_mb_tenths = Rc::new(RefCell::new(u64::MAX)); // tenths of MB
    let prev_est_mb_hr = Rc::new(RefCell::new(u64::MAX));
    let prev_est_peer_count = Rc::new(RefCell::new(usize::MAX));
    // Wall-clock instants for periodic tasks (immune to adaptive tick rate drift)
    let last_slow_update = Rc::new(RefCell::new(Instant::now()));
    let last_typing_expiry = Rc::new(RefCell::new(Instant::now()));
    let last_ping_update = Rc::new(RefCell::new(Instant::now()));

    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(TICK_MS_ACTIVE),
        move || {
            let Some(w) = window_weak.upgrade() else {
                return;
            };

            let in_call = *audio_started_conn.borrow();
            let current_view = w.get_current_view();
            let viewing_room = current_view == 1;

            // --- Adaptive timer rate ---
            // Switch to 40Hz when in a voice call (or previewing mic), 10Hz otherwise.
            // Tick increments proportionally so all tick-based timeouts keep correct
            // wall-clock timing regardless of the current rate.
            let needs_active_rate =
                in_call || (current_view == 2 && w.get_mic_preview_active());
            {
                let mut is_active = timer_is_active_rate.borrow_mut();
                if needs_active_rate && !*is_active {
                    timer_handle
                        .set_interval(std::time::Duration::from_millis(TICK_MS_ACTIVE));
                    *is_active = true;
                } else if !needs_active_rate && *is_active {
                    timer_handle
                        .set_interval(std::time::Duration::from_millis(TICK_MS_IDLE));
                    *is_active = false;
                }
            }

            let tick = {
                let mut t = tick_count.borrow_mut();
                let increment = if needs_active_rate {
                    1
                } else {
                    TICKS_PER_IDLE_FIRE
                };
                *t += increment;
                *t
            };

            // Reset audio recovery state and bandwidth counters when entering a new call
            {
                let prev = *was_in_call.borrow();
                *was_in_call.borrow_mut() = in_call;
                if in_call && !prev {
                    audio_recovery.borrow_mut().reset();
                    *session_bytes_total.borrow_mut() = 0;
                    *prev_session_mb_tenths.borrow_mut() = u64::MAX;
                    *prev_est_mb_hr.borrow_mut() = u64::MAX;
                    *prev_est_peer_count.borrow_mut() = usize::MAX;
                    w.set_session_data_mb("0.0".into());
                    w.set_est_data_per_hour("".into());
                    // Drain any stale bandwidth counters from previous session
                    if let Ok(net) = network.try_lock() {
                        let _ = net.swap_bandwidth_counters();
                    }
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
            // Skip OS keyboard polling when disconnected and not listening for keybinds.
            // This avoids the cost of device_query syscalls during idle.
            let is_connected = w.get_is_connected();
            let listening_shared = w.get_listening_keybind();
            let need_keys = is_connected || !listening_shared.is_empty();
            let keys = if need_keys {
                device_state.get_keys()
            } else {
                Vec::new()
            };

            // --- Keybind listening mode ---
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

                // Ctrl+K / Cmd+K quick switcher toggle
                {
                    let ctrl_or_cmd = keys.contains(&Keycode::LControl)
                        || keys.contains(&Keycode::RControl)
                        || keys.contains(&Keycode::LMeta)
                        || keys.contains(&Keycode::RMeta);
                    let k_held = ctrl_or_cmd && keys.contains(&Keycode::K);
                    let was_held = *prev_ctrl_k_held.borrow();
                    if k_held && !was_held {
                        let vis = !w.get_quick_switcher_visible();
                        w.set_quick_switcher_visible(vis);
                        if !vis {
                            w.set_quick_switcher_query("".into());
                        }
                    }
                    *prev_ctrl_k_held.borrow_mut() = k_held;
                }

                // Quick switcher keyboard navigation (Up/Down to change selection)
                // Enter is handled by VxInput's `accepted` callback in Slint
                if w.get_quick_switcher_visible() {
                    let up = keys.contains(&Keycode::Up);
                    let down = keys.contains(&Keycode::Down);
                    let arrow_held = up || down;
                    let was_arrow = *prev_qs_arrow_held.borrow();
                    if arrow_held && !was_arrow {
                        let count = w.get_quick_switcher_items().row_count() as i32;
                        let max_visible = count.min(8);
                        if max_visible > 0 {
                            let cur = w.get_quick_switcher_index();
                            let next = if up {
                                if cur <= 0 { max_visible - 1 } else { cur - 1 }
                            } else {
                                if cur + 1 >= max_visible { 0 } else { cur + 1 }
                            };
                            w.set_quick_switcher_index(next);
                        }
                    }
                    *prev_qs_arrow_held.borrow_mut() = arrow_held;
                } else {
                    *prev_qs_arrow_held.borrow_mut() = false;
                }

                // Channel navigation in Space view (Up/Down/Enter without modifiers)
                if current_view == 4 && !w.get_quick_switcher_visible() {
                    let up = keys.contains(&Keycode::Up);
                    let down = keys.contains(&Keycode::Down);
                    let alt = keys.contains(&Keycode::LAlt) || keys.contains(&Keycode::RAlt);
                    let arrow_held = (up || down) && !alt;
                    let was_arrow = *prev_ch_arrow_held.borrow();
                    if arrow_held && !was_arrow {
                        let channels: Vec<ui_shell::ChannelData> = w.get_channels().iter().collect();
                        // Build list of navigable channel indices (non-header, non-collapsed)
                        let nav_indices: Vec<usize> = channels.iter().enumerate()
                            .filter(|(_, c)| !c.is_category_header && !c.category_collapsed)
                            .map(|(i, _)| i)
                            .collect();
                        if !nav_indices.is_empty() {
                            let cur = w.get_focused_channel_index();
                            let cur_pos = nav_indices.iter().position(|&i| i as i32 == cur);
                            let next_pos = if up {
                                match cur_pos {
                                    Some(0) | None => nav_indices.len() - 1,
                                    Some(p) => p - 1,
                                }
                            } else {
                                match cur_pos {
                                    Some(p) if p + 1 < nav_indices.len() => p + 1,
                                    _ => 0,
                                }
                            };
                            w.set_focused_channel_index(nav_indices[next_pos] as i32);
                        }
                    }
                    *prev_ch_arrow_held.borrow_mut() = arrow_held;

                    // Enter activates the focused channel
                    let enter_held = keys.contains(&Keycode::Enter);
                    let was_enter = *prev_ch_enter_held.borrow();
                    if enter_held && !was_enter {
                        let idx = w.get_focused_channel_index();
                        if idx >= 0 {
                            let channels: Vec<ui_shell::ChannelData> = w.get_channels().iter().collect();
                            if let Some(ch) = channels.get(idx as usize) {
                                if !ch.is_category_header {
                                    if ch.is_voice {
                                        w.invoke_join_channel(ch.id.clone());
                                    } else {
                                        w.invoke_select_text_channel(ch.id.clone());
                                    }
                                }
                            }
                        }
                    }
                    *prev_ch_enter_held.borrow_mut() = enter_held;
                } else {
                    if *prev_ch_arrow_held.borrow() || *prev_ch_enter_held.borrow() {
                        *prev_ch_arrow_held.borrow_mut() = false;
                        *prev_ch_enter_held.borrow_mut() = false;
                    }
                    // Reset focused index when leaving space view
                    if w.get_focused_channel_index() >= 0 && current_view != 4 {
                        w.set_focused_channel_index(-1);
                    }
                }

                // Ctrl+/ keyboard shortcuts overlay
                if (keys.contains(&Keycode::LControl) && keys.contains(&Keycode::Slash) ||
                   keys.contains(&Keycode::RControl) && keys.contains(&Keycode::Slash))
                    && !w.get_shortcuts_visible() {
                        w.set_shortcuts_visible(true);
                    }

                // Alt+Up/Down channel switching (Space or TextChat view)
                {
                    let alt = keys.contains(&Keycode::LAlt) || keys.contains(&Keycode::RAlt);
                    let up = keys.contains(&Keycode::Up);
                    let down = keys.contains(&Keycode::Down);
                    let was_held = prev_alt_arrow_held.borrow().0;
                    let now_held = alt && (up || down);
                    if alt && (up || down) && !was_held && (current_view == 4 || current_view == 5) {
                        let channels: Vec<ui_shell::ChannelData> = w.get_channels().iter().collect();
                        let current_ch = w.get_chat_channel_id().to_string();
                        // Find text channels (non-header, non-voice)
                        let text_ids: Vec<String> = channels.iter()
                            .filter(|c| !c.is_category_header && !c.is_voice && !c.category_collapsed)
                            .map(|c| c.id.to_string())
                            .collect();
                        if !text_ids.is_empty() {
                            let current_idx = text_ids.iter().position(|id| id == &current_ch);
                            let next_idx = if up {
                                match current_idx {
                                    Some(0) | None => text_ids.len() - 1,
                                    Some(i) => i - 1,
                                }
                            } else {
                                match current_idx {
                                    Some(i) if i + 1 < text_ids.len() => i + 1,
                                    _ => 0,
                                }
                            };
                            w.invoke_select_text_channel(text_ids[next_idx].as_str().into());
                        }
                    }
                    *prev_alt_arrow_held.borrow_mut() = (now_held, tick);
                }

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
                    );

                    // Soundboard keybind triggering
                    {
                        let combos = soundboard_combos.borrow();
                        let mut was_held = soundboard_was_held.borrow_mut();
                        for (i, (clip_idx, combo)) in combos.iter().enumerate() {
                            let held = combo_held(combo, &keys);
                            let prev = was_held.get(i).copied().unwrap_or(false);
                            if held && !prev {
                                let audio = audio.clone();
                                let idx = *clip_idx;
                                rt_handle.spawn(async move {
                                    if let Ok(aud) = audio.try_lock() {
                                        aud.play_soundboard_clip(idx);
                                    }
                                });
                            }
                            if i < was_held.len() {
                                was_held[i] = held;
                            }
                        }
                    }

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
                                        let screen_timer_share = screen_share.clone();
                                        move || {
                                            let Some(w) = screen_timer_window.upgrade() else {
                                                return;
                                            };
                                            if !w.get_has_screen_share() {
                                                return;
                                            }
                                            if w.get_is_sharing_screen() {
                                                screen_timer_share.apply_latest_preview(&w);
                                            } else {
                                                signal_handler::connection::drain_screen_share_frame(
                                                    &screen_timer_network,
                                                    &w,
                                                );
                                            }
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

                    update_mic_level(tick, &audio, &state, &w, &smoothed_levels, &peer_level_cache);
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

            // Typing dot animation phase (0->1->2->0, every 400ms = 16 ticks)
            if tick % 16 == 0 {
                let phase = w.get_typing_dot_phase();
                w.set_typing_dot_phase((phase + 1) % 3);
            }
            // Unread pulse toggle (every 30 ticks = ~750ms)
            if tick % 30 == 0 {
                w.set_unread_pulse(!w.get_unread_pulse());
            }

            // --- Timed auto-hides ---
            auto_hide_notification(&notification_at_tick, tick, &w);
            auto_clear_errors(&error_at_tick, tick, &w);
            auto_hide_copied(&copied_at_tick, tick, &w);
            // Toast auto-hide after 3 seconds (wall-clock)
            if w.get_toast_visible() {
                if toast_at_tick.borrow().is_none() {
                    *toast_at_tick.borrow_mut() = Some(Instant::now());
                }
                if toast_at_tick.borrow().is_some_and(|t| t.elapsed() >= Duration::from_secs(3)) {
                    w.set_toast_visible(false);
                    *toast_at_tick.borrow_mut() = None;
                }
            } else {
                *toast_at_tick.borrow_mut() = None;
            }

            // --- Idle auto-status (compare slices without cloning) ---
            {
                let changed = keys != prev_keys_for_idle.borrow().as_slice();
                if changed {
                    *last_input_time.borrow_mut() = Instant::now();
                    *prev_keys_for_idle.borrow_mut() = keys.to_vec();
                    if *is_idle.borrow() {
                        *is_idle.borrow_mut() = false;
                        let net = network.clone();
                        let rt = rt_handle.clone();
                        rt.spawn(async move {
                            let n = net.lock().await;
                            let _ = n.send_signal(&SignalMessage::SetStatusPreset {
                                preset: shared_types::UserStatus::Online,
                            }).await;
                        });
                    }
                }
                let idle_mins = config_store::load_config().idle_timeout_mins.max(1) as u64;
                let idle_dur = Duration::from_secs(idle_mins * 60);
                if !*is_idle.borrow() && last_input_time.borrow().elapsed() >= idle_dur {
                    *is_idle.borrow_mut() = true;
                    let net = network.clone();
                    let rt = rt_handle.clone();
                    rt.spawn(async move {
                        let n = net.lock().await;
                        let _ = n.send_signal(&SignalMessage::SetStatusPreset {
                            preset: shared_types::UserStatus::Idle,
                        }).await;
                    });
                }
            }

            // --- Slow updates every ~1s (wall-clock) ---
            if last_slow_update.borrow().elapsed() >= Duration::from_secs(1) {
                *last_slow_update.borrow_mut() = Instant::now();
                let total_dropped_frames =
                    perf.borrow()
                        .dropped_frames
                        .load(std::sync::atomic::Ordering::Relaxed) as i32;
                w.set_dropped_frames_total(total_dropped_frames);
                w.set_dropped_frames(
                    (total_dropped_frames - w.get_dropped_frames_baseline()).max(0),
                );

                let viewing_remote_screen_share = w.get_has_screen_share()
                    && !w.get_is_sharing_screen()
                    && !w.get_screen_share_owner_id().is_empty();
                let mut screen_feedback = None;

                // --- Bandwidth tracking (every ~1s) ---
                if let Ok(net) = network.try_lock() {
                    let _ = net.expire_stale_screen_chunks();
                    let chunk_counters = net.swap_screen_chunk_counters();
                    let observed_chunk_frames = chunk_counters
                        .frames_completed
                        .saturating_add(chunk_counters.frames_superseded)
                        .saturating_add(chunk_counters.frames_timed_out);
                    {
                        let perf = perf.borrow();
                        perf.screen_frames_completed.store(
                            chunk_counters.frames_completed.min(u32::MAX as u64) as u32,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        perf.screen_frames_dropped.store(
                            chunk_counters.frames_superseded.min(u32::MAX as u64) as u32,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        perf.screen_frames_timed_out.store(
                            chunk_counters.frames_timed_out.min(u32::MAX as u64) as u32,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                    if viewing_remote_screen_share && observed_chunk_frames > 0 {
                        screen_feedback = Some((
                            chunk_counters.frames_completed.min(u32::MAX as u64) as u32,
                            chunk_counters.frames_superseded.min(u32::MAX as u64) as u32,
                            chunk_counters.frames_timed_out.min(u32::MAX as u64) as u32,
                        ));
                    }

                    if in_call {
                        let (bytes_sent, bytes_recv) = net.swap_bandwidth_counters();
                        // kbps = bytes * 8 / 1000 (1-second window)
                        let kbps_up = (bytes_sent * 8 / 1000) as i32;
                        let kbps_down = (bytes_recv * 8 / 1000) as i32;
                        w.set_bandwidth_up_kbps(kbps_up);
                        w.set_bandwidth_down_kbps(kbps_down);
                        // Accumulate session total
                        let mut total = session_bytes_total.borrow_mut();
                        *total += bytes_sent + bytes_recv;
                        // Only format when the displayed value changes (tenths of MB)
                        let mb_tenths = (*total * 10) / (1024 * 1024);
                        if *prev_session_mb_tenths.borrow() != mb_tenths {
                            *prev_session_mb_tenths.borrow_mut() = mb_tenths;
                            let mb = *total as f64 / (1024.0 * 1024.0);
                            w.set_session_data_mb(format!("{mb:.1}").into());
                        }
                        // Estimated data per hour based on current bitrate and peer count
                        let peer_count = w.get_participants().row_count();
                        if kbps_up > 0 && peer_count > 0 {
                            // Upload: our stream going out once
                            // Download: one stream per other peer
                            let peers_hearing = peer_count.saturating_sub(1).max(1);
                            let total_kbps = kbps_up as u64 + (kbps_down as u64);
                            // MB/hr = kbps * 3600 / 8 / 1024
                            let mb_hr = total_kbps * 3600 / 8 / 1024;
                            // Only format+set when values actually changed
                            if *prev_est_mb_hr.borrow() != mb_hr
                                || *prev_est_peer_count.borrow() != peer_count
                            {
                                *prev_est_mb_hr.borrow_mut() = mb_hr;
                                *prev_est_peer_count.borrow_mut() = peer_count;
                                w.set_est_data_per_hour(
                                    format!("Est. {} MB/hr with {} user{}", mb_hr, peers_hearing + 1,
                                            if peers_hearing + 1 > 1 { "s" } else { "" }).into(),
                                );
                            }
                        }
                    }
                }
                if !in_call {
                    // Reset bandwidth display when not in a call
                    if w.get_bandwidth_up_kbps() != 0 {
                        w.set_bandwidth_up_kbps(0);
                        w.set_bandwidth_down_kbps(0);
                    }
                    let perf = perf.borrow();
                    perf.screen_frames_completed
                        .store(0, std::sync::atomic::Ordering::Relaxed);
                    perf.screen_frames_dropped
                        .store(0, std::sync::atomic::Ordering::Relaxed);
                    perf.screen_frames_timed_out
                        .store(0, std::sync::atomic::Ordering::Relaxed);
                }

                if let Some((frames_completed, frames_dropped, frames_timed_out)) = screen_feedback
                {
                    let network = network.clone();
                    rt_handle.spawn(async move {
                        let net = network.lock().await;
                        if let Err(err) = net
                            .send_signal(&SignalMessage::ScreenShareTransportFeedback {
                                frames_completed,
                                frames_dropped,
                                frames_timed_out,
                            })
                            .await
                        {
                            log::debug!("Failed to send screen-share transport feedback: {err}");
                        }
                    });
                }

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

            // --- Expire stale typing indicators every ~1s (wall-clock) ---
            if last_typing_expiry.borrow().elapsed() >= Duration::from_secs(1) {
                *last_typing_expiry.borrow_mut() = Instant::now();
                signal_handler::chat::expire_stale_typing(&w, &state, tick);

                // Decrement slow mode countdown timer (1s intervals)
                let remaining = w.get_slow_mode_remaining();
                if remaining > 0 {
                    w.set_slow_mode_remaining(remaining - 1);
                }
            }

            // --- Retry pending messages every ~2s ---
            if tick.is_multiple_of(80) {
                retry_pending_messages(&state, &network, &rt_handle, &w);
            }

            // --- Ping every ~3s (wall-clock) ---
            if last_ping_update.borrow().elapsed() >= Duration::from_secs(3) {
                *last_ping_update.borrow_mut() = Instant::now();
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
        w.set_status_text("Keybind listening timed out".into());
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
    crate::helpers::spawn_config_save(move || {
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
    signal_handler::process_signals(&signals, w, state, audio_ctx, tick);
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
        // Close overlays first, then navigate
        if w.get_quick_switcher_visible() {
            w.set_quick_switcher_visible(false);
        } else if w.get_shortcuts_visible() {
            w.set_shortcuts_visible(false);
        } else {
            match current_view {
                2 | 3 => w.invoke_navigate(w.get_previous_view()),
                4 => w.invoke_navigate(0),
                _ => {}
            }
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
                w.invoke_toggle_mute(); // callback handles feedback sound
                *m_cd = 4;
            }
            *prev_m_held.borrow_mut() = m_held;
        }

        let deafen_combo = deafen_key_cell.borrow();
        if !deafen_combo.is_empty() {
            let d_held = combo_held(&deafen_combo, keys);
            let was_d = *prev_d_held.borrow();
            if d_held && !was_d && *d_cd == 0 {
                w.invoke_toggle_deafen(); // callback handles feedback sound
                *d_cd = 4;
            }
            *prev_d_held.borrow_mut() = d_held;
        }
    }
}

// ─── Mic Level ───

/// Convert raw RMS (0.0–1.0) to a 0–100 percentage using dB mapping.
/// -40dB maps to 0%, 0dB maps to 100%.
#[inline]
fn rms_to_pct(rms: f32) -> f32 {
    if rms < 1e-6 {
        return 0.0;
    }
    let db = 20.0 * rms.log10();
    ((db + 40.0) * 2.5).clamp(0.0, 100.0)
}

fn update_mic_level(
    tick: u64,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    state: &Rc<RefCell<shared_types::AppState>>,
    w: &MainWindow,
    smoothed_levels: &Rc<RefCell<HashMap<String, f32>>>,
    peer_level_cache: &Rc<RefCell<HashMap<String, f32>>>,
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

        // Read per-peer RMS levels via callback — no Vec/String allocation.
        // We reuse a persistent cache HashMap to avoid re-allocating keys.
        {
            let mut cache = peer_level_cache.borrow_mut();
            // Mark all entries as stale (zero) so departed peers get 0.0
            for val in cache.values_mut() {
                *val = 0.0;
            }
            aud.for_each_peer_rms_level(|id, rms| {
                if let Some(val) = cache.get_mut(id) {
                    *val = rms;
                } else {
                    cache.insert(id.to_owned(), rms);
                }
            });
        }

        // Compute self mic level on the same dB scale
        let self_pct = rms_to_pct(level);

        let cache = peer_level_cache.borrow();
        let mut smoothed = smoothed_levels.borrow_mut();
        let mut s = state.borrow_mut();
        let mut changed = false;

        for p in s.room.participants.iter_mut() {
            let new_raw = if p.id == "self" {
                self_pct
            } else {
                cache.get(&p.id).map(|rms| rms_to_pct(*rms)).unwrap_or(0.0)
            };

            // Smoothing: fast attack (instant), slow decay (0.85 per tick)
            let prev = smoothed.get(&p.id).copied().unwrap_or(0.0);
            let displayed = if new_raw >= prev {
                new_raw
            } else {
                prev * 0.85
            };
            if let Some(val) = smoothed.get_mut(&p.id) {
                *val = displayed;
            } else {
                smoothed.insert(p.id.clone(), displayed);
            }

            let level_i32 = displayed as i32;
            if p.audio_level != level_i32 {
                p.audio_level = level_i32;
                changed = true;
            }

            // Update speaking state for self
            if p.id == "self" && p.is_speaking != self_speaking {
                p.is_speaking = self_speaking;
                changed = true;
            }
        }

        if changed {
            // Fast path: update only changed rows in the existing model.
            // Falls back to full rebuild if the row count changed (join/leave).
            if !ui_shell::update_participant_levels(w, &s.room.participants) {
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
        .is_some_and(|t| tick.saturating_sub(t) >= 120);
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
    w: &MainWindow,
) {
    let mut to_send: Vec<shared_types::PendingMessage> = Vec::new();
    let mut dropped_count = 0u32;
    {
        let mut s = state.borrow_mut();
        for mut msg in s.pending_messages.drain(..) {
            if msg.retry_count >= 3 {
                log::warn!(
                    "Dropping message after 3 retries: {}",
                    msg.content.chars().take(50).collect::<String>()
                );
                dropped_count += 1;
                continue;
            }
            msg.retry_count += 1;
            to_send.push(msg);
        }
    }

    if dropped_count > 0 {
        let text = if dropped_count == 1 {
            "Message failed to send after 3 attempts".to_string()
        } else {
            format!("{dropped_count} messages failed to send after 3 attempts")
        };
        w.set_status_text(text.into());
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

    // Dynamically adjust Opus FEC redundancy based on observed loss
    let fec_pct = if loss > 0.15 {
        20
    } else if loss > 0.05 {
        10
    } else if loss > 0.01 {
        5
    } else {
        2
    };
    aud.set_fec_loss_pct(fec_pct);

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
        let playback_err = aud
            .playback_error
            .load(std::sync::atomic::Ordering::Relaxed);

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
                .map(|name| {
                    !current_inputs.contains(name) && rec.cached_input_devices.contains(name)
                })
                .unwrap_or(false);
            let output_disappeared = saved_out
                .as_ref()
                .map(|name| {
                    !current_outputs.contains(name) && rec.cached_output_devices.contains(name)
                })
                .unwrap_or(false);

            // Check if saved device reappeared
            let input_reappeared = saved_in
                .as_ref()
                .map(|name| {
                    current_inputs.contains(name) && !rec.cached_input_devices.contains(name)
                })
                .unwrap_or(false);
            let output_reappeared = saved_out
                .as_ref()
                .map(|name| {
                    current_outputs.contains(name) && !rec.cached_output_devices.contains(name)
                })
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
                    recovered = false;
                } else {
                    aud.playback_error
                        .store(false, std::sync::atomic::Ordering::Relaxed);
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

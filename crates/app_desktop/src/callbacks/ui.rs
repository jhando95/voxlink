use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use device_query::Keycode;
use shared_types::AppView;
use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use tokio::time::{sleep, Duration};
use ui_shell::MainWindow;

use crate::helpers;

pub fn setup_navigate(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    perf: &Rc<RefCell<perf_metrics::PerfCollector>>,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    audio_started: &Rc<RefCell<bool>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let state = state.clone();
    let perf = perf.clone();
    let network = network.clone();
    let audio = audio.clone();
    let audio_started = audio_started.clone();
    let rt_handle = rt_handle.clone();
    window.on_navigate(move |view_index| {
        let view = ui_shell::index_to_view(view_index);
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let current = w.get_current_view();
        let in_live_call = *audio_started.borrow();

        if current == 2 && view_index != 2 {
            helpers::auto_save_settings(&w, &audio, &audio_started, &rt_handle);
            if w.get_mic_preview_active() {
                let audio = audio.clone();
                let window_weak = window_weak.clone();
                rt_handle.spawn(async move {
                    let mut aud = audio.lock().await;
                    aud.stop_capture();
                    if let Some(w) = window_weak.upgrade() {
                        w.set_mic_preview_active(false);
                        w.set_mic_level(0.0);
                    }
                });
            }
            if w.get_speaker_test_active() {
                let audio = audio.clone();
                let window_weak = window_weak.clone();
                rt_handle.spawn(async move {
                    if !in_live_call {
                        audio.lock().await.stop_playback();
                    }
                    if let Some(w) = window_weak.upgrade() {
                        w.set_speaker_test_active(false);
                    }
                });
            }
        }

        if current == ui_shell::view_to_index(AppView::TextChat) && view_index != current {
            let target_id = w.get_chat_channel_id().to_string();
            let is_direct_message = w.get_chat_is_direct_message();
            if !target_id.is_empty() {
                let network = network.clone();
                rt_handle.spawn(async move {
                    let net = network.lock().await;
                    let _ = if is_direct_message {
                        net.send_signal(&shared_types::SignalMessage::SetDirectTyping {
                            user_id: target_id,
                            is_typing: false,
                        })
                        .await
                    } else {
                        net.send_signal(&shared_types::SignalMessage::SetTyping {
                            channel_id: target_id,
                            is_typing: false,
                        })
                        .await
                    };
                });
            }
            w.set_chat_typing_text(slint::SharedString::default());
        }

        if current != view_index {
            w.set_previous_view(current);
        }
        state.borrow_mut().current_view = view;
        w.set_current_view(view_index);
        w.set_show_saved(false);
        if view == AppView::Performance {
            let snap = perf.borrow_mut().snapshot();
            ui_shell::update_perf_display(&w, &snap);
        }
    });
}

pub fn setup_save_settings(
    window: &MainWindow,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    audio_started: &Rc<RefCell<bool>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let audio = audio.clone();
    let audio_started = audio_started.clone();
    let rt_handle = rt_handle.clone();
    window.on_save_settings(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        helpers::auto_save_settings(&w, &audio, &audio_started, &rt_handle);
    });
}

pub fn setup_copy_room_code(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_copy_room_code(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let code = w.get_room_code().to_string();
        if !code.is_empty() && !helpers::copy_to_clipboard(&code) {
            w.set_room_status("Failed to copy to clipboard".into());
        }
    });
}

pub fn setup_refresh_devices(
    window: &MainWindow,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let audio = audio.clone();
    let rt_handle = rt_handle.clone();
    window.on_refresh_devices(move || {
        if window_weak.upgrade().is_none() {
            return;
        }
        let audio = audio.clone();
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let mut aud = audio.lock().await;
            aud.refresh_host();
            let inputs: Vec<String> = aud
                .list_input_devices()
                .into_iter()
                .map(|d| format!("{}{}", d.name, d.device_type.label()))
                .collect();
            let outputs: Vec<String> = aud
                .list_output_devices()
                .into_iter()
                .map(|d| format!("{}{}", d.name, d.device_type.label()))
                .collect();
            if let Some(w) = window_weak.upgrade() {
                ui_shell::set_device_lists(&w, &inputs, &outputs);
                let max_input = inputs.len().saturating_sub(1) as i32;
                let max_output = outputs.len().saturating_sub(1) as i32;
                let prev_input = w.get_selected_input();
                let prev_output = w.get_selected_output();
                let new_input = prev_input.min(max_input).max(0);
                let new_output = prev_output.min(max_output).max(0);
                w.set_selected_input(new_input);
                w.set_selected_output(new_output);
                if new_input != prev_input || new_output != prev_output {
                    w.set_status_text("Audio device changed — selection updated".into());
                }
            }
        });
    });
}

pub fn setup_toggle_mic_preview(
    window: &MainWindow,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    audio_started: &Rc<RefCell<bool>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let audio = audio.clone();
    let audio_started = audio_started.clone();
    let rt_handle = rt_handle.clone();
    window.on_toggle_mic_preview(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };

        if *audio_started.borrow() {
            w.set_status_text("Mic check unavailable during a live call".into());
            return;
        }

        let is_active = w.get_mic_preview_active();
        let selected_input = w.get_selected_input() as usize;
        let audio = audio.clone();
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            let mut aud = audio.lock().await;
            if is_active {
                aud.stop_capture();
                log::info!("Mic preview stopped");
                let ww = window_weak.clone();
                slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        w.set_mic_preview_active(false);
                        w.set_mic_level(0.0);
                    }
                }).ok();
                return;
            }

            let device_name = aud
                .list_input_devices()
                .get(selected_input)
                .map(|device| device.name.clone());
            log::info!("Starting mic preview on: {:?}", device_name.as_deref().unwrap_or("default"));
            match aud.start_capture(device_name.as_deref()) {
                Ok(()) => {
                    log::info!("Mic preview started successfully");
                    let ww = window_weak.clone();
                    slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww.upgrade() {
                            w.set_mic_preview_active(true);
                        }
                    }).ok();
                }
                Err(e) => {
                    log::error!("Failed to start mic preview: {e}");
                    // Try default device as fallback
                    if device_name.is_some() {
                        log::info!("Trying default input device as fallback...");
                        match aud.start_capture(None) {
                            Ok(()) => {
                                log::info!("Mic preview started on default device");
                                let ww = window_weak.clone();
                                slint::invoke_from_event_loop(move || {
                                    if let Some(w) = ww.upgrade() {
                                        w.set_mic_preview_active(true);
                                    }
                                }).ok();
                                return;
                            }
                            Err(e2) => log::error!("Default input device also failed: {e2}"),
                        }
                    }
                    let ww = window_weak.clone();
                    slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww.upgrade() {
                            w.set_mic_preview_active(false);
                            w.set_mic_level(0.0);
                            w.set_status_text("Mic check could not start".into());
                        }
                    }).ok();
                }
            }
        });
    });
}

pub fn setup_play_speaker_test(
    window: &MainWindow,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    audio_started: &Rc<RefCell<bool>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let audio = audio.clone();
    let audio_started = audio_started.clone();
    let rt_handle = rt_handle.clone();
    window.on_play_speaker_test(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        if w.get_speaker_test_active() {
            return;
        }

        let selected_output = w.get_selected_output() as usize;
        let in_live_call = *audio_started.borrow();
        w.set_speaker_test_active(true);

        let audio = audio.clone();
        let window_weak = window_weak.clone();
        rt_handle.spawn(async move {
            // Helper to reset state via event loop (guaranteed to work from any thread)
            let reset = {
                let ww = window_weak.clone();
                move |msg: Option<&str>| {
                    let ww = ww.clone();
                    let msg = msg.map(|s| s.to_string());
                    slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww.upgrade() {
                            w.set_speaker_test_active(false);
                            if let Some(m) = msg {
                                w.set_status_text(m.into());
                            }
                        }
                    }).ok();
                }
            };

            // Try to acquire audio lock with timeout — don't wait forever
            let lock_result = tokio::time::timeout(
                Duration::from_secs(2),
                audio.lock(),
            ).await;

            let mut aud = match lock_result {
                Ok(guard) => guard,
                Err(_) => {
                    log::error!("Speaker test: audio lock timeout");
                    reset(Some("Audio busy — try again"));
                    return;
                }
            };

            let device_name = aud
                .list_output_devices()
                .get(selected_output)
                .map(|device| device.name.clone());

            if device_name.is_none() {
                reset(Some("No speaker route available"));
                return;
            }

            log::info!("Speaker test on: {:?}", device_name);

            let preview_playback = !in_live_call;
            if preview_playback {
                if let Err(e) = aud.start_playback(device_name.as_deref()) {
                    log::error!("Failed to start speaker test playback: {e}");
                    reset(Some("Speaker test could not start"));
                    return;
                }
                // Give the output device time to initialize before triggering the tone
                drop(aud);
                sleep(Duration::from_millis(50)).await;
                let aud = audio.lock().await;
                aud.play_output_preview();
                drop(aud);
            } else {
                aud.play_output_preview();
                drop(aud);
            }

            // Wait for tone to finish playing (300ms preview tone + buffer latency)
            sleep(Duration::from_millis(600)).await;

            if preview_playback {
                audio.lock().await.stop_playback();
            }
            reset(None);
            log::info!("Speaker test complete");
        });
    });
}

pub fn setup_clear_keybind(
    window: &MainWindow,
    ptt_key: &Rc<RefCell<Vec<Keycode>>>,
    mute_key: &Rc<RefCell<Vec<Keycode>>>,
    deafen_key: &Rc<RefCell<Vec<Keycode>>>,
) {
    let window_weak = window.as_weak();
    let ptt = ptt_key.clone();
    let mute = mute_key.clone();
    let deafen = deafen_key.clone();
    window.on_clear_keybind(move |slot| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        match slot.as_str() {
            "ptt" => {
                ptt.borrow_mut().clear();
                w.set_ptt_key_display("".into());
            }
            "mute" => {
                mute.borrow_mut().clear();
                w.set_mute_key_display("".into());
            }
            "deafen" => {
                deafen.borrow_mut().clear();
                w.set_deafen_key_display("".into());
            }
            _ => {}
        }
        w.set_listening_keybind("".into());

        let s = slot.to_string();
        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            match s.as_str() {
                "ptt" => cfg.push_to_talk_key = Some(String::new()),
                "mute" => cfg.mute_key = Some(String::new()),
                "deafen" => cfg.deafen_key = Some(String::new()),
                _ => {}
            }
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_toggle_dark_mode(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_toggle_dark_mode(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let new_mode = !w.get_dark_mode();
        w.set_dark_mode(new_mode);
        ui_shell::sync_member_widget_theme(new_mode, w.get_theme_preset());

        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.dark_mode = Some(new_mode);
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_select_theme_preset(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_select_theme_preset(move |preset| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let preset = helpers::sanitize_theme_preset(preset);
        if preset == w.get_theme_preset() {
            return;
        }
        w.set_theme_preset(preset);
        ui_shell::sync_member_widget_theme(w.get_dark_mode(), preset);

        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.theme_preset = helpers::theme_preset_key(preset).into();
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_toggle_member_widget(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
) {
    let window_weak = window.as_weak();
    let state = state.clone();
    window.on_toggle_member_widget(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let next_visible = !w.get_member_widget_visible();
        if next_visible {
            if !ui_shell::ensure_member_widget() {
                w.set_status_text("Member pop-out could not open".into());
                return;
            }
            let state = state.borrow();
            ui_shell::sync_member_widget(state.space.as_ref(), &state.favorite_friends);
            ui_shell::sync_member_widget_theme(w.get_dark_mode(), w.get_theme_preset());
        }

        if ui_shell::set_member_widget_visible(next_visible) {
            w.set_member_widget_visible(next_visible);
            crate::helpers::save_member_widget_state_async(next_visible, None);
        } else if next_visible {
            w.set_status_text("Member pop-out could not open".into());
        }
    });
}

pub fn setup_friend_actions(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    {
        let network = network.clone();
        let rt_handle = rt_handle.clone();
        let window_weak = window.as_weak();
        window.on_send_friend_request(move |user_id| {
            let user_id = user_id.trim().to_string();
            if user_id.is_empty() {
                return;
            }
            if let Some(w) = window_weak.upgrade() {
                w.set_status_text("Friend request sent".into());
            }
            let network = network.clone();
            rt_handle.spawn(async move {
                let net = network.lock().await;
                let _ = net
                    .send_signal(&shared_types::SignalMessage::SendFriendRequest { user_id })
                    .await;
            });
        });
    }

    {
        let network = network.clone();
        let rt_handle = rt_handle.clone();
        let window_weak = window.as_weak();
        window.on_send_friend_request_by_name(move |name| {
            let name = name.trim().to_string();
            if name.is_empty() {
                return;
            }
            if let Some(w) = window_weak.upgrade() {
                w.set_status_text(format!("Sending friend request to {name}...").into());
            }
            let network = network.clone();
            rt_handle.spawn(async move {
                let net = network.lock().await;
                let _ = net
                    .send_signal(&shared_types::SignalMessage::SendFriendRequestByName { name })
                    .await;
            });
        });
    }

    {
        let network = network.clone();
        let rt_handle = rt_handle.clone();
        let window_weak = window.as_weak();
        window.on_accept_friend_request(move |user_id| {
            let user_id = user_id.trim().to_string();
            if user_id.is_empty() {
                return;
            }
            if let Some(w) = window_weak.upgrade() {
                w.set_status_text("Friend request accepted".into());
            }
            let network = network.clone();
            rt_handle.spawn(async move {
                let net = network.lock().await;
                let _ = net
                    .send_signal(&shared_types::SignalMessage::RespondFriendRequest {
                        user_id,
                        accept: true,
                    })
                    .await;
            });
        });
    }

    {
        let network = network.clone();
        let rt_handle = rt_handle.clone();
        let window_weak = window.as_weak();
        window.on_decline_friend_request(move |user_id| {
            let user_id = user_id.trim().to_string();
            if user_id.is_empty() {
                return;
            }
            if let Some(w) = window_weak.upgrade() {
                w.set_status_text("Friend request declined".into());
            }
            let network = network.clone();
            rt_handle.spawn(async move {
                let net = network.lock().await;
                let _ = net
                    .send_signal(&shared_types::SignalMessage::RespondFriendRequest {
                        user_id,
                        accept: false,
                    })
                    .await;
            });
        });
    }

    {
        let network = network.clone();
        let rt_handle = rt_handle.clone();
        let window_weak = window.as_weak();
        window.on_cancel_friend_request(move |user_id| {
            let user_id = user_id.trim().to_string();
            if user_id.is_empty() {
                return;
            }
            if let Some(w) = window_weak.upgrade() {
                w.set_status_text("Friend request canceled".into());
            }
            let network = network.clone();
            rt_handle.spawn(async move {
                let net = network.lock().await;
                let _ = net
                    .send_signal(&shared_types::SignalMessage::CancelFriendRequest { user_id })
                    .await;
            });
        });
    }

    {
        let network = network.clone();
        let rt_handle = rt_handle.clone();
        let window_weak = window.as_weak();
        window.on_remove_friend(move |user_id| {
            let user_id = user_id.trim().to_string();
            if user_id.is_empty() {
                return;
            }
            if let Some(w) = window_weak.upgrade() {
                w.set_status_text("Friend removed".into());
            }
            let network = network.clone();
            rt_handle.spawn(async move {
                let net = network.lock().await;
                let _ = net
                    .send_signal(&shared_types::SignalMessage::RemoveFriend { user_id })
                    .await;
            });
        });
    }
}

pub fn setup_toggle_feedback_sound(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_toggle_feedback_sound(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let new_val = !w.get_feedback_sound();
        w.set_feedback_sound(new_val);

        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.feedback_sound = new_val;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_toggle_notifications(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_toggle_notifications(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let new_val = !w.get_notifications_enabled();
        w.set_notifications_enabled(new_val);

        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.notifications_enabled = new_val;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_toggle_neural_noise_suppression(
    window: &MainWindow,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let audio = audio.clone();
    let rt_handle = rt_handle.clone();
    window.on_toggle_neural_noise_suppression(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let new_val = !w.get_neural_noise_suppression();
        w.set_neural_noise_suppression(new_val);

        let audio = audio.clone();
        rt_handle.spawn(async move {
            let aud = audio.lock().await;
            aud.set_noise_suppression(new_val);
        });

        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.neural_noise_suppression = new_val;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_toggle_echo_cancellation(
    window: &MainWindow,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let audio = audio.clone();
    let rt_handle = rt_handle.clone();
    window.on_toggle_echo_cancellation(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let new_val = !w.get_echo_cancellation();
        w.set_echo_cancellation(new_val);

        let audio = audio.clone();
        rt_handle.spawn(async move {
            let aud = audio.lock().await;
            aud.set_echo_cancellation(new_val);
        });

        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.echo_cancellation = new_val;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_toggle_minimize_to_tray(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_toggle_minimize_to_tray(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let new_val = !w.get_minimize_to_tray();
        w.set_minimize_to_tray(new_val);

        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.minimize_to_tray = new_val;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_noise_suppression(
    window: &MainWindow,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let audio = audio.clone();
    let rt_handle = rt_handle.clone();
    let audio2 = audio.clone();
    let rt_handle2 = rt_handle.clone();
    window.on_input_volume_changed(move |val| {
        let audio = audio2.clone();
        rt_handle2.spawn(async move {
            audio.lock().await.set_input_gain(val);
        });
        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.input_volume = val;
            let _ = config_store::save_config(&cfg);
        });
    });

    let audio3 = audio.clone();
    let rt_handle3 = rt_handle.clone();
    window.on_output_volume_changed(move |val| {
        let audio = audio3.clone();
        rt_handle3.spawn(async move {
            audio.lock().await.set_output_volume(val);
        });
        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.output_volume = val;
            let _ = config_store::save_config(&cfg);
        });
    });

    let window_weak = window.as_weak();
    window.on_noise_suppression_changed(move |val| {
        // Slider: 0=off (no suppression), 1=max (aggressive suppression)
        // Noise gate sensitivity: 0=least sensitive (high threshold), 1=most sensitive (low threshold)
        // Invert: high suppression slider = low sensitivity = stricter gate
        let sensitivity = 1.0 - val;
        let audio = audio.clone();
        rt_handle.spawn(async move {
            audio.lock().await.set_sensitivity(sensitivity);
        });

        // Update the noise gate threshold display for the processing pipeline
        if let Some(w) = window_weak.upgrade() {
            let gate_db = -50.0 + (val * 20.0);
            w.set_audio_noise_gate_threshold(format!("{:.0} dB", gate_db).into());
        }

        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.noise_suppression = val;
            cfg.open_mic_sensitivity = sensitivity;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_quick_switcher(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let state_ref = state.clone();

    // Filter callback: populate quick-switcher-items from channels + DM threads
    window.on_quick_switcher_filter({
        let window_weak = window_weak.clone();
        let state_ref = state_ref.clone();
        move |query| {
            let Some(w) = window_weak.upgrade() else { return };
            let q = query.to_string().trim().to_lowercase();
            let state = state_ref.borrow();

            let mut items: Vec<ui_shell::ChannelData> = Vec::new();

            // Add space channels (text and voice)
            if let Some(space) = &state.space {
                for ch in &space.channels {
                    if !q.is_empty() && !ch.name.to_lowercase().contains(&q) {
                        continue;
                    }
                    items.push(ui_shell::ChannelData {
                        id: ch.id.clone().into(),
                        name: ch.name.clone().into(),
                        is_voice: ch.channel_type == shared_types::ChannelType::Voice,
                        peer_count: ch.peer_count as i32,
                        category: if ch.category.is_empty() {
                            space.name.clone().into()
                        } else {
                            ch.category.clone().into()
                        },
                        ..Default::default()
                    });
                }
            }

            // Add DM threads
            for dm in &state.direct_message_threads {
                if !q.is_empty() && !dm.user_name.to_lowercase().contains(&q) {
                    continue;
                }
                items.push(ui_shell::ChannelData {
                    id: dm.user_id.clone().into(),
                    name: dm.user_name.clone().into(),
                    status: "dm".into(),
                    ..Default::default()
                });
            }

            let model = std::rc::Rc::new(slint::VecModel::from(items));
            w.set_quick_switcher_items(model.into());
            w.set_quick_switcher_index(0);
        }
    });

    // Select callback: navigate to the selected item
    window.on_quick_switcher_select({
        let window_weak = window_weak.clone();
        let state_ref = state_ref.clone();
        let network = network.clone();
        let rt_handle = rt_handle.clone();
        move |item_id| {
            let Some(w) = window_weak.upgrade() else { return };
            let item_id_str = item_id.to_string();

            // Check if item_id matches a DM thread user_id
            let is_dm = {
                let state = state_ref.borrow();
                state.direct_message_threads.iter().any(|dm| dm.user_id == item_id_str)
            };

            // Check if item_id matches a voice channel
            let is_voice = {
                let state = state_ref.borrow();
                state.space.as_ref().map_or(false, |space| {
                    space.channels.iter().any(|ch| {
                        ch.id == item_id_str
                            && ch.channel_type == shared_types::ChannelType::Voice
                    })
                })
            };

            if is_dm {
                // Navigate to DM view
                let network = network.clone();
                let id = item_id_str.clone();
                rt_handle.spawn(async move {
                    let net = network.lock().await;
                    let _ = net
                        .send_signal(&shared_types::SignalMessage::SelectDirectMessage {
                            user_id: id,
                        })
                        .await;
                });
                w.set_current_view(5);
            } else if is_voice {
                // Join voice channel
                let network = network.clone();
                rt_handle.spawn(async move {
                    let net = network.lock().await;
                    let _ = net
                        .send_signal(&shared_types::SignalMessage::JoinChannel {
                            channel_id: item_id_str,
                        })
                        .await;
                });
            } else {
                // Navigate to text channel
                let network = network.clone();
                rt_handle.spawn(async move {
                    let net = network.lock().await;
                    let _ = net
                        .send_signal(&shared_types::SignalMessage::SelectTextChannel {
                            channel_id: item_id_str,
                        })
                        .await;
                });
                w.set_current_view(5);
            }
        }
    });
}

pub fn setup_toggle_join_leave_sounds(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_toggle_join_leave_sounds(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let new_val = !w.get_join_leave_sounds();
        w.set_join_leave_sounds(new_val);

        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.join_leave_sounds = new_val;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_toggle_show_spoilers(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_toggle_show_spoilers(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let new_val = !w.get_show_spoilers();
        w.set_show_spoilers(new_val);

        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.show_spoilers = new_val;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_toggle_compact_chat(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_toggle_compact_chat(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let new_val = !w.get_compact_chat();
        w.set_compact_chat(new_val);

        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.compact_chat = new_val;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_toggle_streamer_mode(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_toggle_streamer_mode(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let new_val = !w.get_streamer_mode();
        w.set_streamer_mode(new_val);

        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.streamer_mode = new_val;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_toggle_desktop_notifications(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_toggle_desktop_notifications(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let new_val = !w.get_desktop_notifications();
        w.set_desktop_notifications(new_val);

        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.desktop_notifications = new_val;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_move_channel(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    // Move channel up
    window.on_move_channel_up({
        let state = state.clone();
        let network = network.clone();
        let rt_handle = rt_handle.clone();
        move |channel_id| {
            let channel_id = channel_id.to_string();
            let ids = {
                let s = state.borrow();
                let Some(space) = &s.space else { return };
                let mut ids: Vec<String> = space.channels.iter().map(|c| c.id.clone()).collect();
                if let Some(pos) = ids.iter().position(|id| id == &channel_id) {
                    if pos == 0 { return; }
                    ids.swap(pos, pos - 1);
                } else {
                    return;
                }
                ids
            };
            let network = network.clone();
            rt_handle.spawn(async move {
                let net = network.lock().await;
                let _ = net.send_signal(&shared_types::SignalMessage::ReorderChannels {
                    channel_ids: ids,
                }).await;
            });
        }
    });

    // Move channel down
    window.on_move_channel_down({
        let state = state.clone();
        let network = network.clone();
        let rt_handle = rt_handle.clone();
        move |channel_id| {
            let channel_id = channel_id.to_string();
            let ids = {
                let s = state.borrow();
                let Some(space) = &s.space else { return };
                let mut ids: Vec<String> = space.channels.iter().map(|c| c.id.clone()).collect();
                if let Some(pos) = ids.iter().position(|id| id == &channel_id) {
                    if pos >= ids.len() - 1 { return; }
                    ids.swap(pos, pos + 1);
                } else {
                    return;
                }
                ids
            };
            let network = network.clone();
            rt_handle.spawn(async move {
                let net = network.lock().await;
                let _ = net.send_signal(&shared_types::SignalMessage::ReorderChannels {
                    channel_ids: ids,
                }).await;
            });
        }
    });
}

pub fn setup_set_status_preset(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_set_status_preset(move |preset_index| {
        let preset = match preset_index {
            0 => shared_types::UserStatus::Online,
            1 => shared_types::UserStatus::Idle,
            2 => shared_types::UserStatus::DoNotDisturb,
            3 => shared_types::UserStatus::Invisible,
            _ => return,
        };
        if let Some(w) = window_weak.upgrade() {
            w.set_status_preset(preset_index);
        }
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;
            let _ = net.send_signal(&shared_types::SignalMessage::SetStatusPreset { preset }).await;
        });

        // Persist to config
        let preset_str = match preset_index {
            0 => "online",
            1 => "idle",
            2 => "dnd",
            3 => "invisible",
            _ => "online",
        };
        let preset_owned = preset_str.to_string();
        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.status_preset = preset_owned;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_login(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt = rt_handle.clone();
    window.on_login(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let email = w.get_auth_email().to_string();
        let password = w.get_auth_password().to_string();
        if email.is_empty() || password.is_empty() {
            w.set_auth_error("Email and password are required".into());
            return;
        }
        let net = network.clone();
        rt.spawn(async move {
            let net = net.lock().await;
            let _ = net
                .send_signal(&shared_types::SignalMessage::Login { email, password })
                .await;
        });
    });
}

pub fn setup_create_account(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt = rt_handle.clone();
    window.on_create_account(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let email = w.get_auth_email().to_string();
        let password = w.get_auth_password().to_string();
        let display_name = w.get_auth_display_name().to_string();
        if email.is_empty() || password.is_empty() || display_name.is_empty() {
            w.set_auth_error("All fields are required".into());
            return;
        }
        if password.len() < 6 {
            w.set_auth_error("Password must be at least 6 characters".into());
            return;
        }
        let net = network.clone();
        rt.spawn(async move {
            let net = net.lock().await;
            let _ = net
                .send_signal(&shared_types::SignalMessage::CreateAccount {
                    email,
                    password,
                    display_name,
                })
                .await;
        });
    });
}

pub fn setup_logout(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt = rt_handle.clone();
    window.on_logout(move || {
        let net = network.clone();
        rt.spawn(async move {
            let net = net.lock().await;
            let _ = net.send_signal(&shared_types::SignalMessage::Logout).await;
        });
    });
}

pub fn setup_revoke_all_sessions(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt = rt_handle.clone();
    window.on_revoke_all_sessions(move || {
        let net = network.clone();
        rt.spawn(async move {
            let net = net.lock().await;
            let _ = net
                .send_signal(&shared_types::SignalMessage::RevokeAllSessions)
                .await;
        });
    });
}

pub fn setup_change_display_name(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt = rt_handle.clone();
    window.on_change_display_name(move |name| {
        let name = name.to_string().trim().to_string();
        if name.is_empty() {
            return;
        }
        let net = network.clone();
        rt.spawn(async move {
            let net = net.lock().await;
            let _ = net
                .send_signal(&shared_types::SignalMessage::SetDisplayName { name })
                .await;
        });
    });
}

pub fn setup_delete_account(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt = rt_handle.clone();
    window.on_delete_account(move || {
        let net = network.clone();
        rt.spawn(async move {
            let net = net.lock().await;
            let _ = net
                .send_signal(&shared_types::SignalMessage::DeleteAccount)
                .await;
        });
    });
}

pub fn setup_toggle_category_collapse(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
) {
    let window_weak = window.as_weak();
    let state = state.clone();
    window.on_toggle_category_collapse(move |category| {
        let cat = category.to_string();
        if cat.is_empty() {
            return;
        }
        // Update config
        let mut cfg = config_store::load_config();
        if let Some(pos) = cfg.collapsed_categories.iter().position(|c| c == &cat) {
            cfg.collapsed_categories.remove(pos);
        } else {
            cfg.collapsed_categories.push(cat);
        }
        let _ = config_store::save_config(&cfg);
        // Re-render space view to reflect the change
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let s = state.borrow();
        if let Some(ref space) = s.space {
            let query = w.get_space_search_query().to_string();
            let cfg = config_store::load_config();
            ui_shell::render_space(
                &w,
                space,
                &query,
                &s.favorite_friends,
                &s.incoming_friend_requests,
                &s.outgoing_friend_requests,
                s.self_user_id.as_deref(),
                &cfg.collapsed_categories,
                &cfg.user_notes,
                &cfg.channel_notification_overrides,
                &cfg.favorite_channels,
            );
        }
    });
}

pub fn setup_set_notification_sound(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_set_notification_sound(move |index| {
        if !(0..=3).contains(&index) {
            return;
        }
        if let Some(w) = window_weak.upgrade() {
            w.set_notification_sound_index(index);
        }
        let key = match index {
            1 => "subtle",
            2 => "chime",
            3 => "none",
            _ => "default",
        };
        let key_owned = key.to_string();
        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.notification_sound = key_owned;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_set_idle_timeout(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_set_idle_timeout(move |mins| {
        let mins = mins.max(0) as u32;
        if let Some(w) = window_weak.upgrade() {
            w.set_idle_timeout_mins(mins as i32);
        }
        crate::helpers::spawn_config_save(move || {
            let mut cfg = config_store::load_config();
            cfg.idle_timeout_mins = mins;
            let _ = config_store::save_config(&cfg);
        });
    });
}

pub fn setup_set_channel_notification(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
) {
    let window_weak = window.as_weak();
    let state = state.clone();
    window.on_set_channel_notification(move |channel_id, setting| {
        let channel_id = channel_id.to_string();
        let setting = setting.to_string();
        if channel_id.is_empty() {
            return;
        }
        // Update config
        let mut cfg = config_store::load_config();
        if setting == "all" || setting.is_empty() {
            // "all" is the default — remove override to keep config clean
            cfg.channel_notification_overrides.remove(&channel_id);
        } else {
            cfg.channel_notification_overrides
                .insert(channel_id, setting);
        }
        let _ = config_store::save_config(&cfg);
        // Re-render space view to reflect the change
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let s = state.borrow();
        if let Some(ref space) = s.space {
            let query = w.get_space_search_query().to_string();
            let cfg = config_store::load_config();
            ui_shell::render_space(
                &w,
                space,
                &query,
                &s.favorite_friends,
                &s.incoming_friend_requests,
                &s.outgoing_friend_requests,
                s.self_user_id.as_deref(),
                &cfg.collapsed_categories,
                &cfg.user_notes,
                &cfg.channel_notification_overrides,
                &cfg.favorite_channels,
            );
        }
    });
}

pub fn setup_change_password(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let network = network.clone();
    let rt = rt_handle.clone();
    let window_weak = window.as_weak();
    window.on_change_password(move |current_password, new_password| {
        let current_password = current_password.to_string();
        let new_password = new_password.to_string();
        if current_password.is_empty() || new_password.is_empty() {
            if let Some(w) = window_weak.upgrade() {
                helpers::show_toast(&w, "Both passwords are required", 3);
            }
            return;
        }
        if new_password.len() < 6 {
            if let Some(w) = window_weak.upgrade() {
                helpers::show_toast(&w, "New password must be at least 6 characters", 3);
            }
            return;
        }
        let net = network.clone();
        rt.spawn(async move {
            let net = net.lock().await;
            let _ = net
                .send_signal(&shared_types::SignalMessage::ChangePassword {
                    current_password,
                    new_password,
                })
                .await;
        });
    });
}

pub fn setup_show_profile(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
) {
    let state = state.clone();
    let window_weak = window.as_weak();
    window.on_show_profile(move |user_id| {
        let user_id = user_id.trim().to_string();
        if user_id.is_empty() {
            return;
        }
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        // Look up user info from space member list or friend list
        let s = state.borrow();
        let mut found = false;
        if let Some(ref space) = s.space {
            if let Some(member) = space.members.iter().find(|m| {
                m.user_id.as_deref() == Some(&user_id) || m.id == user_id
            }) {
                let name = member.nickname.as_deref().unwrap_or(&member.name);
                let initial = name.chars().next().unwrap_or('?').to_uppercase().to_string();
                let ci = name
                    .bytes()
                    .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32))
                    % 8;
                let role_label = match member.role {
                    shared_types::SpaceRole::Owner => "Owner",
                    shared_types::SpaceRole::Admin => "Admin",
                    shared_types::SpaceRole::Moderator => "Moderator",
                    shared_types::SpaceRole::Member => "Member",
                };
                w.set_profile_popup_user_id(user_id.as_str().into());
                w.set_profile_popup_name(name.into());
                w.set_profile_popup_bio(member.bio.as_str().into());
                w.set_profile_popup_status(member.status.as_str().into());
                w.set_profile_popup_role(role_label.into());
                w.set_profile_popup_role_color(member.role_color.as_str().into());
                w.set_profile_popup_activity(member.activity.as_str().into());
                w.set_profile_popup_initial(initial.into());
                w.set_profile_popup_color_index(ci as i32);
                w.set_profile_popup_visible(true);
                found = true;
            }
        }
        if !found {
            // Check friend list
            if let Some(friend) = s.favorite_friends.iter().find(|f| f.user_id == user_id) {
                let initial = friend
                    .name
                    .chars()
                    .next()
                    .unwrap_or('?')
                    .to_uppercase()
                    .to_string();
                let ci = friend
                    .name
                    .bytes()
                    .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32))
                    % 8;
                w.set_profile_popup_user_id(user_id.as_str().into());
                w.set_profile_popup_name(friend.name.as_str().into());
                w.set_profile_popup_bio(slint::SharedString::default());
                w.set_profile_popup_status(slint::SharedString::default());
                w.set_profile_popup_role(slint::SharedString::default());
                w.set_profile_popup_role_color(slint::SharedString::default());
                w.set_profile_popup_activity(slint::SharedString::default());
                w.set_profile_popup_initial(initial.into());
                w.set_profile_popup_color_index(ci as i32);
                w.set_profile_popup_visible(true);
            }
        }
    });
}

pub fn setup_toggle_favorite_channel(
    window: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
) {
    let state = state.clone();
    let window_weak = window.as_weak();
    window.on_toggle_favorite_channel(move |channel_id| {
        let channel_id = channel_id.to_string();
        if channel_id.is_empty() {
            return;
        }
        // Toggle in config
        let mut cfg = config_store::load_config();
        if let Some(pos) = cfg.favorite_channels.iter().position(|c| c == &channel_id) {
            cfg.favorite_channels.remove(pos);
        } else {
            cfg.favorite_channels.push(channel_id);
        }
        let _ = config_store::save_config(&cfg);
        // Re-render space view to reflect the change
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let s = state.borrow();
        if let Some(ref space) = s.space {
            let query = w.get_space_search_query().to_string();
            let cfg = config_store::load_config();
            ui_shell::render_space(
                &w,
                space,
                &query,
                &s.favorite_friends,
                &s.incoming_friend_requests,
                &s.outgoing_friend_requests,
                s.self_user_id.as_deref(),
                &cfg.collapsed_categories,
                &cfg.user_notes,
                &cfg.channel_notification_overrides,
                &cfg.favorite_channels,
            );
        }
    });
}

pub fn setup_dismiss_welcome(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_dismiss_welcome(move || {
        if let Some(w) = window_weak.upgrade() {
            w.set_first_run(false);
        }
        // Persist the dismissal so the welcome card never shows again
        let mut cfg = config_store::load_config();
        cfg.first_run_completed = true;
        if let Err(e) = config_store::save_config(&cfg) {
            log::warn!("Failed to save first_run_completed: {e}");
        }
    });
}

pub fn setup_clear_local_data(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_clear_local_data(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        match config_store::reset_to_defaults() {
            Ok(()) => {
                log::info!("Local data cleared (config reset to defaults)");
                helpers::show_toast(&w, "Local data cleared — config reset to defaults", 1);
            }
            Err(e) => {
                log::error!("Failed to clear local data: {e}");
                helpers::show_toast(&w, "Failed to clear local data", 3);
            }
        }
    });
}

pub fn setup_export_my_data(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_export_my_data(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let json = config_store::export_config_json();
        if helpers::copy_to_clipboard(&json) {
            helpers::show_toast(&w, "Config JSON copied to clipboard", 1);
        } else {
            helpers::show_toast(&w, "Failed to copy to clipboard", 3);
        }
    });
}

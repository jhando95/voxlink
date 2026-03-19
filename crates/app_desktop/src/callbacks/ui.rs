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
                w.set_selected_input(w.get_selected_input().min(max_input).max(0));
                w.set_selected_output(w.get_selected_output().min(max_output).max(0));
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
        std::thread::spawn(move || {
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

        std::thread::spawn(move || {
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

        std::thread::spawn(move || {
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

        std::thread::spawn(move || {
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

        std::thread::spawn(move || {
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

        std::thread::spawn(move || {
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

        std::thread::spawn(move || {
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

        std::thread::spawn(move || {
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
        std::thread::spawn(move || {
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
        std::thread::spawn(move || {
            let mut cfg = config_store::load_config();
            cfg.output_volume = val;
            let _ = config_store::save_config(&cfg);
        });
    });

    window.on_noise_suppression_changed(move |val| {
        // Slider: 0=off (no suppression), 1=max (aggressive suppression)
        // Noise gate sensitivity: 0=least sensitive (high threshold), 1=most sensitive (low threshold)
        // Invert: high suppression slider = low sensitivity = stricter gate
        let sensitivity = 1.0 - val;
        let audio = audio.clone();
        rt_handle.spawn(async move {
            audio.lock().await.set_sensitivity(sensitivity);
        });

        std::thread::spawn(move || {
            let mut cfg = config_store::load_config();
            cfg.noise_suppression = val;
            cfg.open_mic_sensitivity = sensitivity;
            let _ = config_store::save_config(&cfg);
        });
    });
}

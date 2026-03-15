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
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    audio_started: &Rc<RefCell<bool>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let state = state.clone();
    let perf = perf.clone();
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
                if let Some(w) = window_weak.upgrade() {
                    w.set_mic_preview_active(false);
                    w.set_mic_level(0.0);
                }
                return;
            }

            let device_name = aud
                .list_input_devices()
                .get(selected_input)
                .map(|device| device.name.clone());
            match aud.start_capture(device_name.as_deref()) {
                Ok(()) => {
                    if let Some(w) = window_weak.upgrade() {
                        w.set_mic_preview_active(true);
                    }
                }
                Err(e) => {
                    log::error!("Failed to start mic preview: {e}");
                    if let Some(w) = window_weak.upgrade() {
                        w.set_mic_preview_active(false);
                        w.set_mic_level(0.0);
                        w.set_status_text("Mic check could not start".into());
                    }
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
            let mut aud = audio.lock().await;
            let device_name = aud
                .list_output_devices()
                .get(selected_output)
                .map(|device| device.name.clone());

            if device_name.is_none() {
                if let Some(w) = window_weak.upgrade() {
                    w.set_speaker_test_active(false);
                    w.set_status_text("No speaker route available".into());
                }
                return;
            }

            let preview_playback = !in_live_call;
            if preview_playback {
                if let Err(e) = aud.start_playback(device_name.as_deref()) {
                    log::error!("Failed to start speaker test playback: {e}");
                    if let Some(w) = window_weak.upgrade() {
                        w.set_speaker_test_active(false);
                        w.set_status_text("Speaker test could not start".into());
                    }
                    return;
                }
            }

            aud.play_output_preview();
            drop(aud);

            sleep(Duration::from_millis(320)).await;

            if preview_playback {
                audio.lock().await.stop_playback();
            }
            if let Some(w) = window_weak.upgrade() {
                w.set_speaker_test_active(false);
            }
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

        std::thread::spawn(move || {
            let mut cfg = config_store::load_config();
            cfg.dark_mode = Some(new_mode);
            let _ = config_store::save_config(&cfg);
        });
    });
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

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

/// Auto-save all settings to config file with device hot-swap.
pub fn auto_save_settings(
    w: &MainWindow,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    audio_started: &Rc<RefCell<bool>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let input_idx = w.get_selected_input() as usize;
    let output_idx = w.get_selected_output() as usize;
    let is_open_mic = w.get_is_open_mic();
    let user_name = w.get_user_name().to_string().trim().to_string();
    let server_address = w.get_server_address().to_string().trim().to_string();
    let is_in_room = *audio_started.borrow();

    let audio = audio.clone();
    rt_handle.spawn(async move {
        let mut aud = audio.lock().await;
        let inputs = aud.list_input_devices();
        let outputs = aud.list_output_devices();
        let input_name = inputs.get(input_idx).map(|d| d.name.clone());
        let output_name = outputs.get(output_idx).map(|d| d.name.clone());

        if is_in_room {
            if let Err(e) = aud.restart_capture(input_name.as_deref()) {
                log::error!("Failed to restart capture: {e}");
            }
            if let Err(e) = aud.restart_playback(output_name.as_deref()) {
                log::error!("Failed to restart playback: {e}");
            }
            log::info!("Audio devices hot-swapped");
        }

        let existing = config_store::load_config();
        let cfg = config_store::AppConfig {
            input_device: input_name,
            output_device: output_name,
            push_to_talk_key: existing.push_to_talk_key,
            open_mic_sensitivity: existing.open_mic_sensitivity,
            mic_mode: if is_open_mic {
                "open_mic".into()
            } else {
                "push_to_talk".into()
            },
            user_name,
            server_address,
            last_room_code: existing.last_room_code,
            window_width: existing.window_width,
            window_height: existing.window_height,
            mute_key: existing.mute_key,
            deafen_key: existing.deafen_key,
            dark_mode: existing.dark_mode,
            saved_spaces: existing.saved_spaces,
            last_space_id: existing.last_space_id,
            last_channel_id: existing.last_channel_id,
            feedback_sound: existing.feedback_sound,
            noise_suppression: existing.noise_suppression,
        };
        match config_store::save_config(&cfg) {
            Ok(()) => log::info!("Settings auto-saved"),
            Err(e) => log::error!("Failed to save settings: {e}"),
        }
    });
}

/// Save room code to config in background (non-blocking).
pub fn save_room_code_async(code: String) {
    std::thread::spawn(move || {
        let mut cfg = config_store::load_config();
        cfg.last_room_code = Some(code);
        let _ = config_store::save_config(&cfg);
    });
}

/// Clear saved room code so we don't auto-rejoin on next launch.
pub fn clear_room_code_async() {
    std::thread::spawn(|| {
        let mut cfg = config_store::load_config();
        cfg.last_room_code = None;
        let _ = config_store::save_config(&cfg);
    });
}

/// Copy text to system clipboard. Returns true on success.
pub fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[cfg(target_os = "macos")]
    let child = Command::new("pbcopy").stdin(Stdio::piped()).spawn();

    #[cfg(target_os = "windows")]
    let child = Command::new("clip").stdin(Stdio::piped()).spawn();

    #[cfg(target_os = "linux")]
    let child = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(Stdio::piped())
        .spawn();

    match child {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                if stdin.write_all(text.as_bytes()).is_err() {
                    log::warn!("Failed to write to clipboard");
                    return false;
                }
            }
            child.wait().is_ok()
        }
        Err(e) => {
            log::warn!("Clipboard command failed: {e}");
            false
        }
    }
}

/// Save window dimensions to config (called on exit).
pub fn save_window_size(width: u32, height: u32) {
    let mut cfg = config_store::load_config();
    cfg.window_width = Some(width);
    cfg.window_height = Some(height);
    let _ = config_store::save_config(&cfg);
}

/// Start audio capture + playback if not already running.
pub fn start_audio_if_needed(
    started: &Rc<RefCell<bool>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    media: &Arc<TokioMutex<media_transport::MediaSession>>,
    audio_flag: &Arc<AtomicBool>,
    rt_handle: &tokio::runtime::Handle,
    input_device: Option<String>,
    output_device: Option<String>,
    window_weak: Option<slint::Weak<MainWindow>>,
) {
    if *started.borrow() {
        return;
    }
    *started.borrow_mut() = true;

    let audio = audio.clone();
    let media = media.clone();
    let flag = audio_flag.clone();
    rt_handle.spawn(async move {
        let mut aud = audio.lock().await;
        let mut audio_ok = true;

        if let Err(e) = aud.start_capture(input_device.as_deref()) {
            log::error!("Failed to start capture: {e}");
            audio_ok = false;
            if let Some(ref ww) = window_weak {
                if let Some(w) = ww.upgrade() {
                    w.set_room_status("Mic error — check audio settings".into());
                }
            }
        }
        if let Err(e) = aud.start_playback(output_device.as_deref()) {
            log::error!("Failed to start playback: {e}");
            audio_ok = false;
            if let Some(ref ww) = window_weak {
                if let Some(w) = ww.upgrade() {
                    w.set_room_status("Speaker error — check audio settings".into());
                }
            }
        }
        let m = media.lock().await;
        if let Err(e) = m.start().await {
            log::error!("Failed to start media session: {e}");
        }
        if audio_ok {
            flag.store(true, Ordering::Relaxed);
        }
    });
}

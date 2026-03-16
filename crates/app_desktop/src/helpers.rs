use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use slint::ComponentHandle;
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
            input_volume: existing.input_volume,
            output_volume: existing.output_volume,
            notifications_enabled: existing.notifications_enabled,
            auth_token: existing.auth_token,
            member_widget_visible: existing.member_widget_visible,
            member_widget_x: existing.member_widget_x,
            member_widget_y: existing.member_widget_y,
            favorite_friends: existing.favorite_friends,
            recent_direct_messages: existing.recent_direct_messages,
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

/// Send a desktop notification (non-blocking).
pub fn send_notification(title: &str, body: &str) {
    let title = title.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        if let Err(e) = notify_rust::Notification::new()
            .summary(&title)
            .body(&body)
            .appname("Voxlink")
            .show()
        {
            log::debug!("Notification failed: {e}");
        }
    });
}

/// Save auth token to config in background.
pub fn save_auth_token_async(token: String) {
    std::thread::spawn(move || {
        let mut cfg = config_store::load_config();
        cfg.auth_token = Some(token);
        let _ = config_store::save_config(&cfg);
    });
}

pub fn save_last_text_channel_async(space_id: String, channel_id: String) {
    std::thread::spawn(move || {
        let mut cfg = config_store::load_config();
        cfg.last_space_id = Some(space_id);
        cfg.last_channel_id = Some(channel_id);
        let _ = config_store::save_config(&cfg);
    });
}

pub fn remember_saved_space(window: &MainWindow, space: &shared_types::SpaceInfo) {
    let mut cfg = config_store::load_config();
    let server_address = cfg.server_address.clone();
    if let Some(existing) = cfg
        .saved_spaces
        .iter_mut()
        .find(|saved| saved.id == space.id)
    {
        existing.name = space.name.clone();
        existing.invite_code = space.invite_code.clone();
        existing.server_address = server_address.clone();
    } else {
        cfg.saved_spaces.push(config_store::SavedSpace {
            id: space.id.clone(),
            name: space.name.clone(),
            invite_code: space.invite_code.clone(),
            server_address,
        });
    }
    cfg.last_space_id = Some(space.id.clone());

    let spaces = cfg
        .saved_spaces
        .iter()
        .map(|saved| shared_types::SpaceInfo {
            id: saved.id.clone(),
            name: saved.name.clone(),
            invite_code: saved.invite_code.clone(),
            member_count: 0,
            channel_count: 0,
            is_owner: false,
        })
        .collect::<Vec<_>>();
    ui_shell::set_spaces(window, &spaces);

    if let Err(err) = config_store::save_config(&cfg) {
        log::warn!("Failed to persist saved space {}: {err}", space.id);
    }
}

pub fn remove_saved_space_async(space_id: String) {
    std::thread::spawn(move || {
        let mut cfg = config_store::load_config();
        cfg.saved_spaces.retain(|space| space.id != space_id);
        if cfg.last_space_id.as_deref() == Some(space_id.as_str()) {
            cfg.last_space_id = None;
            cfg.last_channel_id = None;
        }
        let _ = config_store::save_config(&cfg);
    });
}

pub fn clear_deleted_channel_async(space_id: String, channel_id: String) {
    std::thread::spawn(move || {
        let mut cfg = config_store::load_config();
        if cfg.last_space_id.as_deref() == Some(space_id.as_str())
            && cfg.last_channel_id.as_deref() == Some(channel_id.as_str())
        {
            cfg.last_channel_id = None;
            let _ = config_store::save_config(&cfg);
        }
    });
}

pub fn sync_saved_spaces_ui(window: &MainWindow, exclude_space_id: Option<&str>) {
    let mut cfg = config_store::load_config();
    if let Some(space_id) = exclude_space_id {
        cfg.saved_spaces.retain(|space| space.id != space_id);
    }
    let spaces = cfg
        .saved_spaces
        .iter()
        .map(|space| shared_types::SpaceInfo {
            id: space.id.clone(),
            name: space.name.clone(),
            invite_code: space.invite_code.clone(),
            member_count: 0,
            channel_count: 0,
            is_owner: false,
        })
        .collect::<Vec<_>>();
    ui_shell::set_spaces(window, &spaces);
}

pub fn save_member_widget_state_async(visible: bool, position: Option<(i32, i32)>) {
    std::thread::spawn(move || {
        let mut cfg = config_store::load_config();
        cfg.member_widget_visible = visible;
        if let Some((x, y)) = position {
            cfg.member_widget_x = Some(x);
            cfg.member_widget_y = Some(y);
        }
        let _ = config_store::save_config(&cfg);
    });
}

pub fn save_favorite_friends_async(favorites: Vec<shared_types::FavoriteFriend>) {
    std::thread::spawn(move || {
        let mut cfg = config_store::load_config();
        cfg.favorite_friends = favorites;
        let _ = config_store::save_config(&cfg);
    });
}

pub fn save_recent_direct_messages_async(threads: Vec<shared_types::DirectMessageThread>) {
    std::thread::spawn(move || {
        let mut cfg = config_store::load_config();
        cfg.recent_direct_messages = threads;
        let _ = config_store::save_config(&cfg);
    });
}

/// Check GitHub Releases for a newer version. Runs in a background thread.
/// Sets `update-available` and `update-version` on the window if a newer release exists.
pub fn check_for_updates(window: &MainWindow) {
    let window_weak = window.as_weak();
    let current = env!("CARGO_PKG_VERSION").to_string();
    std::thread::spawn(move || match fetch_latest_version() {
        Some(latest) if latest != current && is_newer(&latest, &current) => {
            log::info!("Update available: v{latest} (current: v{current})");
            if let Some(w) = window_weak.upgrade() {
                w.set_update_available(true);
                w.set_update_version(latest.into());
            }
        }
        Some(_) => log::debug!("Running latest version"),
        None => log::debug!("Update check failed or skipped"),
    });
}

fn fetch_latest_version() -> Option<String> {
    // GitHub Releases API returns latest release tag
    let url = "https://api.github.com/repos/jph/voxlink/releases/latest";
    let body = ureq::get(url)
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", "Voxlink-Desktop")
        .call()
        .ok()?
        .into_body()
        .read_to_string()
        .ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;
    let tag = parsed.get("tag_name")?.as_str()?;
    // Strip leading 'v' if present
    Some(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

/// Compare semver strings: returns true if `a` is newer than `b`.
fn is_newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> (u32, u32, u32) {
        let parts: Vec<u32> = s.split('.').filter_map(|p| p.parse().ok()).collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
        )
    };
    parse(a) > parse(b)
}

/// Start audio capture + playback if not already running.
#[allow(clippy::too_many_arguments)]
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

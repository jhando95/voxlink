use std::sync::Arc;
use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

pub fn setup_play_clip(
    window: &MainWindow,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let audio = audio.clone();
    let rt = rt_handle.clone();
    window.on_play_soundboard_clip(move |index| {
        let audio = audio.clone();
        rt.spawn(async move {
            if let Ok(aud) = audio.try_lock() {
                aud.play_soundboard_clip(index as usize);
            }
        });
    });
}

pub fn setup_remove_clip(
    window: &MainWindow,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let audio = audio.clone();
    let rt = rt_handle.clone();
    window.on_remove_soundboard_clip(move |index| {
        let idx = index as usize;
        // Remove from config and reload
        let audio = audio.clone();
        let window_weak = window_weak.clone();
        rt.spawn(async move {
            let _lock = crate::helpers::CONFIG_LOCK.lock().ok();
            let mut cfg = config_store::load_config();
            if idx < cfg.soundboard_clips.len() {
                cfg.soundboard_clips.remove(idx);
                let _ = config_store::save_config(&cfg);
            }
            // Reload clips in audio engine
            if let Ok(aud) = audio.try_lock() {
                aud.clear_soundboard();
                for clip in &cfg.soundboard_clips {
                    if let Err(e) = aud.load_soundboard_clip(&clip.path) {
                        log::warn!("Failed to reload soundboard clip '{}': {e}", clip.name);
                    }
                }
            }
            // Update UI
            let clip_tuples: Vec<(String, String, String)> = cfg
                .soundboard_clips
                .iter()
                .map(|c| (c.name.clone(), c.path.clone(), c.keybind.clone().unwrap_or_default()))
                .collect();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = window_weak.upgrade() {
                    ui_shell::set_soundboard_clips(&w, &clip_tuples);
                }
            });
        });
    });
}

// Hide the console window on Windows release builds.
// Users see only the Slint GUI window, not a terminal.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod callbacks;
mod helpers;
mod signal_handler;
mod tick_loop;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use shared_types::MicMode;
use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

fn main() {
    // #18: Set up logging — stderr + log file
    setup_logging();

    log::info!("Voxlink starting");

    let config = config_store::load_config();
    log::info!("Config loaded");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime");
    let rt_handle = rt.handle().clone();

    // Core systems
    let perf = Rc::new(RefCell::new(perf_metrics::PerfCollector::new()));
    let audio_active_flag = perf.borrow().audio_active.clone();
    let network_flag = perf.borrow().network_connected.clone();
    let voice = Rc::new(RefCell::new(voice_engine::VoiceSession::new()));
    let state = Rc::new(RefCell::new(shared_types::AppState::default()));
    let network = Arc::new(TokioMutex::new(net_control::NetworkClient::new()));

    // Audio engine
    let audio = Arc::new(TokioMutex::new(match audio_core::AudioEngine::new() {
        Ok(engine) => {
            log::info!("Audio engine initialized");
            engine
        }
        Err(e) => {
            log::error!("Audio engine init failed: {e}");
            panic!("Cannot run without audio: {e}");
        }
    }));

    // #6: Wire noise gate sensitivity + volume from config
    // noise_suppression: 0=off, 1=max; sensitivity: inverted (1-suppression)
    rt.block_on(async {
        let aud = audio.lock().await;
        aud.set_sensitivity(1.0 - config.noise_suppression);
        aud.set_input_gain(config.input_volume);
        aud.set_output_volume(config.output_volume);
    });

    let media = Arc::new(TokioMutex::new(media_transport::MediaSession::new(
        audio.clone(),
        network.clone(),
        perf.borrow().dropped_frames.clone(),
    )));

    let window = MainWindow::new().unwrap();

    // #13: Restore saved window size
    if let (Some(w), Some(h)) = (config.window_width, config.window_height) {
        if w > 100 && h > 100 {
            window.window().set_size(slint::LogicalSize::new(w as f32, h as f32));
        }
    }

    // Populate device lists & find saved device indices
    let (saved_input_idx, saved_output_idx) = populate_devices(&window, &audio, &rt, &config);

    // Apply saved config to UI
    apply_config(&window, &config, saved_input_idx, saved_output_idx, &voice);

    // Saved device names for audio startup
    let saved_input_device: Rc<RefCell<Option<String>>> =
        Rc::new(RefCell::new(config.input_device.clone()));
    let saved_output_device: Rc<RefCell<Option<String>>> =
        Rc::new(RefCell::new(config.output_device.clone()));
    let audio_started = Rc::new(RefCell::new(false));
    let speaking_ticks: Rc<RefCell<HashMap<String, u64>>> =
        Rc::new(RefCell::new(HashMap::new()));

    // Configurable keybinds (combo support) — shared between callbacks and tick_loop
    let ptt_key = Rc::new(RefCell::new(resolve_combo(&config.push_to_talk_key, "space")));
    let mute_key = Rc::new(RefCell::new(resolve_combo(&config.mute_key, "m")));
    let deafen_key = Rc::new(RefCell::new(resolve_combo(&config.deafen_key, "d")));

    // Set keybind display names on UI
    set_key_display(&window, &ptt_key, &mute_key, &deafen_key);

    // Apply dark mode from config
    let is_dark = config.dark_mode.unwrap_or(true);
    window.set_dark_mode(is_dark);

    // Apply feedback sound, noise suppression, and volume from config
    window.set_feedback_sound(config.feedback_sound);
    window.set_noise_suppression(config.noise_suppression);
    window.set_input_volume(config.input_volume);
    window.set_output_volume(config.output_volume);
    window.set_notifications_enabled(config.notifications_enabled);

    // Wire all UI callbacks
    callbacks::setup(
        &window, &network, &audio, &state, &voice, &perf,
        &audio_started, &audio_active_flag, &speaking_ticks, &rt_handle,
        &ptt_key, &mute_key, &deafen_key,
    );

    // Auto-connect on startup + auto-rejoin last room
    auto_connect(&window, &config, &network, &rt_handle);

    // Start the event loop timer
    tick_loop::start(
        &window, &state, &voice, &network, &audio, &media, &perf,
        &audio_started, &audio_active_flag, &network_flag, &speaking_ticks,
        saved_input_device, saved_output_device, &rt_handle,
        ptt_key, mute_key, deafen_key,
    );

    // M7B: Check for updates in background
    helpers::check_for_updates(&window);

    log::info!("Voxlink ready");
    window.run().unwrap();
    log::info!("Voxlink exiting");

    // #13: Save window size on exit
    let size = window.window().size();
    helpers::save_window_size(size.width, size.height);

    // Cleanup with timeout — prevents freeze if tasks are stuck
    let cleanup_done = rt.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let mut aud = audio.lock().await;
            aud.stop_capture();
            aud.stop_playback();
            drop(aud);
            network.lock().await.disconnect().await;
        })
        .await
    });
    if cleanup_done.is_err() {
        log::warn!("Cleanup timed out after 2s, forcing shutdown");
    }
    rt.shutdown_timeout(std::time::Duration::from_secs(1));
}

fn populate_devices(
    window: &MainWindow,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    rt: &tokio::runtime::Runtime,
    config: &config_store::AppConfig,
) -> (i32, i32) {
    let audio_guard = rt.block_on(audio.lock());
    let inputs: Vec<String> = audio_guard
        .list_input_devices()
        .into_iter()
        .map(|d| d.name)
        .collect();
    let outputs: Vec<String> = audio_guard
        .list_output_devices()
        .into_iter()
        .map(|d| d.name)
        .collect();

    let input_idx = config
        .input_device
        .as_ref()
        .and_then(|saved| inputs.iter().position(|n| n == saved))
        .unwrap_or(0);
    let output_idx = config
        .output_device
        .as_ref()
        .and_then(|saved| outputs.iter().position(|n| n == saved))
        .unwrap_or(0);

    ui_shell::set_device_lists(window, &inputs, &outputs);
    (input_idx as i32, output_idx as i32)
}

fn apply_config(
    window: &MainWindow,
    config: &config_store::AppConfig,
    input_idx: i32,
    output_idx: i32,
    voice: &Rc<RefCell<voice_engine::VoiceSession>>,
) {
    window.set_version_text(env!("CARGO_PKG_VERSION").into());
    window.set_selected_input(input_idx);
    window.set_selected_output(output_idx);
    window.set_user_name(config.user_name.clone().into());
    window.set_server_address(config.server_address.clone().into());
    if let Some(ref code) = config.last_room_code {
        window.set_join_code(code.clone().into());
    }
    if config.mic_mode == "push_to_talk" {
        window.set_is_open_mic(false);
        voice.borrow_mut().set_mic_mode(MicMode::PushToTalk);
    }

    // Populate saved spaces list
    if !config.saved_spaces.is_empty() {
        let space_infos: Vec<shared_types::SpaceInfo> = config
            .saved_spaces
            .iter()
            .map(|s| shared_types::SpaceInfo {
                id: s.id.clone(),
                name: s.name.clone(),
                invite_code: s.invite_code.clone(),
                member_count: 0,
                channel_count: 0,
            })
            .collect();
        ui_shell::set_spaces(window, &space_infos);
    }
}

// #18: Dual logger — writes to both stderr and a log file
struct DualLogger {
    env_logger: env_logger::Logger,
    file: std::sync::Mutex<std::fs::File>,
}

impl log::Log for DualLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.env_logger.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            // Write to stderr via env_logger
            self.env_logger.log(record);
            // Also write to file
            if let Ok(mut f) = self.file.lock() {
                use std::io::Write;
                let _ = writeln!(f, "[{}] {} - {}", record.level(), record.target(), record.args());
            }
        }
    }

    fn flush(&self) {
        log::Log::flush(&self.env_logger);
        if let Ok(mut f) = self.file.lock() {
            use std::io::Write;
            let _ = f.flush();
        }
    }
}

fn setup_logging() {
    let log_path = directories::ProjectDirs::from("com", "voxlink", "Voxlink")
        .map(|dirs| {
            let log_dir = dirs.data_dir();
            let _ = std::fs::create_dir_all(log_dir);
            log_dir.join("voxlink.log")
        });

    let env_logger = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .format_timestamp_millis()
    .build();

    let max_level = env_logger.filter();

    if let Some(path) = log_path {
        if let Ok(file) = std::fs::File::create(&path) {
            let dual = DualLogger {
                env_logger,
                file: std::sync::Mutex::new(file),
            };
            let _ = log::set_boxed_logger(Box::new(dual));
            log::set_max_level(max_level);
            return;
        }
    }

    // Fallback: stderr only
    let _ = log::set_boxed_logger(Box::new(env_logger));
    log::set_max_level(max_level);
}

/// Resolve a config key value to a combo Vec. None → use default. Some("") → cleared.
fn resolve_combo(config_val: &Option<String>, default: &str) -> Vec<device_query::Keycode> {
    match config_val.as_deref() {
        None => tick_loop::keys::parse_combo(default),
        Some("") => Vec::new(),
        Some(name) => tick_loop::keys::parse_combo(name),
    }
}

/// Set keybind display names on the UI from runtime combo values.
fn set_key_display(
    window: &MainWindow,
    ptt: &Rc<RefCell<Vec<device_query::Keycode>>>,
    mute: &Rc<RefCell<Vec<device_query::Keycode>>>,
    deafen: &Rc<RefCell<Vec<device_query::Keycode>>>,
) {
    window.set_ptt_key_display(tick_loop::keys::combo_to_display(&ptt.borrow()).into());
    window.set_mute_key_display(tick_loop::keys::combo_to_display(&mute.borrow()).into());
    window.set_deafen_key_display(tick_loop::keys::combo_to_display(&deafen.borrow()).into());
}

fn auto_connect(
    window: &MainWindow,
    config: &config_store::AppConfig,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let server_addr = config.server_address.clone();
    let last_room = config.last_room_code.clone();
    let user_name = config.user_name.clone();

    if server_addr.is_empty() {
        return;
    }

    log::info!("Auto-connecting to saved server: {server_addr}");
    window.set_status_text("Connecting...".into());
    let network = network.clone();
    let window_weak = window.as_weak();
    rt_handle.spawn(async move {
        let mut net = network.lock().await;
        match net.connect(&server_addr).await {
            Ok(()) => {
                log::info!("Auto-connected to server");
                if let Some(w) = window_weak.upgrade() {
                    w.set_is_connected(true);
                    w.set_status_text("Connected".into());
                }

                if let Some(room_code) = last_room {
                    if !room_code.is_empty() {
                        log::info!("Auto-rejoining last room: {room_code}");
                        if let Err(e) = net
                            .send_signal(&shared_types::SignalMessage::JoinRoom {
                                room_code,
                                user_name,
                                password: None,
                            })
                            .await
                        {
                            log::warn!("Auto-rejoin failed: {e}");
                        }
                    }
                }
            }
            Err(e) => {
                log::warn!("Auto-connect failed: {e}");
                if let Some(w) = window_weak.upgrade() {
                    w.set_status_text("Tap Connect".into());
                }
            }
        }
    });
}

mod channel;
mod chat;
mod connection;
mod controls;
mod room;
mod space;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use device_query::Keycode;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

/// Wire all `window.on_*()` callbacks. Each callback captures only what it needs.
#[allow(clippy::too_many_arguments)]
pub fn setup(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    state: &Rc<RefCell<shared_types::AppState>>,
    voice: &Rc<RefCell<voice_engine::VoiceSession>>,
    perf: &Rc<RefCell<perf_metrics::PerfCollector>>,
    audio_started: &Rc<RefCell<bool>>,
    audio_active_flag: &Arc<AtomicBool>,
    speaking_ticks: &Rc<RefCell<std::collections::HashMap<String, u64>>>,
    rt_handle: &tokio::runtime::Handle,
    ptt_key: &Rc<RefCell<Vec<Keycode>>>,
    mute_key: &Rc<RefCell<Vec<Keycode>>>,
    deafen_key: &Rc<RefCell<Vec<Keycode>>>,
) {
    // Connection
    connection::setup_connect(window, network, rt_handle);
    connection::setup_disconnect(window, network, rt_handle);
    connection::setup_find_server(window, rt_handle);

    // Room
    room::setup_create_room(window, network, rt_handle);
    room::setup_join_room(window, network, rt_handle);
    room::setup_leave_room(window, state, voice, network, audio, audio_started, audio_active_flag, speaking_ticks, rt_handle);

    // Controls
    controls::setup_toggle_mute(window, state, voice, audio, network, rt_handle);
    controls::setup_toggle_deafen(window, state, voice, audio, network, rt_handle);
    controls::setup_toggle_mic_mode(window, voice, audio, rt_handle);
    controls::setup_volume_changed(window, state, audio, rt_handle);

    // Space
    space::setup_create_space(window, network, rt_handle);
    space::setup_join_space(window, network, rt_handle);
    space::setup_select_space(window, state, network, rt_handle);
    space::setup_leave_space(window, state, voice, network, audio, audio_started, audio_active_flag, speaking_ticks, rt_handle);
    space::setup_copy_invite_code(window);
    space::setup_delete_space(window, network, rt_handle);

    // Channel
    channel::setup_create_channel(window, network, rt_handle);
    channel::setup_join_channel(window, network, rt_handle);
    channel::setup_leave_channel(window, state, voice, network, audio, audio_started, audio_active_flag, speaking_ticks, rt_handle);

    // UI / Config
    ui::setup_navigate(window, state, perf, audio, audio_started, rt_handle);
    ui::setup_save_settings(window, audio, audio_started, rt_handle);
    ui::setup_copy_room_code(window);
    ui::setup_refresh_devices(window, audio, rt_handle);
    ui::setup_clear_keybind(window, ptt_key, mute_key, deafen_key);
    ui::setup_toggle_dark_mode(window);
    ui::setup_toggle_feedback_sound(window);
    ui::setup_toggle_notifications(window);
    ui::setup_noise_suppression(window, audio, rt_handle);

    // Chat
    chat::setup_select_text_channel(window, network, rt_handle);
    chat::setup_send_text_message(window, network, rt_handle);
    chat::setup_edit_text_message(window, network, rt_handle);
    chat::setup_delete_text_message(window, network, rt_handle);
    chat::setup_react_to_message(window, network, rt_handle);
}

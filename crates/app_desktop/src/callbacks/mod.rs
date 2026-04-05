mod channel;
mod chat;
mod connection;
mod controls;
mod room;
mod soundboard;
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
    screen_share: &Arc<crate::screen_share::ScreenShareController>,
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
    connection::setup_save_server(window);
    connection::setup_remove_server(window);

    // Room
    room::setup_create_room(window, network, rt_handle);
    room::setup_join_room(window, network, rt_handle);
    room::setup_leave_room(
        window,
        state,
        voice,
        network,
        audio,
        audio_started,
        audio_active_flag,
        screen_share,
        speaking_ticks,
        rt_handle,
    );
    room::setup_toggle_screen_share(window, network, screen_share, rt_handle);
    room::setup_refresh_screen_share_sources(window, screen_share);
    room::setup_select_screen_share_source(window, screen_share);
    room::setup_select_screen_share_profile(window, screen_share);

    // Controls
    controls::setup_toggle_mute(window, state, voice, audio, network, rt_handle);
    controls::setup_toggle_deafen(window, state, voice, audio, network, rt_handle);
    controls::setup_toggle_mic_mode(window, voice, audio, rt_handle);
    controls::setup_volume_changed(window, state, audio, rt_handle);

    // Space
    space::setup_create_space(window, network, rt_handle);
    space::setup_join_space(window, network, rt_handle);
    space::setup_select_space(window, state, network, rt_handle);
    space::setup_filter_space(window, state);
    space::setup_leave_space(
        window,
        state,
        voice,
        network,
        audio,
        audio_started,
        audio_active_flag,
        screen_share,
        speaking_ticks,
        rt_handle,
    );
    space::setup_copy_invite_code(window);
    space::setup_copy_share_message(window);
    space::setup_delete_space(window, network, rt_handle);
    space::setup_kick_member(window, network, rt_handle);
    space::setup_ban_member(window, network, rt_handle);
    space::setup_server_mute_member(window, network, rt_handle);
    space::setup_set_member_role(window, network, rt_handle);
    space::setup_set_user_status(window, network, rt_handle);
    space::setup_set_channel_topic(window, network, rt_handle);
    space::setup_set_role_color(window, network, rt_handle);
    space::setup_set_activity(window, network, rt_handle);

    // Channel
    channel::setup_create_channel(window, network, rt_handle);
    channel::setup_delete_channel(window, network, rt_handle);
    channel::setup_join_channel(window, network, rt_handle);
    channel::setup_leave_channel(
        window,
        state,
        voice,
        network,
        audio,
        audio_started,
        audio_active_flag,
        screen_share,
        speaking_ticks,
        rt_handle,
    );

    // UI / Config
    ui::setup_navigate(
        window,
        state,
        perf,
        network,
        audio,
        audio_started,
        rt_handle,
    );
    ui::setup_save_settings(window, audio, audio_started, rt_handle);
    ui::setup_copy_room_code(window);
    ui::setup_refresh_devices(window, audio, rt_handle);
    ui::setup_toggle_mic_preview(window, audio, audio_started, rt_handle);
    ui::setup_play_speaker_test(window, audio, audio_started, rt_handle);
    ui::setup_clear_keybind(window, ptt_key, mute_key, deafen_key);
    ui::setup_toggle_dark_mode(window);
    ui::setup_select_theme_preset(window);
    ui::setup_toggle_member_widget(window, state);
    ui::setup_friend_actions(window, network, rt_handle);
    ui::setup_toggle_feedback_sound(window);
    ui::setup_toggle_notifications(window);
    ui::setup_toggle_minimize_to_tray(window);
    ui::setup_toggle_join_leave_sounds(window);
    ui::setup_toggle_show_spoilers(window);
    ui::setup_toggle_compact_chat(window);
    ui::setup_toggle_category_collapse(window, state);
    ui::setup_set_channel_notification(window, state);
    ui::setup_toggle_streamer_mode(window);
    ui::setup_toggle_desktop_notifications(window);
    ui::setup_quick_switcher(window, state, network, rt_handle);
    ui::setup_move_channel(window, state, network, rt_handle);
    ui::setup_set_status_preset(window, network, rt_handle);
    ui::setup_set_notification_sound(window);
    ui::setup_set_idle_timeout(window);
    ui::setup_toggle_neural_noise_suppression(window, audio, rt_handle);
    ui::setup_toggle_echo_cancellation(window, audio, rt_handle);
    ui::setup_noise_suppression(window, audio, rt_handle);
    ui::setup_login(window, network, rt_handle);
    ui::setup_create_account(window, network, rt_handle);
    ui::setup_logout(window, network, rt_handle);
    ui::setup_revoke_all_sessions(window, network, rt_handle);
    ui::setup_change_display_name(window, network, rt_handle);
    ui::setup_delete_account(window, network, rt_handle);

    // Welcome / onboarding
    ui::setup_dismiss_welcome(window);

    // Chat
    chat::setup_open_direct_message(window, network, rt_handle);
    chat::setup_close_direct_message(window, state);
    chat::setup_select_text_channel(window, network, rt_handle);
    chat::setup_chat_typing_activity(window, network, rt_handle);
    chat::setup_send_text_message(window, network, rt_handle);
    chat::setup_edit_text_message(window, network, rt_handle);
    chat::setup_delete_text_message(window, network, rt_handle);
    chat::setup_react_to_message(window, network, rt_handle);
    chat::setup_toggle_pin_message(window, network, rt_handle);
    chat::setup_forward_message(window, state, network, rt_handle);
    chat::setup_copy_message_text(window);
    chat::setup_search_messages(window, network, rt_handle);
    chat::setup_open_thread(window, network, rt_handle);
    chat::setup_close_thread(window);
    chat::setup_mention_input_changed(window, state);
    chat::setup_mention_selected(window);
    space::setup_set_profile(window, network, rt_handle);

    // v0.7 features
    space::setup_set_channel_user_limit(window, network, rt_handle);
    space::setup_set_channel_slow_mode(window, network, rt_handle);
    space::setup_set_channel_auto_delete(window, network, rt_handle);
    space::setup_set_channel_category(window, network, rt_handle);
    space::setup_set_channel_status(window, network, rt_handle);
    space::setup_timeout_member(window, network, rt_handle);
    space::setup_set_priority_speaker(window, network, rt_handle);
    space::setup_whisper(window, network, rt_handle);
    space::setup_save_user_note(window);
    space::setup_rename_space(window, network, rt_handle);
    space::setup_set_space_description(window, network, rt_handle);
    space::setup_create_event(window, network, rt_handle);
    space::setup_delete_event(window, network, rt_handle);
    space::setup_toggle_event_interest(window, network, rt_handle);
    space::setup_list_events(window, network, rt_handle);
    space::setup_browse_public_spaces(window, network, rt_handle);
    space::setup_join_public_space(window, network, rt_handle);
    space::setup_set_space_public(window, network, rt_handle);
    space::setup_add_automod_word(window, network, rt_handle);
    space::setup_remove_automod_word(window, network, rt_handle);
    space::setup_list_automod_words(window, network, rt_handle);

    // Soundboard
    soundboard::setup_play_clip(window, audio, rt_handle);
    soundboard::setup_remove_clip(window, audio, rt_handle);
    soundboard::setup_add_clip(window, audio, rt_handle);
}

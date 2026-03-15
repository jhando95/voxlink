use std::cell::RefCell;
use std::rc::Rc;

use shared_types::{AppView, SpaceState};
use ui_shell::MainWindow;

use super::AudioContext;

pub fn handle_space_created(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    space: &shared_types::SpaceInfo,
    channels: &[shared_types::ChannelInfo],
) {
    log::info!("Space created: {} ({})", space.name, space.id);

    let space_state = SpaceState {
        id: space.id.clone(),
        name: space.name.clone(),
        invite_code: space.invite_code.clone(),
        channels: channels.to_vec(),
        members: Vec::new(),
        active_channel_id: None,
        selected_text_channel_id: remembered_text_channel(space, channels),
        unread_text_channels: Default::default(),
    };

    {
        let mut s = state.borrow_mut();
        s.space = Some(space_state);
        s.current_view = AppView::Space;
    }

    w.set_current_space_id(space.id.clone().into());
    w.set_current_space_name(space.name.clone().into());
    w.set_current_space_invite(space.invite_code.clone().into());
    w.set_space_search_query(slint::SharedString::default());
    w.set_chat_channel_id(slint::SharedString::default());
    w.set_chat_channel_name(slint::SharedString::default());
    w.set_chat_input(slint::SharedString::default());
    w.set_editing_message_id(slint::SharedString::default());
    w.set_editing_original_content(slint::SharedString::default());
    w.set_is_space_owner(space.is_owner);
    {
        let s = state.borrow();
        if let Some(ref space) = s.space {
            ui_shell::render_space(w, space, "");
        }
    }
    w.set_current_view(ui_shell::view_to_index(AppView::Space));
    w.set_space_name(slint::SharedString::default());
    w.set_status_text("Space created".into());

    save_space_async(space);
}

pub fn handle_space_joined(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    space: &shared_types::SpaceInfo,
    channels: &[shared_types::ChannelInfo],
    members: &[shared_types::MemberInfo],
) {
    log::info!("Joined space: {} ({})", space.name, space.id);

    let space_state = SpaceState {
        id: space.id.clone(),
        name: space.name.clone(),
        invite_code: space.invite_code.clone(),
        channels: channels.to_vec(),
        members: members.to_vec(),
        active_channel_id: None,
        selected_text_channel_id: remembered_text_channel(space, channels),
        unread_text_channels: Default::default(),
    };

    {
        let mut s = state.borrow_mut();
        s.space = Some(space_state);
        s.current_view = AppView::Space;
    }

    w.set_current_space_id(space.id.clone().into());
    w.set_current_space_name(space.name.clone().into());
    w.set_current_space_invite(space.invite_code.clone().into());
    w.set_space_search_query(slint::SharedString::default());
    w.set_chat_channel_id(slint::SharedString::default());
    w.set_chat_channel_name(slint::SharedString::default());
    w.set_chat_input(slint::SharedString::default());
    w.set_editing_message_id(slint::SharedString::default());
    w.set_editing_original_content(slint::SharedString::default());
    w.set_is_space_owner(space.is_owner);
    {
        let s = state.borrow();
        if let Some(ref space) = s.space {
            ui_shell::render_space(w, space, "");
        }
    }
    w.set_current_view(ui_shell::view_to_index(AppView::Space));
    w.set_space_invite_code(slint::SharedString::default());
    w.set_status_text("Joined space".into());

    save_space_async(space);
}

pub fn handle_space_deleted(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    ctx: &AudioContext,
) {
    log::info!("Space deleted by owner");
    {
        let mut s = state.borrow_mut();
        s.room = Default::default();
        s.space = None;
        s.current_view = AppView::Home;
    }
    *ctx.audio_started.borrow_mut() = false;

    w.set_current_view(ui_shell::view_to_index(AppView::Home));
    w.set_room_code(slint::SharedString::default());
    w.set_current_space_id(slint::SharedString::default());
    w.set_current_space_name(slint::SharedString::default());
    w.set_current_space_invite(slint::SharedString::default());
    w.set_space_search_query(slint::SharedString::default());
    w.set_visible_text_channels(0);
    w.set_visible_voice_channels(0);
    w.set_visible_members(0);
    w.set_chat_channel_id(slint::SharedString::default());
    w.set_chat_channel_name(slint::SharedString::default());
    w.set_chat_input(slint::SharedString::default());
    w.set_editing_message_id(slint::SharedString::default());
    w.set_editing_original_content(slint::SharedString::default());
    w.set_is_muted(false);
    w.set_is_deafened(false);
    w.set_in_space_channel(false);
    w.set_is_space_owner(false);
    w.set_mic_level(0.0);
    w.set_window_title("Voxlink".into());
    w.set_status_text("Space was deleted".into());
    ui_shell::set_channels(w, &[]);
    ui_shell::set_members(w, &[]);

    let audio = ctx.audio.clone();
    let flag = ctx.audio_active_flag.clone();
    ctx.rt_handle.spawn(async move {
        let mut aud = audio.lock().await;
        aud.stop_capture();
        aud.stop_playback();
        flag.store(false, std::sync::atomic::Ordering::Relaxed);
    });
}

fn remembered_text_channel(
    space: &shared_types::SpaceInfo,
    channels: &[shared_types::ChannelInfo],
) -> Option<String> {
    let cfg = config_store::load_config();
    if cfg.last_space_id.as_deref() != Some(space.id.as_str()) {
        return None;
    }

    cfg.last_channel_id.filter(|channel_id| {
        channels.iter().any(|channel| {
            channel.id == *channel_id && channel.channel_type == shared_types::ChannelType::Text
        })
    })
}

fn save_space_async(space: &shared_types::SpaceInfo) {
    let id = space.id.clone();
    let name = space.name.clone();
    let invite_code = space.invite_code.clone();
    std::thread::spawn(move || {
        let mut cfg = config_store::load_config();
        if let Some(existing) = cfg.saved_spaces.iter_mut().find(|s| s.id == id) {
            existing.name = name;
            existing.invite_code = invite_code;
        } else {
            cfg.saved_spaces.push(config_store::SavedSpace {
                id: id.clone(),
                name,
                invite_code,
                server_address: cfg.server_address.clone(),
            });
        }
        cfg.last_space_id = Some(id);
        let _ = config_store::save_config(&cfg);
    });
}

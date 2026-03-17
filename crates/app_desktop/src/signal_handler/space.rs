use std::cell::RefCell;
use std::rc::Rc;

use shared_types::{AppView, SpaceRole, SpaceState};
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
        audit_log: Vec::new(),
        active_channel_id: None,
        selected_text_channel_id: remembered_text_channel(space, channels),
        self_role: space.self_role,
        unread_text_channels: Default::default(),
        typing_users: Default::default(),
    };

    {
        let mut s = state.borrow_mut();
        s.space = Some(space_state);
        s.active_direct_message_user_id = None;
        s.direct_typing_users.clear();
        s.current_view = AppView::Space;
    }

    w.set_current_space_id(space.id.clone().into());
    w.set_current_space_name(space.name.clone().into());
    w.set_current_space_invite(space.invite_code.clone().into());
    w.set_space_search_query(slint::SharedString::default());
    w.set_confirm_delete_channel_id(slint::SharedString::default());
    w.set_chat_channel_id(slint::SharedString::default());
    w.set_chat_channel_name(slint::SharedString::default());
    w.set_chat_is_direct_message(false);
    w.set_chat_context_subtitle(slint::SharedString::default());
    w.set_chat_back_view(ui_shell::view_to_index(AppView::Space));
    w.set_chat_input(slint::SharedString::default());
    w.set_chat_pinned_messages(
        std::rc::Rc::new(slint::VecModel::<ui_shell::ChatMessage>::from(Vec::new())).into(),
    );
    w.set_chat_typing_text(slint::SharedString::default());
    w.set_editing_message_id(slint::SharedString::default());
    w.set_editing_original_content(slint::SharedString::default());
    w.set_reply_target_message_id(slint::SharedString::default());
    w.set_reply_target_sender_name(slint::SharedString::default());
    w.set_reply_target_preview(slint::SharedString::default());
    w.set_is_space_owner(space.is_owner);
    apply_space_permissions(w, space.self_role);
    ui_shell::set_space_audit_log(w, &[]);
    {
        let mut s = state.borrow_mut();
        let favorites_changed = crate::friends::refresh_metadata_in_place(&mut s);
        drop(s);
        crate::friends::sync_ui(w, state);
        if favorites_changed {
            crate::friends::persist(state);
        }
    }
    w.set_current_view(ui_shell::view_to_index(AppView::Space));
    w.set_space_name(slint::SharedString::default());
    w.set_status_text("Space created".into());

    crate::helpers::remember_saved_space(w, space);
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
        audit_log: Vec::new(),
        active_channel_id: None,
        selected_text_channel_id: remembered_text_channel(space, channels),
        self_role: space.self_role,
        unread_text_channels: Default::default(),
        typing_users: Default::default(),
    };

    {
        let mut s = state.borrow_mut();
        s.space = Some(space_state);
        s.active_direct_message_user_id = None;
        s.direct_typing_users.clear();
        s.current_view = AppView::Space;
    }

    w.set_current_space_id(space.id.clone().into());
    w.set_current_space_name(space.name.clone().into());
    w.set_current_space_invite(space.invite_code.clone().into());
    w.set_space_search_query(slint::SharedString::default());
    w.set_confirm_delete_channel_id(slint::SharedString::default());
    w.set_chat_channel_id(slint::SharedString::default());
    w.set_chat_channel_name(slint::SharedString::default());
    w.set_chat_is_direct_message(false);
    w.set_chat_context_subtitle(slint::SharedString::default());
    w.set_chat_back_view(ui_shell::view_to_index(AppView::Space));
    w.set_chat_input(slint::SharedString::default());
    w.set_chat_pinned_messages(
        std::rc::Rc::new(slint::VecModel::<ui_shell::ChatMessage>::from(Vec::new())).into(),
    );
    w.set_chat_typing_text(slint::SharedString::default());
    w.set_editing_message_id(slint::SharedString::default());
    w.set_editing_original_content(slint::SharedString::default());
    w.set_reply_target_message_id(slint::SharedString::default());
    w.set_reply_target_sender_name(slint::SharedString::default());
    w.set_reply_target_preview(slint::SharedString::default());
    w.set_is_space_owner(space.is_owner);
    apply_space_permissions(w, space.self_role);
    ui_shell::set_space_audit_log(w, &[]);
    {
        let mut s = state.borrow_mut();
        let favorites_changed = crate::friends::refresh_metadata_in_place(&mut s);
        drop(s);
        crate::friends::sync_ui(w, state);
        if favorites_changed {
            crate::friends::persist(state);
        }
    }
    w.set_current_view(ui_shell::view_to_index(AppView::Space));
    w.set_space_invite_code(slint::SharedString::default());
    w.set_status_text("Joined space".into());

    crate::helpers::remember_saved_space(w, space);
}

pub fn handle_space_deleted(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    ctx: &AudioContext,
) {
    log::info!("Space deleted by owner");
    let deleted_space_id = state
        .borrow()
        .space
        .as_ref()
        .map(|space| space.id.clone())
        .filter(|space_id| !space_id.is_empty())
        .unwrap_or_else(|| w.get_current_space_id().to_string());
    {
        let mut s = state.borrow_mut();
        s.room = Default::default();
        s.space = None;
        s.active_direct_message_user_id = None;
        s.direct_typing_users.clear();
        s.current_view = AppView::Home;
    }
    *ctx.audio_started.borrow_mut() = false;

    w.set_current_view(ui_shell::view_to_index(AppView::Home));
    w.set_room_code(slint::SharedString::default());
    w.set_current_space_id(slint::SharedString::default());
    w.set_current_space_name(slint::SharedString::default());
    w.set_current_space_invite(slint::SharedString::default());
    w.set_space_search_query(slint::SharedString::default());
    w.set_confirm_delete_channel_id(slint::SharedString::default());
    w.set_visible_text_channels(0);
    w.set_visible_voice_channels(0);
    w.set_visible_members(0);
    w.set_chat_channel_id(slint::SharedString::default());
    w.set_chat_channel_name(slint::SharedString::default());
    w.set_chat_is_direct_message(false);
    w.set_chat_context_subtitle(slint::SharedString::default());
    w.set_chat_back_view(ui_shell::view_to_index(AppView::Space));
    w.set_chat_input(slint::SharedString::default());
    w.set_chat_pinned_messages(
        std::rc::Rc::new(slint::VecModel::<ui_shell::ChatMessage>::from(Vec::new())).into(),
    );
    w.set_chat_typing_text(slint::SharedString::default());
    w.set_editing_message_id(slint::SharedString::default());
    w.set_editing_original_content(slint::SharedString::default());
    w.set_reply_target_message_id(slint::SharedString::default());
    w.set_reply_target_sender_name(slint::SharedString::default());
    w.set_reply_target_preview(slint::SharedString::default());
    w.set_is_muted(false);
    w.set_is_deafened(false);
    w.set_in_space_channel(false);
    w.set_is_space_owner(false);
    apply_space_permissions(w, SpaceRole::Member);
    w.set_mic_level(0.0);
    w.set_window_title("Voxlink".into());
    w.set_status_text("Space was deleted".into());
    ui_shell::set_channels(w, &[]);
    ui_shell::set_members(w, &[]);
    ui_shell::set_space_audit_log(w, &[]);
    if !deleted_space_id.is_empty() {
        crate::helpers::remove_saved_space_async(deleted_space_id.clone());
        crate::helpers::sync_saved_spaces_ui(w, Some(&deleted_space_id));
    } else {
        crate::helpers::sync_saved_spaces_ui(w, None);
    }
    crate::friends::sync_ui(w, state);

    let audio = ctx.audio.clone();
    let flag = ctx.audio_active_flag.clone();
    ctx.rt_handle.spawn(async move {
        let mut aud = audio.lock().await;
        aud.stop_capture();
        aud.stop_playback();
        flag.store(false, std::sync::atomic::Ordering::Relaxed);
    });
}

pub fn apply_space_permissions(window: &MainWindow, role: SpaceRole) {
    window.set_space_role_label(
        match role {
            SpaceRole::Owner => "Owner",
            SpaceRole::Admin => "Admin",
            SpaceRole::Moderator => "Moderator",
            SpaceRole::Member => "Member",
        }
        .into(),
    );
    window.set_can_manage_space_channels(matches!(role, SpaceRole::Owner | SpaceRole::Admin));
    window.set_can_manage_space_members(matches!(
        role,
        SpaceRole::Owner | SpaceRole::Admin | SpaceRole::Moderator
    ));
    window.set_can_manage_space_roles(matches!(role, SpaceRole::Owner | SpaceRole::Admin));
    window.set_can_view_space_audit(true);
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

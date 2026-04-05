use std::cell::RefCell;
use std::rc::Rc;

use shared_types::SpaceRole;
use ui_shell::MainWindow;

pub fn handle_member_online(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    member: &shared_types::MemberInfo,
) {
    log::info!("Member online: {} ({})", member.name, member.id);
    let mut s = state.borrow_mut();
    if let Some(ref mut space) = s.space {
        // Remove any existing entry for the same user_id (reconnect with new peer_id)
        if let Some(ref uid) = member.user_id {
            if !uid.is_empty() {
                space
                    .members
                    .retain(|m| m.user_id.as_deref() != Some(uid) || m.id == member.id);
            }
        }
        // Don't add duplicates by peer_id
        if !space.members.iter().any(|m| m.id == member.id) {
            space.members.push(member.clone());
        }
    }
    let favorites_changed = crate::friends::refresh_metadata_in_place(&mut s);
    drop(s);
    crate::friends::sync_ui(w, state);
    if favorites_changed {
        crate::friends::persist(state);
    }
}

pub fn handle_member_offline(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    member_id: &str,
) {
    log::info!("Member offline: {member_id}");
    let mut s = state.borrow_mut();
    if let Some(ref mut space) = s.space {
        space.members.retain(|m| m.id != member_id);
    }
    drop(s);
    crate::friends::sync_ui(w, state);
}

pub fn handle_user_status_changed(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    member_id: &str,
    status: &str,
) {
    let mut s = state.borrow_mut();
    if let Some(ref mut space) = s.space {
        if let Some(member) = space.members.iter_mut().find(|m| m.id == member_id) {
            member.status = status.to_string();
        }
    }
    drop(s);
    crate::friends::sync_ui(w, state);
}

pub fn handle_member_channel_changed(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    member_id: &str,
    channel_id: &Option<String>,
    channel_name: &Option<String>,
) {
    let mut s = state.borrow_mut();
    if let Some(ref mut space) = s.space {
        if let Some(member) = space.members.iter_mut().find(|m| m.id == member_id) {
            member.channel_id = channel_id.clone();
            member.channel_name = channel_name.clone();
        }

        // Update channel peer counts from member data
        for ch in space.channels.iter_mut() {
            ch.peer_count = space
                .members
                .iter()
                .filter(|m| m.channel_id.as_deref() == Some(&ch.id))
                .count() as u32;
        }
    }
    let favorites_changed = crate::friends::refresh_metadata_in_place(&mut s);
    drop(s);
    crate::friends::sync_ui(w, state);
    if favorites_changed {
        crate::friends::persist(state);
    }
}

pub fn handle_member_role_changed(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    user_id: &str,
    role: SpaceRole,
) {
    let mut s = state.borrow_mut();
    let self_user_id = s.self_user_id.clone();
    if let Some(ref mut space) = s.space {
        for member in &mut space.members {
            if member.user_id.as_deref() == Some(user_id) {
                member.role = role;
            }
        }
        if self_user_id.as_deref() == Some(user_id) {
            space.self_role = role;
            crate::signal_handler::apply_space_permissions(w, role);
            w.set_is_space_owner(role == SpaceRole::Owner);
        }
    }
    drop(s);
    crate::friends::sync_ui(w, state);
}

pub fn handle_space_audit_snapshot(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    entries: &[shared_types::SpaceAuditEntry],
) {
    if let Some(ref mut space) = state.borrow_mut().space {
        space.audit_log = entries.to_vec();
    }
    ui_shell::set_space_audit_log(w, entries);
}

pub fn handle_profile_updated(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    user_id: &str,
    bio: &str,
) {
    // Update bio in member list if visible
    if let Some(ref mut space) = state.borrow_mut().space {
        if let Some(member) = space.members.iter_mut().find(|m| {
            m.user_id.as_deref() == Some(user_id) || m.id == user_id
        }) {
            member.bio = bio.to_string();
        }
    }
    let members = state
        .borrow()
        .space
        .as_ref()
        .map(|s| s.members.clone())
        .unwrap_or_default();
    ui_shell::set_members(w, &members);
}

pub fn handle_space_audit_appended(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    entry: &shared_types::SpaceAuditEntry,
) {
    let mut entries = Vec::new();
    if let Some(ref mut space) = state.borrow_mut().space {
        space.audit_log.insert(0, entry.clone());
        if space.audit_log.len() > 64 {
            space.audit_log.truncate(64);
        }
        entries = space.audit_log.clone();
    }
    ui_shell::set_space_audit_log(w, &entries);
}

pub fn handle_role_color_changed(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    role: shared_types::SpaceRole,
    color: &str,
) {
    // Update the role_color for all members with this role
    let mut s = state.borrow_mut();
    if let Some(ref mut space) = s.space {
        for member in &mut space.members {
            if member.role == role {
                member.role_color = color.to_string();
            }
        }
    }
    drop(s);
    crate::friends::sync_ui(w, state);
}

pub fn handle_activity_changed(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    member_id: &str,
    activity: &str,
) {
    let mut s = state.borrow_mut();
    if let Some(ref mut space) = s.space {
        if let Some(member) = space.members.iter_mut().find(|m| m.id == member_id) {
            member.activity = activity.to_string();
        }
    }
    drop(s);
    crate::friends::sync_ui(w, state);
}

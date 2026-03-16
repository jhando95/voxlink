use std::cell::RefCell;
use std::rc::Rc;

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

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
        // Don't add duplicates
        if !space.members.iter().any(|m| m.id == member.id) {
            space.members.push(member.clone());
        }
        ui_shell::set_members(w, &space.members);
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
        ui_shell::set_members(w, &space.members);
    }
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
        ui_shell::set_members(w, &space.members);

        // Update channel peer counts from member data
        for ch in space.channels.iter_mut() {
            ch.peer_count = space
                .members
                .iter()
                .filter(|m| m.channel_id.as_deref() == Some(&ch.id))
                .count() as u32;
        }
        ui_shell::set_channels(w, &space.channels);
    }
}

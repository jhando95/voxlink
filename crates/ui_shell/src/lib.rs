slint::include_modules!();

use shared_types::{AppView, PerfSnapshot};

pub fn view_to_index(view: AppView) -> i32 {
    match view {
        AppView::Home => 0,
        AppView::Room => 1,
        AppView::Settings => 2,
        AppView::Performance => 3,
        AppView::Space => 4,
        AppView::TextChat => 5,
    }
}

pub fn index_to_view(index: i32) -> AppView {
    match index {
        0 => AppView::Home,
        1 => AppView::Room,
        2 => AppView::Settings,
        3 => AppView::Performance,
        4 => AppView::Space,
        5 => AppView::TextChat,
        _ => AppView::Home,
    }
}

pub fn update_perf_display(window: &MainWindow, snap: &PerfSnapshot) {
    let perf = PerfData {
        cpu_percent: snap.cpu_percent,
        memory_mb: snap.memory_mb,
        uptime_secs: snap.uptime_secs as i32,
        audio_active: snap.audio_active,
        network_connected: snap.network_connected,
        dropped_frames: snap.dropped_frames as i32,
    };
    window.set_perf(perf);

    // Format uptime as human-readable
    let secs = snap.uptime_secs;
    let uptime = if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    };
    window.set_uptime_text(uptime.into());
}

pub fn set_participants(window: &MainWindow, participants: &[shared_types::Participant]) {
    // Sort: self always first, then alphabetical by name
    let mut sorted: Vec<&shared_types::Participant> = participants.iter().collect();
    sorted.sort_by(|a, b| {
        if a.id == "self" {
            std::cmp::Ordering::Less
        } else if b.id == "self" {
            std::cmp::Ordering::Greater
        } else {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        }
    });

    let model: Vec<ParticipantData> = sorted
        .iter()
        .map(|p| {
            let initial = p.name.chars().next().unwrap_or('?').to_uppercase().to_string();
            // Stable color from name hash — same person always gets same color
            let color_index = p.name.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32)) % 8;
            ParticipantData {
                id: p.id.clone().into(),
                name: p.name.clone().into(),
                initial: initial.into(),
                is_muted: p.is_muted,
                is_deafened: p.is_deafened,
                is_speaking: p.is_speaking,
                volume: p.volume,
                color_index: color_index as i32,
            }
        })
        .collect();
    let rc = std::rc::Rc::new(slint::VecModel::from(model));
    window.set_participants(rc.into());
}

pub fn set_spaces(window: &MainWindow, spaces: &[shared_types::SpaceInfo]) {
    let model: Vec<SpaceData> = spaces
        .iter()
        .map(|s| {
            let initial = s
                .name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            let color_index = s
                .name
                .bytes()
                .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32))
                % 8;
            SpaceData {
                id: s.id.clone().into(),
                name: s.name.clone().into(),
                initial: initial.into(),
                member_count: s.member_count as i32,
                channel_count: s.channel_count as i32,
                color_index: color_index as i32,
            }
        })
        .collect();
    let rc = std::rc::Rc::new(slint::VecModel::from(model));
    window.set_spaces(rc.into());
}

pub fn set_channels(window: &MainWindow, channels: &[shared_types::ChannelInfo]) {
    let model: Vec<ChannelData> = channels
        .iter()
        .map(|c| ChannelData {
            id: c.id.clone().into(),
            name: c.name.clone().into(),
            peer_count: c.peer_count as i32,
            is_voice: c.channel_type == shared_types::ChannelType::Voice,
        })
        .collect();
    let rc = std::rc::Rc::new(slint::VecModel::from(model));
    window.set_channels(rc.into());
}

pub fn set_members(window: &MainWindow, members: &[shared_types::MemberInfo]) {
    let model: Vec<MemberData> = members
        .iter()
        .map(|m| {
            let initial = m
                .name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            let color_index = m
                .name
                .bytes()
                .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32))
                % 8;
            MemberData {
                id: m.id.clone().into(),
                name: m.name.clone().into(),
                initial: initial.into(),
                channel_name: m.channel_name.clone().unwrap_or_default().into(),
                is_in_voice: m.channel_id.is_some(),
                color_index: color_index as i32,
            }
        })
        .collect();
    let rc = std::rc::Rc::new(slint::VecModel::from(model));
    window.set_members(rc.into());
}

pub fn set_chat_messages(window: &MainWindow, messages: &[shared_types::TextMessageData], self_name: &str) {
    let model: Vec<ChatMessage> = messages
        .iter()
        .map(|m| text_msg_to_chat_msg(m, self_name))
        .collect();
    let rc = std::rc::Rc::new(slint::VecModel::from(model));
    window.set_chat_messages(rc.into());
}

pub fn text_msg_to_chat_msg(m: &shared_types::TextMessageData, self_name: &str) -> ChatMessage {
    let color_index = m.sender_name.bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32)) % 8;
    let reactions_str = format_reactions(&m.reactions);
    ChatMessage {
        sender_name: m.sender_name.clone().into(),
        content: m.content.clone().into(),
        timestamp: format_timestamp(m.timestamp).into(),
        is_self: m.sender_name == self_name,
        color_index: color_index as i32,
        message_id: m.message_id.clone().into(),
        edited: m.edited,
        reactions: reactions_str.into(),
    }
}

pub fn format_reactions(reactions: &[shared_types::ReactionData]) -> String {
    if reactions.is_empty() {
        return String::new();
    }
    reactions.iter()
        .map(|r| format!("{} {}", r.emoji, r.users.len()))
        .collect::<Vec<_>>()
        .join("  ")
}

pub fn format_timestamp(unix_secs: u64) -> String {
    if unix_secs == 0 {
        return String::new();
    }
    // Simple HH:MM format from unix timestamp
    let secs_today = unix_secs % 86400;
    let hours = secs_today / 3600;
    let minutes = (secs_today % 3600) / 60;
    format!("{hours:02}:{minutes:02}")
}

pub fn set_device_lists(window: &MainWindow, inputs: &[String], outputs: &[String]) {
    let input_model: Vec<slint::SharedString> = inputs.iter().map(|s| s.into()).collect();
    let output_model: Vec<slint::SharedString> = outputs.iter().map(|s| s.into()).collect();
    window.set_input_devices(std::rc::Rc::new(slint::VecModel::from(input_model)).into());
    window.set_output_devices(std::rc::Rc::new(slint::VecModel::from(output_model)).into());
}

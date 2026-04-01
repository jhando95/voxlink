slint::include_modules!();

use std::cell::RefCell;
use std::collections::HashSet;

use shared_types::{AppView, PerfSnapshot, SpaceRole};
use slint::ComponentHandle;

thread_local! {
    static MEMBER_WIDGET: RefCell<Option<slint::Weak<MemberWidgetWindow>>> = const { RefCell::new(None) };
    static MEMBER_WIDGET_VISIBLE: RefCell<bool> = const { RefCell::new(false) };
    static MEMBER_WIDGET_INITIALIZER: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

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
        jitter_buffer_ms: snap.jitter_buffer_ms as i32,
        frame_loss_percent: snap.frame_loss_rate * 100.0,
        encode_bitrate_kbps: snap.encode_bitrate_kbps as i32,
        decode_peers: snap.decode_peers as i32,
        udp_active: snap.udp_active,
        ping_ms: snap.ping_ms,
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

pub fn register_member_widget(widget: &MemberWidgetWindow) {
    MEMBER_WIDGET.with(|slot| {
        *slot.borrow_mut() = Some(widget.as_weak());
    });
}

pub fn register_member_widget_initializer(f: impl Fn() + 'static) {
    MEMBER_WIDGET_INITIALIZER.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(f));
    });
}

pub fn ensure_member_widget() -> bool {
    if with_member_widget(|_| ()).is_some() {
        return true;
    }
    MEMBER_WIDGET_INITIALIZER.with(|slot| {
        if let Some(initializer) = slot.borrow().as_ref() {
            initializer();
        }
    });
    with_member_widget(|_| ()).is_some()
}

pub fn set_member_widget_visible(visible: bool) -> bool {
    if visible && !ensure_member_widget() {
        MEMBER_WIDGET_VISIBLE.with(|slot| {
            *slot.borrow_mut() = false;
        });
        return false;
    }

    with_member_widget(|widget| {
        let result = if visible {
            widget.show()
        } else {
            widget.hide()
        };
        if let Err(err) = result {
            log::warn!("Failed to change member widget visibility: {err}");
            return false;
        }
        MEMBER_WIDGET_VISIBLE.with(|slot| {
            *slot.borrow_mut() = visible;
        });
        true
    })
    .unwrap_or_else(|| {
        MEMBER_WIDGET_VISIBLE.with(|slot| {
            *slot.borrow_mut() = false;
        });
        false
    })
}

pub fn sync_member_widget_theme(dark_mode: bool, theme_preset: i32) {
    with_member_widget(|widget| {
        widget.set_dark_mode(dark_mode);
        widget.set_theme_preset(theme_preset);
    });
}

pub fn sync_member_widget(
    space: Option<&shared_types::SpaceState>,
    favorites: &[shared_types::FavoriteFriend],
) {
    let is_visible = MEMBER_WIDGET_VISIBLE.with(|slot| *slot.borrow());
    if !is_visible {
        return;
    }
    with_member_widget(|widget| match space {
        Some(space) => {
            let friends = member_widget_entries(Some(space), favorites);
            let (member_count, voice_count) = if favorites.is_empty() {
                (
                    space.members.len() as i32,
                    space
                        .members
                        .iter()
                        .filter(|member| member.channel_id.is_some())
                        .count() as i32,
                )
            } else {
                (
                    favorites.iter().filter(|friend| friend.is_online).count() as i32,
                    favorites
                        .iter()
                        .filter(|friend| friend.is_in_voice || friend.in_private_call)
                        .count() as i32,
                )
            };
            widget.set_space_name(if favorites.is_empty() {
                space.name.clone().into()
            } else {
                slint::SharedString::default()
            });
            widget.set_favorite_count(favorites.len() as i32);
            widget.set_status_text(
                if favorites.is_empty() && member_count == 0 {
                    "Nobody is online in this space yet. Add people as friends to keep them here.".into()
                } else if favorites.is_empty() {
                    "Active space members appear here. Add people as friends to keep them between sessions."
                        .into()
                } else if member_count == 0 {
                    "Friends stay pinned here. They will light up anywhere on this server as soon as they come online."
                        .into()
                } else {
                    "Friends stay pinned anywhere on this server. Live status updates without joining their space."
                        .into()
                },
            );
            widget.set_member_count(member_count);
            widget.set_voice_count(voice_count);
            widget.set_friends(std::rc::Rc::new(slint::VecModel::from(friends)).into());
        }
        None => {
            widget.set_space_name(slint::SharedString::default());
            widget.set_status_text(
                if favorites.is_empty() {
                    "Join a space to add people here.".into()
                } else {
                    "Friends stay here between spaces. Live status updates as they move around the server."
                        .into()
                },
            );
            widget.set_member_count(favorites.iter().filter(|friend| friend.is_online).count() as i32);
            widget.set_voice_count(
                favorites
                    .iter()
                    .filter(|friend| friend.is_in_voice || friend.in_private_call)
                    .count() as i32,
            );
            widget.set_favorite_count(favorites.len() as i32);
            widget.set_friends(
                std::rc::Rc::new(slint::VecModel::from(member_widget_entries(
                    None, favorites,
                )))
                .into(),
            );
        }
    });
}

pub fn set_friend_counts(window: &MainWindow, friends: &[shared_types::FavoriteFriend]) {
    window.set_favorite_friends_count(friends.len() as i32);
    window
        .set_online_friends_count(friends.iter().filter(|friend| friend.is_online).count() as i32);
    window.set_live_friends_count(
        friends
            .iter()
            .filter(|friend| friend.is_in_voice || friend.in_private_call)
            .count() as i32,
    );
}

pub fn set_friend_list(window: &MainWindow, friends: &[shared_types::FavoriteFriend]) {
    let model = friends
        .iter()
        .map(friend_data_from_favorite)
        .collect::<Vec<_>>();
    window.set_favorite_friends(std::rc::Rc::new(slint::VecModel::from(model)).into());
}

pub fn set_direct_message_threads(
    window: &MainWindow,
    threads: &[shared_types::DirectMessageThread],
) {
    let model = threads
        .iter()
        .map(|thread| DirectMessageThreadData {
            user_id: thread.user_id.clone().into(),
            name: thread.user_name.clone().into(),
            initial: member_initial(&thread.user_name),
            preview: if thread.last_message_preview.is_empty() {
                "No messages yet".into()
            } else {
                thread.last_message_preview.clone().into()
            },
            meta: direct_message_meta(thread).into(),
            unread_count: thread.unread_count as i32,
            is_online: thread.is_online,
            is_in_voice: thread.is_in_voice,
            color_index: member_color_index(&thread.user_name),
        })
        .collect::<Vec<_>>();
    window.set_direct_message_threads(std::rc::Rc::new(slint::VecModel::from(model)).into());
}

pub fn set_friend_requests(
    window: &MainWindow,
    incoming: &[shared_types::FriendRequest],
    outgoing: &[shared_types::FriendRequest],
) {
    let incoming_model = incoming
        .iter()
        .map(|request| friend_request_data(request, true))
        .collect::<Vec<_>>();
    let outgoing_model = outgoing
        .iter()
        .map(|request| friend_request_data(request, false))
        .collect::<Vec<_>>();
    window.set_incoming_friend_requests(
        std::rc::Rc::new(slint::VecModel::from(incoming_model)).into(),
    );
    window.set_outgoing_friend_requests(
        std::rc::Rc::new(slint::VecModel::from(outgoing_model)).into(),
    );
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
            let initial = p
                .name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            // Stable color from name hash — same person always gets same color
            let color_index = p
                .name
                .bytes()
                .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32))
                % 8;
            ParticipantData {
                id: p.id.clone().into(),
                name: p.name.clone().into(),
                initial: initial.into(),
                is_muted: p.is_muted,
                is_deafened: p.is_deafened,
                is_speaking: p.is_speaking,
                volume: p.volume,
                color_index: color_index as i32,
                is_priority_speaker: false,
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
                has_unread: false,
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
            is_active: false,
            unread_count: 0,
            topic: c.topic.clone().into(),
            voice_quality: c.voice_quality as i32,
            category: c.category.clone().into(),
            status: c.status.clone().into(),
            user_limit: c.user_limit as i32,
            slow_mode_secs: c.slow_mode_secs as i32,
            is_category_header: false,
            category_collapsed: false,
            mention_count: 0,
        })
        .collect();
    let rc = std::rc::Rc::new(slint::VecModel::from(model));
    window.set_channels(rc.into());
}

pub fn render_space(
    window: &MainWindow,
    space: &shared_types::SpaceState,
    search_query: &str,
    favorites: &[shared_types::FavoriteFriend],
    incoming_requests: &[shared_types::FriendRequest],
    outgoing_requests: &[shared_types::FriendRequest],
    self_user_id: Option<&str>,
    collapsed_categories: &[String],
) {
    let query = search_query.trim().to_lowercase();
    let mut visible_text_channels = 0i32;
    let mut visible_voice_channels = 0i32;
    let favorite_ids: HashSet<&str> = favorites
        .iter()
        .map(|friend| friend.user_id.as_str())
        .collect();
    let incoming_ids: HashSet<&str> = incoming_requests
        .iter()
        .map(|request| request.user_id.as_str())
        .collect();
    let outgoing_ids: HashSet<&str> = outgoing_requests
        .iter()
        .map(|request| request.user_id.as_str())
        .collect();

    let collapsed = collapsed_categories;
    let raw_channels: Vec<ChannelData> = space
        .channels
        .iter()
        .filter(|channel| query.is_empty() || channel.name.to_lowercase().contains(&query))
        .map(|channel| {
            if channel.channel_type == shared_types::ChannelType::Voice {
                visible_voice_channels += 1;
            } else {
                visible_text_channels += 1;
            }
            let cat = channel.category.clone();
            let is_collapsed = !cat.is_empty() && collapsed.contains(&cat);

            ChannelData {
                id: channel.id.clone().into(),
                name: channel.name.clone().into(),
                peer_count: channel.peer_count as i32,
                is_voice: channel.channel_type == shared_types::ChannelType::Voice,
                is_active: space.active_channel_id.as_deref() == Some(channel.id.as_str())
                    || space.selected_text_channel_id.as_deref() == Some(channel.id.as_str()),
                unread_count: space
                    .unread_text_channels
                    .get(&channel.id)
                    .copied()
                    .unwrap_or(0) as i32,
                topic: channel.topic.clone().into(),
                voice_quality: channel.voice_quality as i32,
                category: cat.into(),
                status: channel.status.clone().into(),
                user_limit: channel.user_limit as i32,
                slow_mode_secs: channel.slow_mode_secs as i32,
                is_category_header: false,
                category_collapsed: is_collapsed,
                mention_count: 0,
            }
        })
        .collect();

    // Insert category headers before each group of channels with the same category
    let mut channels: Vec<ChannelData> = Vec::with_capacity(raw_channels.len() + 8);
    let mut last_category = String::new();
    for ch in raw_channels {
        let cat = ch.category.to_string();
        if !cat.is_empty() && cat != last_category {
            let is_collapsed = collapsed.contains(&cat);
            channels.push(ChannelData {
                id: Default::default(),
                name: cat.to_uppercase().into(),
                category: cat.clone().into(),
                is_category_header: true,
                category_collapsed: is_collapsed,
                ..Default::default()
            });
            last_category = cat;
        }
        channels.push(ch);
    }

    let mut visible_members: Vec<&shared_types::MemberInfo> = space
        .members
        .iter()
        .filter(|member| {
            query.is_empty()
                || member.name.to_lowercase().contains(&query)
                || member
                    .channel_name
                    .as_deref()
                    .map(|channel_name| channel_name.to_lowercase().contains(&query))
                    .unwrap_or(false)
        })
        .collect();
    visible_members.sort_by(|left, right| {
        favorite_ids
            .contains(stable_member_key(right))
            .cmp(&favorite_ids.contains(stable_member_key(left)))
            .then_with(|| member_role_tier(right.role).cmp(&member_role_tier(left.role)))
            .then_with(|| right.channel_id.is_some().cmp(&left.channel_id.is_some()))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    let members: Vec<MemberData> = visible_members
        .into_iter()
        .map(|member| {
            let stable_id = stable_member_key(member);
            member_data_from_info(
                member,
                favorite_ids.contains(stable_id),
                incoming_ids.contains(stable_id),
                outgoing_ids.contains(stable_id),
                self_user_id == Some(stable_id),
            )
        })
        .collect();

    let visible_member_count = members.len() as i32;
    window.set_channels(std::rc::Rc::new(slint::VecModel::from(channels)).into());
    window.set_members(std::rc::Rc::new(slint::VecModel::from(members)).into());
    window.set_visible_text_channels(visible_text_channels);
    window.set_visible_voice_channels(visible_voice_channels);
    window.set_visible_members(visible_member_count);
}

pub fn set_members(window: &MainWindow, members: &[shared_types::MemberInfo]) {
    let model: Vec<MemberData> = members
        .iter()
        .map(|member| member_data_from_info(member, false, false, false, false))
        .collect();
    let rc = std::rc::Rc::new(slint::VecModel::from(model));
    window.set_members(rc.into());
}

pub fn set_space_audit_log(window: &MainWindow, entries: &[shared_types::SpaceAuditEntry]) {
    let model = entries
        .iter()
        .map(|entry| AuditEntryData {
            actor_name: entry.actor_name.clone().into(),
            action: entry.action.to_uppercase().into(),
            target_name: entry.target_name.clone().into(),
            detail: entry.detail.clone().into(),
            timestamp: audit_entry_time(entry.timestamp).into(),
        })
        .collect::<Vec<_>>();
    window.set_space_audit_entries(std::rc::Rc::new(slint::VecModel::from(model)).into());
}

pub fn set_chat_messages(
    window: &MainWindow,
    messages: &[shared_types::TextMessageData],
    self_name: &str,
) {
    let model: Vec<ChatMessage> = messages
        .iter()
        .map(|m| text_msg_to_chat_msg(m, self_name))
        .collect();
    let rc = std::rc::Rc::new(slint::VecModel::from(model));
    window.set_chat_messages(rc.into());
}

fn with_member_widget<T>(f: impl FnOnce(MemberWidgetWindow) -> T) -> Option<T> {
    MEMBER_WIDGET.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|widget| widget.upgrade())
            .map(f)
    })
}

fn member_data_from_info(
    member: &shared_types::MemberInfo,
    is_friend: bool,
    has_incoming_request: bool,
    has_outgoing_request: bool,
    is_self: bool,
) -> MemberData {
    MemberData {
        id: member.id.clone().into(),
        user_id: stable_member_id(member).into(),
        name: member.name.clone().into(),
        initial: member_initial(&member.name),
        role_label: member_role_label(member.role).into(),
        role_tier: member_role_tier(member.role),
        channel_name: member.channel_name.clone().unwrap_or_default().into(),
        status: member.status.clone().into(),
        is_online: true,
        is_in_voice: member.channel_id.is_some(),
        color_index: member_color_index(&member.name),
        status_level: match member.status_preset {
            shared_types::UserStatus::Online => 1,
            shared_types::UserStatus::Idle => 2,
            shared_types::UserStatus::DoNotDisturb => 3,
            shared_types::UserStatus::Invisible => 0,
        },
        is_server_muted: false,
        is_friend,
        has_incoming_request,
        has_outgoing_request,
        is_self,
    }
}

fn member_role_label(role: SpaceRole) -> &'static str {
    match role {
        SpaceRole::Owner => "Owner",
        SpaceRole::Admin => "Admin",
        SpaceRole::Moderator => "Mod",
        SpaceRole::Member => "Member",
    }
}

fn member_role_tier(role: SpaceRole) -> i32 {
    match role {
        SpaceRole::Member => 0,
        SpaceRole::Moderator => 1,
        SpaceRole::Admin => 2,
        SpaceRole::Owner => 3,
    }
}

fn audit_entry_time(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let delta = now.saturating_sub(timestamp);
    if delta < 60 {
        format!("{delta}s ago")
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86_400)
    }
}

fn member_widget_entries(
    space: Option<&shared_types::SpaceState>,
    favorites: &[shared_types::FavoriteFriend],
) -> Vec<FriendData> {
    if !favorites.is_empty() {
        let mut ordered = favorites.iter().collect::<Vec<_>>();
        ordered.sort_by(|left, right| {
            right
                .is_online
                .cmp(&left.is_online)
                .then_with(|| {
                    (right.is_in_voice || right.in_private_call)
                        .cmp(&(left.is_in_voice || left.in_private_call))
                })
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        return ordered.into_iter().map(friend_data_from_favorite).collect();
    }

    let Some(space) = space else {
        return Vec::new();
    };

    let mut live_members: Vec<&shared_types::MemberInfo> = space.members.iter().collect();
    live_members.sort_by(|left, right| {
        right
            .channel_id
            .is_some()
            .cmp(&left.channel_id.is_some())
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });

    live_members
        .into_iter()
        .map(|member| FriendData {
            user_id: stable_member_id(member).into(),
            name: member.name.clone().into(),
            initial: member_initial(&member.name),
            detail: member
                .channel_name
                .clone()
                .unwrap_or_else(|| format!("Online in {}", space.name))
                .into(),
            status_label: if member.channel_id.is_some() {
                "Voice".into()
            } else {
                "Online".into()
            },
            is_online: true,
            is_in_voice: member.channel_id.is_some(),
            color_index: member_color_index(&member.name),
            is_friend: false,
        })
        .collect()
}

fn member_initial(name: &str) -> slint::SharedString {
    name.chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string()
        .into()
}

fn member_color_index(name: &str) -> i32 {
    (name.bytes().fold(0u32, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as u32)
    }) % 8) as i32
}

fn stable_member_id(member: &shared_types::MemberInfo) -> String {
    stable_member_key(member).to_string()
}

fn stable_member_key(member: &shared_types::MemberInfo) -> &str {
    member
        .user_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .unwrap_or(member.id.as_str())
}

fn offline_friend_detail(friend: &shared_types::FavoriteFriend) -> String {
    if !friend.last_channel_name.is_empty() {
        if !friend.last_space_name.is_empty() {
            return format!(
                "Last seen in {} / {}",
                friend.last_space_name, friend.last_channel_name
            );
        }
        return format!("Last seen in {}", friend.last_channel_name);
    }
    if !friend.last_space_name.is_empty() {
        return format!("Last seen in {}", friend.last_space_name);
    }
    "Waiting for live presence".into()
}

fn friend_data_from_favorite(friend: &shared_types::FavoriteFriend) -> FriendData {
    let (detail, status_label, is_in_voice) = if friend.is_online {
        if !friend.active_channel_name.is_empty() && !friend.active_space_name.is_empty() {
            (
                format!(
                    "{} / {}",
                    friend.active_space_name, friend.active_channel_name
                ),
                if friend.is_in_voice {
                    "Voice"
                } else {
                    "Online"
                },
                friend.is_in_voice,
            )
        } else if !friend.active_space_name.is_empty() {
            (
                format!("Online in {}", friend.active_space_name),
                if friend.is_in_voice {
                    "Voice"
                } else {
                    "Online"
                },
                friend.is_in_voice,
            )
        } else if friend.in_private_call {
            ("In a private call".into(), "Call", true)
        } else {
            ("Online on this server".into(), "Online", false)
        }
    } else {
        (offline_friend_detail(friend), "Offline", false)
    };

    FriendData {
        user_id: friend.user_id.clone().into(),
        name: friend.name.clone().into(),
        initial: member_initial(&friend.name),
        detail: detail.into(),
        status_label: status_label.into(),
        is_online: friend.is_online,
        is_in_voice,
        color_index: member_color_index(&friend.name),
        is_friend: true,
    }
}

fn direct_message_meta(thread: &shared_types::DirectMessageThread) -> String {
    let presence = if thread.is_in_voice {
        "In voice"
    } else if thread.is_online {
        "Online"
    } else {
        "Offline"
    };

    if thread.last_message_at == 0 {
        presence.into()
    } else {
        format!("{presence} · {}", format_timestamp(thread.last_message_at))
    }
}

fn friend_request_data(request: &shared_types::FriendRequest, incoming: bool) -> FriendRequestData {
    let detail = if request.requested_at == 0 {
        if incoming {
            "Wants to connect".to_string()
        } else {
            "Waiting for reply".to_string()
        }
    } else if incoming {
        format!("Requested at {}", format_timestamp(request.requested_at))
    } else {
        format!("Sent at {}", format_timestamp(request.requested_at))
    };

    FriendRequestData {
        user_id: request.user_id.clone().into(),
        name: request.name.clone().into(),
        initial: member_initial(&request.name),
        detail: detail.into(),
        color_index: member_color_index(&request.name),
    }
}

fn format_file_size(bytes: u32) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

pub fn text_msg_to_chat_msg(m: &shared_types::TextMessageData, self_name: &str) -> ChatMessage {
    let color_index = m
        .sender_name
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32))
        % 8;
    let reactions_str = format_reactions(&m.reactions);
    let sender_initial = m
        .sender_name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let (content, is_code_block) = render_markdown(&m.content);
    ChatMessage {
        sender_name: m.sender_name.clone().into(),
        sender_initial: sender_initial.into(),
        content: content.into(),
        timestamp: format_timestamp(m.timestamp).into(),
        is_self: m.sender_name == self_name,
        mentions_self: message_mentions_user(&m.content, self_name),
        reply_sender_name: m.reply_to_sender_name.clone().unwrap_or_default().into(),
        reply_preview: m.reply_preview.clone().unwrap_or_default().into(),
        is_pinned: m.pinned,
        color_index: color_index as i32,
        message_id: m.message_id.clone().into(),
        edited: m.edited,
        reactions: reactions_str.into(),
        is_code_block,
        forwarded_from: m.forwarded_from.clone().unwrap_or_default().into(),
        attachment_name: m.attachment_name.clone().unwrap_or_default().into(),
        attachment_size: m.attachment_size.map(format_file_size).unwrap_or_default().into(),
        channel_name: Default::default(),
    }
}

/// Basic markdown rendering for Slint (which only supports single-style Text).
/// Returns (rendered_content, is_code_block).
/// - Triple-backtick blocks → strip markers, flag as code block
/// - Inline `code` → keep backticks (they serve as visual delimiters)
/// - **bold** → strip markers, uppercase the text (visual emphasis substitute)
/// - *italic* → strip markers (no italic in monospace anyway)
/// - ~~strikethrough~~ → strip markers
/// - > blockquote → prefix with "│ "
pub fn render_markdown(content: &str) -> (String, bool) {
    let trimmed = content.trim();

    // Full code block: ```...```
    if trimmed.starts_with("```") && trimmed.ends_with("```") && trimmed.len() > 6 {
        let inner = &trimmed[3..trimmed.len() - 3];
        // Strip optional language tag on first line
        let code = if let Some(newline_pos) = inner.find('\n') {
            let first_line = &inner[..newline_pos];
            let after_tag = &inner[newline_pos + 1..];
            if first_line.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                && !after_tag.trim().is_empty()
            {
                after_tag.trim_end().to_string()
            } else {
                inner.trim().to_string()
            }
        } else {
            inner.trim().to_string()
        };
        return (code, true);
    }

    let mut result = String::with_capacity(content.len());
    for line in content.lines() {
        if !result.is_empty() {
            result.push('\n');
        }
        // Blockquote
        if let Some(quoted) = line.strip_prefix("> ") {
            result.push_str("\u{2502} ");
            result.push_str(&render_inline(quoted));
        } else if line == ">" {
            result.push('\u{2502}');
        } else {
            result.push_str(&render_inline(line));
        }
    }
    (result, false)
}

fn render_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // **bold** → UPPERCASE
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(end) = find_closing(&chars, i + 2, &['*', '*']) {
                let inner: String = chars[i + 2..end].iter().collect();
                out.push_str(&inner.to_uppercase());
                i = end + 2;
                continue;
            }
        }
        // ~~strikethrough~~ → strip markers
        if i + 1 < len && chars[i] == '~' && chars[i + 1] == '~' {
            if let Some(end) = find_closing(&chars, i + 2, &['~', '~']) {
                let inner: String = chars[i + 2..end].iter().collect();
                out.push_str(&inner);
                i = end + 2;
                continue;
            }
        }
        // ||spoiler|| → [SPOILER] or revealed text
        if i + 1 < len && chars[i] == '|' && chars[i + 1] == '|' {
            if let Some(end) = find_closing(&chars, i + 2, &['|', '|']) {
                let inner: String = chars[i + 2..end].iter().collect();
                // Always reveal spoiler text (client config controls this at a higher level)
                out.push_str(&inner);
                i = end + 2;
                continue;
            }
        }
        // *italic* → strip markers (but not **)
        if chars[i] == '*' && (i + 1 >= len || chars[i + 1] != '*') {
            if let Some(end) = find_closing_single(&chars, i + 1, '*') {
                let inner: String = chars[i + 1..end].iter().collect();
                out.push_str(&inner);
                i = end + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn find_closing(chars: &[char], start: usize, marker: &[char; 2]) -> Option<usize> {
    let len = chars.len();
    let mut i = start;
    while i + 1 < len {
        if chars[i] == marker[0] && chars[i + 1] == marker[1] {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_closing_single(chars: &[char], start: usize, marker: char) -> Option<usize> {
    chars[start..].iter().position(|&c| c == marker).map(|pos| start + pos)
}

fn message_mentions_user(content: &str, user_name: &str) -> bool {
    let trimmed_name = user_name.trim();
    if trimmed_name.is_empty() {
        return false;
    }
    content
        .to_lowercase()
        .contains(&format!("@{}", trimmed_name.to_lowercase()))
}

pub fn format_reactions(reactions: &[shared_types::ReactionData]) -> String {
    if reactions.is_empty() {
        return String::new();
    }
    reactions
        .iter()
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

/// Set soundboard clips in the UI. Each tuple is (name, path, keybind).
pub fn set_soundboard_clips(window: &MainWindow, clips: &[(String, String, String)]) {
    let model: Vec<SoundboardClipData> = clips
        .iter()
        .map(|(name, path, keybind)| SoundboardClipData {
            name: name.into(),
            path: path.into(),
            keybind: keybind.into(),
        })
        .collect();
    window.set_soundboard_clips(std::rc::Rc::new(slint::VecModel::from(model)).into());
}

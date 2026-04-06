slint::include_modules!();

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use shared_types::{AppView, PerfSnapshot, SpaceRole};
use slint::{ComponentHandle, Model};

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
    // Sort: online first, then in-voice, then alphabetical
    let mut sorted: Vec<&shared_types::FavoriteFriend> = friends.iter().collect();
    sorted.sort_by(|a, b| {
        b.is_online
            .cmp(&a.is_online)
            .then_with(|| {
                (b.is_in_voice || b.in_private_call)
                    .cmp(&(a.is_in_voice || a.in_private_call))
            })
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let model = sorted
        .iter()
        .map(|f| friend_data_from_favorite(f))
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
                is_priority_speaker: p.is_priority_speaker,
                audio_level: p.audio_level,
                eq_bass: p.eq_bass,
                eq_mid: p.eq_mid,
                eq_treble: p.eq_treble,
                pan: p.pan,
            }
        })
        .collect();
    let rc = std::rc::Rc::new(slint::VecModel::from(model));
    window.set_participants(rc.into());
}

/// Update only audio levels and speaking state on existing participant rows.
/// This avoids rebuilding the entire VecModel on every tick — only rows with
/// changed data get a `set_row_data()` call. Returns `false` if the model
/// row count doesn't match (caller should fall back to full `set_participants`).
pub fn update_participant_levels(
    window: &MainWindow,
    participants: &[shared_types::Participant],
) -> bool {
    let model = window.get_participants();
    // Sort the same way as set_participants so indices align
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

    if model.row_count() != sorted.len() {
        return false;
    }

    for (i, p) in sorted.iter().enumerate() {
        if let Some(existing) = model.row_data(i) {
            // Only issue set_row_data when something actually changed
            if existing.audio_level != p.audio_level
                || existing.is_speaking != p.is_speaking
                || existing.is_muted != p.is_muted
                || existing.is_deafened != p.is_deafened
            {
                let mut updated = existing;
                updated.audio_level = p.audio_level;
                updated.is_speaking = p.is_speaking;
                updated.is_muted = p.is_muted;
                updated.is_deafened = p.is_deafened;
                model.set_row_data(i, updated);
            }
        }
    }
    true
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
            auto_delete_hours: c.auto_delete_hours as i32,
            ..Default::default()
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
    user_notes: &std::collections::HashMap<String, String>,
    channel_notification_overrides: &std::collections::HashMap<String, String>,
    favorite_channels: &[String],
) {
    let query = search_query.trim().to_lowercase();
    let mut visible_text_channels = 0i32;
    let mut visible_voice_channels = 0i32;
    let mut visible_favorite_channels = 0i32;
    let fav_channel_ids: HashSet<&str> = favorite_channels.iter().map(|s| s.as_str()).collect();
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
            let is_fav = fav_channel_ids.contains(channel.id.as_str());
            if is_fav {
                visible_favorite_channels += 1;
            }

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
                auto_delete_hours: channel.auto_delete_hours as i32,
                is_favorite: is_fav,
                notification_setting: channel_notification_overrides
                    .get(&channel.id)
                    .cloned()
                    .unwrap_or_default()
                    .into(),
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
        // Favorites first, then in-voice, then by role, then alphabetical
        favorite_ids
            .contains(stable_member_key(right))
            .cmp(&favorite_ids.contains(stable_member_key(left)))
            .then_with(|| right.channel_id.is_some().cmp(&left.channel_id.is_some()))
            .then_with(|| member_role_tier(right.role).cmp(&member_role_tier(left.role)))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    let members: Vec<MemberData> = visible_members
        .into_iter()
        .map(|member| {
            let stable_id = stable_member_key(member);
            let mut md = member_data_from_info(
                member,
                favorite_ids.contains(stable_id),
                incoming_ids.contains(stable_id),
                outgoing_ids.contains(stable_id),
                self_user_id == Some(stable_id),
            );
            if let Some(note) = user_notes.get(stable_id) {
                md.user_note = note.clone().into();
            }
            md
        })
        .collect();

    let visible_member_count = members.len() as i32;
    window.set_channels(std::rc::Rc::new(slint::VecModel::from(channels)).into());
    window.set_members(std::rc::Rc::new(slint::VecModel::from(members)).into());
    window.set_visible_text_channels(visible_text_channels);
    window.set_visible_voice_channels(visible_voice_channels);
    window.set_visible_favorite_channels(visible_favorite_channels);
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
    set_chat_messages_with_last_read(window, messages, self_name, None);
}

/// Set chat messages with an optional last-read message ID.
/// If provided, the first message after that ID gets a "NEW" separator.
pub fn set_chat_messages_with_last_read(
    window: &MainWindow,
    messages: &[shared_types::TextMessageData],
    self_name: &str,
    last_read_message_id: Option<&str>,
) {
    // Count replies per parent message
    let mut reply_counts: HashMap<&str, i32> = HashMap::new();
    for m in messages {
        if let Some(ref parent_id) = m.reply_to_message_id {
            *reply_counts.entry(parent_id.as_str()).or_default() += 1;
        }
    }

    // Find the index of the first unread message (the one right after last_read_message_id)
    let new_separator_idx: Option<usize> = last_read_message_id.and_then(|last_read| {
        messages
            .iter()
            .position(|m| m.message_id == last_read)
            .and_then(|pos| {
                // The separator goes on the next message (first unread)
                let next = pos + 1;
                if next < messages.len() {
                    Some(next)
                } else {
                    None // all messages are read
                }
            })
    });

    let mut model: Vec<ChatMessage> = Vec::with_capacity(messages.len());
    let mut prev_sender: Option<&str> = None;
    let mut prev_timestamp: u64 = 0;
    let mut prev_day: u64 = 0;

    for (idx, m) in messages.iter().enumerate() {
        let day = m.timestamp / 86400;
        // Insert date separator at day boundaries
        if day != prev_day && m.timestamp > 0 {
            if prev_day > 0 {
                let sep_text = format_day_separator(m.timestamp);
                let mut sep = ChatMessage::default();
                sep.date_separator = sep_text.into();
                model.push(sep);
            }
            prev_day = day;
        }

        let mut msg = text_msg_to_chat_msg(m, self_name);
        msg.reply_count = reply_counts.get(m.message_id.as_str()).copied().unwrap_or(0);

        // Mark the first unread message with the NEW separator
        if new_separator_idx == Some(idx) {
            msg.is_new_separator = true;
        }

        // Group consecutive messages from same sender within 5 minutes
        let same_sender = prev_sender == Some(m.sender_name.as_str());
        let within_window = m.timestamp.saturating_sub(prev_timestamp) < 300;
        msg.show_header = !same_sender || !within_window;

        prev_sender = Some(m.sender_name.as_str());
        prev_timestamp = m.timestamp;
        model.push(msg);
    }

    let rc = std::rc::Rc::new(slint::VecModel::from(model));
    window.set_chat_messages(rc.into());
}

fn format_day_separator(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let today = now / 86400;
    let msg_day = timestamp / 86400;
    let diff = today.saturating_sub(msg_day);

    if diff == 0 {
        "Today".into()
    } else if diff == 1 {
        "Yesterday".into()
    } else if diff < 7 {
        format!("{diff} days ago")
    } else {
        // Simple month/day from unix timestamp
        let days_since_epoch = msg_day;
        // Approximate: good enough for display
        let year = 1970 + (days_since_epoch * 4 / 1461) as u32; // ~365.25 days/year
        let day_of_year = (days_since_epoch - ((year as u64 - 1970) * 1461 / 4)) as u32;
        let months = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut month = 0;
        let mut remaining = day_of_year;
        for (i, &m) in months.iter().enumerate() {
            if remaining < m {
                month = i;
                break;
            }
            remaining -= m;
        }
        let month_names = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun",
            "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        format!("{} {}", month_names[month], remaining + 1)
    }
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
        bio: member.bio.clone().into(),
        nickname: member.nickname.clone().unwrap_or_default().into(),
        user_note: Default::default(),
        role_color_index: hex_color_to_index(&member.role_color),
        activity: member.activity.clone().into(),
    }
}

/// Map a hex color string from the server to a role color index for the UI.
/// Returns 0 for default/unknown, 1-8 for known palette colors.
fn hex_color_to_index(hex: &str) -> i32 {
    match hex.to_lowercase().as_str() {
        "#ff5555" => 1, // red
        "#5599ff" => 2, // blue
        "#55ff55" => 3, // green
        "#ffd700" => 4, // gold
        "#b366ff" => 5, // purple
        "#ff9944" => 6, // orange
        "#ff77aa" => 7, // pink
        "#44dddd" => 8, // cyan
        _ => 0,         // default
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
            last_seen: Default::default(),
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

/// Format a unix timestamp into a human-readable relative time string.
/// Returns empty string if the timestamp is 0.
pub fn format_relative_time(unix_secs: u64) -> String {
    if unix_secs == 0 {
        return String::new();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let delta = now.saturating_sub(unix_secs);
    if delta < 60 {
        "Just now".into()
    } else if delta < 3600 {
        let mins = delta / 60;
        if mins == 1 {
            "1 min ago".into()
        } else {
            format!("{mins} min ago")
        }
    } else if delta < 86_400 {
        let hours = delta / 3600;
        if hours == 1 {
            "1 hour ago".into()
        } else {
            format!("{hours} hours ago")
        }
    } else if delta < 604_800 {
        let days = delta / 86_400;
        if days == 1 {
            "Yesterday".into()
        } else {
            format!("{days} days ago")
        }
    } else {
        let weeks = delta / 604_800;
        if weeks == 1 {
            "1 week ago".into()
        } else {
            format!("{weeks} weeks ago")
        }
    }
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

    let last_seen = if !friend.is_online && friend.last_seen_at > 0 {
        format!("Last seen {}", format_relative_time(friend.last_seen_at))
    } else {
        String::new()
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
        last_seen: last_seen.into(),
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
        sender_id: m.sender_id.clone().into(),
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
        show_header: true,
        date_separator: Default::default(),
        reply_count: 0,
        link_url: m.link_url.clone().unwrap_or_default().into(),
        is_new_separator: false,
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
        // @everyone / @here → UPPERCASE for visual emphasis
        if chars[i] == '@' {
            let rest: String = chars[i..].iter().collect();
            if rest.starts_with("@everyone") {
                out.push_str("@EVERYONE");
                i += "@everyone".len();
                continue;
            }
            if rest.starts_with("@here") {
                out.push_str("@HERE");
                i += "@here".len();
                continue;
            }
        }
        // URL detection — wrap http:// and https:// URLs in angle brackets
        if chars[i] == 'h' {
            let rest: String = chars[i..].iter().collect();
            if rest.starts_with("https://") || rest.starts_with("http://") {
                // Find end of URL (space, newline, or end of string)
                let url_end = rest
                    .find(|c: char| c.is_whitespace())
                    .unwrap_or(rest.len());
                let url = &rest[..url_end];
                out.push_str("\u{1f517} <");
                out.push_str(url);
                out.push('>');
                i += url_end;
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

pub fn message_mentions_user(content: &str, user_name: &str) -> bool {
    let lower = content.to_lowercase();
    if lower.contains("@everyone") || lower.contains("@here") {
        return true;
    }
    let trimmed_name = user_name.trim();
    if trimmed_name.is_empty() {
        return false;
    }
    lower.contains(&format!("@{}", trimmed_name.to_lowercase()))
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

/// Format an epoch timestamp as a human-readable date+time string for scheduled events.
fn format_event_time(epoch: i64) -> String {
    if epoch <= 0 {
        return String::new();
    }
    let epoch = epoch as u64;
    // Days since epoch → approximate date (good enough for display)
    let secs_in_day: u64 = 86400;
    let total_days = epoch / secs_in_day;
    // Gregorian calendar from days since 1970-01-01
    let (year, month, day) = {
        // Algorithm from http://howardhinnant.github.io/date_algorithms.html
        let z = total_days + 719468;
        let era = z / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        (y, m, d)
    };
    let secs_today = epoch % secs_in_day;
    let hours = secs_today / 3600;
    let minutes = (secs_today % 3600) / 60;
    format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}")
}

/// Convert a shared_types::ScheduledEvent to the Slint ScheduledEventData struct.
pub fn scheduled_event_to_data(event: &shared_types::ScheduledEvent) -> ScheduledEventData {
    ScheduledEventData {
        id: event.id.as_str().into(),
        title: event.title.as_str().into(),
        description: event.description.as_str().into(),
        start_time: format_event_time(event.start_time).into(),
        creator_name: event.creator_name.as_str().into(),
        interested_count: event.interested_count as i32,
        is_interested: event.is_interested,
    }
}

/// Set the full list of scheduled events on the window.
pub fn set_scheduled_events(window: &MainWindow, events: &[shared_types::ScheduledEvent]) {
    let model: Vec<ScheduledEventData> = events.iter().map(scheduled_event_to_data).collect();
    window.set_scheduled_events(std::rc::Rc::new(slint::VecModel::from(model)).into());
}

/// Set the full list of automod words on the window.
pub fn set_automod_words(window: &MainWindow, words: &[shared_types::AutomodWord]) {
    let model: Vec<AutomodWordData> = words
        .iter()
        .map(|aw| AutomodWordData {
            word: aw.word.as_str().into(),
            action: aw.action.as_str().into(),
        })
        .collect();
    window.set_automod_words(std::rc::Rc::new(slint::VecModel::from(model)).into());
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

/// Set the recent reactions quick-access list in the emoji picker.
pub fn set_recent_reactions(window: &MainWindow, reactions: &[String]) {
    let model: Vec<slint::SharedString> = reactions.iter().map(|s| s.into()).collect();
    window.set_recent_reactions(std::rc::Rc::new(slint::VecModel::from(model)).into());
}

/// Show a profile popup for a member identified by user_id.
/// Looks up the member in the provided space state and populates the popup fields.
pub fn show_profile_popup(
    window: &MainWindow,
    space: Option<&shared_types::SpaceState>,
    user_id: &str,
) {
    let member = space.and_then(|s| {
        s.members.iter().find(|m| {
            // Match by user_id if available, fall back to peer_id
            (m.user_id.is_some() && m.user_id.as_deref() == Some(user_id))
                || m.id == user_id
        })
    });

    let Some(member) = member else {
        // Could not find the member — nothing to show
        return;
    };

    let initial = member
        .name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let color_index = member_color_index(&member.name);

    let status_text = match member.status_preset {
        shared_types::UserStatus::Online => "Online",
        shared_types::UserStatus::Idle => "Idle",
        shared_types::UserStatus::DoNotDisturb => "Do Not Disturb",
        shared_types::UserStatus::Invisible => "Invisible",
    };

    window.set_profile_popup_user_id(user_id.into());
    window.set_profile_popup_name(member.name.clone().into());
    window.set_profile_popup_initial(initial.into());
    window.set_profile_popup_color_index(color_index);
    window.set_profile_popup_bio(member.bio.clone().into());
    window.set_profile_popup_status(status_text.into());
    window.set_profile_popup_role(member_role_label(member.role).into());
    window.set_profile_popup_role_color(member.role_color.clone().into());
    window.set_profile_popup_role_color_index(hex_color_to_index(&member.role_color));
    window.set_profile_popup_activity(member.activity.clone().into());
    window.set_profile_popup_visible(true);
}

/// Emoji shortcode lookup table: (shortcode, unicode_char).
pub fn emoji_shortcodes() -> &'static [(&'static str, &'static str)] {
    &[
        ("thumbsup", "\u{1F44D}"),
        ("thumbsdown", "\u{1F44E}"),
        ("heart", "\u{2764}\u{FE0F}"),
        ("fire", "\u{1F525}"),
        ("star", "\u{2B50}"),
        ("party", "\u{1F389}"),
        ("laugh", "\u{1F602}"),
        ("cry", "\u{1F622}"),
        ("angry", "\u{1F621}"),
        ("thinking", "\u{1F914}"),
        ("cool", "\u{1F60E}"),
        ("clap", "\u{1F44F}"),
        ("wave", "\u{1F44B}"),
        ("pray", "\u{1F64F}"),
        ("100", "\u{1F4AF}"),
        ("check", "\u{2705}"),
        ("x", "\u{274C}"),
        ("warning", "\u{26A0}\u{FE0F}"),
        ("music", "\u{1F3B5}"),
        ("eyes", "\u{1F440}"),
        ("rocket", "\u{1F680}"),
        ("skull", "\u{1F480}"),
        ("ghost", "\u{1F47B}"),
        ("sparkles", "\u{2728}"),
        ("tada", "\u{1F389}"),
        ("sob", "\u{1F62D}"),
        ("joy", "\u{1F602}"),
        ("wink", "\u{1F609}"),
        ("grin", "\u{1F601}"),
        ("smile", "\u{1F604}"),
        ("sunglasses", "\u{1F60E}"),
        ("ok", "\u{1F44C}"),
        ("muscle", "\u{1F4AA}"),
        ("brain", "\u{1F9E0}"),
        ("crown", "\u{1F451}"),
        ("gem", "\u{1F48E}"),
        ("bulb", "\u{1F4A1}"),
        ("boom", "\u{1F4A5}"),
        ("zzz", "\u{1F4A4}"),
        ("poop", "\u{1F4A9}"),
    ]
}

/// Set the recent-reactions model on the window from a list of emoji strings.
/// Replace `:shortcode:` patterns in text with their unicode emoji.
/// Unknown shortcodes are left as-is.
pub fn resolve_emoji_shortcodes(text: &str) -> String {
    let table = emoji_shortcodes();
    let mut result = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find(':') {
        // Push everything before the colon
        result.push_str(&rest[..start]);

        // Look for closing colon after the opening one
        let after_colon = &rest[start + 1..];
        if let Some(end) = after_colon.find(':') {
            let candidate = &after_colon[..end];
            // Valid shortcode: non-empty, alphanumeric/underscore only
            if !candidate.is_empty()
                && candidate
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_')
            {
                let lower = candidate.to_lowercase();
                if let Some((_, emoji)) = table.iter().find(|(name, _)| *name == lower) {
                    result.push_str(emoji);
                    rest = &after_colon[end + 1..];
                    continue;
                }
            }
            // Not a known shortcode — emit the colon and continue scanning
            result.push(':');
            rest = after_colon;
        } else {
            // No closing colon — emit the rest and stop
            result.push_str(&rest[start..]);
            rest = "";
        }
    }
    result.push_str(rest);
    result
}

use std::fs::{self, File};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Once;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::{ComponentHandle, ModelRc, PhysicalSize, Rgba8Pixel, SharedPixelBuffer, VecModel};
use ui_shell::{
    AuditEntryData, ChannelData, ChatMessage, DirectMessageThreadData, FriendData,
    FriendRequestData, MainWindow, MemberData, PerfData, PublicSpaceData, SavedServerData,
    SoundboardClipData, SpaceData,
};

static INIT_BACKEND: Once = Once::new();

struct SnapshotPlatform;

impl Platform for SnapshotPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer))
    }
}

#[derive(Clone, Copy)]
enum LayoutWidth {
    Narrow,
    Wide,
}

impl LayoutWidth {
    fn label(self) -> &'static str {
        match self {
            Self::Narrow => "narrow",
            Self::Wide => "wide",
        }
    }

    fn size(self, tall: bool) -> PhysicalSize {
        match (self, tall) {
            (Self::Narrow, true) => PhysicalSize::new(460, 1500),
            (Self::Narrow, false) => PhysicalSize::new(460, 1200),
            (Self::Wide, true) => PhysicalSize::new(1440, 1500),
            (Self::Wide, false) => PhysicalSize::new(1440, 1100),
        }
    }
}

#[derive(Clone, Copy)]
enum ShellTheme {
    Dark,
    Light,
}

impl ShellTheme {
    fn label(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    fn dark_mode(self) -> bool {
        matches!(self, Self::Dark)
    }
}

#[derive(Clone, Copy)]
enum UiScenario {
    LoginOverlay,
    Home,
    Settings,
    System,
    Space,
    Chat,
    ChatThread,
    ChatMentionPopup,
    QuickSwitcher,
    IncomingCallOverlay,
    ToastBanner,
    ProfilePopup,
    WelcomeOverlay,
}

impl UiScenario {
    fn label(self) -> &'static str {
        match self {
            Self::LoginOverlay => "login-overlay",
            Self::Home => "home",
            Self::Settings => "settings",
            Self::System => "system",
            Self::Space => "space",
            Self::Chat => "chat",
            Self::ChatThread => "chat-thread",
            Self::ChatMentionPopup => "chat-mention-popup",
            Self::QuickSwitcher => "quick-switcher",
            Self::IncomingCallOverlay => "incoming-call-overlay",
            Self::ToastBanner => "toast-banner",
            Self::ProfilePopup => "profile-popup",
            Self::WelcomeOverlay => "welcome-overlay",
        }
    }

    fn tall_layout(self) -> bool {
        matches!(self, Self::Settings | Self::System)
    }
}

#[derive(Clone, Copy)]
struct RelativeRect {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl RelativeRect {
    const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    fn resolve(self, width: u32, height: u32) -> PixelRect {
        let left = ((width as f32) * self.left)
            .floor()
            .clamp(0.0, width as f32 - 1.0) as u32;
        let top = ((height as f32) * self.top)
            .floor()
            .clamp(0.0, height as f32 - 1.0) as u32;
        let right = ((width as f32) * self.right)
            .ceil()
            .clamp(left as f32 + 1.0, width as f32) as u32;
        let bottom = ((height as f32) * self.bottom)
            .ceil()
            .clamp(top as f32 + 1.0, height as f32) as u32;
        PixelRect {
            left,
            top,
            right,
            bottom,
        }
    }
}

#[derive(Clone, Copy)]
struct PixelRect {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

#[derive(Clone, Copy)]
struct RegionExpectation {
    name: &'static str,
    rect: RelativeRect,
    min_edge_ratio: f32,
    min_color_buckets: usize,
    min_luma_deviation: f32,
}

fn init_backend() {
    INIT_BACKEND.call_once(|| {
        slint::platform::set_platform(Box::new(SnapshotPlatform))
            .expect("snapshot platform should install once");
    });
}

fn s(value: &str) -> slint::SharedString {
    value.into()
}

fn model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    Rc::new(VecModel::from(rows)).into()
}

fn luma_deviation(snapshot: &SharedPixelBuffer<Rgba8Pixel>, rect: PixelRect) -> f32 {
    let width = snapshot.width();
    let bytes = snapshot.as_bytes();
    let mut lumas =
        Vec::with_capacity(((rect.right - rect.left) * (rect.bottom - rect.top)) as usize);
    for y in rect.top..rect.bottom {
        for x in rect.left..rect.right {
            let idx = ((y * width + x) * 4) as usize;
            let luma = 0.2126 * bytes[idx] as f32
                + 0.7152 * bytes[idx + 1] as f32
                + 0.0722 * bytes[idx + 2] as f32;
            lumas.push(luma);
        }
    }
    let mean = lumas.iter().sum::<f32>() / lumas.len() as f32;
    lumas.iter().map(|v| (v - mean).abs()).sum::<f32>() / lumas.len() as f32
}

fn edge_ratio(snapshot: &SharedPixelBuffer<Rgba8Pixel>, rect: PixelRect) -> f32 {
    let width = snapshot.width();
    let bytes = snapshot.as_bytes();
    let mut edges = 0usize;
    let mut samples = 0usize;

    for y in rect.top..rect.bottom.saturating_sub(1) {
        for x in rect.left..rect.right.saturating_sub(1) {
            let idx = ((y * width + x) * 4) as usize;
            let right = idx + 4;
            let down = (((y + 1) * width + x) * 4) as usize;
            let dx = (bytes[idx] as i16 - bytes[right] as i16).abs()
                + (bytes[idx + 1] as i16 - bytes[right + 1] as i16).abs()
                + (bytes[idx + 2] as i16 - bytes[right + 2] as i16).abs();
            let dy = (bytes[idx] as i16 - bytes[down] as i16).abs()
                + (bytes[idx + 1] as i16 - bytes[down + 1] as i16).abs()
                + (bytes[idx + 2] as i16 - bytes[down + 2] as i16).abs();
            if dx > 44 {
                edges += 1;
            }
            if dy > 44 {
                edges += 1;
            }
            samples += 2;
        }
    }

    if samples == 0 {
        0.0
    } else {
        edges as f32 / samples as f32
    }
}

fn color_bucket_count(snapshot: &SharedPixelBuffer<Rgba8Pixel>, rect: PixelRect) -> usize {
    let width = snapshot.width();
    let bytes = snapshot.as_bytes();
    let mut buckets = std::collections::BTreeSet::new();
    for y in rect.top..rect.bottom {
        for x in rect.left..rect.right {
            let idx = ((y * width + x) * 4) as usize;
            let r = bytes[idx] / 32;
            let g = bytes[idx + 1] / 32;
            let b = bytes[idx + 2] / 32;
            buckets.insert(((r as u16) << 10) | ((g as u16) << 5) | b as u16);
        }
    }
    buckets.len()
}

fn assert_visual_region(
    snapshot: &SharedPixelBuffer<Rgba8Pixel>,
    scenario: UiScenario,
    width: LayoutWidth,
    theme: ShellTheme,
    region: RegionExpectation,
) {
    let rect = region.rect.resolve(snapshot.width(), snapshot.height());
    let edges = edge_ratio(snapshot, rect);
    let buckets = color_bucket_count(snapshot, rect);
    let deviation = luma_deviation(snapshot, rect);
    assert!(
        edges >= region.min_edge_ratio,
        "{} {} {} {} edge ratio too low: {:.4} < {:.4}",
        scenario.label(),
        width.label(),
        theme.label(),
        region.name,
        edges,
        region.min_edge_ratio
    );
    assert!(
        buckets >= region.min_color_buckets,
        "{} {} {} {} color buckets too low: {} < {}",
        scenario.label(),
        width.label(),
        theme.label(),
        region.name,
        buckets,
        region.min_color_buckets
    );
    assert!(
        deviation >= region.min_luma_deviation,
        "{} {} {} {} luma deviation too low: {:.2} < {:.2}",
        scenario.label(),
        width.label(),
        theme.label(),
        region.name,
        deviation,
        region.min_luma_deviation
    );
}

fn assert_snapshot_has_content(
    snapshot: &SharedPixelBuffer<Rgba8Pixel>,
    scenario: UiScenario,
    width: LayoutWidth,
    theme: ShellTheme,
) {
    let full = PixelRect {
        left: 0,
        top: 0,
        right: snapshot.width(),
        bottom: snapshot.height(),
    };
    let buckets = color_bucket_count(snapshot, full);
    let edges = edge_ratio(snapshot, full);
    let deviation = luma_deviation(snapshot, full);
    let min_deviation = match scenario {
        UiScenario::IncomingCallOverlay => 7.0,
        UiScenario::ToastBanner | UiScenario::ProfilePopup | UiScenario::WelcomeOverlay => 6.0,
        _ => 8.0,
    };

    assert!(
        buckets >= 16,
        "{} {} {} rendered too few color buckets: {}",
        scenario.label(),
        width.label(),
        theme.label(),
        buckets
    );
    assert!(
        edges >= 0.008,
        "{} {} {} rendered too few edges: {:.4}",
        scenario.label(),
        width.label(),
        theme.label(),
        edges
    );
    assert!(
        deviation >= min_deviation,
        "{} {} {} rendered too little tonal variation: {:.2} < {:.2}",
        scenario.label(),
        width.label(),
        theme.label(),
        deviation,
        min_deviation
    );
}

fn maybe_write_snapshot(
    scenario: UiScenario,
    width: LayoutWidth,
    theme: ShellTheme,
    snapshot: &SharedPixelBuffer<Rgba8Pixel>,
) -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("VOXLINK_UI_WRITE_SNAPSHOTS").is_none() {
        return Ok(());
    }

    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui_visibility_snapshots");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "{}-{}-{}.png",
        scenario.label(),
        width.label(),
        theme.label()
    ));
    let file = File::create(path)?;
    let mut encoder = png::Encoder::new(file, snapshot.width(), snapshot.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(snapshot.as_bytes())?;
    Ok(())
}

fn sample_saved_servers() -> ModelRc<SavedServerData> {
    model(vec![
        SavedServerData {
            name: s("Primary"),
            address: s("wss://voice.voxlink.dev"),
            is_default: true,
        },
        SavedServerData {
            name: s("Local"),
            address: s("ws://127.0.0.1:9090"),
            is_default: false,
        },
    ])
}

fn sample_public_spaces() -> ModelRc<PublicSpaceData> {
    model(vec![
        PublicSpaceData {
            id: s("space-public-1"),
            name: s("Studio Ops"),
            description: s("Live product reviews and voice design critiques."),
            invite_code: s("studio-ops"),
            member_count: 184,
            channel_count: 12,
            online_count: 37,
            initial: s("S"),
            color_index: 2,
        },
        PublicSpaceData {
            id: s("space-public-2"),
            name: s("Build Club"),
            description: s("Fast feedback for desktop UX and audio workflows."),
            invite_code: s("build-club"),
            member_count: 92,
            channel_count: 9,
            online_count: 14,
            initial: s("B"),
            color_index: 5,
        },
    ])
}

fn sample_spaces() -> ModelRc<SpaceData> {
    model(vec![
        SpaceData {
            id: s("space-1"),
            name: s("Design Ops"),
            initial: s("D"),
            member_count: 18,
            channel_count: 9,
            color_index: 1,
            has_unread: true,
        },
        SpaceData {
            id: s("space-2"),
            name: s("Launch Crew"),
            initial: s("L"),
            member_count: 11,
            channel_count: 5,
            color_index: 4,
            has_unread: false,
        },
    ])
}

fn sample_threads() -> ModelRc<DirectMessageThreadData> {
    model(vec![
        DirectMessageThreadData {
            user_id: s("user-alex"),
            name: s("Alex"),
            initial: s("A"),
            preview: s("Can you sanity check the narrow layout?"),
            meta: s("2m ago"),
            unread_count: 3,
            is_online: true,
            is_in_voice: false,
            color_index: 1,
        },
        DirectMessageThreadData {
            user_id: s("user-zoe"),
            name: s("Zoe"),
            initial: s("Z"),
            preview: s("Pushed the latest soundboard tweaks."),
            meta: s("1h ago"),
            unread_count: 0,
            is_online: true,
            is_in_voice: true,
            color_index: 6,
        },
    ])
}

fn sample_friends() -> ModelRc<FriendData> {
    model(vec![
        FriendData {
            user_id: s("user-alex"),
            name: s("Alex"),
            initial: s("A"),
            detail: s("Reviewing UI"),
            status_label: s("Online"),
            is_online: true,
            is_in_voice: false,
            color_index: 1,
            is_friend: true,
            last_seen: s("now"),
        },
        FriendData {
            user_id: s("user-riley"),
            name: s("Riley"),
            initial: s("R"),
            detail: s("In Launch Room"),
            status_label: s("In voice"),
            is_online: true,
            is_in_voice: true,
            color_index: 3,
            is_friend: true,
            last_seen: s("now"),
        },
    ])
}

fn sample_friend_requests() -> (ModelRc<FriendRequestData>, ModelRc<FriendRequestData>) {
    let incoming = model(vec![FriendRequestData {
        user_id: s("user-sam"),
        name: s("Sam"),
        initial: s("S"),
        detail: s("Wants to connect"),
        color_index: 2,
    }]);
    let outgoing = model(vec![FriendRequestData {
        user_id: s("user-jules"),
        name: s("Jules"),
        initial: s("J"),
        detail: s("Request pending"),
        color_index: 4,
    }]);
    (incoming, outgoing)
}

fn sample_channels() -> ModelRc<ChannelData> {
    model(vec![
        ChannelData {
            id: s("cat-design"),
            name: s("Design"),
            is_category_header: true,
            category: s("Design"),
            category_collapsed: false,
            ..Default::default()
        },
        ChannelData {
            id: s("chan-roadmap"),
            name: s("roadmap"),
            category: s("Design"),
            is_voice: false,
            unread_count: 4,
            mention_count: 1,
            is_favorite: true,
            notification_setting: s("all"),
            topic: s("Sprint and feature planning"),
            ..Default::default()
        },
        ChannelData {
            id: s("chan-reviews"),
            name: s("reviews"),
            category: s("Design"),
            is_voice: false,
            is_active: true,
            unread_count: 1,
            topic: s("UX reviews and approvals"),
            notification_setting: s("mentions"),
            ..Default::default()
        },
        ChannelData {
            id: s("chan-lounge"),
            name: s("design-lounge"),
            category: s("Design"),
            is_voice: true,
            peer_count: 5,
            voice_quality: 2,
            user_limit: 12,
            is_favorite: true,
            ..Default::default()
        },
        ChannelData {
            id: s("chan-critique"),
            name: s("critique"),
            category: s("Design"),
            is_voice: true,
            peer_count: 2,
            voice_quality: 3,
            status: s("Live"),
            ..Default::default()
        },
    ])
}

fn sample_members() -> ModelRc<MemberData> {
    model(vec![
        MemberData {
            id: s("member-1"),
            user_id: s("user-you"),
            name: s("Jordan"),
            initial: s("J"),
            role_label: s("Owner"),
            role_tier: 3,
            channel_name: s("design-lounge"),
            status: s("Reviewing narrow layouts"),
            is_online: true,
            is_in_voice: true,
            color_index: 1,
            status_level: 0,
            is_server_muted: false,
            is_friend: false,
            has_incoming_request: false,
            has_outgoing_request: false,
            is_self: true,
            bio: s("Shipping Voxlink UX polish."),
            nickname: s("JPH"),
            user_note: s(""),
            role_color_index: 4,
            activity: s("Editing UI"),
        },
        MemberData {
            id: s("member-2"),
            user_id: s("user-alex"),
            name: s("Alex"),
            initial: s("A"),
            role_label: s("Moderator"),
            role_tier: 1,
            channel_name: s("critique"),
            status: s("In critique"),
            is_online: true,
            is_in_voice: true,
            color_index: 2,
            status_level: 1,
            is_server_muted: false,
            is_friend: true,
            has_incoming_request: false,
            has_outgoing_request: false,
            is_self: false,
            bio: s("Testing moderation flows."),
            nickname: s(""),
            user_note: s("Needs a DM follow-up"),
            role_color_index: 2,
            activity: s("Reviewing prototype"),
        },
        MemberData {
            id: s("member-3"),
            user_id: s("user-riley"),
            name: s("Riley"),
            initial: s("R"),
            role_label: s("Member"),
            role_tier: 0,
            channel_name: s(""),
            status: s("Offline soon"),
            is_online: true,
            is_in_voice: false,
            color_index: 5,
            status_level: 0,
            is_server_muted: false,
            is_friend: false,
            has_incoming_request: true,
            has_outgoing_request: false,
            is_self: false,
            bio: s("Watching the rollout."),
            nickname: s(""),
            user_note: s(""),
            role_color_index: 0,
            activity: s(""),
        },
    ])
}

fn sample_audit_entries() -> ModelRc<AuditEntryData> {
    model(vec![
        AuditEntryData {
            actor_name: s("Jordan"),
            action: s("created"),
            target_name: s("#reviews"),
            detail: s("Text channel created for design approvals"),
            timestamp: s("09:42"),
        },
        AuditEntryData {
            actor_name: s("Alex"),
            action: s("updated"),
            target_name: s("Riley"),
            detail: s("Role changed to moderator"),
            timestamp: s("10:05"),
        },
    ])
}

fn sample_messages() -> ModelRc<ChatMessage> {
    model(vec![
        ChatMessage {
            sender_name: s("Alex"),
            sender_initial: s("A"),
            sender_id: s("user-alex"),
            content: s("Need the add-friend field to stay readable at narrow widths."),
            timestamp: s("09:51"),
            color_index: 2,
            show_header: true,
            message_id: s("m1"),
            ..Default::default()
        },
        ChatMessage {
            sender_name: s("Jordan"),
            sender_initial: s("J"),
            sender_id: s("user-you"),
            content: s("Fixed the shared input sizing and now checking the compact forms."),
            timestamp: s("09:53"),
            is_self: true,
            color_index: 1,
            show_header: true,
            edited: true,
            reactions: s("👍 2"),
            message_id: s("m2"),
            ..Default::default()
        },
        ChatMessage {
            sender_name: s("Riley"),
            sender_initial: s("R"),
            sender_id: s("user-riley"),
            content: s("Search overlay also needs to stay legible."),
            timestamp: s("09:55"),
            reply_sender_name: s("Jordan"),
            reply_preview: s("Fixed the shared input sizing"),
            color_index: 5,
            show_header: true,
            message_id: s("m3"),
            ..Default::default()
        },
    ])
}

fn sample_thread_messages() -> ModelRc<ChatMessage> {
    model(vec![
        ChatMessage {
            sender_name: s("Jordan"),
            sender_initial: s("J"),
            sender_id: s("user-you"),
            content: s("Thread kickoff: keep the composer readable on narrow screens."),
            timestamp: s("10:02"),
            color_index: 1,
            show_header: true,
            message_id: s("tm1"),
            ..Default::default()
        },
        ChatMessage {
            sender_name: s("Alex"),
            sender_initial: s("A"),
            sender_id: s("user-alex"),
            content: s(
                "Confirmed. The quick switcher and thread panel still need screenshot coverage.",
            ),
            timestamp: s("10:04"),
            color_index: 2,
            show_header: true,
            message_id: s("tm2"),
            ..Default::default()
        },
    ])
}

fn sample_soundboard_clips() -> ModelRc<SoundboardClipData> {
    model(vec![
        SoundboardClipData {
            name: s("Ship it"),
            path: s("/tmp/ship-it.wav"),
            keybind: s("f5"),
        },
        SoundboardClipData {
            name: s("Mic check"),
            path: s("/tmp/mic-check.wav"),
            keybind: s("f6"),
        },
    ])
}

fn sample_perf() -> PerfData {
    PerfData {
        cpu_percent: 12.0,
        memory_mb: 96.4,
        uptime_secs: 3661,
        audio_active: true,
        network_connected: true,
        dropped_frames: 1,
        jitter_buffer_ms: 22,
        frame_loss_percent: 0.2,
        encode_bitrate_kbps: 64,
        decode_peers: 3,
        udp_active: true,
        ping_ms: 38,
        screen_frames_completed: 14,
        screen_frames_dropped: 1,
        screen_frames_timed_out: 0,
    }
}

fn populate_fixture(window: &MainWindow, scenario: UiScenario, theme: ShellTheme) {
    let (incoming_requests, outgoing_requests) = sample_friend_requests();

    window.set_dark_mode(theme.dark_mode());
    window.set_theme_preset(0);
    window.set_first_run(false);
    window.set_user_name(s("Jordan"));
    window.set_status_text(s("Connected"));
    window.set_is_connected(true);
    window.set_server_address(s("wss://voice.voxlink.dev"));
    window.set_show_saved(true);
    window.set_saved_servers(sample_saved_servers());
    window.set_activity_text(s("Reviewing UI visibility"));
    window.set_public_spaces(sample_public_spaces());
    window.set_spaces(sample_spaces());
    window.set_space_name(s("Design Ops"));
    window.set_space_invite_code(s("design-ops"));
    window.set_show_quick_call(true);
    window.set_join_code(s("team-sync"));
    window.set_room_password(s("synth"));
    window.set_direct_message_threads(sample_threads());
    window.set_unread_direct_messages_count(3);
    window.set_favorite_friends(sample_friends());
    window.set_favorite_friends_count(2);
    window.set_online_friends_count(2);
    window.set_live_friends_count(1);
    window.set_incoming_friend_requests(incoming_requests);
    window.set_outgoing_friend_requests(outgoing_requests);
    window.set_perf(sample_perf());
    window.set_uptime_text(s("1h 1m"));
    window.set_version_text(s("0.10.3"));
    window.set_ping_ms(38);
    window.set_room_code(s(""));
    window.set_current_space_name(s("Design Ops"));
    window.set_current_space_invite(s("design-ops"));
    window.set_space_description(s("Channels, reviews, and voice critique in one space."));
    window.set_space_role_label(s("Owner"));
    window.set_can_manage_space_channels(true);
    window.set_can_manage_space_members(true);
    window.set_can_manage_space_roles(true);
    window.set_can_view_space_audit(true);
    window.set_channels(sample_channels());
    window.set_members(sample_members());
    window.set_space_audit_entries(sample_audit_entries());
    window.set_visible_text_channels(2);
    window.set_visible_voice_channels(2);
    window.set_visible_favorite_channels(2);
    window.set_visible_members(3);
    window.set_new_channel_name(s("launch-reviews"));
    window.set_chat_channel_name(s("reviews"));
    window.set_chat_context_subtitle(s("Design Ops"));
    window.set_chat_messages(sample_messages());
    window.set_chat_search_query(s("layout"));
    window.set_chat_search_results(sample_messages());
    window.set_chat_input(s("Message #reviews"));
    window.set_chat_typing_text(s("Alex is typing"));
    window.set_thread_panel_visible(false);
    window.set_thread_messages(sample_thread_messages());
    window.set_thread_parent_sender(s("Alex"));
    window.set_thread_parent_content(s(
        "Need a visibility pass on the thread panel and mention popup.",
    ));
    window.set_thread_parent_timestamp(s("10:01"));
    window.set_mention_popup_visible(false);
    window.set_mention_suggestions(model(vec![s("Alex"), s("Avery"), s("Alicia"), s("Alonzo")]));
    window.set_mention_selected_index(1);
    window.set_input_devices(model(vec![s("Built-in Microphone"), s("USB Interface")]));
    window.set_output_devices(model(vec![s("Studio Monitors"), s("Headphones")]));
    window.set_is_logged_in(true);
    window.set_account_email(s("jordan@voxlink.dev"));
    window.set_privacy_config_path(s(
        "/Users/jordan/Library/Application Support/Voxlink/config.json",
    ));
    window.set_privacy_log_path(s("/Users/jordan/Library/Logs/Voxlink/voice.log"));
    window.set_soundboard_clips(sample_soundboard_clips());
    window.set_show_login_view(false);
    window.set_login_mode(true);
    window.set_incoming_call_visible(false);
    window.set_toast_visible(false);
    window.set_profile_popup_visible(false);
    window.set_show_welcome_overlay(false);
    window.set_quick_switcher_visible(false);
    window.set_quick_switcher_query(s("review"));
    window.set_quick_switcher_items(sample_channels());

    match scenario {
        UiScenario::LoginOverlay => {
            window.set_current_view(0);
            window.set_show_login_view(true);
            window.set_login_mode(false);
            window.set_auth_email(s("you@example.com"));
            window.set_auth_password(s("visible-password"));
            window.set_auth_display_name(s("Jordan"));
        }
        UiScenario::Home => {
            window.set_current_view(0);
        }
        UiScenario::Settings => {
            window.set_current_view(2);
            window.set_previous_view(1);
        }
        UiScenario::System => {
            window.set_current_view(3);
        }
        UiScenario::Space => {
            window.set_current_view(4);
        }
        UiScenario::Chat => {
            window.set_current_view(5);
            window.set_chat_is_direct_message(false);
        }
        UiScenario::ChatThread => {
            window.set_current_view(5);
            window.set_chat_is_direct_message(false);
            window.set_thread_panel_visible(true);
        }
        UiScenario::ChatMentionPopup => {
            window.set_current_view(5);
            window.set_chat_is_direct_message(false);
            window.set_chat_input(s(
                "@al Need a second set of eyes on the member card layout.",
            ));
            window.set_reply_target_message_id(s("m1"));
            window.set_reply_target_sender_name(s("Alex"));
            window.set_reply_target_preview(s(
                "Need the add-friend field to stay readable at narrow widths.",
            ));
            window.set_mention_popup_visible(true);
        }
        UiScenario::QuickSwitcher => {
            window.set_current_view(0);
            window.set_quick_switcher_visible(true);
            window.set_quick_switcher_query(s("re"));
        }
        UiScenario::IncomingCallOverlay => {
            window.set_current_view(0);
            window.set_incoming_call_name(s("Alex"));
            window.set_incoming_call_visible(true);
        }
        UiScenario::ToastBanner => {
            window.set_current_view(5);
            window.set_toast_message(s("Saved display name and refreshed the current session."));
            window.set_toast_type(1);
            window.set_toast_visible(true);
        }
        UiScenario::ProfilePopup => {
            window.set_current_view(3);
            window.set_profile_popup_visible(true);
            window.set_profile_popup_user_id(s("user-alex"));
            window.set_profile_popup_name(s("Alex"));
            window.set_profile_popup_initial(s("A"));
            window.set_profile_popup_status(s("Online"));
            window.set_profile_popup_role(s("Moderator"));
            window.set_profile_popup_role_color_index(2);
            window.set_profile_popup_activity(s("Reviewing the latest visibility regressions."));
            window.set_profile_popup_bio(s(
                "Keeps the launch room readable under load and on narrow layouts.",
            ));
            window.set_profile_popup_color_index(2);
        }
        UiScenario::WelcomeOverlay => {
            window.set_current_view(4);
            window.set_show_welcome_overlay(true);
            window.set_welcome_message(s("Start in #reviews for feedback, jump into design-lounge for voice, and use the quick switcher any time."));
        }
    }
}

fn expected_regions(scenario: UiScenario, width: LayoutWidth) -> Vec<RegionExpectation> {
    match (scenario, width) {
        (UiScenario::LoginOverlay, LayoutWidth::Narrow)
        | (UiScenario::LoginOverlay, LayoutWidth::Wide) => vec![RegionExpectation {
            name: "auth-panel",
            rect: RelativeRect::new(0.22, 0.22, 0.78, 0.84),
            min_edge_ratio: 0.008,
            min_color_buckets: 8,
            min_luma_deviation: 10.0,
        }],
        (UiScenario::Home, LayoutWidth::Narrow) => vec![
            RegionExpectation {
                name: "server-card",
                rect: RelativeRect::new(0.04, 0.18, 0.96, 0.42),
                min_edge_ratio: 0.008,
                min_color_buckets: 8,
                min_luma_deviation: 10.0,
            },
            RegionExpectation {
                name: "quick-call",
                rect: RelativeRect::new(0.04, 0.50, 0.96, 0.78),
                min_edge_ratio: 0.008,
                min_color_buckets: 8,
                min_luma_deviation: 10.0,
            },
        ],
        (UiScenario::Home, LayoutWidth::Wide) => vec![
            RegionExpectation {
                name: "server-card",
                rect: RelativeRect::new(0.04, 0.18, 0.58, 0.42),
                min_edge_ratio: 0.008,
                min_color_buckets: 8,
                min_luma_deviation: 10.0,
            },
            RegionExpectation {
                name: "quick-call",
                rect: RelativeRect::new(0.64, 0.18, 0.96, 0.56),
                min_edge_ratio: 0.008,
                min_color_buckets: 8,
                min_luma_deviation: 10.0,
            },
        ],
        (UiScenario::Settings, LayoutWidth::Narrow) | (UiScenario::Settings, LayoutWidth::Wide) => {
            vec![RegionExpectation {
                name: "settings-header",
                rect: RelativeRect::new(0.04, 0.02, 0.96, 0.18),
                min_edge_ratio: 0.008,
                min_color_buckets: 8,
                min_luma_deviation: 10.0,
            }]
        }
        (UiScenario::System, LayoutWidth::Narrow) => vec![RegionExpectation {
            name: "add-friend-card",
            rect: RelativeRect::new(0.04, 0.28, 0.96, 0.52),
            min_edge_ratio: 0.008,
            min_color_buckets: 8,
            min_luma_deviation: 9.0,
        }],
        (UiScenario::System, LayoutWidth::Wide) => vec![RegionExpectation {
            name: "add-friend-card",
            rect: RelativeRect::new(0.70, 0.18, 0.96, 0.40),
            min_edge_ratio: 0.008,
            min_color_buckets: 8,
            min_luma_deviation: 8.0,
        }],
        (UiScenario::Space, LayoutWidth::Narrow) | (UiScenario::Space, LayoutWidth::Wide) => {
            vec![RegionExpectation {
                name: "search-bar",
                rect: RelativeRect::new(0.04, 0.08, 0.72, 0.18),
                min_edge_ratio: 0.010,
                min_color_buckets: 8,
                min_luma_deviation: 10.0,
            }]
        }
        (UiScenario::Chat, LayoutWidth::Narrow) | (UiScenario::Chat, LayoutWidth::Wide) => {
            vec![RegionExpectation {
                name: "composer",
                rect: RelativeRect::new(0.04, 0.84, 0.96, 0.98),
                min_edge_ratio: 0.010,
                min_color_buckets: 8,
                min_luma_deviation: 10.0,
            }]
        }
        (UiScenario::ChatThread, LayoutWidth::Narrow) => vec![RegionExpectation {
            name: "thread-panel",
            rect: RelativeRect::new(0.48, 0.04, 0.98, 0.98),
            min_edge_ratio: 0.008,
            min_color_buckets: 8,
            min_luma_deviation: 8.0,
        }],
        (UiScenario::ChatThread, LayoutWidth::Wide) => vec![RegionExpectation {
            name: "thread-panel",
            rect: RelativeRect::new(0.62, 0.04, 0.98, 0.98),
            min_edge_ratio: 0.008,
            min_color_buckets: 8,
            min_luma_deviation: 7.0,
        }],
        (UiScenario::ChatMentionPopup, LayoutWidth::Narrow)
        | (UiScenario::ChatMentionPopup, LayoutWidth::Wide) => vec![RegionExpectation {
            name: "mention-popup",
            rect: RelativeRect::new(0.16, 0.72, 0.78, 0.94),
            min_edge_ratio: 0.008,
            min_color_buckets: 8,
            min_luma_deviation: 7.0,
        }],
        (UiScenario::QuickSwitcher, LayoutWidth::Narrow)
        | (UiScenario::QuickSwitcher, LayoutWidth::Wide) => vec![RegionExpectation {
            name: "switcher",
            rect: RelativeRect::new(0.18, 0.10, 0.82, 0.54),
            min_edge_ratio: 0.008,
            min_color_buckets: 8,
            min_luma_deviation: 6.0,
        }],
        (UiScenario::IncomingCallOverlay, LayoutWidth::Narrow)
        | (UiScenario::IncomingCallOverlay, LayoutWidth::Wide) => vec![RegionExpectation {
            name: "incoming-call-card",
            rect: RelativeRect::new(0.24, 0.26, 0.76, 0.62),
            min_edge_ratio: 0.008,
            min_color_buckets: 8,
            min_luma_deviation: 9.0,
        }],
        (UiScenario::ToastBanner, LayoutWidth::Narrow)
        | (UiScenario::ToastBanner, LayoutWidth::Wide) => vec![RegionExpectation {
            name: "toast",
            rect: RelativeRect::new(0.24, 0.90, 0.76, 0.98),
            min_edge_ratio: 0.008,
            min_color_buckets: 6,
            min_luma_deviation: 6.0,
        }],
        (UiScenario::ProfilePopup, LayoutWidth::Narrow)
        | (UiScenario::ProfilePopup, LayoutWidth::Wide) => vec![RegionExpectation {
            name: "profile-popup",
            rect: RelativeRect::new(0.24, 0.18, 0.76, 0.76),
            min_edge_ratio: 0.008,
            min_color_buckets: 8,
            min_luma_deviation: 4.0,
        }],
        (UiScenario::WelcomeOverlay, LayoutWidth::Narrow)
        | (UiScenario::WelcomeOverlay, LayoutWidth::Wide) => vec![RegionExpectation {
            name: "welcome-card",
            rect: RelativeRect::new(0.22, 0.24, 0.78, 0.74),
            min_edge_ratio: 0.008,
            min_color_buckets: 8,
            min_luma_deviation: 9.0,
        }],
    }
}

fn capture_snapshot(
    scenario: UiScenario,
    width: LayoutWidth,
    theme: ShellTheme,
) -> SharedPixelBuffer<Rgba8Pixel> {
    init_backend();

    let window = MainWindow::new().expect("main window should build");
    populate_fixture(&window, scenario, theme);
    window.window().set_size(width.size(scenario.tall_layout()));
    window
        .show()
        .expect("window should show in testing backend");
    let snapshot = window.window().take_snapshot().unwrap_or_else(|err| {
        panic!(
            "failed to snapshot {} {} {}: {err}",
            scenario.label(),
            width.label(),
            theme.label()
        )
    });
    let _ = window.hide();
    snapshot
}

fn run_snapshot_matrix() {
    let scenarios = [
        UiScenario::LoginOverlay,
        UiScenario::Home,
        UiScenario::Settings,
        UiScenario::System,
        UiScenario::Space,
        UiScenario::Chat,
        UiScenario::ChatThread,
        UiScenario::ChatMentionPopup,
        UiScenario::QuickSwitcher,
        UiScenario::IncomingCallOverlay,
        UiScenario::ToastBanner,
        UiScenario::ProfilePopup,
        UiScenario::WelcomeOverlay,
    ];

    for scenario in scenarios {
        for width in [LayoutWidth::Narrow, LayoutWidth::Wide] {
            for theme in [ShellTheme::Dark, ShellTheme::Light] {
                let snapshot = capture_snapshot(scenario, width, theme);
                assert_snapshot_has_content(&snapshot, scenario, width, theme);
                for region in expected_regions(scenario, width) {
                    assert_visual_region(&snapshot, scenario, width, theme, region);
                }
                maybe_write_snapshot(scenario, width, theme, &snapshot)
                    .expect("snapshot write should succeed");
            }
        }
    }
}

#[test]
fn snapshot_matrix_covers_key_views_in_narrow_and_wide_layouts() {
    let handle = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(run_snapshot_matrix)
        .expect("snapshot test thread should spawn");
    handle
        .join()
        .expect("snapshot test thread should complete without panic");
}

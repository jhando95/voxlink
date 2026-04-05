use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppView {
    #[default]
    Home,
    Room,
    Settings,
    Performance,
    Space,
    TextChat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicMode {
    #[default]
    OpenMic,
    PushToTalk,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug, Clone)]
pub struct Participant {
    pub id: String,
    pub name: String,
    pub is_muted: bool,
    pub is_deafened: bool,
    pub is_speaking: bool,
    pub volume: f32,
}

#[derive(Debug, Clone, Default)]
pub struct RoomState {
    pub room_code: String,
    pub participants: Vec<Participant>,
    pub is_muted: bool,
    pub is_deafened: bool,
    pub mic_mode: MicMode,
    pub connection: ConnectionState,
    pub active_screen_share_peer_id: Option<String>,
    pub active_screen_share_peer_name: Option<String>,
    pub is_sharing_screen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub invite_code: String,
    pub member_count: u32,
    pub channel_count: u32,
    #[serde(default)]
    pub is_owner: bool,
    #[serde(default)]
    pub self_role: SpaceRole,
    #[serde(default)]
    pub is_public: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelType {
    #[default]
    Voice,
    Text,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpaceRole {
    Owner,
    Admin,
    Moderator,
    #[default]
    Member,
}

impl SpaceRole {
    /// Numeric privilege level (higher = more privilege).
    pub fn level(self) -> u8 {
        match self {
            SpaceRole::Owner => 3,
            SpaceRole::Admin => 2,
            SpaceRole::Moderator => 1,
            SpaceRole::Member => 0,
        }
    }

    /// Returns true if this role has at least the privilege of `required`.
    pub fn has_at_least(self, required: SpaceRole) -> bool {
        self.level() >= required.level()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserStatus {
    #[default]
    Online,
    Idle,
    DoNotDisturb,
    Invisible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BanInfo {
    pub user_id: String,
    pub user_name: String,
    pub banned_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    pub peer_count: u32,
    #[serde(default)]
    pub channel_type: ChannelType,
    #[serde(default)]
    pub topic: String,
    /// Voice quality preset: 0=Low(24kbps), 1=Standard(48kbps), 2=High(64kbps), 3=Ultra(96kbps)
    #[serde(default = "default_voice_quality")]
    pub voice_quality: u8,
    /// Max users allowed in this voice channel (0 = unlimited)
    #[serde(default)]
    pub user_limit: u32,
    /// Category/group name for channel organization
    #[serde(default)]
    pub category: String,
    /// Short status text displayed on voice channels (e.g. "Playing Valorant")
    #[serde(default)]
    pub status: String,
    /// Slow mode cooldown in seconds (0 = disabled)
    #[serde(default)]
    pub slow_mode_secs: u32,
    /// Channel display position (lower = higher in list). Default 0 = insertion order.
    #[serde(default)]
    pub position: u32,
    /// Auto-delete messages after N hours (0 = disabled)
    #[serde(default)]
    pub auto_delete_hours: u32,
    /// Minimum role required to access this channel (empty = member)
    #[serde(default)]
    pub min_role: String,
}

fn default_voice_quality() -> u8 {
    2 // High (64kbps) — current default
}

fn default_search_limit() -> u32 {
    50
}

/// Map voice quality preset index to Opus bitrate in bps
pub fn voice_quality_bitrate(quality: u8) -> i32 {
    match quality {
        0 => 24000,  // Low — poor network, minimal bandwidth
        1 => 48000,  // Standard — good voice clarity
        3 => 96000,  // Ultra — near-transparent voice
        _ => 64000,  // High (default) — excellent quality
    }
}

/// Display label for voice quality preset
pub fn voice_quality_label(quality: u8) -> &'static str {
    match quality {
        0 => "Low",
        1 => "Standard",
        3 => "Ultra",
        _ => "High",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberInfo {
    pub id: String,
    #[serde(default)]
    pub user_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub role: SpaceRole,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub channel_name: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub bio: String,
    #[serde(default)]
    pub nickname: Option<String>,
    #[serde(default)]
    pub status_preset: UserStatus,
    /// Hex color for the member's role (e.g. "#ff5555"), empty for default
    #[serde(default)]
    pub role_color: String,
    /// Activity status text (e.g. "Playing Valorant"), empty if none
    #[serde(default)]
    pub activity: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpaceAuditEntry {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub actor_name: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub target_name: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub timestamp: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FavoriteFriend {
    pub user_id: String,
    pub name: String,
    #[serde(default)]
    pub is_online: bool,
    #[serde(default)]
    pub is_in_voice: bool,
    #[serde(default)]
    pub in_private_call: bool,
    #[serde(default)]
    pub active_space_name: String,
    #[serde(default)]
    pub active_channel_name: String,
    #[serde(default)]
    pub last_space_name: String,
    #[serde(default)]
    pub last_channel_name: String,
    #[serde(default)]
    pub last_seen_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FriendPresence {
    pub user_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub is_online: bool,
    #[serde(default)]
    pub is_in_voice: bool,
    #[serde(default)]
    pub in_private_call: bool,
    #[serde(default)]
    pub active_space_name: Option<String>,
    #[serde(default)]
    pub active_channel_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FriendRequest {
    pub user_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub requested_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectMessageThread {
    pub user_id: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub last_message_id: String,
    #[serde(default)]
    pub last_message_preview: String,
    #[serde(default)]
    pub last_message_at: u64,
    #[serde(default)]
    pub unread_count: u32,
    #[serde(default)]
    pub is_online: bool,
    #[serde(default)]
    pub is_in_voice: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SpaceState {
    pub id: String,
    pub name: String,
    pub description: String,
    pub invite_code: String,
    pub channels: Vec<ChannelInfo>,
    pub members: Vec<MemberInfo>,
    pub audit_log: Vec<SpaceAuditEntry>,
    pub active_channel_id: Option<String>,
    pub selected_text_channel_id: Option<String>,
    pub self_role: SpaceRole,
    pub unread_text_channels: HashMap<String, u32>,
    pub typing_users: HashMap<String, Vec<String>>,
    /// Typing timestamps: (channel_id, user_name) → tick when typing started.
    /// Used for client-side 5-second timeout of stale typing indicators.
    pub typing_ticks: HashMap<(String, String), u64>,
}

#[derive(Debug, Clone)]
pub struct PendingMessage {
    pub channel_id: String,
    pub content: String,
    pub is_direct: bool,
    pub retry_count: u8,
    pub queued_at: u64,
}

#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub current_view: AppView,
    pub room: RoomState,
    pub space: Option<SpaceState>,
    pub self_user_id: Option<String>,
    pub favorite_friends: Vec<FavoriteFriend>,
    pub incoming_friend_requests: Vec<FriendRequest>,
    pub outgoing_friend_requests: Vec<FriendRequest>,
    pub active_direct_message_user_id: Option<String>,
    pub direct_typing_users: HashMap<String, Vec<String>>,
    /// DM typing timestamps: user_id → tick when typing started.
    pub direct_typing_ticks: HashMap<String, u64>,
    pub direct_message_threads: Vec<DirectMessageThread>,
    pub pending_messages: Vec<PendingMessage>,
}

#[derive(Debug, Clone, Default)]
pub struct PerfSnapshot {
    pub cpu_percent: f32,
    pub memory_mb: f32,
    pub uptime_secs: u64,
    pub audio_active: bool,
    pub network_connected: bool,
    pub dropped_frames: u64,
    // Audio metrics (M3)
    pub jitter_buffer_ms: u32,
    pub frame_loss_rate: f32,
    pub encode_bitrate_kbps: u32,
    pub decode_peers: u32,
    // Transport (v0.6)
    pub udp_active: bool,
    pub ping_ms: i32,
}

/// Messages between client and signaling server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SignalMessage {
    // Client -> Server
    CreateRoom {
        user_name: String,
        #[serde(default)]
        password: Option<String>,
    },
    JoinRoom {
        room_code: String,
        user_name: String,
        #[serde(default)]
        password: Option<String>,
    },
    LeaveRoom,
    MuteChanged {
        is_muted: bool,
    },
    DeafenChanged {
        is_deafened: bool,
    },
    StartScreenShare,
    StopScreenShare,

    // Server -> Client
    RoomCreated {
        room_code: String,
    },
    RoomJoined {
        room_code: String,
        participants: Vec<ParticipantInfo>,
    },
    PeerJoined {
        peer: ParticipantInfo,
    },
    PeerLeft {
        peer_id: String,
    },
    PeerMuteChanged {
        peer_id: String,
        is_muted: bool,
    },
    PeerDeafenChanged {
        peer_id: String,
        is_deafened: bool,
    },
    ScreenShareStarted {
        sharer_id: String,
        sharer_name: String,
        is_self: bool,
    },
    ScreenShareStopped {
        sharer_id: String,
    },
    Error {
        message: String,
    },

    // Client -> Server (Space)
    CreateSpace {
        name: String,
        user_name: String,
    },
    JoinSpace {
        invite_code: String,
        user_name: String,
    },
    LeaveSpace,
    DeleteSpace,
    RenameSpace {
        name: String,
    },
    SetSpaceDescription {
        description: String,
    },
    SpaceRenamed {
        name: String,
    },
    SpaceDescriptionChanged {
        description: String,
    },
    CreateChannel {
        channel_name: String,
        #[serde(default)]
        channel_type: ChannelType,
        #[serde(default = "default_voice_quality")]
        voice_quality: u8,
    },
    DeleteChannel {
        channel_id: String,
    },
    JoinChannel {
        channel_id: String,
    },
    LeaveChannel,
    SelectTextChannel {
        channel_id: String,
    },
    SetTyping {
        channel_id: String,
        is_typing: bool,
    },
    SendTextMessage {
        channel_id: String,
        content: String,
        #[serde(default)]
        reply_to_message_id: Option<String>,
    },
    PinMessage {
        channel_id: String,
        message_id: String,
        pinned: bool,
    },
    WatchFriendPresence {
        user_ids: Vec<String>,
    },
    SendFriendRequest {
        user_id: String,
    },
    SendFriendRequestByName {
        name: String,
    },
    RespondFriendRequest {
        user_id: String,
        accept: bool,
    },
    CancelFriendRequest {
        user_id: String,
    },
    RemoveFriend {
        user_id: String,
    },
    SelectDirectMessage {
        user_id: String,
    },
    SetDirectTyping {
        user_id: String,
        is_typing: bool,
    },
    SendDirectMessage {
        user_id: String,
        content: String,
        #[serde(default)]
        reply_to_message_id: Option<String>,
    },
    EditDirectMessage {
        user_id: String,
        message_id: String,
        new_content: String,
    },
    DeleteDirectMessage {
        user_id: String,
        message_id: String,
    },
    ReactToDirectMessage {
        user_id: String,
        message_id: String,
        emoji: String,
    },

    // Server -> Client (Space)
    SpaceCreated {
        space: SpaceInfo,
        channels: Vec<ChannelInfo>,
    },
    SpaceJoined {
        space: SpaceInfo,
        channels: Vec<ChannelInfo>,
        members: Vec<MemberInfo>,
        #[serde(default)]
        welcome_message: Option<String>,
    },
    SpaceDeleted,
    ChannelCreated {
        channel: ChannelInfo,
    },
    ChannelDeleted {
        channel_id: String,
    },
    ChannelJoined {
        channel_id: String,
        channel_name: String,
        participants: Vec<ParticipantInfo>,
        #[serde(default = "default_voice_quality")]
        voice_quality: u8,
    },
    ChannelLeft,
    TextChannelSelected {
        channel_id: String,
        channel_name: String,
        history: Vec<TextMessageData>,
    },
    TextMessage {
        channel_id: String,
        message: TextMessageData,
    },
    TypingState {
        channel_id: String,
        user_name: String,
        is_typing: bool,
    },
    MemberOnline {
        member: MemberInfo,
    },
    MemberOffline {
        member_id: String,
    },
    MemberChannelChanged {
        member_id: String,
        channel_id: Option<String>,
        channel_name: Option<String>,
    },

    // Auth (Milestone 4)
    Authenticate {
        token: Option<String>,
        user_name: String,
    },
    Authenticated {
        token: String,
        user_id: String,
    },
    /// Create a new account with email and password.
    CreateAccount {
        email: String,
        password: String,
        display_name: String,
    },
    /// Account created successfully — client should store the token.
    AccountCreated {
        token: String,
        user_id: String,
    },
    /// Login with email and password.
    Login {
        email: String,
        password: String,
    },
    /// Login succeeded — token + user_id + display_name returned.
    LoginSuccess {
        token: String,
        user_id: String,
        display_name: String,
    },
    /// Login or account creation failed.
    AuthError {
        message: String,
    },
    /// Logout — server invalidates the token, client clears local state.
    Logout,
    /// Server confirms logout.
    LoggedOut,
    /// Change password (requires current password).
    ChangePassword {
        current_password: String,
        new_password: String,
    },
    /// Password changed successfully.
    PasswordChanged,
    /// Revoke all sessions — invalidates all tokens, forces re-login everywhere.
    RevokeAllSessions,
    /// Server confirms all sessions were revoked.
    AllSessionsRevoked,
    FriendSnapshot {
        friends: Vec<FavoriteFriend>,
        incoming_requests: Vec<FriendRequest>,
        outgoing_requests: Vec<FriendRequest>,
    },
    DirectMessageSelected {
        user_id: String,
        user_name: String,
        history: Vec<TextMessageData>,
    },
    DirectMessage {
        user_id: String,
        message: TextMessageData,
    },
    DirectTypingState {
        user_id: String,
        user_name: String,
        is_typing: bool,
    },
    DirectMessageEdited {
        user_id: String,
        message_id: String,
        new_content: String,
    },
    DirectMessageDeleted {
        user_id: String,
        message_id: String,
    },
    FriendPresenceSnapshot {
        presences: Vec<FriendPresence>,
    },
    FriendPresenceChanged {
        presence: FriendPresence,
    },

    // Chat improvements (Milestone 5)
    EditTextMessage {
        channel_id: String,
        message_id: String,
        new_content: String,
    },
    DeleteTextMessage {
        channel_id: String,
        message_id: String,
    },
    ReactToMessage {
        channel_id: String,
        message_id: String,
        emoji: String,
    },
    TextMessageEdited {
        channel_id: String,
        message_id: String,
        new_content: String,
    },
    TextMessageDeleted {
        channel_id: String,
        message_id: String,
    },
    MessageReaction {
        channel_id: String,
        message_id: String,
        emoji: String,
        user_name: String,
    },
    DirectMessageReaction {
        user_id: String,
        message_id: String,
        emoji: String,
        user_name: String,
    },
    MessagePinned {
        channel_id: String,
        message_id: String,
        pinned: bool,
    },

    // User status & Channel topics
    SetUserStatus {
        status: String,
    },
    UserStatusChanged {
        member_id: String,
        status: String,
    },
    SetChannelTopic {
        channel_id: String,
        topic: String,
    },
    ChannelTopicChanged {
        channel_id: String,
        topic: String,
    },

    // Moderation (Milestone 6)
    KickMember {
        member_id: String,
    },
    MuteMember {
        member_id: String,
        muted: bool,
    },
    BanMember {
        member_id: String,
    },
    SetMemberRole {
        user_id: String,
        role: SpaceRole,
    },
    Kicked {
        reason: String,
    },
    MemberMuted {
        member_id: String,
        muted: bool,
    },
    /// Moderator -> Server: server-deafen or undeafen a member (stops audio relay TO them).
    ServerDeafenMember {
        member_id: String,
        deafened: bool,
    },
    /// Server -> All in space: a member has been server-deafened or undeafened.
    MemberServerDeafened {
        member_id: String,
        deafened: bool,
    },
    MemberRoleChanged {
        user_id: String,
        role: SpaceRole,
    },
    SpaceAuditLogSnapshot {
        entries: Vec<SpaceAuditEntry>,
    },
    SpaceAuditLogAppended {
        entry: SpaceAuditEntry,
    },

    /// Server is shutting down gracefully
    ServerShutdown,

    // UDP transport negotiation
    /// Client -> Server: Request a UDP session for audio transport.
    /// Server will respond with UdpReady containing the session token and port.
    RequestUdp,
    /// Server -> Client: UDP relay is available. Client should send a UDP
    /// "hello" packet with this token to register its address.
    UdpReady {
        /// 16-char hex string (8 random bytes)
        token: String,
        /// UDP port the server is listening on
        port: u16,
    },
    /// Server -> Client: UDP is not available on this server.
    UdpUnavailable,

    // Channel settings
    SetChannelUserLimit {
        channel_id: String,
        user_limit: u32,
    },
    ChannelUserLimitChanged {
        channel_id: String,
        user_limit: u32,
    },
    SetChannelSlowMode {
        channel_id: String,
        slow_mode_secs: u32,
    },
    ChannelSlowModeChanged {
        channel_id: String,
        slow_mode_secs: u32,
    },
    SetChannelCategory {
        channel_id: String,
        category: String,
    },
    ChannelCategoryChanged {
        channel_id: String,
        category: String,
    },
    SetChannelStatus {
        channel_id: String,
        status: String,
    },
    ChannelStatusChanged {
        channel_id: String,
        status: String,
    },

    // Channel permissions (v0.9.0)
    /// Set minimum role required to access a channel.
    /// `min_role`: "member" (default), "moderator", "admin", "owner"
    SetChannelPermissions {
        channel_id: String,
        min_role: String,
    },
    ChannelPermissionsChanged {
        channel_id: String,
        min_role: String,
    },

    // Channel auto-delete
    /// Set auto-delete interval for a text channel. 0 = disabled.
    SetChannelAutoDelete {
        channel_id: String,
        auto_delete_hours: u32,
    },
    ChannelAutoDeleteChanged {
        channel_id: String,
        auto_delete_hours: u32,
    },

    // Channel ordering (v0.9.0)
    /// Reorder channels in a space. `channel_ids` is the new order from top to bottom.
    ReorderChannels {
        channel_ids: Vec<String>,
    },
    /// Server broadcasts updated channel positions after reorder.
    ChannelsReordered {
        channel_ids: Vec<String>,
    },

    // Priority speaker
    SetPrioritySpeaker {
        peer_id: String,
        enabled: bool,
    },
    PrioritySpeakerChanged {
        peer_id: String,
        enabled: bool,
    },

    // Whisper (targeted private voice)
    WhisperTo {
        target_peer_ids: Vec<String>,
    },
    WhisperStopped,

    // Timeout (timed mute)
    TimeoutMember {
        member_id: String,
        duration_secs: u64,
    },
    MemberTimedOut {
        member_id: String,
        until_epoch: u64,
    },
    MemberTimeoutExpired {
        member_id: String,
    },

    // M10: Message Search
    SearchMessages {
        channel_id: String,
        query: String,
        #[serde(default = "default_search_limit")]
        limit: u32,
    },
    SearchResults {
        channel_id: String,
        messages: Vec<TextMessageData>,
    },
    /// Search all text channels in the current space.
    SearchSpaceMessages {
        query: String,
        #[serde(default = "default_search_limit")]
        limit: u32,
    },
    /// Results from a space-wide search — includes channel_name for context.
    SpaceSearchResults {
        results: Vec<SpaceSearchResult>,
    },

    // M11: User Profiles
    SetProfile {
        bio: String,
    },
    ProfileUpdated {
        user_id: String,
        bio: String,
    },

    // v0.8.0: Status presets
    SetStatusPreset {
        preset: UserStatus,
    },
    StatusPresetChanged {
        member_id: String,
        preset: UserStatus,
    },

    // v0.8.0: @Mention notifications
    MentionNotification {
        channel_id: String,
        channel_name: String,
        sender_name: String,
        preview: String,
    },

    // v0.8.0: Block/Unblock users
    BlockUser {
        user_id: String,
    },
    UnblockUser {
        user_id: String,
    },
    UserBlocked {
        user_id: String,
    },
    UserUnblocked {
        user_id: String,
    },

    // v0.8.0: Ban management
    UnbanMember {
        user_id: String,
    },
    ListBans,
    BanList {
        bans: Vec<BanInfo>,
    },

    // v0.8.0: Group DMs
    CreateGroupDM {
        user_ids: Vec<String>,
        name: Option<String>,
    },
    GroupDMCreated {
        group_id: String,
        name: String,
        members: Vec<String>,
    },
    SendGroupMessage {
        group_id: String,
        content: String,
        #[serde(default)]
        reply_to_message_id: Option<String>,
    },
    GroupMessage {
        group_id: String,
        message: TextMessageData,
    },
    SelectGroupDM {
        group_id: String,
    },
    GroupDMSelected {
        group_id: String,
        name: String,
        members: Vec<String>,
        history: Vec<TextMessageData>,
    },

    // v0.8.0: Invite expiration
    SetInviteSettings {
        expires_hours: Option<u32>,
        max_uses: Option<u32>,
    },
    InviteSettingsUpdated {
        expires_hours: Option<u32>,
        max_uses: Option<u32>,
        uses: u32,
    },

    // v0.8.0: Message threads
    GetThread {
        channel_id: String,
        message_id: String,
    },
    ThreadMessages {
        channel_id: String,
        root_message_id: String,
        messages: Vec<TextMessageData>,
    },

    // v0.8.0: Server nicknames
    SetNickname {
        nickname: String,
    },
    NicknameChanged {
        user_id: String,
        nickname: Option<String>,
    },

    // v0.8.0: Message forwarding
    ForwardMessage {
        source_channel_id: String,
        message_id: String,
        target_channel_id: String,
    },

    // v0.10.0: Auto-moderation word filter
    AddAutomodWord {
        word: String,
        action: String,
    },
    RemoveAutomodWord {
        word: String,
    },
    AutomodWordAdded {
        word: String,
        action: String,
    },
    AutomodWordRemoved {
        word: String,
    },
    ListAutomodWords,
    AutomodWordList {
        words: Vec<AutomodWord>,
    },

    // v0.10.0: Role colors
    /// Set the display color for a role in the current space.
    /// Requires Admin+ permissions. `color` is a hex string like "#ff5555" or empty to clear.
    SetRoleColor {
        role: SpaceRole,
        color: String,
    },
    /// Server broadcasts that a role's color was changed.
    RoleColorChanged {
        role: SpaceRole,
        color: String,
    },

    // v0.10.0: Activity status
    /// Set the user's activity text (e.g. "Playing Valorant"). Empty to clear.
    SetActivity {
        activity: String,
    },
    /// Server broadcasts that a member's activity changed.
    ActivityChanged {
        member_id: String,
        activity: String,
    },

    // DM Voice Calls
    /// Initiate a voice call with a DM partner.
    CallUser {
        target_user_id: String,
    },
    /// Server notifies the target of an incoming call.
    IncomingCall {
        caller_id: String,
        caller_name: String,
        room_key: String,
    },
    /// Accept an incoming call — both peers join the room.
    AcceptCall {
        room_key: String,
    },
    /// Decline or cancel a call.
    DeclineCall {
        room_key: String,
    },
    /// Server notifies that the call was declined, cancelled, or failed.
    CallEnded {
        room_key: String,
        reason: String,
    },

    // v0.10.0: Scheduled Events
    CreateScheduledEvent {
        title: String,
        description: String,
        start_time: i64,
        end_time: i64,
    },
    ScheduledEventCreated {
        event: ScheduledEvent,
    },
    DeleteScheduledEvent {
        event_id: String,
    },
    ScheduledEventDeleted {
        event_id: String,
    },
    ToggleEventInterest {
        event_id: String,
    },
    EventInterestUpdated {
        event_id: String,
        interested_count: u32,
        is_interested: bool,
    },
    ListScheduledEvents,
    ScheduledEventList {
        events: Vec<ScheduledEvent>,
    },

    // v0.10.0: Voice Recording (scaffolding)
    StartRecording {
        channel_id: String,
    },
    StopRecording {
        channel_id: String,
    },
    RecordingStarted {
        channel_id: String,
        started_by: String,
    },
    RecordingStopped {
        channel_id: String,
    },

    // v0.10.0: Message Scheduling (send later)
    ScheduleMessage {
        channel_id: String,
        content: String,
        send_at: i64,
    },
    MessageScheduled {
        schedule_id: String,
        channel_id: String,
        content: String,
        send_at: i64,
    },
    CancelScheduledMessage {
        schedule_id: String,
    },
    ScheduledMessageCancelled {
        schedule_id: String,
    },

    // v0.10.0: Welcome Message
    SetWelcomeMessage {
        message: String,
    },
    WelcomeMessageChanged {
        message: String,
    },

    // v0.10.0: Account management
    DeleteAccount,
    AccountDeleted,
    SetDisplayName {
        name: String,
    },
    DisplayNameChanged {
        user_id: String,
        name: String,
    },

    // Server discovery
    SetSpacePublic {
        is_public: bool,
    },
    SpacePublicChanged {
        is_public: bool,
    },
    BrowsePublicSpaces,
    PublicSpaceList {
        spaces: Vec<PublicSpaceInfo>,
    },

    // Favorite channels
    ToggleFavoriteChannel {
        channel_id: String,
    },

    // Voice notes (async voice messages)
    SendVoiceNote {
        channel_id: String,
        #[serde(default)]
        duration_secs: u32,
        data: Vec<u8>,
    },
    VoiceNote {
        channel_id: String,
        message: TextMessageData,
        #[serde(default)]
        duration_secs: u32,
    },

    // Reactions with user info
    MessageReacted {
        channel_id: String,
        message_id: String,
        emoji: String,
        reactor_name: String,
        count: u32,
    },
}

/// Public space info for the discovery/browse listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicSpaceInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub invite_code: String,
    pub member_count: u32,
    pub channel_count: u32,
    #[serde(default)]
    pub online_count: u32,
}

/// Auto-moderation filter word entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomodWord {
    pub word: String,
    pub action: String,
}

/// A scheduled event in a space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledEvent {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub start_time: i64,
    #[serde(default)]
    pub end_time: i64,
    pub creator_name: String,
    #[serde(default)]
    pub interested_count: u32,
    #[serde(default)]
    pub is_interested: bool,
}

/// Maximum audio frame size in bytes (Opus at 24kbps, 20ms = ~60 bytes typical, 256 max)
pub const MAX_AUDIO_FRAME_SIZE: usize = 4096;
pub const MAX_SCREEN_FRAME_SIZE: usize = 512 * 1024;
pub const MEDIA_PACKET_AUDIO: u8 = 1;
pub const MEDIA_PACKET_SCREEN: u8 = 2;

/// UDP session token length in bytes (random, assigned by server on RequestUdp).
pub const UDP_SESSION_TOKEN_LEN: usize = 8;
/// Default UDP relay port (same as WebSocket port + 1).
pub const UDP_DEFAULT_PORT_OFFSET: u16 = 1;
/// UDP keepalive packet type — sent every 15s to keep NAT mappings alive.
pub const UDP_KEEPALIVE: u8 = 0xFE;
/// Interval between UDP keepalive packets.
pub const UDP_KEEPALIVE_INTERVAL_SECS: u64 = 15;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticipantInfo {
    pub id: String,
    pub name: String,
    pub is_muted: bool,
    #[serde(default)]
    pub is_deafened: bool,
    #[serde(default)]
    pub is_priority_speaker: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextMessageData {
    pub sender_id: String,
    pub sender_name: String,
    pub content: String,
    pub timestamp: u64, // unix seconds
    #[serde(default)]
    pub message_id: String,
    #[serde(default)]
    pub edited: bool,
    #[serde(default)]
    pub reactions: Vec<ReactionData>,
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
    #[serde(default)]
    pub reply_to_sender_name: Option<String>,
    #[serde(default)]
    pub reply_preview: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub forwarded_from: Option<String>,
    #[serde(default)]
    pub attachment_name: Option<String>,
    #[serde(default)]
    pub attachment_size: Option<u32>,
    /// First URL found in message content (for link preview card)
    #[serde(default)]
    pub link_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionData {
    pub emoji: String,
    pub users: Vec<String>,
}

/// A search result from space-wide search, including the originating channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceSearchResult {
    pub channel_id: String,
    pub channel_name: String,
    pub message: TextMessageData,
}

pub const SAMPLE_RATE: u32 = 48000;
pub const CHANNELS: u16 = 1;
pub const FRAME_SIZE: usize = 960; // 20ms at 48kHz

/// Extract the first URL (http:// or https://) from message content.
pub fn extract_first_url(content: &str) -> Option<String> {
    for word in content.split_whitespace() {
        if word.starts_with("http://") || word.starts_with("https://") {
            // Strip trailing punctuation that's likely not part of the URL
            let trimmed = word.trim_end_matches(|c: char| matches!(c, ',' | '.' | ')' | ']' | '>' | ';'));
            return Some(trimmed.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_message_round_trip_create_room() {
        let msg = SignalMessage::CreateRoom {
            user_name: "Alice".into(),
            password: Some("secret".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::CreateRoom {
                user_name,
                password,
            } => {
                assert_eq!(user_name, "Alice");
                assert_eq!(password.as_deref(), Some("secret"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_join_room() {
        let msg = SignalMessage::JoinRoom {
            room_code: "123456".into(),
            user_name: "Bob".into(),
            password: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::JoinRoom {
                room_code,
                user_name,
                password,
            } => {
                assert_eq!(room_code, "123456");
                assert_eq!(user_name, "Bob");
                assert!(password.is_none());
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_leave_room() {
        let msg = SignalMessage::LeaveRoom;
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, SignalMessage::LeaveRoom));
    }

    #[test]
    fn signal_message_round_trip_mute_changed() {
        let msg = SignalMessage::MuteChanged { is_muted: true };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::MuteChanged { is_muted } => assert!(is_muted),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_deafen_changed() {
        let msg = SignalMessage::DeafenChanged { is_deafened: true };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::DeafenChanged { is_deafened } => assert!(is_deafened),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_room_created() {
        let msg = SignalMessage::RoomCreated {
            room_code: "654321".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::RoomCreated { room_code } => assert_eq!(room_code, "654321"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_room_joined() {
        let msg = SignalMessage::RoomJoined {
            room_code: "111111".into(),
            participants: vec![ParticipantInfo {
                id: "p1".into(),
                name: "Alice".into(),
                is_muted: false,
                is_deafened: true,
                is_priority_speaker: false,
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::RoomJoined {
                room_code,
                participants,
            } => {
                assert_eq!(room_code, "111111");
                assert_eq!(participants.len(), 1);
                assert_eq!(participants[0].name, "Alice");
                assert!(participants[0].is_deafened);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_peer_joined() {
        let msg = SignalMessage::PeerJoined {
            peer: ParticipantInfo {
                id: "p2".into(),
                name: "Bob".into(),
                is_muted: true,
                is_deafened: false,
                is_priority_speaker: false,
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::PeerJoined { peer } => {
                assert_eq!(peer.id, "p2");
                assert!(peer.is_muted);
                assert!(!peer.is_deafened);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_peer_left() {
        let msg = SignalMessage::PeerLeft {
            peer_id: "p3".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::PeerLeft { peer_id } => assert_eq!(peer_id, "p3"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_peer_mute_changed() {
        let msg = SignalMessage::PeerMuteChanged {
            peer_id: "p4".into(),
            is_muted: false,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::PeerMuteChanged { peer_id, is_muted } => {
                assert_eq!(peer_id, "p4");
                assert!(!is_muted);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_peer_deafen_changed() {
        let msg = SignalMessage::PeerDeafenChanged {
            peer_id: "p5".into(),
            is_deafened: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::PeerDeafenChanged {
                peer_id,
                is_deafened,
            } => {
                assert_eq!(peer_id, "p5");
                assert!(is_deafened);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_error() {
        let msg = SignalMessage::Error {
            message: "something went wrong".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::Error { message } => assert_eq!(message, "something went wrong"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn participant_info_backward_compat_missing_is_deafened() {
        // Old JSON without is_deafened field should default to false
        let json = r#"{"id":"p1","name":"Alice","is_muted":false}"#;
        let info: ParticipantInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.id, "p1");
        assert_eq!(info.name, "Alice");
        assert!(!info.is_muted);
        assert!(!info.is_deafened); // defaults to false
    }

    #[test]
    fn signal_message_round_trip_create_space() {
        let msg = SignalMessage::CreateSpace {
            name: "My Space".into(),
            user_name: "Alice".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::CreateSpace { name, user_name } => {
                assert_eq!(name, "My Space");
                assert_eq!(user_name, "Alice");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_join_space() {
        let msg = SignalMessage::JoinSpace {
            invite_code: "AbCd1234".into(),
            user_name: "Bob".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::JoinSpace {
                invite_code,
                user_name,
            } => {
                assert_eq!(invite_code, "AbCd1234");
                assert_eq!(user_name, "Bob");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_delete_channel() {
        let msg = SignalMessage::DeleteChannel {
            channel_id: "c42".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::DeleteChannel { channel_id } => assert_eq!(channel_id, "c42"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_channel_deleted() {
        let msg = SignalMessage::ChannelDeleted {
            channel_id: "c42".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::ChannelDeleted { channel_id } => assert_eq!(channel_id, "c42"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_space_created() {
        let msg = SignalMessage::SpaceCreated {
            space: SpaceInfo {
                id: "s1".into(),
                name: "Test Space".into(),
                description: String::new(),
                invite_code: "XyZ12345".into(),
                member_count: 1,
                channel_count: 1,
                is_owner: true,
                self_role: SpaceRole::Owner,
                is_public: false,
            },
            channels: vec![ChannelInfo {
                id: "c1".into(),
                name: "General".into(),
                peer_count: 0,
                channel_type: ChannelType::Voice,
                topic: String::new(),
                voice_quality: 2,
                user_limit: 0,
                category: String::new(),
                status: String::new(),
                slow_mode_secs: 0, position: 0, auto_delete_hours: 0,
                min_role: String::new(),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SpaceCreated { space, channels } => {
                assert_eq!(space.id, "s1");
                assert_eq!(space.name, "Test Space");
                assert_eq!(space.invite_code, "XyZ12345");
                assert_eq!(channels.len(), 1);
                assert_eq!(channels[0].name, "General");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_space_joined() {
        let msg = SignalMessage::SpaceJoined {
            space: SpaceInfo {
                id: "s2".into(),
                name: "Fun Space".into(),
                description: String::new(),
                invite_code: "Abc12345".into(),
                member_count: 2,
                channel_count: 1,
                is_owner: false,
                self_role: SpaceRole::Member,
                is_public: false,
            },
            channels: vec![ChannelInfo {
                id: "c1".into(),
                name: "General".into(),
                peer_count: 1,
                channel_type: ChannelType::Voice,
                topic: String::new(),
                voice_quality: 2,
                user_limit: 0,
                category: String::new(),
                status: String::new(),
                slow_mode_secs: 0, position: 0, auto_delete_hours: 0,
                min_role: String::new(),
            }],
            members: vec![MemberInfo {
                id: "p1".into(),
                user_id: Some("u1".into()),
                name: "Alice".into(),
                role: SpaceRole::Member,
                channel_id: Some("c1".into()),
                channel_name: Some("General".into()),
                status: String::new(),
                bio: String::new(),
                nickname: None,
                status_preset: UserStatus::Online,
                role_color: String::new(),
                activity: String::new(),
            }],
            welcome_message: Some("Welcome to the space!".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SpaceJoined {
                space,
                channels,
                members,
                welcome_message,
            } => {
                assert_eq!(space.id, "s2");
                assert_eq!(space.member_count, 2);
                assert_eq!(channels.len(), 1);
                assert_eq!(members.len(), 1);
                assert_eq!(members[0].channel_id.as_deref(), Some("c1"));
                assert_eq!(welcome_message.as_deref(), Some("Welcome to the space!"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_channel_joined() {
        let msg = SignalMessage::ChannelJoined {
            channel_id: "c1".into(),
            channel_name: "General".into(),
            participants: vec![ParticipantInfo {
                id: "p1".into(),
                name: "Alice".into(),
                is_muted: false,
                is_deafened: false,
                is_priority_speaker: false,
            }],
            voice_quality: 2,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::ChannelJoined {
                channel_id,
                channel_name,
                participants,
                voice_quality: _,
            } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(channel_name, "General");
                assert_eq!(participants.len(), 1);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_watch_friend_presence() {
        let msg = SignalMessage::WatchFriendPresence {
            user_ids: vec!["u1".into(), "u2".into()],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::WatchFriendPresence { user_ids } => {
                assert_eq!(user_ids, vec!["u1", "u2"]);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_send_friend_request() {
        let msg = SignalMessage::SendFriendRequest {
            user_id: "u2".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SendFriendRequest { user_id } => assert_eq!(user_id, "u2"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_friend_snapshot() {
        let msg = SignalMessage::FriendSnapshot {
            friends: vec![FavoriteFriend {
                user_id: "u1".into(),
                name: "Alice".into(),
                is_online: true,
                is_in_voice: false,
                in_private_call: false,
                active_space_name: "Studio".into(),
                active_channel_name: String::new(),
                last_space_name: "Studio".into(),
                last_channel_name: "General".into(),
                last_seen_at: 42,
            }],
            incoming_requests: vec![FriendRequest {
                user_id: "u3".into(),
                name: "Charlie".into(),
                requested_at: 99,
            }],
            outgoing_requests: vec![FriendRequest {
                user_id: "u4".into(),
                name: "Dana".into(),
                requested_at: 123,
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::FriendSnapshot {
                friends,
                incoming_requests,
                outgoing_requests,
            } => {
                assert_eq!(friends.len(), 1);
                assert_eq!(friends[0].user_id, "u1");
                assert_eq!(incoming_requests.len(), 1);
                assert_eq!(incoming_requests[0].user_id, "u3");
                assert_eq!(outgoing_requests.len(), 1);
                assert_eq!(outgoing_requests[0].user_id, "u4");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_send_direct_message() {
        let msg = SignalMessage::SendDirectMessage {
            user_id: "u2".into(),
            content: "hey".into(),
            reply_to_message_id: Some("m1".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SendDirectMessage {
                user_id,
                content,
                reply_to_message_id,
            } => {
                assert_eq!(user_id, "u2");
                assert_eq!(content, "hey");
                assert_eq!(reply_to_message_id.as_deref(), Some("m1"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_direct_message_selected() {
        let msg = SignalMessage::DirectMessageSelected {
            user_id: "u2".into(),
            user_name: "Bob".into(),
            history: vec![TextMessageData {
                sender_id: "u2".into(),
                sender_name: "Bob".into(),
                content: "hello".into(),
                timestamp: 42,
                message_id: "m1".into(),
                edited: false,
                reactions: Vec::new(),
                reply_to_message_id: None,
                reply_to_sender_name: None,
                reply_preview: None,
                pinned: false,
                forwarded_from: None,
                attachment_name: None,
                attachment_size: None,
                link_url: None,
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::DirectMessageSelected {
                user_id,
                user_name,
                history,
            } => {
                assert_eq!(user_id, "u2");
                assert_eq!(user_name, "Bob");
                assert_eq!(history.len(), 1);
                assert_eq!(history[0].content, "hello");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_friend_presence_snapshot() {
        let msg = SignalMessage::FriendPresenceSnapshot {
            presences: vec![FriendPresence {
                user_id: "u1".into(),
                name: "Alice".into(),
                is_online: true,
                is_in_voice: true,
                in_private_call: false,
                active_space_name: Some("Studio".into()),
                active_channel_name: Some("General".into()),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::FriendPresenceSnapshot { presences } => {
                assert_eq!(presences.len(), 1);
                assert_eq!(presences[0].user_id, "u1");
                assert!(presences[0].is_online);
                assert_eq!(presences[0].active_space_name.as_deref(), Some("Studio"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_member_channel_changed() {
        let msg = SignalMessage::MemberChannelChanged {
            member_id: "p1".into(),
            channel_id: Some("c2".into()),
            channel_name: Some("Gaming".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::MemberChannelChanged {
                member_id,
                channel_id,
                channel_name,
            } => {
                assert_eq!(member_id, "p1");
                assert_eq!(channel_id.as_deref(), Some("c2"));
                assert_eq!(channel_name.as_deref(), Some("Gaming"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn constants_are_correct() {
        assert_eq!(SAMPLE_RATE, 48000);
        assert_eq!(CHANNELS, 1);
        assert_eq!(FRAME_SIZE, 960); // 20ms at 48kHz
        assert_eq!(MAX_AUDIO_FRAME_SIZE, 4096);
        assert_eq!(UDP_SESSION_TOKEN_LEN, 8);
        assert_eq!(UDP_DEFAULT_PORT_OFFSET, 1);
    }

    #[test]
    fn signal_message_round_trip_request_udp() {
        let msg = SignalMessage::RequestUdp;
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, SignalMessage::RequestUdp));
    }

    #[test]
    fn signal_message_round_trip_udp_ready() {
        let msg = SignalMessage::UdpReady {
            token: "0123456789abcdef".into(),
            port: 9091,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::UdpReady { token, port } => {
                assert_eq!(token, "0123456789abcdef");
                assert_eq!(port, 9091);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_udp_unavailable() {
        let msg = SignalMessage::UdpUnavailable;
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, SignalMessage::UdpUnavailable));
    }

    #[test]
    fn signal_message_round_trip_channel_user_limit() {
        let msg = SignalMessage::SetChannelUserLimit {
            channel_id: "c1".into(),
            user_limit: 5,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetChannelUserLimit {
                channel_id,
                user_limit,
            } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(user_limit, 5);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_priority_speaker() {
        let msg = SignalMessage::SetPrioritySpeaker {
            peer_id: "p1".into(),
            enabled: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetPrioritySpeaker { peer_id, enabled } => {
                assert_eq!(peer_id, "p1");
                assert!(enabled);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_whisper() {
        let msg = SignalMessage::WhisperTo {
            target_peer_ids: vec!["p1".into(), "p2".into()],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::WhisperTo { target_peer_ids } => {
                assert_eq!(target_peer_ids, vec!["p1", "p2"]);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_timeout_member() {
        let msg = SignalMessage::TimeoutMember {
            member_id: "p1".into(),
            duration_secs: 300,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::TimeoutMember {
                member_id,
                duration_secs,
            } => {
                assert_eq!(member_id, "p1");
                assert_eq!(duration_secs, 300);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_slow_mode() {
        let msg = SignalMessage::SetChannelSlowMode {
            channel_id: "c1".into(),
            slow_mode_secs: 10,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetChannelSlowMode {
                channel_id,
                slow_mode_secs,
            } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(slow_mode_secs, 10);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_channel_status() {
        let msg = SignalMessage::SetChannelStatus {
            channel_id: "c1".into(),
            status: "Playing Valorant".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetChannelStatus {
                channel_id,
                status,
            } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(status, "Playing Valorant");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn channel_info_new_fields_backward_compat() {
        // Old JSON without new fields should default correctly
        let json = r#"{"id":"c1","name":"General","peer_count":0}"#;
        let info: ChannelInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.user_limit, 0);
        assert_eq!(info.category, "");
        assert_eq!(info.status, "");
        assert_eq!(info.slow_mode_secs, 0);
    }

    #[test]
    fn participant_info_priority_speaker_backward_compat() {
        let json = r#"{"id":"p1","name":"Alice","is_muted":false}"#;
        let info: ParticipantInfo = serde_json::from_str(json).unwrap();
        assert!(!info.is_priority_speaker);
    }

    // ─── v0.8.0 tests ───

    #[test]
    fn user_status_serialization_round_trip() {
        for (variant, expected_str) in [
            (UserStatus::Online, "\"Online\""),
            (UserStatus::Idle, "\"Idle\""),
            (UserStatus::DoNotDisturb, "\"DoNotDisturb\""),
            (UserStatus::Invisible, "\"Invisible\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected_str);
            let decoded: UserStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, variant);
        }
    }

    #[test]
    fn ban_info_serialization() {
        let ban = BanInfo {
            user_id: "u42".into(),
            user_name: "Troll".into(),
            banned_at: 1700000000,
        };
        let json = serde_json::to_string(&ban).unwrap();
        let decoded: BanInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.user_id, "u42");
        assert_eq!(decoded.user_name, "Troll");
        assert_eq!(decoded.banned_at, 1700000000);
    }

    #[test]
    fn text_message_data_with_forwarded_from() {
        let msg = TextMessageData {
            sender_id: "u1".into(),
            sender_name: "Alice".into(),
            content: "forwarded content".into(),
            timestamp: 1000,
            message_id: "m99".into(),
            edited: false,
            reactions: Vec::new(),
            reply_to_message_id: None,
            reply_to_sender_name: None,
            reply_preview: None,
            pinned: false,
            forwarded_from: Some("general".into()),
            attachment_name: None,
            attachment_size: None,
            link_url: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: TextMessageData = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.forwarded_from.as_deref(), Some("general"));

        // Backward compat: missing forwarded_from defaults to None
        let old_json = r#"{"sender_id":"u1","sender_name":"A","content":"hi","timestamp":1}"#;
        let old: TextMessageData = serde_json::from_str(old_json).unwrap();
        assert!(old.forwarded_from.is_none());
    }

    #[test]
    fn member_info_with_nickname_and_status_preset() {
        let member = MemberInfo {
            id: "p1".into(),
            user_id: Some("u1".into()),
            name: "Alice".into(),
            role: SpaceRole::Admin,
            channel_id: None,
            channel_name: None,
            status: "custom status".into(),
            bio: "hello world".into(),
            nickname: Some("Ally".into()),
            status_preset: UserStatus::DoNotDisturb,
            role_color: String::new(),
            activity: String::new(),
        };
        let json = serde_json::to_string(&member).unwrap();
        let decoded: MemberInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.nickname.as_deref(), Some("Ally"));
        assert_eq!(decoded.status_preset, UserStatus::DoNotDisturb);

        // Backward compat: missing nickname/status_preset use defaults
        let old_json = r#"{"id":"p1","name":"Bob"}"#;
        let old: MemberInfo = serde_json::from_str(old_json).unwrap();
        assert!(old.nickname.is_none());
        assert_eq!(old.status_preset, UserStatus::Online);
    }

    #[test]
    fn signal_message_round_trip_set_status_preset() {
        let msg = SignalMessage::SetStatusPreset {
            preset: UserStatus::Idle,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetStatusPreset { preset } => assert_eq!(preset, UserStatus::Idle),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_status_preset_changed() {
        let msg = SignalMessage::StatusPresetChanged {
            member_id: "p1".into(),
            preset: UserStatus::DoNotDisturb,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::StatusPresetChanged { member_id, preset } => {
                assert_eq!(member_id, "p1");
                assert_eq!(preset, UserStatus::DoNotDisturb);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_mention_notification() {
        let msg = SignalMessage::MentionNotification {
            channel_id: "c1".into(),
            channel_name: "General".into(),
            sender_name: "Bob".into(),
            preview: "@Alice check this".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::MentionNotification {
                channel_id,
                channel_name,
                sender_name,
                preview,
            } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(channel_name, "General");
                assert_eq!(sender_name, "Bob");
                assert_eq!(preview, "@Alice check this");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_block_unblock() {
        let msg = SignalMessage::BlockUser {
            user_id: "u5".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::BlockUser { user_id } => assert_eq!(user_id, "u5"),
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::UnblockUser {
            user_id: "u5".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::UnblockUser { user_id } => assert_eq!(user_id, "u5"),
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_user_blocked_unblocked() {
        let msg = SignalMessage::UserBlocked {
            user_id: "u5".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(matches!(
            serde_json::from_str::<SignalMessage>(&json).unwrap(),
            SignalMessage::UserBlocked { .. }
        ));

        let msg = SignalMessage::UserUnblocked {
            user_id: "u5".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(matches!(
            serde_json::from_str::<SignalMessage>(&json).unwrap(),
            SignalMessage::UserUnblocked { .. }
        ));
    }

    #[test]
    fn signal_message_round_trip_ban_management() {
        let msg = SignalMessage::UnbanMember {
            user_id: "u3".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::UnbanMember { user_id } => assert_eq!(user_id, "u3"),
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::ListBans;
        let json = serde_json::to_string(&msg).unwrap();
        assert!(matches!(
            serde_json::from_str::<SignalMessage>(&json).unwrap(),
            SignalMessage::ListBans
        ));

        let msg = SignalMessage::BanList {
            bans: vec![BanInfo {
                user_id: "u3".into(),
                user_name: "BadUser".into(),
                banned_at: 999,
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::BanList { bans } => {
                assert_eq!(bans.len(), 1);
                assert_eq!(bans[0].user_id, "u3");
                assert_eq!(bans[0].user_name, "BadUser");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_group_dm() {
        let msg = SignalMessage::CreateGroupDM {
            user_ids: vec!["u1".into(), "u2".into(), "u3".into()],
            name: Some("Squad".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::CreateGroupDM { user_ids, name } => {
                assert_eq!(user_ids, vec!["u1", "u2", "u3"]);
                assert_eq!(name.as_deref(), Some("Squad"));
            }
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::GroupDMCreated {
            group_id: "g1".into(),
            name: "Squad".into(),
            members: vec!["u1".into(), "u2".into()],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::GroupDMCreated {
                group_id,
                name,
                members,
            } => {
                assert_eq!(group_id, "g1");
                assert_eq!(name, "Squad");
                assert_eq!(members.len(), 2);
            }
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::SendGroupMessage {
            group_id: "g1".into(),
            content: "hello group".into(),
            reply_to_message_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SendGroupMessage {
                group_id, content, ..
            } => {
                assert_eq!(group_id, "g1");
                assert_eq!(content, "hello group");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_invite_settings() {
        let msg = SignalMessage::SetInviteSettings {
            expires_hours: Some(24),
            max_uses: Some(10),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetInviteSettings {
                expires_hours,
                max_uses,
            } => {
                assert_eq!(expires_hours, Some(24));
                assert_eq!(max_uses, Some(10));
            }
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::InviteSettingsUpdated {
            expires_hours: Some(48),
            max_uses: None,
            uses: 3,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::InviteSettingsUpdated {
                expires_hours,
                max_uses,
                uses,
            } => {
                assert_eq!(expires_hours, Some(48));
                assert!(max_uses.is_none());
                assert_eq!(uses, 3);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_threads() {
        let msg = SignalMessage::GetThread {
            channel_id: "c1".into(),
            message_id: "m1".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::GetThread {
                channel_id,
                message_id,
            } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(message_id, "m1");
            }
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::ThreadMessages {
            channel_id: "c1".into(),
            root_message_id: "m1".into(),
            messages: vec![],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::ThreadMessages {
                channel_id,
                root_message_id,
                messages,
            } => {
                assert_eq!(channel_id, "c1");
                assert_eq!(root_message_id, "m1");
                assert!(messages.is_empty());
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_nickname() {
        let msg = SignalMessage::SetNickname {
            nickname: "Cool Guy".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::SetNickname { nickname } => assert_eq!(nickname, "Cool Guy"),
            _ => panic!("Wrong variant"),
        }

        let msg = SignalMessage::NicknameChanged {
            user_id: "u1".into(),
            nickname: Some("Ally".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::NicknameChanged { user_id, nickname } => {
                assert_eq!(user_id, "u1");
                assert_eq!(nickname.as_deref(), Some("Ally"));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn signal_message_round_trip_forward_message() {
        let msg = SignalMessage::ForwardMessage {
            source_channel_id: "c1".into(),
            message_id: "m5".into(),
            target_channel_id: "c2".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SignalMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            SignalMessage::ForwardMessage {
                source_channel_id,
                message_id,
                target_channel_id,
            } => {
                assert_eq!(source_channel_id, "c1");
                assert_eq!(message_id, "m5");
                assert_eq!(target_channel_id, "c2");
            }
            _ => panic!("Wrong variant"),
        }
    }
}

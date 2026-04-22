use serde::{Deserialize, Serialize};

use crate::state::{
    default_search_limit, default_voice_quality, BanInfo, ChannelInfo, FavoriteFriend,
    FriendPresence, FriendRequest, MemberInfo, SpaceAuditEntry, SpaceInfo,
};
use crate::view::{ChannelType, SpaceRole, UserStatus};
use crate::message_data::{
    AutomodWord, ParticipantInfo, PublicSpaceInfo, ScheduledEvent, SpaceSearchResult,
    TextMessageData,
};

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
    ScreenShareTransportFeedback {
        frames_completed: u32,
        frames_dropped: u32,
        frames_timed_out: u32,
    },

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

    // Client -> Server: periodic audio quality report for server-side
    // aggregation. All values are client-observed; server only aggregates
    // into /metrics.
    AudioQualityReport {
        /// Client's current capture callback median in milliseconds.
        capture_callback_median_ms: u32,
        /// Client's current playback callback median in milliseconds.
        playback_callback_median_ms: u32,
        /// Delta of audio glitches since the previous report.
        glitches_delta: u32,
        /// Delta of dropped frames since the previous report.
        frames_dropped_delta: u32,
        /// Current jitter-buffer depth in milliseconds.
        jitter_buffer_ms: u32,
    },
}

impl SignalMessage {
    /// Stable numeric index for each variant. Used by the metrics layer to
    /// index a per-variant counter array without allocating a HashMap on
    /// the hot path.
    ///
    /// IMPORTANT: the match below has no `_ =>` arm on purpose — adding
    /// a new variant forces the compiler to flag this function, and once
    /// you give the new variant an index you must also extend
    /// `VARIANT_NAMES` below.
    pub fn variant_index(&self) -> usize {
        match self {
            Self::CreateRoom { .. } => 0,
            Self::JoinRoom { .. } => 1,
            Self::LeaveRoom => 2,
            Self::MuteChanged { .. } => 3,
            Self::DeafenChanged { .. } => 4,
            Self::StartScreenShare => 5,
            Self::StopScreenShare => 6,
            Self::ScreenShareTransportFeedback { .. } => 7,
            Self::RoomCreated { .. } => 8,
            Self::RoomJoined { .. } => 9,
            Self::PeerJoined { .. } => 10,
            Self::PeerLeft { .. } => 11,
            Self::PeerMuteChanged { .. } => 12,
            Self::PeerDeafenChanged { .. } => 13,
            Self::ScreenShareStarted { .. } => 14,
            Self::ScreenShareStopped { .. } => 15,
            Self::Error { .. } => 16,
            Self::CreateSpace { .. } => 17,
            Self::JoinSpace { .. } => 18,
            Self::LeaveSpace => 19,
            Self::DeleteSpace => 20,
            Self::RenameSpace { .. } => 21,
            Self::SetSpaceDescription { .. } => 22,
            Self::SpaceRenamed { .. } => 23,
            Self::SpaceDescriptionChanged { .. } => 24,
            Self::CreateChannel { .. } => 25,
            Self::DeleteChannel { .. } => 26,
            Self::JoinChannel { .. } => 27,
            Self::LeaveChannel => 28,
            Self::SelectTextChannel { .. } => 29,
            Self::SetTyping { .. } => 30,
            Self::SendTextMessage { .. } => 31,
            Self::PinMessage { .. } => 32,
            Self::WatchFriendPresence { .. } => 33,
            Self::SendFriendRequest { .. } => 34,
            Self::SendFriendRequestByName { .. } => 35,
            Self::RespondFriendRequest { .. } => 36,
            Self::CancelFriendRequest { .. } => 37,
            Self::RemoveFriend { .. } => 38,
            Self::SelectDirectMessage { .. } => 39,
            Self::SetDirectTyping { .. } => 40,
            Self::SendDirectMessage { .. } => 41,
            Self::EditDirectMessage { .. } => 42,
            Self::DeleteDirectMessage { .. } => 43,
            Self::ReactToDirectMessage { .. } => 44,
            Self::SpaceCreated { .. } => 45,
            Self::SpaceJoined { .. } => 46,
            Self::SpaceDeleted => 47,
            Self::ChannelCreated { .. } => 48,
            Self::ChannelDeleted { .. } => 49,
            Self::ChannelJoined { .. } => 50,
            Self::ChannelLeft => 51,
            Self::TextChannelSelected { .. } => 52,
            Self::TextMessage { .. } => 53,
            Self::TypingState { .. } => 54,
            Self::MemberOnline { .. } => 55,
            Self::MemberOffline { .. } => 56,
            Self::MemberChannelChanged { .. } => 57,
            Self::Authenticate { .. } => 58,
            Self::Authenticated { .. } => 59,
            Self::CreateAccount { .. } => 60,
            Self::AccountCreated { .. } => 61,
            Self::Login { .. } => 62,
            Self::LoginSuccess { .. } => 63,
            Self::AuthError { .. } => 64,
            Self::Logout => 65,
            Self::LoggedOut => 66,
            Self::ChangePassword { .. } => 67,
            Self::PasswordChanged => 68,
            Self::RevokeAllSessions => 69,
            Self::AllSessionsRevoked => 70,
            Self::FriendSnapshot { .. } => 71,
            Self::DirectMessageSelected { .. } => 72,
            Self::DirectMessage { .. } => 73,
            Self::DirectTypingState { .. } => 74,
            Self::DirectMessageEdited { .. } => 75,
            Self::DirectMessageDeleted { .. } => 76,
            Self::FriendPresenceSnapshot { .. } => 77,
            Self::FriendPresenceChanged { .. } => 78,
            Self::EditTextMessage { .. } => 79,
            Self::DeleteTextMessage { .. } => 80,
            Self::ReactToMessage { .. } => 81,
            Self::TextMessageEdited { .. } => 82,
            Self::TextMessageDeleted { .. } => 83,
            Self::MessageReaction { .. } => 84,
            Self::DirectMessageReaction { .. } => 85,
            Self::MessagePinned { .. } => 86,
            Self::SetUserStatus { .. } => 87,
            Self::UserStatusChanged { .. } => 88,
            Self::SetChannelTopic { .. } => 89,
            Self::ChannelTopicChanged { .. } => 90,
            Self::KickMember { .. } => 91,
            Self::MuteMember { .. } => 92,
            Self::BanMember { .. } => 93,
            Self::SetMemberRole { .. } => 94,
            Self::Kicked { .. } => 95,
            Self::MemberMuted { .. } => 96,
            Self::ServerDeafenMember { .. } => 97,
            Self::MemberServerDeafened { .. } => 98,
            Self::MemberRoleChanged { .. } => 99,
            Self::SpaceAuditLogSnapshot { .. } => 100,
            Self::SpaceAuditLogAppended { .. } => 101,
            Self::ServerShutdown => 102,
            Self::RequestUdp => 103,
            Self::UdpReady { .. } => 104,
            Self::UdpUnavailable => 105,
            Self::SetChannelUserLimit { .. } => 106,
            Self::ChannelUserLimitChanged { .. } => 107,
            Self::SetChannelSlowMode { .. } => 108,
            Self::ChannelSlowModeChanged { .. } => 109,
            Self::SetChannelCategory { .. } => 110,
            Self::ChannelCategoryChanged { .. } => 111,
            Self::SetChannelStatus { .. } => 112,
            Self::ChannelStatusChanged { .. } => 113,
            Self::SetChannelPermissions { .. } => 114,
            Self::ChannelPermissionsChanged { .. } => 115,
            Self::SetChannelAutoDelete { .. } => 116,
            Self::ChannelAutoDeleteChanged { .. } => 117,
            Self::ReorderChannels { .. } => 118,
            Self::ChannelsReordered { .. } => 119,
            Self::SetPrioritySpeaker { .. } => 120,
            Self::PrioritySpeakerChanged { .. } => 121,
            Self::WhisperTo { .. } => 122,
            Self::WhisperStopped => 123,
            Self::TimeoutMember { .. } => 124,
            Self::MemberTimedOut { .. } => 125,
            Self::MemberTimeoutExpired { .. } => 126,
            Self::SearchMessages { .. } => 127,
            Self::SearchResults { .. } => 128,
            Self::SearchSpaceMessages { .. } => 129,
            Self::SpaceSearchResults { .. } => 130,
            Self::SetProfile { .. } => 131,
            Self::ProfileUpdated { .. } => 132,
            Self::SetStatusPreset { .. } => 133,
            Self::StatusPresetChanged { .. } => 134,
            Self::MentionNotification { .. } => 135,
            Self::BlockUser { .. } => 136,
            Self::UnblockUser { .. } => 137,
            Self::UserBlocked { .. } => 138,
            Self::UserUnblocked { .. } => 139,
            Self::UnbanMember { .. } => 140,
            Self::ListBans => 141,
            Self::BanList { .. } => 142,
            Self::CreateGroupDM { .. } => 143,
            Self::GroupDMCreated { .. } => 144,
            Self::SendGroupMessage { .. } => 145,
            Self::GroupMessage { .. } => 146,
            Self::SelectGroupDM { .. } => 147,
            Self::GroupDMSelected { .. } => 148,
            Self::SetInviteSettings { .. } => 149,
            Self::InviteSettingsUpdated { .. } => 150,
            Self::GetThread { .. } => 151,
            Self::ThreadMessages { .. } => 152,
            Self::SetNickname { .. } => 153,
            Self::NicknameChanged { .. } => 154,
            Self::ForwardMessage { .. } => 155,
            Self::AddAutomodWord { .. } => 156,
            Self::RemoveAutomodWord { .. } => 157,
            Self::AutomodWordAdded { .. } => 158,
            Self::AutomodWordRemoved { .. } => 159,
            Self::ListAutomodWords => 160,
            Self::AutomodWordList { .. } => 161,
            Self::SetRoleColor { .. } => 162,
            Self::RoleColorChanged { .. } => 163,
            Self::SetActivity { .. } => 164,
            Self::ActivityChanged { .. } => 165,
            Self::CallUser { .. } => 166,
            Self::IncomingCall { .. } => 167,
            Self::AcceptCall { .. } => 168,
            Self::DeclineCall { .. } => 169,
            Self::CallEnded { .. } => 170,
            Self::CreateScheduledEvent { .. } => 171,
            Self::ScheduledEventCreated { .. } => 172,
            Self::DeleteScheduledEvent { .. } => 173,
            Self::ScheduledEventDeleted { .. } => 174,
            Self::ToggleEventInterest { .. } => 175,
            Self::EventInterestUpdated { .. } => 176,
            Self::ListScheduledEvents => 177,
            Self::ScheduledEventList { .. } => 178,
            Self::StartRecording { .. } => 179,
            Self::StopRecording { .. } => 180,
            Self::RecordingStarted { .. } => 181,
            Self::RecordingStopped { .. } => 182,
            Self::ScheduleMessage { .. } => 183,
            Self::MessageScheduled { .. } => 184,
            Self::CancelScheduledMessage { .. } => 185,
            Self::ScheduledMessageCancelled { .. } => 186,
            Self::SetWelcomeMessage { .. } => 187,
            Self::WelcomeMessageChanged { .. } => 188,
            Self::DeleteAccount => 189,
            Self::AccountDeleted => 190,
            Self::SetDisplayName { .. } => 191,
            Self::DisplayNameChanged { .. } => 192,
            Self::SetSpacePublic { .. } => 193,
            Self::SpacePublicChanged { .. } => 194,
            Self::BrowsePublicSpaces => 195,
            Self::PublicSpaceList { .. } => 196,
            Self::ToggleFavoriteChannel { .. } => 197,
            Self::SendVoiceNote { .. } => 198,
            Self::VoiceNote { .. } => 199,
            Self::MessageReacted { .. } => 200,
            Self::AudioQualityReport { .. } => 201,
        }
    }

    /// Human-readable variant names. Order must exactly match `variant_index`.
    pub const VARIANT_NAMES: &'static [&'static str] = &[
        "CreateRoom",
        "JoinRoom",
        "LeaveRoom",
        "MuteChanged",
        "DeafenChanged",
        "StartScreenShare",
        "StopScreenShare",
        "ScreenShareTransportFeedback",
        "RoomCreated",
        "RoomJoined",
        "PeerJoined",
        "PeerLeft",
        "PeerMuteChanged",
        "PeerDeafenChanged",
        "ScreenShareStarted",
        "ScreenShareStopped",
        "Error",
        "CreateSpace",
        "JoinSpace",
        "LeaveSpace",
        "DeleteSpace",
        "RenameSpace",
        "SetSpaceDescription",
        "SpaceRenamed",
        "SpaceDescriptionChanged",
        "CreateChannel",
        "DeleteChannel",
        "JoinChannel",
        "LeaveChannel",
        "SelectTextChannel",
        "SetTyping",
        "SendTextMessage",
        "PinMessage",
        "WatchFriendPresence",
        "SendFriendRequest",
        "SendFriendRequestByName",
        "RespondFriendRequest",
        "CancelFriendRequest",
        "RemoveFriend",
        "SelectDirectMessage",
        "SetDirectTyping",
        "SendDirectMessage",
        "EditDirectMessage",
        "DeleteDirectMessage",
        "ReactToDirectMessage",
        "SpaceCreated",
        "SpaceJoined",
        "SpaceDeleted",
        "ChannelCreated",
        "ChannelDeleted",
        "ChannelJoined",
        "ChannelLeft",
        "TextChannelSelected",
        "TextMessage",
        "TypingState",
        "MemberOnline",
        "MemberOffline",
        "MemberChannelChanged",
        "Authenticate",
        "Authenticated",
        "CreateAccount",
        "AccountCreated",
        "Login",
        "LoginSuccess",
        "AuthError",
        "Logout",
        "LoggedOut",
        "ChangePassword",
        "PasswordChanged",
        "RevokeAllSessions",
        "AllSessionsRevoked",
        "FriendSnapshot",
        "DirectMessageSelected",
        "DirectMessage",
        "DirectTypingState",
        "DirectMessageEdited",
        "DirectMessageDeleted",
        "FriendPresenceSnapshot",
        "FriendPresenceChanged",
        "EditTextMessage",
        "DeleteTextMessage",
        "ReactToMessage",
        "TextMessageEdited",
        "TextMessageDeleted",
        "MessageReaction",
        "DirectMessageReaction",
        "MessagePinned",
        "SetUserStatus",
        "UserStatusChanged",
        "SetChannelTopic",
        "ChannelTopicChanged",
        "KickMember",
        "MuteMember",
        "BanMember",
        "SetMemberRole",
        "Kicked",
        "MemberMuted",
        "ServerDeafenMember",
        "MemberServerDeafened",
        "MemberRoleChanged",
        "SpaceAuditLogSnapshot",
        "SpaceAuditLogAppended",
        "ServerShutdown",
        "RequestUdp",
        "UdpReady",
        "UdpUnavailable",
        "SetChannelUserLimit",
        "ChannelUserLimitChanged",
        "SetChannelSlowMode",
        "ChannelSlowModeChanged",
        "SetChannelCategory",
        "ChannelCategoryChanged",
        "SetChannelStatus",
        "ChannelStatusChanged",
        "SetChannelPermissions",
        "ChannelPermissionsChanged",
        "SetChannelAutoDelete",
        "ChannelAutoDeleteChanged",
        "ReorderChannels",
        "ChannelsReordered",
        "SetPrioritySpeaker",
        "PrioritySpeakerChanged",
        "WhisperTo",
        "WhisperStopped",
        "TimeoutMember",
        "MemberTimedOut",
        "MemberTimeoutExpired",
        "SearchMessages",
        "SearchResults",
        "SearchSpaceMessages",
        "SpaceSearchResults",
        "SetProfile",
        "ProfileUpdated",
        "SetStatusPreset",
        "StatusPresetChanged",
        "MentionNotification",
        "BlockUser",
        "UnblockUser",
        "UserBlocked",
        "UserUnblocked",
        "UnbanMember",
        "ListBans",
        "BanList",
        "CreateGroupDM",
        "GroupDMCreated",
        "SendGroupMessage",
        "GroupMessage",
        "SelectGroupDM",
        "GroupDMSelected",
        "SetInviteSettings",
        "InviteSettingsUpdated",
        "GetThread",
        "ThreadMessages",
        "SetNickname",
        "NicknameChanged",
        "ForwardMessage",
        "AddAutomodWord",
        "RemoveAutomodWord",
        "AutomodWordAdded",
        "AutomodWordRemoved",
        "ListAutomodWords",
        "AutomodWordList",
        "SetRoleColor",
        "RoleColorChanged",
        "SetActivity",
        "ActivityChanged",
        "CallUser",
        "IncomingCall",
        "AcceptCall",
        "DeclineCall",
        "CallEnded",
        "CreateScheduledEvent",
        "ScheduledEventCreated",
        "DeleteScheduledEvent",
        "ScheduledEventDeleted",
        "ToggleEventInterest",
        "EventInterestUpdated",
        "ListScheduledEvents",
        "ScheduledEventList",
        "StartRecording",
        "StopRecording",
        "RecordingStarted",
        "RecordingStopped",
        "ScheduleMessage",
        "MessageScheduled",
        "CancelScheduledMessage",
        "ScheduledMessageCancelled",
        "SetWelcomeMessage",
        "WelcomeMessageChanged",
        "DeleteAccount",
        "AccountDeleted",
        "SetDisplayName",
        "DisplayNameChanged",
        "SetSpacePublic",
        "SpacePublicChanged",
        "BrowsePublicSpaces",
        "PublicSpaceList",
        "ToggleFavoriteChannel",
        "SendVoiceNote",
        "VoiceNote",
        "MessageReacted",
        "AudioQualityReport",
    ];
}

/// Number of variants in `SignalMessage`. Defined at the module level so
/// it can be used in `const` contexts (e.g., sizing a `[AtomicU64; N]`).
pub const SIGNAL_MESSAGE_VARIANT_COUNT: usize = SignalMessage::VARIANT_NAMES.len();

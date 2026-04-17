use serde::{Deserialize, Serialize};

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

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserStatus {
    #[default]
    Online,
    Idle,
    DoNotDisturb,
    Invisible,
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
pub enum ChannelType {
    #[default]
    Voice,
    Text,
}

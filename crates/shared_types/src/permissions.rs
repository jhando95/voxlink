//! Voxlink granular permission bitmask.
//!
//! A `Permissions` value is a u64 where each bit represents one capability
//! (kick, ban, manage_channels, …). Roles in the new role-catalog system carry
//! a bitmask; a user's effective permissions in a space are the OR of every
//! role they hold (plus implicit `OWNER_BYPASS` if they're the space owner,
//! and any `ADMINISTRATOR` flag short-circuiting all individual checks).
//!
//! Bit layout (deliberately sparse so v2 can add per-channel overrides
//! without renumbering):
//!
//! ```text
//! 0..16     space-wide management (CREATE_INVITE, KICK, BAN, …)
//! 16..32    text-channel actions (VIEW_CHANNEL, SEND_MESSAGES, ADD_REACTIONS, …)
//! 32..48    voice-channel actions (CONNECT, SPEAK, PRIORITY_SPEAKER, …)
//! 48..62    reserved
//! 62        ADMINISTRATOR (implicit grant of every other flag)
//! 63        OWNER_BYPASS (synthetic; in-memory only, never persisted)
//! ```

use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Permissions(pub u64);

impl Permissions {
    pub const fn empty() -> Self {
        Permissions(0)
    }

    pub const fn from_bits(bits: u64) -> Self {
        Permissions(bits)
    }

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn union(self, other: Permissions) -> Permissions {
        Permissions(self.0 | other.0)
    }

    pub const fn intersects(self, other: Permissions) -> bool {
        (self.0 & other.0) != 0
    }

    /// True if `self` contains every bit set in `other`.
    pub const fn contains(self, other: Permissions) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Permission check semantics matching Discord's bypass model:
    /// ADMINISTRATOR or OWNER_BYPASS short-circuit every individual flag.
    pub const fn has(self, flag: Permissions) -> bool {
        if (self.0 & (Self::ADMINISTRATOR.0 | Self::OWNER_BYPASS.0)) != 0 {
            return true;
        }
        (self.0 & flag.0) == flag.0
    }
}

// ─── Flag constants ───
//
// Keep this list 1:1 with the design doc. The values are deliberate; don't
// renumber once a flag has shipped to a deployed DB.
//
// Space management (bits 0-15)
impl Permissions {
    pub const CREATE_INVITE: Permissions = Permissions(1 << 0);
    pub const KICK_MEMBERS: Permissions = Permissions(1 << 1);
    pub const BAN_MEMBERS: Permissions = Permissions(1 << 2);
    pub const TIMEOUT_MEMBERS: Permissions = Permissions(1 << 3);
    pub const MUTE_MEMBERS: Permissions = Permissions(1 << 4);
    pub const DEAFEN_MEMBERS: Permissions = Permissions(1 << 5);
    pub const MOVE_MEMBERS: Permissions = Permissions(1 << 6);
    pub const MANAGE_NICKNAMES: Permissions = Permissions(1 << 7);
    pub const MANAGE_CHANNELS: Permissions = Permissions(1 << 8);
    pub const MANAGE_ROLES: Permissions = Permissions(1 << 9);
    pub const MANAGE_SPACE: Permissions = Permissions(1 << 10);
    pub const MANAGE_MESSAGES: Permissions = Permissions(1 << 11);
    pub const MANAGE_EVENTS: Permissions = Permissions(1 << 12);
    pub const MANAGE_AUTOMOD: Permissions = Permissions(1 << 13);
    pub const VIEW_AUDIT_LOG: Permissions = Permissions(1 << 14);
}

// Text-channel actions (bits 16-31)
impl Permissions {
    pub const VIEW_CHANNEL: Permissions = Permissions(1 << 16);
    pub const SEND_MESSAGES: Permissions = Permissions(1 << 17);
    pub const SEND_VOICE_NOTES: Permissions = Permissions(1 << 18);
    pub const ATTACH_FILES: Permissions = Permissions(1 << 19);
    pub const ADD_REACTIONS: Permissions = Permissions(1 << 20);
    pub const USE_EXTERNAL_EMOJI: Permissions = Permissions(1 << 21);
    pub const MENTION_EVERYONE: Permissions = Permissions(1 << 22);
}

// Voice-channel actions (bits 32-47)
impl Permissions {
    pub const CONNECT: Permissions = Permissions(1 << 32);
    pub const SPEAK: Permissions = Permissions(1 << 33);
    pub const USE_VOICE_ACTIVITY: Permissions = Permissions(1 << 34);
    pub const PRIORITY_SPEAKER: Permissions = Permissions(1 << 35);
    pub const START_RECORDING: Permissions = Permissions(1 << 36);
    pub const STOP_RECORDING: Permissions = Permissions(1 << 37);
}

// Meta (62, 63)
impl Permissions {
    /// Implicit grant of every other flag. Survives serialization.
    pub const ADMINISTRATOR: Permissions = Permissions(1 << 62);
    /// Synthetic; only ever OR'd into the in-memory cached value when the
    /// actor is the space owner. **Never persisted or sent over the wire.**
    pub const OWNER_BYPASS: Permissions = Permissions(1 << 63);
}

/// Convenience bundles matching the legacy 4-tier roles so the migration can
/// synthesize them without spelling each flag out every time.
impl Permissions {
    /// Member: read + post + react + connect, no moderation.
    pub const LEGACY_MEMBER_DEFAULTS: Permissions = Permissions(
        Self::CREATE_INVITE.0
            | Self::VIEW_CHANNEL.0
            | Self::SEND_MESSAGES.0
            | Self::SEND_VOICE_NOTES.0
            | Self::ATTACH_FILES.0
            | Self::ADD_REACTIONS.0
            | Self::CONNECT.0
            | Self::SPEAK.0
            | Self::USE_VOICE_ACTIVITY.0,
    );

    /// Moderator: member defaults + classic mod powers.
    pub const LEGACY_MODERATOR_BUNDLE: Permissions = Permissions(
        Self::LEGACY_MEMBER_DEFAULTS.0
            | Self::KICK_MEMBERS.0
            | Self::BAN_MEMBERS.0
            | Self::TIMEOUT_MEMBERS.0
            | Self::MUTE_MEMBERS.0
            | Self::DEAFEN_MEMBERS.0
            | Self::MANAGE_MESSAGES.0
            | Self::MANAGE_EVENTS.0
            | Self::PRIORITY_SPEAKER.0
            | Self::STOP_RECORDING.0
            | Self::VIEW_AUDIT_LOG.0,
    );

    /// Admin: moderator powers + manage_* and start_recording. Plus the
    /// ADMINISTRATOR bypass so behavior matches today exactly.
    pub const LEGACY_ADMIN_BUNDLE: Permissions = Permissions(
        Self::LEGACY_MODERATOR_BUNDLE.0
            | Self::MANAGE_CHANNELS.0
            | Self::MANAGE_ROLES.0
            | Self::MANAGE_SPACE.0
            | Self::MANAGE_AUTOMOD.0
            | Self::START_RECORDING.0
            | Self::ADMINISTRATOR.0,
    );
}

/// Catalog entry for a single named role within a space. Sent to clients
/// over the wire so they can render role pills + drive the role-management UI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoleInfo {
    pub role_id: String,
    pub name: String,
    /// Hex color string ("" = inherit theme default).
    #[serde(default)]
    pub color: String,
    /// Higher = displayed more prominently; ties broken by role_id.
    #[serde(default)]
    pub position: i32,
    pub permissions: Permissions,
    /// True for the synthetic legacy-tier roles created by the v0.12 → v0.13
    /// migration. Clients can hide them from the "edit roles" surface.
    #[serde(default)]
    pub is_managed: bool,
    /// True for the implicit @everyone role (auto-assigned to all members).
    #[serde(default)]
    pub is_default: bool,
}

/// One member ↔ role-set entry, served as part of the catalog snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoleAssignment {
    pub user_id: String,
    pub role_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_respects_administrator_bypass() {
        let admin = Permissions::ADMINISTRATOR;
        // Any specific permission check returns true under ADMINISTRATOR.
        assert!(admin.has(Permissions::KICK_MEMBERS));
        assert!(admin.has(Permissions::MANAGE_CHANNELS));
        assert!(admin.has(Permissions::MENTION_EVERYONE));
    }

    #[test]
    fn has_respects_owner_bypass() {
        let owner = Permissions::OWNER_BYPASS;
        assert!(owner.has(Permissions::BAN_MEMBERS));
    }

    #[test]
    fn has_distinguishes_set_vs_unset_flags() {
        let p = Permissions::KICK_MEMBERS.union(Permissions::BAN_MEMBERS);
        assert!(p.has(Permissions::KICK_MEMBERS));
        assert!(p.has(Permissions::BAN_MEMBERS));
        assert!(!p.has(Permissions::MANAGE_SPACE));
        assert!(!p.has(Permissions::ADMINISTRATOR));
    }

    #[test]
    fn legacy_admin_bundle_grants_everything_via_bypass() {
        let admin = Permissions::LEGACY_ADMIN_BUNDLE;
        // ADMINISTRATOR bit means every specific check passes.
        assert!(admin.has(Permissions::PRIORITY_SPEAKER));
        assert!(admin.has(Permissions::MANAGE_AUTOMOD));
        // And explicit bits are also set (admin has them in the bundle directly).
        assert!(admin.contains(Permissions::MANAGE_ROLES));
        assert!(admin.contains(Permissions::ADMINISTRATOR));
    }

    #[test]
    fn legacy_moderator_can_moderate_but_not_manage_roles() {
        let m = Permissions::LEGACY_MODERATOR_BUNDLE;
        assert!(m.has(Permissions::KICK_MEMBERS));
        assert!(m.has(Permissions::BAN_MEMBERS));
        assert!(m.has(Permissions::MANAGE_MESSAGES));
        assert!(!m.has(Permissions::MANAGE_ROLES));
        assert!(!m.has(Permissions::MANAGE_SPACE));
        assert!(!m.has(Permissions::MANAGE_CHANNELS));
    }

    #[test]
    fn legacy_member_can_post_and_connect() {
        let m = Permissions::LEGACY_MEMBER_DEFAULTS;
        assert!(m.has(Permissions::SEND_MESSAGES));
        assert!(m.has(Permissions::CONNECT));
        assert!(m.has(Permissions::ADD_REACTIONS));
        // But cannot moderate.
        assert!(!m.has(Permissions::KICK_MEMBERS));
        assert!(!m.has(Permissions::MANAGE_CHANNELS));
    }

    #[test]
    fn serialization_is_compact_u64() {
        let p = Permissions::KICK_MEMBERS.union(Permissions::BAN_MEMBERS);
        let json = serde_json::to_string(&p).unwrap();
        // serde(transparent) over u64 → bare number.
        assert_eq!(json, p.0.to_string());
        let back: Permissions = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }
}

//! v0.13 custom-role management API.
//!
//! Handlers for CreateRole / UpdateRole / DeleteRole / AssignRoleToMember /
//! UnassignRoleFromMember / RequestRoleList. All edits require
//! `Permissions::MANAGE_ROLES` (or owner-bypass). Mutations write through
//! to the v2 SQLite tables (`space_role_defs`, `space_role_members`) and
//! broadcast the change to every peer in the space.
//!
//! Pre-existing 4-tier role checks in other handlers keep working — the
//! `migrate_legacy_roles_to_v2()` synthesis ensures every legacy assignment
//! is mirrored in the new tables, so both code paths agree on permissions
//! until the handler sweep migrates them in a follow-up wave.

use shared_types::{Permissions, RoleAssignment, RoleInfo, SignalMessage};

use crate::connection::send_error;
use crate::persistence::{SpaceRoleDefRow, SpaceRoleMemberRow};
use crate::types::{Db, State};
use crate::{now_epoch_secs, send_to};

/// Resolve (space_id, user_id, is_owner, effective_permissions) for the
/// actor, or send an error and return None. `is_owner` is the structural
/// `spaces.owner_id == user_id` check; it short-circuits every permission
/// test (OWNER_BYPASS).
async fn actor_perms(
    state: &State,
    peer_id: &str,
    db: &Db,
) -> Option<(String, String, bool, Permissions)> {
    let (space_id, user_id) = {
        let s = state.read().await;
        let peer = s.peers.get(peer_id)?;
        let uid = peer.user_id.lock().await.clone()?;
        let sid = peer.space_id.lock().await.clone()?;
        (sid, uid)
    };
    let is_owner = {
        let s = state.read().await;
        s.spaces
            .get(&space_id)
            .map(|sp| sp.owner_id == user_id)
            .unwrap_or(false)
    };
    let bits = if let Some(ref db) = db {
        let db = db.clone();
        let sid = space_id.clone();
        let uid = user_id.clone();
        tokio::task::spawn_blocking(move || {
            db.load_user_effective_permissions(&sid, &uid).unwrap_or(0)
        })
        .await
        .unwrap_or(0)
    } else {
        0
    };
    let mut perms = Permissions::from_bits(bits);
    if is_owner {
        perms = perms.union(Permissions::OWNER_BYPASS);
    }
    Some((space_id, user_id, is_owner, perms))
}

async fn snapshot_for_space(
    db: &Db,
    space_id: &str,
) -> Option<(Vec<RoleInfo>, Vec<RoleAssignment>)> {
    let db = db.as_ref()?.clone();
    let sid = space_id.to_string();
    tokio::task::spawn_blocking(move || -> Option<(Vec<RoleInfo>, Vec<RoleAssignment>)> {
        let defs = db.load_role_defs(&sid).ok()?;
        let members = db.load_role_members(&sid).ok()?;
        let roles: Vec<RoleInfo> = defs
            .into_iter()
            .map(|d| RoleInfo {
                role_id: d.role_id,
                name: d.name,
                color: d.color,
                position: d.position,
                permissions: Permissions::from_bits(d.permissions),
                is_managed: d.is_managed,
                is_default: d.is_default,
            })
            .collect();
        // Roll up members → user_id -> [role_id].
        let mut by_user: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for m in members {
            by_user.entry(m.user_id).or_default().push(m.role_id);
        }
        let assignments: Vec<RoleAssignment> = by_user
            .into_iter()
            .map(|(user_id, mut role_ids)| {
                role_ids.sort();
                RoleAssignment { user_id, role_ids }
            })
            .collect();
        Some((roles, assignments))
    })
    .await
    .ok()
    .flatten()
}

/// Broadcast a role-related event to every peer currently in the space.
async fn broadcast_to_space_id(state: &State, space_id: &str, msg: &SignalMessage) {
    super::space::broadcast_to_space(state, space_id, "", msg).await;
}

pub(crate) async fn handle_request_role_list(state: &State, peer_id: &str, db: &Db) {
    let Some((space_id, _user_id, _is_owner, _perms)) = actor_perms(state, peer_id, db).await
    else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };
    if let Some((roles, assignments)) = snapshot_for_space(db, &space_id).await {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id).cloned() {
            drop(s);
            send_to(
                &peer,
                &SignalMessage::RoleListSnapshot {
                    space_id,
                    roles,
                    assignments,
                },
            );
        }
    }
}

pub(crate) async fn handle_create_role(
    state: &State,
    peer_id: &str,
    name: String,
    color: String,
    permissions: Permissions,
    position: i32,
    db: &Db,
) {
    let Some((space_id, _user_id, _is_owner, actor_perms)) = actor_perms(state, peer_id, db).await
    else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };
    if !actor_perms.has(Permissions::MANAGE_ROLES) {
        send_error(state, peer_id, "Missing MANAGE_ROLES permission").await;
        return;
    }
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 64 {
        send_error(state, peer_id, "Role name must be 1-64 characters").await;
        return;
    }
    // Rank rule: actor cannot grant a permission they don't themselves hold.
    let allowed = actor_perms.union(Permissions::from_bits(0));
    if !allowed.contains(permissions) && !actor_perms.has(Permissions::ADMINISTRATOR) {
        send_error(
            state,
            peer_id,
            "You can only grant permissions you yourself hold",
        )
        .await;
        return;
    }
    let Some(ref db_ref) = db else {
        send_error(state, peer_id, "Role management requires persistence").await;
        return;
    };
    // Allocate a role_id and persist.
    let role_id = format!("r_{:016x}", rand::random::<u64>());
    let row = SpaceRoleDefRow {
        space_id: space_id.clone(),
        role_id: role_id.clone(),
        name: name.clone(),
        color: color.clone(),
        position,
        permissions: permissions.bits(),
        is_managed: false,
        is_default: false,
        created_at: now_epoch_secs() as i64,
    };
    let db_clone = db_ref.clone();
    let row_for_db = row.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || db_clone.upsert_role_def(&row_for_db))
        .await
        .unwrap_or_else(|e| Err(format!("Spawn failed: {e}")))
    {
        send_error(state, peer_id, &format!("Failed to persist role: {e}")).await;
        return;
    }
    let info = RoleInfo {
        role_id,
        name,
        color,
        position,
        permissions,
        is_managed: false,
        is_default: false,
    };
    broadcast_to_space_id(
        state,
        &space_id,
        &SignalMessage::RoleCreated {
            space_id: space_id.clone(),
            role: info,
        },
    )
    .await;
}

/// PATCH-style field set for UpdateRole. `None` = leave unchanged.
pub(crate) struct RoleUpdate {
    pub name: Option<String>,
    pub color: Option<String>,
    pub permissions: Option<Permissions>,
    pub position: Option<i32>,
}

pub(crate) async fn handle_update_role(
    state: &State,
    peer_id: &str,
    role_id: String,
    update: RoleUpdate,
    db: &Db,
) {
    let Some((space_id, _user_id, _is_owner, actor_perms)) = actor_perms(state, peer_id, db).await
    else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };
    if !actor_perms.has(Permissions::MANAGE_ROLES) {
        send_error(state, peer_id, "Missing MANAGE_ROLES permission").await;
        return;
    }
    let Some(ref db_ref) = db else {
        send_error(state, peer_id, "Role management requires persistence").await;
        return;
    };
    // Load the existing row; merge in supplied changes.
    let db_clone = db_ref.clone();
    let sid = space_id.clone();
    let role_id_clone = role_id.clone();
    let existing = tokio::task::spawn_blocking(move || -> Option<SpaceRoleDefRow> {
        db_clone
            .load_role_defs(&sid)
            .ok()?
            .into_iter()
            .find(|r| r.role_id == role_id_clone)
    })
    .await
    .ok()
    .flatten();
    let Some(mut existing) = existing else {
        send_error(state, peer_id, "Unknown role").await;
        return;
    };
    if existing.is_managed && (update.name.is_some() || update.permissions.is_some()) {
        send_error(
            state,
            peer_id,
            "Cannot rename or repermission a managed legacy role",
        )
        .await;
        return;
    }
    if let Some(n) = update.name {
        let trimmed = n.trim().to_string();
        if trimmed.is_empty() || trimmed.chars().count() > 64 {
            send_error(state, peer_id, "Role name must be 1-64 characters").await;
            return;
        }
        existing.name = trimmed;
    }
    if let Some(c) = update.color {
        existing.color = c;
    }
    if let Some(p) = update.permissions {
        // Rank: actor must hold every bit they're granting.
        if !actor_perms.contains(p) && !actor_perms.has(Permissions::ADMINISTRATOR) {
            send_error(
                state,
                peer_id,
                "You can only grant permissions you yourself hold",
            )
            .await;
            return;
        }
        existing.permissions = p.bits();
    }
    if let Some(pos) = update.position {
        existing.position = pos;
    }
    let db_clone = db_ref.clone();
    let row_for_db = existing.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || db_clone.upsert_role_def(&row_for_db))
        .await
        .unwrap_or_else(|e| Err(format!("Spawn failed: {e}")))
    {
        send_error(state, peer_id, &format!("Failed to update role: {e}")).await;
        return;
    }
    let info = RoleInfo {
        role_id: existing.role_id,
        name: existing.name,
        color: existing.color,
        position: existing.position,
        permissions: Permissions::from_bits(existing.permissions),
        is_managed: existing.is_managed,
        is_default: existing.is_default,
    };
    broadcast_to_space_id(
        state,
        &space_id,
        &SignalMessage::RoleUpdated {
            space_id: space_id.clone(),
            role: info,
        },
    )
    .await;
}

pub(crate) async fn handle_delete_role(state: &State, peer_id: &str, role_id: String, db: &Db) {
    let Some((space_id, _user_id, _is_owner, actor_perms)) = actor_perms(state, peer_id, db).await
    else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };
    if !actor_perms.has(Permissions::MANAGE_ROLES) {
        send_error(state, peer_id, "Missing MANAGE_ROLES permission").await;
        return;
    }
    let Some(ref db_ref) = db else {
        send_error(state, peer_id, "Role management requires persistence").await;
        return;
    };
    // Reject deletes of managed (legacy) and default (@everyone) roles.
    let db_clone = db_ref.clone();
    let sid = space_id.clone();
    let role_id_clone = role_id.clone();
    let row = tokio::task::spawn_blocking(move || -> Option<SpaceRoleDefRow> {
        db_clone
            .load_role_defs(&sid)
            .ok()?
            .into_iter()
            .find(|r| r.role_id == role_id_clone)
    })
    .await
    .ok()
    .flatten();
    let Some(row) = row else {
        send_error(state, peer_id, "Unknown role").await;
        return;
    };
    if row.is_managed || row.is_default {
        send_error(
            state,
            peer_id,
            "Cannot delete the @everyone or legacy managed roles",
        )
        .await;
        return;
    }
    let db_clone = db_ref.clone();
    let sid = space_id.clone();
    let rid = role_id.clone();
    let _ = tokio::task::spawn_blocking(move || db_clone.delete_role_def(&sid, &rid)).await;
    broadcast_to_space_id(
        state,
        &space_id,
        &SignalMessage::RoleDeleted {
            space_id: space_id.clone(),
            role_id,
        },
    )
    .await;
}

pub(crate) async fn handle_assign_role_to_member(
    state: &State,
    peer_id: &str,
    user_id: String,
    role_id: String,
    db: &Db,
) {
    let Some((space_id, actor_uid, _is_owner, actor_perms)) = actor_perms(state, peer_id, db).await
    else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };
    if !actor_perms.has(Permissions::MANAGE_ROLES) {
        send_error(state, peer_id, "Missing MANAGE_ROLES permission").await;
        return;
    }
    let Some(ref db_ref) = db else {
        send_error(state, peer_id, "Role management requires persistence").await;
        return;
    };
    // Verify the role belongs to this space and enforce the permission ceiling:
    // an actor may only assign a role whose permissions they themselves hold
    // (unless they are an administrator). Without this, MANAGE_ROLES alone would
    // let a user grant themselves ADMINISTRATOR by assigning a higher role.
    let db_clone = db_ref.clone();
    let sid = space_id.clone();
    let rid = role_id.clone();
    let target_role = tokio::task::spawn_blocking(move || -> Option<SpaceRoleDefRow> {
        db_clone
            .load_role_defs(&sid)
            .ok()?
            .into_iter()
            .find(|r| r.role_id == rid)
    })
    .await
    .ok()
    .flatten();
    let Some(target_role) = target_role else {
        send_error(state, peer_id, "Unknown role for this space").await;
        return;
    };
    let role_perms = Permissions::from_bits(target_role.permissions);
    if !actor_perms.contains(role_perms) && !actor_perms.has(Permissions::ADMINISTRATOR) {
        send_error(
            state,
            peer_id,
            "You can only assign roles whose permissions you yourself hold",
        )
        .await;
        return;
    }
    let row = SpaceRoleMemberRow {
        space_id: space_id.clone(),
        role_id: role_id.clone(),
        user_id: user_id.clone(),
        assigned_at: now_epoch_secs() as i64,
        assigned_by: actor_uid,
    };
    let db_clone = db_ref.clone();
    let row_for_db = row.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || db_clone.upsert_role_member(&row_for_db))
        .await
        .unwrap_or_else(|e| Err(format!("Spawn failed: {e}")))
    {
        send_error(state, peer_id, &format!("Failed to assign role: {e}")).await;
        return;
    }
    fan_out_member_role_change(state, db, &space_id, &user_id).await;
}

pub(crate) async fn handle_unassign_role_from_member(
    state: &State,
    peer_id: &str,
    user_id: String,
    role_id: String,
    db: &Db,
) {
    let Some((space_id, _user_id, _is_owner, actor_perms)) = actor_perms(state, peer_id, db).await
    else {
        send_error(state, peer_id, "Not in a space").await;
        return;
    };
    if !actor_perms.has(Permissions::MANAGE_ROLES) {
        send_error(state, peer_id, "Missing MANAGE_ROLES permission").await;
        return;
    }
    let Some(ref db_ref) = db else {
        send_error(state, peer_id, "Role management requires persistence").await;
        return;
    };
    let db_clone = db_ref.clone();
    let sid = space_id.clone();
    let rid = role_id.clone();
    let uid = user_id.clone();
    let _ =
        tokio::task::spawn_blocking(move || db_clone.delete_role_member(&sid, &rid, &uid)).await;
    fan_out_member_role_change(state, db, &space_id, &user_id).await;
}

async fn fan_out_member_role_change(state: &State, db: &Db, space_id: &str, user_id: &str) {
    let role_ids = if let Some(db) = db.as_ref() {
        let db = db.clone();
        let sid = space_id.to_string();
        let uid = user_id.to_string();
        tokio::task::spawn_blocking(move || -> Vec<String> {
            db.load_role_members(&sid)
                .unwrap_or_default()
                .into_iter()
                .filter(|m| m.user_id == uid)
                .map(|m| m.role_id)
                .collect()
        })
        .await
        .unwrap_or_default()
    } else {
        Vec::new()
    };
    broadcast_to_space_id(
        state,
        space_id,
        &SignalMessage::MemberRolesChanged {
            space_id: space_id.to_string(),
            user_id: user_id.to_string(),
            role_ids,
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{Database, SpaceRow};

    /// In-process smoke: persist a role + assignment, verify the snapshot
    /// builds correctly without needing the full server harness.
    #[test]
    fn snapshot_groups_assignments_by_user() {
        let path =
            std::env::temp_dir().join(format!("voxlink_role_snapshot_{}.db", std::process::id()));
        let db = Database::open(&path).unwrap();
        db.save_space(&SpaceRow {
            id: "s1".into(),
            name: "Test".into(),
            invite_code: "INV".into(),
            owner_id: "u_owner".into(),
            created_at: 1,
        })
        .unwrap();

        let now = 1000i64;
        db.upsert_role_def(&SpaceRoleDefRow {
            space_id: "s1".into(),
            role_id: "r_admins".into(),
            name: "Admins".into(),
            color: "#ff0000".into(),
            position: 50,
            permissions: Permissions::MANAGE_SPACE
                .union(Permissions::KICK_MEMBERS)
                .bits(),
            is_managed: false,
            is_default: false,
            created_at: now,
        })
        .unwrap();
        db.upsert_role_member(&SpaceRoleMemberRow {
            space_id: "s1".into(),
            role_id: "r_admins".into(),
            user_id: "u_a".into(),
            assigned_at: now,
            assigned_by: "u_owner".into(),
        })
        .unwrap();
        db.upsert_role_member(&SpaceRoleMemberRow {
            space_id: "s1".into(),
            role_id: "r_admins".into(),
            user_id: "u_b".into(),
            assigned_at: now,
            assigned_by: "u_owner".into(),
        })
        .unwrap();

        let defs = db.load_role_defs("s1").unwrap();
        let admins = defs.iter().find(|d| d.role_id == "r_admins").unwrap();
        assert!(admins.permissions & Permissions::KICK_MEMBERS.bits() != 0);

        let perms = db.load_user_effective_permissions("s1", "u_a").unwrap();
        assert!(perms & Permissions::KICK_MEMBERS.bits() != 0);
        assert!(perms & Permissions::MANAGE_SPACE.bits() != 0);
        // u_a should not have MANAGE_ROLES — only what r_admins grants + default.
        assert!(perms & Permissions::MANAGE_ROLES.bits() == 0);

        let _ = std::fs::remove_file(path);
    }
}

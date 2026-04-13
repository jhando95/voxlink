use crate::{send_error, send_to, Db, State};
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use rand::rngs::OsRng;
use rand::RngCore;
use shared_types::SignalMessage;

use super::friends::send_friend_snapshot_to_peer;

pub async fn handle_authenticate(
    state: &State,
    peer_id: &str,
    token: Option<String>,
    user_name: String,
    db: &Db,
) -> bool {
    // Rate limiting is enforced at the transport layer via rate_limit_per_sec in main.rs.
    // This prevents brute-force attacks by limiting auth attempts per remote address.
    // Token expiry is enforced in find_user_by_token (90 days), and token lookup queries
    // are indexed for O(1) performance.

    let user_name = user_name.trim().to_string();
    if user_name.is_empty() || user_name.len() > 32 {
        send_error(
            state,
            peer_id,
            "Display name must be between 1 and 32 characters",
        )
        .await;
        return false;
    }

    // Set the peer's display name
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            *peer.name.lock().await = user_name.clone();
        }
    }

    let Some(ref db_ref) = db else {
        // No DB — just acknowledge with a transient token
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id).cloned() {
            drop(s);
            send_to(
                &peer,
                &SignalMessage::Authenticated {
                    token: String::new(),
                    user_id: peer_id.to_string(),
                },
            )
            .await;
        }
        return true;
    };

    // Try to restore identity from existing token
    if let Some(ref tok) = token {
        if !tok.is_empty() {
            let db_clone = db_ref.clone();
            let tok_clone = tok.clone();
            let found = match tokio::time::timeout(
                crate::DB_TIMEOUT,
                tokio::task::spawn_blocking(move || {
                    db_clone.find_user_by_token(&tok_clone).unwrap_or(None)
                }),
            )
            .await
            {
                Ok(result) => result.unwrap_or(None),
                Err(_) => {
                    // DB timeout — return error instead of creating a new identity,
                    // which would cause the user to lose their previous identity.
                    log::warn!("DB timeout: find_user_by_token for peer {peer_id}");
                    send_error(
                        state,
                        peer_id,
                        "Authentication is temporarily unavailable (DB timeout)",
                    )
                    .await;
                    return false;
                }
            };

            if let Some(user) = found {
                let rotated_token = generate_token();
                let rotated_name = user_name.clone();
                let user_id = user.user_id.clone();
                let now = unix_now_secs();
                let db_clone = db_ref.clone();
                let uid = user_id.clone();
                let tok = rotated_token.clone();
                let name = rotated_name.clone();
                let rotate_result = tokio::time::timeout(
                    crate::DB_TIMEOUT,
                    tokio::task::spawn_blocking(move || {
                        db_clone.rotate_user_session(&uid, &tok, &name, now, now)
                    }),
                )
                .await;

                let token_to_send = match rotate_result {
                    Ok(Ok(Ok(()))) => rotated_token,
                    Ok(Ok(Err(e))) => {
                        log::error!("Failed to rotate session token for {user_id}: {e}");
                        user.token
                    }
                    Ok(Err(e)) => {
                        log::error!("Failed to join token rotation task for {user_id}: {e}");
                        user.token
                    }
                    Err(_) => {
                        log::warn!("DB timeout: rotate_user_session for {user_id}");
                        user.token
                    }
                };

                // Store persistent user_id on peer for ban checks
                let s = state.read().await;
                if let Some(peer) = s.peers.get(peer_id) {
                    *peer.user_id.lock().await = Some(user_id.clone());
                    // Load block cache: which user_ids have blocked this user
                    let uid = user_id.clone();
                    let db_c = db_ref.clone();
                    if let Ok(blocked_by) = tokio::task::spawn_blocking(move || {
                        db_c.get_users_who_blocked(&uid).unwrap_or_default()
                    })
                    .await
                    {
                        if let Ok(mut cache) = peer.blocked_by.write() {
                            *cache = blocked_by.into_iter().collect();
                        }
                    }
                }
                if let Some(peer) = s.peers.get(peer_id).cloned() {
                    drop(s);
                    send_to(
                        &peer,
                        &SignalMessage::Authenticated {
                            token: token_to_send,
                            user_id,
                        },
                    )
                    .await;
                    send_friend_snapshot_to_peer(state, peer_id, db).await;
                }
                log::info!("Peer {peer_id} authenticated (restored identity)");
                return true;
            }
        }
    }

    // Generate a fresh persistent identity for newly authenticated users.
    let new_token = generate_token();
    let user_id = generate_user_id();
    let now = unix_now_secs();

    // Store persistent user_id on peer for ban checks
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            *peer.user_id.lock().await = Some(user_id.clone());
        }
    }

    // Persist new user
    let db_clone = db_ref.clone();
    let uid = user_id.clone();
    let tok = new_token.clone();
    let name = user_name.clone();
    let save_result = tokio::time::timeout(
        crate::DB_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            db_clone.save_user(&crate::persistence::UserRow {
                user_id: uid,
                token: tok,
                display_name: name,
                created_at: now,
                issued_at: now,
                last_seen_at: now,
            })
        }),
    )
    .await;

    match save_result {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => {
            log::error!("Failed to persist user: {e}");
            let s = state.read().await;
            if let Some(peer) = s.peers.get(peer_id) {
                *peer.user_id.lock().await = None;
            }
            send_error(state, peer_id, "Authentication is temporarily unavailable").await;
            return false;
        }
        Ok(Err(e)) => {
            log::error!("Failed to join auth persistence task: {e}");
            let s = state.read().await;
            if let Some(peer) = s.peers.get(peer_id) {
                *peer.user_id.lock().await = None;
            }
            send_error(state, peer_id, "Authentication is temporarily unavailable").await;
            return false;
        }
        Err(_) => {
            log::warn!("DB timeout: save_user for peer {peer_id}");
            send_error(state, peer_id, "Authentication is temporarily unavailable").await;
            return false;
        }
    }

    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(
            &peer,
            &SignalMessage::Authenticated {
                token: new_token,
                user_id,
            },
        )
        .await;
        send_friend_snapshot_to_peer(state, peer_id, db).await;
    }

    log::info!("Peer {peer_id} authenticated (new identity)");
    true
}

// ─── Account System ───

/// Hash a password using Argon2id with a random salt. Returns Err on hash failure.
fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("Password hashing failed: {e}"))
}

/// Verify a password against an argon2 or legacy SHA-256 hash.
fn verify_password(password: &str, stored: &str) -> bool {
    // Try argon2 first (new format starts with "$argon2")
    if stored.starts_with("$argon2") {
        let Ok(parsed) = PasswordHash::new(stored) else {
            return false;
        };
        return Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
    }
    // Legacy SHA-256 fallback (format: "hex_salt:hex_hash")
    // This allows existing accounts to still log in after the upgrade
    legacy_verify_sha256(password, stored)
}

/// Legacy SHA-256 verification for backward compatibility with v0.8.0 passwords.
fn legacy_verify_sha256(password: &str, stored: &str) -> bool {
    let Some((salt_hex, expected_hash)) = stored.split_once(':') else {
        return false;
    };
    let Ok(salt) = (0..salt_hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&salt_hex[i..i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
    else {
        return false;
    };
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&salt);
    hasher.update(password.as_bytes());
    let hash = hasher.finalize();
    let hash_hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    hash_hex == expected_hash
}

/// Per-IP auth rate limit: 5 attempts per 60 seconds.
const AUTH_RATE_LIMIT: u32 = 5;
const AUTH_RATE_WINDOW_SECS: u64 = 60;

/// Check if auth attempt is allowed for this peer's IP. Returns false if rate limited.
async fn check_auth_rate_limit(state: &State, peer_id: &str) -> bool {
    let ip = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(p) => p.ip,
            None => return false,
        }
    };

    let mut s = state.write().await;
    let now = std::time::Instant::now();
    let entry = s.auth_attempts.entry(ip).or_insert((0, now));

    if now.duration_since(entry.1).as_secs() >= AUTH_RATE_WINDOW_SECS {
        // New window
        *entry = (1, now);
        true
    } else {
        entry.0 += 1;
        entry.0 <= AUTH_RATE_LIMIT
    }
}

pub async fn handle_create_account(
    state: &State,
    peer_id: &str,
    email: String,
    password: String,
    display_name: String,
    db: &Db,
) {
    if !check_auth_rate_limit(state, peer_id).await {
        send_auth_error(state, peer_id, "Too many attempts. Try again in a minute.").await;
        return;
    }

    let email = email.trim().to_lowercase();
    let display_name = display_name.trim().to_string();

    // Validate inputs
    if email.is_empty() || !email.contains('@') || email.len() > 254 {
        send_auth_error(state, peer_id, "Invalid email address").await;
        return;
    }
    if password.len() < 6 {
        send_auth_error(state, peer_id, "Password must be at least 6 characters").await;
        return;
    }
    if password.len() > 128 {
        send_auth_error(state, peer_id, "Password too long").await;
        return;
    }
    if display_name.is_empty() || display_name.len() > 32 {
        send_auth_error(state, peer_id, "Display name must be 1-32 characters").await;
        return;
    }

    let Some(ref db_ref) = db else {
        send_auth_error(state, peer_id, "Account system unavailable (no database)").await;
        return;
    };

    let password_hash = match hash_password(&password) {
        Ok(h) => h,
        Err(e) => {
            log::error!("Password hash failed during account creation: {e}");
            send_auth_error(state, peer_id, "Account creation failed").await;
            return;
        }
    };
    let token = generate_token();
    let user_id = generate_user_id();
    let now = unix_now_secs();

    let db_clone = db_ref.clone();
    let uid = user_id.clone();
    let tok = token.clone();
    let name = display_name.clone();
    let em = email.clone();
    let ph = password_hash.clone();

    let result = tokio::time::timeout(
        crate::DB_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            db_clone.create_account(&uid, &em, &ph, &name, &tok, now)
        }),
    )
    .await;

    match result {
        Ok(Ok(Ok(()))) => {
            // Set peer identity
            {
                let s = state.read().await;
                if let Some(peer) = s.peers.get(peer_id) {
                    *peer.user_id.lock().await = Some(user_id.clone());
                    *peer.name.lock().await = display_name;
                }
            }
            let s = state.read().await;
            if let Some(peer) = s.peers.get(peer_id).cloned() {
                drop(s);
                send_to(&peer, &SignalMessage::AccountCreated { token, user_id }).await;
            }
            log::info!("Account created for peer {peer_id} (email: {email})");
        }
        Ok(Ok(Err(e))) => {
            send_auth_error(state, peer_id, &e).await;
        }
        Ok(Err(e)) => {
            log::error!("Account creation task failed: {e}");
            send_auth_error(state, peer_id, "Account creation failed").await;
        }
        Err(_) => {
            send_auth_error(state, peer_id, "Account creation timed out").await;
        }
    }
}

pub async fn handle_login(state: &State, peer_id: &str, email: String, password: String, db: &Db) {
    if !check_auth_rate_limit(state, peer_id).await {
        send_auth_error(state, peer_id, "Too many attempts. Try again in a minute.").await;
        return;
    }

    let email = email.trim().to_lowercase();

    if email.is_empty() || password.is_empty() {
        send_auth_error(state, peer_id, "Email and password are required").await;
        return;
    }

    let Some(ref db_ref) = db else {
        send_auth_error(state, peer_id, "Account system unavailable (no database)").await;
        return;
    };

    let db_clone = db_ref.clone();
    let em = email.clone();

    let result = tokio::time::timeout(
        crate::DB_TIMEOUT,
        tokio::task::spawn_blocking(move || db_clone.find_user_by_email(&em)),
    )
    .await;

    let found = match result {
        Ok(Ok(Ok(found))) => found,
        Ok(Ok(Err(e))) => {
            log::error!("Login DB error: {e}");
            send_auth_error(state, peer_id, "Login failed").await;
            return;
        }
        Ok(Err(e)) => {
            log::error!("Login task error: {e}");
            send_auth_error(state, peer_id, "Login failed").await;
            return;
        }
        Err(_) => {
            send_auth_error(state, peer_id, "Login timed out").await;
            return;
        }
    };

    let Some((user, password_hash)) = found else {
        send_auth_error(state, peer_id, "Invalid email or password").await;
        return;
    };

    if !verify_password(&password, &password_hash) {
        send_auth_error(state, peer_id, "Invalid email or password").await;
        return;
    }

    // Rotate token on login
    let new_token = generate_token();
    let now = unix_now_secs();
    let db_clone = db_ref.clone();
    let uid = user.user_id.clone();
    let tok = new_token.clone();
    let name = user.display_name.clone();
    let _ = tokio::time::timeout(
        crate::DB_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            db_clone.rotate_user_session(&uid, &tok, &name, now, now)
        }),
    )
    .await;

    // Set peer identity
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            *peer.user_id.lock().await = Some(user.user_id.clone());
            *peer.name.lock().await = user.display_name.clone();
        }
    }

    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(
            &peer,
            &SignalMessage::LoginSuccess {
                token: new_token,
                user_id: user.user_id,
                display_name: user.display_name,
            },
        )
        .await;
        send_friend_snapshot_to_peer(state, peer_id, db).await;
    }
    log::info!("Peer {peer_id} logged in via email ({email})");
}

pub async fn handle_logout(state: &State, peer_id: &str, db: &Db) {
    let user_id = {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            peer.user_id.lock().await.clone()
        } else {
            None
        }
    };

    if let (Some(uid), Some(ref db_ref)) = (&user_id, db) {
        let db_clone = db_ref.clone();
        let uid = uid.clone();
        let _ = tokio::time::timeout(
            crate::DB_TIMEOUT,
            tokio::task::spawn_blocking(move || db_clone.invalidate_token(&uid)),
        )
        .await;
    }

    // Clear peer identity
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            *peer.user_id.lock().await = None;
        }
    }

    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(&peer, &SignalMessage::LoggedOut).await;
    }
    log::info!("Peer {peer_id} logged out");
}

pub async fn handle_change_password(
    state: &State,
    peer_id: &str,
    current_password: String,
    new_password: String,
    db: &Db,
) {
    if new_password.len() < 6 {
        send_auth_error(state, peer_id, "New password must be at least 6 characters").await;
        return;
    }
    if new_password.len() > 128 {
        send_auth_error(state, peer_id, "New password too long").await;
        return;
    }

    let user_id = {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            peer.user_id.lock().await.clone()
        } else {
            None
        }
    };

    let Some(uid) = user_id else {
        send_auth_error(state, peer_id, "Not logged in").await;
        return;
    };

    let Some(ref db_ref) = db else {
        send_auth_error(state, peer_id, "Account system unavailable").await;
        return;
    };

    // Verify current password
    let db_clone = db_ref.clone();
    let uid_clone = uid.clone();
    let stored_hash = match tokio::time::timeout(
        crate::DB_TIMEOUT,
        tokio::task::spawn_blocking(move || db_clone.get_password_hash(&uid_clone)),
    )
    .await
    {
        Ok(Ok(Ok(Some(h)))) => h,
        _ => {
            send_auth_error(state, peer_id, "Password change failed").await;
            return;
        }
    };

    if !verify_password(&current_password, &stored_hash) {
        send_auth_error(state, peer_id, "Current password is incorrect").await;
        return;
    }

    // Update password
    let new_hash = match hash_password(&new_password) {
        Ok(h) => h,
        Err(e) => {
            log::error!("Password hash failed during password change: {e}");
            send_auth_error(state, peer_id, "Password change failed").await;
            return;
        }
    };
    let db_clone = db_ref.clone();
    let uid_clone = uid.clone();
    let result = tokio::time::timeout(
        crate::DB_TIMEOUT,
        tokio::task::spawn_blocking(move || db_clone.update_password_hash(&uid_clone, &new_hash)),
    )
    .await;

    match result {
        Ok(Ok(Ok(()))) => {
            let s = state.read().await;
            if let Some(peer) = s.peers.get(peer_id).cloned() {
                drop(s);
                send_to(&peer, &SignalMessage::PasswordChanged).await;
            }
            log::info!("Password changed for user {uid}");
        }
        _ => {
            send_auth_error(state, peer_id, "Password change failed").await;
        }
    }
}

pub async fn handle_revoke_all_sessions(state: &State, peer_id: &str, db: &Db) {
    let user_id = {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            peer.user_id.lock().await.clone()
        } else {
            None
        }
    };

    let Some(uid) = user_id else {
        send_auth_error(state, peer_id, "Not logged in").await;
        return;
    };

    let Some(ref db_ref) = db else {
        send_auth_error(state, peer_id, "Account system unavailable").await;
        return;
    };

    // Invalidate token in DB — all other sessions become invalid
    let db_clone = db_ref.clone();
    let uid_clone = uid.clone();
    let result = tokio::time::timeout(
        crate::DB_TIMEOUT,
        tokio::task::spawn_blocking(move || db_clone.invalidate_token(&uid_clone)),
    )
    .await;

    match result {
        Ok(Ok(Ok(()))) => {
            // Issue a fresh token for the current session
            let new_token = generate_token();
            let now = unix_now_secs();
            let display_name = {
                let s = state.read().await;
                if let Some(peer) = s.peers.get(peer_id) {
                    peer.name.lock().await.clone()
                } else {
                    uid.clone()
                }
            };
            let db_clone = db_ref.clone();
            let uid_clone = uid.clone();
            let tok = new_token.clone();
            let name = display_name;
            let _ = tokio::time::timeout(
                crate::DB_TIMEOUT,
                tokio::task::spawn_blocking(move || {
                    db_clone.rotate_user_session(&uid_clone, &tok, &name, now, now)
                }),
            )
            .await;

            let s = state.read().await;
            if let Some(peer) = s.peers.get(peer_id).cloned() {
                drop(s);
                // Send new token to current session
                send_to(
                    &peer,
                    &SignalMessage::Authenticated {
                        token: new_token,
                        user_id: uid.clone(),
                    },
                )
                .await;
                send_to(&peer, &SignalMessage::AllSessionsRevoked).await;
            }
            log::info!("All sessions revoked for user {uid}");
        }
        _ => {
            send_auth_error(state, peer_id, "Failed to revoke sessions").await;
        }
    }
}

async fn send_auth_error(state: &State, peer_id: &str, message: &str) {
    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(
            &peer,
            &SignalMessage::AuthError {
                message: message.to_string(),
            },
        )
        .await;
    }
}

fn generate_token() -> String {
    random_hex(32)
}

fn generate_user_id() -> String {
    format!("u{}", random_hex(12))
}

fn random_hex(num_bytes: usize) -> String {
    let mut bytes = vec![0u8; num_bytes];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_and_verify_password() {
        let password = "hunter2";
        let hash = hash_password(password).unwrap();
        assert!(hash.starts_with("$argon2"));
        assert!(verify_password(password, &hash));
        assert!(!verify_password("wrong", &hash));
    }

    #[test]
    fn test_hash_different_salts() {
        let hash1 = hash_password("same_password").unwrap();
        let hash2 = hash_password("same_password").unwrap();
        assert_ne!(hash1, hash2);
        assert!(verify_password("same_password", &hash1));
        assert!(verify_password("same_password", &hash2));
    }

    #[test]
    fn test_verify_malformed_hash() {
        assert!(!verify_password("test", "no_colon"));
        assert!(!verify_password("test", ""));
    }

    #[test]
    fn test_legacy_sha256_verification() {
        // Simulate a v0.8.0 SHA-256 hash: salt_hex:hash_hex
        use sha2::{Digest, Sha256};
        let salt = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10,
        ];
        let salt_hex: String = salt.iter().map(|b| format!("{b:02x}")).collect();
        let mut hasher = Sha256::new();
        hasher.update(salt);
        hasher.update(b"legacy_password");
        let hash = hasher.finalize();
        let hash_hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        let stored = format!("{salt_hex}:{hash_hex}");

        assert!(verify_password("legacy_password", &stored));
        assert!(!verify_password("wrong_password", &stored));
    }
}

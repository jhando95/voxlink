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
            );
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
                    );
                    send_friend_snapshot_to_peer(state, peer_id, db).await;
                    super::read_state::send_snapshot_to_peer(state, peer_id, db).await;
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
        );
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
pub(crate) fn verify_password(password: &str, stored: &str) -> bool {
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

/// Per-account login lockout: after this many failed attempts within
/// LOGIN_ACCOUNT_LOCK_WINDOW_SECS, the email is locked out for the rest of
/// the window. Tracks attempts whether the email exists or not, so unknown
/// emails can't be used as a probing oracle.
const LOGIN_ACCOUNT_FAIL_LIMIT: u32 = 10;
const LOGIN_ACCOUNT_LOCK_WINDOW_SECS: u64 = 900; // 15 minutes

async fn check_login_account_lock(state: &State, email: &str) -> bool {
    let s = state.read().await;
    let Some(&(count, started)) = s.login_failures_per_email.get(email) else {
        return true;
    };
    let in_window = started.elapsed().as_secs() < LOGIN_ACCOUNT_LOCK_WINDOW_SECS;
    if in_window && count >= LOGIN_ACCOUNT_FAIL_LIMIT {
        return false;
    }
    true
}

async fn record_login_failure_for_email(state: &State, email: &str) {
    let mut s = state.write().await;
    let now = std::time::Instant::now();
    // Opportunistic prune: keep the map bounded.
    s.login_failures_per_email
        .retain(|_, (_, started)| started.elapsed().as_secs() < LOGIN_ACCOUNT_LOCK_WINDOW_SECS);
    let entry = s
        .login_failures_per_email
        .entry(email.to_string())
        .or_insert((0, now));
    if entry.1.elapsed().as_secs() >= LOGIN_ACCOUNT_LOCK_WINDOW_SECS {
        // Window expired — start a fresh count.
        *entry = (1, now);
    } else {
        entry.0 += 1;
    }
}

async fn clear_login_failures_for_email(state: &State, email: &str) {
    let mut s = state.write().await;
    s.login_failures_per_email.remove(email);
}

/// Reset transient per-connection state (room, space, mute/deafen, typing,
/// whisper, blocked-by cache) when a peer switches identities via Login or
/// CreateAccount. Without this, signing into a different account on the same
/// WebSocket leaks the previous user's room membership + flags into the new
/// session — and the audit log + ban check resolve against the wrong user_id.
async fn clear_session_state_on_identity_switch(state: &State, peer_id: &str) {
    let s = state.read().await;
    let peer = match s.peers.get(peer_id).cloned() {
        Some(p) => p,
        None => return,
    };
    drop(s);
    // Drop room membership (so the peer doesn't continue receiving relayed audio).
    peer.set_room_code(None).await;
    *peer.space_id.lock().await = None;
    *peer.typing_channel_id.lock().await = None;
    *peer.typing_dm_user_id.lock().await = None;
    peer.is_muted
        .store(false, std::sync::atomic::Ordering::Relaxed);
    peer.is_deafened
        .store(false, std::sync::atomic::Ordering::Relaxed);
    peer.is_server_deafened
        .store(false, std::sync::atomic::Ordering::Relaxed);
    peer.is_priority_speaker
        .store(false, std::sync::atomic::Ordering::Relaxed);
    peer.timeout_until
        .store(0, std::sync::atomic::Ordering::Relaxed);
    peer.space_perms
        .store(0, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut w) = peer.whisper_targets.write() {
        w.clear();
    }
    if let Ok(mut b) = peer.blocked_by.write() {
        b.clear();
    };
}

/// Lightweight email shape check. Not RFC-5322 perfect, but rejects the obvious
/// junk a `contains('@')` test waves through (no local part, no dot in domain,
/// embedded whitespace/control bytes, leading/trailing dots).
fn is_valid_email(email: &str) -> bool {
    if email.is_empty() || email.len() > 254 {
        return false;
    }
    if email.contains(char::is_whitespace) || email.bytes().any(|b| b < 0x20) {
        return false;
    }
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    if local.is_empty() || local.len() > 64 || local.starts_with('.') || local.ends_with('.') {
        return false;
    }
    if local.contains("..") {
        return false;
    }
    if domain.len() < 3 || !domain.contains('.') {
        return false;
    }
    if domain.starts_with('.') || domain.ends_with('.') || domain.contains("..") {
        return false;
    }
    if domain.contains('@') {
        return false;
    }
    // No bare dotless top-level domains, and the TLD must be at least 2 chars.
    let tld = domain.rsplit('.').next().unwrap_or("");
    if tld.len() < 2 {
        return false;
    }
    true
}

/// Check if auth attempt is allowed for this peer's IP. Returns false if rate limited.
pub(crate) async fn check_auth_rate_limit(state: &State, peer_id: &str) -> bool {
    let ip = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(p) => p.ip,
            None => return false,
        }
    };

    let mut s = state.write().await;
    let now = std::time::Instant::now();
    // Opportunistic cleanup: prune expired entries before inserting a new one.
    s.auth_attempts
        .retain(|_, (_, window_start)| window_start.elapsed().as_secs() < 600);
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
    if !is_valid_email(&email) {
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
            // Drop any leftover room/space membership from a prior identity on
            // the same WebSocket before adopting the new user_id.
            clear_session_state_on_identity_switch(state, peer_id).await;
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
                send_to(&peer, &SignalMessage::AccountCreated { token, user_id });
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

    // Per-account lockout: 10 failures in 15 minutes locks that email out
    // until the window expires, even from a fresh IP. Pairs with the per-IP
    // gate to cover the case of a multi-IP attacker grinding one account.
    if !check_login_account_lock(state, &email).await {
        send_auth_error(
            state,
            peer_id,
            "This account is temporarily locked due to too many failed logins. Try again later.",
        )
        .await;
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
        // Run a verify against a known-bad Argon2id hash so the unknown-email
        // path takes ~the same time as the wrong-password path. Without this,
        // timing alone reveals whether an email is registered.
        const DUMMY_ARGON2_HASH: &str =
            "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$wAjB/QGNgU8sCgsAaB7m0fX0c5/JzN/+r0YwYsqWmcs";
        let pw = password.clone();
        let _ = tokio::task::spawn_blocking(move || {
            // We don't care about the result — only that the CPU work happened.
            let _ = verify_password(&pw, DUMMY_ARGON2_HASH);
        })
        .await;
        // Count the failure under the supplied email so unknown emails also
        // contribute toward lockout (no enumeration oracle).
        record_login_failure_for_email(state, &email).await;
        send_auth_error(state, peer_id, "Invalid email or password").await;
        return;
    };

    if !verify_password(&password, &password_hash) {
        record_login_failure_for_email(state, &email).await;
        send_auth_error(state, peer_id, "Invalid email or password").await;
        return;
    }

    // Success — reset the per-account failure counter.
    clear_login_failures_for_email(state, &email).await;

    // If the stored hash is legacy SHA-256, transparently re-hash with Argon2id so
    // the next login uses the modern verifier. Best-effort: a re-hash failure here
    // must not block the user from logging in.
    if !password_hash.starts_with("$argon2") {
        if let Ok(new_hash) = hash_password(&password) {
            let db_clone = db_ref.clone();
            let uid_for_rehash = user.user_id.clone();
            let _ = tokio::time::timeout(
                crate::DB_TIMEOUT,
                tokio::task::spawn_blocking(move || {
                    db_clone.update_password_hash(&uid_for_rehash, &new_hash)
                }),
            )
            .await;
            log::info!(
                "Rehashed legacy SHA-256 password to Argon2id for user {}",
                user.user_id
            );
        }
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

    // Drop any leftover room/space membership from a prior identity on the same
    // WebSocket before adopting the new user_id.
    clear_session_state_on_identity_switch(state, peer_id).await;

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
        );
        send_friend_snapshot_to_peer(state, peer_id, db).await;
        super::read_state::send_snapshot_to_peer(state, peer_id, db).await;
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
        send_to(&peer, &SignalMessage::LoggedOut);
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
    // Reuse the per-IP auth rate limit to bound credential-flipping attempts.
    if !check_auth_rate_limit(state, peer_id).await {
        send_auth_error(state, peer_id, "Too many attempts. Try again in a minute.").await;
        return;
    }
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
                send_to(&peer, &SignalMessage::PasswordChanged);
            }
            log::info!("Password changed for user {uid}");
        }
        _ => {
            send_auth_error(state, peer_id, "Password change failed").await;
        }
    }
}

pub async fn handle_revoke_all_sessions(state: &State, peer_id: &str, db: &Db) {
    if !check_auth_rate_limit(state, peer_id).await {
        send_auth_error(state, peer_id, "Too many attempts. Try again in a minute.").await;
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
                );
                send_to(&peer, &SignalMessage::AllSessionsRevoked);
            }
            log::info!("All sessions revoked for user {uid}");
        }
        _ => {
            send_auth_error(state, peer_id, "Failed to revoke sessions").await;
        }
    }
}

pub async fn handle_change_email(
    state: &State,
    peer_id: &str,
    current_password: String,
    new_email: String,
    db: &Db,
) {
    if !check_auth_rate_limit(state, peer_id).await {
        send_auth_error(state, peer_id, "Too many attempts. Try again in a minute.").await;
        return;
    }
    let new_email = new_email.trim().to_lowercase();
    if !is_valid_email(&new_email) {
        send_auth_error(state, peer_id, "Invalid email address").await;
        return;
    }

    let user_id = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => peer.user_id.lock().await.clone(),
            None => None,
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

    // Verify the current password before allowing the change.
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
            send_auth_error(state, peer_id, "Email change failed").await;
            return;
        }
    };
    if !verify_password(&current_password, &stored_hash) {
        send_auth_error(state, peer_id, "Current password is incorrect").await;
        return;
    }

    // Update the email (DB enforces uniqueness via the idx_users_email index).
    let db_clone = db_ref.clone();
    let uid_clone = uid.clone();
    let email_for_db = new_email.clone();
    let result = tokio::time::timeout(
        crate::DB_TIMEOUT,
        tokio::task::spawn_blocking(move || db_clone.update_user_email(&uid_clone, &email_for_db)),
    )
    .await;

    match result {
        Ok(Ok(Ok(()))) => {
            let s = state.read().await;
            if let Some(peer) = s.peers.get(peer_id).cloned() {
                drop(s);
                send_to(&peer, &SignalMessage::EmailChanged { email: new_email });
            }
            log::info!("Email changed for user {uid}");
        }
        Ok(Ok(Err(e))) => send_auth_error(state, peer_id, &e).await,
        _ => send_auth_error(state, peer_id, "Email change failed").await,
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
        );
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
    fn email_validator_accepts_well_formed_and_rejects_obvious_junk() {
        assert!(is_valid_email("alice@example.com"));
        assert!(is_valid_email("alice.smith+tag@example.co.uk"));

        // Empty / oversize
        assert!(!is_valid_email(""));
        let huge = format!("{}@example.com", "a".repeat(250));
        assert!(!is_valid_email(&huge));

        // Structural
        assert!(!is_valid_email("noatsign.example.com"));
        assert!(!is_valid_email("@example.com"));
        assert!(!is_valid_email("alice@"));
        assert!(!is_valid_email("alice@nodot"));
        assert!(!is_valid_email("alice@example."));
        assert!(!is_valid_email("alice@example.c")); // 1-char TLD

        // Embedded whitespace / control characters
        assert!(!is_valid_email("ali ce@example.com"));
        assert!(!is_valid_email("alice@example.com\n"));

        // Double dots / leading dot
        assert!(!is_valid_email(".alice@example.com"));
        assert!(!is_valid_email("alice..smith@example.com"));
        assert!(!is_valid_email("alice@example..com"));
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

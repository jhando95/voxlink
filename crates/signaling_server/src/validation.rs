use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;
use crate::types::State;
use crate::LIMITS;

// ─── Constants ───

pub(crate) const MAX_NAME_LEN: usize = 32;
pub(crate) const MAX_PASSWORD_LEN: usize = 64;

// ─── Rate Limiting ───

/// Monotonic millisecond timestamp for lock-free rate limiting.
pub(crate) fn instant_to_ms() -> u64 {
    // Using system uptime-style monotonic clock avoids Instant → u64 issues.
    // We only need relative 1-second windows, so wrapping after ~584 million years is fine.
    static EPOCH: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
    EPOCH.elapsed().as_millis() as u64
}

/// Lock-free rate limit check using atomic timestamp + counter.
pub(crate) fn atomic_rate_check(window_ms: &AtomicU64, counter: &AtomicU32, limit: u32) -> bool {
    let now = instant_to_ms();
    let prev = window_ms.load(Ordering::Relaxed);
    if now.wrapping_sub(prev) >= 1000 {
        // New window — reset counter. CAS to avoid races resetting twice.
        if window_ms
            .compare_exchange(prev, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            counter.store(1, Ordering::Relaxed);
            return true;
        }
        // CAS failed — another thread already reset. Fall through to count check.
    }
    let count = counter.fetch_add(1, Ordering::Relaxed);
    count < limit
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChunkedScreenSequenceState {
    NewFrame,
    ExistingFrame,
    StaleFrame,
}

pub(crate) fn chunked_screen_sequence_state(
    last_sequence: &AtomicU32,
    sequence: u32,
) -> ChunkedScreenSequenceState {
    let mut current = last_sequence.load(Ordering::Relaxed);
    loop {
        if current == sequence {
            return ChunkedScreenSequenceState::ExistingFrame;
        }
        if current != 0 && sequence.wrapping_sub(current) >= 0x8000_0000 {
            return ChunkedScreenSequenceState::StaleFrame;
        }
        match last_sequence.compare_exchange(
            current,
            sequence,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return ChunkedScreenSequenceState::NewFrame,
            Err(updated) => current = updated,
        }
    }
}

pub(crate) async fn check_rate_limit(state: &State, peer_id: &str) -> bool {
    let s = state.read().await;
    let peer = match s.peers.get(peer_id) {
        Some(p) => p.clone(),
        None => return false,
    };
    drop(s);

    atomic_rate_check(
        &peer.rate_window_ms,
        &peer.msg_count,
        LIMITS.rate_limit_per_sec,
    )
}

// ─── Input Validation ───

pub(crate) fn validate_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Name cannot be empty".into());
    }
    if trimmed.len() > MAX_NAME_LEN {
        return Err(format!("Name too long (max {} characters)", MAX_NAME_LEN));
    }
    Ok(())
}

pub(crate) fn validate_room_code(code: &str) -> Result<(), String> {
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        return Err("Invalid room code (must be 6 digits)".into());
    }
    Ok(())
}

pub(crate) fn validate_password(pw: &Option<String>) -> Result<(), String> {
    if let Some(ref p) = pw {
        if p.len() > MAX_PASSWORD_LEN {
            return Err(format!(
                "Password too long (max {} characters)",
                MAX_PASSWORD_LEN
            ));
        }
    }
    Ok(())
}

pub(crate) fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

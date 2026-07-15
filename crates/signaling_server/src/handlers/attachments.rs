//! File / image attachment upload and retrieval.
//!
//! Like voice notes, an upload directly posts a chat message; the bytes are
//! stored in the DB and served on demand (`RequestAttachment`) instead of being
//! broadcast to every recipient. Bytes arrive base64-encoded inside the JSON
//! control frame.

use std::sync::atomic::Ordering;

use shared_types::{SignalMessage, MAX_ATTACHMENT_SIZE};

use crate::connection::{send_error, send_to};
use crate::types::{Db, State};
use crate::validation::now_epoch_secs;

/// MIME types accepted for upload. Conservative on purpose: images for inline
/// rendering plus common document/media/archive types. Scriptable types such as
/// `image/svg+xml` and `text/html` are intentionally excluded.
pub const ALLOWED_ATTACHMENT_MIME: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "application/pdf",
    "text/plain",
    "application/zip",
    "audio/mpeg",
    "audio/ogg",
    "audio/wav",
    "video/mp4",
    "video/webm",
];

/// Total attachment storage budget for the whole server. Keeps a self-hosted
/// instance from filling its disk. Overridable via `VOXLINK_MAX_ATTACHMENT_STORAGE_MB`.
fn max_total_storage_bytes() -> u64 {
    const DEFAULT_MB: u64 = 256;
    std::env::var("VOXLINK_MAX_ATTACHMENT_STORAGE_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&mb| mb > 0)
        .unwrap_or(DEFAULT_MB)
        * 1024
        * 1024
}

/// Validate a single upload's declared MIME type and decoded size.
/// Pure (no I/O) so it can be unit-tested directly.
pub fn validate_upload(mime: &str, size_bytes: usize) -> Result<(), String> {
    if size_bytes == 0 {
        return Err("Attachment is empty".to_string());
    }
    if size_bytes > MAX_ATTACHMENT_SIZE {
        return Err(format!(
            "Attachment too large (max {} MB)",
            MAX_ATTACHMENT_SIZE / (1024 * 1024)
        ));
    }
    if !ALLOWED_ATTACHMENT_MIME.contains(&mime) {
        return Err(format!("Unsupported file type: {mime}"));
    }
    Ok(())
}

fn gen_attachment_id() -> String {
    use rand::rngs::OsRng;
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("att_{hex}")
}

/// Client -> Server: store an uploaded file and post it as a chat message.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_upload_attachment(
    state: &State,
    peer_id: &str,
    channel_id: String,
    file_name: String,
    mime: String,
    caption: String,
    data_b64: String,
    db: &Db,
) {
    // Decode and validate the payload before doing any work.
    let Some(data) = shared_types::b64::decode(&data_b64) else {
        send_error(state, peer_id, "Attachment encoding was invalid").await;
        return;
    };
    if let Err(e) = validate_upload(&mime, data.len()) {
        send_error(state, peer_id, &e).await;
        return;
    }
    // Tighten metadata caps. file_name comes from the client; without bounds it
    // can be megabytes of arbitrary text, contain NUL bytes (sqlite-hostile),
    // or carry embedded path separators that confuse later download logic.
    let mut file_name = file_name;
    if file_name.contains('\0') || file_name.contains('/') || file_name.contains('\\') {
        send_error(state, peer_id, "Invalid filename").await;
        return;
    }
    if file_name.chars().count() > 255 {
        file_name = file_name.chars().take(255).collect();
    }
    if mime.len() > 64 || mime.contains('\0') {
        send_error(state, peer_id, "Invalid MIME type").await;
        return;
    }
    let caption = caption.chars().take(500).collect::<String>();

    let Some(ref db_ref) = db else {
        send_error(state, peer_id, "Attachments require a database").await;
        return;
    };

    let space_id = {
        let s = state.read().await;
        match s.peers.get(peer_id) {
            Some(peer) => peer.space_id.lock().await.clone(),
            None => None,
        }
    };
    let Some(space_id) = space_id else { return };

    // Reject if the sender is timed out.
    {
        let s = state.read().await;
        let timed_out = s
            .peers
            .get(peer_id)
            .map(|p| {
                let until = p.timeout_until.load(Ordering::Relaxed);
                until > 0 && now_epoch_secs() < until
            })
            .unwrap_or(false);
        drop(s);
        if timed_out {
            send_error(state, peer_id, "You are timed out and cannot send messages").await;
            return;
        }
    }

    // Channel min_role permission.
    {
        let s = state.read().await;
        let min_role = s
            .spaces
            .get(&space_id)
            .and_then(|sp| sp.channels.iter().find(|ch| ch.id == channel_id))
            .map(|ch| ch.min_role)
            .unwrap_or(shared_types::SpaceRole::Member);
        if min_role != shared_types::SpaceRole::Member {
            let user_role = if let Some(peer) = s.peers.get(peer_id) {
                if let Some(uid) = peer.user_id.lock().await.as_deref() {
                    s.spaces
                        .get(&space_id)
                        .and_then(|sp| sp.member_roles.get(uid).copied())
                        .unwrap_or(shared_types::SpaceRole::Member)
                } else {
                    shared_types::SpaceRole::Member
                }
            } else {
                shared_types::SpaceRole::Member
            };
            if !user_role.has_at_least(min_role) {
                drop(s);
                send_error(
                    state,
                    peer_id,
                    "You don't have permission to use this channel",
                )
                .await;
                return;
            }
        }
    }

    // Slow mode.
    {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            let slow_mode_secs = space
                .channels
                .iter()
                .find(|ch| ch.id == channel_id)
                .map(|ch| ch.slow_mode_secs)
                .unwrap_or(0);
            if slow_mode_secs > 0 {
                let now = now_epoch_secs();
                let key = (channel_id.clone(), peer_id.to_string());
                if let Some(&last) = space.slow_mode_timestamps.get(&key) {
                    if now < last + slow_mode_secs as u64 {
                        let remaining = (last + slow_mode_secs as u64) - now;
                        drop(s);
                        send_error(
                            state,
                            peer_id,
                            &format!("Slow mode: wait {remaining}s before sending again"),
                        )
                        .await;
                        return;
                    }
                }
                space.slow_mode_timestamps.insert(key, now);
            }
        }
    }

    // Auto-moderation on the caption + filename.
    let automod_text = format!("{caption} {file_name}");
    if let Some((matched_word, action)) =
        crate::handlers::moderation::check_automod(db, &space_id, &automod_text).await
    {
        if action == "block" {
            send_error(
                state,
                peer_id,
                &format!("Message blocked by auto-moderation (matched: {matched_word})"),
            )
            .await;
            return;
        }
    }

    // Enforce the server-wide storage budget.
    let added = data.len() as u64;
    {
        let db_clone = db_ref.clone();
        match tokio::time::timeout(
            crate::DB_TIMEOUT,
            tokio::task::spawn_blocking(move || db_clone.total_attachment_bytes()),
        )
        .await
        {
            Ok(Ok(Ok(total))) if total + added > max_total_storage_bytes() => {
                send_error(state, peer_id, "Server attachment storage is full").await;
                return;
            }
            Ok(Ok(Ok(_))) => {}
            _ => {
                send_error(
                    state,
                    peer_id,
                    "Attachment storage is temporarily unavailable",
                )
                .await;
                return;
            }
        }
    }

    // Resolve sender identity.
    let (user_id, sender_name) = {
        let s = state.read().await;
        let Some(peer) = s.peers.get(peer_id).cloned() else {
            return;
        };
        let uid = peer
            .user_id
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| peer_id.to_string());
        let mut name = peer.name.lock().await.clone();
        if name.is_empty() {
            name = "Anonymous".to_string();
        }
        (uid, name)
    };

    let message_id = {
        let mut s = state.write().await;
        s.alloc_message_id()
    };
    let attachment_id = gen_attachment_id();
    let size = data.len() as u32;
    let now_secs = now_epoch_secs();

    // Store the bytes.
    {
        let db_clone = db_ref.clone();
        let row = crate::persistence::AttachmentRow {
            id: attachment_id.clone(),
            file_name: file_name.clone(),
            mime: mime.clone(),
            size,
            uploader_id: user_id.clone(),
            created_at: now_secs as i64,
        };
        match tokio::time::timeout(
            crate::DB_TIMEOUT,
            tokio::task::spawn_blocking(move || db_clone.insert_attachment(&row, &data)),
        )
        .await
        {
            Ok(Ok(Ok(()))) => {}
            _ => {
                send_error(state, peer_id, "Failed to store attachment").await;
                return;
            }
        }
    }

    let msg_data = shared_types::TextMessageData {
        sender_id: user_id.clone(),
        sender_name,
        content: caption.clone(),
        timestamp: now_secs,
        message_id: message_id.clone(),
        edited: false,
        reactions: Vec::new(),
        reply_to_message_id: None,
        reply_to_sender_name: None,
        reply_preview: None,
        pinned: false,
        forwarded_from: None,
        attachment_name: Some(file_name.clone()),
        attachment_size: Some(size),
        attachment_id: Some(attachment_id.clone()),
        link_url: None,
    };

    // Cache in memory (so it appears in history without a DB round-trip).
    {
        let mut s = state.write().await;
        if let Some(space) = s.spaces.get_mut(&space_id) {
            let msgs = space.text_messages.entry(channel_id.clone()).or_default();
            msgs.push_back(msg_data.clone());
            if msgs.len() > crate::max_channel_messages() {
                msgs.pop_front();
            }
        }
    }

    // Persist the message row so it survives a restart.
    {
        let db_clone = db_ref.clone();
        let row = crate::persistence::MessageRow {
            id: message_id.clone(),
            channel_id: channel_id.clone(),
            sender_id: user_id.clone(),
            sender_name: msg_data.sender_name.clone(),
            content: caption.clone(),
            timestamp: now_secs as i64,
            edited: false,
            reply_to_message_id: None,
            reply_to_sender_name: None,
            reply_preview: None,
            pinned: false,
            link_url: None,
            attachment_id: Some(attachment_id.clone()),
            attachment_name: Some(file_name.clone()),
            attachment_size: Some(size),
        };
        tokio::task::spawn_blocking(move || {
            if let Err(e) = db_clone.save_message(&row) {
                log::error!("Failed to persist attachment message: {e}");
            }
        });
    }

    // Broadcast to all space members. `member_ids` holds peer-map keys, so look
    // them up directly — mirroring handle_send_text_message.
    let notify = SignalMessage::TextMessage {
        channel_id: channel_id.clone(),
        message: msg_data.clone(),
    };
    let s = state.read().await;
    if let Some(space) = s.spaces.get(&space_id) {
        let members: Vec<_> = space
            .member_ids
            .iter()
            .filter_map(|id| s.peers.get(id).cloned())
            .collect();
        drop(s);
        for peer in &members {
            send_to(peer, &notify);
        }
    }
}

/// Client -> Server: return the bytes of a stored attachment to the requester.
pub(crate) async fn handle_request_attachment(
    state: &State,
    peer_id: &str,
    attachment_id: String,
    db: &Db,
) {
    let Some(ref db_ref) = db else {
        send_error(state, peer_id, "Attachments require a database").await;
        return;
    };

    let db_clone = db_ref.clone();
    let id = attachment_id.clone();
    let found = match tokio::time::timeout(
        crate::DB_TIMEOUT,
        tokio::task::spawn_blocking(move || db_clone.get_attachment(&id)),
    )
    .await
    {
        Ok(Ok(Ok(Some(v)))) => v,
        Ok(Ok(Ok(None))) => {
            send_error(state, peer_id, "Attachment not found").await;
            return;
        }
        _ => {
            send_error(state, peer_id, "Failed to load attachment").await;
            return;
        }
    };

    let (row, bytes) = found;
    let data_b64 = shared_types::b64::encode(&bytes);
    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id).cloned() {
        drop(s);
        send_to(
            &peer,
            &SignalMessage::AttachmentData {
                attachment_id: row.id,
                file_name: row.file_name,
                mime: row.mime,
                data_b64,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_uploads() {
        assert!(validate_upload("image/png", 0).is_err());
    }

    #[test]
    fn rejects_oversize_uploads() {
        assert!(validate_upload("image/png", MAX_ATTACHMENT_SIZE + 1).is_err());
        assert!(validate_upload("image/png", MAX_ATTACHMENT_SIZE).is_ok());
    }

    #[test]
    fn rejects_disallowed_mime() {
        assert!(validate_upload("image/svg+xml", 100).is_err());
        assert!(validate_upload("text/html", 100).is_err());
        assert!(validate_upload("application/x-msdownload", 100).is_err());
    }

    #[test]
    fn accepts_common_safe_types() {
        for mime in ["image/png", "image/jpeg", "application/pdf", "text/plain"] {
            assert!(
                validate_upload(mime, 100).is_ok(),
                "{mime} should be allowed"
            );
        }
    }
}

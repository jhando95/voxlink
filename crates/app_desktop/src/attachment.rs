//! Client-side helper for turning a local file into an `UploadAttachment` signal.

use shared_types::{SignalMessage, MAX_ATTACHMENT_SIZE};

/// Read a local file and build an `UploadAttachment` message for `channel_id`
/// with the given caption. Returns a user-facing error string on failure
/// (missing file, too large, unnamed path).
pub fn build_upload_attachment(
    path: &std::path::Path,
    channel_id: &str,
    caption: &str,
) -> Result<SignalMessage, String> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Invalid file name".to_string())?
        .to_string();

    let data = std::fs::read(path).map_err(|e| format!("Could not read file: {e}"))?;
    if data.is_empty() {
        return Err("File is empty".to_string());
    }
    if data.len() > MAX_ATTACHMENT_SIZE {
        return Err(format!(
            "File too large ({} MB); max is {} MB",
            data.len() / (1024 * 1024),
            MAX_ATTACHMENT_SIZE / (1024 * 1024)
        ));
    }

    let mime = shared_types::attachment::mime_for_filename(&file_name).to_string();
    Ok(SignalMessage::UploadAttachment {
        channel_id: channel_id.to_string(),
        file_name,
        mime,
        caption: caption.to_string(),
        data_b64: shared_types::b64::encode(&data),
    })
}

/// Strip any directory components from a server-provided file name so a
/// malicious name can't escape the download directory (path traversal).
pub fn sanitize_filename(file_name: &str) -> String {
    let base = file_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(file_name)
        .trim();
    let cleaned: String = base
        .chars()
        .filter(|c| !matches!(c, '\0' | ':' | '<' | '>' | '"' | '|' | '?' | '*'))
        .collect();
    let cleaned = cleaned.trim_matches('.').trim();
    if cleaned.is_empty() {
        "download".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Write `bytes` into `dir` under a sanitized, collision-free name. Returns the
/// path written. Pure aside from the file write, so it's unit-testable.
pub fn save_to_dir(
    dir: &std::path::Path,
    file_name: &str,
    bytes: &[u8],
) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create dir: {e}"))?;
    let safe = sanitize_filename(file_name);
    let mut candidate = dir.join(&safe);
    let (stem, ext) = match safe.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (safe.clone(), String::new()),
    };
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::write(&candidate, bytes).map_err(|e| format!("write: {e}"))?;
    Ok(candidate)
}

/// Directory where downloaded attachments are saved (the user's Downloads
/// folder, falling back to the temp dir).
pub fn download_dir() -> std::path::PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(|p| p.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
}

/// Save a received attachment to the download directory.
pub fn save_downloaded(file_name: &str, bytes: &[u8]) -> Result<std::path::PathBuf, String> {
    save_to_dir(&download_dir(), file_name, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("voxlink_attach_test_{}_{name}", std::process::id()));
        p
    }

    #[test]
    fn builds_upload_from_file() {
        let path = temp_path("cat.png");
        let bytes = vec![1u8, 2, 3, 4, 5];
        std::fs::write(&path, &bytes).unwrap();

        let msg = build_upload_attachment(&path, "chan1", "look").unwrap();
        match msg {
            SignalMessage::UploadAttachment {
                channel_id,
                file_name,
                mime,
                caption,
                data_b64,
            } => {
                assert_eq!(channel_id, "chan1");
                assert!(file_name.ends_with("cat.png"));
                assert_eq!(mime, "image/png");
                assert_eq!(caption, "look");
                assert_eq!(shared_types::b64::decode(&data_b64).unwrap(), bytes);
            }
            other => panic!("expected UploadAttachment, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_missing_file() {
        let path = temp_path("does_not_exist.png");
        let _ = std::fs::remove_file(&path);
        assert!(build_upload_attachment(&path, "c", "").is_err());
    }

    #[test]
    fn rejects_empty_file() {
        let path = temp_path("empty.png");
        std::fs::write(&path, []).unwrap();
        assert!(build_upload_attachment(&path, "c", "").is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sanitizes_path_traversal_names() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a/b/c.png"), "c.png");
        assert_eq!(sanitize_filename("..\\..\\win.exe"), "win.exe");
        assert_eq!(sanitize_filename(""), "download");
        assert_eq!(sanitize_filename("..."), "download");
        assert_eq!(sanitize_filename("bad:name?.png"), "badname.png");
    }

    #[test]
    fn save_to_dir_writes_and_avoids_collisions() {
        let dir = std::env::temp_dir().join(format!("voxlink_dl_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p1 = save_to_dir(&dir, "pic.png", b"first").unwrap();
        assert_eq!(std::fs::read(&p1).unwrap(), b"first");
        let p2 = save_to_dir(&dir, "pic.png", b"second").unwrap();
        assert_ne!(p1, p2);
        assert_eq!(std::fs::read(&p2).unwrap(), b"second");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

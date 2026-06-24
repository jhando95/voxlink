//! Pure helpers for classifying attachments by file name. Shared so the client
//! (deciding MIME on upload and whether to render inline) and server stay aligned.

/// Best-effort MIME type for a file name, by extension. Falls back to
/// `application/octet-stream` for unknown extensions.
pub fn mime_for_filename(file_name: &str) -> &'static str {
    match extension_lower(file_name).as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("pdf") => "application/pdf",
        Some("txt") | Some("log") | Some("md") => "text/plain",
        Some("zip") => "application/zip",
        Some("mp3") => "audio/mpeg",
        Some("ogg") | Some("opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        _ => "application/octet-stream",
    }
}

/// Whether a file name looks like an image we can render inline.
pub fn is_image_filename(file_name: &str) -> bool {
    matches!(
        extension_lower(file_name).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp")
    )
}

fn extension_lower(file_name: &str) -> Option<String> {
    file_name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_extensions() {
        assert_eq!(mime_for_filename("cat.png"), "image/png");
        assert_eq!(mime_for_filename("photo.JPG"), "image/jpeg");
        assert_eq!(mime_for_filename("photo.jpeg"), "image/jpeg");
        assert_eq!(mime_for_filename("loop.gif"), "image/gif");
        assert_eq!(mime_for_filename("pic.webp"), "image/webp");
        assert_eq!(mime_for_filename("doc.pdf"), "application/pdf");
        assert_eq!(mime_for_filename("notes.txt"), "text/plain");
        assert_eq!(mime_for_filename("archive.zip"), "application/zip");
    }

    #[test]
    fn unknown_extension_is_octet_stream() {
        assert_eq!(mime_for_filename("mystery.xyz"), "application/octet-stream");
        assert_eq!(mime_for_filename("noext"), "application/octet-stream");
    }

    #[test]
    fn detects_images_case_insensitively() {
        assert!(is_image_filename("a.png"));
        assert!(is_image_filename("a.PNG"));
        assert!(is_image_filename("a.jpeg"));
        assert!(is_image_filename("a.gif"));
        assert!(is_image_filename("a.webp"));
        assert!(!is_image_filename("a.pdf"));
        assert!(!is_image_filename("a.txt"));
        assert!(!is_image_filename("noext"));
    }
}

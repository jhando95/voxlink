/// Map voice quality preset index to Opus bitrate in bps
pub fn voice_quality_bitrate(quality: u8) -> i32 {
    match quality {
        0 => 16000,  // Economy — great for slow connections, minimal data
        1 => 32000,  // Standard — balanced quality and bandwidth
        3 => 128000, // Studio — maximum quality for podcasts/music
        _ => 64000,  // High (default) — clear voice, recommended
    }
}

/// Estimated kbps per user for a given voice quality preset (for UI display).
pub fn voice_quality_kbps(quality: u8) -> u32 {
    (voice_quality_bitrate(quality) / 1000) as u32
}

/// Display label for voice quality preset
pub fn voice_quality_label(quality: u8) -> &'static str {
    match quality {
        0 => "Economy",
        1 => "Standard",
        3 => "Studio",
        _ => "High",
    }
}

/// Extract the first URL (http:// or https://) from message content.
pub fn extract_first_url(content: &str) -> Option<String> {
    for word in content.split_whitespace() {
        if word.starts_with("http://") || word.starts_with("https://") {
            // Strip trailing punctuation that's likely not part of the URL
            let trimmed = word.trim_end_matches([',', '.', ')', ']', '>', ';']);
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Clean display host for a URL, e.g. "https://www.example.com/p?x=1" -> "example.com".
/// Strips scheme, userinfo, port, and a leading "www.". Only http(s) URLs.
pub fn extract_url_host(url: &str) -> Option<String> {
    let after_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    // Drop any "user:pass@" prefix, then the ":port" suffix.
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let host = host_port.split(':').next().unwrap_or("");
    let host = host.strip_prefix("www.").unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

#[cfg(test)]
mod url_host_tests {
    use super::*;

    #[test]
    fn extracts_clean_host() {
        assert_eq!(
            extract_url_host("https://www.example.com/path?x=1").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            extract_url_host("http://example.com").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            extract_url_host("https://sub.example.com:8080/x").as_deref(),
            Some("sub.example.com")
        );
        assert_eq!(
            extract_url_host("https://user:pw@host.com/x").as_deref(),
            Some("host.com")
        );
    }

    #[test]
    fn rejects_non_http_or_hostless() {
        assert_eq!(extract_url_host("ftp://example.com"), None);
        assert_eq!(extract_url_host("not a url"), None);
        assert_eq!(extract_url_host("https://"), None);
    }
}

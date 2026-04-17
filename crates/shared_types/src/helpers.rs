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

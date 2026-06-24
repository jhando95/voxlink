//! Opt-in OpenGraph link previews. Disabled unless `VOXLINK_LINK_PREVIEWS` is set
//! (the server stays a pure relay by default). When enabled, the server fetches a
//! posted URL's metadata behind a strict SSRF guard and a bounded reader.

use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

/// Extracted preview metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkPreviewData {
    pub title: Option<String>,
    pub description: Option<String>,
}

/// Whether an IP must never be fetched (SSRF guard): private, loopback,
/// link-local (incl. cloud metadata 169.254.169.254), CGNAT, multicast,
/// unspecified, broadcast, ULA, and IPv4-mapped forms of all of these.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10 (CGNAT)
        }
        IpAddr::V6(v6) => {
            // Re-check IPv4-mapped addresses against the IPv4 rules.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked_ip(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                || (seg[0] & 0xfe00) == 0xfc00 // ULA fc00::/7
                || (seg[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
        }
    }
}

/// Extract (title, description) from HTML, preferring OpenGraph tags and falling
/// back to `<title>` / `<meta name="description">`. Values are entity-decoded and
/// length-capped.
pub fn parse_open_graph(html: &str) -> LinkPreviewData {
    LinkPreviewData {
        title: meta_content(html, "og:title")
            .or_else(|| title_tag(html))
            .map(|s| truncate(&s, 200)),
        description: meta_content(html, "og:description")
            .or_else(|| meta_content(html, "description"))
            .map(|s| truncate(&s, 500)),
    }
}

/// Content of a `<meta>` tag whose `property` or `name` equals `key`.
fn meta_content(html: &str, key: &str) -> Option<String> {
    for chunk in html.split("<meta").skip(1) {
        let tag = &chunk[..chunk.find('>').unwrap_or(chunk.len())];
        let lower = tag.to_ascii_lowercase(); // ASCII-lowercase preserves byte positions
        let matches_key = [
            format!("property=\"{key}\""),
            format!("name=\"{key}\""),
            format!("property='{key}'"),
            format!("name='{key}'"),
        ]
        .iter()
        .any(|needle| lower.contains(needle.as_str()));
        if !matches_key {
            continue;
        }
        if let Some(content) = attr_value(tag, &lower, "content") {
            let decoded = decode_entities(content.trim());
            if !decoded.is_empty() {
                return Some(decoded);
            }
        }
    }
    None
}

/// Value of a quoted attribute within a tag. `lower` is the ASCII-lowercased tag
/// (same byte positions as `tag`) used for a case-insensitive name match.
fn attr_value(tag: &str, lower: &str, attr: &str) -> Option<String> {
    let pos = lower.find(&format!("{attr}="))?;
    let after = tag[pos + attr.len() + 1..].trim_start();
    let mut chars = after.chars();
    let quote = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None; // unquoted attributes not supported
    }
    let rest = &after[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn title_tag(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let close = lower[open_end..].find("</title>")? + open_end;
    let title = decode_entities(html[open_end..close].trim());
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('\u{2026}'); // …
        out
    }
}

/// Link previews are opt-in: the server only fetches URLs when
/// `VOXLINK_LINK_PREVIEWS` is set to a non-empty, non-"0" value.
pub fn previews_enabled() -> bool {
    std::env::var("VOXLINK_LINK_PREVIEWS")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

/// Split an http(s) URL into (host, port) for DNS resolution. IPv6 literals are
/// intentionally unsupported (such URLs simply won't preview).
fn host_and_port(url: &str) -> Option<(String, u16)> {
    let (default_port, rest) = if let Some(r) = url.strip_prefix("https://") {
        (443u16, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (80u16, r)
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(""); // strip userinfo
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && !h.contains(':') => {
            (h.to_string(), p.parse().unwrap_or(default_port))
        }
        _ => (authority.to_string(), default_port),
    };
    if host.is_empty() {
        None
    } else {
        Some((host, port))
    }
}

/// Fetch a URL and extract its preview metadata behind the SSRF guard.
/// Blocking (DNS + HTTP) — call from `spawn_blocking`. Returns `None` if the
/// host is non-public/unresolvable, the fetch fails, or no metadata is found.
pub fn fetch_link_preview(url: &str) -> Option<LinkPreviewData> {
    let (host, port) = host_and_port(url)?;

    // SSRF guard: resolve the host and require EVERY address to be public.
    let addrs: Vec<_> = (host.as_str(), port).to_socket_addrs().ok()?.collect();
    if addrs.is_empty() || addrs.iter().any(|a| is_blocked_ip(a.ip())) {
        log::warn!("link preview blocked: non-public or unresolvable host '{host}'");
        return None;
    }

    // No redirects (avoids redirect-based SSRF), bounded time.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(4)))
        .build()
        .into();
    let body = agent
        .get(url)
        .header("User-Agent", "Voxlink-LinkPreview/1.0")
        .header("Accept", "text/html")
        .call()
        .ok()?
        .into_body()
        .read_to_string()
        .ok()?;

    let preview = parse_open_graph(&body);
    if preview.title.is_none() && preview.description.is_none() {
        None
    } else {
        Some(preview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::str::FromStr;

    fn ip(s: &str) -> IpAddr {
        IpAddr::from_str(s).unwrap()
    }

    #[test]
    fn blocks_private_and_local_ipv4() {
        for s in [
            "10.0.0.1",
            "192.168.1.1",
            "172.16.5.4",
            "127.0.0.1",
            "169.254.169.254", // cloud metadata
            "0.0.0.0",
            "100.64.0.1", // CGNAT
            "255.255.255.255",
            "224.0.0.1", // multicast
        ] {
            assert!(is_blocked_ip(ip(s)), "{s} must be blocked");
        }
    }

    #[test]
    fn allows_public_ipv4() {
        for s in ["8.8.8.8", "1.1.1.1", "93.184.216.34"] {
            assert!(!is_blocked_ip(ip(s)), "{s} must be allowed");
        }
    }

    #[test]
    fn blocks_local_ipv6_and_mapped() {
        assert!(is_blocked_ip(ip("::1"))); // loopback
        assert!(is_blocked_ip(ip("fe80::1"))); // link-local
        assert!(is_blocked_ip(ip("fc00::1"))); // ULA
        assert!(is_blocked_ip(ip("::"))); // unspecified
        // IPv4-mapped private address must also be blocked.
        let mapped = IpAddr::V6(Ipv4Addr::new(10, 0, 0, 1).to_ipv6_mapped());
        assert!(is_blocked_ip(mapped));
    }

    #[test]
    fn allows_public_ipv6() {
        assert!(!is_blocked_ip(ip("2606:4700:4700::1111"))); // cloudflare
    }

    #[test]
    fn parses_open_graph_tags() {
        let html = r#"<html><head>
            <meta property="og:title" content="Hello &amp; World">
            <meta property="og:description" content="A nice page.">
            <title>Fallback Title</title>
        </head></html>"#;
        let p = parse_open_graph(html);
        assert_eq!(p.title.as_deref(), Some("Hello & World"));
        assert_eq!(p.description.as_deref(), Some("A nice page."));
    }

    #[test]
    fn falls_back_to_title_and_meta_description() {
        let html = r#"<head><title>Just A Title</title>
            <meta name="description" content="Meta desc."></head>"#;
        let p = parse_open_graph(html);
        assert_eq!(p.title.as_deref(), Some("Just A Title"));
        assert_eq!(p.description.as_deref(), Some("Meta desc."));
    }

    #[test]
    fn returns_empty_when_no_metadata() {
        let p = parse_open_graph("<html><body>nothing</body></html>");
        assert_eq!(p, LinkPreviewData::default());
    }
}

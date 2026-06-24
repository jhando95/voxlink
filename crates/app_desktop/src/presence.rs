//! Rich presence: broadcast the foreground app as activity ("Playing X").
//!
//! This is a pure client-side feature layered on the existing `SetActivity`
//! protocol — there is no new server message. A background task polls the
//! foreground app every few seconds, runs it through [`decide_presence`]
//! (allowlist gate + debounce), and only sends `SetActivity` when the
//! broadcast actually needs to change.
//!
//! Opt-in and allowlisted by design: an empty allowlist never broadcasts, so a
//! user can never accidentally leak an app they didn't explicitly permit.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use shared_types::SignalMessage;
use tokio::sync::Mutex as TokioMutex;

/// How often the poll task samples the foreground app. Deliberately slow (vs.
/// Discord's ~1s) to keep rich presence idle-friendly per the efficiency budget.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// What the presence poller should do this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceAction {
    /// Resolved activity is unchanged since the last broadcast — send nothing.
    Nothing,
    /// Broadcast this app as the user's activity (caller formats + sends).
    Set(String),
    /// We were broadcasting an activity but should no longer be — clear it.
    Clear,
}

/// Decide what activity update (if any) to broadcast.
///
/// - `enabled`: master opt-in toggle.
/// - `current_app`: foreground app detected this tick (`None` if unknown).
/// - `allowlist`: apps the user permits broadcasting. **Empty = never broadcast.**
/// - `last_sent`: the app name we last broadcast (`None` if we last cleared or
///   never sent).
///
/// The result is debounced: when the app we *would* broadcast equals
/// `last_sent`, this returns [`PresenceAction::Nothing`] so the poller stays
/// silent on the wire while the user keeps the same app focused.
pub fn decide_presence(
    enabled: bool,
    current_app: Option<&str>,
    allowlist: &[String],
    last_sent: Option<&str>,
) -> PresenceAction {
    // The app we *would* broadcast given the current toggle + allowlist.
    let resolved: Option<&str> = if enabled {
        match current_app {
            Some(app) if allowlist.iter().any(|a| a == app) => Some(app),
            _ => None,
        }
    } else {
        None
    };

    match (resolved, last_sent) {
        // Same app we already announced — debounce.
        (Some(app), Some(prev)) if app == prev => PresenceAction::Nothing,
        // A new (or first) allowlisted app to announce.
        (Some(app), _) => PresenceAction::Set(app.to_string()),
        // Nothing to announce now, but something is still out there — clear it.
        (None, Some(_)) => PresenceAction::Clear,
        // Nothing now, nothing before.
        (None, None) => PresenceAction::Nothing,
    }
}

/// Format a detected app name into a broadcast activity string.
pub fn format_activity(app: &str) -> String {
    format!("Playing {app}")
}

/// Detect the foreground application's name, or `None` if it can't be
/// determined. Shells out to a small platform query — no native binding crates.
///
/// This spawns a short-lived process, so call it off the UI thread (the poll
/// task wraps it in `spawn_blocking`). Returns `None` on any failure so a flaky
/// query never crashes or spams presence updates.
#[cfg(target_os = "macos")]
pub fn foreground_app() -> Option<String> {
    // System Events reports the frontmost process's name (e.g. "Safari").
    // First use prompts for Automation permission — acceptable for an opt-in
    // feature the user explicitly enabled.
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(
            "tell application \"System Events\" to get name of \
             first application process whose frontmost is true",
        )
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// GetForegroundWindow -> PID -> process name, via a small inline PowerShell.
#[cfg(target_os = "windows")]
pub fn foreground_app() -> Option<String> {
    const SCRIPT: &str = r#"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Fg {
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
}
"@
$procId = 0
$h = [Fg]::GetForegroundWindow()
[void][Fg]::GetWindowThreadProcessId($h, [ref]$procId)
try { (Get-Process -Id $procId -ErrorAction Stop).ProcessName } catch { "" }
"#;
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Linux/X11 foreground detection is too fragmented for v0.11 — disabled.
/// (The roadmap defers this to "detect-only-if-xorg".)
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn foreground_app() -> Option<String> {
    None
}

/// Parse a comma-separated allowlist string (as typed in settings) into the
/// stored form: trimmed, empties dropped, exact duplicates removed, order kept.
pub fn parse_allowlist(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in input.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !out.iter().any(|existing| existing == trimmed) {
            out.push(trimmed.to_string());
        }
    }
    out
}

/// Render a stored allowlist back into the comma-separated form shown in the UI.
pub fn join_allowlist(apps: &[String]) -> String {
    apps.join(", ")
}

/// Shared, runtime-mutable rich-presence settings. The settings UI writes these
/// (lock-free toggle + a short-held lock for the allowlist) and the poll task
/// reads them. Kept tiny so the idle path is just an atomic load.
#[derive(Clone)]
pub struct PresenceState {
    pub enabled: Arc<AtomicBool>,
    pub allowlist: Arc<Mutex<Vec<String>>>,
}

impl PresenceState {
    pub fn new(enabled: bool, allowlist: Vec<String>) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(enabled)),
            allowlist: Arc::new(Mutex::new(allowlist)),
        }
    }
}

/// Spawn the background poll task. It samples the foreground app every
/// [`POLL_INTERVAL`] and broadcasts `SetActivity` only when the resolved
/// activity changes. Costs ~nothing when disabled (one atomic load + sleep) and
/// never spawns the OS query unless presence is both enabled and connected.
pub fn spawn_poll_task(
    rt: &tokio::runtime::Handle,
    network: Arc<TokioMutex<net_control::NetworkClient>>,
    state: PresenceState,
) {
    rt.spawn(async move {
        let mut last_sent: Option<String> = None;
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        // Don't fire a burst if a tick is missed (e.g. machine sleep/wake).
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;

            let enabled = state.enabled.load(Ordering::Relaxed);
            // Cheapest idle path: feature off and nothing is live on the wire.
            if !enabled && last_sent.is_none() {
                continue;
            }

            // Only sample the OS (a process spawn) when we'd actually broadcast.
            let active = enabled && { network.lock().await.is_connected() };
            let allowlist = if active {
                state.allowlist.lock().map(|g| g.clone()).unwrap_or_default()
            } else {
                Vec::new()
            };
            let current = if active {
                tokio::task::spawn_blocking(foreground_app)
                    .await
                    .unwrap_or(None)
            } else {
                None
            };

            match decide_presence(active, current.as_deref(), &allowlist, last_sent.as_deref()) {
                PresenceAction::Nothing => {}
                PresenceAction::Set(app) => {
                    let activity = format_activity(&app);
                    let net = network.lock().await;
                    let _ = net.send_signal(&SignalMessage::SetActivity { activity }).await;
                    last_sent = Some(app);
                }
                PresenceAction::Clear => {
                    let net = network.lock().await;
                    let _ = net
                        .send_signal(&SignalMessage::SetActivity {
                            activity: String::new(),
                        })
                        .await;
                    last_sent = None;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(apps: &[&str]) -> Vec<String> {
        apps.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn disabled_never_sets() {
        let list = allow(&["Helldivers 2"]);
        assert_eq!(
            decide_presence(false, Some("Helldivers 2"), &list, None),
            PresenceAction::Nothing
        );
    }

    #[test]
    fn disabling_clears_a_live_presence() {
        let list = allow(&["Helldivers 2"]);
        // Toggle just turned off but we had been broadcasting.
        assert_eq!(
            decide_presence(false, Some("Helldivers 2"), &list, Some("Helldivers 2")),
            PresenceAction::Clear
        );
    }

    #[test]
    fn allowlisted_app_first_time_sets() {
        let list = allow(&["Helldivers 2"]);
        assert_eq!(
            decide_presence(true, Some("Helldivers 2"), &list, None),
            PresenceAction::Set("Helldivers 2".to_string())
        );
    }

    #[test]
    fn same_app_is_debounced() {
        let list = allow(&["Helldivers 2"]);
        assert_eq!(
            decide_presence(true, Some("Helldivers 2"), &list, Some("Helldivers 2")),
            PresenceAction::Nothing
        );
    }

    #[test]
    fn switching_to_a_different_allowlisted_app_sets() {
        let list = allow(&["Helldivers 2", "Spotify"]);
        assert_eq!(
            decide_presence(true, Some("Spotify"), &list, Some("Helldivers 2")),
            PresenceAction::Set("Spotify".to_string())
        );
    }

    #[test]
    fn non_allowlisted_app_with_nothing_live_does_nothing() {
        let list = allow(&["Helldivers 2"]);
        assert_eq!(
            decide_presence(true, Some("Secret.app"), &list, None),
            PresenceAction::Nothing
        );
    }

    #[test]
    fn switching_away_from_allowlisted_app_clears() {
        let list = allow(&["Helldivers 2"]);
        // User alt-tabbed to a non-allowlisted app while we were broadcasting.
        assert_eq!(
            decide_presence(true, Some("Secret.app"), &list, Some("Helldivers 2")),
            PresenceAction::Clear
        );
    }

    #[test]
    fn empty_allowlist_never_broadcasts() {
        let list: Vec<String> = Vec::new();
        assert_eq!(
            decide_presence(true, Some("Helldivers 2"), &list, None),
            PresenceAction::Nothing
        );
    }

    #[test]
    fn empty_allowlist_clears_any_live_presence() {
        let list: Vec<String> = Vec::new();
        assert_eq!(
            decide_presence(true, Some("Helldivers 2"), &list, Some("Helldivers 2")),
            PresenceAction::Clear
        );
    }

    #[test]
    fn unknown_foreground_clears_live_presence() {
        let list = allow(&["Helldivers 2"]);
        assert_eq!(
            decide_presence(true, None, &list, Some("Helldivers 2")),
            PresenceAction::Clear
        );
    }

    #[test]
    fn unknown_foreground_with_nothing_live_does_nothing() {
        let list = allow(&["Helldivers 2"]);
        assert_eq!(
            decide_presence(true, None, &list, None),
            PresenceAction::Nothing
        );
    }

    #[test]
    fn format_activity_prefixes_playing() {
        assert_eq!(format_activity("Helldivers 2"), "Playing Helldivers 2");
    }

    #[test]
    fn parse_allowlist_trims_and_drops_empties() {
        assert_eq!(
            parse_allowlist("  Helldivers 2 , , Spotify ,"),
            vec!["Helldivers 2".to_string(), "Spotify".to_string()]
        );
    }

    #[test]
    fn parse_allowlist_dedups_preserving_order() {
        assert_eq!(
            parse_allowlist("Code, Spotify, Code"),
            vec!["Code".to_string(), "Spotify".to_string()]
        );
    }

    #[test]
    fn parse_allowlist_empty_input_is_empty() {
        assert!(parse_allowlist("   ,  , ").is_empty());
        assert!(parse_allowlist("").is_empty());
    }

    #[test]
    fn join_allowlist_round_trips_through_parse() {
        let apps = vec!["Helldivers 2".to_string(), "Spotify".to_string()];
        assert_eq!(parse_allowlist(&join_allowlist(&apps)), apps);
    }
}

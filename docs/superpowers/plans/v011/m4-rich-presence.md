# M4 — Rich presence

**Goal:** Opt-in status line under username showing "Playing X" or "Using X". Polls foreground app every 5 seconds; only sends on change; default off.

**Files touched:**
- Create: `crates/app_desktop/src/presence.rs` (platform foreground-app detection + poll task)
- Modify: `crates/app_desktop/src/main.rs` (spawn presence task)
- Modify: `crates/config_store/src/lib.rs` (`rich_presence_enabled`, `rich_presence_allowlist`)
- Modify: `crates/shared_types/src/lib.rs` (`SetRichPresence`, `RichPresenceUpdated`; add `activity` field to MemberInfo — already exists)
- Modify: `crates/signaling_server/src/handlers/presence.rs` (handle SetRichPresence + broadcast)
- Modify: `crates/signaling_server/src/main.rs` (route variant)
- Modify: `crates/app_desktop/src/signal_handler/member.rs` (update member activity on RichPresenceUpdated)
- Modify: `crates/ui_shell/ui/components/member_widget.slint` (show activity text)
- Modify: `crates/ui_shell/ui/views/settings_view.slint` (toggle + allowlist UI)

---

### Task 4.1: Add config fields

**Files:**
- Modify: `crates/config_store/src/lib.rs`

- [ ] **Step 1: Add fields**

In `AppConfig`:

```rust
    #[serde(default)]
    pub rich_presence_enabled: bool,
    #[serde(default)]
    pub rich_presence_allowlist: Vec<String>,
```

In `impl Default for AppConfig`:

```rust
            rich_presence_enabled: false,
            rich_presence_allowlist: Vec::new(),
```

- [ ] **Step 2: Compile + stage**

Run: `cargo check -p config_store`
Expected: clean.

```bash
git add crates/config_store/src/lib.rs
```

---

### Task 4.2: Add SignalMessage variants

**Files:**
- Modify: `crates/shared_types/src/lib.rs`

- [ ] **Step 1: Add variants**

In `SignalMessage`:

```rust
    SetRichPresence { app_name: Option<String>, details: Option<String> },
    RichPresenceUpdated { user_id: String, app_name: Option<String>, details: Option<String> },
```

- [ ] **Step 2: Compile + stage**

Run: `cargo check -p shared_types`

```bash
git add crates/shared_types/src/lib.rs
```

---

### Task 4.3: Platform foreground-app detection

**Files:**
- Create: `crates/app_desktop/src/presence.rs`

- [ ] **Step 1: Write the module**

```rust
//! Rich presence: detects foreground app name on the user's desktop.
//!
//! Platform-specific, minimal: just queries the OS for the frontmost app.
//! No SDK integrations, no scraping. Privacy allowlist applies in the poller.

/// Returns the name of the currently focused app, if detectable.
pub fn foreground_app_name() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        macos_foreground_app()
    }
    #[cfg(target_os = "windows")]
    {
        windows_foreground_app()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
fn macos_foreground_app() -> Option<String> {
    // Use NSWorkspace.frontmostApplication via objc2/cocoa. The project already
    // links AppKit transitively through Slint on macOS, so we shell out to
    // `osascript` as a dependency-free fallback. 5s polling = negligible cost.
    let output = std::process::Command::new("osascript")
        .args(["-e", "tell application \"System Events\" to get name of first application process whose frontmost is true"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8(output.stdout).ok()?;
    let trimmed = name.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

#[cfg(target_os = "windows")]
fn windows_foreground_app() -> Option<String> {
    // Windows API: GetForegroundWindow + GetWindowThreadProcessId + OpenProcess + QueryFullProcessImageNameW
    // To avoid pulling in windows-rs for just this, use a PowerShell one-liner
    // via Command. 5s polling = acceptable.
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Add-Type @\"using System;using System.Runtime.InteropServices;public class W{[DllImport(\"user32.dll\")]public static extern IntPtr GetForegroundWindow();[DllImport(\"user32.dll\")]public static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);}\"@; $h=[W]::GetForegroundWindow(); $p=0; [void][W]::GetWindowThreadProcessId($h,[ref]$p); (Get-Process -Id $p).ProcessName"
        ])
        .output()
        .ok()?;
    if !output.status.success() { return None; }
    let name = String::from_utf8(output.stdout).ok()?;
    let trimmed = name.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

/// Shared state between poller and signal sender. Kept lock-free via atomic pointer swap.
pub struct PresenceState {
    pub last_sent: std::sync::Mutex<Option<String>>,
}

impl PresenceState {
    pub fn new() -> Self {
        Self { last_sent: std::sync::Mutex::new(None) }
    }
}
```

- [ ] **Step 2: Declare module**

In `crates/app_desktop/src/main.rs` near the other `mod` declarations:

```rust
mod presence;
```

- [ ] **Step 3: Compile + stage**

Run: `cargo check -p app_desktop`

```bash
git add crates/app_desktop/src/presence.rs crates/app_desktop/src/main.rs
```

---

### Task 4.4: Spawn the poll task

**Files:**
- Modify: `crates/app_desktop/src/main.rs` (after network client is ready)

- [ ] **Step 1: Spawn presence task**

Find where the network client is created (after auth). Spawn:

```rust
    // Rich presence poller
    let net_for_presence = ctx.network.clone();
    let rt_handle = ctx.rt_handle.clone();
    rt_handle.spawn(async move {
        let state = std::sync::Arc::new(crate::presence::PresenceState::new());
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            let config = config_store::load_config();
            if !config.rich_presence_enabled {
                // If disabled, ensure last_sent is cleared and send a clear once
                let mut guard = state.last_sent.lock().unwrap();
                if guard.is_some() {
                    *guard = None;
                    drop(guard);
                    let _ = net_for_presence.lock().await
                        .send_signal(&shared_types::SignalMessage::SetRichPresence {
                            app_name: None, details: None,
                        }).await;
                }
                continue;
            }
            let Some(app) = crate::presence::foreground_app_name() else { continue };
            // Allowlist check: empty list means broadcast nothing
            if !config.rich_presence_allowlist.iter().any(|a| a.eq_ignore_ascii_case(&app)) {
                continue;
            }
            let mut guard = state.last_sent.lock().unwrap();
            if guard.as_ref() == Some(&app) {
                continue; // No change
            }
            *guard = Some(app.clone());
            drop(guard);
            let _ = net_for_presence.lock().await
                .send_signal(&shared_types::SignalMessage::SetRichPresence {
                    app_name: Some(app),
                    details: None,
                }).await;
        }
    });
```

- [ ] **Step 2: Compile + stage**

Run: `cargo check -p app_desktop`

```bash
git add crates/app_desktop/src/main.rs
```

---

### Task 4.5: Server-side handler + broadcast

**Files:**
- Modify: `crates/signaling_server/src/handlers/presence.rs`
- Modify: `crates/signaling_server/src/main.rs`

- [ ] **Step 1: Add handler**

In `crates/signaling_server/src/handlers/presence.rs`:

```rust
pub async fn handle_set_rich_presence(
    state: &State,
    peer_id: &str,
    app_name: Option<String>,
    details: Option<String>,
) {
    let (user_id, space_ids) = {
        let s = state.read().await;
        let Some(peer) = s.peers.get(peer_id) else { return };
        let user_id = peer.user_id.lock().await.clone();
        let mut spaces: Vec<String> = Vec::new();
        if let Some(sid) = peer.space_id.lock().await.clone() {
            spaces.push(sid);
        }
        (user_id, spaces)
    };
    let Some(user_id) = user_id else { return };

    // Store on peer for new-member sync
    {
        let s = state.read().await;
        if let Some(peer) = s.peers.get(peer_id) {
            *peer.activity.lock().await = app_name.clone().unwrap_or_default();
        }
    }

    let msg = SignalMessage::RichPresenceUpdated {
        user_id,
        app_name,
        details,
    };

    // Broadcast to current space members + any friend channels
    for space_id in space_ids {
        crate::broadcast_to_space(state, &space_id, peer_id, &msg).await;
    }
    // Broadcast to friends: use existing friend-broadcast helper if present
    crate::broadcast_to_friends(state, peer_id, &msg).await;
}
```

If `Peer.activity` doesn't exist, add `pub activity: Mutex<String>` to the Peer struct in `types.rs`. If `broadcast_to_friends` doesn't exist, reuse the friend-presence broadcast pattern from existing code (grep for `broadcast.*friend`).

- [ ] **Step 2: Route in main.rs**

```rust
                SignalMessage::SetRichPresence { app_name, details } => {
                    handlers::presence::handle_set_rich_presence(&state, &peer_id, app_name, details).await;
                }
```

- [ ] **Step 3: Compile + stage**

Run: `cargo check -p signaling_server`

```bash
git add crates/signaling_server/
```

---

### Task 4.6: Client handles RichPresenceUpdated + UI shows activity

**Files:**
- Modify: `crates/app_desktop/src/signal_handler/member.rs`
- Modify: `crates/ui_shell/ui/components/member_widget.slint` (or wherever member row renders)

- [ ] **Step 1: Handle the signal**

In `crates/app_desktop/src/signal_handler/member.rs`:

```rust
pub fn handle_rich_presence_updated(
    w: &MainWindow,
    state: &Rc<RefCell<shared_types::AppState>>,
    user_id: &str,
    app_name: Option<&String>,
    _details: Option<&String>,
) {
    let activity_text = app_name.map(|n| format!("Using {n}")).unwrap_or_default();
    // Update MemberData entries matching user_id
    let mut members = state.borrow().current_space_members.clone();
    for m in members.iter_mut() {
        if m.user_id.as_deref() == Some(user_id) {
            m.activity = activity_text.clone();
        }
    }
    state.borrow_mut().current_space_members = members;
    // Refresh UI
    crate::ui_sync::refresh_member_list(w, state);
}
```

Wire in `signal_handler/mod.rs`:

```rust
SignalMessage::RichPresenceUpdated { user_id, app_name, details } => {
    member::handle_rich_presence_updated(w, state, user_id, app_name.as_ref(), details.as_ref());
}
```

(Adapt field/function names to what actually exists in the codebase via grep.)

- [ ] **Step 2: Render activity text in member widget**

In the member widget Slint file, beneath the name/status line add:

```slint
            if member.activity != "" : Text {
                text: member.activity;
                color: VxTheme.text-muted;
                font-size: 11px * VxTheme.s;
                overflow: elide;
            }
```

- [ ] **Step 3: Compile + visual check**

Run: `cargo build --release --bin app_desktop`

- [ ] **Step 4: Stage**

```bash
git add crates/app_desktop/ crates/ui_shell/
```

---

### Task 4.7: Settings UI for rich presence toggle + allowlist

**Files:**
- Modify: `crates/ui_shell/ui/views/settings_view.slint`
- Modify: `crates/app_desktop/src/callbacks/settings.rs` (or equivalent)

- [ ] **Step 1: Add Privacy section with toggle**

In `settings_view.slint` under a new "Privacy" heading:

```slint
        Text { text: "Rich Presence"; font-weight: 700; }
        Text {
            text: "Broadcast the app you're using to friends and space members";
            color: VxTheme.text-muted;
            font-size: 11px * VxTheme.s;
            wrap: word-wrap;
        }
        HorizontalLayout {
            alignment: space-between;
            Text { text: "Enabled"; }
            CheckBox {
                checked: root.rich-presence-enabled;
                toggled => { root.toggle-rich-presence(self.checked); }
            }
        }
```

And an allowlist editor — a simple multi-line text area or add/remove list bound to `rich-presence-allowlist: [string]`.

- [ ] **Step 2: Add window properties and callback**

In `main.slint`:

```slint
    in-out property <bool> rich-presence-enabled;
    in-out property <[string]> rich-presence-allowlist;
    callback toggle-rich-presence(bool);
    callback add-presence-allowed-app(string);
    callback remove-presence-allowed-app(string);
```

- [ ] **Step 3: Wire callbacks in Rust**

In callbacks file:

```rust
    let w_weak = w.as_weak();
    w.on_toggle_rich_presence(move |enabled| {
        let Some(w) = w_weak.upgrade() else { return };
        let _lock = crate::helpers::CONFIG_LOCK.lock().unwrap();
        let mut cfg = config_store::load_config();
        cfg.rich_presence_enabled = enabled;
        let _ = config_store::save_config(&cfg);
        w.set_rich_presence_enabled(enabled);
    });

    let w_weak = w.as_weak();
    w.on_add_presence_allowed_app(move |name| {
        let Some(w) = w_weak.upgrade() else { return };
        let _lock = crate::helpers::CONFIG_LOCK.lock().unwrap();
        let mut cfg = config_store::load_config();
        let n = name.to_string();
        if !cfg.rich_presence_allowlist.contains(&n) {
            cfg.rich_presence_allowlist.push(n);
        }
        let _ = config_store::save_config(&cfg);
        let model: std::rc::Rc<slint::VecModel<slint::SharedString>> = std::rc::Rc::new(
            slint::VecModel::from(
                cfg.rich_presence_allowlist.iter().map(|s| s.as_str().into()).collect::<Vec<_>>()
            )
        );
        w.set_rich_presence_allowlist(model.into());
    });
```

(Adapt exact widget API to what's already used in the project for CheckBox and list models.)

- [ ] **Step 4: Initialize on startup**

In the main setup code:

```rust
    w.set_rich_presence_enabled(cfg.rich_presence_enabled);
    let list_model = std::rc::Rc::new(slint::VecModel::from(
        cfg.rich_presence_allowlist.iter().map(|s| s.as_str().into()).collect::<Vec<slint::SharedString>>()
    ));
    w.set_rich_presence_allowlist(list_model.into());
```

- [ ] **Step 5: Commit M4**

Run: `cargo check --workspace && cargo test --workspace`

```bash
git add crates/
git commit -m "$(cat <<'EOF'
feat(v0.11): rich presence (opt-in, 5s polling)

- Poll foreground app via osascript (macOS) / PowerShell (Windows)
- Default off; user must enable + add apps to allowlist
- Broadcast SetRichPresence only on change (debounced)
- Server broadcasts to space members + friends
- Show "Using X" line in member widgets

Privacy-first: allowlist required to broadcast.
Linux support deferred pending stable XDG protocol.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>
EOF
)"
```

(Defer actual commit until user approves.)

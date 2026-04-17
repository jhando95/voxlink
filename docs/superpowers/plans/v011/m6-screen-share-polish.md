# M6 — Screen share polish

**Goal:** Viewer zoom/fit, pop-out window, pause-when-hidden. No wasted CPU when viewer is minimized.

**Files touched:**
- Modify: `crates/ui_shell/ui/components/screen_share_widget.slint` (zoom, fit, pop-out buttons)
- Modify: `crates/app_desktop/src/screen_share.rs` (pause decode when viewer hidden)
- Modify: `crates/ui_shell/ui/main.slint` (pop-out window instance)
- Modify: `crates/app_desktop/src/callbacks/room.rs` (open pop-out window on demand)

**Note on transport split:** The original spec proposed a separate UDP lane. Upon review of `net_control/src/lib.rs`, screen frames already use `MEDIA_PACKET_SCREEN` / `MEDIA_PACKET_SCREEN_CHUNK` packet types over the same UDP socket, with a `screen_latest` latest-only queue for audio/screen frame separation. The existing design already achieves the "separate lane" intent. **No transport changes needed for M6.** Verify this with the test in Task 6.4.

---

### Task 6.1: Add viewer fit/zoom controls

**Files:**
- Modify: `crates/ui_shell/ui/components/screen_share_widget.slint` (or wherever viewer lives)

- [ ] **Step 1: Locate the widget**

Run: `grep -rn "screen.share\|ScreenShare" /Users/jph/Voiceapp/workspace_template/crates/ui_shell/ui/`
Expected: shows the viewer component(s).

- [ ] **Step 2: Add zoom/fit state**

```slint
component ScreenShareViewer inherits Rectangle {
    in property <image> frame;
    in property <string> sharer-name;
    in-out property <string> fit-mode: "fit";   // "fit" | "100" | "200"
    callback pop-out();
    callback close();

    clip: true;
    background: VxTheme.surface-muted;

    image := Image {
        source: root.frame;
        image-fit: root.fit-mode == "fit" ? contain : preserve;
        width: root.fit-mode == "100" ? root.frame.width * 1px
             : root.fit-mode == "200" ? root.frame.width * 2px
             : parent.width;
        height: root.fit-mode == "100" ? root.frame.height * 1px
              : root.fit-mode == "200" ? root.frame.height * 2px
              : parent.height;
    }

    // Toolbar overlay
    Rectangle {
        x: 8px; y: 8px;
        width: 220px; height: 36px;
        background: VxTheme.surface-elevated;
        border-radius: 6px;
        opacity: ta-widget.has-hover ? 1.0 : 0.0;
        animate opacity { duration: 200ms; }
        HorizontalLayout {
            padding: 4px;
            spacing: 4px;
            Rectangle {
                background: root.fit-mode == "fit" ? VxTheme.accent : transparent;
                border-radius: 4px;
                width: 50px;
                Text { text: "Fit"; color: VxTheme.text; }
                TouchArea { clicked => { root.fit-mode = "fit"; } }
            }
            Rectangle {
                background: root.fit-mode == "100" ? VxTheme.accent : transparent;
                border-radius: 4px;
                width: 50px;
                Text { text: "100%"; color: VxTheme.text; }
                TouchArea { clicked => { root.fit-mode = "100"; } }
            }
            Rectangle {
                background: root.fit-mode == "200" ? VxTheme.accent : transparent;
                border-radius: 4px;
                width: 50px;
                Text { text: "200%"; color: VxTheme.text; }
                TouchArea { clicked => { root.fit-mode = "200"; } }
            }
            Rectangle {
                border-radius: 4px;
                width: 50px;
                Text { text: "Pop-out"; color: VxTheme.text-muted; font-size: 10px * VxTheme.s; }
                TouchArea { clicked => { root.pop-out(); } }
            }
        }
    }

    ta-widget := TouchArea { }
}
```

- [ ] **Step 3: Build and visual-check**

Run: `cargo build --release --bin app_desktop`
Manual: start a screen share, hover the viewer, confirm fit/100%/200% buttons work.

- [ ] **Step 4: Stage**

```bash
git add crates/ui_shell/ui/
```

---

### Task 6.2: Pause decode when viewer hidden

**Files:**
- Modify: `crates/app_desktop/src/screen_share.rs` (or wherever JPEG decode happens)
- Modify: `crates/ui_shell/ui/main.slint` (track viewer visibility)

- [ ] **Step 1: Add visibility flag**

In `main.slint` add an in-out property reflecting viewer visibility:

```slint
    in-out property <bool> screen-share-viewer-visible: true;
```

Hook its setter in Rust to an atomic flag. Find the screen share controller:

Run: `grep -n "pub struct ScreenShareController" /Users/jph/Voiceapp/workspace_template/crates/app_desktop/src/screen_share.rs`

Add field:

```rust
pub struct ScreenShareController {
    // ... existing fields ...
    pub viewer_visible: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
```

- [ ] **Step 2: Skip decode when hidden**

In the tick_loop or wherever the latest frame is drained and decoded:

```rust
if !ctx.screen_share.viewer_visible.load(std::sync::atomic::Ordering::Relaxed) {
    // Drop any pending frames to avoid memory growth; don't decode.
    let _ = ctx.network.blocking_lock_if_possible()
        .map(|n| n.drop_pending_screen_frames());
    return;
}
// ... existing decode path ...
```

Provide a helper `drop_pending_screen_frames()` in `net_control` that clears the `screen_latest` slot without decoding.

- [ ] **Step 3: Hook window minimize/visible state**

In the main window callbacks (visibility change handlers), set the flag:

```rust
    // Slint MainWindow exposes shown/hidden via binding
    let w_weak = w.as_weak();
    let flag = ctx.screen_share.viewer_visible.clone();
    w.on_window_visibility_changed(move |visible| {
        flag.store(visible, std::sync::atomic::Ordering::Relaxed);
    });
```

If Slint doesn't expose a window-visibility change callback directly, poll `w.window().is_visible()` from the tick_loop (25 ms cadence) and update the flag only on transition.

- [ ] **Step 4: Compile + verify**

Run: `cargo check -p app_desktop`

Manual verification:
- Start a screen share.
- Minimize the viewer window.
- Confirm CPU usage drops to baseline (compared to pre-pause behavior).

- [ ] **Step 5: Stage**

```bash
git add crates/app_desktop/ crates/ui_shell/ui/main.slint
```

---

### Task 6.3: Pop-out secondary window (optional polish)

**Files:**
- Modify: `crates/ui_shell/ui/main.slint` (define pop-out component)
- Modify: `crates/app_desktop/src/callbacks/room.rs`

**Constraint:** Slint 1.15 supports multiple `Window { }` instances at the top level. We define a `PopOutShareWindow` and instantiate it from Rust on demand.

- [ ] **Step 1: Define pop-out window**

At the top level of `main.slint`:

```slint
export component PopOutShareWindow inherits Window {
    title: "Screen Share";
    preferred-width: 960px;
    preferred-height: 540px;
    in property <image> frame;
    background: #000;
    Image {
        source: root.frame;
        image-fit: contain;
        width: parent.width;
        height: parent.height;
    }
}
```

- [ ] **Step 2: Instantiate on `pop-out` callback**

In the room-view callback file, on the `pop-out` callback:

```rust
    use slint::ComponentHandle;
    let popout = PopOutShareWindow::new().unwrap();
    popout.set_frame(current_frame_image.clone());
    // Keep a handle alive (box into app state) so it doesn't drop immediately
    state_rc.borrow_mut().popout_window = Some(popout.clone());
    popout.show().unwrap();
```

Update the frame periodically from the tick loop — send the latest decoded image to both the main window and the pop-out:

```rust
if let Some(popout) = state.borrow().popout_window.as_ref() {
    popout.set_frame(frame.clone());
}
```

- [ ] **Step 3: Handle close event**

```rust
    let state_weak = state_rc.downgrade();
    popout.window().on_close_requested(move || {
        if let Some(state) = state_weak.upgrade() {
            state.borrow_mut().popout_window = None;
        }
        slint::CloseRequestResponse::HideWindow
    });
```

- [ ] **Step 4: Compile + manual test**

Run: `cargo build --release --bin app_desktop`

Manual: start share, click "Pop-out", confirm secondary window opens with live frame.

- [ ] **Step 5: Stage**

```bash
git add crates/ui_shell/ crates/app_desktop/
```

---

### Task 6.4: Verify voice quality during screen share

**Files:**
- Modify: `crates/integration_tests/tests/server_tests.rs` (optional benchmark)

- [ ] **Step 1: Manual efficiency measurement**

Launch two app instances. In app A, start a screen share at 8 fps 960px quality 55 (current default). In app B, join the share and enter a voice call with a third peer.

Measure jitter in the perf panel (or log output) during the share. Expected: jitter stays ≤ 5 ms regression from pre-M6 baseline.

- [ ] **Step 2: Commit M6**

Run: `cargo check --workspace && cargo test --workspace`

```bash
git add crates/
git commit -m "$(cat <<'EOF'
feat(v0.11): screen share polish

- Fit / 100% / 200% zoom controls
- Pop-out secondary window
- Pause decode when viewer hidden (no wasted CPU on minimized)
- Hover-reveal toolbar overlay

No transport changes — existing UDP packet-type discrimination
already separates screen/audio lanes per spec analysis.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>
EOF
)"
```

(Defer commit until user approves.)

---

## v0.11 release checklist (after all 6 milestones merged)

- [ ] `cargo check --workspace` clean (zero warnings)
- [ ] `cargo test --workspace` all passing
- [ ] Idle CPU measured: ≤ 0.5% on M-series
- [ ] Idle RAM measured: ≤ 120 MB
- [ ] Binary size: `ls -lh target/release/app_desktop` ≤ 40 MB
- [ ] Screen share voice-jitter regression: ≤ 5 ms
- [ ] Bump version in `Cargo.toml` (workspace), `installer/voxlink.iss`, `README.md` to `0.11.0`
- [ ] Update `crates/ui_shell/ui/main.slint` version-text to "0.11.0"
- [ ] Tag `v0.11.0-rc1`, push, verify CI installers build
- [ ] Soak test: 1-hour session with emojis, screen share, presence, stickers
- [ ] Deploy server: `./deploy/push-to-server.sh ubuntu@129.158.231.26`
- [ ] Tag `v0.11.0`, publish release
- [ ] Update memory: bump version in `/Users/jph/.claude/projects/-Users-jph-Voiceapp/memory/MEMORY.md`

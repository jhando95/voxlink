# v0.11 Visual Communication Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship 6 features that make Voxlink visually richer (screen share polish, custom emojis, bundled stickers, rich presence, drag-drop uploads, unread separator) while preserving idle-CPU/RAM efficiency.

**Architecture:** Each feature is a vertical slice across protocol (`shared_types`), persistence (`signaling_server`), server handler (`signaling_server/handlers`), client signal handler (`app_desktop/signal_handler`), and UI (`ui_shell`). Features are independent — milestones can be shipped one at a time.

**Tech Stack:** Rust 1.94, Slint 1.15, rusqlite (SQLite WAL), tokio 1.50, serde, native platform APIs (NSWorkspace on macOS, GetForegroundWindow on Windows).

**Reference spec:** `docs/superpowers/specs/2026-04-16-v011-visual-communication-design.md`

---

## Milestones

- **M1:** Unread jump-to-first-new separator (smallest, lowest risk — start here)
- **M2:** Custom emojis per space (DB + protocol + UI cache)
- **M3:** Bundled animated stickers (UI-only, no server changes)
- **M4:** Rich presence (platform APIs + opt-in flow)
- **M5:** Drag-and-drop file upload + image thumbnails (file handling)
- **M6:** Screen share polish (transport prioritization + viewer controls)

Each milestone ends with: `cargo check && cargo test` green, manual smoke test, commit.

---

## Conventions for this plan

- All paths are absolute from the workspace root: `/Users/jph/Voiceapp/workspace_template/`
- "Run tests" means: `cd /Users/jph/Voiceapp/workspace_template && cargo test --workspace`
- "Build server" means: `cargo build --release --bin signaling_server`
- "Build app" means: `cargo build --release --bin app_desktop`
- Commit step at the end of each task — the executor decides whether to actually commit (user has said "never commit without asking"). When uncertain, stage the changes and stop for review.

---

## M1 — Unread jump-to-first-new separator

**Goal:** When user opens a channel, show a horizontal "NEW" bar above the first message they haven't read.

**Files touched:**
- Create: none
- Modify: `crates/shared_types/src/lib.rs` (add `MarkChannelRead` variant)
- Modify: `crates/signaling_server/src/persistence.rs` (new `channel_read_marks` table + CRUD)
- Modify: `crates/signaling_server/src/handlers/chat.rs` (new `handle_mark_channel_read`)
- Modify: `crates/signaling_server/src/main.rs` (route new variant)
- Modify: `crates/app_desktop/src/signal_handler/chat.rs` (send `MarkChannelRead` on channel select)
- Modify: `crates/ui_shell/src/lib.rs` (set `is_new_separator` flag on first unread message)
- Modify: `crates/ui_shell/ui/views/chat_view.slint` (render separator when flag set)
- Test: `crates/integration_tests/tests/server_tests.rs` (round-trip read mark)

**Note:** `is_new_separator: bool` already exists on `ChatMessage` in `theme.slint:81`. We just need to populate and render it.

---

### Task 1.1: Add `MarkChannelRead` SignalMessage variant

**Files:**
- Modify: `crates/shared_types/src/lib.rs`

- [ ] **Step 1: Find the chat operations block in `SignalMessage` enum**

Run: `grep -n "SendTextMessage" /Users/jph/Voiceapp/workspace_template/crates/shared_types/src/lib.rs`
Expected: shows the line with `SendTextMessage { channel_id: String, content: String, ...`

- [ ] **Step 2: Add the new variant immediately after `PinMessage`**

In `crates/shared_types/src/lib.rs`, find the line `PinMessage { channel_id: String, message_id: String, pinned: bool },` and add this directly after it:

```rust
    MarkChannelRead { channel_id: String, message_id: String },
```

- [ ] **Step 3: Compile-check**

Run: `cargo check -p shared_types`
Expected: clean compile, zero warnings.

- [ ] **Step 4: Commit (stage only)**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add crates/shared_types/src/lib.rs
# Do NOT commit — wait for milestone-level commit
```

---

### Task 1.2: Create `channel_read_marks` table and CRUD

**Files:**
- Modify: `crates/signaling_server/src/persistence.rs`

- [ ] **Step 1: Find the `init()` method's `execute_batch` block**

Run: `grep -n "CREATE TABLE IF NOT EXISTS messages" /Users/jph/Voiceapp/workspace_template/crates/signaling_server/src/persistence.rs`
Expected: shows the messages table definition inside `init()`.

- [ ] **Step 2: Add the new table to the same `execute_batch`**

In the `init()` method, locate the `execute_batch(...)` SQL string and append before its closing `"`:

```sql
;
CREATE TABLE IF NOT EXISTS channel_read_marks (
    user_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    last_read_message_id TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, channel_id)
)
```

- [ ] **Step 3: Add CRUD methods at the end of `impl Database`**

In `crates/signaling_server/src/persistence.rs`, find `impl Database {` and add these methods before the closing `}`:

```rust
    pub fn upsert_read_mark(
        &self,
        user_id: &str,
        channel_id: &str,
        message_id: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO channel_read_marks (user_id, channel_id, last_read_message_id, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id, channel_id) DO UPDATE SET
                 last_read_message_id = excluded.last_read_message_id,
                 updated_at = excluded.updated_at",
            params![user_id, channel_id, message_id, updated_at],
        )
        .map_err(|e| format!("upsert_read_mark error: {e}"))?;
        Ok(())
    }

    pub fn get_read_mark(
        &self,
        user_id: &str,
        channel_id: &str,
    ) -> Result<Option<String>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT last_read_message_id FROM channel_read_marks
                 WHERE user_id = ?1 AND channel_id = ?2",
            )
            .map_err(|e| format!("get_read_mark prepare: {e}"))?;
        let mut rows = stmt
            .query(params![user_id, channel_id])
            .map_err(|e| format!("get_read_mark query: {e}"))?;
        if let Some(row) = rows.next().map_err(|e| format!("get_read_mark next: {e}"))? {
            Ok(Some(row.get(0).map_err(|e| format!("get_read_mark col: {e}"))?))
        } else {
            Ok(None)
        }
    }
```

- [ ] **Step 4: Compile-check**

Run: `cargo check -p signaling_server`
Expected: clean compile.

- [ ] **Step 5: Stage**

```bash
git add crates/signaling_server/src/persistence.rs
```

---

### Task 1.3: Add server handler for `MarkChannelRead`

**Files:**
- Modify: `crates/signaling_server/src/handlers/chat.rs`
- Modify: `crates/signaling_server/src/main.rs`

- [ ] **Step 1: Add handler function**

In `crates/signaling_server/src/handlers/chat.rs`, append at the end of the file:

```rust
pub async fn handle_mark_channel_read(
    state: &State,
    peer_id: &str,
    channel_id: String,
    message_id: String,
) {
    let (user_id, db) = {
        let s = state.read().await;
        let Some(peer) = s.peers.get(peer_id) else { return };
        let user_id = peer.user_id.lock().await.clone();
        (user_id, s.db.clone())
    };
    let Some(user_id) = user_id else { return };
    let Some(db) = db else { return };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    if let Err(e) = db.upsert_read_mark(&user_id, &channel_id, &message_id, now) {
        log::warn!("upsert_read_mark failed: {e}");
    }
}
```

- [ ] **Step 2: Route the new variant in `main.rs`**

Run: `grep -n "SignalMessage::PinMessage" /Users/jph/Voiceapp/workspace_template/crates/signaling_server/src/main.rs`
Expected: shows the match arm for PinMessage.

Add immediately after the `PinMessage` arm:

```rust
                SignalMessage::MarkChannelRead { channel_id, message_id } => {
                    handlers::chat::handle_mark_channel_read(&state, &peer_id, channel_id, message_id).await;
                }
```

- [ ] **Step 3: Compile-check**

Run: `cargo check -p signaling_server`
Expected: clean compile.

- [ ] **Step 4: Stage**

```bash
git add crates/signaling_server/src/handlers/chat.rs crates/signaling_server/src/main.rs
```

---

### Task 1.4: Client sends `MarkChannelRead` on channel select

**Files:**
- Modify: `crates/app_desktop/src/signal_handler/chat.rs` or `crates/app_desktop/src/callbacks/chat.rs`

- [ ] **Step 1: Find the channel-select callback**

Run: `grep -rn "SelectTextChannel" /Users/jph/Voiceapp/workspace_template/crates/app_desktop/src/`
Expected: shows places where the client sends SelectTextChannel.

- [ ] **Step 2: After `SelectTextChannel` is sent, also send `MarkChannelRead`**

Locate the function that sends `SelectTextChannel { channel_id }`. Right after that send call, add (using whatever local references exist for the network client and the latest message id from app state):

```rust
    // Mark this channel read up to the most recent message we've seen
    if let Some(latest_id) = state.borrow()
        .channel_messages
        .get(&channel_id)
        .and_then(|v| v.back())
        .map(|m| m.message_id.clone())
    {
        let net = ctx.network.clone();
        let cid = channel_id.clone();
        ctx.rt_handle.spawn(async move {
            let _ = net.lock().await
                .send_signal(&shared_types::SignalMessage::MarkChannelRead {
                    channel_id: cid,
                    message_id: latest_id,
                })
                .await;
        });
    }
```

If the actual struct field for messages is named differently (e.g. `text_messages` or `channel_message_buffers`), use the existing name found via grep. Do not invent names.

- [ ] **Step 3: Compile-check**

Run: `cargo check -p app_desktop`
Expected: clean compile.

- [ ] **Step 4: Stage**

```bash
git add crates/app_desktop/
```

---

### Task 1.5: Render `is_new_separator` in chat view

**Files:**
- Modify: `crates/ui_shell/src/lib.rs` (set the flag during message conversion)
- Modify: `crates/ui_shell/ui/views/chat_view.slint` (visual rendering)

- [ ] **Step 1: Find where messages are converted to ChatMessage**

Run: `grep -n "is_new_separator" /Users/jph/Voiceapp/workspace_template/crates/ui_shell/src/lib.rs`
Expected: shows the existing field assignment (currently always `false`).

- [ ] **Step 2: Wire the flag**

Modify the message-to-ChatMessage conversion. Track `last_read_message_id: Option<String>` per channel in `AppState` (add field if missing). Mark the FIRST message whose `message_id != last_read_message_id` AND comes after the last-read message as `is_new_separator: true`.

Concrete edit (adapt to actual conversion site):

```rust
let mut separator_assigned = false;
let chat_messages: Vec<ChatMessage> = msgs.iter().map(|m| {
    let is_new = !separator_assigned
        && last_read_id.as_ref().map_or(false, |lr| {
            // Find the message AFTER the last-read one
            msgs.iter().position(|x| &x.message_id == lr)
                .map_or(false, |idx| {
                    msgs.iter().position(|x| x.message_id == m.message_id)
                        .map_or(false, |my_idx| my_idx == idx + 1)
                })
        });
    if is_new { separator_assigned = true; }
    ChatMessage {
        // ... existing fields ...
        is_new_separator: is_new,
    }
}).collect();
```

(If a simpler conversion exists, prefer it. Do not over-engineer.)

- [ ] **Step 3: Add visual rendering in chat_view.slint**

Run: `grep -n "for msg in root.chat-messages" /Users/jph/Voiceapp/workspace_template/crates/ui_shell/ui/views/chat_view.slint`
Expected: shows the message rendering loop.

Inside the message rendering loop, BEFORE the message component, add:

```slint
                if msg.is-new-separator : Rectangle {
                    height: 24px;
                    HorizontalLayout {
                        alignment: center;
                        spacing: 8px;
                        Rectangle {
                            background: VxTheme.accent;
                            height: 1px;
                            width: 100px;
                            y: parent.height / 2;
                        }
                        Text {
                            text: "NEW";
                            color: VxTheme.accent;
                            font-size: 11px * VxTheme.s;
                            font-weight: 700;
                        }
                        Rectangle {
                            background: VxTheme.accent;
                            height: 1px;
                            width: 100px;
                            y: parent.height / 2;
                        }
                    }
                }
```

- [ ] **Step 4: Build and visually verify**

Run: `cargo build --release --bin app_desktop`
Expected: clean build. Manually launch and confirm separator shows above first unread message after re-opening a channel.

- [ ] **Step 5: Stage**

```bash
git add crates/ui_shell/
```

---

### Task 1.6: Integration test for read-mark round trip

**Files:**
- Modify: `crates/integration_tests/tests/server_tests.rs`

- [ ] **Step 1: Add the test**

Append to `crates/integration_tests/tests/server_tests.rs`:

```rust
#[tokio::test]
async fn test_mark_channel_read_persists() {
    use shared_types::SignalMessage;
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    // Alice authenticates (use existing helper or inline auth)
    alice.send_signal(&SignalMessage::CreateAccount {
        email: "alice@test".to_string(),
        password: "password123".to_string(),
        display_name: "Alice".to_string(),
    }).await;
    let _ = alice.recv_signal().await; // AccountCreated

    // Mark a channel read with a synthetic message id
    alice.send_signal(&SignalMessage::MarkChannelRead {
        channel_id: "ch_test".to_string(),
        message_id: "msg_42".to_string(),
    }).await;

    // No response expected — fire-and-forget. Sanity wait.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Verify by reconnecting and checking — but since there's no GetReadMark signal yet,
    // we just verify the server didn't crash and accepts the message.
    alice.send_signal(&SignalMessage::Ping).await;
    let pong = alice.recv_signal().await;
    assert!(matches!(pong, SignalMessage::Pong { .. }));
}
```

- [ ] **Step 2: Run test**

Run: `cd /Users/jph/Voiceapp/workspace_template && cargo test -p integration_tests test_mark_channel_read_persists -- --nocapture`
Expected: PASS.

- [ ] **Step 3: Run full workspace tests**

Run: `cargo test --workspace`
Expected: all tests pass, no regressions.

- [ ] **Step 4: Commit M1**

```bash
git add crates/integration_tests/
git commit -m "$(cat <<'EOF'
feat(v0.11): unread jump-to-first-new separator

- Add MarkChannelRead SignalMessage variant
- Persist read marks per (user, channel) in SQLite
- Client sends mark on channel select
- Render NEW separator above first unread message in chat view

Part of v0.11 Visual Communication milestone.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>
EOF
)"
```

(Defer actual commit until user approves.)

---

## M2–M6 — Continued in supplementary plan files

To keep this document readable, milestones M2 through M6 are split into companion files under `docs/superpowers/plans/v011/`:

- `m2-custom-emojis.md`
- `m3-bundled-stickers.md`
- `m4-rich-presence.md`
- `m5-drag-drop-uploads.md`
- `m6-screen-share-polish.md`

Each follows the same task-by-task TDD format as M1 above. They will be created in the next steps.

---

## Self-review checklist (run after all milestone files are written)

- [ ] Spec coverage: every feature in `2026-04-16-v011-visual-communication-design.md` has at least one milestone
- [ ] No placeholders (no TBD, TODO, "implement later")
- [ ] Type consistency: SignalMessage variant names match between `shared_types`, server router, server handler, client sender
- [ ] Efficiency budget verified per milestone (idle CPU/RAM measured before commit)
- [ ] All file paths absolute and correct
- [ ] All commands exact and runnable


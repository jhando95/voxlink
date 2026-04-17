# M2 — Custom emojis per space

**Goal:** Spaces can upload up to 100 custom emojis (256 KB max each), used in chat as `:name:` shortcuts and in the reaction picker.

**Files touched:**
- Modify: `crates/shared_types/src/lib.rs` (6 new SignalMessage variants + `EmojiInfo` struct)
- Modify: `crates/signaling_server/src/persistence.rs` (`space_emojis` table + 4 CRUD methods)
- Create: `crates/signaling_server/src/handlers/emoji.rs` (4 handler functions)
- Modify: `crates/signaling_server/src/handlers/mod.rs` (re-export)
- Modify: `crates/signaling_server/src/main.rs` (route 4 new variants)
- Modify: `crates/config_store/src/lib.rs` (add `emoji_cache_limit_mb: u32`)
- Modify: `crates/ui_shell/src/lib.rs` (cache + render :name: substitution)
- Modify: `crates/ui_shell/ui/views/space_view.slint` (emoji management tab)
- Modify: `crates/app_desktop/src/signal_handler/space.rs` (handle EmojiList/EmojiUploaded)
- Test: `crates/integration_tests/tests/server_tests.rs` (upload + list)

---

### Task 2.1: Define `EmojiInfo` struct + 6 SignalMessage variants

**Files:**
- Modify: `crates/shared_types/src/lib.rs`

- [ ] **Step 1: Add `EmojiInfo` struct**

After the existing `MemberInfo` struct definition in `crates/shared_types/src/lib.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmojiInfo {
    pub id: String,
    pub space_id: String,
    pub name: String,           // without colons
    pub image_type: String,     // "png" | "webp"
    pub uploaded_by: String,
    pub created_at: u64,
}
```

- [ ] **Step 2: Add 6 SignalMessage variants**

In the `SignalMessage` enum, after the existing space-related variants, add:

```rust
    UploadEmoji {
        space_id: String,
        name: String,
        data: Vec<u8>,
        image_type: String,
    },
    EmojiUploaded { emoji: EmojiInfo },
    DeleteEmoji { space_id: String, emoji_id: String },
    EmojiDeleted { space_id: String, emoji_id: String },
    ListEmojis { space_id: String },
    EmojiList { space_id: String, emojis: Vec<EmojiInfo> },
    FetchEmoji { emoji_id: String },
    EmojiData { emoji_id: String, data: Vec<u8>, image_type: String },
```

- [ ] **Step 3: Compile-check**

Run: `cargo check -p shared_types`
Expected: clean compile.

- [ ] **Step 4: Stage**

```bash
git add crates/shared_types/src/lib.rs
```

---

### Task 2.2: Add `space_emojis` table + CRUD methods

**Files:**
- Modify: `crates/signaling_server/src/persistence.rs`

- [ ] **Step 1: Add table to init() execute_batch**

Append to the same `execute_batch` SQL string in `init()`:

```sql
;
CREATE TABLE IF NOT EXISTS space_emojis (
    id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL,
    name TEXT NOT NULL,
    image_data BLOB NOT NULL,
    image_type TEXT NOT NULL,
    uploaded_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(space_id, name),
    FOREIGN KEY (space_id) REFERENCES spaces(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_space_emojis_space ON space_emojis(space_id)
```

- [ ] **Step 2: Add `EmojiRow` struct**

Near the other `*Row` structs at the top of the file:

```rust
#[derive(Debug, Clone)]
pub struct EmojiRow {
    pub id: String,
    pub space_id: String,
    pub name: String,
    pub image_data: Vec<u8>,
    pub image_type: String,
    pub uploaded_by: String,
    pub created_at: i64,
}
```

- [ ] **Step 3: Add 4 CRUD methods to `impl Database`**

```rust
    pub fn insert_emoji(&self, e: &EmojiRow) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO space_emojis
             (id, space_id, name, image_data, image_type, uploaded_by, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![e.id, e.space_id, e.name, e.image_data, e.image_type, e.uploaded_by, e.created_at],
        )
        .map_err(|err| format!("insert_emoji: {err}"))?;
        Ok(())
    }

    pub fn delete_emoji(&self, emoji_id: &str) -> Result<(), String> {
        let conn = self.lock_conn()?;
        conn.execute(
            "DELETE FROM space_emojis WHERE id = ?1",
            params![emoji_id],
        )
        .map_err(|err| format!("delete_emoji: {err}"))?;
        Ok(())
    }

    pub fn list_emojis_for_space(&self, space_id: &str) -> Result<Vec<EmojiRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, space_id, name, image_data, image_type, uploaded_by, created_at
                 FROM space_emojis WHERE space_id = ?1 ORDER BY created_at ASC",
            )
            .map_err(|e| format!("list_emojis prepare: {e}"))?;
        let rows = stmt
            .query_map(params![space_id], |row| {
                Ok(EmojiRow {
                    id: row.get(0)?,
                    space_id: row.get(1)?,
                    name: row.get(2)?,
                    image_data: row.get(3)?,
                    image_type: row.get(4)?,
                    uploaded_by: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })
            .map_err(|e| format!("list_emojis query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("list_emojis collect: {e}"))
    }

    pub fn get_emoji(&self, emoji_id: &str) -> Result<Option<EmojiRow>, String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, space_id, name, image_data, image_type, uploaded_by, created_at
                 FROM space_emojis WHERE id = ?1",
            )
            .map_err(|e| format!("get_emoji prepare: {e}"))?;
        let mut rows = stmt
            .query(params![emoji_id])
            .map_err(|e| format!("get_emoji query: {e}"))?;
        if let Some(row) = rows.next().map_err(|e| format!("get_emoji next: {e}"))? {
            Ok(Some(EmojiRow {
                id: row.get(0).map_err(|e| format!("get_emoji col0: {e}"))?,
                space_id: row.get(1).map_err(|e| format!("get_emoji col1: {e}"))?,
                name: row.get(2).map_err(|e| format!("get_emoji col2: {e}"))?,
                image_data: row.get(3).map_err(|e| format!("get_emoji col3: {e}"))?,
                image_type: row.get(4).map_err(|e| format!("get_emoji col4: {e}"))?,
                uploaded_by: row.get(5).map_err(|e| format!("get_emoji col5: {e}"))?,
                created_at: row.get(6).map_err(|e| format!("get_emoji col6: {e}"))?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn count_emojis_for_space(&self, space_id: &str) -> Result<u32, String> {
        let conn = self.lock_conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM space_emojis WHERE space_id = ?1",
                params![space_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("count_emojis: {e}"))?;
        Ok(count as u32)
    }
```

- [ ] **Step 4: Compile-check + stage**

Run: `cargo check -p signaling_server`
Expected: clean.

```bash
git add crates/signaling_server/src/persistence.rs
```

---

### Task 2.3: Create `handlers/emoji.rs` with 4 handlers

**Files:**
- Create: `crates/signaling_server/src/handlers/emoji.rs`
- Modify: `crates/signaling_server/src/handlers/mod.rs`

- [ ] **Step 1: Create the new file**

Write `crates/signaling_server/src/handlers/emoji.rs`:

```rust
use crate::persistence::EmojiRow;
use crate::{send_error, send_to};
use crate::State;
use shared_types::{EmojiInfo, SignalMessage};

const MAX_EMOJI_BYTES: usize = 256 * 1024;
const MAX_EMOJIS_PER_SPACE: u32 = 100;
const ALLOWED_TYPES: &[&str] = &["png", "webp"];

pub async fn handle_upload_emoji(
    state: &State,
    peer_id: &str,
    space_id: String,
    name: String,
    data: Vec<u8>,
    image_type: String,
) {
    // Validate size
    if data.len() > MAX_EMOJI_BYTES {
        send_error(state, peer_id, "Emoji too large (max 256 KB)").await;
        return;
    }
    // Validate type
    if !ALLOWED_TYPES.contains(&image_type.as_str()) {
        send_error(state, peer_id, "Emoji type must be png or webp").await;
        return;
    }
    // Validate name (alphanumeric + underscore, max 32 chars)
    if name.is_empty() || name.len() > 32
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        send_error(state, peer_id, "Invalid emoji name").await;
        return;
    }

    let (db, user_id, is_permitted) = {
        let s = state.read().await;
        let Some(peer) = s.peers.get(peer_id) else { return };
        let user_id = peer.user_id.lock().await.clone();
        let role = if let Some(space) = s.spaces.get(&space_id) {
            space.role_for(user_id.as_deref())
        } else {
            send_error(state, peer_id, "Space not found").await;
            return;
        };
        let permitted = matches!(
            role.as_str(),
            "owner" | "admin" | "mod"
        );
        (s.db.clone(), user_id, permitted)
    };

    if !is_permitted {
        send_error(state, peer_id, "Permission denied").await;
        return;
    }
    let Some(user_id) = user_id else {
        send_error(state, peer_id, "Not authenticated").await;
        return;
    };
    let Some(db) = db else {
        send_error(state, peer_id, "Database unavailable").await;
        return;
    };

    // Enforce per-space cap
    match db.count_emojis_for_space(&space_id) {
        Ok(c) if c >= MAX_EMOJIS_PER_SPACE => {
            send_error(state, peer_id, "Emoji limit reached (100 per space)").await;
            return;
        }
        Err(e) => {
            log::warn!("count_emojis_for_space failed: {e}");
            send_error(state, peer_id, "Internal error").await;
            return;
        }
        _ => {}
    }

    let id = format!("em_{}", uuid_like());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let row = EmojiRow {
        id: id.clone(),
        space_id: space_id.clone(),
        name: name.clone(),
        image_data: data,
        image_type: image_type.clone(),
        uploaded_by: user_id.clone(),
        created_at: now,
    };

    if let Err(e) = db.insert_emoji(&row) {
        send_error(state, peer_id, &format!("Insert failed: {e}")).await;
        return;
    }

    let info = EmojiInfo {
        id,
        space_id: space_id.clone(),
        name,
        image_type,
        uploaded_by: user_id,
        created_at: now as u64,
    };
    let msg = SignalMessage::EmojiUploaded { emoji: info };
    crate::broadcast_to_space(state, &space_id, "", &msg).await;
}

pub async fn handle_delete_emoji(state: &State, peer_id: &str, space_id: String, emoji_id: String) {
    let (db, is_permitted) = {
        let s = state.read().await;
        let Some(peer) = s.peers.get(peer_id) else { return };
        let user_id = peer.user_id.lock().await.clone();
        let role = if let Some(space) = s.spaces.get(&space_id) {
            space.role_for(user_id.as_deref())
        } else {
            return;
        };
        let permitted = matches!(role.as_str(), "owner" | "admin" | "mod");
        (s.db.clone(), permitted)
    };
    if !is_permitted {
        send_error(state, peer_id, "Permission denied").await;
        return;
    }
    let Some(db) = db else { return };
    if let Err(e) = db.delete_emoji(&emoji_id) {
        log::warn!("delete_emoji failed: {e}");
        return;
    }
    let msg = SignalMessage::EmojiDeleted { space_id: space_id.clone(), emoji_id };
    crate::broadcast_to_space(state, &space_id, "", &msg).await;
}

pub async fn handle_list_emojis(state: &State, peer_id: &str, space_id: String) {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let Some(db) = db else { return };
    let rows = match db.list_emojis_for_space(&space_id) {
        Ok(r) => r,
        Err(_) => return,
    };
    let emojis: Vec<EmojiInfo> = rows
        .into_iter()
        .map(|r| EmojiInfo {
            id: r.id,
            space_id: r.space_id,
            name: r.name,
            image_type: r.image_type,
            uploaded_by: r.uploaded_by,
            created_at: r.created_at as u64,
        })
        .collect();

    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id) {
        let _ = send_to(peer, &SignalMessage::EmojiList { space_id, emojis }).await;
    }
}

pub async fn handle_fetch_emoji(state: &State, peer_id: &str, emoji_id: String) {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let Some(db) = db else { return };
    let row = match db.get_emoji(&emoji_id) {
        Ok(Some(r)) => r,
        _ => return,
    };
    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id) {
        let _ = send_to(peer, &SignalMessage::EmojiData {
            emoji_id: row.id,
            data: row.image_data,
            image_type: row.image_type,
        }).await;
    }
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}
```

- [ ] **Step 2: Re-export from handlers/mod.rs**

Add to `crates/signaling_server/src/handlers/mod.rs`:

```rust
pub mod emoji;
```

- [ ] **Step 3: Compile-check**

Run: `cargo check -p signaling_server`
Expected: clean. If `space.role_for` doesn't exist with that exact signature, find the actual permission check pattern via:
`grep -n "role_for\|fn check_role\|role.*owner.*admin" /Users/jph/Voiceapp/workspace_template/crates/signaling_server/src/`
and adapt accordingly.

- [ ] **Step 4: Stage**

```bash
git add crates/signaling_server/src/handlers/
```

---

### Task 2.4: Route 4 new variants in main.rs

**Files:**
- Modify: `crates/signaling_server/src/main.rs`

- [ ] **Step 1: Add 4 match arms**

Inside the message dispatch `match` (after the `MarkChannelRead` arm from M1):

```rust
                SignalMessage::UploadEmoji { space_id, name, data, image_type } => {
                    handlers::emoji::handle_upload_emoji(&state, &peer_id, space_id, name, data, image_type).await;
                }
                SignalMessage::DeleteEmoji { space_id, emoji_id } => {
                    handlers::emoji::handle_delete_emoji(&state, &peer_id, space_id, emoji_id).await;
                }
                SignalMessage::ListEmojis { space_id } => {
                    handlers::emoji::handle_list_emojis(&state, &peer_id, space_id).await;
                }
                SignalMessage::FetchEmoji { emoji_id } => {
                    handlers::emoji::handle_fetch_emoji(&state, &peer_id, emoji_id).await;
                }
```

- [ ] **Step 2: Compile + stage**

Run: `cargo check -p signaling_server`

```bash
git add crates/signaling_server/src/main.rs
```

---

### Task 2.5: Add `emoji_cache_limit_mb` to AppConfig

**Files:**
- Modify: `crates/config_store/src/lib.rs`

- [ ] **Step 1: Add field**

In `AppConfig` struct:

```rust
    #[serde(default = "default_emoji_cache_mb")]
    pub emoji_cache_limit_mb: u32,
```

- [ ] **Step 2: Add default function and update `Default` impl**

```rust
fn default_emoji_cache_mb() -> u32 { 50 }
```

In `impl Default for AppConfig`:

```rust
            emoji_cache_limit_mb: 50,
```

- [ ] **Step 3: Compile + stage**

Run: `cargo check -p config_store`

```bash
git add crates/config_store/src/lib.rs
```

---

### Task 2.6: Client-side emoji cache + render

**Files:**
- Modify: `crates/ui_shell/src/lib.rs`
- Modify: `crates/app_desktop/src/signal_handler/space.rs`

- [ ] **Step 1: Add per-space emoji map to AppState**

Find `AppState` definition (likely in `shared_types` or `app_desktop`):
Run: `grep -rn "pub struct AppState" /Users/jph/Voiceapp/workspace_template/crates/`

Add field:
```rust
    pub space_emojis: HashMap<String, Vec<shared_types::EmojiInfo>>,
    pub emoji_data_cache: HashMap<String, Vec<u8>>,  // emoji_id -> bytes
```

- [ ] **Step 2: Handle EmojiList / EmojiUploaded / EmojiDeleted / EmojiData in signal_handler/space.rs**

```rust
SignalMessage::EmojiList { space_id, emojis } => {
    state.borrow_mut().space_emojis.insert(space_id.clone(), emojis.clone());
}
SignalMessage::EmojiUploaded { emoji } => {
    state.borrow_mut().space_emojis
        .entry(emoji.space_id.clone())
        .or_default()
        .push(emoji.clone());
}
SignalMessage::EmojiDeleted { space_id, emoji_id } => {
    if let Some(list) = state.borrow_mut().space_emojis.get_mut(space_id) {
        list.retain(|e| &e.id != emoji_id);
    }
}
SignalMessage::EmojiData { emoji_id, data, image_type: _ } => {
    state.borrow_mut().emoji_data_cache.insert(emoji_id.clone(), data.clone());
    // Trigger UI refresh of any open chat view
}
```

- [ ] **Step 3: Auto-fetch missing emoji bytes when render encounters :name:**

In `crates/ui_shell/src/lib.rs`, in `render_markdown()` or a new helper:
- Detect `:name:` pattern via regex `:([a-z0-9_]+):`
- If name matches an emoji in current space's `space_emojis` AND its bytes aren't in `emoji_data_cache`, queue a `FetchEmoji { emoji_id }` send
- Replace `:name:` with a placeholder like `[:name:]` in the rendered text (Slint can't currently render arbitrary inline images in markdown — this is a known v0.11 limitation; tracked for v0.12)

Acceptable v0.11 result: emojis show as text shortcodes; reaction picker can use them. Inline rendering is a v0.12 polish item.

- [ ] **Step 4: Compile + stage**

Run: `cargo check -p ui_shell -p app_desktop`

```bash
git add crates/ui_shell/ crates/app_desktop/
```

---

### Task 2.7: Integration test + manual verification + commit M2

**Files:**
- Modify: `crates/integration_tests/tests/server_tests.rs`

- [ ] **Step 1: Add test**

```rust
#[tokio::test]
async fn test_emoji_upload_and_list() {
    use shared_types::SignalMessage;
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    // Create account, then space
    alice.send_signal(&SignalMessage::CreateAccount {
        email: "alice2@test".to_string(),
        password: "password123".to_string(),
        display_name: "Alice".to_string(),
    }).await;
    let _ = alice.recv_signal().await;

    alice.send_signal(&SignalMessage::CreateSpace {
        name: "TestSpace".to_string(),
        user_name: "Alice".to_string(),
    }).await;
    let space_id = loop {
        match alice.recv_signal().await {
            SignalMessage::SpaceCreated { space, .. } => break space.id,
            _ => continue,
        }
    };

    // Upload an emoji (1x1 PNG)
    let png_1x1: Vec<u8> = vec![
        0x89,0x50,0x4E,0x47,0x0D,0x0A,0x1A,0x0A,
        0x00,0x00,0x00,0x0D,0x49,0x48,0x44,0x52,
        0x00,0x00,0x00,0x01,0x00,0x00,0x00,0x01,
        0x08,0x06,0x00,0x00,0x00,0x1F,0x15,0xC4,
        0x89,0x00,0x00,0x00,0x0D,0x49,0x44,0x41,
        0x54,0x78,0x9C,0x62,0x00,0x01,0x00,0x00,
        0x05,0x00,0x01,0x0D,0x0A,0x2D,0xB4,0x00,
        0x00,0x00,0x00,0x49,0x45,0x4E,0x44,0xAE,
        0x42,0x60,0x82,
    ];
    alice.send_signal(&SignalMessage::UploadEmoji {
        space_id: space_id.clone(),
        name: "blob".to_string(),
        data: png_1x1,
        image_type: "png".to_string(),
    }).await;

    // Expect EmojiUploaded broadcast
    let mut got_uploaded = false;
    for _ in 0..5 {
        match alice.recv_signal().await {
            SignalMessage::EmojiUploaded { emoji } => {
                assert_eq!(emoji.name, "blob");
                got_uploaded = true;
                break;
            }
            _ => continue,
        }
    }
    assert!(got_uploaded, "Expected EmojiUploaded");

    // List emojis
    alice.send_signal(&SignalMessage::ListEmojis { space_id: space_id.clone() }).await;
    let mut got_list = false;
    for _ in 0..5 {
        match alice.recv_signal().await {
            SignalMessage::EmojiList { emojis, .. } => {
                assert_eq!(emojis.len(), 1);
                assert_eq!(emojis[0].name, "blob");
                got_list = true;
                break;
            }
            _ => continue,
        }
    }
    assert!(got_list, "Expected EmojiList");
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p integration_tests test_emoji_upload_and_list -- --nocapture`
Expected: PASS.

Run: `cargo test --workspace`
Expected: all green.

- [ ] **Step 3: Commit M2**

```bash
git add crates/integration_tests/
git commit -m "$(cat <<'EOF'
feat(v0.11): custom emojis per space

- Add UploadEmoji/DeleteEmoji/ListEmojis/FetchEmoji protocol variants
- Persist emojis in space_emojis table (256 KB cap, 100/space)
- Server enforces role permission (owner/admin/mod) for upload
- Client caches emoji bytes lazily, configurable cache limit
- Inline rendering deferred to v0.12 (Slint markdown limitation)

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>
EOF
)"
```

(Defer actual commit until user approves.)

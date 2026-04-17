# v0.11 — Visual Communication — Design Spec

**Release theme:** See each other, react with personality, show what you're doing.

**Scope:** Screen share polish, custom emojis per space, animated stickers, rich presence, drag-and-drop uploads, unread separator.

**Explicit non-goals:** Full video calling (moves to v0.11.5 or v0.12), GIF picker (defer), animated avatar uploads (defer).

**Target delivery:** 6–8 weeks.

---

## 1. Screen share polish

### What exists
Screen share already works (see `docs/screen-share-v1.md`). Frames are relayed over WebSocket. Viewer shows raw incoming frames.

### What's missing
- No zoom / fit controls
- No pop-out window
- No pause-when-hidden (wastes CPU when viewer minimized)
- Shares the WebSocket path with audio — large screen frames can delay voice packets

### Design

**Transport split.** Move screen share frames to a separate UDP lane (parallel to voice lane). Use the same server-relay model but with a different frame-type tag. Keep WebSocket as fallback for browsers or NAT-hostile networks.

- New `net_control` module: `screen_transport.rs`
- Latest-only bounded queue (size 2) — drop older frames if viewer can't keep up
- New signal message: `ScreenShareUdpReady { token, port }`
- Server-side: new UDP handler branch checks token, fans out to room viewers

**Viewer controls.** Add overlay on screen share viewer:
- Fit to window / 100% / 200% buttons
- Pop-out to separate window (second Slint window instance)
- Auto-pause decode when viewer widget `visible` is false

**Pause-when-hidden.** Track viewer visibility state. When hidden:
- Server sends keep-alive control frames, not video
- Viewer client stops decoding, keeps WS alive
- On re-show, request keyframe

### Efficiency
- Viewer CPU when hidden: ~0% (currently decodes anyway)
- No impact on idle (screen share is only active during share)
- Separate transport means audio jitter unaffected by screen share spikes

### Testing
- Integration test: screen share in room while voice active, verify voice jitter < 5 ms regression
- Manual: start share, minimize window, confirm CPU drops to baseline
- Manual: pop-out, drag to second monitor, verify independent window

---

## 2. Custom emojis per space

### Design

**Storage.** New SQLite table:
```sql
CREATE TABLE IF NOT EXISTS space_emojis (
    id TEXT PRIMARY KEY,
    space_id TEXT NOT NULL,
    name TEXT NOT NULL,           -- :blobwave:
    image_data BLOB NOT NULL,     -- PNG/WebP, capped 256KB
    image_type TEXT NOT NULL,     -- "png" | "webp"
    uploaded_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(space_id, name)
);
```

**Hard limits:**
- 256 KB per emoji
- 100 emojis per space
- Server rejects uploads that exceed either cap
- Dimensions capped at 128×128; server re-encodes if larger

**Protocol (shared_types):**
```rust
UploadEmoji { space_id, name, data: Vec<u8>, image_type: String }
EmojiUploaded { emoji: EmojiInfo }
DeleteEmoji { space_id, emoji_id }
EmojiDeleted { space_id, emoji_id }
ListEmojis { space_id }
EmojiList { space_id, emojis: Vec<EmojiInfo> }

struct EmojiInfo {
    id: String,
    space_id: String,
    name: String,       // without colons
    url_hint: String,   // "emoji://<id>" — client fetches via FetchEmoji
}
```

**Rendering.** In `render_markdown()`, detect `:name:` pattern. Look up in per-space emoji cache. Replace with inline image.

**Client cache.** LRU disk cache in `config_store/emojis/<emoji_id>.png`. Capped at 50 MB total. Lazy-fetch on first render.

**Permissions.** Only owner/admin/mod can upload. Any member can use.

### UI
- Space settings → "Emoji" tab
- Upload button opens file picker (PNG/WebP only)
- Grid view of current emojis with delete button (if permitted)
- Emoji picker accessible from chat input (similar to reaction picker, extended)

### Efficiency
- Disk cache, not RAM — only decoded when rendered on-screen
- 50 MB disk cap (user-configurable up to 200 MB)
- LRU eviction on cache exceed
- No animated emojis in v0.11 (static only) — animation is a CPU trap

### Testing
- Unit: upload reject > 256 KB, reject > 100 per space, name uniqueness
- Integration: upload on server A, client B renders in chat
- Manual: upload 100 emojis, verify cache doesn't exceed cap

---

## 3. Animated stickers (one-shot)

### Rationale
Stickers are a "send this once" expression. Unlike emojis, they're not reused via `:name:` shortcuts. They're picked from a tray and sent as a message.

### Design

**Storage.** Bundled stickers ship with the app (no upload). A small curated pack (~30 stickers) in `crates/ui_shell/assets/stickers/`. Files are APNG or static PNG — no GIF, no Lottie.

**Limits:**
- One sticker per message (message is just the sticker)
- Animation plays once on arrival, then freezes on last frame
- If user scrolls away and back, plays again once

**Protocol:** Stickers ride existing `TextMessageData`:
```rust
sticker_id: Option<String>,  // "blob_wave" | "party_cat" | etc.
```

When set, `content` field is ignored and sticker is rendered instead.

**Rendering.**
- Chat view detects `sticker_id`, shows sticker at 128×128 max
- Uses Slint's `Image` with `source-clip-*` frame indexing for APNG-like behavior (if Slint supports; else render single frame)
- "Play once" logic: `animation-played` property per message, set after completion

### Efficiency
- Stickers bundled with binary — no network fetch, no disk cache
- Single-play-then-freeze prevents perpetual redraw (Discord's sticker loop is a known CPU drain)
- Deferred: user-uploadable stickers, animated stickers with >30 frames

### Out of scope
- User uploads (v0.13+)
- Sticker pack marketplace
- GIF integration (Tenor API = bloat + privacy concern)

### Testing
- Manual: send sticker, verify plays once, stops on last frame
- Manual: scroll away, scroll back, verify re-plays once
- Verify idle CPU with 20 stickers in chat ≤ 0.1%

---

## 4. Rich presence

### Design

**What users see:**
- Status line under username: "Playing Helldivers 2" or "Listening to Spotify - Track Name"
- Shown in member list, profile popover, DM list
- Opt-in toggle in settings (default off)

**Detection.** Platform-specific, using small native APIs only — no SDK integrations, no scraping:
- **macOS:** `NSWorkspace.frontmostApplication` (already imported transitively) — just read app name
- **Windows:** `GetForegroundWindow` + `GetWindowText` — basic, reads window title
- **Linux:** Read `_NET_ACTIVE_WINDOW` via x11 or skip (too fragmented)

**Batching.** Poll foreground app every 5 seconds (not 1s like Discord). Only send update if changed. This keeps rich presence idle-friendly.

**Privacy allowlist.** User configures which apps can broadcast presence. Blank allowlist = never broadcast. Prevents accidental "Playing CriminalActivity.exe".

**Protocol:**
```rust
SetRichPresence { app_name: Option<String>, details: Option<String> }
RichPresenceUpdated { user_id, app_name, details }
```

Server broadcasts to friends + space members only.

### Efficiency
- 5s poll interval on dedicated tokio task
- Only sends on change (debounced)
- Idle CPU: ~0% (one syscall every 5s)
- Default off — zero cost for users who don't enable

### UI
- Settings → Privacy → Rich Presence
- Toggle: enable/disable
- App allowlist: list of apps we've seen, checkboxes for broadcast
- Details line shown in member list via existing `member_presence` hook

### Testing
- Unit: presence debouncer (no send if unchanged)
- Manual: enable, switch apps, verify 5s update arrives
- Manual: disable, verify no broadcasts

---

## 5. Drag-and-drop file upload + image preview

### Current state
Attachments work (1 MB cap from v0.8). Users pick via button. Images render as download cards.

### Design

**Drag-and-drop.** Slint doesn't natively support drop events well. Use `winit`-level drop handling (already underneath Slint) via Slint's `DragEvent` API if available, else use a platform-specific hook in `app_desktop`.

Approach:
- Register native drop target on main window
- On drop, check file size vs cap, confirm with user if > 500 KB
- Auto-upload on confirm

**Image preview.** For image attachments (PNG/JPG/WebP):
- Decode client-side into a thumbnail (max 400×300, max 64 KB encoded)
- Render inline in chat, not as download card
- Click to open full size

**Thumbnail storage.** Server stores both full image (existing) and thumbnail. New column:
```sql
ALTER TABLE attachments ADD COLUMN thumbnail BLOB;
```

`ensure_column` idempotent migration.

### Efficiency
- Thumbnail decode happens once server-side on upload
- Client renders 64 KB thumbnails, not 1 MB originals
- Chat scrollback with 100 images: ~6 MB RAM vs ~100 MB without thumbnails

### Testing
- Unit: thumbnail cap enforcement
- Integration: upload 1 MB image, verify thumbnail generated
- Manual: drag image from Finder → Voxlink, verify upload flow

---

## 6. Unread jump-to-first-new separator

### Current state
Unread counts work. No visual separator in scrollback.

### Design

**Track `last_read_message_id` per channel** (already half-exists in notification state).

**In chat view:**
- On channel open, record last read message
- Render horizontal "NEW" separator above first message newer than last-read
- Separator auto-scrolls into view on open
- Marker clears when user scrolls past last message (set `last_read` = latest)

**Storage:** Server-side table:
```sql
CREATE TABLE IF NOT EXISTS channel_read_marks (
    user_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    last_read_message_id TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, channel_id)
);
```

**Protocol:**
```rust
MarkChannelRead { channel_id, message_id }
```

### Efficiency
- No polling — server pushes updated mark on other-device sync
- Single row per user-channel pair
- Marker UI is a single Rectangle with a text label — cheap

### Testing
- Integration: receive message while channel unfocused, open channel, verify separator above new messages
- Unit: mark-read updates row (not insert-duplicate)

---

## Protocol additions summary

New `SignalMessage` variants (total 10):
- Screen share: `ScreenShareUdpReady`
- Emojis: `UploadEmoji`, `EmojiUploaded`, `DeleteEmoji`, `EmojiDeleted`, `ListEmojis`, `EmojiList`
- Presence: `SetRichPresence`, `RichPresenceUpdated`
- Reads: `MarkChannelRead`

New DB tables (2): `space_emojis`, `channel_read_marks`
New DB columns (1): `attachments.thumbnail`
New config fields (3): `rich_presence_enabled: bool`, `rich_presence_allowlist: Vec<String>`, `emoji_cache_limit_mb: u32`

## Test strategy

- **Unit tests:** ~25 new (emoji caps, presence debouncer, thumbnail caps, read-mark updates, transport splits)
- **Integration tests:** ~8 new (emoji upload/render round-trip, screen share on UDP, read-mark sync across clients)
- **Manual verification:** screen share pause, emoji picker, sticker play-once, rich presence allowlist, drag-and-drop, unread separator
- **Efficiency verification:** idle CPU/RAM check, screen-share-during-voice jitter measurement

**Target:** ~350 existing + 33 new = ~383 tests passing, zero warnings.

## Dependencies

**Minimal additions.** Reuses:
- `rusqlite` for new tables/columns
- Slint for all UI
- Platform APIs directly (`osascript` on macOS, `powershell` on Windows) for rich presence — no binding crate needed

**One justified exception:**
- `rfd` (~200 KB, no transitive bloat) — native file open dialog for attachments. Slint 1.15 has no native dialog API and we prefer this over reimplementing per-platform dialogs via FFI.

## Rollout

1. Build & test locally (all features behind release-channel check if needed)
2. Tag `v0.11.0-rc1`, deploy server
3. Manual soak: 1 hour with emojis, screen share, presence active
4. Verify efficiency budgets
5. Tag `v0.11.0`, publish installers, update README

## Risks

1. **Slint drag-drop API may not be complete.** Mitigation: fall back to "paste image from clipboard" if drop is not workable.
2. **Native rich-presence APIs vary across platforms.** Mitigation: ship macOS + Windows first; Linux as "detect-only-if-xorg".
3. **Emoji uploads could bloat server storage.** Mitigation: 256 KB × 100 per space × N spaces; add per-space quota check before accepting.
4. **Screen-share UDP lane adds protocol complexity.** Mitigation: WebSocket fallback stays as default; UDP is upgrade only.

## Success criteria

- All 6 features shipped and testable
- Zero warnings, zero failing tests
- Idle CPU ≤ 0.5%, idle RAM ≤ 120 MB
- Voice quality during active screen share: ≤ 5 ms jitter regression from v0.10.4 baseline
- Binary size growth ≤ 5 MB

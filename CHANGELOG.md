# Changelog

## v0.9.0 — Performance & Robustness

### New
- **Voice pipeline integration tests** — Full-duplex audio tests simulating two clients exchanging Opus-encoded audio over WebSocket and UDP, with WAV recording for manual verification.
- **Password masking** — Room password inputs now use `InputType.password` for proper bullet masking.

### Improved
- **Server audio relay optimization** — Merged whisper filtering into a single state read lock (eliminated redundant lock acquisition per audio frame). Replaced tokio Mutex with `std::sync::RwLock` for `udp_addr` and `whisper_targets` fields, enabling lock-free reads on the 50fps hot path.
- **Tokio runtime** — Removed hardcoded 2-thread limit; server now uses all available CPU cores.
- **Resource cleanup** — Periodic cleanup task now expires stale `auth_attempts`, `join_failures`, `slow_mode_timestamps`, and orphaned UDP session tokens. Prevents unbounded memory growth on long-running servers.
- **Config thread safety** — Added `CONFIG_LOCK` mutex to serialize all config load-modify-save cycles, preventing data races across background save threads.
- **Whisper cleanup** — Server clears whisper targets on peer disconnect.
- **Auto-update URL** — Fixed GitHub releases API URL for update checks.
- **Dead code removal** — Removed unused `AudioEngine::adapt_bitrate()` (already implemented in tick loop).

### Stats
- 351 tests passing, zero warnings
- 11 workspace crates

## v0.8.0 — Social Features & Account System

### New — 22 Features
- **Email account system** — Create account, login, logout, change password with salted SHA-256 hashing. Token rotation on login, persistent email in config.
- **Join/leave notification sounds** — Configurable rising/descending two-note chimes when peers enter or leave a room.
- **Channel categories** — Organize channels under bold section headers with `SetChannelCategory` support.
- **Unread indicators** — Badge counts on channels (mention count) and dot badges on space cards in home view.
- **Status presets** — Online, Idle, DND, Invisible. Invisible hides from member/friend lists. DND suppresses notifications.
- **Idle auto-status** — Automatically sets status to Idle after 5 minutes of keyboard inactivity, restores on input.
- **@Mentions with notifications** — Extract `@username` from messages, send `MentionNotification` to mentioned users with sound.
- **Block/unblock users** — Server-side `user_blocks` table, client-side message filtering, block/unblock SignalMessage variants.
- **Ban management UI** — `ListBans`, `UnbanMember`, ban list view in space settings.
- **Group DMs** — Multi-user direct message conversations with `group_conversations`, `group_members`, `group_messages` tables.
- **Invite expiration & max uses** — `invite_expires_at`, `invite_max_uses`, `invite_uses` columns with server-side validation.
- **Per-channel notification settings** — Override notifications per channel: all / mentions only / none.
- **Quick switcher (Ctrl+K)** — Fuzzy search overlay for channels and DMs with keyboard navigation.
- **User avatars** — Color-coded circles with initials, replacing inline rendering across all views.
- **Message threads** — Reply chains via `GetThread` / `ThreadMessages`, leveraging existing `reply_to_message_id`.
- **Volume ducking** — Auto-lower non-speaking peers when someone is talking. Configurable amount and threshold.
- **File attachments** — `attachments` table with 1MB cap, attachment metadata on `TextMessageData`.
- **Soundboard** — `SoundboardClip` with pre-decoded WAV samples, mixed into capture stream.
- **Server nicknames** — `space_nicknames` table, `SetNickname` / `NicknameChanged` protocol.
- **Message forwarding** — `ForwardMessage` copies messages between channels with "Forwarded from" header.
- **Spoiler tags** — `||text||` syntax detected in `render_markdown()`.
- **Compact chat density** — Toggle for reduced padding/font in chat messages.

### Improved
- **Performance** — Idle detection eliminated 40 heap allocs/sec; volume ducking uses single-pass atomic caching; ring buffer `peek_energy` uses contiguous fast path.
- **Test coverage** — 338 tests across all crates (up from 316).
- **Installer** — `build-portable.ps1` now reads version dynamically from Cargo.toml. `voxlink.iss` bumped to 0.8.0.

### New DB Tables
- `user_blocks`, `group_conversations`, `group_members`, `group_messages`, `attachments`, `space_nicknames`

### New Config Fields
- `join_leave_sounds`, `show_spoilers`, `compact_chat`, `blocked_users`, `status_preset`, `idle_timeout_mins`, `channel_notification_overrides`, `ducking_amount`, `ducking_threshold`, `soundboard_clips`, `account_email`

### New Dependencies
- `sha2 0.10` — Password hashing for account system

## v0.7.0 — Reliability & Quality

### New
- **Adaptive bitrate** — Audio encoder automatically adjusts bitrate based on packet loss (60–100% of target).
- **Server metrics** — Prometheus-format `/metrics` endpoint with UDP frame counters, room/space stats, and uptime tracking.
- **Server module refactor** — Extracted type definitions into `types.rs` for maintainability.

### Improved
- **Test coverage** — 316 tests across all crates (up from ~235). Added 30+ audio DSP tests, 9 network edge-case tests, and fixed flaky integration tests.
- **UDP safety** — Server UDP token parsing uses graceful error handling instead of unwrap (prevents panic on malformed packets).
- **Audio pipeline docs** — README updated to reflect full DSP chain including neural noise suppression and adaptive bitrate.

### Fixed
- **Integration test build** — Removed invalid re-export, added missing dependency.
- **Slint UI** — Removed invalid `vertical-alignment` on Rectangle elements.
- **Config persistence** — `saved_servers` field now properly preserved on settings save.
- **Test reliability** — Fixed message ordering issues in space join/text message tests (FriendSnapshot interleaving).

## v0.6.0 — Audio Quality & Transport

### New
- **UDP audio transport** — Lower-latency audio delivery with automatic WebSocket fallback. Server relays UDP alongside WebSocket; clients negotiate via signaling.
- **UDP keepalive** — Periodic 15s keepalive packets prevent NAT mapping expiry for long sessions.
- **Transport indicator** — Room view and perf panel show whether audio is flowing over UDP or WebSocket, with color-coded ping badge.
- **Noise gate auto-calibration** — Measures ambient noise during first 2 seconds of capture and sets the gate threshold automatically.
- **Per-peer volume persistence** — Volume adjustments are remembered by peer name across sessions and restored on rejoin.
- **Perf panel enhancements** — Transport type, ping latency, jitter buffer depth, frame loss rate, encode bitrate, and peer count all visible in the system overview.
- **Startup timing** — Logs startup duration in milliseconds for profiling.

### Improved
- **Audio metrics** — `PerfSnapshot` extended with `udp_active`, `ping_ms`, jitter buffer, frame loss, bitrate, and decode peer count.
- **Config store** — Added `peer_volumes` field for persistent per-peer volume adjustments.

### Fixed
- **Perf collector wiring** — `ping_ms` and `udp_active` atomics now correctly updated from the tick loop.

## v0.5.3 — Spaces, Chat, and Friends

Previous release with spaces architecture, text chat, friend system, direct messages, moderation tools, and CI/CD pipeline.

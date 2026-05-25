# Voxlink Roadmap — March 2026

## Current posture (v0.8.0)

Voxlink has a strong feature set covering voice, text, social, and moderation:

**Done:**
- Voice: rooms, spaces, channels, PTT, mute/deafen, per-peer volume, adaptive bitrate, UDP transport, neural noise suppression, volume ducking, soundboard, priority speaker, whisper
- Social: friends, DMs, group DMs, @mentions, status presets (online/idle/DND/invisible), idle auto-status, block/unblock, server nicknames
- Chat: send/edit/delete, reactions, pins, search, threads, forwarding, attachments (1MB), spoiler tags, compact mode, markdown
- Account system: email/password registration, login, logout, change password (salted SHA-256)
- Moderation: kick/ban/timeout/server-mute, roles (owner/admin/mod/member), ban management UI, audit log
- Organization: channel categories, quick switcher (Ctrl+K), unread indicators, per-channel notification settings, invite expiration/max-uses
- Desktop: screen share, global hotkeys, 7 theme presets, auto-update, system tray, perf panel
- 338 tests, zero warnings, deployed on Oracle Cloud

**The main gaps are now:**
1. Security hardening (TLS, stronger auth, abuse resistance)
2. Network architecture improvements (separate media lanes, protocol versioning)
3. Quality-of-life polish and edge case handling
4. Safe media sharing ("Listen Together")

## Phase 0 — Security & Operational Hardening

Do this before major new user-facing scope.

### 0.1 Transport security
- Make TLS the default for any non-localhost bind
- Allow plain `ws://` only for explicit local development
- Add a startup failure if the server binds publicly without certs
- Certificate rotation and deployment docs for renewal

### 0.2 Auth hardening
- ~~Replace token generator with cryptographic randomness~~ (DONE — uses OsRng)
- ~~Token rotation on login~~ (DONE)
- ~~Token revoke on logout~~ (DONE)
- ~~Email/password accounts~~ (DONE)
- Upgrade from SHA-256 to argon2 or bcrypt for password hashing (sha2 is fast but not ideal for passwords)
- Add rate limiting specifically for login/registration attempts
- Add "forgot password" flow (requires email sending capability)
- Add revoke-all-sessions endpoint

### 0.3 Abuse resistance
- Split rate limits by message type (auth, chat, DM, room control, audio)
- Add caps for: watched friend count, outbound DM burst, channel create/delete churn
- Add temporary bans/cooldowns for repeated abuse from same IP or token
- Add message content length limits and spam detection

### 0.4 Permissions hardening
- ~~Server-side roles (owner/admin/mod/member)~~ (DONE)
- ~~Audit log~~ (DONE)
- Move ALL destructive controls behind server-checked permissions (some may still be UI-only)
- Channel permission matrix (per-channel role overrides)
- Locked voice/text channels

### 0.5 Ops and observability
- ~~Prometheus-style metrics endpoint~~ (DONE)
- Add structured logging (JSON format option)
- Track: DB operation latency, rate-limit hits, auth failures by IP
- Add panic/crash capture for the desktop app to local logs

## Phase 1 — Quality of Life & Polish

### 1.1 Account UX
- Password masking in login UI (requires VxInput input-type support or custom masked input)
- "Remember me" / auto-login flow
- Account settings page (change email, change display name, delete account)
- Email verification (requires SMTP integration)

### 1.2 Chat completeness
- File/image preview in chat (currently just download card)
- Drag-and-drop file upload
- Unread jump-to-first-new separator
- Message search across all channels (currently per-channel)
- Code block syntax highlighting in markdown

### 1.3 Voice quality UX
- Call-side output meter per peer
- Packet-loss and jitter trend graph in perf panel
- Device fallback explanation when hardware disappears
- Better screen-share viewer controls (zoom, fit, pop-out, pause when hidden)

### 1.4 Desktop polish
- System tray context menu (mute/deafen/disconnect)
- Minimize to tray on close (currently just a setting flag)
- Keyboard shortcuts help overlay
- Onboarding diagnostics (mic blocked, wrong device, network unstable)

## Phase 2 — Network & Media Architecture

### 2.1 Separate media lanes
- Move screen share off the WebSocket path
- Keep voice isolated from screen-share spikes
- Bounded latest-only queues for screen data

### 2.2 Voice transport upgrade
- Keep WebSocket relay for simple self-hosting
- Add optional QUIC or WebRTC path for larger rooms
- Gate behind capabilities negotiation so old clients still work

### 2.3 Protocol versioning
- Add protocol version and feature-capability negotiation at connect time
- Refuse incompatible clients cleanly
- Make migrations explicit for screen share transport, attachment support, permission model

### 2.4 Scale testing
- 30-minute and 2-hour soak suites
- Chaos tests (packet drop, reconnect storms, DB locked, rapid join/leave)
- Windows/macOS resource profiling for idle, active chat, voice call, screen share

## Phase 3 — Safe Media Sharing

### Listen Together (recommended first)
- User opens media locally on their machine
- Voxlink captures local app/system audio with explicit consent
- Audio streamed into room as a "media send" path
- Separate media volume slider for listeners
- No server-side scraping, downloading, or ad skipping

### Media Queue (optional, after Listen Together)
- Room-scoped player accepting uploaded local audio files
- Licensed/public web radio or HLS audio sources only
- play/pause/skip/stop/queue/volume/now-playing controls
- One bot per room, bounded queue, max duration/file size

## Recommended implementation order

1. **Auth hardening** — argon2, login rate limits, revoke-all-sessions
2. **TLS by default** — required for public binds
3. **Permissions hardening** — channel permission matrix, locked channels
4. **Account UX** — password masking, auto-login, account settings
5. **Chat polish** — file preview, drag-drop, unread separator
6. **Separate screen-share transport** from voice
7. **Long soak and chaos testing**
8. **Listen Together** local media sharing

## Concrete next milestone

**v0.9.0 — Security & Auth Hardening**

Scope:
- Upgrade password hashing to argon2
- Login/registration rate limiting (per IP)
- TLS required for public binds (plain WS only for localhost/dev)
- Channel permission matrix
- Password masking in login UI
- Auto-login flow (skip login view if valid token exists)
- Revoke-all-sessions endpoint

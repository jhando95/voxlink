# Voxlink Roadmap - March 2026

## Current posture

Voxlink already has a good small-group voice foundation:

- Bounded client media queues keep audio and screen traffic from growing without limit.
- The server already has connection and signaling rate limits.
- There is a real integration and live stress test surface.
- Spaces, channels, direct messages, screen share, moderation, and friend presence exist.

The main gaps are not "basic Discord parity" anymore. The biggest needs are:

1. security hardening
2. network and media isolation
3. permissions and admin tooling
4. richer collaboration features
5. a safe media-sharing feature that does not depend on scraping or ad-skipping YouTube

## What the code says now

- Voice and screen media are still carried over the WebSocket client path, with bounded queues on the desktop side.
- The server is still optimized for small rooms today: `MAX_ROOM_PEERS` is `10`.
- TLS is optional instead of being required by default.
- Auth is lightweight token restore, but token generation is not cryptographically strong enough for a public service.
- There is already a live stress harness and multi-client integration coverage, so the next step is longer soak and chaos coverage, not starting from zero.

## Phase 0 - Security and operational hardening

Do this before major new user-facing scope.

### 0.1 Transport security

- Make TLS the default for any non-localhost bind.
- Allow plain `ws://` only for explicit local development.
- Add a startup failure if the server binds publicly without certs.
- Add certificate rotation and basic deployment docs for renewal.

### 0.2 Identity and session security

- Replace the current token generator with `rand::rngs::OsRng` or equivalent cryptographic randomness.
- Add token rotation, revoke-on-logout, and revoke-all-sessions.
- Store a session creation timestamp and last-seen timestamp.
- Add a separate stable `user_id` allocator that is not derived from transient peer IDs.

### 0.3 Abuse resistance

- Split rate limits by message type:
  - auth
  - chat
  - DM
  - room control
  - screen frames
  - audio frames
- Add caps for:
  - watched friend count
  - outbound DM burst
  - channel create/delete churn
  - room/space create churn
- Add explicit temporary bans or cooldowns for repeated abuse from the same IP or token.

### 0.4 Permissions and auditability

- Add server-side roles:
  - owner
  - admin
  - moderator
  - member
- Move destructive controls behind server-checked permissions, not UI-only visibility.
- Add an audit log for:
  - space delete
  - channel create/delete
  - kick
  - ban
  - role changes
  - message pin/delete/edit moderation events

### 0.5 Ops and observability

- Add structured logs.
- Add Prometheus-style metrics or an internal metrics endpoint.
- Track:
  - active connections
  - active rooms
  - active spaces
  - reconnect rate
  - average ping
  - dropped audio frames
  - screen share active count
  - DB operation latency
  - rate-limit hits
- Add panic/crash capture for the desktop app, at least to local logs.

## Phase 1 - Product gaps that matter more than feature bloat

### 1.1 Permissions and moderation UX

- Role assignment UI.
- Channel permission matrix.
- Locked text channels.
- Locked voice channels.
- Ban list management.
- Audit log viewer.

### 1.2 Chat completeness

- Message search.
- File and image attachments.
- Drag and drop paste.
- Pinned messages panel.
- Unread jump-to-first-new.
- Mention notifications with user-level toggles.

### 1.3 Social and session UX

- Group DMs or private multi-user calls.
- Better invite deep links.
- Presence privacy toggles.
- Richer onboarding diagnostics:
  - mic blocked
  - wrong output device
  - network unstable
  - TLS/server trust issue

### 1.4 Call quality UX

- Optional call-side output meter.
- Packet-loss and jitter trend view.
- Device fallback explanation when hardware disappears.
- Better screen-share viewer controls:
  - zoom
  - fit
  - pop-out
  - pause rendering when hidden

## Phase 2 - Networking and media architecture

This is the main technical step if Voxlink grows beyond small-group rooms.

### 2.1 Separate media lanes

- Move screen share off the current shared WebSocket media lane.
- Keep voice isolated from screen-share spikes.
- Keep bounded latest-only queues for screen data.

### 2.2 Voice transport upgrade path

- Keep the current WebSocket relay for simple self-hosting and small groups.
- Add an optional higher-performance voice path for larger rooms:
  - QUIC or UDP-based relay
  - or a WebRTC path if NAT traversal and browser support ever matter
- Gate this behind capabilities negotiation so old clients still work.

### 2.3 Protocol versioning

- Add protocol version and feature-capability negotiation at connect/auth time.
- Refuse incompatible clients cleanly.
- Make migrations explicit for:
  - screen share transport
  - attachment support
  - permission model
  - future media features

### 2.4 Reliability and scale testing

- Add 30-minute and 2-hour soak suites.
- Add chaos tests:
  - packet drop
  - reconnect storms
  - DB locked or slow
  - screen-share start/stop churn
  - rapid join/leave across several rooms
- Add Windows resource profiling for:
  - idle
  - active chat
  - voice call
  - screen share
  - widget open

## Phase 3 - Safe media and "music bot" direction

### What not to build

Do not build a server-side "YouTube without ads" bot.

Reasons:

- It creates legal and platform-policy risk.
- It encourages a scraper/downloader design that will become a maintenance problem.
- It adds abuse surface and likely increases compute, bandwidth, and moderation burden.

### What to build instead

Build one of these two features:

#### Option A - Media queue bot

A room-scoped media player that only accepts:

- uploaded local audio files
- explicitly supported direct audio stream URLs
- licensed/public web radio or HLS audio sources

MVP controls:

- play
- pause
- skip
- stop
- queue
- volume
- now playing

Guardrails:

- one bot per room
- bounded queue length
- max media duration or file size
- supported MIME types only
- no arbitrary website extraction

#### Option B - Listen Together

This is the better fit for Voxlink.

- A user opens media locally on their own machine.
- Voxlink captures that local app/system audio with explicit consent.
- The audio is streamed into the room as a dedicated "media send" path.
- No server-side scraping, downloading, or ad skipping.

This can later pair with:

- browser-source sharing
- local system audio share
- a "listen along" mode with a separate media volume slider

### Recommendation

Do `Listen Together` before a bot.

Why:

- lower legal and operational risk
- better fit with the current desktop-native architecture
- no remote media ingestion pipeline required
- avoids becoming a content-hosting or scraping service

## Quality bar changes

Before large new features land, raise the engineering floor:

- Clippy clean on core crates.
- Feature flags for experimental media features.
- Every new protocol feature needs:
  - shared type round-trip tests
  - server integration coverage
  - reconnect coverage
  - persistence coverage if applicable
- Add one repeatable Windows profiling script and one Linux soak script.

## Recommended implementation order

1. TLS-by-default, cryptographic tokens, session revoke, metrics endpoint.
2. Roles, permissions, audit log, and channel permission matrix.
3. Attachments, search, and moderation UX polish.
4. Separate screen-share transport from voice.
5. Long soak and chaos testing.
6. `Listen Together` local media sharing.
7. Optional room media queue for local files and licensed streams.

## Concrete next milestone

If work starts now, the best next milestone is:

`Security hardening + permissions foundation`

Scope:

- TLS required for public binds
- secure token generation and rotation
- owner/admin/mod/member roles
- server-side permission checks for all destructive actions
- audit log persistence and UI
- metrics endpoint

That gives Voxlink a safer base for every feature that comes after it, including media features.

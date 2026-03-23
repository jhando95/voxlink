# Voxlink — Project Reference

## Repositories & Access

- **GitHub**: https://github.com/jhando95/voxlink.git
- **Branch**: `main`
- **Workspace root**: `/Users/jph/Voiceapp/workspace_template/`

## Oracle Cloud Server

- **IP**: 129.158.231.26
- **WebSocket**: `ws://129.158.231.26:9090` (signaling + audio fallback)
- **UDP Audio**: `129.158.231.26:9091` (relay transport)
- **SSH**: `ssh -i ~/.ssh/oracle_key ubuntu@129.158.231.26`
- **OS**: Ubuntu x86_64, Oracle Cloud free-tier (1GB RAM + 2GB swap)
- **Binary**: `/opt/voxlink/signaling_server`
- **Database**: `/var/lib/voxlink/voxlink.db` (SQLite, WAL mode)
- **Service**: systemd `voxlink.service` (runs as nobody:nogroup)
- **Deploy**: `./deploy/push-to-server.sh ubuntu@129.158.231.26`

### Server commands
```bash
sudo systemctl status voxlink      # Check status
sudo journalctl -u voxlink -f      # Live logs
sudo systemctl restart voxlink     # Restart
sudo systemctl stop voxlink        # Stop
```

## Tech Stack

- **Language**: Rust 1.94.0
- **UI**: Slint 1.15 (native desktop)
- **Audio**: cpal 0.15, audiopus (Opus codec), 48kHz mono, 20ms frames
- **Networking**: tokio 1.50, tungstenite (WebSocket), tokio UDP
- **Auth**: sha2 (salted SHA-256 password hashing)
- **Version**: 0.8.0
- **Tests**: 338 (unit + integration + stress)
- **Warnings**: 0

## Workspace Crates

| Crate | Purpose |
|-------|---------|
| `app_desktop` | Main binary — UI callbacks, signal handling, tick loop, screen share |
| `ui_shell` | Slint UI components, data conversion, member widget |
| `audio_core` | Capture/playback, Opus encode/decode, DSP chain, jitter buffer, soundboard |
| `voice_engine` | Mute/deafen/PTT state machine |
| `net_control` | WebSocket + UDP client, reconnect logic |
| `media_transport` | Audio frame routing between network and audio engine |
| `perf_metrics` | CPU/memory/audio metrics collection |
| `config_store` | JSON config persistence (devices, keybinds, volumes, notes, account) |
| `shared_types` | SignalMessage protocol (~70 variants), AppState, all shared data types |
| `signaling_server` | WebSocket + UDP relay server, SQLite persistence, room/space/auth management |
| `integration_tests` | Server integration tests + live stress tests |

## Architecture

- **Audio path**: cpal capture → HPF → noise gate → AGC → de-esser → neural denoise → Opus encode (adaptive bitrate) → UDP/WebSocket → Opus decode (PLC/FEC) → jitter buffer → per-peer AGC → volume ducking → soft clip → cpal playback
- **State**: `Rc<RefCell>` on UI thread, `Arc<TokioMutex>` for async, `AtomicBool/U32/U64` for cross-thread flags
- **Networking**: WebSocket signaling, server-relayed UDP audio with 8-byte session tokens
- **UI loop**: Slint timer at 40Hz (25ms tick) for polling events
- **Async runtime**: Tokio (2 threads) for networking
- **Auth**: Email/password accounts with salted SHA-256, token-based sessions (64-char hex, 90-day expiry, rotation on login)
- **Persistence**: SQLite WAL mode — users, spaces, channels, messages, bans, blocks, nicknames, group DMs, attachments

## Key Files

| File | What's in it |
|------|-------------|
| `crates/app_desktop/src/main.rs` | App entry, window setup, runtime init |
| `crates/app_desktop/src/callbacks/` | All UI callback wiring (space, room, channel, chat, controls, auth) |
| `crates/app_desktop/src/signal_handler/mod.rs` | Signal dispatch from server messages |
| `crates/signaling_server/src/main.rs` | Server entry, WebSocket handler, audio relay, signal routing |
| `crates/signaling_server/src/handlers/` | Auth, channel, room, space, chat, friends, moderation, presence handlers |
| `crates/signaling_server/src/persistence.rs` | SQLite schema, migrations, and queries |
| `crates/shared_types/src/lib.rs` | SignalMessage enum (~70 variants), all shared structs |
| `crates/ui_shell/ui/main.slint` | Root UI component, all properties and callbacks |
| `crates/ui_shell/ui/theme.slint` | Data structs, theme colors, design tokens |
| `crates/ui_shell/ui/views/` | Home, Room, Space, Chat, Settings, System views |
| `crates/audio_core/src/lib.rs` | AudioEngine — capture, playback, peer buffers, ducking |
| `crates/audio_core/src/codec.rs` | DSP: noise gate, AGC, HPF, comfort noise, soft clip |
| `crates/audio_core/src/buffers.rs` | SPSC ring buffers, jitter buffer, capture ring, peek_energy |

## Branding

- **App name**: Voxlink ("Voice without limits")
- **UI prefix**: Vx (VxTheme, VxButton, VxCard, VxInput, VxSlider)
- **Accent**: violet-indigo (#7c5bf5 dark / #6841ea light)
- **Config path**: `com.voxlink.Voxlink`
- **Log file**: `voxlink.log`

## Installers

| Platform | Script | Output |
|----------|--------|--------|
| Windows | `installer/voxlink.iss` (Inno Setup) | `Voxlink-Setup-0.8.0.exe` |
| Windows | `installer/build-portable.ps1` | `Voxlink-0.8.0-portable.zip` |
| macOS | `installer/build-macos.sh` | `Voxlink-0.8.0-macos.dmg` |
| Linux | `installer/build-linux.sh` | `.deb` + `.tar.gz` |

## Build & Test

```bash
cargo check                                    # Compile check (zero warnings required)
cargo test --workspace --exclude integration_tests  # Unit tests only
cargo test -p integration_tests --test server_tests # Integration tests (needs live server)
cargo test -p integration_tests --test live_stress_test # Stress tests (needs live server)
cargo build --release -p app_desktop           # Build client
cargo build --release -p signaling_server      # Build server
```

## Feature Summary (v0.8.0)

**Voice & Audio**
- Create/join rooms by code, open mic + push-to-talk, mute/deafen, per-peer volume
- UDP audio transport with WebSocket fallback, adaptive bitrate
- Volume ducking, soundboard, priority speaker, whisper/private voice
- Neural noise suppression, auto-calibrating noise gate, AGC, de-esser
- Join/leave notification sounds

**Spaces & Channels**
- Spaces with invite codes (expiration + max uses)
- Voice channels + text channels, channel categories
- Channel user limits, slow mode, status text
- Per-channel notification settings
- Quick switcher (Ctrl+K)

**Social**
- Email account system (create, login, logout, change password)
- Friends, presence, DMs, group DMs, friend requests
- Status presets (Online/Idle/DND/Invisible), idle auto-status
- @Mentions with notifications, unread indicators
- Block/unblock users, server nicknames

**Chat**
- Send/edit/delete/react/pin/search, typing indicators, markdown
- Message threads (reply chains), message forwarding
- Spoiler tags, compact chat density
- File attachments (1MB cap)

**Moderation**
- Kick/ban/timeout/server-mute, role management (Owner/Admin/Mod/Member)
- Ban management UI with unban
- Audit log, user notes (local-only)

**Desktop**
- Screen share with adaptive quality
- Global hotkeys (PTT, mute, deafen)
- Desktop notifications, system tray
- 7 theme presets, dark/light mode
- Auto-reconnect, device hotplug recovery
- Performance panel with audio metrics
- Auto-update checker

## DB Tables

| Table | Purpose |
|-------|---------|
| `users` | User accounts (id, token, display_name, email, password_hash, timestamps) |
| `spaces` | Spaces (id, name, invite_code, owner, invite settings) |
| `channels` | Channels within spaces (voice or text, categories, limits) |
| `messages` | Chat messages (content, sender, reactions, pins, replies, forwarding) |
| `bans` | Space bans |
| `space_roles` | Role assignments (owner/admin/mod/member) |
| `audit_log` | Moderation actions |
| `friend_requests` | Pending friend requests |
| `friendships` | Confirmed friendships |
| `direct_messages` | 1:1 DM history |
| `user_blocks` | Block relationships |
| `group_conversations` | Group DM metadata |
| `group_members` | Group DM membership |
| `group_messages` | Group DM message history |
| `attachments` | File attachment blobs (1MB cap) |
| `space_nicknames` | Per-space display name overrides |

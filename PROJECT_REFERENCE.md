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
- **Version**: 0.7.0
- **Tests**: 243 (135 unit + 98 integration + 10 live stress)
- **Warnings**: 0

## Workspace Crates

| Crate | Purpose |
|-------|---------|
| `app_desktop` | Main binary — UI callbacks, signal handling, tick loop, screen share |
| `ui_shell` | Slint UI components, data conversion, member widget |
| `audio_core` | Capture/playback, Opus encode/decode, DSP chain, jitter buffer |
| `voice_engine` | Mute/deafen/PTT state machine |
| `net_control` | WebSocket + UDP client, reconnect logic |
| `media_transport` | Audio frame routing between network and audio engine |
| `perf_metrics` | CPU/memory/audio metrics collection |
| `config_store` | JSON config persistence (devices, keybinds, volumes, notes) |
| `shared_types` | SignalMessage protocol, AppState, all shared data types |
| `signaling_server` | WebSocket + UDP relay server, SQLite persistence, room/space management |
| `integration_tests` | 98 server tests + 10 live stress tests |

## Architecture

- **Audio path**: cpal capture → noise gate → AGC → HPF → Opus encode → UDP/WebSocket → Opus decode → per-peer AGC → SPSC ring buffer → cpal playback
- **State**: `Rc<RefCell>` on UI thread, `Arc<TokioMutex>` for async, `AtomicBool/U32/U64` for cross-thread flags
- **Networking**: WebSocket signaling, server-relayed UDP audio with 8-byte session tokens
- **UI loop**: Slint timer at 40Hz (25ms tick) for polling events
- **Async runtime**: Tokio (2 threads) for networking

## Key Files

| File | What's in it |
|------|-------------|
| `crates/app_desktop/src/main.rs` | App entry, window setup, runtime init |
| `crates/app_desktop/src/callbacks/` | All UI callback wiring (space, room, channel, chat, controls) |
| `crates/app_desktop/src/signal_handler/mod.rs` | Signal dispatch from server messages |
| `crates/signaling_server/src/main.rs` | Server entry, WebSocket handler, audio relay, signal routing |
| `crates/signaling_server/src/handlers/` | Channel, room, space, chat, auth handlers |
| `crates/signaling_server/src/persistence.rs` | SQLite schema and queries |
| `crates/shared_types/src/lib.rs` | SignalMessage enum (50+ variants), all shared structs |
| `crates/ui_shell/ui/main.slint` | Root UI component, all properties and callbacks |
| `crates/ui_shell/ui/theme.slint` | Data structs, theme colors, design tokens |
| `crates/ui_shell/ui/views/` | Home, Room, Space, Chat, Settings, System views |
| `crates/audio_core/src/lib.rs` | AudioEngine — capture, playback, peer buffers |
| `crates/audio_core/src/codec.rs` | DSP: noise gate, AGC, HPF, comfort noise, soft clip |
| `crates/audio_core/src/buffers.rs` | SPSC ring buffers, jitter buffer, capture ring |

## Branding

- **App name**: Voxlink ("Voice without limits")
- **UI prefix**: Vx (VxTheme, VxButton, VxCard, VxInput, VxSlider)
- **Accent**: violet-indigo (#7c5bf5 dark / #6841ea light)
- **Config path**: `com.voxlink.Voxlink`
- **Log file**: `voxlink.log`

## Installers

| Platform | Script | Output |
|----------|--------|--------|
| Windows | `installer/voxlink.iss` (Inno Setup) | `Voxlink-Setup-0.7.0.exe` |
| macOS | `installer/build-macos.sh` | `.dmg` |
| Linux | `installer/build-linux.sh` | `.deb` + tarball |

## Build & Test

```bash
cargo check                                    # Compile check (zero warnings required)
cargo test --workspace --exclude integration_tests  # Unit tests only
cargo test -p integration_tests --test server_tests # Integration tests (needs live server)
cargo test -p integration_tests --test live_stress_test # Stress tests (needs live server)
cargo build --release -p app_desktop           # Build client
cargo build --release -p signaling_server      # Build server
```

## Feature Summary (v0.7.0)

- Create/join rooms by code, spaces with invite codes
- Open mic + push-to-talk, mute/deafen, per-peer volume
- Voice channels + text channels in spaces
- Friends, presence, DMs, friend requests
- Chat: send/edit/delete/react/pin/search, typing indicators, markdown
- Moderation: kick/ban/timeout/server-mute, role management (Owner/Admin/Mod/Member)
- Screen share with adaptive quality
- Priority speaker, whisper/private voice
- Channel user limits, slow mode, categories, status text
- User notes (local-only)
- UDP audio transport with WebSocket fallback
- Auto-reconnect, device hotplug recovery
- Performance panel with audio metrics
- Global hotkeys (PTT, mute, deafen)
- Desktop notifications, system tray
- Theme presets (Default, Party, Space, Retro, Amber, Noir, Arctic)
- Auto-update check

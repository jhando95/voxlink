# Voxlink

**Voice without limits.** A fast, private desktop voice chat built from scratch in Rust.

## Download

### [Download Voxlink](https://github.com/jhando95/voxlink/releases/latest)

Available for **Windows** (.exe installer), **macOS** (.dmg), and **Linux** (.deb + tarball).

---

## What is Voxlink?

Voxlink is a lightweight voice chat app for groups. Think Discord voice channels, but:
- **Instant startup** — no loading screens
- **Tiny footprint** — minimal CPU and RAM usage
- **No Electron** — native Rust, not a wrapped web browser
- **Privacy-first** — no telemetry, no tracking, self-hostable server

## Features

**Voice**
- Crystal-clear audio (Opus adaptive bitrate, fullband 48kHz)
- Automatic gain control — everyone sounds the same volume
- Neural noise suppression (RNNoise) with adjustable sensitivity
- Volume ducking — auto-lower non-speaking peers
- Mute / Deafen with distinct audio feedback tones
- Push-to-talk or open mic modes
- Per-user volume control
- Soundboard — play WAV clips into voice chat
- Priority speaker and whisper modes

**Social**
- Spaces & Channels (like Discord servers) with invite codes
- Text chat with markdown, reactions, pins, search, threads
- @Mentions with notifications
- Group DMs and direct messages
- Friends list with online/idle/DND/invisible status
- Email account system — sign in across devices
- Block/unblock users
- Server nicknames per space

**Organization**
- Channel categories for grouping
- Quick switcher (Ctrl+K) for fast navigation
- Unread indicators and mention badges
- Per-channel notification settings (all / mentions / none)
- Invite expiration and max-use limits
- Compact chat density mode, spoiler tags

**Moderation**
- Kick, ban, timeout, server-mute
- Role hierarchy: Owner / Admin / Mod / Member
- Ban management with unban UI
- Audit log

**Technical**
- UDP audio transport with WebSocket fallback
- Global hotkey support (mute/deafen/PTT)
- Screen share with adaptive quality
- Auto-reconnect and device hotplug recovery
- 7 theme presets (Default, Party, Space, Retro, Amber, Noir, Arctic)
- Performance panel (CPU, memory, latency, jitter, frame loss)
- Auto-update checker
- Desktop notifications and system tray

## Getting Started

1. **Download and install** from the [Releases page](https://github.com/jhando95/voxlink/releases/latest)
2. **Open Voxlink** — create an account or continue as guest
3. **Create or join a Space** — share the invite code with friends
4. **Talk**

The public server is already running — no setup needed.

## Self-Hosting

Want to run your own server? The installer includes `Voxlink-Server.exe`:

```sh
Voxlink-Server.exe
```

Runs on port 9090 (WebSocket) and 9091 (UDP audio). Tell your friends to connect to `ws://YOUR_IP:9090`.

## Building from Source

```sh
cargo build --release
```

Requires: Rust 1.94+, CMake, C++ build tools (for libopus).

## Architecture

```
crates/
├── app_desktop/        Main binary — startup, wiring, lifecycle
├── signaling_server/   WebSocket + UDP server — rooms, spaces, auth, audio relay
├── ui_shell/           Slint UI — views, components, data conversion
├── audio_core/         Audio capture/playback, Opus encode/decode, DSP pipeline
├── voice_engine/       Mute/deafen/PTT logic, voice session state
├── net_control/        WebSocket + UDP client, reconnect logic
├── media_transport/    Wires audio engine to network client
├── perf_metrics/       Real CPU/memory sampling via sysinfo
├── config_store/       Persistent JSON settings
├── shared_types/       Shared enums, DTOs, protocol messages
└── integration_tests/  Server integration + live stress tests
```

Audio pipeline: cpal capture → high-pass filter → noise gate → AGC → de-esser → neural noise suppression → Opus encode (adaptive bitrate) → UDP/WebSocket → Opus decode (with PLC/FEC) → jitter buffer → playback AGC → soft clip → cpal playback

## Stats

- **Version**: 0.8.0
- **Tests**: 338 (zero warnings)
- **Crates**: 11

## License

Private. All rights reserved.

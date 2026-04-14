# Voxlink

**Voice without limits.** A fast, private desktop voice chat built from scratch in Rust.

## Download

### [Download Voxlink](https://github.com/jhando95/voxlink/releases/latest)

Available for **Windows** (.exe installer), **macOS** (.dmg), and **Linux** (.deb + tarball).

---

## What is Voxlink?

Voxlink is a lightweight voice chat app for groups. Think Discord voice channels, but:
- **Instant startup** — no loading screens, native binary
- **142 MB memory** — vs Discord's 300-500 MB (Electron)
- **0.3% idle CPU** — near-zero when not in a call
- **No Electron** — native Rust + Slint, not a wrapped web browser
- **Privacy-first** — no telemetry, no tracking, no voice recording, self-hostable server

## Why Voxlink over Discord?

| Feature | Voxlink | Discord |
|---------|---------|---------|
| Per-user 3-band EQ | Bass/mid/treble per person | Volume only |
| Stereo panning | Position users left/right | Mono mix |
| Real-time VU meters | Level bars per participant | Ring glow only |
| Live bandwidth display | Exact kbps + session total | Hidden |
| Audio pipeline transparency | See all 7 processing stages | Black box |
| Privacy dashboard | Full data disclosure | 50-page ToS |
| No telemetry | Zero analytics | Heavy tracking |
| Memory usage | ~142 MB | ~300-500 MB |
| Server binary | 5 MB / 5 MB RAM | Cloud-only |
| Startup time | <1 second | 5-10 seconds |

## Features

**Voice — Studio-Grade Audio Control**
- Crystal-clear audio (Opus adaptive bitrate, fullband 48kHz)
- Per-user 3-band equalizer (bass/mid/treble at 300Hz/1kHz/3kHz)
- Stereo panning — position each person in the stereo field
- Real-time audio level meters with smooth decay
- Automatic gain control — everyone sounds the same volume
- Neural noise suppression (RNNoise) with adjustable sensitivity
- Volume ducking — auto-lower non-speaking peers
- Per-user volume control saved by name
- Push-to-talk or open mic modes
- Soundboard — play WAV clips into voice chat
- Priority speaker and whisper modes
- DM voice calls — call friends directly
- Voice quality presets: Economy (16k) / Standard (32k) / High (64k) / Studio (128k)
- Live bandwidth display with estimated data usage per hour

**Social**
- Spaces & Channels (like Discord servers) with invite codes
- Text chat with markdown, reactions, pins, search, threads
- @Mentions with autocomplete dropdown
- Group DMs and direct messages
- Friends list with online/idle/DND/invisible status
- Last seen timestamps for offline friends
- Email account system — sign in across devices
- Block/unblock users
- Server nicknames per space
- Activity status ("Playing Valorant")
- Role colors for visual distinction
- User profile popups

**Chat Polish**
- Message copy button
- Recent reactions quick-access (last 5 emojis)
- Message date separators ("Today", "Yesterday")
- Reply quote blocks and thread view
- Message forwarding with attribution
- File attachments with download cards
- Link detection in messages
- Character counter (warns at 1800+)
- Spoiler tags, compact chat mode
- Typing indicators with animated dots

**Organization**
- Channel categories with collapse toggle
- Quick switcher (Ctrl+K) for fast navigation
- Favorite channels pinned to top
- Unread indicators and mention badges
- Per-channel notification settings (all / mentions / none)
- Keyboard shortcuts help overlay (Ctrl+/ or ? button)
- Channel topic display in voice room header
- Invite expiration and max-use limits

**Moderation**
- Kick, ban, timeout, server-mute, server-deafen
- Role hierarchy: Owner / Admin / Mod / Member
- Auto-moderation word filter per space
- Ban management with unban UI
- Audit log
- Welcome messages for new space members

**Scheduled & Automated**
- Scheduled events with interest tracking
- Message scheduling (send later)
- Channel auto-delete (expire messages after N hours)
- Voice recording indicators

**Privacy & Transparency**
- Privacy dashboard showing exactly what data is stored
- Audio processing pipeline visualization (7 stages)
- No voice data ever recorded or stored
- No telemetry or analytics collected
- Clear Local Data and Export My Data buttons
- Self-hostable server — your data stays on your hardware

**Technical**
- Zero-allocation audio callbacks (pre-allocated buffer pool)
- Lock-free DSP: biquad EQ, noise gate, AGC, de-esser, soft clip
- UDP audio transport with WebSocket fallback
- Incremental UI updates (only changed rows redrawn)
- Per-connection reusable relay buffers on server
- Global hotkey support (mute/deafen/PTT)
- Screen share with adaptive quality
- Auto-reconnect and device hotplug recovery
- 7 theme presets (Default, Party, Space, Retro, Amber, Noir, Arctic)
- Performance panel (CPU, memory, latency, jitter, frame loss)
- Connection status bar (ping, protocol, quality indicator)
- Auto-update checker
- Desktop notifications and system tray
- First-run onboarding tutorial
- About dialog with bug report link

## Getting Started

1. **Download and install** from the [Releases page](https://github.com/jhando95/voxlink/releases/latest)
2. **Open Voxlink** — create an account or continue as guest
3. **Create or join a Space** — share the invite code with friends
4. **Talk**

The public server is already running — no setup needed.

## Self-Hosting

Want to run your own server? The installer includes the server binary:

```sh
# Windows
Voxlink-Server.exe

# Linux/macOS
./signaling_server
```

Runs on port 9090 (WebSocket) and 9091 (UDP audio). Tell your friends to connect to `ws://YOUR_IP:9090`.

Server requirements: **5 MB disk, 5 MB RAM, any Linux/macOS/Windows machine.**

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
├── audio_core/         Audio capture/playback, Opus encode/decode, DSP, EQ
├── voice_engine/       Mute/deafen/PTT logic, voice session state
├── net_control/        WebSocket + UDP client, reconnect logic
├── media_transport/    Wires audio engine to network client (buffer pool)
├── perf_metrics/       Real CPU/memory sampling via sysinfo
├── config_store/       Persistent JSON settings
├── shared_types/       Shared enums, DTOs, protocol messages
└── integration_tests/  Server integration + live stress tests
```

Audio pipeline: cpal capture → high-pass filter → noise gate → AGC → de-esser → neural noise suppression → Opus encode (adaptive bitrate) → UDP/WebSocket → Opus decode (with PLC/FEC) → jitter buffer → per-user EQ → stereo pan → playback AGC → volume ducking → soft clip → cpal playback

## Stats

- **Version**: 0.10.4
- **Tests**: 355+ (zero warnings)
- **Crates**: 11
- **Client binary**: 32 MB
- **Server binary**: 7 MB
- **Client idle memory**: ~142 MB
- **Server idle memory**: ~5 MB
- **Client idle CPU**: <0.5%
- **Server idle CPU**: 0.0%

## License

Private. All rights reserved.

# Voxlink

**Voice without limits.** A fast, private desktop voice chat built from scratch in Rust.

## Download

### [⬇️ Download Voxlink for Windows](https://github.com/jhando95/voxlink/releases/latest)

Double-click the installer, done. No accounts, no sign-ups, no bloat.

---

## What is Voxlink?

Voxlink is a lightweight voice chat app for groups. Think Discord voice channels, but:
- **Instant startup** — no loading screens
- **Tiny footprint** — minimal CPU and RAM usage
- **No Electron** — native Rust, not a wrapped web browser
- **Privacy-first** — no telemetry, no tracking, self-hostable server

## Features

- Crystal-clear voice (Opus adaptive bitrate, fullband audio)
- Automatic gain control — everyone sounds the same volume
- Neural noise suppression (RNNoise) with adjustable sensitivity
- Mute / Deafen with distinct audio feedback tones
- Push-to-talk or open mic modes
- Spaces & Channels (like Discord servers)
- Text chat within channels
- Per-user volume control
- Global hotkey support (mute/deafen/PTT)
- Dark mode
- Performance panel (CPU, memory, latency)

## Getting Started

1. **Download and install** from the [Releases page](https://github.com/jhando95/voxlink/releases/latest)
2. **Open Voxlink** and pick a username
3. **Create or join a Space** — share the invite code with friends
4. **Talk**

The public server is already running — no setup needed.

## Self-Hosting

Want to run your own server? The installer includes `Voxlink-Server.exe`:

```sh
Voxlink-Server.exe
```

Runs on port 9090. Tell your friends to connect to `ws://YOUR_IP:9090`.

## Building from Source

```sh
cargo build --release
```

Requires: Rust, CMake, C++ build tools (for libopus).

## Architecture

```
crates/
├── app_desktop/        Main binary — startup, wiring, lifecycle
├── signaling_server/   WebSocket server — room management, audio relay
├── ui_shell/           Slint UI — views, components, user interaction
├── audio_core/         Audio capture/playback, Opus encode/decode, DSP
├── voice_engine/       Mute/deafen/PTT logic, voice session state
├── net_control/        WebSocket client for signaling + audio transport
├── media_transport/    Wires audio engine to network client
├── perf_metrics/       Real CPU/memory sampling via sysinfo
├── config_store/       Persistent JSON settings
└── shared_types/       Shared enums, DTOs, protocol messages
```

Audio pipeline: cpal capture → high-pass filter → noise gate → AGC → de-esser → neural noise suppression → Opus encode (adaptive bitrate) → UDP/WebSocket → Opus decode (with PLC/FEC) → jitter buffer → playback AGC → soft clip → cpal playback

## License

Private. All rights reserved.

# Voxlink

Voice without limits. A private desktop voice application built with Rust and Slint.

## Quick Start

```sh
# Terminal 1: Start the signaling server
cargo run --release --bin signaling_server

# Terminal 2: Start the desktop app
cargo run --release --bin app_desktop
```

Then in the app:
1. Enter your name
2. Click "Connect" (default: ws://127.0.0.1:9090)
3. Click "Create Room" or enter a room code and "Join"
4. Talk! Use Mute/Deafen/PTT controls as needed

To test with multiple users, run `app_desktop` in separate terminals.

## Workspace Structure

```
crates/
├── app_desktop/        Main binary — startup, wiring, lifecycle
├── signaling_server/   WebSocket server — room management, audio relay
├── ui_shell/           Slint UI — views, components, user interaction
├── audio_core/         cpal-based capture/playback, encode/decode
├── voice_engine/       Mute/deafen/PTT logic, voice session state
├── net_control/        WebSocket client for signaling + audio transport
├── media_transport/    Wires audio engine to network client
├── perf_metrics/       Real CPU/memory sampling via sysinfo
├── config_store/       Persistent JSON settings
└── shared_types/       Shared enums, DTOs, protocol messages
```

## Features

- **Create/join rooms** by 6-digit code
- **Real-time voice** via cpal audio capture/playback
- **Open mic** and **push-to-talk** modes
- **Mute/deafen** controls
- **Participant list** with mute indicators
- **Input/output device selection** from real system devices
- **Performance panel** showing real CPU, memory, uptime
- **Settings persistence** to JSON config file
- **Dark theme** native Slint UI
- **WebSocket signaling** + audio relay server

## Architecture

- Audio captured at 48kHz mono, encoded to i16 for wire, decoded to f32 for playback
- Server relays audio as binary WebSocket frames
- Signaling uses JSON messages over the same WebSocket connection
- UI polls for network events at 60fps via Slint timer
- Tokio runtime handles async networking on 2 worker threads
- Audio callbacks run on dedicated OS threads (never blocked by UI/network)

## Building

```sh
cargo build              # debug
cargo build --release    # optimized
```

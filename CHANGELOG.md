# Changelog

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

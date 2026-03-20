# Changelog

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

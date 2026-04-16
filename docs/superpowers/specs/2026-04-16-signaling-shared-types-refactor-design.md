# Design — Milestone 1: Signaling Server & Shared Types Refactor

**Date:** 2026-04-16
**Status:** Approved (pending user review of written spec)
**Scope:** Pure file reorganization. Zero behavior changes.

## Context

Voxlink v0.9.1 has two files that have grown too large to hold in context and review cleanly:

- `crates/signaling_server/src/main.rs` — 4,238 lines. Mixes bootstrap, TLS plumbing, metrics HTTP server, LAN discovery, the WebSocket accept loop, a 618-line signal-dispatch match, ~40 per-variant handler functions, and all audio/screen relay logic (WS + UDP).
- `crates/shared_types/src/lib.rs` — 2,637 lines. Holds view enums, state structs, a 132-variant `SignalMessage` enum (~850 lines on its own), message-data helper structs, the screen chunk codec, and small helpers.

Both files are central — `shared_types` is depended on by nearly every other crate, so rebuild times suffer when it changes. `main.rs` holds the server's hot path and is hard to review or extend.

This milestone is the first of four on the current roadmap (refactors → security foundations → hardening/observability → E2E DMs). Refactors go first because they are low-risk and they reduce the surface area of the files touched by later milestones.

## Goals

1. Split both files into focused modules, each with a single clear responsibility.
2. Preserve existing external API (`use shared_types::SignalMessage` must keep working unchanged across the workspace).
3. Keep the workspace compileable and all 351 tests green after every commit.
4. Preserve the zero-warnings / zero-clippy-warnings policy.

## Non-goals

- No behavior changes.
- No API changes visible to callers.
- No new features, no dead-code removal beyond what trivially falls out of the move.
- No changes to unrelated files (the uncommitted working-tree changes stay untouched — the refactor commits are layered on top of whatever branch the user is on).

## Part A — Split `signaling_server/src/main.rs`

Target: `main.rs` holds only bootstrap (env parsing, tokio runtime setup, socket binds, spawn of accept loops and background tasks). Target size ~200 lines.

### Proposed module layout

All paths are under `crates/signaling_server/src/`.

| File | Contents | Est. lines |
|---|---|---|
| `main.rs` | Bootstrap only: env parsing, tokio runtime, socket binds, spawn accept loops | ~200 |
| `tls.rs` | `ServerStream` enum + `AsyncRead`/`AsyncWrite` impls, `load_tls_config`, `bind_requires_tls`, `allow_insecure_public_bind` | ~150 |
| `metrics_server.rs` | `ServerMetrics` struct + `Default` impl, `run_metrics_server`, `render_metrics` | ~250 |
| `discovery.rs` | `run_discovery` (LAN UDP broadcast) | ~40 |
| `connection.rs` | `handle_connection` (per-peer WS loop), `handle_disconnect`, `send_to`, `send_error`, `decrement_ip` | ~270 |
| `dispatch.rs` | `handle_signal` (the big match) | ~620 |
| `validation.rs` | `validate_name`, `validate_room_code`, `validate_password`, `check_rate_limit`, `atomic_rate_check`, `instant_to_ms`, `chunked_screen_sequence_state`, `now_epoch_secs` | ~150 |
| `relay/mod.rs` | `pub mod` declarations, shared helpers if any | ~10 |
| `relay/audio.rs` | `relay_audio`, `relay_audio_udp` | ~250 |
| `relay/screen.rs` | `prepare_screen_relay`, `relay_screen`, `relay_screen_chunk`, `relay_screen_udp`, `send_screen_frame_to_peers`, `screen_chunk_is_plausible` | ~370 |
| `relay/udp.rs` | `run_udp_relay`, `handle_request_udp`, `hex_encode` | ~260 |
| `handlers/calls.rs` | `handle_call_user`, `handle_accept_call`, `handle_decline_call` | ~150 |
| `handlers/events.rs` | `handle_create_event`, `handle_delete_event`, `handle_toggle_event_interest`, `handle_list_events` | ~200 |
| `handlers/scheduling.rs` | `handle_schedule_message`, `handle_cancel_scheduled_message`, `handle_set_welcome_message` | ~180 |
| `handlers/recording.rs` | `handle_start_recording`, `handle_stop_recording`, `handle_send_voice_note` | ~250 |
| `handlers/account.rs` | `handle_set_display_name`, `handle_delete_account`, `handle_set_user_status`, `handle_set_profile` | ~150 |
| `handlers/channel_settings.rs` | `handle_set_channel_topic`, `handle_channel_setting`, `handle_set_priority_speaker`, `handle_set_space_public`, `handle_browse_public_spaces` | ~300 |
| `handlers/timeouts.rs` | `handle_timeout_member` | ~80 |
| `handlers/whisper.rs` | `handle_whisper_to`, `handle_whisper_stopped` | ~30 |

This follows the existing `handlers/{auth,chat,room,space,channel,presence,friends,moderation}.rs` pattern — it finishes the job rather than introducing a new convention.

### Visibility rules

- Types declared in `types.rs` (existing) stay in `types.rs` and gain any new `pub(crate)` exposure needed.
- Functions that are only called inside the crate become `pub(crate)`. No new `pub` APIs.
- Module-private helpers stay private unless another module needs them.

### What stays in `main.rs`

Only:
1. `mod` declarations for all new modules.
2. Imports needed by bootstrap.
3. Constants (`MAX_NAME_LEN`, `MAX_PASSWORD_LEN`, `DB_TIMEOUT`).
4. `ServerLimits` struct and `env_or` helper.
5. `async fn main()` — the startup sequence.

## Part B — Split `shared_types/src/lib.rs`

Target: `lib.rs` becomes a thin re-export surface. External callers (`use shared_types::SignalMessage`) see no change.

### Proposed module layout

All paths under `crates/shared_types/src/`.

| File | Contents | Est. lines |
|---|---|---|
| `lib.rs` | `pub mod` declarations + `pub use` re-exports | ~60 |
| `view.rs` | `AppView`, `MicMode`, `ConnectionState`, `UserStatus`, `SpaceRole` (+ impl), `ChannelType`, `AppState` | ~160 |
| `state.rs` | `Participant`, `RoomState`, `SpaceInfo`, `ChannelInfo`, `MemberInfo`, `BanInfo`, `SpaceAuditEntry`, `SpaceState`, `PendingMessage`, `PerfSnapshot`, `FriendPresence`, `FriendRequest`, `DirectMessageThread`, `FavoriteFriend` | ~310 |
| `protocol.rs` | `SignalMessage` enum only | ~850 |
| `message_data.rs` | `TextMessageData`, `ReactionData`, `ParticipantInfo`, `SpaceSearchResult`, `PublicSpaceInfo`, `AutomodWord`, `ScheduledEvent` | ~180 |
| `screen.rs` | `ScreenChunkMetadata`, `encode_screen_chunk_metadata`, `decode_screen_chunk_metadata` | ~50 |
| `helpers.rs` | `default_voice_quality`, `default_search_limit`, `voice_quality_bitrate`, `voice_quality_kbps`, `voice_quality_label`, `extract_first_url` | ~50 |
| `tests.rs` | Existing test module (moved verbatim) | unchanged |

### Re-export policy

`lib.rs` uses `pub use <module>::*;` for each module so every existing `use shared_types::<Name>` call site keeps working with no edits. This is the only way to keep the refactor a pure reorganization — any narrower re-export policy would require touching consumers.

### `SignalMessage` stays one file

The user asked whether to subdivide the 132-variant enum further (e.g., `protocol/auth.rs`, `protocol/room.rs`). Answer: no — it is a single Rust enum and must live in a single file. Splitting would force either an artificial wrapper enum or heavy macro machinery. The file is long but has uniform structure (variant + serde attrs), which is the easy-to-review kind of long.

## Verification

After each commit:

- `cargo check --workspace` — must succeed
- `cargo clippy --workspace --all-targets -- -D warnings` — zero warnings
- `cargo test --workspace` at the end of each Part — all 351 tests pass

No new tests are added. Existing tests are the regression harness; if any test breaks, the refactor introduced a behavior change and the commit is wrong.

## Risk & rollback

- **Risk: accidental behavior change during move.** Mitigation: each commit does a mechanical move of one logical unit. No renames, no signature changes, no inlining. Tests catch regressions.
- **Risk: import ordering or macro-hygiene issues in `shared_types` re-exports.** Mitigation: `cargo check` after every file split. If a downstream crate fails to compile, fix by adjusting `pub use` in `lib.rs`, not by editing consumers.
- **Risk: conflict with uncommitted working-tree changes.** The user has uncommitted changes to `Cargo.lock`, `crates/app_desktop/*`, etc. Refactor commits touch only `signaling_server` and `shared_types` (and any necessary import updates elsewhere), so the working-tree changes will not overlap. We do not stash or reset their work.
- **Rollback:** Each commit is independently revertable. The workspace stays green at every commit boundary, so `git revert` of any single commit is safe.

## Commit strategy

Two logical Parts, multiple commits each. Each commit keeps the workspace green.

**Part A (signaling_server):**
1. Extract `tls.rs`
2. Extract `metrics_server.rs`
3. Extract `discovery.rs`
4. Extract `validation.rs`
5. Extract `relay/` (audio, screen, udp)
6. Extract `connection.rs` (+ `send_to`, `send_error`, `handle_disconnect`)
7. Extract `dispatch.rs` (`handle_signal` match)
8. Move per-variant handlers into `handlers/{calls,events,scheduling,recording,account,channel_settings,timeouts,whisper}.rs`

**Part B (shared_types):**
1. Extract `view.rs`
2. Extract `state.rs`
3. Extract `protocol.rs`
4. Extract `message_data.rs`
5. Extract `screen.rs`
6. Extract `helpers.rs`
7. Move tests to `tests.rs`
8. Thin `lib.rs` to re-exports

## Success criteria

1. `cargo check --workspace` clean after every commit.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean at end.
3. All 351 tests pass.
4. `main.rs` ≤ 250 lines, `shared_types/lib.rs` ≤ 80 lines.
5. No consumer crate outside `signaling_server` or `shared_types` needs edits beyond at most a handful of import path adjustments.

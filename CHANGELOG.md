# Changelog

## v0.14.0 — Visual redesign: quiet graphite, violet accent

Complete visual overhaul in response to the shell reading as janky and
cluttered on Windows: clipped text boxes, stray lines through text, three
competing button styles, native gray input chrome, and pill/status overload.

### One design system
- **Neutral graphite base for every theme preset** (dark + light). The seven
  presets are now accent-hue variants (violet default, green, blue, mint,
  amber, steel, cyan) on one tuned base instead of seven full-canvas tints —
  the biggest source of visual inconsistency. Per-preset shape gimmicks
  (radius, border weights, uppercase buttons, logo art) are gone; one radius
  scale (12/8), 1px hairlines, tonal depth only. Brand accent returns to
  Voxlink violet.
- **One button language.** Filled accent/danger, outline default, tonal soft,
  text ghost. Icon plates inside buttons removed. Buttons hug their content —
  the full-width slab primaries are gone (buttons and status pills now default
  to `horizontal-stretch: 0`, which also kills the stretched-pill borders that
  read as random lines through text).
- **Custom-drawn inputs on the raw `TextInput` primitive.** The std `LineEdit`
  painted platform-style chrome that fought the theme and clipped descenders
  at some DPI/font combinations — the "cut off text boxes". Same API,
  plus `forward-focus` and full-field click-to-focus. The std-widget style is
  pinned to `fluent-dark` in build.rs so the remaining std widgets (multi-line
  chat composer, scrollbars) match the theme on every platform.
- **Shell simplified.** TopBar is one flat 52px bar with a single hairline —
  the multi-hue gradient underline (the most-reported "line through text") is
  gone, as is the breadcrumb strip below it. The rail drops the SESSION
  marketing card (whose fixed-width content clipped mid-word), uses compact
  34px nav rows via a shared RailItem component, keeps a one-row live-room
  shortcut, and ends in a flat user footer. Reconnect banner is now a quiet
  warning strip instead of a red alarm.
- **Views swept**: hero cards trimmed, redundant description lines and counter
  pills removed, profile forms pair input + compact action, chat composer
  surface flattened, welcome/onboarding restyled, theme-preset cards show the
  new accent swatches.

### Tests
- Source-shape and snapshot tests recalibrated to the intentionally flatter
  design: measured-value-informed luma/edge floors (blank frames still fail by
  a wide margin), new assertions locking in the TextInput-based field and
  hugging action buttons.
- New `VOXLINK_START_VIEW` env var opens the app on a given view — used for
  screenshot-driven visual review, harmless in production.

## v0.13.5 — Input-poll efficiency & trustworthy test suite

Patch release: the last open item from the idle-CPU audit, plus the fix that
lets CI stop skipping four integration tests.

### In-call keyboard polling drops to 10 Hz for open-mic users
- The OS keyboard poll (`device_query`) ran at the full 40 Hz tick rate for
  the entire duration of every call. Only push-to-talk actually needs
  sub-100 ms press/release latency; everything else the poll feeds (mute /
  deafen hotkeys, soundboard combos, Esc, Ctrl+K, channel navigation) is an
  edge-triggered toggle or shortcut.
- In-call polling now runs at the 10 Hz out-of-call rate unless push-to-talk
  is the active mic mode, a keybind is being captured in Settings, or the
  quick switcher is open — those keep the full 40 Hz. Skipped ticks reuse the
  previous OS sample, so edge detection never sees a phantom release and
  hotkey cooldown timing is unchanged.
- This closes the last THROTTLE item in `docs/IDLE_AUDIT.md`: a 4× reduction
  in keyboard syscalls during calls for open-mic (default-mode) users.

### Flaky-test root cause fixed; CI skip list removed
- Integration tests reserve ephemeral ports by binding and dropping a probe
  socket before spawning the server process. Under parallel load another test
  could grab the port inside that window; the server then exited at startup
  and the test burned its full 20 s timeout — the "startup timeout" flakes
  that got `test_create_space`, `test_audio_after_leave_room`,
  `test_channel_audio_relay`, and `test_authenticate_invalid_token_creates_new`
  onto the CI skip list.
- `TestServer::start` (both `server_tests.rs` and `voice_pipeline_test.rs`)
  now detects the early server exit via `child.try_wait()` and respawns on
  fresh ports (up to 3 attempts) instead of waiting out the timeout.
- The four tests are back in the CI gate; only `live_stress` (needs the
  remote production server) remains skipped. `docs/CI.md` refreshed to match
  reality: strict clippy gate, release automation, and the simplified local
  gate commands.

## v0.13.4 — Custom roles are now authoritative

Server-side permission overhaul: the granular permission bitmask introduced
with the v0.13.0 role-management API now actually gates every moderation and
management action. Until now a custom role like "Bouncer (KICK_MEMBERS)" was
stored and broadcast but ignored — every handler still checked only the
legacy Owner/Admin/Moderator/Member ladder.

### Permission sweep (23 capability gates across 9 handler files)
- Every capability check now consults the actor's **effective permission
  bitmask** (OR of all held v2 roles + the @everyone default, plus
  OWNER_BYPASS for the space owner) instead of the legacy 4-tier role:
  kick/mute/server-deafen/ban/unban/view-bans (KICK_MEMBERS, MUTE_MEMBERS,
  DEAFEN_MEMBERS, BAN_MEMBERS), timeouts (TIMEOUT_MEMBERS), automod word
  lists (MANAGE_AUTOMOD), channel create/delete/topic/settings
  (MANAGE_CHANNELS), priority-speaker-on-others (PRIORITY_SPEAKER),
  scheduled events (MANAGE_EVENTS), start/stop recording (START_RECORDING /
  STOP_RECORDING), pin + delete-any-message (MANAGE_MESSAGES), audit-log
  visibility (VIEW_AUDIT_LOG), set-member-role + role colors (MANAGE_ROLES),
  rename/description/welcome/invite settings (MANAGE_SPACE).
- **Rank protection retained where it matters**: elevated targets
  (Moderator/Admin/Owner on the legacy ladder) can still only be moderated
  by a strictly higher tier; plain Members can be moderated by anyone
  holding the capability bit, so custom roles work without a legacy tier.
- Channel `min_role` gates are unchanged — that's a parallel per-channel
  access mechanism, not an actor capability.

### Per-peer permission cache
- New `Peer.space_perms: AtomicU64` — effective bitmask cached lock-free per
  connection. Refreshed on space join/create, SetMemberRole, role
  assign/unassign (per-user), role update/delete (whole space), and cleared
  on leave/kick/space-delete/identity-switch. Permission checks on hot
  handler paths are now a single atomic load instead of a DB query.
- Fresh spaces seed their managed role catalog (@everyone + legacy trio) at
  creation time instead of only after the next server restart, and legacy
  SetMemberRole writes through to the v2 role tables so both systems agree.
- Anonymous space owners can now use the role-management API (the handlers
  resolved identity differently from the rest of the server and rejected
  unauthenticated owners).

### Tests
- New integration test proves a custom role holding only KICK_MEMBERS grants
  kick capability to a legacy plain Member (and that the same member is
  denied before the role is assigned).

## v0.13.3 — Zero clippy warnings, rustfmt everywhere, CI actually green

Hygiene + CI-reliability release. No user-facing feature changes; the ships
here are that the workspace now holds a strict lint bar and the CI pipeline
that was silently red on every push since it was added is now green and
trustworthy.

### Lint: workspace at zero clippy warnings, gated
- **~55 clippy warnings across all targets → 0.** Highlights beyond the
  mechanical sweep (`useless_conversion`, `map_or`, casts, `format!`,
  `while_let_loop`, `needless_range_loop`, `iter_nth`, stale doc comment):
  - **live_stress_test's `SERIAL` mutex → `tokio::sync::Mutex`** — the guard
    is held across every await in each test body; the std guard blocked the
    runtime worker (10 × `await_holding_lock`). Bonus: no poisoning, so one
    panicking test no longer cascades `PoisonError`s into the rest.
  - **`render_space` 12 positional args → 5** via new `SpaceSocialContext` +
    `SpaceRenderPrefs` structs. It had grown an argument every time a local
    preference was threaded in; named fields stop the drift.
  - **`handle_update_role` PATCH fields → `RoleUpdate` struct.**
  - **`create_scheduled_event` / `schedule_message` → `NewScheduledEvent` /
    `NewScheduledMessage` named-field param structs**, and the 6-tuple from
    `get_due_scheduled_messages` → `DueScheduledMessage` row struct. Wide
    positional DB write paths were an audit-flagged argument-order hazard.
  - **`ChunkedScreenSequenceState::{NewFrame,ExistingFrame,StaleFrame}` →
    `{New,Existing,Stale}`** (enum_variant_names).
  - `SoundboardCombos` type alias for the shared hotkey-binding list.
- **CI clippy gate tightened from "≤63 warnings" ratchet to
  `cargo clippy --workspace --all-targets -- -D warnings`.** New warnings now
  fail CI instead of accruing.
- **`cargo fmt --all` run once** (46 files) so the committed
  `cargo fmt --check` lint gate passes; the tree stays rustfmt-clean from
  here on.

### CI: fixed the two structural failures that kept every run red
- **build-test**: `server_tests` spawn `target/debug/signaling_server` and
  `app_desktop` at runtime, but the job only ran `cargo check` + `cargo test`,
  which never produce those binaries on a fresh runner — every integration
  test failed with "No such file or directory". Added an explicit
  `cargo build -p signaling_server -p app_desktop` step.
- **windows-installer**: `voxlink.iss` bundles
  `installer/redist/vc_redist.x64.exe`, which is gitignored; release.yml
  downloads it but ci.yml didn't, so ISCC aborted on the missing source
  file. Added the same download step.

### Also in this release (landed alongside)
- **Role-assignment privilege-escalation guard** — `AssignRoleToMember` now
  verifies the role belongs to the actor's space and that the actor holds
  every permission the role grants (or is an administrator). Previously
  MANAGE_ROLES alone could self-assign an ADMINISTRATOR role.
- **Presence is friendship-gated** — `WatchFriendPresence` only reveals
  presence for users the watcher is actually friends with, closing a
  privacy oracle that let arbitrary user IDs be probed for online state and
  current space/channel.
- **Timeout rank checks** — a moderator can no longer time out an equal or
  higher-ranked member, and the target must belong to the actor's own space.
- **Scheduled-event deletion is space-scoped** — deleting an event now
  verifies it belongs to the actor's space instead of trusting the id.

## v0.13.2 — UI: simplification + token consolidation

Audit-driven simplification on top of v0.13.1's section-definition pass. No
new functionality; the .slint files are shorter and the cascades they used
to type out 4-7 times now live in one place.

- **`pick-emoji(emoji)` function on the chat root** replaces 8 hand-typed
  copies of the same add-vs-react + close-picker + clear-search bookkeeping
  (7 emoji-picker tabs + recent-reactions strip). Adding a new tab or
  changing the click behavior is now a one-line edit instead of a
  cross-file find-and-replace.
- **Toast severity cascade hoisted** to derived `toast-accent` /
  `toast-glyph` / `toast-text` / `toast-meta` properties on the toast
  Rectangle. The same 4-arm ternary previously appeared in background,
  border, icon glyph, label color, and close-X color bindings; now each
  references the cached value. Adding a severity tier is 2 line edits, not
  4-5.
- **Room status-bar quality-color hoisted** to `status-bar-row.quality-color`
  + `quality-label`. The connection-quality cascade (none/connecting/good/
  unstable/poor) was previously typed three times across the dot, label
  text, and ping color bindings. New tiers now go in one place.
- **`VxTheme.pad-card` / `pad-card-compact` / `space-card-rows` tokens**
  introduced so future density tweaks land in one line instead of sweeping
  ~60 inline `padding: 18px` literals across the view files.
- **Dead default removed** — `version-text: "0.11.0"` literal default on
  the main window was three releases stale and only ever overwritten by
  Rust at startup. Now declared without a default so the actual version is
  always whatever Rust sets.

No protocol or functional changes — server doesn't need redeploy from
v0.13.0 or v0.13.1.

## v0.13.1 — UI: clearer sector definition

Cosmetic-only pass on top of v0.13.0. The view zones (sidebar / content /
members) now read as distinct surfaces at a glance instead of blending into
one tinted plane.

- **Stronger surface contrast in dark mode** — `bg-rail` (sidebar) pushed
  darker so the side panel visibly recedes; `bg-panel` (content surface) and
  `bg-card` (raised cards) lifted so the 3-tier hierarchy is unambiguous.
  All 7 themes shifted proportionally; light mode tweaked subtly so the
  same zoning reads.
- **`border-subtle` and `border-strong` pushed** about 25–30% more saturated
  so dividers between panels are visible without dominating.
- **Chat sidebar switched from `bg-panel` to `bg-rail`** with a 1px
  `border-strong` divider on its right edge — sidebar feels anchored to the
  screen edge instead of bleeding into the chat surface.
- **Member panel left divider upgraded** from `border-subtle` to `border-strong`
  so the members column reads as its own panel.
- **Active-channel / active-row left accent strip** thickened from 3px → 4px
  for clearer selection state.
- **`SectionLabel` typography** bumped to font-weight 800 with wider letter
  spacing (1.4px) and a more visible color tier (`text-secondary` not
  `text-muted`) — section headers act as intentional dividers.
- **`Divider` component** gained a `strong: true` mode (2px on `border-strong`)
  for use at major zone boundaries.

## v0.13.0 — Correctness, Durability, Personal-Safety & Custom Roles

A "complete-and-fully-functional" sweep driven by a workspace-wide audit, plus
the foundation + management API for custom roles. Closes concentrated
correctness and durability gaps, finishes the personal-safety story
(Block/Unblock), gives threads a working composer + transitive replies,
introduces a per-peer back-pressure model on the server, and lays the
groundwork for granular permissions. Server stays a pure relay; no new
transports, no new external deps.

### Deferred to future waves
- **Custom-role handler sweep** (~50 sites). The new system runs in parallel
  with the legacy 4-tier path via the v2 synthesis migration, so behavior is
  unchanged. Migrating each `has_at_least(SpaceRole::X)` to
  `perms.has(Permissions::FLAG)` is a focused mechanical sweep that's safer as
  its own wave with full coverage tests in between.
- **Per-channel permission overrides** (depends on the handler sweep above).
  Bit layout already reserves space (16–31 for text-channel actions, 32–47 for
  voice).
- **Video calling** — ≤8-peer HW-accelerated encode (VideoToolbox/NVENC/VAAPI),
  separate UDP lane, video tiles UI. Multi-week feature; design spec lives in
  `docs/superpowers/specs/2026-04-16-voxlink-roadmap.md` under v0.13/v0.14.
- **Typography token rollout** (~489 hardcoded font-size sites). Mechanical
  replacement, but several px → token mappings shift size by 1px which the UI
  snapshot tests are sensitive to. Needs a dedicated wave with visual
  verification.

### Fixed (P0 — audio + server correctness)
- **Opus decoder no longer requests FEC on every packet** — Normal in-order decodes now use `fec=false`; every "good" packet was previously decoding the *prior* FEC payload instead of current audio (audio_core/src/lib.rs:1186). Largest shipping audio-quality bug in v0.11.
- **rustls 0.23 CryptoProvider installed at startup** — TLS startup previously panicked unless `PV_ALLOW_INSECURE=1` was set. Server now installs `ring` as the default provider before any `ServerConfig::builder()` call (signaling_server/src/main.rs).
- **Playback callback no longer panics on oversized cpal callbacks** — `stereo_frames` is capped to the scratch buffer length with a one-shot warning, replacing a release-mode `debug_assert` that would still panic in debug.
- **Server now honors SIGTERM** — `systemctl stop` / `docker stop` were silently bypassing `ServerShutdown` broadcasts every redeploy; the accept-loop now races SIGINT *and* SIGTERM on Unix.

### Fixed (P1 — persistence + security)
- **SQLite pragmas hardened on open** — `foreign_keys=ON` (so declared `ON DELETE CASCADE` actually fires), `busy_timeout=5000` (matches DB_TIMEOUT), `synchronous=NORMAL` (durable + fast under WAL).
- **DeleteAccount requires current password + auth rate limit** — A stolen session token could previously destroy an account in one call. The protocol now carries `current_password`; the handler reuses the per-IP login rate limiter.
- **Legacy SHA-256 passwords auto-rehash to Argon2id on successful login** — Old hashes silently upgrade on next sign-in.
- **Moderation timeouts persist + survive reconnects** — New `space_timeouts(space_id, user_id, until_epoch, …)` table with PK and `idx_space_timeouts_until`. `handle_timeout_member` writes through; `handle_join_space` restores the active timeout when a peer reconnects.
- **Moderation timeouts also gate voice relay** — Both WS and UDP audio relay paths now drop frames while `timeout_until > now`. Previously, timeouts only suppressed chat sends.
- **Moderators+ can pin and delete messages** — Server-side permission check now ORs in `actor_role.has_at_least(Moderator)` alongside sender/owner.
- **Message reactions persist** — New `message_reactions(message_id, emoji, user_name, created_at)` table, idempotent add, JOIN-protected load, and replay onto loaded text-channel history on server startup.
- **Channel categories persist** — `channels.category` column + `set_channel_category` setter; the existing `SetChannelCategory` protocol now writes through.
- **Status preset persists on the Peer** — `Peer.status_preset` (Mutex<UserStatus>) is written by `handle_set_status_preset` and read at every `MemberInfo` build site, so reconnects no longer flip everyone back to Online.
- **DisplayNameChanged broadcasts to every surface** — Space, voice room, and friend-presence watchers, in addition to the originating peer.
- **Three more keychain stalls moved off the UI thread** — `load_auth_token()` now runs under `spawn_blocking` at the manual-connect and auto-reconnect sites in addition to startup.

### Personal safety (closes the v0.11 gap)
- **Block / Unblock now has a UI entry point** — Member context menu shows "Block" / "Unblock" toggle alongside Add Friend / Message. The button reflects the local blocked list (new `MemberData.is_blocked_by_me`). Client persists changes to `config.blocked_users` optimistically and re-syncs on server ack so a second device sees state changes.

### Threads
- **Inline reply composer in the thread panel** — New `send-thread-reply(parent_id, text)` callback wired through the chat view, with `thread-parent-id` propagated from `ThreadMessages`.
- **Reply chains return the full subtree** — `handle_get_thread` now BFS-walks `reply_to_message_id` instead of collecting only direct replies of the root.

### Mentions
- **Per-channel `mention_count` actually populated** — `SpaceState.mentioned_text_channels` is now maintained (incremented on `MentionNotification` for non-active channels, cleared when the channel is opened/deleted), and exposed through `render_space` to the existing warning-tinted `@N` Slint badge.

### Group DMs end-to-end (closes the v0.11 server-paid / client-unreachable gap)
- **New group-DM creation flow** — Home view DM section now has a "New group" toggle that opens an inline multi-select friend picker. Pick 2+ friends → "Create group" → `CreateGroupDM`; the resulting `GroupDMCreated` snapshot lands locally and auto-opens the chat. Selection model is `[string]` driven; `toggle-new-group-dm-friend` flips membership.
- **Group thread list in home view** — Server-paid `GroupDMSelected` and `GroupMessage` events update a new `AppState.group_dm_threads` list, surfaced as a Slint `GroupDMThreadData` model alongside 1:1 DMs.
- **Send-routing now knows about groups** — `chat-group-id` property routes send through `SendGroupMessage` when set, alongside the existing DM and channel paths. Cleared in `handle_text_channel_selected` / `handle_direct_message_selected` so context switches don't misroute.
- **Quick switcher now spans spaces, channels, 1:1 DMs, and group DMs** — Saved-space invite codes from `cfg.saved_spaces` are surfaced as switcher entries and joined via `JoinSpace`. Group DMs are disambiguated from channel IDs via a `group_dm_threads` membership check and opened with `SelectGroupDM`.

### Personal-safety + persistence cleanup
- **Forwarding preserves attachment + link metadata** — `handle_forward_message` now copies `attachment_id`, `attachment_name`, `attachment_size`, and `link_url` from the source. Bytes stay content-addressed via the existing attachments table; only the row reference moves.
- **BanList shows banned user names** — `load_bans_for_space` LEFT-JOINs against `users(display_name)` so `BanInfo.user_name` is populated from one query. Missing-user rows return an empty string (graceful, not crash).
- **Attachment cascade-delete on message delete** — `delete_message` now runs as a single transaction that drops the message row, its reactions, and the orphan attachment blob (only when no other message references the same `attachment_id`). Stops orphan bytes from accumulating against the storage cap forever.
- **Auth-adjacent endpoints are now rate-limited** — `ChangePassword`, `ChangeEmail`, and `RevokeAllSessions` reuse the per-IP 5-attempts-per-60s limiter that already protects `Login` and `CreateAccount`. Stops a stolen token from being a brute-force oracle against the user's current password.
- **Tighter email validation** — New `is_valid_email` rejects the obvious junk a `contains('@')` test waves through: empty local/TLD, whitespace, control bytes, `..` sequences, leading/trailing dots, 1-char TLDs.
- **Tighter attachment metadata caps** — `file_name` capped at 255 chars and rejected if it contains NUL or path separators; `mime` capped at 64 chars and rejected if it contains NUL; `caption` truncated to 500 chars.
- **`CreateScheduledEvent` actually validates inputs** — Empty/oversized titles, multi-megabyte descriptions, past-dated start times, and end-before-start intervals are now rejected with a clear error (was happily persisted to SQLite before).

### Perf
- **Audio relay shares one `Bytes` across recipients** — `relay_audio` and the UDP relay's WS-fallback now build a single `tokio_tungstenite::tungstenite::Bytes` per frame and clone it for each peer. Cheap refcount bump replaces the per-peer `Vec<u8>::clone()` allocation; per-frame fan-out cost drops from O(N·frame_len) to O(N).
- **`prepare_cached` everywhere in persistence** — All 33 `.prepare(...)` call sites in `signaling_server/src/persistence.rs` are now `.prepare_cached(...)`, so prepared statements survive per connection across calls. Biggest win on the chat hot path (`save_message`, `load_messages_for_channel`, `search_messages`).
- **Friend snapshot does one batched lookup, not N+1** — `load_snapshot_rows` collects every user_id it needs across friendships + incoming + outgoing requests, then resolves all display names through a new `find_user_names_by_ids(IN ?,?,…)` chunked query. A 50-friend user goes from 50 lookups to 1 (3 if outgoing/incoming requests are also large).

### Tests
- **+7 server unit tests, total 59** in `signaling_server` (was 52 in v0.11): reactions round-trip + dedup, `delete_message` cascade for attachment + reactions, `delete_message` keeps attachment when another message references it, `space_timeouts` upsert + load + clear + purge, batch user-name lookup (and missing-id absence), `load_bans_for_space` JOIN populates `user_name`, plus the new email-validator coverage.

### Outbound back-pressure overhaul (wave 12)
- **Per-peer bounded mpsc lanes** — every peer now owns two `tokio::sync::mpsc` channels and a single per-peer drain task that owns the WebSocket sink. Signaling lane capacity 256, media lane capacity 8 (≈160ms of audio at 50fps). Senders never lock anything: they `try_send` and the lane's overflow policy kicks in. New module `signaling_server::outbound` exports the drain loop + send helpers.
- **Lossy media lane** — audio + screen relay sites now call `outbound::try_send_media`. On Full: bump `ws_frames_dropped_lossy_total` and drop the frame; on Closed: silent. The 500ms / 300ms `tokio::time::timeout` wrappers and per-call `peer.tx.lock().await` are gone. Per-recipient cost is now ~one atomic + a `Bytes` refcount bump.
- **Reliable signaling lane with disconnect-on-overflow** — `connection::send_to` is now a non-async function: serialize → `try_send` on the signaling channel. On Full it bumps `ws_frames_dropped_overflow_total`, notifies the peer's `disconnect` watcher, and the rx loop tears the peer down on next iter. Caught the misbehaving / stuck client at the source instead of letting it pile up writes.
- **One drain task per peer** — owns the sink for the connection's lifetime; biases signaling over media in its `select!` so reliable messages never starve under audio fan-out. Exits cleanly when both senders drop or the sink errors; on error it `notify_one()`s the rx loop to break.
- **Pong-loop simplified** — keepalive task is now a thin `try_send_ping` against the signaling lane; no longer takes a sink mutex, no longer races against handlers for the lock.
- **Screen WS-fallback now shares Bytes** — previously did per-peer `frame.to_vec()`; now copies once and hands each recipient a refcount-bump clone, matching the audio relay pattern.
- **Three new Prometheus counters** exposed at `/metrics`: `voxlink_ws_frames_dropped_lossy_total`, `voxlink_ws_frames_dropped_overflow_total`, `voxlink_ws_overflow_disconnects_total`.
- **Migration mechanics** — ~114 `send_to(...).await` sites across the server crate became `send_to(...)`. Compiler-guided sweep; all 112 server integration tests + all 374 unit tests still green.
- **Removed `Peer.tx`** — the old `Mutex<SplitSink<...>>` field is gone; the sink lives in the drain task. No more cross-handler write contention.

### DM reactions persistence + versioned migration framework (wave 11)
- **DM reactions persist** — new `dm_reactions(message_id, emoji, user_name, created_at)` table with PK + `idx_dm_reactions_message`. Toggle (add ↔ remove) is server-decided by probing the DB. `delete_direct_message` runs in a transaction that drops the row's reactions first. `handle_select_direct_message` reloads reactions onto the history via `load_dm_reactions_for_pair(low, high)` using the canonical low/high ordering the `direct_messages` table already enforces. Two new unit tests cover the round-trip + cascade.
- **Versioned migration framework** — `apply_migrations()` reads `PRAGMA user_version`, treats everything the existing `ensure_column` calls have built as baseline `schema_version = 1`, stamps fresh and pre-versioned DBs at v1, and applies a `MIGRATIONS` array of post-baseline SQL scripts in order. Each migration runs in a single transaction with a `user_version` bump at the end; idempotent on re-run. Future schema changes no longer need ad-hoc `ensure_column` patches — append to `MIGRATIONS` and bump the version.

### Docs catch-up (wave 10)
- `PROJECT_REFERENCE.md` refreshed from v0.8.0 → v0.12.0: feature summary now spans v0.8 → v0.12 with per-version markers; tech stack lists Argon2id + rustls + WAL + prepare_cached; DB tables table gained `message_reactions`, `space_timeouts`, `user_read_state` rows.
- `docs/superpowers/specs/2026-04-16-voxlink-roadmap.md` updated: "current posture" bumped from v0.10.4 to v0.12.0, v0.11/v0.12 marked **shipped** with the actual shipped scope, and the originally-planned v0.12 ("Server Identity") work (full role matrix + video calling) is explicitly deferred to v0.13+.

### Server hardening (wave 9)
- **Limits framework** — Three new env-configurable caps with sensible defaults:
  `VOXLINK_MAX_SPACES_PER_USER` (20), `VOXLINK_MAX_CHANNELS_PER_SPACE` (100),
  `VOXLINK_MAX_MEMBERS_PER_SPACE` (500). Enforced in `handle_create_space`,
  `handle_create_channel`, and the JoinSpace member-add path with friendly
  error messages; logged at startup alongside existing limits.
- **Server-side read-state for multi-device sync** — New `user_read_state(user_id, channel_id, last_read_message_id, last_read_at)` table with PK and `idx_user_read_state_user` for fan-out reads. New protocol messages: client → server `MarkChannelRead { channel_id, message_id }`, server → client `ReadStateSnapshot { entries: Vec<ReadStateEntry> }`. On Authenticate / Login success the server fans out a snapshot of the user's full read state, so a fresh device starts in sync instead of showing everything as unread. Client wires both directions via a new `pending-mark-read` Slint property drained by the tick loop; inbound snapshots overlay the local `last_read_messages` map.

### Reliability + identity hygiene
- **Send-failed messages auto-queue for retry** — `setup_send_text_message`'s failure branch now pushes a serialized record (`type\u{1f}target\u{1f}content`) into a new `pending-outbox-drop-in` Slint property; the tick loop drains it into `AppState.pending_messages` on the UI thread, where the existing retry helper picks it up every ~2s. The user sees "Message queued — will retry once you're reconnected" instead of the message being dropped into the composer.
- **Login + CreateAccount clear stale peer-connection state** — `clear_session_state_on_identity_switch` drops room/space membership, typing flags, mute/deafen/timeout, whisper targets, and the blocked-by cache before the new `user_id` is written. Stops leftover state from a prior identity from spilling into a fresh login on the same WebSocket.
- **Server periodically WAL-checkpoints + purges expired timeouts** — A 5-minute interval task runs `PRAGMA wal_checkpoint(PASSIVE)` (yields to in-flight writers) and prunes `space_timeouts` rows whose `until_epoch` has already passed. Keeps the WAL bounded and stops the timeout table from growing unbounded over time.
- **Three more SQLite indices for hot lookups** — `idx_messages_attachment` (cascade-GC check on delete), `idx_messages_reply_parent` (thread BFS walker), and `idx_messages_channel_pinned` (pinned-message listings). All partial indices to keep them tiny.

### Custom roles + granular permissions: foundation (wave 13)
- **`shared_types::Permissions`** newtype over `u64` with 25 flag constants (CREATE_INVITE, KICK_MEMBERS, BAN_MEMBERS, TIMEOUT_MEMBERS, MUTE_MEMBERS, DEAFEN_MEMBERS, MOVE_MEMBERS, MANAGE_NICKNAMES, MANAGE_CHANNELS, MANAGE_ROLES, MANAGE_SPACE, MANAGE_MESSAGES, MANAGE_EVENTS, MANAGE_AUTOMOD, VIEW_AUDIT_LOG, VIEW_CHANNEL, SEND_MESSAGES, SEND_VOICE_NOTES, ATTACH_FILES, ADD_REACTIONS, USE_EXTERNAL_EMOJI, MENTION_EVERYONE, CONNECT, SPEAK, USE_VOICE_ACTIVITY, PRIORITY_SPEAKER, START_RECORDING, STOP_RECORDING) plus `ADMINISTRATOR` (bit 62) and synthetic `OWNER_BYPASS` (bit 63, in-memory only). `Permissions::has(flag)` short-circuits under ADMINISTRATOR or OWNER_BYPASS — matches Discord's bypass semantics. Sparse layout (bits 0-15 space, 16-31 text-channel, 32-47 voice-channel) reserves room for per-channel overrides later. Plus `LEGACY_MEMBER_DEFAULTS` / `LEGACY_MODERATOR_BUNDLE` / `LEGACY_ADMIN_BUNDLE` convenience constants used by the migration.
- **`RoleInfo` + `RoleAssignment`** shared types for catalog snapshots over the wire.
- **Two new SQLite tables** (`space_role_defs`, `space_role_members`) added via a **versioned migration** — first real migration to use the v0.12 framework. `space_role_defs` carries `(space_id, role_id, name, color, position, permissions, is_managed, is_default, created_at)`; `space_role_members` is the (space, role, user) join. Three new indices: `idx_space_role_defs_space`, `idx_space_role_defs_name` (unique per space), `idx_role_members_user`, `idx_role_members_role`.
- **Idempotent legacy synthesis** — `migrate_legacy_roles_to_v2()` runs after every migration ladder pass: for every space, seeds four managed roles (`@everyone`, `Member`, `Moderator`, `Admin`) with the bitmasks that match today's `can_*` helpers, and copies every `space_roles` row into `space_role_members`. Owner is intentionally NOT seeded — ownership stays computed from `spaces.owner_id` and `OWNER_BYPASS` is applied in-memory at check time only.
- **Persistence helpers** — `upsert_role_def`, `delete_role_def`, `load_role_defs`, `upsert_role_member`, `delete_role_member`, `clear_user_role_assignments`, `load_role_members`, and the bitmask-OR aggregator `load_user_effective_permissions(space_id, user_id) -> u64` (default-role bitmask OR'd with every explicitly assigned role).
- **Tests** — `migration_v2_synthesizes_legacy_roles_and_assigns_users` covers fresh-DB seeding, idempotent re-run, role-set correctness, and per-user effective bitmask values for owner / mod / member.
- **What's NOT in this wave (deferred)** — the handler-by-handler migration from `has_at_least(SpaceRole::X)` to `perms.has(Permissions::FLAG)`, the new protocol messages (CreateRole / UpdateRole / AssignRoleToMember / RoleListSnapshot / MemberRolesChanged), the per-Peer cached effective bitmask, and the role-management UI. The audit identified ~50 role-check sites across handlers; doing the sweep safely is its own focused wave. The foundation here lets that wave land incrementally without further schema churn.

### Misc small fixes (alongside wave 12)
- **Constant-time room password compare** — XOR-accumulate over equal-length byte slices instead of `!=`. Length itself is a coarse channel; bytewise comparison no longer is.
- **Login timing leveling** — the unknown-email path now runs a verify against a known-bad Argon2id hash so it takes ~the same wall time as the wrong-password path. Without this, timing alone revealed whether an email was registered.
- **TCP_NODELAY + SO_KEEPALIVE** on accepted WebSocket connections (via `socket2::TcpKeepalive`, idle 60s, interval 20s). Dead NAT entries surface within a couple of minutes instead of waiting for the next WS Ping cycle; signaling/audio writes hit the wire immediately.
- **Per-account login lockout** — 10 failed login attempts against a specific email within 15 minutes locks the account out for the rest of the window, even from a fresh IP. Pairs with the per-IP rate gate so a multi-IP attacker can't grind one account's password.
- **Mention autocomplete suggests `@everyone` + `@here`** at the top of the popup when the partial prefix matches (server already broadcast those when seen in content; now the composer surfaces them).

### Stats
- 11 workspace crates, **~377 unit + integration tests passing** (server suite at 63; integration `server_tests` suite at 112), zero compiler warnings on `cargo check`. Wave 13 first migration via the v0.12 versioned framework lifted DB to `schema_version = 2`. Clippy hygiene is tracked but a strict gate isn't enabled yet.
- Startup remains the v0.11 baseline (~300 ms after keychain hardening).

### Notes
- `SignalMessage::DeleteAccount` is now `DeleteAccount { current_password: String }`. Old clients that send it as a unit variant will get an unrecognized message; users must update to v0.12 to delete their account.
- `handle_channel_setting` now takes `db: &Db` so `Category` writes through. Other settings (UserLimit / SlowMode / Status / MinRole / AutoDelete) keep their existing per-setting persistence routes; this opens the door to consolidating them later.

## v0.11.0 — Attachments, Link Previews, Account Settings & Rich Presence

### New
- **File & image attachments** — Share files and images in chat. Native file picker (`rfd`) in the composer; image attachments preview **inline**, other files download to the user's Downloads folder (with path-traversal-safe filenames). Content is stored server-side in SQLite with a per-file size cap (8 MiB), a MIME allow-list, per-channel permission/slow-mode checks, and a server-wide storage cap; bytes travel base64 inside the control frame and are served on demand rather than broadcast to every recipient.
- **OpenGraph link previews** (opt-in) — With `VOXLINK_LINK_PREVIEWS` set, the server fetches a posted link's title/description in the background and pushes a `LinkPreviewReady` card to the channel (never delays the message). Strict SSRF guard (blocks private/loopback/link-local incl. cloud metadata, CGNAT, multicast, ULA, IPv4-mapped), redirects disabled, bounded time/size. **Off by default** — the server stays a pure relay unless enabled.
- **Unread "NEW" separator** — Chat renders a divider at the first unread message in a channel, with per-channel last-read tracking persisted in config.
- **Change email** — Account settings can change the account email (verifies current password; server enforces email uniqueness via the unique index).
- **Rich presence** (opt-in) — Broadcast the app you're using (e.g. "Playing Helldivers 2") to your spaces, sampled every 5 s. **Off by default** and gated by a per-app allowlist (empty list = nothing is ever shared), so you can never leak an app you didn't permit. A background task idles at ~0 cost until enabled and only spawns the OS query when presence is *both* enabled and connected. Reuses the existing activity-broadcast path (`SetActivity`/`ActivityChanged`) — **no new protocol or server changes**. Foreground detection on macOS (`osascript`) and Windows (`GetForegroundWindow`); Linux deferred (X11 too fragmented).

### Improved
- **Idle efficiency** — Ping-path Slint property writes (`ping_ms`/`udp_active`) now update only when a view that displays them is open (Room/System), eliminating idle property-diff churn every ~3 s.
- **Link cards** — Show a clean domain (e.g. `example.com`) instead of the raw URL.
- **WebSocket hardening** — Max message/frame size capped at 16 MiB (was tungstenite's 64 MiB default) to bound per-connection memory now that attachments produce larger messages; covers plain and TLS connections.

### Fixed
- **Voice-note delivery** — Voice notes were never delivered: the broadcast matched `member_ids` against each peer's account `user_id` instead of the peer-map key. Now uses the proven `s.peers.get(member_id)` lookup (matching text messages). Covered by a new integration test.

### Tests
- New unit tests: dependency-free base64 (RFC 4648 vectors), attachment validation + DB storage, image decode, unread-separator index logic, link-preview SSRF guard + OpenGraph parser, email-uniqueness, URL host parsing, rich-presence decision/debounce + allowlist parsing.
- New integration tests: attachment upload → broadcast → byte-exact download, voice-note delivery, change-email flow.
- **364 unit + integration tests passing; zero compiler warnings; no new clippy warnings.**

### Notes
- New server env vars: `VOXLINK_LINK_PREVIEWS` (enable link previews), `VOXLINK_MAX_ATTACHMENT_STORAGE_MB` (total attachment storage cap, default 256 MiB).
- New config fields: `rich_presence_enabled` (bool, default false), `rich_presence_allowlist` (`Vec<String>`, default empty).
- New dependencies: `rfd` (native file picker, desktop), `image` (attachment decode, UI), `ureq` (server, opt-in link-preview fetch). Rich presence adds **no** new dependency — foreground detection shells out to the OS (`osascript` / PowerShell).
- GUI rendering (attachment picker + inline images, unread divider, link cards, rich-presence toggle + allowlist field under Settings → Privacy) is compile-verified but should be visually QA'd on-device. Rich-presence foreground detection runs only on macOS/Windows and, on macOS, triggers a one-time Automation permission prompt on first sample.

## v0.9.1 — Security, Soundboard & UI Polish

### New
- **Soundboard** — Load WAV clips, play into voice chat from Settings panel. Auto-resample to 48kHz mono, lock-free mixing into capture stream. Max 16 clips, configurable keybinds.
- **Streamer mode** — Privacy toggle hides server IPs, invite codes, room codes, and email.
- **Channel reordering** — `ReorderChannels` protocol with admin+ permission, position field on channels.
- **Typing indicator timeout** — Client-side 5-second auto-clear prevents stale "X is typing..." indicators.
- **Voice pipeline integration tests** — Full-duplex audio tests (WS + UDP) with WAV recording.
- **Password masking** — Room password inputs use `InputType.password`.

### Improved
- **UI compaction** — 4px spacing scale, tighter buttons (40→36px), inputs (42→36px), TopBar (72→52px), Rail (272→240px), BottomNav (62→48px). ~15-20% more content visible.
- **Server relay optimization** — Merged whisper filtering into single state read lock. `udp_addr` and `whisper_targets` now use `std::sync::RwLock` for lock-free reads on 50fps hot path.
- **Config save worker** — Single dedicated background thread replaces 30+ `std::thread::spawn` calls for config writes.
- **Resource cleanup** — Periodic cleanup for `auth_attempts`, `join_failures`, `slow_mode_timestamps`, orphaned UDP sessions.
- **Tokio runtime** — Removed hardcoded 2-thread limit; server uses all CPU cores.
- **Clippy clean** — Zero production clippy warnings across entire workspace.
- **Flaky test fix** — Integration tests now reserve both TCP and UDP ports explicitly.
- **Accessibility** — `accessible-role` and `accessible-label` on all navigation buttons.
- **UX polish** — Improved reaction pills, "(edited)" italic badge, keybind help text, "Copied!" feedback, audio device error guidance, unified navigation labels.
- **Dead code removal** — Removed unused `AudioEngine::adapt_bitrate()`.

### Stats
- 351 tests passing, zero warnings
- 11 workspace crates

## v0.8.0 — Social Features & Account System

### New — 22 Features
- **Email account system** — Create account, login, logout, change password with salted SHA-256 hashing. Token rotation on login, persistent email in config.
- **Join/leave notification sounds** — Configurable rising/descending two-note chimes when peers enter or leave a room.
- **Channel categories** — Organize channels under bold section headers with `SetChannelCategory` support.
- **Unread indicators** — Badge counts on channels (mention count) and dot badges on space cards in home view.
- **Status presets** — Online, Idle, DND, Invisible. Invisible hides from member/friend lists. DND suppresses notifications.
- **Idle auto-status** — Automatically sets status to Idle after 5 minutes of keyboard inactivity, restores on input.
- **@Mentions with notifications** — Extract `@username` from messages, send `MentionNotification` to mentioned users with sound.
- **Block/unblock users** — Server-side `user_blocks` table, client-side message filtering, block/unblock SignalMessage variants.
- **Ban management UI** — `ListBans`, `UnbanMember`, ban list view in space settings.
- **Group DMs** — Multi-user direct message conversations with `group_conversations`, `group_members`, `group_messages` tables.
- **Invite expiration & max uses** — `invite_expires_at`, `invite_max_uses`, `invite_uses` columns with server-side validation.
- **Per-channel notification settings** — Override notifications per channel: all / mentions only / none.
- **Quick switcher (Ctrl+K)** — Fuzzy search overlay for channels and DMs with keyboard navigation.
- **User avatars** — Color-coded circles with initials, replacing inline rendering across all views.
- **Message threads** — Reply chains via `GetThread` / `ThreadMessages`, leveraging existing `reply_to_message_id`.
- **Volume ducking** — Auto-lower non-speaking peers when someone is talking. Configurable amount and threshold.
- **File attachments** — `attachments` table with 1MB cap, attachment metadata on `TextMessageData`.
- **Soundboard** — `SoundboardClip` with pre-decoded WAV samples, mixed into capture stream.
- **Server nicknames** — `space_nicknames` table, `SetNickname` / `NicknameChanged` protocol.
- **Message forwarding** — `ForwardMessage` copies messages between channels with "Forwarded from" header.
- **Spoiler tags** — `||text||` syntax detected in `render_markdown()`.
- **Compact chat density** — Toggle for reduced padding/font in chat messages.

### Improved
- **Performance** — Idle detection eliminated 40 heap allocs/sec; volume ducking uses single-pass atomic caching; ring buffer `peek_energy` uses contiguous fast path.
- **Test coverage** — 338 tests across all crates (up from 316).
- **Installer** — `build-portable.ps1` now reads version dynamically from Cargo.toml. `voxlink.iss` bumped to 0.8.0.

### New DB Tables
- `user_blocks`, `group_conversations`, `group_members`, `group_messages`, `attachments`, `space_nicknames`

### New Config Fields
- `join_leave_sounds`, `show_spoilers`, `compact_chat`, `blocked_users`, `status_preset`, `idle_timeout_mins`, `channel_notification_overrides`, `ducking_amount`, `ducking_threshold`, `soundboard_clips`, `account_email`

### New Dependencies
- `sha2 0.10` — Password hashing for account system

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

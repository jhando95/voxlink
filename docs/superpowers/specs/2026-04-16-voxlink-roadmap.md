# Voxlink Roadmap — April 2026

**Status:** Supersedes `docs/roadmap-2026-03.md` (which referenced v0.9.0 as next milestone). **Current shipping version: v0.12.0** — see `CHANGELOG.md` for the v0.11 → v0.12 deltas. The audit-driven sweep (waves 1–9) landed the entire P0/P1 short-list plus Group DMs end-to-end, server-side read state, the limits framework, and a perf pass over the audio relay + SQLite hot paths.

## Goal

Grow Voxlink into a Discord-class platform while preserving its defining trait: **significantly lower resource usage than Electron-based competitors**. Every feature must justify its CPU, RAM, and bandwidth cost against the efficiency identity in `CLAUDE.md`.

## Guiding principles

1. **Efficiency is a feature.** Every release has an explicit efficiency budget. New features must not regress idle CPU, idle RAM, or cold-start time beyond stated caps.
2. **Native over web-style.** No Electron, no heavy runtimes, no WebView-based rendering. Hardware acceleration where the OS provides it.
3. **Opt-in over opt-out.** Heavy features (video, AI transcription, rich presence) are off by default. Users pay the cost only when they want the benefit.
4. **Server authority, client responsiveness.** Permissions, rate limits, and validation live on the server. UI is optimistic where safe.
5. **Compileable progress.** Small milestones, zero-warnings, passing tests at every commit.

## Current posture (v0.12.0)

**Done:** voice (rooms/spaces/channels/PTT/ducking/soundboard/priority speaker/whisper), social (friends/DMs/group DMs/mentions/status/idle/blocks/nicknames), chat (threads/reactions/pins/search/forwarding/attachments/spoilers/markdown), accounts (email/pw/tokens), moderation (roles/ban/kick/timeout/audit/ban-mgmt), organization (categories/Ctrl+K/unread/per-channel notifs/invites), desktop (screen share, hotkeys, themes, auto-update, tray).

**Infrastructure:** 11-crate Rust workspace, Slint UI, UDP+WS audio transport, SQLite WAL, Oracle Cloud deploy, ~350 tests, zero warnings.

## The gap vs Discord

| Capability | Voxlink | Discord | Priority |
|---|---|---|---|
| Video calling | ❌ | ✅ | High |
| Screen share | ✅ basic | ✅ rich | Medium (polish) |
| Custom emojis per space | ❌ | ✅ | High |
| Stickers / GIFs | ❌ | ✅ | Medium |
| Rich presence ("playing X") | ❌ | ✅ | Medium |
| Granular role permissions | ⚠ basic | ✅ full matrix | High |
| Channel permissions | ❌ | ✅ | High |
| Announcement channels | ❌ | ✅ | Medium |
| Forum channels | ❌ | ✅ | Low |
| Welcome / rules / onboarding | ❌ | ✅ | Medium |
| Webhooks / bot API | ❌ | ✅ | Low |
| Server discovery | ❌ | ✅ | Low |
| Voice-channel text chat | ❌ | ✅ | Medium |
| Live transcription | ❌ | ⚠ limited | Differentiator |
| E2E encryption | ❌ | ❌ | Differentiator |
| P2P direct audio | ❌ | ❌ | Differentiator |
| Plugin system | ❌ | ❌ | Differentiator |
| Federation | ❌ | ❌ | Differentiator (later) |

## Release plan

Four themed minor releases, in order. Each is scoped so it ships independently and is shippable in weeks, not months.

### v0.11 — Visual Communication **(shipped)**

**Theme:** See each other, react with personality, show what you're doing.
**Shipped subset:** attachments + inline image preview, OpenGraph link previews
(opt-in), rich presence (opt-in, allowlist-gated), unread separator, change-email
in account settings. Custom emojis + animated stickers were deferred — see
CHANGELOG.md v0.11 for details.

**Features:**
- Screen share polish (zoom, fit, pop-out, pause when hidden, separate transport lane)
- Custom emojis per space (upload, SQLite-backed, lazy-loaded)
- Animated stickers (one-shot, not infinite loops)
- Rich presence — "Playing Helldivers 2", "Listening to Spotify" (opt-in, 5s batch)
- Drag-and-drop file upload + image previews in chat
- Unread jump-to-first-new separator

**Explicitly deferred:** Full video calling. Starting with screen-share polish and presence first keeps v0.11 shippable. Video moves to v0.11.5 or v0.12.

**Efficiency budget:**
- Idle RAM regression ≤ 15 MB (emoji cache + presence state)
- Idle CPU regression ≤ 0.1%
- Emoji cache LRU capped at 50 MB on disk
- Rich presence polled at 5s intervals, not 1s
- Screen share uses separate UDP lane with latest-only queue

**Out of scope:** video tiles, GIF picker integration, animated avatar uploads.

---

### v0.12 — Correctness, Durability & Personal-Safety **(shipped)**

The originally-planned v0.12 ("Server Identity" with full roles + video calling)
deferred to v0.13+ when a workspace-wide audit surfaced a stack of P0/P1
correctness, durability, and personal-safety gaps that needed to land first.
Themed: **finish the v0.11 product surface before adding new ones**.

**Shipped:**
- Opus FEC decode bug; rustls 0.23 CryptoProvider; server SIGTERM; SQLite pragmas (FK + busy_timeout + synchronous=NORMAL); Argon2id with legacy SHA-256 auto-rehash; DeleteAccount password gate; ChangePassword/ChangeEmail/RevokeAllSessions rate-limit; Login/CreateAccount clear stale peer state.
- **Block/Unblock UI** (server was 100% paid for in v0.11; this exposes it).
- **Threads inline composer + transitive reply walk.**
- **Group DMs end-to-end** — creation flow with multi-select friend picker, group thread list, send-routing, quick-switcher coverage.
- **Persistence holes closed:** reactions, moderation timeouts, channel categories, status presets all survive restart now. DisplayNameChanged broadcasts to all surfaces. mention_count populated.
- **Server-side read state with multi-device sync** via new `user_read_state` table + `MarkChannelRead`/`ReadStateSnapshot` protocol.
- **Limits framework:** `max_spaces_per_user`, `max_channels_per_space`, `max_members_per_space`.
- **Perf:** audio relay shares one `Bytes` across recipients; `prepare_cached` everywhere; friend-snapshot N+1 → batched IN-query; periodic WAL checkpoint + expired-timeout sweep.
- **Forwarding fidelity:** attachments + link_url preserved. **Attachment GC:** orphan blobs cascade-delete with their last message.
- **BanList shows display names via JOIN.**
- **Quick switcher** spans saved spaces + channels + 1:1 DMs + group DMs.
- **Send-failure outbox** with tick-loop retry.

**Deferred to v0.13+:**
- Full role + permissions matrix (20+ granular permissions) and channel permission overrides per role
- Welcome screen + rules screen + onboarding flow
- Announcement channels (broadcast-only, subscribable across spaces)
- Server discovery opt-in (the `is_public` column exists; UX surface deferred)
- Space banner + icon + description upload
- **Video calling** (≤8 peers, HW-accelerated encode via VideoToolbox/NVENC/VAAPI)

---

### v0.13 — Intelligence Layer

**Theme:** Voxlink works *for* you. Differentiator vs Discord.

**Features:**
- On-device live voice transcription (whisper.cpp tiny, ~75 MB model, 0.5x realtime on M-series)
- In-call captions toggle (per-user, local only)
- Voice-channel text chat (text sidebar while in voice)
- Semantic message search via local embeddings (candle-rs, small model)
- Smart notifications (importance scoring — mentions > replies > channel activity)
- Forum channels (if demand)

**Efficiency budget:**
- All AI is **opt-in per user** — zero cost for users who don't enable it
- Whisper loads lazily on first caption toggle; unloads after 60s idle
- Embedding index builds on background thread, capped at 10k most recent messages per channel
- Idle CPU regression ≤ 0% (everything unloaded when unused)

**Out of scope:** cloud AI, translation, summarization (defer based on demand).

---

### v0.14 — Trust & Extension

**Theme:** Power users, privacy, ecosystem.

**Features:**
- E2E encryption for DMs (Signal-style double ratchet, or simpler X25519+ChaCha20-Poly1305 MVP)
- P2P direct audio for ≤4-peer rooms (STUN-assisted, server falls back if NAT hostile)
- WASM plugin sandbox (CPU-slice capped, memory ceiling, no net/file access without grant)
- Webhook receiver (incoming only, for integrations like GitHub notifications)
- Bot API (restricted, rate-limited, must declare intents)
- Federation MVP (cross-server DMs between voxlink servers only)

**Efficiency budget:**
- Plugins: 10ms CPU slice per frame, 64 MB RAM ceiling each, killed if exceeded
- E2E encryption adds ≤ 2 ms latency per DM
- P2P only engaged when NAT traversal succeeds (otherwise transparent fallback)

**Out of scope:** outbound webhooks, full Matrix-style federation, encrypted group DMs (defer).

---

## Efficiency invariants (apply across all releases)

These are non-negotiable. Any feature breaking these must be reworked or cut.

1. **Idle CPU on desktop (not in any call):** ≤ 0.5% on M-series, ≤ 1% on mid-range x86
2. **Idle RAM:** ≤ 120 MB (currently ~90 MB; reserve 30 MB for growth)
3. **Cold start to usable UI:** ≤ 1.5s
4. **Binary size:** ≤ 40 MB compressed (currently ~25 MB)
5. **No feature requires always-on background polling ≥ 1 Hz on idle clients**

## Release cadence

Target one minor release every ~6–8 weeks. Patch releases as needed for bugs. No deadline pressure — correctness > speed.

## Process

For each minor release:
1. Write a design spec in `docs/superpowers/specs/`
2. User review + approval
3. Write an implementation plan in `docs/superpowers/plans/`
4. Execute task-by-task with `subagent-driven-development` or `executing-plans`
5. Verify efficiency budgets before tagging
6. Deploy server (if server changes), push installers, publish release

## Open questions (to resolve per-release)

- **v0.12:** Do we want forum channels or is that too Reddit-like for Voxlink?
- **v0.13:** Which whisper model size? (tiny/base/small — tradeoff accuracy vs RAM/CPU)
- **v0.14:** E2E for group DMs now, or wait for ecosystem maturity?

These get answered in the per-release design docs, not here.

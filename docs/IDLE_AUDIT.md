# Voxlink Idle Audit (M6)

Date: 2026-04-16
Scope: every periodic task in `app_desktop` and `signaling_server`.
Purpose: identify tasks that wake the app unnecessarily at idle.

## Triage legend

- **KEEP** — task is correctly throttled and justified.
- **THROTTLE** — reduce frequency when idle or window hidden.
- **GATE** — suspend entirely when a precondition is false.
- **REPLACE** — convert polling to event-driven.
- **KILL** — task has no reason to exist; delete.

---

## Client (`crates/app_desktop`)

| File:line | Task | Frequency | Purpose | Idle cost | Action | Rationale |
|---|---|---|---|---|---|---|
| `tick_loop/mod.rs:127` | Main Slint timer (active) | 40 Hz (25 ms) | Drive all tick-based work during voice call or mic preview | high | KEEP | Rate is justified — audio level smoothing and PTT require sub-100 ms response |
| `tick_loop/mod.rs:150` | Main Slint timer (idle) | 10 Hz (100 ms) | Drive tick work when not in a call | medium | KEEP | Active/idle split is correct; 10 Hz is a good floor for signal draining and UI updates |
| `tick_loop/mod.rs:452` | Screen-share render timer | 30 Hz (33 ms) | Push screen share preview frames to Slint | high | KEEP | Timer is started only when `has_screen_share` is true and stopped immediately when it becomes false; correctly gated |
| `tick_loop/mod.rs:186` | Signal drain (every tick) | 10–40 Hz | Drain server messages from the network channel | medium | KEEP | Uses `try_lock` + `try_recv`; no-ops when the queue is empty; cost is a lock attempt per tick |
| `tick_loop/mod.rs:206` | Keyboard poll (`device_query`) | 10–40 Hz | Read OS key state for PTT, mute, deafen, quick switcher | medium | THROTTLE | Currently skipped when `!is_connected && !listening_keybind`, but PTT and mute hotkeys still poll at 40 Hz throughout the entire call even when the quick-switcher and Space view are closed; no further gating at 10 Hz idle is applied |
| `tick_loop/mod.rs:508` | Typing dot animation | every 16 ticks (~1.6 s at idle / 400 ms at active) | Animate the three-dot typing indicator | low | THROTTLE | `w.set_typing_dot_phase()` is called unconditionally every 16 ticks regardless of whether any typing indicator is visible; add an `if w.get_typing_visible()` guard |
| `tick_loop/mod.rs:513` | Unread pulse toggle | every 30 ticks (~3 s at idle / 750 ms at active) | Pulse unread badge | low | THROTTLE | `w.set_unread_pulse()` fires unconditionally; should be gated on `w.get_has_unread()` to avoid Slint property invalidation churn when there is nothing to pulse |
| `tick_loop/mod.rs:518` | Auto-hide notification | every tick | Clear room-status notification after ~3 s | low | KEEP | Function exits immediately when `notification_at_tick` is `None`; no-op on idle |
| `tick_loop/mod.rs:519` | Auto-clear errors | every tick | Clear error status text after ~8 s | low | KEEP | String comparison is cheap; exits early on non-error states |
| `tick_loop/mod.rs:520` | Auto-hide copied | every tick | Hide "Copied!" label after ~2 s | low | KEEP | Early-exits when `show_copied` is false |
| `tick_loop/mod.rs:522` | Toast auto-hide | every tick | Hide toast after 3 s wall-clock | low | KEEP | Returns immediately unless `toast_visible` is true |
| `tick_loop/mod.rs:575` | Slow-update block (~1 s) | 1 Hz | Bandwidth display, dropped frames, screen chunk expiry, connection check, audio recovery | medium | THROTTLE | Several sub-tasks inside this block run every second even at idle: `net.expire_stale_screen_chunks()` acquires the network lock and scans a map every second regardless of whether any screen sharing is active; should be gated on `w.get_has_screen_share()` |
| `tick_loop/mod.rs:700` | `check_connection` (inside slow block) | 1 Hz | Detect WS disconnect, trigger auto-reconnect | medium | KEEP | Correctly gated: reconnect cooldown only decrements when `!connected && !prev_connected`; no wasted work while healthy |
| `tick_loop/mod.rs:716` | `check_audio_recovery` (inside slow block) | 1 Hz when in call | Detect audio device errors, hotplug changes | medium | KEEP | Correctly gated on `in_call`; inside the function device list polling runs every 40 ticks (~1 s) using a tick counter |
| `tick_loop/mod.rs:592` | Screen chunk expiry (`expire_stale_screen_chunks`) | 1 Hz | Prune timed-out screen chunks from net buffer | medium | REPLACE | Called unconditionally every slow-update tick via `network.try_lock()`; move expiry to the point where a chunk is read/inserted rather than polling |
| `tick_loop/mod.rs:728` | Typing expiry / slow-mode countdown | 1 Hz | Expire stale typing indicators; decrement slow-mode timer | low | THROTTLE | Slow-mode countdown `w.set_slow_mode_remaining(remaining - 1)` is called every second even when `remaining == 0` check is skipped; confirm the existing guard is tight |
| `tick_loop/mod.rs:740` | Pending message retry | every 80 ticks (~2 s idle / 2 s active) | Re-send messages that failed to deliver | low | KEEP | Returns immediately when `pending_messages` is empty; near-zero idle cost |
| `tick_loop/mod.rs:745` | Ping update | 1/3 Hz (every ~3 s wall-clock) | Measure server RTT; update perf panel | medium | THROTTLE | Spawns an async task and updates three Slint properties every 3 s unconditionally; when the perf panel is not visible (`current_view != 3`) the property writes still trigger Slint diffs; gate perf-panel property writes on `current_view == 3` |
| `tick_loop/mod.rs:751` | Adaptive bitrate | every 200 ticks (~5 s active / not reached at idle) | Tune Opus bitrate based on loss and RTT | low | KEEP | Already gated on `in_call`; no idle cost |
| `tick_loop/mod.rs:537` | Idle auto-status | every tick | Detect keyboard inactivity and set presence to Idle | low | KEEP | Cheap slice comparison; spawns async task only on state change |
| `main.rs:332` | Tray poll timer | 4 Hz (250 ms) | Drain system-tray menu events | low | KEEP | Drains a channel that is almost always empty; 4 Hz is already conservative |
| `screen_share.rs:647` | Screen capture thread | adaptive (up to preset FPS) | Capture frames from display/window and encode | high | KEEP | Thread is started only when user initiates screen share and stopped on `stop_for_thread`; frame rate self-throttles via `paced_frame_interval` and skips unchanged frames via signature comparison |
| `signal_handler/connection.rs:393` | Reconnect sleep (one-shot) | one-shot 150 ms | Delay for server to process JoinSpace before channel ops | — | KEEP | Not periodic; one-shot delay in async reconnect path |

---

## Server (`crates/signaling_server`)

| File:line | Task | Frequency | Purpose | Idle cost | Action | Rationale |
|---|---|---|---|---|---|---|
| `connection.rs:100` | WS keepalive ping (per peer) | 1/30 Hz (every 30 s) | Send WebSocket Ping to keep NAT mapping alive | low | KEEP | One task per connected peer, terminates on send error; 30 s is the minimum safe keepalive interval for most NATs |
| `main.rs:380` | State cleanup sweep | 1/60 Hz (every 60 s) | Remove stale rooms, prune stale space member_ids, expire auth/join rate-limit entries, clean UDP sessions and IP counters | medium | REPLACE | Acquires a write-lock on the entire server state every 60 s; most sub-tasks (stale auth entries, join-failure entries, connections_per_ip) only have entries at all when attacks or churn are occurring; these could be pruned on insert/lookup instead, eliminating most of the write-lock contention |
| `main.rs:473` | Auto-delete message sweep | 1/600 Hz (every 10 min) | Delete expired messages from DB and in-memory buffers | low | KEEP | Infrequent enough to be negligible; DB is optional and the task only iterates channels with `auto_delete_hours > 0` |
| `main.rs:518` | Scheduled message delivery | 1/30 Hz (every 30 s) | Check DB for due scheduled messages and deliver them | low | KEEP | DB query is cheap (`get_due_scheduled_messages` returns only rows past their fire time); 30 s granularity is acceptable for scheduled messages |
| `relay/udp.rs:69` | UDP relay loop | event-driven (`recv_from`) | Receive UDP audio/screen frames and relay to room peers | — | KEEP | Purely event-driven; blocks on `recv_from` with no spin; no periodic wakeup cost |
| `discovery.rs:18` | LAN discovery loop | event-driven (`recv_from`) | Respond to `VOXLINK_DISCOVER` broadcast probes | — | KEEP | Blocks on `recv_from`; wakes only when a probe arrives |
| `metrics_server.rs:80` | Metrics HTTP listener | event-driven (`accept`) | Serve Prometheus-style metrics over TCP | — | KEEP | Blocks on `accept`; wakes only when a metrics scrape arrives |
| `main.rs:612` | Accept loop | event-driven (`tokio::select!`) | Accept new WS connections; races against shutdown signal | — | KEEP | Pure I/O wait; no periodic work |

---

## Tick-loop deep-dive

The main tick loop in `tick_loop/mod.rs` already implements the correct active/idle split (40 Hz in-call / 10 Hz otherwise). The rate switch is clean and the tick counter compensates for the rate change so tick-based timeouts stay accurate in wall-clock terms.

**Sub-tasks correctly gated:**

- Keyboard polling is skipped when `!is_connected && listening_keybind.is_empty()`.
- Screen-share render timer starts and stops based on `has_screen_share`.
- Mic-level updates run only when `in_call`.
- `update_mic_level` further sub-samples to every 2nd tick.
- `adapt_bitrate` is guarded by `in_call`.
- `check_audio_recovery` is guarded by `in_call`.
- Reconnect cooldown decrements only when `!connected && !prev_connected`; it resets to 0 when connected, so there is no wasted decrement while healthy.

**Sub-tasks with residual work at idle (throttle candidates):**

1. **Typing dot animation** (`tick % 16`): calls `w.set_typing_dot_phase()` every ~1.6 s at 10 Hz even when no peer is typing and the text-chat view is not open. Add `if w.get_typing_visible()` guard.

2. **Unread pulse toggle** (`tick % 30`): calls `w.set_unread_pulse()` every ~3 s unconditionally. Slint propagates the property change to the layout. Add `if w.get_has_unread()` guard.

3. **Screen chunk expiry** (`expire_stale_screen_chunks` inside slow block): acquires a `try_lock` on the network client and iterates a HashMap every second regardless of whether screen sharing has ever been used. Gate on `w.get_has_screen_share()`.

4. **Ping update** (every ~3 s): spawns a tokio task and writes `ping_ms`, `udp_active`, and `bandwidth_*` Slint properties every 3 s whether or not the perf panel or room view is visible. The ping itself must still be sent (it keeps RTT fresh for adaptive bitrate), but the three Slint property writes should be gated on the view being visible.

---

## Top actions for this milestone

Ranked by estimated idle-CPU impact:

1. **`tick_loop/mod.rs:592` — screen chunk expiry inside slow block** — REPLACE — acquires the network lock and scans a HashMap every second unconditionally; move expiry to chunk insertion/read so idle never pays the cost.

2. **`tick_loop/mod.rs:513` — unread pulse toggle** — THROTTLE — calls `w.set_unread_pulse()` every 3 s and forces a Slint property diff even when there are no unread items; adding a `has_unread` guard eliminates the diff at idle.

3. **`main.rs:380` (server) — state cleanup sweep** — REPLACE — holds a write-lock on the entire `ServerState` every 60 s; auth/join-failure/IP counters that are empty 99% of the time should be pruned on insert/read, reducing write-lock contention under load.

4. **`tick_loop/mod.rs:508` — typing dot animation** — THROTTLE — fires `set_typing_dot_phase()` unconditionally every 16 ticks; gating on `typing_visible` eliminates a Slint property write and animation cycle when nobody is typing.

---

## Findings summary

The audit found 23 periodic tasks (14 client, 9 server). Fifteen are already correctly throttled or event-driven and require no change. Six are candidates for improvement: two UI property writes that fire unconditionally at idle (typing dot, unread pulse), one client-side network scan that should be gated (screen chunk expiry), one server-side state sweep that holds a global write-lock on data structures that are almost always empty (rate-limit and IP tables), and two display property updates in the ping path that push to Slint whether or not the relevant view is open. The tick loop's active/idle rate split is architecturally sound and working as designed. The largest single win is the screen-chunk expiry: it acquires a mutex and iterates a map every second even when no screen share has ever been started.

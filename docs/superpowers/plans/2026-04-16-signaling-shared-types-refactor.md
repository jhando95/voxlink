# Signaling Server & Shared Types Refactor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split `signaling_server/src/main.rs` (4,238 lines) and `shared_types/src/lib.rs` (2,637 lines) into focused modules with zero behavior change.

**Architecture:** Pure file reorganization. Move code into new module files, add `pub(crate)` visibility as needed, keep a thin re-export surface so no external imports change. Every commit leaves the workspace green.

**Tech Stack:** Rust 1.94 workspace, tokio 1.50, tokio-tungstenite 0.26. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-04-16-signaling-shared-types-refactor-design.md`

**Workspace root:** `/Users/jph/Voiceapp/workspace_template`

---

## Ground rules for every task

1. **No behavior changes.** Move code only. No renames, no signature changes, no inlining, no "while I'm here" cleanup. If you spot a bug, leave it for a separate PR.
2. **Workspace stays green after every commit.** `cargo check --workspace` must succeed before committing.
3. **Zero warnings.** `cargo clippy --workspace --all-targets -- -D warnings` must be clean at the end of each Part.
4. **351 tests pass.** Run `cargo test --workspace` at the end of each Part.
5. **Commit per logical unit** — one new module file per commit, not one giant commit.
6. **Do not touch uncommitted files.** The working tree has untracked/modified files in `app_desktop`, `Cargo.lock`, etc. Those are the user's in-progress work. Only touch files listed in each task.
7. **Use `pub(crate)` by default** for functions moved out of `main.rs`. Never add `pub` (crate-external visibility) unless the item was already `pub`.
8. **`handlers/mod.rs` pattern:** when you add a handler file, add `pub mod <name>;` to `handlers/mod.rs`. No re-exports unless needed to avoid editing existing call sites.

## Per-move procedure (you'll repeat this ~22 times)

For every code move there are four checkpoints:

1. **Create the target file** with the moved functions + their imports. Make them `pub(crate)` if used elsewhere in the crate.
2. **Delete the original** from `main.rs` (or `lib.rs`).
3. **Wire it up:** add `mod <name>;` to `main.rs`/`lib.rs`. At call sites inside the same crate, prefix with `crate::<module>::` or import via `use crate::<module>::<item>;`. For `shared_types`, add `pub use <module>::*;` to `lib.rs` so external crates see no change.
4. **Verify:** `cargo check --workspace` passes. Then commit.

---

# PART A — signaling_server/main.rs

Start in `/Users/jph/Voiceapp/workspace_template`. The existing `handlers/` subdirectory uses `pub mod`; follow that convention.

---

## Task A0: Baseline verification

**Purpose:** Confirm starting state before touching anything.

- [ ] **Step 1: Verify clean build**

Run: `cd /Users/jph/Voiceapp/workspace_template && cargo check --workspace`
Expected: finishes with no errors.

- [ ] **Step 2: Verify clippy clean**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Verify tests pass**

Run: `cargo test --workspace`
Expected: 351 tests pass.

- [ ] **Step 4: Note the starting line count**

Run: `wc -l crates/signaling_server/src/main.rs crates/shared_types/src/lib.rs`
Expected: ~4238 and ~2637 respectively. Record actuals in your work log.

No commit.

---

## Task A1: Extract `tls.rs`

**Files:**
- Create: `crates/signaling_server/src/tls.rs`
- Modify: `crates/signaling_server/src/main.rs` (remove lines ~62–170 + imports)
- Modify: `crates/signaling_server/src/types.rs:11` (update `use crate::ServerStream` → `use crate::tls::ServerStream`)

**What moves:**
- `enum ServerStream` + `impl AsyncRead for ServerStream` + `impl AsyncWrite for ServerStream` + `impl Unpin for ServerStream`
- `fn bind_requires_tls`
- `fn allow_insecure_public_bind`
- `fn load_tls_config`

- [ ] **Step 1: Create `crates/signaling_server/src/tls.rs`**

Move the four items above into this file. Add necessary imports at the top:

```rust
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
```

Mark `ServerStream` and each function `pub(crate)`. Mark the enum's variants unchanged (they inherit item visibility for use inside the crate).

- [ ] **Step 2: Add `mod tls;` to `main.rs`**

Add `mod tls;` near the top of `main.rs` alongside the other `mod` declarations. Add `pub(crate) use tls::ServerStream;` so `types.rs` sees it via `crate::ServerStream` (or alternatively update `types.rs` in this commit).

- [ ] **Step 3: Remove the moved code from `main.rs`**

Delete the four items from their original location. Remove any imports that were only used by them.

- [ ] **Step 4: Update call sites in `main.rs`**

Where `bind_requires_tls`, `allow_insecure_public_bind`, or `load_tls_config` are called, prefix with `tls::` (or add `use crate::tls::{...};` at the top of `main.rs`).

- [ ] **Step 5: Verify build**

Run: `cargo check --workspace`
Expected: compiles. If `types.rs` fails with `cannot find type ServerStream in crate root`, either add the `pub(crate) use tls::ServerStream;` re-export in `main.rs` OR change `types.rs:11` from `use crate::ServerStream` to `use crate::tls::ServerStream`. Prefer the latter.

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/tls.rs crates/signaling_server/src/main.rs crates/signaling_server/src/types.rs
git commit -m "refactor(signaling_server): extract tls.rs from main.rs"
```

---

## Task A2: Extract `metrics_server.rs`

**Files:**
- Create: `crates/signaling_server/src/metrics_server.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `struct ServerMetrics` and `impl Default for ServerMetrics`
- `async fn run_metrics_server`
- `async fn render_metrics`

Note: if `type Metrics = Arc<ServerMetrics>` (or similar) is defined in `main.rs` or `types.rs`, leave the type alias where it is; re-export from `metrics_server` if needed.

- [ ] **Step 1: Grep for `ServerMetrics` and `Metrics` usage**

Run: `grep -n "ServerMetrics\|type Metrics" crates/signaling_server/src/*.rs`
Record which files reference these types — you may need to update imports in `dispatch`-ish code later.

- [ ] **Step 2: Create `crates/signaling_server/src/metrics_server.rs`**

Move the three items. Imports to include:

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use crate::types::State;
```

Make the struct and functions `pub(crate)`. Make struct fields `pub(crate)`.

- [ ] **Step 3: Add `mod metrics_server;` to `main.rs` and remove the moved code**

- [ ] **Step 4: Update `main.rs` call sites**

Prefix with `metrics_server::` or `use crate::metrics_server::{ServerMetrics, run_metrics_server};`.

- [ ] **Step 5: Verify build**

Run: `cargo check --workspace`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/metrics_server.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract metrics_server.rs from main.rs"
```

---

## Task A3: Extract `discovery.rs`

**Files:**
- Create: `crates/signaling_server/src/discovery.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn run_discovery`

- [ ] **Step 1: Create `crates/signaling_server/src/discovery.rs`**

Content template:

```rust
use tokio::net::UdpSocket;

pub(crate) async fn run_discovery(server_addr: String) {
    // ... body copied verbatim from main.rs ...
}
```

Copy the exact body from `main.rs`. Add any additional imports used inside.

- [ ] **Step 2: Add `mod discovery;` to `main.rs`; delete the original function**

- [ ] **Step 3: Update the call site in `main.rs`**

`tokio::spawn(run_discovery(...))` → `tokio::spawn(discovery::run_discovery(...))` (or import via `use`).

- [ ] **Step 4: Verify**

Run: `cargo check --workspace`

- [ ] **Step 5: Commit**

```bash
git add crates/signaling_server/src/discovery.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract discovery.rs from main.rs"
```

---

## Task A4: Extract `validation.rs`

**Files:**
- Create: `crates/signaling_server/src/validation.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `fn instant_to_ms`
- `fn atomic_rate_check`
- `fn chunked_screen_sequence_state`
- `async fn check_rate_limit`
- `fn validate_name`
- `fn validate_room_code`
- `fn validate_password`
- `fn now_epoch_secs`
- The `MAX_NAME_LEN` and `MAX_PASSWORD_LEN` constants (only if they are used only by these validators — check with grep first)

- [ ] **Step 1: Check constant usage**

Run: `grep -n "MAX_NAME_LEN\|MAX_PASSWORD_LEN" crates/signaling_server/src/*.rs crates/signaling_server/src/**/*.rs`

If constants are used outside validation, leave them in `main.rs`. Otherwise move them.

- [ ] **Step 2: Create `crates/signaling_server/src/validation.rs`**

Move the listed functions. Make each `pub(crate)`. Include necessary imports:

```rust
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;
use crate::types::State;
use crate::LIMITS;
```

Note: `LIMITS` is a `LazyLock<ServerLimits>` currently defined in `main.rs`. It must be accessible via `crate::LIMITS`; leave it in `main.rs` for now.

- [ ] **Step 3: Add `mod validation;` to `main.rs`; delete originals**

- [ ] **Step 4: Update call sites in `main.rs`**

Every call to `validate_name`, `validate_room_code`, etc. now needs `validation::`. Add `use crate::validation::{...};` at top of `main.rs`.

- [ ] **Step 5: Verify**

Run: `cargo check --workspace`

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/validation.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract validation.rs from main.rs"
```

---

## Task A5: Extract `relay/audio.rs`

**Files:**
- Create: `crates/signaling_server/src/relay/mod.rs`
- Create: `crates/signaling_server/src/relay/audio.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn relay_audio`
- `async fn relay_audio_udp`

- [ ] **Step 1: Create `crates/signaling_server/src/relay/mod.rs`**

```rust
pub mod audio;
```

- [ ] **Step 2: Create `crates/signaling_server/src/relay/audio.rs`**

Move `relay_audio` and `relay_audio_udp` with all their imports. Make both `pub(crate)`.

Imports likely needed:

```rust
use std::sync::Arc;
use tokio::net::UdpSocket;
use crate::types::{State, Peer};
use crate::metrics_server::ServerMetrics;
use crate::LIMITS;
```

(Check the body of each function for the full import list — these are the usual suspects.)

- [ ] **Step 3: Add `mod relay;` to `main.rs`; delete originals**

- [ ] **Step 4: Update call sites**

`relay_audio(...)` → `relay::audio::relay_audio(...)` (or import via `use`).

- [ ] **Step 5: Verify**

Run: `cargo check --workspace`

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/relay crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract relay/audio.rs from main.rs"
```

---

## Task A6: Extract `relay/screen.rs`

**Files:**
- Create: `crates/signaling_server/src/relay/screen.rs`
- Modify: `crates/signaling_server/src/relay/mod.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn prepare_screen_relay`
- `fn screen_chunk_is_plausible`
- `async fn send_screen_frame_to_peers`
- `async fn relay_screen`
- `async fn relay_screen_chunk`
- `async fn relay_screen_udp`

- [ ] **Step 1: Create `crates/signaling_server/src/relay/screen.rs`**

Move the six items. Make each `pub(crate)`.

- [ ] **Step 2: Add `pub mod screen;` to `relay/mod.rs`**

- [ ] **Step 3: Delete originals from `main.rs`**

- [ ] **Step 4: Update call sites in `main.rs`**

Prefix with `relay::screen::`.

- [ ] **Step 5: Verify**

Run: `cargo check --workspace`

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/relay/screen.rs crates/signaling_server/src/relay/mod.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract relay/screen.rs from main.rs"
```

---

## Task A7: Extract `relay/udp.rs`

**Files:**
- Create: `crates/signaling_server/src/relay/udp.rs`
- Modify: `crates/signaling_server/src/relay/mod.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn run_udp_relay`
- `async fn handle_request_udp`
- `fn hex_encode`

(Note: `handle_request_udp` is an RPC handler, but it belongs here because it's tightly coupled to UDP state. Keeping it with the rest of the UDP code avoids a circular dep.)

- [ ] **Step 1: Create `crates/signaling_server/src/relay/udp.rs`**

Move the three items as `pub(crate)`.

- [ ] **Step 2: Add `pub mod udp;` to `relay/mod.rs`**

- [ ] **Step 3: Delete originals from `main.rs`**

- [ ] **Step 4: Update call sites**

Includes any call from `dispatch`-equivalent code — check the `handle_signal` match for `RequestUdp` handling.

- [ ] **Step 5: Verify**

Run: `cargo check --workspace`

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/relay/udp.rs crates/signaling_server/src/relay/mod.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract relay/udp.rs from main.rs"
```

---

## Task A8: Extract `connection.rs`

**Files:**
- Create: `crates/signaling_server/src/connection.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn handle_connection`
- `async fn handle_disconnect`
- `async fn send_to`
- `async fn send_error`
- `async fn decrement_ip`

- [ ] **Step 1: Create `crates/signaling_server/src/connection.rs`**

Move the five items as `pub(crate)`. This is the largest extraction so far — be careful with imports. Likely:

```rust
use std::net::IpAddr;
use std::sync::Arc;
use futures_util::{SinkExt, StreamExt};
use shared_types::SignalMessage;
use tokio_tungstenite::tungstenite::Message;
use crate::types::{Peer, State};
use crate::tls::ServerStream;
use crate::metrics_server::ServerMetrics;
```

`handle_connection` calls `handle_signal` — but at this point `handle_signal` is still in `main.rs`. Reference it as `crate::handle_signal` for now; Task A9 will move it.

- [ ] **Step 2: Add `mod connection;` to `main.rs`; delete originals**

- [ ] **Step 3: Update call sites**

`handle_connection` is invoked from the accept loop in `main()`. Prefix with `connection::` or import.

- [ ] **Step 4: Verify**

Run: `cargo check --workspace`

- [ ] **Step 5: Commit**

```bash
git add crates/signaling_server/src/connection.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract connection.rs from main.rs"
```

---

## Task A9: Extract `dispatch.rs`

**Files:**
- Create: `crates/signaling_server/src/dispatch.rs`
- Modify: `crates/signaling_server/src/connection.rs` (update `crate::handle_signal` → `crate::dispatch::handle_signal`)
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn handle_signal` — the 618-line match block

This is the largest single move. `handle_signal` calls ~40 per-variant `handle_*` functions which are still in `main.rs` at this point; they will be moved in Tasks A10+. Keep the references as `crate::handle_*` for now.

- [ ] **Step 1: Create `crates/signaling_server/src/dispatch.rs`**

Copy the entire `handle_signal` function. Imports:

```rust
use std::sync::Arc;
use shared_types::SignalMessage;
use crate::types::{Peer, State, Db};
use crate::metrics_server::ServerMetrics;
use crate::validation::{validate_name, validate_room_code, validate_password};
use crate::relay::{audio, screen, udp};
use crate::handlers;
```

Mark `handle_signal` as `pub(crate)`.

- [ ] **Step 2: Add `mod dispatch;` to `main.rs`; delete the original**

- [ ] **Step 3: Update `connection.rs` reference**

Change `crate::handle_signal(...)` → `crate::dispatch::handle_signal(...)`.

- [ ] **Step 4: Verify**

Run: `cargo check --workspace`
Expected: compiles. The `crate::handle_*` references inside `dispatch.rs` still resolve because those handlers are still in `main.rs`. Rust's module system looks them up as crate-root items.

- [ ] **Step 5: Commit**

```bash
git add crates/signaling_server/src/dispatch.rs crates/signaling_server/src/connection.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract dispatch.rs from main.rs"
```

---

## Task A10: Extract `handlers/calls.rs`

**Files:**
- Create: `crates/signaling_server/src/handlers/calls.rs`
- Modify: `crates/signaling_server/src/handlers/mod.rs`
- Modify: `crates/signaling_server/src/dispatch.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn handle_call_user`
- `async fn handle_accept_call`
- `async fn handle_decline_call`

- [ ] **Step 1: Create `crates/signaling_server/src/handlers/calls.rs`**

Move the three functions as `pub(crate)`. Imports from existing `main.rs` uses.

- [ ] **Step 2: Add `pub mod calls;` to `handlers/mod.rs`**

- [ ] **Step 3: Delete originals from `main.rs`**

- [ ] **Step 4: Update `dispatch.rs`**

Change `crate::handle_call_user(...)` → `crate::handlers::calls::handle_call_user(...)` (and similar for the other two).

- [ ] **Step 5: Verify**

Run: `cargo check --workspace`

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/handlers/calls.rs crates/signaling_server/src/handlers/mod.rs crates/signaling_server/src/dispatch.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract handlers/calls.rs from main.rs"
```

---

## Task A11: Extract `handlers/events.rs`

**Files:**
- Create: `crates/signaling_server/src/handlers/events.rs`
- Modify: `crates/signaling_server/src/handlers/mod.rs`
- Modify: `crates/signaling_server/src/dispatch.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn handle_create_event`
- `async fn handle_delete_event`
- `async fn handle_toggle_event_interest`
- `async fn handle_list_events`

- [ ] **Step 1: Create `crates/signaling_server/src/handlers/events.rs`**

Move the four functions verbatim. Mark each `pub(crate)`. Copy their existing imports from `main.rs`.

- [ ] **Step 2: Add `pub mod events;` to `handlers/mod.rs`**

- [ ] **Step 3: Delete originals from `main.rs`**

- [ ] **Step 4: Update `dispatch.rs` call sites**

Change each `crate::handle_create_event(...)` → `crate::handlers::events::handle_create_event(...)` (and similarly for the other three). Or add `use crate::handlers::events::{...};` at the top of `dispatch.rs`.

- [ ] **Step 5: Verify**

Run: `cargo check --workspace`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/handlers/events.rs crates/signaling_server/src/handlers/mod.rs crates/signaling_server/src/dispatch.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract handlers/events.rs from main.rs"
```

---

## Task A12: Extract `handlers/scheduling.rs`

**Files:**
- Create: `crates/signaling_server/src/handlers/scheduling.rs`
- Modify: `crates/signaling_server/src/handlers/mod.rs`
- Modify: `crates/signaling_server/src/dispatch.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn handle_schedule_message`
- `async fn handle_cancel_scheduled_message`
- `async fn handle_set_welcome_message`

- [ ] **Step 1: Create `crates/signaling_server/src/handlers/scheduling.rs`**

Move the three functions. Mark each `pub(crate)`. Copy their imports from `main.rs`.

- [ ] **Step 2: Add `pub mod scheduling;` to `handlers/mod.rs`**

- [ ] **Step 3: Delete originals from `main.rs`**

- [ ] **Step 4: Update `dispatch.rs` call sites**

Prefix calls with `crate::handlers::scheduling::`.

- [ ] **Step 5: Verify**

Run: `cargo check --workspace`

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/handlers/scheduling.rs crates/signaling_server/src/handlers/mod.rs crates/signaling_server/src/dispatch.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract handlers/scheduling.rs from main.rs"
```

---

## Task A13: Extract `handlers/recording.rs`

**Files:**
- Create: `crates/signaling_server/src/handlers/recording.rs`
- Modify: `crates/signaling_server/src/handlers/mod.rs`
- Modify: `crates/signaling_server/src/dispatch.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn handle_start_recording`
- `async fn handle_stop_recording`
- `async fn handle_send_voice_note`

- [ ] **Step 1: Create `crates/signaling_server/src/handlers/recording.rs`**

Move the three functions as `pub(crate)`.

- [ ] **Step 2: Add `pub mod recording;` to `handlers/mod.rs`**

- [ ] **Step 3: Delete originals from `main.rs`**

- [ ] **Step 4: Update `dispatch.rs` call sites**

Prefix with `crate::handlers::recording::`.

- [ ] **Step 5: Verify**

Run: `cargo check --workspace`

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/handlers/recording.rs crates/signaling_server/src/handlers/mod.rs crates/signaling_server/src/dispatch.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract handlers/recording.rs from main.rs"
```

---

## Task A14: Extract `handlers/account.rs`

**Files:**
- Create: `crates/signaling_server/src/handlers/account.rs`
- Modify: `crates/signaling_server/src/handlers/mod.rs`
- Modify: `crates/signaling_server/src/dispatch.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn handle_set_display_name`
- `async fn handle_delete_account`
- `async fn handle_set_user_status`
- `async fn handle_set_profile`

- [ ] **Step 1: Create `crates/signaling_server/src/handlers/account.rs`**

Move the four functions as `pub(crate)`.

- [ ] **Step 2: Add `pub mod account;` to `handlers/mod.rs`**

- [ ] **Step 3: Delete originals from `main.rs`**

- [ ] **Step 4: Update `dispatch.rs` call sites**

Prefix with `crate::handlers::account::`.

- [ ] **Step 5: Verify**

Run: `cargo check --workspace`

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/handlers/account.rs crates/signaling_server/src/handlers/mod.rs crates/signaling_server/src/dispatch.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract handlers/account.rs from main.rs"
```

---

## Task A15: Extract `handlers/channel_settings.rs`

**Files:**
- Create: `crates/signaling_server/src/handlers/channel_settings.rs`
- Modify: `crates/signaling_server/src/handlers/mod.rs`
- Modify: `crates/signaling_server/src/dispatch.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn handle_set_channel_topic`
- `async fn handle_channel_setting`
- `async fn handle_set_priority_speaker`
- `async fn handle_set_space_public`
- `async fn handle_browse_public_spaces`

- [ ] **Step 1: Create `crates/signaling_server/src/handlers/channel_settings.rs`**

Move the five functions as `pub(crate)`.

- [ ] **Step 2: Add `pub mod channel_settings;` to `handlers/mod.rs`**

- [ ] **Step 3: Delete originals from `main.rs`**

- [ ] **Step 4: Update `dispatch.rs` call sites**

Prefix with `crate::handlers::channel_settings::`.

- [ ] **Step 5: Verify**

Run: `cargo check --workspace`

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/handlers/channel_settings.rs crates/signaling_server/src/handlers/mod.rs crates/signaling_server/src/dispatch.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract handlers/channel_settings.rs from main.rs"
```

---

## Task A16: Extract `handlers/timeouts.rs`

**Files:**
- Create: `crates/signaling_server/src/handlers/timeouts.rs`
- Modify: `crates/signaling_server/src/handlers/mod.rs`
- Modify: `crates/signaling_server/src/dispatch.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn handle_timeout_member`

- [ ] **Step 1: Create `crates/signaling_server/src/handlers/timeouts.rs`**

Move the function as `pub(crate)`.

- [ ] **Step 2: Add `pub mod timeouts;` to `handlers/mod.rs`**

- [ ] **Step 3: Delete original from `main.rs`**

- [ ] **Step 4: Update `dispatch.rs` call site**

Prefix with `crate::handlers::timeouts::`.

- [ ] **Step 5: Verify**

Run: `cargo check --workspace`

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/handlers/timeouts.rs crates/signaling_server/src/handlers/mod.rs crates/signaling_server/src/dispatch.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract handlers/timeouts.rs from main.rs"
```

---

## Task A17: Extract `handlers/whisper.rs`

**Files:**
- Create: `crates/signaling_server/src/handlers/whisper.rs`
- Modify: `crates/signaling_server/src/handlers/mod.rs`
- Modify: `crates/signaling_server/src/dispatch.rs`
- Modify: `crates/signaling_server/src/main.rs`

**What moves:**
- `async fn handle_whisper_to`
- `async fn handle_whisper_stopped`

- [ ] **Step 1: Create `crates/signaling_server/src/handlers/whisper.rs`**

Move the two functions as `pub(crate)`.

- [ ] **Step 2: Add `pub mod whisper;` to `handlers/mod.rs`**

- [ ] **Step 3: Delete originals from `main.rs`**

- [ ] **Step 4: Update `dispatch.rs` call sites**

Prefix with `crate::handlers::whisper::`.

- [ ] **Step 5: Verify**

Run: `cargo check --workspace`

- [ ] **Step 6: Commit**

```bash
git add crates/signaling_server/src/handlers/whisper.rs crates/signaling_server/src/handlers/mod.rs crates/signaling_server/src/dispatch.rs crates/signaling_server/src/main.rs
git commit -m "refactor(signaling_server): extract handlers/whisper.rs from main.rs"
```

---

## Task A18: Part A verification

- [ ] **Step 1: Line count check**

Run: `wc -l crates/signaling_server/src/main.rs`
Expected: ≤ 250 lines. If larger, scan for stragglers that belong in one of the new modules.

- [ ] **Step 2: Full test suite**

Run: `cargo test --workspace`
Expected: all 351 tests pass.

- [ ] **Step 3: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 4: Integration test**

Run: `cargo test -p integration_tests`
Expected: all server integration tests pass.

No commit (verification only).

---

# PART B — shared_types/lib.rs

Now split `shared_types/src/lib.rs`. The `lib.rs` becomes a thin re-export surface so no consumer crate needs edits.

---

## Task B1: Baseline

- [ ] **Step 1: Verify green**

Run: `cargo check --workspace`
Expected: clean (after Part A).

- [ ] **Step 2: Record starting state**

Run: `wc -l crates/shared_types/src/lib.rs`
Expected: ~2637 lines.

No commit.

---

## Task B2: Extract `view.rs`

**Files:**
- Create: `crates/shared_types/src/view.rs`
- Modify: `crates/shared_types/src/lib.rs`

**What moves:**
- `pub enum AppView`
- `pub enum MicMode`
- `pub enum ConnectionState`
- `pub enum UserStatus`
- `pub enum SpaceRole` + its `impl`
- `pub enum ChannelType`
- `pub struct AppState`

- [ ] **Step 1: Create `crates/shared_types/src/view.rs`**

Move the seven items verbatim. Add imports:

```rust
use serde::{Deserialize, Serialize};
```

(And any other imports their derive macros or method bodies require.)

- [ ] **Step 2: Add `pub mod view;` and `pub use view::*;` to `lib.rs`**

Top of `lib.rs`:

```rust
pub mod view;
pub use view::*;
```

- [ ] **Step 3: Delete originals from `lib.rs`**

- [ ] **Step 4: Verify**

Run: `cargo check --workspace`
Expected: clean. External crates using `shared_types::AppView` still work because of the `pub use view::*;`.

- [ ] **Step 5: Commit**

```bash
git add crates/shared_types/src/view.rs crates/shared_types/src/lib.rs
git commit -m "refactor(shared_types): extract view.rs from lib.rs"
```

---

## Task B3: Extract `state.rs`

**Files:**
- Create: `crates/shared_types/src/state.rs`
- Modify: `crates/shared_types/src/lib.rs`

**What moves:**
- `pub struct Participant`
- `pub struct RoomState`
- `pub struct SpaceInfo`
- `pub struct ChannelInfo` (and its `#[serde(default = "...")]` helpers `default_voice_quality`, `default_search_limit`)
- `pub struct MemberInfo`
- `pub struct BanInfo`
- `pub struct SpaceAuditEntry`
- `pub struct SpaceState`
- `pub struct PendingMessage`
- `pub struct PerfSnapshot`
- `pub struct FriendPresence`
- `pub struct FriendRequest`
- `pub struct DirectMessageThread`
- `pub struct FavoriteFriend`

Note: `default_voice_quality` and `default_search_limit` are private helpers referenced by `#[serde(default = "…")]` attributes. They move with `ChannelInfo`.

- [ ] **Step 1: Create `crates/shared_types/src/state.rs`**

Move the 14 items + the two `default_*` helpers. They must be `pub(crate)` or private; serde's `#[serde(default = "path")]` allows private functions.

- [ ] **Step 2: Add `pub mod state;` and `pub use state::*;` to `lib.rs`**

- [ ] **Step 3: Delete originals**

- [ ] **Step 4: Verify**

Run: `cargo check --workspace`

- [ ] **Step 5: Commit**

```bash
git add crates/shared_types/src/state.rs crates/shared_types/src/lib.rs
git commit -m "refactor(shared_types): extract state.rs from lib.rs"
```

---

## Task B4: Extract `protocol.rs`

**Files:**
- Create: `crates/shared_types/src/protocol.rs`
- Modify: `crates/shared_types/src/lib.rs`

**What moves:**
- `pub enum SignalMessage` — all ~132 variants, ~850 lines

This is the single biggest move in the entire refactor. Be methodical.

- [ ] **Step 1: Create `crates/shared_types/src/protocol.rs`**

Imports at top:

```rust
use serde::{Deserialize, Serialize};
use crate::{
    state::{ChannelInfo, MemberInfo, BanInfo, SpaceAuditEntry, SpaceState, Participant, RoomState, SpaceInfo, PerfSnapshot, FriendPresence, FriendRequest, DirectMessageThread, FavoriteFriend},
    view::{UserStatus, ChannelType, SpaceRole, AppView, MicMode},
};
```

(Adjust the `use` list based on what variants actually reference. The compiler will tell you what's missing.)

Move the entire `SignalMessage` enum verbatim.

- [ ] **Step 2: Add `pub mod protocol;` and `pub use protocol::*;` to `lib.rs`**

- [ ] **Step 3: Delete the original from `lib.rs`**

- [ ] **Step 4: Verify**

Run: `cargo check --workspace`
Expected: clean. If there are unresolved names, they are likely additional types (from `message_data.rs` which hasn't been split yet, or screen chunk types). Fix by adding `use crate::*;` and then narrowing the imports after B5/B6 land.

- [ ] **Step 5: Commit**

```bash
git add crates/shared_types/src/protocol.rs crates/shared_types/src/lib.rs
git commit -m "refactor(shared_types): extract protocol.rs (SignalMessage enum) from lib.rs"
```

---

## Task B5: Extract `message_data.rs`

**Files:**
- Create: `crates/shared_types/src/message_data.rs`
- Modify: `crates/shared_types/src/lib.rs`

**What moves:**
- `pub struct TextMessageData`
- `pub struct ReactionData`
- `pub struct ParticipantInfo`
- `pub struct SpaceSearchResult`
- `pub struct PublicSpaceInfo`
- `pub struct AutomodWord`
- `pub struct ScheduledEvent`

- [ ] **Step 1: Create `crates/shared_types/src/message_data.rs`**

Move the seven structs. Imports:

```rust
use serde::{Deserialize, Serialize};
```

- [ ] **Step 2: Add `pub mod message_data;` and `pub use message_data::*;` to `lib.rs`**

- [ ] **Step 3: Delete originals**

- [ ] **Step 4: Verify**

Run: `cargo check --workspace`
Expected: clean. If `protocol.rs` fails to find any of these, narrow its imports (it can either `use crate::*;` or `use crate::message_data::*;`).

- [ ] **Step 5: Commit**

```bash
git add crates/shared_types/src/message_data.rs crates/shared_types/src/lib.rs
git commit -m "refactor(shared_types): extract message_data.rs from lib.rs"
```

---

## Task B6: Extract `screen.rs`

**Files:**
- Create: `crates/shared_types/src/screen.rs`
- Modify: `crates/shared_types/src/lib.rs`

**What moves:**
- `pub struct ScreenChunkMetadata`
- `pub fn encode_screen_chunk_metadata`
- `pub fn decode_screen_chunk_metadata`
- `pub const SCREEN_CHUNK_METADATA_LEN`
- `pub const MAX_UDP_SCREEN_CHUNK_SIZE`
- `pub const MAX_SCREEN_FRAME_SIZE`
- `pub const MEDIA_PACKET_SCREEN`
- `pub const MEDIA_PACKET_SCREEN_CHUNK`

(Keep the screen-related constants with the screen codec.)

- [ ] **Step 1: Create `crates/shared_types/src/screen.rs`**

Move the struct, two functions, and five constants.

- [ ] **Step 2: Add `pub mod screen;` and `pub use screen::*;` to `lib.rs`**

- [ ] **Step 3: Delete originals**

- [ ] **Step 4: Verify**

Run: `cargo check --workspace`

- [ ] **Step 5: Commit**

```bash
git add crates/shared_types/src/screen.rs crates/shared_types/src/lib.rs
git commit -m "refactor(shared_types): extract screen.rs from lib.rs"
```

---

## Task B7: Extract `helpers.rs`

**Files:**
- Create: `crates/shared_types/src/helpers.rs`
- Modify: `crates/shared_types/src/lib.rs`

**What moves:**
- `pub fn voice_quality_bitrate`
- `pub fn voice_quality_kbps`
- `pub fn voice_quality_label`
- `pub fn extract_first_url`

(Note: `default_voice_quality` and `default_search_limit` already moved with `ChannelInfo` in Task B3.)

- [ ] **Step 1: Create `crates/shared_types/src/helpers.rs`**

Move the four functions.

- [ ] **Step 2: Add `pub mod helpers;` and `pub use helpers::*;` to `lib.rs`**

- [ ] **Step 3: Delete originals**

- [ ] **Step 4: Verify**

Run: `cargo check --workspace`

- [ ] **Step 5: Commit**

```bash
git add crates/shared_types/src/helpers.rs crates/shared_types/src/lib.rs
git commit -m "refactor(shared_types): extract helpers.rs from lib.rs"
```

---

## Task B8: Move tests module

**Files:**
- Create: `crates/shared_types/src/tests.rs`
- Modify: `crates/shared_types/src/lib.rs`

**What moves:**
- The `#[cfg(test)] mod tests { ... }` block at the bottom of `lib.rs`.

- [ ] **Step 1: Copy the test module body into `crates/shared_types/src/tests.rs`**

Keep the `#[cfg(test)]` attributes on individual test functions if needed. Simplest: make the whole file `#[cfg(test)]` at the top:

```rust
#![cfg(test)]

use super::*; // or explicit imports

// ... test functions ...
```

- [ ] **Step 2: In `lib.rs`, add at the bottom:**

```rust
#[cfg(test)]
mod tests;
```

- [ ] **Step 3: Delete the inline test module**

- [ ] **Step 4: Verify**

Run: `cargo test -p shared_types`
Expected: all shared_types tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/shared_types/src/tests.rs crates/shared_types/src/lib.rs
git commit -m "refactor(shared_types): move inline tests to tests.rs"
```

---

## Task B9: Audit remaining `lib.rs`

**Purpose:** After Tasks B2–B8, `lib.rs` should be near-empty. Scan for stragglers (constants, helper functions, or stray type aliases that weren't categorized) and decide where each belongs.

- [ ] **Step 1: Check line count**

Run: `wc -l crates/shared_types/src/lib.rs`
Expected: < 100 lines.

- [ ] **Step 2: Inspect what remains**

Run: `cat crates/shared_types/src/lib.rs`

The expected end state:

```rust
pub mod view;
pub mod state;
pub mod protocol;
pub mod message_data;
pub mod screen;
pub mod helpers;

pub use view::*;
pub use state::*;
pub use protocol::*;
pub use message_data::*;
pub use screen::*;
pub use helpers::*;

// Audio-pipeline constants used across crates
pub const SAMPLE_RATE: u32 = 48000;
pub const CHANNELS: u16 = 1;
pub const FRAME_SIZE: usize = 960;

// UDP media protocol constants not tied to screen
pub const MAX_AUDIO_FRAME_SIZE: usize = 4096;
pub const MAX_UDP_MEDIA_PAYLOAD_SIZE: usize = 60 * 1024;
pub const MEDIA_PACKET_AUDIO: u8 = 1;
pub const UDP_SESSION_TOKEN_LEN: usize = 8;
pub const UDP_DEFAULT_PORT_OFFSET: u16 = 1;
pub const UDP_KEEPALIVE: u8 = 0xFE;
pub const UDP_KEEPALIVE_INTERVAL_SECS: u64 = 15;

#[cfg(test)]
mod tests;
```

Keep audio constants in `lib.rs` because they are deeply cross-crate (`audio_core`, `signaling_server`, `shared_types` itself). Same for the non-screen UDP constants.

- [ ] **Step 3: If there are stragglers not covered above, decide:**
  - Matches audio pipeline → stay in `lib.rs` root.
  - Matches screen → move to `screen.rs`.
  - Matches protocol (used only by SignalMessage) → move to `protocol.rs`.

- [ ] **Step 4: Verify**

Run: `cargo check --workspace`

- [ ] **Step 5: Commit (only if changes were made)**

```bash
git add crates/shared_types/src/lib.rs
git commit -m "refactor(shared_types): clean up lib.rs stragglers"
```

If no stragglers, skip this commit.

---

## Task B10: Final verification

- [ ] **Step 1: Line counts**

Run: `wc -l crates/signaling_server/src/main.rs crates/shared_types/src/lib.rs`
Expected: `main.rs` ≤ 250, `lib.rs` ≤ 80.

- [ ] **Step 2: Check workspace**

Run: `cargo check --workspace`
Expected: clean.

- [ ] **Step 3: Full test suite**

Run: `cargo test --workspace`
Expected: all 351 tests pass.

- [ ] **Step 4: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 5: Spot-check the build binary still runs**

Run: `cargo build --release -p signaling_server`
Expected: produces binary. Optionally run briefly with `VOXLINK_BIND=127.0.0.1:19090 target/release/signaling_server` and confirm it accepts a WebSocket connection, then Ctrl+C.

- [ ] **Step 6: Update line-count memory note (optional)**

If you keep a per-session work log, record: `main.rs 4238→<N>`, `shared_types/lib.rs 2637→<N>`.

No commit (verification only).

---

## Task B11: Summary commit / tag (optional)

If desired, tag the end-of-milestone state:

- [ ] **Step 1:**

```bash
git tag -a m1-refactor-complete -m "Milestone 1 (signaling_server + shared_types refactor) complete"
```

Don't push the tag unless the user asks.

---

# Completion criteria

All of:
1. `wc -l crates/signaling_server/src/main.rs` ≤ 250.
2. `wc -l crates/shared_types/src/lib.rs` ≤ 80.
3. `cargo check --workspace` clean.
4. `cargo clippy --workspace --all-targets -- -D warnings` clean.
5. `cargo test --workspace` — all 351 tests pass.
6. No edits to any file outside `crates/signaling_server/src/` or `crates/shared_types/src/` (except possibly a handful of import paths in other crates if `pub use` re-exports didn't cover something — but for shared_types the `pub use *;` pattern should make this unnecessary).

# If something goes wrong

- **Build fails after a move:** most likely an import is missing or `pub(crate)` should be `pub`. Fix before committing — never commit red.
- **Import path issue in external crate:** e.g. `app_desktop` can't find `shared_types::SomeType`. Fix by adding/adjusting the `pub use` in `shared_types/src/lib.rs`. Do not edit the consumer.
- **Test fails:** the refactor accidentally introduced a behavior change. Revert the last commit and redo the move more carefully. The 351 tests are the regression harness.
- **Clippy warning appears:** the move exposed a lint that was previously suppressed by the giant file. Fix the lint (usually unused import or `use` that is now wrong scope). Never `#[allow]` without the user's OK.

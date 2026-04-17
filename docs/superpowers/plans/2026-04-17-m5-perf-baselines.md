# M5 — Performance Baselines & Regression Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add criterion benches for M1/M3 hot paths, write numeric performance baselines to `docs/PERFORMANCE_TARGETS.md`, and provide shell scripts to record a baseline and check for regressions.

**Architecture:** Bench-only. No runtime code changes. Two new bench files (`crates/signaling_server/benches/hot_path.rs`, `crates/shared_types/benches/protocol.rs`) following the existing `audio_core` bench pattern. Two new shell scripts under `scripts/`. `PERFORMANCE_TARGETS.md` replaced with a document that contains observed ns/op values from a fresh run on the dev machine.

**Tech Stack:** Rust 1.94, `criterion 0.5` (already a workspace member's dev-dep), plain bash for scripts.

**Spec:** `docs/superpowers/specs/2026-04-17-m5-perf-baselines-design.md`

**Workspace root:** `/Users/jph/Voiceapp/workspace_template`

**Branch:** `main` (M1+M2+M3 already merged). Start a fresh feature branch `feat/m5-perf-baselines` before making any edits.

---

## Ground rules

1. **Zero runtime code changes.** Only files under `benches/`, `scripts/`, `Cargo.toml`, and `docs/`.
2. **Workspace stays green.** `cargo check --workspace` passes before each commit.
3. **No new clippy warnings.** Baseline after M3 is 62 workspace warnings; must not exceed.
4. **Existing tests keep passing.** Known-flaky integration tests (`live_stress_*`, `test_create_space`, `test_audio_after_leave_room`, `test_channel_audio_relay`, `test_authenticate_invalid_token_creates_new`) are pre-existing — skip in verification.
5. **Fresh machine measurements.** The target numbers in `PERFORMANCE_TARGETS.md` come from a real `cargo bench` run on the dev machine. Not placeholders.

---

## Task 0: Branch + baseline verification

**Purpose:** Start from a known-good state on a feature branch.

- [ ] **Step 1: Verify working tree is clean**

```
cd /Users/jph/Voiceapp/workspace_template && git status --short
```
Expected: empty output (nothing modified, nothing untracked that matters).

If there are modifications, commit them or stash before proceeding. Do not start M5 on top of uncommitted work.

- [ ] **Step 2: Create feature branch**

```
git checkout -b feat/m5-perf-baselines
```

- [ ] **Step 3: Verify clean build**

```
cargo check --workspace
```
Expected: clean.

- [ ] **Step 4: Record starting clippy count**

```
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: `62`. If different, investigate before proceeding.

No commit.

---

## Task 1: `shared_types` protocol benches

**Files:**
- Modify: `crates/shared_types/Cargo.toml`
- Create: `crates/shared_types/benches/protocol.rs`

**What this adds:** Two criterion benches exercising `SignalMessage::variant_index()` on both a struct variant and a unit variant. Validates the "zero-cost dispatch" claim from M3.

- [ ] **Step 1: Add criterion dev-dep + bench entry to `crates/shared_types/Cargo.toml`**

Open the file. Find `[dev-dependencies]` (currently contains just `serde_json = { workspace = true }`). Add `criterion = "0.5"` so the section becomes:

```toml
[dev-dependencies]
serde_json = { workspace = true }
criterion = "0.5"
```

At the bottom of the file, append:

```toml
[[bench]]
name = "protocol"
harness = false
```

The `harness = false` line is required by criterion — it disables Rust's default `#[bench]` harness so criterion can run its own.

- [ ] **Step 2: Create `crates/shared_types/benches/protocol.rs`**

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use shared_types::SignalMessage;

/// Benchmark the jump-table match that indexes a struct variant.
fn bench_variant_index_struct(c: &mut Criterion) {
    let msg = SignalMessage::CreateRoom {
        user_name: "alice".into(),
        password: None,
    };
    c.bench_function("signal_message_variant_index_struct", |b| {
        b.iter(|| black_box(&msg).variant_index())
    });
}

/// Benchmark the same function on a unit variant (different match arm shape).
fn bench_variant_index_unit(c: &mut Criterion) {
    let msg = SignalMessage::LeaveRoom;
    c.bench_function("signal_message_variant_index_unit", |b| {
        b.iter(|| black_box(&msg).variant_index())
    });
}

criterion_group!(benches, bench_variant_index_struct, bench_variant_index_unit);
criterion_main!(benches);
```

- [ ] **Step 3: Build the benches**

```
cd /Users/jph/Voiceapp/workspace_template
cargo build --benches -p shared_types
```
Expected: clean build. `target/debug/deps/protocol-<hash>` binary exists.

If you get a compile error because `CreateRoom`'s shape has additional required fields, adjust the construction — the exact variant doesn't matter, pick any valid struct variant from `SignalMessage`. `LeaveRoom` should be a unit variant; if it isn't, substitute any unit variant (grep for `^    [A-Z][A-Za-z0-9]+,$` in `crates/shared_types/src/protocol.rs` to list unit variants).

- [ ] **Step 4: Run the benches once to confirm they work**

```
cargo bench -p shared_types
```
Expected: criterion prints output like:
```
signal_message_variant_index_struct
                        time:   [2.5 ns 2.7 ns 2.9 ns]
signal_message_variant_index_unit
                        time:   [2.4 ns 2.6 ns 2.8 ns]
```
Numbers will vary by machine; should be in the single-digit-nanosecond range.

**Record the observed numbers.** You'll paste them into `PERFORMANCE_TARGETS.md` in Task 4.

- [ ] **Step 5: Verify workspace still builds and clippy is clean**

```
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: check clean, clippy ≤ 62.

- [ ] **Step 6: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add crates/shared_types/Cargo.toml crates/shared_types/benches/protocol.rs
git commit -m "bench(shared_types): variant_index benches (struct + unit variants)"
```

---

## Task 2: `signaling_server` hot-path benches

**Files:**
- Modify: `crates/signaling_server/Cargo.toml`
- Create: `crates/signaling_server/benches/hot_path.rs`

**What this adds:** Five criterion benches covering `Histogram::observe`, `SignalMessage` serde round trip, and `decode_screen_chunk_metadata`. Validates the "under 1 µs histogram observe" claim from M3.

- [ ] **Step 1: Add criterion dev-dep + bench entry to `crates/signaling_server/Cargo.toml`**

Open the file. Find `[dev-dependencies]` (currently contains `tempfile = "3"` and `rcgen = "0.13"`). Add `criterion = "0.5"`:

```toml
[dev-dependencies]
tempfile = "3"
rcgen = "0.13"
criterion = "0.5"
```

At the bottom of the file, append:

```toml
[[bench]]
name = "hot_path"
harness = false
```

- [ ] **Step 2: Expose the `Histogram` type to bench code**

The `Histogram` type is `pub(crate)` inside the server binary crate. Benches are external — they can't reach crate-private items.

Check the current visibility:
```
grep -n "pub(crate) struct Histogram" crates/signaling_server/src/histogram.rs
```

To let benches use it without loosening runtime visibility, add a small `#[cfg(any(test, feature = "bench"))]`-gated public re-export.

Open `crates/signaling_server/src/histogram.rs`. Change:
```rust
pub(crate) struct Histogram {
```
to:
```rust
pub struct Histogram {
```

Also change the two `pub(crate)` methods (`new`, `observe`, `render`) to `pub`. This is a necessary visibility relaxation for bench access. The crate type is a binary (`signaling_server`) — external users don't consume this type. Flag this trade-off in the commit message.

Also change the module declaration in `crates/signaling_server/src/main.rs`:
```
grep -n "^mod histogram" crates/signaling_server/src/main.rs
```

Should currently be `mod histogram;`. Change it to `pub mod histogram;` so benches can path-reference as `signaling_server::histogram::Histogram`.

Note: the server binary crate doesn't have a `lib.rs` today — it's binary-only. Criterion benches need a library target. Check:

```
ls crates/signaling_server/src/lib.rs 2>/dev/null && echo "lib exists" || echo "no lib"
```

- [ ] **Step 3: If there's no lib target, create a minimal one**

If step 2 showed "no lib", we need one. Without a lib, `benches/*.rs` can't import anything from the server crate.

Create `crates/signaling_server/src/lib.rs`:

```rust
//! Library façade that re-exports internal modules for bench and test access.
//!
//! The server is a binary (see main.rs). This lib.rs exists only so criterion
//! benches and integration tests can see the public types they need to
//! benchmark or exercise.

pub mod histogram;
```

Open `crates/signaling_server/Cargo.toml`. Find the `[[bin]]` block (or the implicit `src/main.rs` target). Add an explicit library target alongside:

```toml
[lib]
name = "signaling_server"
path = "src/lib.rs"

[[bin]]
name = "signaling_server"
path = "src/main.rs"
```

If `[[bin]]` isn't already present, add both blocks. If there's an existing `name = "signaling_server"` under `[package]`, the `[lib]` and `[[bin]]` blocks can both use that name — cargo allows a lib and a bin with the same name.

Then in `src/main.rs`, where `mod histogram;` currently lives, the declaration stays but is now redundant because `lib.rs` also declares it. To keep the binary compilable without the library, keep the `mod histogram;` in main.rs. Cargo will compile `main.rs` as a binary and `lib.rs` as a library, each with its own module tree. That means histogram.rs is compiled twice in a clean build — acceptable, it's ~100 lines.

Actually — duplicating module compilation is wasteful. The cleaner pattern: make `main.rs` a thin wrapper that `use`s from its own library.

Better approach:

1. Create `crates/signaling_server/src/lib.rs` with all the `mod` declarations that currently live in `main.rs`:
   ```rust
   pub mod connection;
   pub mod dispatch;
   pub mod discovery;
   pub mod handlers;
   pub mod histogram;
   pub mod metrics_server;
   pub mod persistence;
   pub mod relay;
   pub mod tls;
   pub mod types;
   pub mod validation;

   pub use types::{
       max_channel_messages, ChannelMeta, Db, Peer, Room, ServerState, Space, State,
       MAX_SPACE_AUDIT_ENTRIES,
   };
   pub use tls::{allow_insecure_public_bind, bind_requires_tls, load_tls_config, ServerStream};
   pub use metrics_server::{run_metrics_server, ServerMetrics};
   pub use validation::{now_epoch_secs, validate_name, validate_password, validate_room_code};
   pub use relay::udp::{handle_request_udp, run_udp_relay};
   pub use connection::{decrement_ip, handle_connection, handle_disconnect, send_error, send_to};
   pub use dispatch::handle_signal;
   ```
   (Use the exact set of re-exports currently in main.rs — open `crates/signaling_server/src/main.rs` and copy the top of the file.)

2. Reduce `main.rs` to just import from the lib and run:
   ```rust
   use signaling_server::*;
   // ... rest of main.rs unchanged ...
   ```

   **This is a significant refactor.** If it feels too risky, take the smaller step: keep `main.rs` as-is, add a minimal `lib.rs` that only declares `pub mod histogram;`, accept the ~100-line duplicate compile. That's what Task 3's simpler plan does below.

   **Decision for this plan: go with the minimal `lib.rs`.** Smaller diff, zero-risk to runtime. Document in the commit.

- [ ] **Step 3-alt (simpler): Minimal lib.rs**

Actually do this. Replace Step 3 above with:

Create `crates/signaling_server/src/lib.rs`:

```rust
//! Minimal library surface for criterion benches.
//!
//! The server is a binary (see main.rs); this lib.rs exists only so that
//! criterion benches can reach internal types like `Histogram`. Runtime
//! code continues to use the `mod` tree in main.rs.

pub mod histogram;
```

In `crates/signaling_server/Cargo.toml`, add (if not present):

```toml
[lib]
name = "signaling_server"
path = "src/lib.rs"

[[bin]]
name = "signaling_server"
path = "src/main.rs"
```

Leave `main.rs`'s `mod histogram;` declaration unchanged — it continues to compile the module under the binary crate. The lib crate compiles it a second time; ~100 lines is negligible overhead.

- [ ] **Step 4: Verify the crate still builds**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
```
Expected: clean. Both `signaling_server` (bin) and `signaling_server` (lib) compile.

- [ ] **Step 5: Create `crates/signaling_server/benches/hot_path.rs`**

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use shared_types::{decode_screen_chunk_metadata, SignalMessage};
use signaling_server::histogram::Histogram;

/// Benchmark a single histogram observation: bucket scan + three atomic fetch_adds.
fn bench_histogram_observe(c: &mut Criterion) {
    let h = Histogram::new("bench", "bench help");
    c.bench_function("histogram_observe", |b| {
        b.iter(|| h.observe(black_box(0.003)))
    });
}

/// Deserialize a simple unit-variant SignalMessage.
fn bench_signal_from_slice_simple(c: &mut Criterion) {
    let data = br#""LeaveRoom""#;
    c.bench_function("signal_message_from_slice_simple", |b| {
        b.iter(|| {
            let _: SignalMessage =
                serde_json::from_slice(black_box(data)).expect("parse");
        })
    });
}

/// Deserialize a realistic struct-variant SignalMessage.
fn bench_signal_from_slice_complex(c: &mut Criterion) {
    let data = br#"{"CreateRoom":{"user_name":"alice","password":null}}"#;
    c.bench_function("signal_message_from_slice_complex", |b| {
        b.iter(|| {
            let _: SignalMessage =
                serde_json::from_slice(black_box(data)).expect("parse");
        })
    });
}

/// Serialize a realistic SignalMessage to JSON.
fn bench_signal_to_string(c: &mut Criterion) {
    let msg = SignalMessage::CreateRoom {
        user_name: "alice".into(),
        password: None,
    };
    c.bench_function("signal_message_to_string", |b| {
        b.iter(|| serde_json::to_string(black_box(&msg)).expect("serialize"))
    });
}

/// Decode the fixed-size UDP screen-chunk metadata header.
fn bench_decode_screen_chunk_metadata(c: &mut Criterion) {
    // 8-byte metadata header + tiny payload. Contents don't matter — the
    // bench measures parsing time, not correctness.
    let data = [0u8; 16];
    c.bench_function("decode_screen_chunk_metadata", |b| {
        b.iter(|| decode_screen_chunk_metadata(black_box(&data)))
    });
}

criterion_group!(
    benches,
    bench_histogram_observe,
    bench_signal_from_slice_simple,
    bench_signal_from_slice_complex,
    bench_signal_to_string,
    bench_decode_screen_chunk_metadata,
);
criterion_main!(benches);
```

If `CreateRoom`'s signature differs (extra required fields), adjust the literal. Check `grep -n "CreateRoom" crates/shared_types/src/protocol.rs` for the actual field list.

If `decode_screen_chunk_metadata`'s return type is unfamiliar, accept whatever it returns — the `black_box` wraps the call, no need to match on the result.

If the simple-variant JSON `br#""LeaveRoom""#` doesn't deserialize (serde may want `{"LeaveRoom":null}` for a unit variant, depending on the enum's attrs), try both:
- `br#""LeaveRoom""#` for default (externally tagged)
- `br#"{"LeaveRoom":null}"#` if tagged differently

Test-run the bench once in step 6; adjust the literal if serde rejects it.

- [ ] **Step 6: Build + run the benches**

```
cd /Users/jph/Voiceapp/workspace_template
cargo build --benches -p signaling_server
cargo bench -p signaling_server
```

Expected: five benches print results in single-digit-microsecond to sub-microsecond range. `histogram_observe` should be <200 ns.

**Record the observed numbers.** You'll paste them into `PERFORMANCE_TARGETS.md` in Task 4.

If any bench fails to parse JSON (the simple variant literal), fix the literal and re-run. If the `signaling_server` lib doesn't resolve `signaling_server::histogram::Histogram`, confirm `lib.rs` exists and has `pub mod histogram;`.

- [ ] **Step 7: Verify workspace clean**

```
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: check clean; clippy ≤ 62.

- [ ] **Step 8: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add crates/signaling_server/Cargo.toml crates/signaling_server/src/lib.rs crates/signaling_server/benches/hot_path.rs
git commit -m "bench(signaling_server): hot-path benches (histogram, signal serde, screen chunk)"
```

---

## Task 3: Bench scripts

**Files:**
- Create: `scripts/bench-record-baseline.sh`
- Create: `scripts/bench-check.sh`

**What this adds:** Two shell scripts. One records the current machine's bench times as the "main" baseline. The other runs benches against that saved baseline and flags regressions using criterion's built-in detection.

- [ ] **Step 1: Create `scripts/bench-record-baseline.sh`**

```bash
#!/usr/bin/env bash
# Record the current machine's bench times as the "main" baseline.
# Run this after an intentional perf change, or when setting up on a new machine.
#
# Usage:
#   ./scripts/bench-record-baseline.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo bench --workspace -- --save-baseline main
echo
echo "Baseline saved under target/criterion/<bench-name>/main/"
echo
echo "To check for regressions later, run: scripts/bench-check.sh"
```

- [ ] **Step 2: Create `scripts/bench-check.sh`**

```bash
#!/usr/bin/env bash
# Run benches against the saved "main" baseline and flag regressions >20%.
# Requires: a previously-saved "main" baseline (run bench-record-baseline.sh).
#
# Usage:
#   ./scripts/bench-check.sh
#
# Exit codes:
#   0 — no regressions
#   1 — one or more benches regressed >20% (criterion default threshold)
#   2 — no baseline found
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if [ ! -d target/criterion ]; then
    echo "No baseline found. Run scripts/bench-record-baseline.sh first."
    exit 2
fi

LOG=/tmp/voxlink-bench.log
cargo bench --workspace -- --baseline main 2>&1 | tee "$LOG"

# Criterion prints this phrase when a bench's mean time moved outside the
# noise threshold in the slower direction.
if grep -E "Performance has regressed\." "$LOG" > /dev/null; then
    echo
    echo "REGRESSION DETECTED:"
    grep -B2 -A2 "Performance has regressed\." "$LOG"
    echo
    echo "If this is an expected slowdown, re-record the baseline:"
    echo "  scripts/bench-record-baseline.sh"
    exit 1
fi

echo
echo "Benchmarks OK — no regressions."
```

- [ ] **Step 3: Make scripts executable**

```
cd /Users/jph/Voiceapp/workspace_template
chmod +x scripts/bench-record-baseline.sh scripts/bench-check.sh
```

- [ ] **Step 4: Syntax-check both scripts**

```
bash -n scripts/bench-record-baseline.sh
bash -n scripts/bench-check.sh
```
Expected: no syntax errors.

- [ ] **Step 5: Shellcheck if available (skip silently if not)**

```
command -v shellcheck >/dev/null 2>&1 && shellcheck scripts/bench-record-baseline.sh scripts/bench-check.sh || true
```
Expected: no errors if shellcheck is installed. Otherwise silent.

- [ ] **Step 6: Smoke-test `bench-record-baseline.sh`**

```
cd /Users/jph/Voiceapp/workspace_template
./scripts/bench-record-baseline.sh
```
Expected: runs all workspace benches, prints "Baseline saved…". Takes ~1-2 minutes.

After it finishes:
```
ls target/criterion/
```
Expected: subdirectories for each bench group with `main/` baseline folders inside.

- [ ] **Step 7: Smoke-test `bench-check.sh`**

```
./scripts/bench-check.sh
```
Expected: runs all benches, compares to the just-saved baseline, prints "Benchmarks OK — no regressions." Exits 0.

- [ ] **Step 8: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add scripts/bench-record-baseline.sh scripts/bench-check.sh
git commit -m "scripts: bench baseline recorder + regression gate"
```

---

## Task 4: Numeric PERFORMANCE_TARGETS.md

**Files:**
- Modify: `docs/PERFORMANCE_TARGETS.md`

**What this adds:** Replaces the current thin stub (one paragraph, single "Deployment" section heading — created as a byproduct during M2) with a full document carrying the parent repo's philosophical goals AND a numeric baselines table populated from a fresh bench run.

- [ ] **Step 1: Inspect the current file**

```
cat docs/PERFORMANCE_TARGETS.md 2>/dev/null || echo "file absent"
```

The file is very thin. You'll replace it wholesale.

- [ ] **Step 2: Capture the observed bench numbers from Tasks 1 and 2**

Re-run benches once more to collect clean numbers:

```
cd /Users/jph/Voiceapp/workspace_template
cargo bench --workspace 2>&1 | tee /tmp/voxlink-bench-final.log
```

From `/tmp/voxlink-bench-final.log`, extract the per-bench "time:" line. A quick grep:

```
grep -E "^(frame_energy|soft_clip|i16_to_f32|mix_4_peers|histogram_observe|signal_message|decode_screen_chunk_metadata)" /tmp/voxlink-bench-final.log -A1 | grep -E "time:" -B1 | head -40
```

Or simpler — open the log and visually pull each bench name + its `time:   [lo mid hi]` mid value.

Note the CPU model of the dev machine:
```
sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m
```

And Rust version:
```
rustc --version
```

- [ ] **Step 3: Write the new `docs/PERFORMANCE_TARGETS.md`**

Substitute `<OBSERVED-FOO>` placeholders with actual numbers from step 2. Substitute `<DEV-MACHINE>` with the CPU brand string, `<RUSTC-VERSION>` with the rustc output.

```markdown
# Performance Targets

These are guiding targets for Voxlink. They shape implementation decisions and gate regressions via `scripts/bench-check.sh`.

## Product-level targets (qualitative)

- Cold start should feel fast.
- Idle app at home screen should use very little CPU.
- Background/idle room state should not spin work unnecessarily.
- Joining a room should feel near-instant on a healthy network.
- Device switching should be responsive.

## Engineering rules

- No busy loops.
- No large recurring allocations in hot paths.
- Avoid unnecessary cloning of state.
- Throttle metrics updates to sensible frequencies.
- UI should redraw only when needed.
- Audio callbacks must stay extremely lightweight.

## Early instrumentation priorities

- App startup timing.
- Idle CPU sampling.
- Idle memory reporting.
- UI update cadence.
- Audio callback timing.
- Reconnect count.
- Room join timing.

## Validation mindset

Every subsystem should eventually answer:
- What does it cost at idle?
- What does it cost during a 4-person call?
- What work runs every second, and why?

---

## Microbenchmark baselines

Measured on: `<DEV-MACHINE>`, `<RUSTC-VERSION>`, 2026-04-17.

Each row's "Observed" is the median of criterion's `[lo mid hi]` triple on a quiet machine. "Target" is the regression threshold: if `scripts/bench-check.sh` later reports a value >1.2× the target, investigate before merging.

### audio_core

| Benchmark | Target | Observed | Notes |
|---|---|---|---|
| `frame_energy_960` | < 500 ns | `<OBSERVED-FRAME-ENERGY>` | 20 ms frame at 48 kHz |
| `soft_clip_960` | < 3 µs | `<OBSERVED-SOFT-CLIP>` | Per-sample soft clipper |
| `i16_to_f32_960` | < 1 µs | `<OBSERVED-I16-F32>` | Every decoded frame |
| `mix_4_peers_960` | < 5 µs | `<OBSERVED-MIX4>` | Four-peer output mix |
| `frame_energy_silence` | < 500 ns | `<OBSERVED-FRAME-SILENCE>` | Early-exit path |
| `soft_clip_passthrough_960` | < 3 µs | `<OBSERVED-SOFT-CLIP-PASS>` | In-range samples |

### signaling_server

| Benchmark | Target | Observed | Notes |
|---|---|---|---|
| `histogram_observe` | < 200 ns | `<OBSERVED-HISTOGRAM>` | One observation |
| `signal_message_from_slice_simple` | < 2 µs | `<OBSERVED-DESER-SIMPLE>` | Unit variant |
| `signal_message_from_slice_complex` | < 10 µs | `<OBSERVED-DESER-COMPLEX>` | Struct variant with payload |
| `signal_message_to_string` | < 10 µs | `<OBSERVED-SER>` | Serialize |
| `decode_screen_chunk_metadata` | < 50 ns | `<OBSERVED-SCREEN-META>` | 8-byte header parse |

### shared_types

| Benchmark | Target | Observed | Notes |
|---|---|---|---|
| `signal_message_variant_index_struct` | < 5 ns | `<OBSERVED-VI-STRUCT>` | Struct variant jump table |
| `signal_message_variant_index_unit` | < 5 ns | `<OBSERVED-VI-UNIT>` | Unit variant jump table |

## How to regenerate baselines

After an intentional performance change, or when moving to a different dev machine:

```
./scripts/bench-record-baseline.sh
```

Then re-run this file's "Observed" column against the new numbers.

## How to gate a change on regressions

```
./scripts/bench-check.sh
```

Exit code 1 means one or more benches regressed past criterion's significance threshold vs the saved "main" baseline.
```

Replace every `<OBSERVED-*>` placeholder with the actual number (with units, e.g., `2.8 ns` or `1.2 µs`). No `<OBSERVED-*>` tokens may survive into the committed file.

- [ ] **Step 4: Verify no placeholders remain**

```
grep "<OBSERVED" docs/PERFORMANCE_TARGETS.md
grep "<DEV-MACHINE\|<RUSTC-VERSION" docs/PERFORMANCE_TARGETS.md
```
Expected: both grep calls produce no output (all placeholders replaced).

- [ ] **Step 5: Verify workspace clean**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: check clean; clippy ≤ 62. (Docs changes shouldn't affect these, but verify anyway.)

- [ ] **Step 6: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add docs/PERFORMANCE_TARGETS.md
git commit -m "docs: numeric performance baselines in PERFORMANCE_TARGETS.md"
```

---

## Task 5: Final verification + merge

- [ ] **Step 1: Full workspace build**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
```
Expected: clean.

- [ ] **Step 2: Clippy gate**

```
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: ≤ 62.

- [ ] **Step 3: Run non-flaky tests**

```
cargo test --workspace --no-fail-fast -- \
  --skip live_stress \
  --skip test_create_space \
  --skip test_audio_after_leave_room \
  --skip test_channel_audio_relay \
  --skip test_authenticate_invalid_token_creates_new \
  2>&1 | awk '/test result:/ {ok+=$4; fail+=$6} END {print "passed="ok, "failed="fail}'
```
Expected: `failed=0`.

- [ ] **Step 4: Full bench run**

```
./scripts/bench-check.sh
```
Expected: exits 0, "Benchmarks OK — no regressions."

- [ ] **Step 5: Commit manifest**

```
git log --oneline main..HEAD
```
Expected: 4 commits on `feat/m5-perf-baselines` (Tasks 1, 2, 3, 4).

- [ ] **Step 6: Merge to main (if ready to ship)**

```
git checkout main
git merge --ff-only feat/m5-perf-baselines
git branch -d feat/m5-perf-baselines
```

If the user wants to review first, skip this step — leave the branch for a future merge.

---

# Completion criteria

All of:

1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets` ≤ 62 warnings.
3. All non-flaky tests pass.
4. `cargo bench --workspace` runs all benches successfully; three crates have bench output.
5. `docs/PERFORMANCE_TARGETS.md` has a full numeric table with observed ns/µs values from a fresh run — no `<OBSERVED-*>` placeholders remain.
6. `scripts/bench-record-baseline.sh` and `scripts/bench-check.sh` are both executable and work end-to-end on the dev machine.
7. `scripts/bench-check.sh` exits 0 against the freshly-recorded baseline.
8. Observed `histogram_observe` is < 200 ns.
9. Observed `signal_message_variant_index_*` is < 10 ns.

# If something goes wrong

- **Bench target fails to compile with "cannot find type Histogram"**: confirm `crates/signaling_server/src/lib.rs` was created and contains `pub mod histogram;`, and that `Histogram` in `histogram.rs` is `pub` (not `pub(crate)`).
- **Bench target fails with "cannot find SignalMessage"**: `shared_types` should already export it via `pub use protocol::*;` in `lib.rs`. If it doesn't, add the explicit re-export — but first verify with `cargo doc --open -p shared_types`.
- **`bench-check.sh` exits 0 when a regression should be flagged**: check the criterion output — it may have said "Change within noise threshold" instead. That's correct behavior; the regression wasn't statistically significant. Re-run with a larger workload or tighter significance via `-- --significance-level 0.01`.
- **Unit-variant JSON literal rejected by serde**: `SignalMessage` uses serde's default externally-tagged enum representation — try the forms `br#""LeaveRoom""#`, `br#"{"LeaveRoom":null}"#`, and `br#"{"LeaveRoom":{}}"#` in that order. Whichever parses is the right form.
- **Clippy warning count increases**: new warnings likely come from bench code (e.g., `clippy::unit_arg` if `observe(...)` returns `()` and that's what the closure returns). Fix with an explicit `let _ = ...` if that helps, or add a minimal `#[allow(...)]` with a one-line justification.
- **`main.rs` fails to compile after adding `lib.rs`**: likely because `main.rs` now has implicit name collision with the new `lib.rs`. Pin both targets explicitly in `Cargo.toml` with `[lib]` and `[[bin]]` blocks as shown in Task 2 Step 3.
- **`cargo bench` is very slow on first run**: normal — criterion does a warmup + measurement for each bench. Budget 1-2 minutes for the full workspace.

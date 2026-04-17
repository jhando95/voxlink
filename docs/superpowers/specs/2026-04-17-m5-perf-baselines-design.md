# Design — Milestone 5: Performance Baselines & Regression Gate

**Date:** 2026-04-17
**Status:** Approved (pending spec review)
**Scope:** Extend criterion benchmark coverage to hot paths introduced in M1/M3, document numeric baselines in `PERFORMANCE_TARGETS.md`, and add a regression-gate shell script that catches >20% slowdowns. No runtime-code changes.

## Context

CLAUDE.md names "Performance first — minimal idle CPU and RAM" as engineering priority #1. The parent repo's `docs/PERFORMANCE_TARGETS.md` describes goals philosophically ("cold start should feel fast") without numbers.

Existing state:

- `crates/audio_core/benches/audio_benchmarks.rs` has six DSP benches: `frame_energy_960`, `soft_clip_960`, `i16_to_f32_960`, `mix_4_peers_960`, `frame_energy_silence`, `soft_clip_passthrough_960`. `criterion 0.5` is a dev-dep on `audio_core`.
- No benches in `signaling_server` or `shared_types` crates.
- No saved baselines, no regression check, no numeric targets.

M1+M2+M3 added hot-path code (lock-free histogram, variant-index dispatch, JSON parser wrappers) without bench coverage. Claims like "Histogram::observe is under 1µs" are unverified.

## Goals

1. Add benches for the five hot paths introduced by recent milestones.
2. Record observed numbers in a table readers can sanity-check against.
3. Provide a one-command regression gate the user can run before merging future work.
4. Zero runtime-code changes.

## Non-goals

- **Macro benchmarks** (load-test the server with N simulated clients). Flaky, expensive, and the M3 observability work already surfaces equivalent signals from real traffic. Its own milestone if needed.
- **Memory profiling.** Criterion doesn't measure memory; `dhat` / `heaptrack` are the right tools. Separate concern.
- **Client cold-start time.** Requires OS-level instrumentation; manual measurement for now.
- **CI integration.** Repo has no CI config. Regression gate is a local script.
- **Trend dashboards / historical graphs.** Criterion's HTML reports are enough for manual inspection.
- **Benches for every function in audio_core.** Existing coverage is adequate; don't re-bench what's already benched.

## Architecture

### 1. Bench additions

Two new bench files, one per crate that lacks coverage. Each file follows the same pattern as the existing `audio_core/benches/audio_benchmarks.rs` — `criterion_group!` + `criterion_main!` at the bottom, `c.bench_function("name", |b| b.iter(|| ...))` for each target.

**`crates/signaling_server/benches/hot_path.rs`** — three benches:

- `histogram_observe` — construct a `Histogram`, run `observe(0.003)` in a tight loop. Expected: <200 ns.
- `signal_message_from_slice_simple` — deserialize `{"LeaveRoom":null}` (a unit variant). Expected: <2 µs.
- `signal_message_from_slice_complex` — deserialize a `SendAudio` or similar struct variant with a realistic payload. Expected: <10 µs.
- `signal_message_to_string` — serialize a representative variant.
- `decode_screen_chunk_metadata` — parse a valid 8-byte header + bit of payload. Expected: <50 ns.

**`crates/shared_types/benches/protocol.rs`** — two benches:

- `signal_message_variant_index` — construct one struct variant, call `variant_index()` in a tight loop. Expected: <5 ns (single jump-table match).
- `signal_message_variant_index_unit` — same for a unit variant. Expected: <5 ns.

Each bench's body is small (5–20 lines). Total new bench code: ~150 lines across the two files.

### 2. Numeric baselines doc

Replace the workspace-template `docs/PERFORMANCE_TARGETS.md` (currently the thin stub I created in M2, a single paragraph) with a full document that merges the parent repo's philosophical goals and a new numeric-baselines table.

Structure:

```markdown
# Performance Targets

## Product-level targets (qualitative)
- Fast cold start
- Very low idle CPU
- No unnecessary background work
- Near-instant room join
- Responsive device switching

## Engineering rules
- No busy loops
- No large recurring allocations in hot paths
- Throttled metrics
- UI redraws only when needed
- Audio callbacks stay lightweight

## Benchmark baselines

Measured on: <dev-machine-model>, Rust <version>, <date>.

### audio_core
| Benchmark | Target | Observed | Notes |
|---|---|---|---|
| frame_energy_960 | <500 ns | … | 20ms frame at 48kHz |
| soft_clip_960 | <3 µs | … | Every output sample |
| i16_to_f32_960 | <1 µs | … | Every decoded frame |
| mix_4_peers_960 | <5 µs | … | Four-peer mix |
| frame_energy_silence | <500 ns | … | |
| soft_clip_passthrough_960 | <3 µs | … | |

### signaling_server
| Benchmark | Target | Observed | Notes |
|---|---|---|---|
| histogram_observe | <200 ns | … | Single observation, four atomics |
| signal_message_from_slice_simple | <2 µs | … | Unit variant |
| signal_message_from_slice_complex | <10 µs | … | Struct variant w/ payload |
| signal_message_to_string | <10 µs | … | Serialize to JSON |
| decode_screen_chunk_metadata | <50 ns | … | 8-byte header |

### shared_types
| Benchmark | Target | Observed | Notes |
|---|---|---|---|
| signal_message_variant_index | <5 ns | … | Struct variant match |
| signal_message_variant_index_unit | <5 ns | … | Unit variant match |
```

The `…` cells get filled in during implementation, from a fresh bench run on the dev machine. Target column sets the regression gate: more than 20% over target warrants investigation.

### 3. Regression gate script

Two scripts under `scripts/`:

**`scripts/bench-record-baseline.sh`** (opt-in, infrequent):
```bash
#!/usr/bin/env bash
# Record the current machine's bench times as the "main" baseline.
# Run this after an intentional perf change, or when setting up on a new machine.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo bench --workspace -- --save-baseline main
echo "Baseline saved under target/criterion/*/main/"
echo "Commit those files if you want to share: target/criterion is gitignored by default;"
echo "the baselines above ship via docs/PERFORMANCE_TARGETS.md only."
```

**`scripts/bench-check.sh`** (run before merging performance-sensitive work):
```bash
#!/usr/bin/env bash
# Run benches against the saved "main" baseline and flag regressions >20%.
# Requires: cargo-criterion, a previously-saved "main" baseline.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if [ ! -d target/criterion ]; then
    echo "No baseline found. Run scripts/bench-record-baseline.sh first."
    exit 2
fi

cargo bench --workspace -- --baseline main 2>&1 | tee /tmp/voxlink-bench.log

# Look for criterion's "Performance has regressed" markers in output.
if grep -E "Performance has regressed\." /tmp/voxlink-bench.log > /dev/null; then
    echo
    echo "REGRESSION DETECTED:"
    grep -B2 -A2 "Performance has regressed\." /tmp/voxlink-bench.log
    echo
    echo "If this is expected, re-record the baseline: scripts/bench-record-baseline.sh"
    exit 1
fi

echo
echo "Benchmarks OK — no regressions."
```

Criterion already detects and reports regressions via its built-in statistical comparison when `--baseline <name>` is passed. The script just watches for the standard regression marker string ("Performance has regressed.") in the output. No extra dependency on `critcmp` or anything else.

20% is the default criterion significance threshold — we don't override it. Users who want a tighter gate can pass `--significance-level 0.01` or similar.

### 4. No runtime changes

No code in `crates/*/src/` is modified. Benches are strictly additive.

## Components

| File | Change |
|---|---|
| `crates/signaling_server/Cargo.toml` | add `criterion = "0.5"` to `[dev-dependencies]`; add `[[bench]] name = "hot_path"; harness = false` |
| `crates/signaling_server/benches/hot_path.rs` *(new)* | five benches per design |
| `crates/shared_types/Cargo.toml` | add `criterion = "0.5"` to `[dev-dependencies]`; add `[[bench]] name = "protocol"; harness = false` |
| `crates/shared_types/benches/protocol.rs` *(new)* | two benches per design |
| `scripts/bench-record-baseline.sh` *(new)* | record baseline |
| `scripts/bench-check.sh` *(new)* | regression gate |
| `docs/PERFORMANCE_TARGETS.md` | replace stub with full numeric doc |

## Testing & verification

- `cargo build --benches --workspace` compiles all three bench binaries.
- `cargo bench --workspace` runs them all; completes in ~60 s on a modern laptop.
- `scripts/bench-record-baseline.sh` then `scripts/bench-check.sh` → clean pass (no regressions vs just-recorded baseline).
- The numeric targets table in `PERFORMANCE_TARGETS.md` is filled in during implementation from the fresh run — no placeholders in the committed doc.
- `cargo clippy --workspace --all-targets -- -D warnings` — benches compile without new warnings (baseline 62, must not exceed).

## Risks & mitigations

- **Flaky numbers on a loaded dev machine.** Criterion's statistical model catches small noise; the 20% default threshold absorbs the rest. Users running benches with heavy background load are expected to re-run.
- **Numbers bitrot across machines.** The committed table reflects the dev machine. A user on a slower laptop might see the `signaling_server/histogram_observe` bench clock 400 ns instead of 60 ns. `scripts/bench-record-baseline.sh` regenerates the comparison baseline per machine; the documented numbers are advisory, not absolute.
- **New benches themselves could have bugs** (e.g., something inlined away by the compiler). Mitigated by `black_box` usage in each bench body and by visibly non-trivial observed times — if a bench reports 0 ns, the test is wrong and we re-write it.
- **Scope creep** — it's tempting to add benches for every DSP function. Resist. The design names exactly the five hot-path additions.

## Commit strategy

1. `bench(shared_types): protocol benches (variant_index + JSON round trip)`
2. `bench(signaling_server): hot-path benches (histogram, signal serde, screen chunk)`
3. `scripts: add bench-record-baseline.sh and bench-check.sh`
4. `docs: numeric performance baselines in PERFORMANCE_TARGETS.md`

Four commits, workspace green at each.

## Success criteria

1. `cargo bench --workspace` succeeds.
2. `docs/PERFORMANCE_TARGETS.md` has a full numeric table with ns/op values from a fresh run.
3. `scripts/bench-check.sh` exits 0 against a freshly-recorded baseline.
4. `cargo check --workspace` clean; clippy ≤ 62.
5. Observed `histogram_observe` time is <200 ns, validating the "under 1 µs" claim.
6. Observed `signal_message_variant_index` time is <5 ns, validating the "zero-cost dispatch" claim.

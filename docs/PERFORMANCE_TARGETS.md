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

Measured on: **Apple M4 Pro, rustc 1.94.0, 2026-04-17**.

Each row's "Observed" is the median of criterion's `[lo mid hi]` triple. "Target" is the regression threshold: if `scripts/bench-check.sh` reports a value more than ~10% above target, investigate before merging.

### audio_core

| Benchmark | Target | Observed | Notes |
|---|---|---|---|
| `frame_energy_960` | < 500 ns | 439.7 ns | 20 ms frame at 48 kHz |
| `soft_clip_960` | < 1 µs | 655.8 ns | Per-sample soft clipper |
| `i16_to_f32_960` | < 100 ns | 54.2 ns | Every decoded frame (bench is noisy — up to ±10% run-to-run) |
| `mix_4_peers_960` | < 500 ns | 221.2 ns | Four-peer output mix |
| `frame_energy_silence` | < 500 ns | 440.5 ns | Early-exit path |
| `soft_clip_passthrough_960` | < 1 µs | 655.9 ns | In-range samples |

### signaling_server

| Benchmark | Target | Observed | Notes |
|---|---|---|---|
| `histogram_observe` | < 200 ns | 2.18 ns | One observation (bucket scan + atomics) |
| `signal_message_from_slice_simple` | < 100 ns | 19.3 ns | Unit variant deserialize |
| `signal_message_from_slice_complex` | < 500 ns | 87.6 ns | Struct variant with payload |
| `signal_message_to_string` | < 200 ns | 40.2 ns | Serialize to JSON |
| `decode_screen_chunk_metadata` | < 10 ns | 780 ps | 8-byte header parse |

### shared_types

| Benchmark | Target | Observed | Notes |
|---|---|---|---|
| `signal_message_variant_index_struct` | < 5 ns | 417 ps | Struct variant jump table |
| `signal_message_variant_index_unit` | < 5 ns | 417 ps | Unit variant jump table |

All measured values are comfortably under their targets. The regression gate will flag any bench that drifts past criterion's significance threshold vs the saved "main" baseline.

## How to regenerate baselines

After an intentional performance change, or when moving to a different dev machine:

```
./scripts/bench-record-baseline.sh
```

Then re-run this file's "Observed" column from the fresh log. Commit the updated numbers.

## How to gate a change on regressions

```
./scripts/bench-check.sh
```

Exit code 1 means one or more benches regressed past criterion's significance threshold vs the saved "main" baseline. If the slowdown is expected, re-record; otherwise investigate.

Note: criterion's detection is statistically rigorous but some benches (notably `i16_to_f32_960`) show run-to-run variation of ~10% on this machine even without code changes. When the gate fires, inspect the magnitude — anything under ~15% on a single run is probably noise; re-run before investigating code.

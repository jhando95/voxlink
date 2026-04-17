# Voxlink Fuzz Targets

Protocol-parsing fuzz targets driven by [`cargo-fuzz`](https://rust-fuzz.github.io/book/cargo-fuzz.html) and libFuzzer.

## Prerequisites

- Rust nightly toolchain: `rustup install nightly`
- cargo-fuzz: `cargo install cargo-fuzz`

## Targets

| Target | Fuzzes |
|---|---|
| `fuzz_signal_message` | `serde_json::from_slice::<SignalMessage>` — the server's top-level signaling parser |
| `fuzz_udp_frame` | UDP packet receive path — session token extraction, packet-type dispatch |
| `fuzz_screen_chunk_metadata` | `decode_screen_chunk_metadata` from `shared_types::screen` |

## Running

From the repository root:

```
cd fuzz
cargo +nightly fuzz run fuzz_signal_message
```

To limit runtime (e.g., for CI-style smoke tests):

```
cargo +nightly fuzz run fuzz_signal_message -- -max_total_time=60
```

## If a crash is found

1. libFuzzer stops and writes the input to `fuzz/artifacts/<target>/crash-<hash>`.
2. Check the reproducer into git alongside the fix: `git add fuzz/artifacts/<target>/crash-<hash>`.
3. Fix the bug. Re-run the target to confirm.
4. On any future run, libFuzzer replays checked-in crashes first — so fixed bugs act as regression tests.

# CI — GitHub Actions

Voxlink's CI runs on **macOS** and **Windows** — the two platforms users actually run. Linux is not in the matrix; Linux-only regressions surface at deploy time via `deploy/push-to-server.sh`.

## Jobs

### `build-test` (matrix: macos-latest, windows-latest)

Runs on every push to any branch and every PR targeting `main`.

Steps:
- `cargo check --workspace --all-targets`
- `cargo test --workspace --no-fail-fast` (with the flaky-test skip list — see the workflow file)

If either OS fails, the gate is red. Both must pass.

### `lint` (macos-latest)

Runs on the same triggers as `build-test`.

Steps:
- `cargo fmt --all -- --check` — fail on formatting drift.
- `cargo clippy --workspace --all-targets` — fail if the total warning count exceeds **63** (the baseline at M10 completion).

### `windows-installer` (windows-latest, tag-gated)

Runs ONLY on tag pushes matching `v*` (e.g., `v0.11.0`). Produces two downloadable artifacts:

- `Voxlink-Setup-<version>.exe` — Inno Setup installer
- `Voxlink-<version>/` — portable zip contents

Artifacts appear under the workflow run's "Artifacts" section in the GitHub Actions UI. They are NOT attached to a GitHub Release automatically; release-page automation is a separate future milestone.

## Triggers

| Event | Jobs run |
|---|---|
| Push to any branch | `build-test`, `lint` |
| PR to `main` | `build-test`, `lint` |
| Push tag `v*` | `build-test`, `lint`, `windows-installer` |

## Reading failures

1. Open the failing run in the Actions tab.
2. Click the red job.
3. Open the red step — the last ~50 lines of output usually pinpoint the failure.
4. If a test failed, look for `test ... FAILED` in the `cargo test` output; the panic message follows.

## Updating the clippy threshold

The threshold lives inline in `.github/workflows/ci.yml` under the `lint` job's `clippy (threshold 63)` step. If a genuine new warning lands (e.g., from a Rust compiler bump adding a new lint), update both the threshold in `ci.yml` and this document. Commit together with a justification.

## Bumping the Rust toolchain

The toolchain is pinned to `1.94` in three places inside `ci.yml` (one per job). Bump by editing each occurrence. Always verify locally first:

```
rustup install <new-version>
rustup override set <new-version>
cargo check --workspace && cargo test --workspace
```

Commit: `ci: bump Rust toolchain 1.94 → 1.95`.

## Cache invalidation

`Swatinem/rust-cache@v2` keys on `Cargo.lock` + the per-job key (`macos-latest` / `windows-latest` / `lint-macos` / `windows-installer`). If a cache ever serves stale artifacts that cause spurious failures, bump its key name in `ci.yml` (e.g., append `-v2`) to force a rebuild.

## Running the gate locally

Mirror the CI gate before pushing:

```
cargo check --workspace --all-targets
cargo test --workspace --no-fail-fast -- \
    --skip live_stress --skip test_create_space \
    --skip test_audio_after_leave_room --skip test_channel_audio_relay \
    --skip test_authenticate_invalid_token_creates_new
cargo fmt --all -- --check
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"   # must be ≤ 63
```

# CI — GitHub Actions

Voxlink's CI runs on **macOS** and **Windows** — the two platforms users actually run. Linux is not in the matrix; Linux-only regressions surface at deploy time via `deploy/push-to-server.sh`.

## Jobs

### `build-test` (matrix: macos-latest, windows-latest)

Runs on every push to any branch and every PR targeting `main`.

Steps:
- `cargo check --workspace --all-targets`
- `cargo test --workspace --no-fail-fast` (skipping only `live_stress`, which needs the remote production server)

The former flaky-test skip list (`test_create_space`, `test_audio_after_leave_room`, `test_channel_audio_relay`, `test_authenticate_invalid_token_creates_new`) was removed in v0.13.5. The flakiness was a port-reservation race in the test harness — the server could lose its reserved port to a parallel test and exit at startup; the harness now detects that early exit and respawns on fresh ports.

If either OS fails, the gate is red. Both must pass.

### `lint` (macos-latest)

Runs on the same triggers as `build-test`.

Steps:
- `cargo fmt --all -- --check` — fail on formatting drift.
- `cargo clippy --workspace --all-targets -- -D warnings` — any clippy warning fails the gate (the workspace reached zero warnings in v0.13.3).

### `windows-installer` (windows-latest, tag-gated)

Runs ONLY on tag pushes matching `v*` (e.g., `v0.11.0`). Produces two downloadable artifacts:

- `Voxlink-Setup-<version>.exe` — Inno Setup installer
- `Voxlink-<version>/` — portable zip contents

Artifacts appear under the workflow run's "Artifacts" section in the GitHub Actions UI. In addition, the separate `release.yml` ("Build & Release") workflow runs on the same tag push: it builds the Windows installer + portable zip, optionally code-signs them, and attaches both to a GitHub Release page automatically.

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

## Clippy policy

The workspace holds a zero-warning bar: `cargo clippy --workspace --all-targets -- -D warnings` must pass. If a toolchain bump introduces a genuinely new lint that needs time to fix, prefer fixing it in the same commit; only `#[allow]` with an inline justification as a last resort.

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
cargo build -p signaling_server -p app_desktop   # test harness spawns these
cargo test --workspace --no-fail-fast -- --skip live_stress
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

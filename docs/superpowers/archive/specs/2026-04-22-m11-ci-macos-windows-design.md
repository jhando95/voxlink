# Design — Milestone 11: GitHub Actions CI (macOS + Windows)

**Date:** 2026-04-22
**Status:** Approved (pending spec review)
**Scope:** Add `.github/workflows/ci.yml` that runs commit-gate checks on macOS and Windows. Tag-gated installer build on Windows produces workflow artifacts. No Linux runner (production deploy-time validation is judged sufficient).

## Context

Voxlink now has ten milestones of code sitting on `main` with zero automated gating. Local `cargo check` and `bench-check.sh` are thorough but only run when the dev remembers to. Users primarily run on Windows; the dev works on macOS; no CI means a Windows regression would only be caught when someone tries to build on Windows. That risk is the highest-priority one to close.

Previous iterations of this spec considered a Linux + Windows matrix (to match the Oracle production server). Feedback pivoted to macOS + Windows with Linux dropped. The tradeoff is explicit: a Linux-only server build break surfaces at `push-to-server.sh` time rather than commit time — observable, non-silent, non-destructive (Oracle keeps the old binary).

## Goals

1. Every push and PR triggers automated `cargo check` + `cargo test` on macOS and Windows.
2. Every push triggers a lint check (`cargo fmt --check`, `cargo clippy`).
3. Release tags trigger a Windows installer build producing downloadable artifacts.
4. Gate latency < 10 min cold cache / < 5 min warm cache.
5. Zero false-failure noise — the skip list for known-flaky integration tests matches local.

## Non-goals

- **Linux runner.** Production build failures surface at deploy time via `push-to-server.sh`.
- **macOS + Apple Silicon AND Intel matrix expansion.** `macos-latest` currently maps to macOS 14 ARM. That's enough.
- **GitHub Releases automation** (creating release pages, uploading binaries to tag). Follow-up work.
- **Cross-version Rust matrix** (stable + nightly + MSRV). Pin to 1.94 matching local; MSRV bumps are deliberate.
- **Benchmarks in CI.** `bench-check.sh` is local-only; shared runners are noisy.
- **`cargo-audit` / `cargo-deny` supply-chain scans.** Future security milestone.
- **Coverage reporting.** Future separate concern.
- **Nightly fuzz sweeps.** `cargo-fuzz` needs nightly; separate workflow if ever wanted.
- **Caching the installer output between runs.** Installer builds run only on tags; caching for single-shot runs offers no wins.

## Architecture

Single workflow file `.github/workflows/ci.yml` with three jobs:

### Job 1: `build-test`

Matrix: `os: [macos-latest, windows-latest]`. Runs on every push to any branch and every PR targeting `main`.

Steps:
1. `actions/checkout@v4` — source code.
2. `dtolnay/rust-toolchain@1.94` — pinned stable Rust.
3. `Swatinem/rust-cache@v2` — incremental cargo cache keyed on `Cargo.lock` + OS.
4. `cargo check --workspace --all-targets` — compiles libs, bins, tests, benches.
5. `cargo test --workspace --no-fail-fast -- <skip list>` — runs all tests except the five known-flaky integration tests (`live_stress_*`, `test_create_space`, `test_audio_after_leave_room`, `test_channel_audio_relay`, `test_authenticate_invalid_token_creates_new`).

No platform deps needed on either runner (CoreAudio / WASAPI / Slint software renderer all preinstalled).

### Job 2: `lint`

Runs on `macos-latest` (cheaper than doubling up on Windows). Runs on the same trigger as `build-test`.

Steps:
1. Checkout + Rust toolchain + cache (same as above).
2. `cargo fmt --all -- --check` — fail if formatting drift detected.
3. `cargo clippy --workspace --all-targets 2>&1 | tee clippy.log` followed by a shell test that greps for `^warning:` and fails if count > 62 (baseline after M9+M10 merges). This mirrors `scripts/bench-check.sh`'s threshold-based approach.

The grep-gate is implemented inline:

```yaml
      - name: clippy
        run: |
          cargo clippy --workspace --all-targets 2>&1 | tee clippy.log
          count=$(grep -cE "^warning:" clippy.log || true)
          echo "Clippy warning count: $count"
          if [ "$count" -gt 62 ]; then
            echo "Clippy warning count $count exceeds baseline of 62"
            exit 1
          fi
```

### Job 3: `windows-installer`

Runs on `windows-latest`. Triggered ONLY by tag pushes matching `v*`. Does not block PRs or commits.

Steps:
1. Checkout + Rust toolchain + cache.
2. `cargo build --release --workspace`.
3. `powershell -ExecutionPolicy Bypass -File installer/build-portable.ps1` — produces the portable zip.
4. Compile the Inno Setup installer. Use the maintained `Minionguyjpro/Inno-Setup-Action@v1` Action with `path: installer/voxlink.iss`. OR invoke `iscc.exe` directly if the Action proves unstable; fallback path is a manual install of Inno Setup via `choco install innosetup -y`.
5. `actions/upload-artifact@v4` — upload the produced `.exe` installer and `.zip` portable bundle as artifacts named after the tag.

### Triggers

```yaml
on:
  push:
    branches: ['**']
    tags: ['v*']
  pull_request:
    branches: [main]
```

`build-test` and `lint` run on all push/PR events.
`windows-installer` runs only when `github.ref_type == 'tag'` (gated via a job-level `if:`).

### Shared Rust setup block

Factored out into an anchored YAML step list if supported, otherwise inlined per job. GitHub Actions doesn't support YAML anchors natively, so each job repeats checkout + toolchain + cache. Acceptable: ~10 lines per job.

## Components

| File | Change |
|---|---|
| `.github/workflows/ci.yml` *(new)* | The workflow, ~120 lines |
| `docs/CI.md` *(new)* | One-page operator doc |

## Testing

- **Initial run:** after committing `ci.yml`, a push to the CI's own feature branch triggers the workflow. Expected result: green on both `build-test` matrix entries and `lint`. If any fails, fix before merge.
- **Intentional break test:** temporarily introduce a compile error on the branch, push, verify `build-test` fails on at least one OS. Revert.
- **Tag test:** push a lightweight tag `test-m11` to trigger `windows-installer`, verify artifact upload. Delete the tag after. Only do this once — real `v*` tags will trigger the job automatically in normal release flow.
- **Warm cache timing:** the second push on the same branch should use the cache and come in under 5 minutes total. Timing is observable in the Actions UI.

## Risks

- **First Windows CI run fails** — M1-M10 all landed without Windows validation. If it fails, that's a find, not a bug in M11. Mitigation: fix whatever CI exposes before landing M11 on main. If the fix is large, extract it into its own small commit on the branch (still atomic).
- **Inno Setup Action breaks or vanishes** — pinned to `@v1`. Fallback: `choco install innosetup -y` + `iscc.exe installer/voxlink.iss`.
- **Flaky skip list insufficient on CI** — CI's shared runners may expose new timing flakes. Mitigation: monitor first ~5 CI runs, extend skip list as needed, document.
- **Clippy warning count drift above 62** — happens every time a new warning is genuine. The gate fails loudly; author either fixes the warning or bumps the threshold deliberately (in the workflow + `PERFORMANCE_TARGETS.md` or similar).
- **Rust toolchain 1.94 gets superseded** — new Rust comes out every ~6 weeks. Pinning means CI continues to work; bump deliberately when needed by editing the workflow.

## Commit strategy

Two commits:

1. `ci: add GitHub Actions workflow for macOS + Windows`
2. `docs: add CI.md operator guide`

Each self-contained.

## Success criteria

1. `.github/workflows/ci.yml` exists, compiles as valid YAML, produces three defined jobs.
2. Pushing the M11 branch triggers a CI run visible in the Actions tab.
3. All three non-installer jobs (`build-test (macos-latest)`, `build-test (windows-latest)`, `lint`) pass on current `main`.
4. A test `v`-prefixed tag triggers the `windows-installer` job and uploads artifacts.
5. Cold-cache wall time ≤ 10 min, warm-cache ≤ 5 min, both observed on an actual run.
6. `docs/CI.md` explains triggers, job shapes, how to read failures, and how to bump toolchain/thresholds.

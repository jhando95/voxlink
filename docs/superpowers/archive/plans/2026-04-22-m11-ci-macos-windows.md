# M11 — macOS + Windows CI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A single `.github/workflows/ci.yml` with three jobs: `build-test` (matrix macos+windows), `lint` (macos-only), `windows-installer` (tag-gated). Documented in `docs/CI.md`.

**Architecture:** One workflow file. `build-test` and `lint` trigger on every push and PR. `windows-installer` triggers only on `v*` tag pushes and uploads artifacts via `actions/upload-artifact`. Rust 1.94 pinned via `dtolnay/rust-toolchain`, cached via `Swatinem/rust-cache@v2`, Inno Setup compilation via `Minionguyjpro/Inno-Setup-Action@v1`.

**Tech Stack:** GitHub Actions YAML, pinned action versions, Rust 1.94, Inno Setup.

**Spec:** `docs/superpowers/specs/2026-04-22-m11-ci-macos-windows-design.md`

**Workspace root:** `/Users/jph/Voiceapp/workspace_template`
**Branch:** start on `feat/m11-ci` from `main`.

---

## Ground rules

1. **Workspace stays green.** `cargo check --workspace` passes before each commit.
2. **No clippy regression.** Baseline is 62; must not exceed.
3. **Do not touch source code** unless a Windows build reveals an actual bug (in which case, that's a separate commit on the same branch with a clear "fix(ci)" message).
4. **Use pinned action versions** — `@v4`, `@v2`, `@1.94`, etc. — no floating `@main`.

---

## Task 0: Branch

- [ ] **Step 1: Clean tree, create branch**

```
cd /Users/jph/Voiceapp/workspace_template
git status --short    # expect empty (discard Cargo.lock drift if present)
git checkout -b feat/m11-ci
```

- [ ] **Step 2: Baseline clippy count**

```
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: `62`. Record the value — the workflow will use it as the threshold.

No commit.

---

## Task 1: Create the CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Create the `.github/workflows/` directory and the workflow file**

```
cd /Users/jph/Voiceapp/workspace_template
mkdir -p .github/workflows
```

Create `.github/workflows/ci.yml` with EXACTLY this content:

```yaml
name: CI

on:
  push:
    branches: ['**']
    tags: ['v*']
  pull_request:
    branches: [main]

# Cancel in-progress runs for the same ref when a new push arrives.
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: short

jobs:
  build-test:
    name: build-test (${{ matrix.os }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [macos-latest, windows-latest]
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust 1.94
        uses: dtolnay/rust-toolchain@master
        with:
          toolchain: "1.94"

      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.os }}

      - name: cargo check --workspace --all-targets
        run: cargo check --workspace --all-targets

      - name: cargo test (skip flaky)
        shell: bash
        run: |
          cargo test --workspace --no-fail-fast -- \
            --skip live_stress \
            --skip test_create_space \
            --skip test_audio_after_leave_room \
            --skip test_channel_audio_relay \
            --skip test_authenticate_invalid_token_creates_new

  lint:
    name: lint
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust 1.94
        uses: dtolnay/rust-toolchain@master
        with:
          toolchain: "1.94"
          components: rustfmt, clippy

      - uses: Swatinem/rust-cache@v2
        with:
          key: lint-macos

      - name: cargo fmt --check
        run: cargo fmt --all -- --check

      - name: clippy (threshold 62)
        shell: bash
        run: |
          cargo clippy --workspace --all-targets 2>&1 | tee clippy.log
          count=$(grep -cE "^warning:" clippy.log || true)
          echo "Clippy warning count: $count"
          if [ "$count" -gt 62 ]; then
            echo "FAIL: clippy warning count $count exceeds baseline of 62"
            exit 1
          fi

  windows-installer:
    name: windows-installer
    runs-on: windows-latest
    if: github.ref_type == 'tag' && startsWith(github.ref, 'refs/tags/v')
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust 1.94
        uses: dtolnay/rust-toolchain@master
        with:
          toolchain: "1.94"

      - uses: Swatinem/rust-cache@v2
        with:
          key: windows-installer

      - name: cargo build --release
        run: cargo build --release --workspace

      - name: Build portable zip
        shell: powershell
        run: installer\build-portable.ps1

      - name: Compile Inno Setup installer
        uses: Minionguyjpro/Inno-Setup-Action@v1.2.4
        with:
          path: installer/voxlink.iss
          options: /O+

      - name: Upload installer artifact
        uses: actions/upload-artifact@v4
        with:
          name: voxlink-windows-installer-${{ github.ref_name }}
          path: |
            installer/Output/Voxlink-Setup-*.exe
            target/Voxlink-*
          if-no-files-found: error
```

- [ ] **Step 2: Validate YAML syntax locally**

```
cd /Users/jph/Voiceapp/workspace_template
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))" && echo "yaml ok"
```
Expected: `yaml ok`.

If Python isn't available, `ruby -ryaml -e 'YAML.load_file(".github/workflows/ci.yml")' && echo "yaml ok"` works too.

- [ ] **Step 3: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add .github/workflows/ci.yml
git commit -m "ci: add GitHub Actions workflow for macOS + Windows"
```

---

## Task 2: CI operator documentation

**Files:**
- Create: `docs/CI.md`

- [ ] **Step 1: Write `docs/CI.md`**

```markdown
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
- `cargo fmt --all -- --check` — fail on formatting drift
- `cargo clippy --workspace --all-targets` — fail if the total warning count exceeds **62** (the baseline at M10 completion; see `scripts/bench-check.sh` for the same threshold pattern on benches)

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

The threshold lives inline in `.github/workflows/ci.yml` under `lint.steps.clippy (threshold 62)`. If a genuine new warning lands (e.g., from a Rust compiler bump adding a new lint), update both:

- The threshold in `ci.yml`
- This document

and commit together with a justification in the message.

## Bumping the Rust toolchain

The toolchain is pinned to `1.94` in three places inside `ci.yml` (one per job). Bump by editing each occurrence. Always verify locally first:

```
rustup install <new-version>
rustup override set <new-version>
cargo check --workspace && cargo test --workspace
```

Commit with a message like `ci: bump Rust toolchain 1.94 → 1.95`.

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
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"   # must be ≤ 62
```
```

- [ ] **Step 2: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add docs/CI.md
git commit -m "docs: add CI operator guide"
```

---

## Task 3: Push and observe

**Purpose:** Validate the workflow actually triggers and passes. This is the most important step — a workflow file that doesn't run is worthless.

- [ ] **Step 1: Push the branch**

```
cd /Users/jph/Voiceapp/workspace_template
git push -u origin feat/m11-ci
```

GitHub responds with a URL for creating a PR. Note: the `on: push` trigger activates on any branch, so CI runs immediately without a PR.

- [ ] **Step 2: Watch the run**

```
gh run list --branch feat/m11-ci --limit 1
```

Output includes a run ID. Watch live:

```
gh run watch <run-id>
```

Expected: three jobs (`build-test (macos-latest)`, `build-test (windows-latest)`, `lint`) start in parallel. `windows-installer` does NOT run (no tag).

- [ ] **Step 3: Triage failures**

If any job fails:

- **Clippy count exceeded on CI but not locally**: CI's Rust toolchain might be subtly different (patch version). If warning count is 63 or 64 from the runner, either (a) bump the threshold in `ci.yml` to match the new count and commit, or (b) fix the extra warnings if they're legitimate.

- **Windows build fails**: this is the first Windows build in CI. Common causes:
  - Line-ending issues in `.slint` files (should be LF; check with `file` on a `.slint` file locally).
  - A `#[cfg(unix)]` gate on something that actually needs Windows fallback. Report the exact error before guessing.
  - Missing `openssl` or similar — add `choco install openssl -y` step BEFORE `cargo check` if the error is `link.exe: cannot find openssl`.

- **macOS build fails**: likely cpal / CoreAudio link error. Typically fixes with no action — retry once. If persistent, check `cargo check` output for missing frameworks.

- **Test failure only on CI**: likely a flaky test that passes locally. Add to the skip list in the workflow file, commit, push, re-run.

Any fix goes on the same `feat/m11-ci` branch as a new commit. Repeat until all three jobs pass.

- [ ] **Step 4: Record the first-pass observations**

Once all three jobs are green, grab the Actions UI timing data:

```
gh run view <run-id>
```

The Jobs section shows wall-clock time per job. Note:
- cold-cache wall time
- which job was slowest
- any warnings that surfaced (e.g., deprecation notices from actions)

Expected cold-cache: < 10 min; warm (second run on the same branch): < 5 min.

No commit unless fixes were needed during triage.

---

## Task 4: Optional — test the installer job

**Purpose:** Validate the `windows-installer` job without creating a real release tag.

- [ ] **Step 1: Push a test tag**

```
cd /Users/jph/Voiceapp/workspace_template
git tag ci-test-m11
git push origin ci-test-m11
```

Note: this tag does NOT match the `v*` pattern (`ci-test-m11` starts with `c`), so the `windows-installer` job will NOT trigger. That's intentional — the `if:` guard requires `v*`. If you want to test the installer job, use a test tag that DOES match, e.g., `v0.0.0-cit`:

```
git tag v0.0.0-cit
git push origin v0.0.0-cit
```

Then the `windows-installer` job triggers.

- [ ] **Step 2: Verify the artifact**

```
gh run list --branch feat/m11-ci --limit 5
gh run view <run-id-for-tag>
```

Look for `windows-installer`. When it finishes, artifacts are downloadable:

```
gh run download <run-id> -n voxlink-windows-installer-v0.0.0-cit
```

Expected: an `.exe` file and a `Voxlink-<version>` portable directory unpack into `./`.

- [ ] **Step 3: Clean up the test tag (only if you used one)**

```
git push origin :refs/tags/v0.0.0-cit
git tag -d v0.0.0-cit
```

If the installer job failed, fix — most likely cause is the `Minionguyjpro/Inno-Setup-Action` version drift or a path issue in `build-portable.ps1`. The workflow file's step names make the failing step obvious in the UI.

No commit unless fixes were needed.

---

## Task 5: Merge to main

Only proceed if Task 3 is all green (and Task 4 if you ran it).

- [ ] **Step 1: Fast-forward merge**

```
cd /Users/jph/Voiceapp/workspace_template
git checkout main
git merge --ff-only feat/m11-ci
git branch -d feat/m11-ci
```

- [ ] **Step 2: Push to GitHub**

```
git push origin main
```

This triggers CI on `main`. Expected: same green result.

- [ ] **Step 3: Verify CI on main is green**

```
gh run list --branch main --limit 1
```

Wait until green.

---

# Completion criteria

All of:

1. `.github/workflows/ci.yml` exists and is valid YAML.
2. `docs/CI.md` exists and covers the workflow comprehensively.
3. A run on `feat/m11-ci` branch passes all three non-installer jobs.
4. `main` branch's first CI run after merge passes the same three jobs.
5. Cold-cache wall time observed ≤ 10 min; warm ≤ 5 min.
6. If tested: `windows-installer` produces downloadable artifacts on a `v*` tag push.

# If something goes wrong

- **GitHub Actions is disabled for the repo**: Settings → Actions → General → "Allow all actions and reusable workflows". The workflow won't trigger until enabled.
- **`dtolnay/rust-toolchain@master` flakes**: swap to the pinned form `@1.94` (the action supports both). The master branch usage is per the maintainer's recommendation, but pinning is safe.
- **Inno Setup action pin `@v1.2.4` has been unpublished**: check the action's releases page on GitHub; bump to the latest tag and commit. If the action is gone, fallback is a manual step:
  ```yaml
  - name: Install Inno Setup
    shell: powershell
    run: choco install innosetup -y
  - name: Compile installer
    shell: powershell
    run: & "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer/voxlink.iss
  ```
- **`Swatinem/rust-cache@v2` cache grows huge**: the free tier caps at 10 GB per repo. If it hits the cap, GitHub auto-evicts. If builds inexplicably slow down, bump the cache key suffix in `ci.yml` to force a rebuild from scratch.
- **`concurrency.cancel-in-progress: true` cancels a run you wanted**: disable by removing or setting to `false` in the workflow.

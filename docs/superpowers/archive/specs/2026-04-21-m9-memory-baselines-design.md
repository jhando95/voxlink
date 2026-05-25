# Design — Milestone 9: Memory Baselines

**Date:** 2026-04-21
**Status:** Approved (pending spec review)
**Scope:** Extend the existing idle-measurement script to sample RSS in addition to CPU, document target memory baselines in `PERFORMANCE_TARGETS.md`, and add a "Growth since launch" row to the Perf panel so operators can spot slow leaks by eyeballing the trend. No new dependencies. Heap profiling (e.g., `dhat`) is deferred.

## Context

CLAUDE.md priority #1 is "minimal idle CPU and RAM." The project has `perf_metrics` tracking `memory_mb` and `peak_memory_mb` via sysinfo, and the Perf panel already displays those numbers. What's missing:

- Documented numeric targets — no written "the client should use less than X MB at idle".
- Automated sampling alongside CPU — `measure-idle-cpu.sh` doesn't report RSS.
- A leak-detection signal — the displayed memory-since-startup delta is invisible to operators without manual subtraction.

None of these require a heap profiler. They require documentation, a script extension, and a Perf-panel row.

## Goals

1. Document target RSS values for each measurable scenario.
2. Have one reproducible command that samples both CPU and RSS for client + server.
3. Add a visible "growth since launch" signal so operators can notice trends over long sessions.

## Non-goals

- **`dhat` heap profiler integration.** Debugging tool, not a baseline tool. An engineer investigating a real leak will reach for `dhat` or Instruments on-demand.
- **Per-subsystem memory breakdown.** Allocator instrumentation; large scope for low day-to-day value.
- **Automatic leak detection / alerting.** Human in the loop is fine; the UI sub-row is the signal.
- **Jemalloc / MiMalloc swap.** Separate concern, orthogonal to baselines.

## Architecture

Four small pieces.

### 1. Script: `measure-idle-cpu.sh` → `measure-idle.sh`

Rename the script. Keep the same structure. Extend each scenario to print two numbers: CPU% and RSS (in MB). Output format:

```
| Scenario              | CPU % | RSS MB |
|-----------------------|------:|-------:|
| server_zero_peers     |   0.1 |   38   |
| client_home           |   0.8 |  142   |
| server_one_idle_peer  |   0.1 |   41   |
```

Sampling RSS: `ps -o rss= -p <pid>` gives kilobytes; divide by 1024 for MB. Sample every second for the measurement window (30 s default) and report the median — same as CPU.

A thin helper function `sample_rss()` mirrors the existing `sample_cpu()`. The existing `median_cpu()` / `median_rss()` helpers both return the median of 3 × 30 s samples.

### 2. `PERFORMANCE_TARGETS.md` — new Memory section

Append a section after "Idle CPU baselines":

```markdown
## Memory baselines

Measured on Apple M4 Pro, release build. RSS in MB via `ps -o rss=`.

| Scenario | Target RSS | Observed |
|---|--:|--:|
| server_zero_peers | < 50 MB | (run script) |
| server_one_idle_peer | < 60 MB | (run script) |
| client_home | < 150 MB | (run script) |
| client_joined_silent (30 s) | < 180 MB | (manual) |

Reproduce: `./scripts/measure-idle.sh`.

Rationale: Voxlink ships with a neural denoiser + Slint render graph + Rust runtime; a strictly tight floor for the client is ~140 MB before any room activity. The server is tiny (no UI, no audio pipeline) and must fit comfortably inside the Oracle free-tier 1 GB VM alongside the OS — target ~50 MB idle.
```

Operator fills in the "Observed" column after running the script.

### 3. Growth-since-launch in `PerfCollector`

Record `initial_memory_mb: f32` in `PerfCollector::new()` (from the first `sysinfo` snapshot). Compute `memory_growth_mb = memory_mb - initial_memory_mb` in `snapshot()`. Expose as a new field on `PerfSnapshot`.

One gotcha: `PerfCollector::new()` defers creating the `sysinfo::System` object to the first `snapshot()` call (explicit comment in the current code — startup optimization from M7). So the "initial" memory reading needs to be recorded lazily: the first time `snapshot()` runs, store `initial_memory_mb = self.current_mb`. Every subsequent call computes the delta.

### 4. Perf panel row

Extend `PerfData` with `memory-growth-mb: float`. In `update_perf_display`, map `snap.memory_growth_mb → perf.memory-growth-mb`. In `SystemView`, add one row inside the existing memory section:

```
Memory          143 MB
Peak memory     151 MB
Growth          +3 MB     <- new
```

The growth row is green when ≤ +10 MB, amber when +10 to +50, red when > +50. Color hints operators when to worry.

## Components

| File | Change |
|---|---|
| `scripts/measure-idle-cpu.sh` → `scripts/measure-idle.sh` | Rename, add RSS sampling |
| `docs/PERFORMANCE_TARGETS.md` | Rename script reference; add "Memory baselines" section |
| `crates/perf_metrics/src/lib.rs` | `initial_memory_mb: Option<f32>`, snapshot logic, `memory_growth_mb` in output |
| `crates/shared_types/src/state.rs` | `memory_growth_mb: f32` on `PerfSnapshot` |
| `crates/ui_shell/ui/theme.slint` | `memory-growth-mb: float` on `PerfData` |
| `crates/ui_shell/src/lib.rs` | Map snapshot → PerfData |
| `crates/ui_shell/ui/views/system_view.slint` | One new row with color-by-threshold text |

Total code: ~40 LoC across the code files.

## Testing

- Unit: `PerfCollector` records `initial_memory_mb` on first `snapshot()` call, returns 0 for `memory_growth_mb`, and returns positive values after sleeping + allocating.
- Manual verification: run client, confirm Perf panel shows Growth starting at `+0 MB` and staying flat.

## Risks

- **RSS grows on first few snapshots as lazy-init completes.** `sysinfo::System` itself allocates memory. Mitigation: record `initial_memory_mb` on snapshot #2 (not #1), so the initialization cost is captured as baseline. Acceptable tradeoff: snapshot 1 reports growth=0; snapshot 2+ reports real growth. Document in code.
- **Script might double-count processes.** If the user has other `signaling_server` or `app_desktop` processes running, `pgrep` could match them too. Mitigation: the script captures the PID from its own spawn, not via `pgrep`, so this is already correct for automated scenarios.
- **macOS `ps -o rss=` reports pages, not bytes.** Confirmed via man page: actually kilobytes. Divide by 1024 for MB.

## Commit strategy

1. `feat(perf): track memory growth since launch in PerfSnapshot`
2. `feat(ui): render Growth row in Perf panel memory section`
3. `scripts: rename to measure-idle.sh and sample RSS alongside CPU`
4. `docs: memory baselines in PERFORMANCE_TARGETS.md`

Four commits, workspace green at each.

## Success criteria

1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets` ≤ 62 warnings.
3. Existing tests pass; new `PerfCollector` unit test passes.
4. `scripts/measure-idle.sh` exists and produces a CPU+RSS table.
5. `docs/PERFORMANCE_TARGETS.md` has a "Memory baselines" section with the four scenarios.
6. Perf panel shows a "Growth" row beneath existing memory rows.

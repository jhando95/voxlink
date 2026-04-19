#!/usr/bin/env bash
# Run benches against the saved "main" baseline and flag regressions.
# Requires: a previously-saved "main" baseline (run bench-record-baseline.sh).
#
# Usage:
#   ./scripts/bench-check.sh
#
# Exit codes:
#   0 — no regressions
#   1 — one or more benches regressed past criterion's significance threshold
#   2 — no baseline found
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if [ ! -d target/criterion ]; then
    echo "No baseline found. Run scripts/bench-record-baseline.sh first."
    exit 2
fi

LOG=/tmp/voxlink-bench.log
: > "$LOG"

# Only the criterion benches — the default test harness rejects --baseline.
# Format: "<crate>:<bench-name>"
TARGETS=(
    "audio_core:audio_benchmarks"
    "shared_types:protocol"
    "signaling_server:hot_path"
)

for target in "${TARGETS[@]}"; do
    crate="${target%%:*}"
    bench="${target##*:}"
    echo "=== $crate / $bench ===" | tee -a "$LOG"
    cargo bench -p "$crate" --bench "$bench" -- --baseline main 2>&1 | tee -a "$LOG"
done

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

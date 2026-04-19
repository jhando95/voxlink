#!/usr/bin/env bash
# Record the current machine's bench times as the "main" baseline.
# Run this after an intentional perf change, or when setting up on a new machine.
#
# Usage:
#   ./scripts/bench-record-baseline.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# Only the criterion benches — the default test harness rejects
# --save-baseline. Pass --bench to target only the criterion bench target.
# Format: "<crate>:<bench-name>"
TARGETS=(
    "audio_core:audio_benchmarks"
    "shared_types:protocol"
    "signaling_server:hot_path"
)

for target in "${TARGETS[@]}"; do
    crate="${target%%:*}"
    bench="${target##*:}"
    echo "=== $crate / $bench ==="
    cargo bench -p "$crate" --bench "$bench" -- --save-baseline main
done
echo
echo "Baseline saved under target/criterion/<bench-name>/main/"
echo
echo "To check for regressions later, run: scripts/bench-check.sh"

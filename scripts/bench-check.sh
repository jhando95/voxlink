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

# Criterion flags any statistically significant slowdown, but on a busy dev
# laptop even 2-3% bench-to-bench drift is routine machine noise. Filter the
# "Performance has regressed." markers by requiring the reported median
# change to exceed REGRESSION_THRESHOLD_PCT. Real regressions (code changes
# that meaningfully hurt a hot path) easily cross 10-20%.
REGRESSION_THRESHOLD_PCT=10

# Extract regressions with their change percentages. Criterion prints:
#   <bench_name>
#                           time:   [...]
#                           change: [+X% +Y% +Z%] ...
#                           Performance has regressed.
# We grab the Y (median) of the change line preceding each regression marker.
#
# awk scans the log, tracks the last-seen bench name and last change line, and
# emits "<bench> <median_pct>" for each bench that has "Performance has regressed."
# within the following few lines.
regressions=$(awk '
    /^[a-zA-Z_][a-zA-Z0-9_]*$/ { last_bench = $0 }
    /change: \[/ {
        match($0, /\+?-?[0-9.]+%[[:space:]]+\+?-?[0-9.]+%[[:space:]]+\+?-?[0-9.]+%/)
        if (RSTART > 0) {
            split(substr($0, RSTART, RLENGTH), parts, "[[:space:]]+")
            gsub(/[+%]/, "", parts[2])
            last_change = parts[2]
        }
    }
    /Performance has regressed\./ {
        printf "%s %s\n", last_bench, last_change
    }
' "$LOG")

significant=""
while IFS=" " read -r bench pct; do
    [ -z "$bench" ] && continue
    # awk returns e.g. "2.5" or "-1.2". Absolute value, then compare to threshold.
    abs=$(echo "$pct" | awk '{print ($1 < 0 ? -$1 : $1)}')
    over=$(echo "$abs $REGRESSION_THRESHOLD_PCT" | awk '{print ($1 >= $2 ? 1 : 0)}')
    if [ "$over" = "1" ]; then
        significant="${significant}  - ${bench}: +${pct}%\n"
    fi
done <<< "$regressions"

if [ -n "$significant" ]; then
    echo
    echo "REGRESSION DETECTED (change >= ${REGRESSION_THRESHOLD_PCT}%):"
    # -e on echo isn't portable; use printf for the embedded \n.
    printf '%b' "$significant"
    echo
    echo "If this is an expected slowdown, re-record the baseline:"
    echo "  scripts/bench-record-baseline.sh"
    exit 1
fi

echo
if [ -n "$regressions" ]; then
    noisy=$(echo "$regressions" | wc -l | tr -d ' ')
    echo "Benchmarks OK — $noisy bench(es) drifted under the ${REGRESSION_THRESHOLD_PCT}% noise threshold; no meaningful regressions."
else
    echo "Benchmarks OK — no regressions."
fi

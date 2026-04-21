#!/usr/bin/env bash
# Measure idle CPU% and RSS memory for Voxlink client and server across scenarios.
# Prints a markdown table to stdout — suitable for pasting into
# docs/PERFORMANCE_TARGETS.md.
#
# Scenarios this script runs automatically:
#   server_zero_peers     — server up, no clients
#   server_one_idle_peer  — server + one client (client sits on home view)
#   client_home           — client on home view, not in a room
#
# Scenarios this script does NOT automate (UI interaction required):
#   client_joined_silent  — join a room, mute mic
#   client_minimized      — joined + window minimized
# For those, follow the manual procedure at the bottom of this script.
#
# Usage:
#   cargo build --release -p signaling_server -p app_desktop
#   ./scripts/measure-idle-cpu.sh
#
# Requirements:
#   - macOS (uses `top -pid ...`). Linux equivalent is left TBD.
#   - Release binaries built (above).
#   - Loopback port 19090 free.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

SERVER_BIN=target/release/signaling_server
CLIENT_BIN=target/release/app_desktop
PORT=19090
SAMPLE_DURATION=30
SAMPLES_PER_SCENARIO=3

if [ ! -x "$SERVER_BIN" ]; then
    echo "Missing $SERVER_BIN — run: cargo build --release -p signaling_server" >&2
    exit 1
fi
if [ ! -x "$CLIENT_BIN" ]; then
    echo "Missing $CLIENT_BIN — run: cargo build --release -p app_desktop" >&2
    exit 1
fi

TMPDIR=$(mktemp -d)
cleanup() {
    # Kill every child spawned from this shell.
    kill "$(jobs -p)" 2>/dev/null || true
    rm -rf "$TMPDIR"
}
trap cleanup EXIT

# Sample CPU% for PID over SAMPLE_DURATION seconds. Outputs one floating-point
# number: the mean CPU% across the sampling window (skipping the first sample
# which is always 0 or noise).
sample_cpu() {
    local pid=$1
    top -pid "$pid" -l "$((SAMPLE_DURATION + 1))" -stats cpu 2>/dev/null \
        | awk 'NR > 2 && /^[0-9.]+$/ {sum+=$1; n++} END {if (n > 0) printf "%.1f", sum/n; else print "NaN"}'
}

# Take SAMPLES_PER_SCENARIO measurements, print the median.
median_cpu() {
    local pid=$1
    local values=()
    local i
    for _ in $(seq 1 $SAMPLES_PER_SCENARIO); do
        values+=("$(sample_cpu "$pid")")
    done
    printf "%s\n" "${values[@]}" | sort -n \
        | awk -v n=$SAMPLES_PER_SCENARIO 'NR == int((n+1)/2)'
}

# Sample RSS memory (in MB) for PID over SAMPLE_DURATION seconds. Averages
# the per-second samples.
sample_rss() {
    local pid=$1
    local n=0
    local sum=0
    local kb=""
    for _ in $(seq 1 $SAMPLE_DURATION); do
        kb=$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ')
        if [ -n "$kb" ]; then
            sum=$((sum + kb))
            n=$((n + 1))
        fi
        sleep 1
    done
    if [ $n -gt 0 ]; then
        awk -v s=$sum -v n=$n 'BEGIN { printf "%.1f", (s / n) / 1024 }'
    else
        echo "NaN"
    fi
}

# Take SAMPLES_PER_SCENARIO RSS measurements, print the median.
median_rss() {
    local pid=$1
    local values=()
    for _ in $(seq 1 $SAMPLES_PER_SCENARIO); do
        values+=("$(sample_rss "$pid")")
    done
    printf "%s\n" "${values[@]}" | sort -n \
        | awk -v n=$SAMPLES_PER_SCENARIO 'NR == int((n+1)/2)'
}

echo "# Voxlink Idle CPU Measurement"
echo
echo "Generated: $(date '+%Y-%m-%d %H:%M %Z')"
echo "Machine:   $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m)"
echo "Rust:      $(rustc --version 2>/dev/null | head -1)"
echo "Samples:   ${SAMPLES_PER_SCENARIO} × ${SAMPLE_DURATION}s (median reported)"
echo
echo "| Scenario | CPU % | RSS MB |"
echo "|---|--:|--:|"

# --- server_zero_peers ---
PV_ADDR=127.0.0.1:$PORT "$SERVER_BIN" > "$TMPDIR/server.log" 2>&1 &
SERVER_PID=$!
sleep 3    # let server bind + settle

CPU=$(median_cpu "$SERVER_PID")
RSS=$(median_rss "$SERVER_PID")
printf "| server_zero_peers | %s | %s |\n" "$CPU" "$RSS"

# --- client_home ---
VOXLINK_SERVER=ws://127.0.0.1:$PORT "$CLIENT_BIN" > "$TMPDIR/client1.log" 2>&1 &
CLIENT1_PID=$!
sleep 6    # let client window open + connect

CPU=$(median_cpu "$CLIENT1_PID")
RSS=$(median_rss "$CLIENT1_PID")
printf "| client_home | %s | %s |\n" "$CPU" "$RSS"

# --- server_one_idle_peer ---
# Same client1 is still connected; re-sample the server.
CPU=$(median_cpu "$SERVER_PID")
RSS=$(median_rss "$SERVER_PID")
printf "| server_one_idle_peer | %s | %s |\n" "$CPU" "$RSS"

# --- manual scenarios ---
echo "| client_joined_silent | (manual — see below) | (manual) |"
echo "| client_minimized | (manual — see below) | (manual) |"
echo
echo "## Manual scenarios"
echo
echo "To measure \`client_joined_silent\` and \`client_minimized\`:"
echo
echo "1. Launch server + client manually:"
echo "   \`\`\`"
echo "   PV_ADDR=127.0.0.1:$PORT $SERVER_BIN &"
echo "   VOXLINK_SERVER=ws://127.0.0.1:$PORT $CLIENT_BIN &"
echo "   \`\`\`"
echo "2. In the client UI, create or join any room, then mute the mic."
echo "3. For client_joined_silent: leave the window visible."
echo "4. For client_minimized: command-H (macOS) to hide the window."
echo "5. In another terminal:"
echo "   \`\`\`"
echo "   PID=\$(pgrep -x app_desktop)"
echo "   for _ in 1 2 3; do"
echo "     top -pid \$PID -l 31 -stats cpu | awk 'NR > 2 && /^[0-9.]+\$/ {sum+=\$1; n++} END {if (n > 0) printf \"cpu=%.1f%% \", sum/n}'"
echo "     kb=\$(ps -o rss= -p \$PID | tr -d ' ')"
echo "     awk -v kb=\$kb 'BEGIN { printf \"rss=%.1f MB\\n\", kb/1024 }'"
echo "   done | sort -k1 -t= | sed -n '2p'   # median of 3"
echo "   \`\`\`"
echo
echo "6. Kill both processes when done."

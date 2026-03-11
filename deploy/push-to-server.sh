#!/bin/bash
# ============================================================
# Push Voxlink server to Oracle Cloud VM and build remotely
# ============================================================
#
# Run this FROM YOUR MAC to deploy/update the server.
#
# Usage:
#   ./deploy/push-to-server.sh <user>@<server-ip>
#
# Example:
#   ./deploy/push-to-server.sh ubuntu@129.146.123.45
#   ./deploy/push-to-server.sh opc@129.146.123.45
#
# First run: uploads source + builds + installs (~5 min)
# Updates:   uploads changes + rebuilds + restarts (~1 min)
# ============================================================

set -e

if [ -z "$1" ]; then
    echo "Usage: $0 <user>@<server-ip>"
    echo "Example: $0 ubuntu@129.146.123.45"
    exit 1
fi

SERVER="$1"
REMOTE_DIR="~/voxlink"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

echo ""
echo "Deploying Voxlink server to $SERVER..."
echo ""

# Create remote directory structure
ssh "$SERVER" "mkdir -p ${REMOTE_DIR}/crates/shared_types ${REMOTE_DIR}/crates/signaling_server"

# Upload only what's needed for the server build
echo "[1/3] Uploading source code..."
rsync -avz --progress \
    "${PROJECT_DIR}/crates/shared_types/" \
    "${SERVER}:${REMOTE_DIR}/crates/shared_types/"

rsync -avz --progress \
    "${PROJECT_DIR}/crates/signaling_server/" \
    "${SERVER}:${REMOTE_DIR}/crates/signaling_server/"

# Use server-only workspace Cargo.toml (no desktop GUI deps)
rsync -avz --progress \
    "${SCRIPT_DIR}/Cargo.toml" \
    "${SERVER}:${REMOTE_DIR}/Cargo.toml"

# Upload Cargo.lock for reproducible builds
rsync -avz --progress \
    "${PROJECT_DIR}/Cargo.lock" \
    "${SERVER}:${REMOTE_DIR}/Cargo.lock"

# Upload setup script
rsync -avz --progress \
    "${SCRIPT_DIR}/setup-server.sh" \
    "${SERVER}:${REMOTE_DIR}/setup-server.sh"

echo ""
echo "[2/3] Building and installing on server..."
ssh "$SERVER" "cd ${REMOTE_DIR} && chmod +x setup-server.sh && ./setup-server.sh"

echo ""
echo "[3/3] Done!"
echo ""

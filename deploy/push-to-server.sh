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

TLS_DOMAIN=""
SERVER=""

while [ $# -gt 0 ]; do
    case "$1" in
        --tls)
            if [ -z "${2:-}" ]; then
                echo "ERROR: --tls requires a domain argument."
                exit 2
            fi
            TLS_DOMAIN="$2"
            shift 2
            ;;
        --tls=*)
            TLS_DOMAIN="${1#--tls=}"
            shift
            ;;
        -*)
            echo "ERROR: unknown flag: $1"
            exit 2
            ;;
        *)
            if [ -z "$SERVER" ]; then
                SERVER="$1"
            else
                echo "ERROR: unexpected positional argument: $1"
                exit 2
            fi
            shift
            ;;
    esac
done

if [ -z "$SERVER" ]; then
    echo "Usage: $0 [--tls <domain>] <user>@<server-ip>"
    echo "Example:"
    echo "  $0 ubuntu@129.146.123.45"
    echo "  $0 --tls voice.example.com ubuntu@129.146.123.45"
    exit 1
fi
REMOTE_DIR="~/voxlink"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Use oracle SSH key if available
SSH_KEY=""
if [ -f "$HOME/.ssh/oracle_key" ]; then
    SSH_KEY="-i $HOME/.ssh/oracle_key"
fi
export RSYNC_RSH="ssh $SSH_KEY"

echo ""
echo "Deploying Voxlink server to $SERVER..."
echo ""

# Create remote directory structure
ssh $SSH_KEY "$SERVER" "mkdir -p ${REMOTE_DIR}/crates/shared_types ${REMOTE_DIR}/crates/signaling_server"

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
ssh $SSH_KEY "$SERVER" "cd ${REMOTE_DIR} && chmod +x setup-server.sh && ./setup-server.sh"

echo ""
echo "[3/3] Done!"

if [ -n "$TLS_DOMAIN" ]; then
    echo
    echo "=== Configuring TLS for $TLS_DOMAIN ==="
    scp "$SCRIPT_DIR/setup-tls.sh" "$SERVER:/tmp/voxlink-setup-tls.sh"
    ssh "$SERVER" "chmod +x /tmp/voxlink-setup-tls.sh && sudo /tmp/voxlink-setup-tls.sh '$TLS_DOMAIN'"
fi

echo ""

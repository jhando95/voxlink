#!/usr/bin/env bash
# ============================================================
# Voxlink TLS setup — provision Let's Encrypt cert, wire renewal,
# update systemd unit, restart voxlink.
#
# Run ON THE SERVER (not locally). push-to-server.sh calls this
# remotely when invoked with --tls <domain>.
#
# Usage:
#   sudo bash setup-tls.sh <domain> [<email>]
#
# Idempotent: re-running with the same domain does nothing if a
# cert already exists and voxlink is already configured.
# ============================================================

set -euo pipefail

DOMAIN="${1:-}"
EMAIL="${2:-admin@$DOMAIN}"

if [ -z "$DOMAIN" ]; then
    echo "Usage: $0 <domain> [<email>]"
    exit 2
fi

echo "Voxlink TLS setup for $DOMAIN (contact: $EMAIL)"
echo

# --- Preflight: does DNS resolve to us? ---
PUBLIC_IP="$(curl -s --max-time 5 https://ifconfig.me || true)"
RESOLVED="$(getent hosts "$DOMAIN" | awk '{print $1}' | head -n1 || true)"

if [ -z "$RESOLVED" ]; then
    echo "ERROR: DNS for $DOMAIN does not resolve."
    echo "Point an A record to this server (public IP: ${PUBLIC_IP:-unknown}) first."
    echo "See docs/TLS_SETUP.md for details."
    exit 1
fi

if [ -n "$PUBLIC_IP" ] && [ "$RESOLVED" != "$PUBLIC_IP" ]; then
    echo "WARNING: DNS for $DOMAIN resolves to $RESOLVED,"
    echo "         but this server's public IP is $PUBLIC_IP."
    echo "Continuing anyway — this is fine if you're behind a load balancer,"
    echo "but if certbot fails with an HTTP-01 challenge timeout, check your DNS."
    echo
fi

# --- Install certbot ---
if ! command -v certbot >/dev/null 2>&1; then
    echo "Installing certbot..."
    if command -v apt-get >/dev/null 2>&1; then
        sudo apt-get update -y
        sudo apt-get install -y certbot
    elif command -v dnf >/dev/null 2>&1; then
        sudo dnf install -y certbot
    else
        echo "ERROR: don't know how to install certbot on this distro."
        exit 1
    fi
fi

# --- Acquire cert (skip if already exists) ---
CERT_PATH="/etc/letsencrypt/live/$DOMAIN/fullchain.pem"
KEY_PATH="/etc/letsencrypt/live/$DOMAIN/privkey.pem"

if [ -f "$CERT_PATH" ]; then
    echo "Cert for $DOMAIN already exists at $CERT_PATH — skipping acquisition."
else
    echo "Acquiring Let's Encrypt cert for $DOMAIN..."
    # Stop voxlink briefly in case it's somehow holding port 80 (it shouldn't be).
    sudo systemctl stop voxlink.service 2>/dev/null || true
    sudo certbot certonly --standalone \
        --non-interactive --agree-tos \
        --email "$EMAIL" \
        -d "$DOMAIN"
fi

# --- Make certs readable by the nobody service user ---
echo "Applying ACLs so voxlink (runs as nobody) can read the cert..."
if ! command -v setfacl >/dev/null 2>&1; then
    sudo apt-get install -y acl
fi
sudo setfacl -R -m u:nobody:rx /etc/letsencrypt/live /etc/letsencrypt/archive

# --- Renewal deploy-hook ---
HOOK=/etc/letsencrypt/renewal-hooks/deploy/voxlink.sh
echo "Installing renewal deploy-hook at $HOOK..."
sudo mkdir -p "$(dirname "$HOOK")"
sudo tee "$HOOK" > /dev/null << 'HOOK_EOF'
#!/bin/sh
# Voxlink post-renewal hook: re-apply ACLs, restart service.
set -e
setfacl -R -m u:nobody:rx /etc/letsencrypt/live /etc/letsencrypt/archive
systemctl restart voxlink.service
HOOK_EOF
sudo chmod 0755 "$HOOK"

# --- Update voxlink.service with cert paths ---
# Using systemd's drop-in directory so we don't edit the main unit.
DROPIN_DIR=/etc/systemd/system/voxlink.service.d
DROPIN_FILE="$DROPIN_DIR/tls.conf"
echo "Writing systemd drop-in at $DROPIN_FILE..."
sudo mkdir -p "$DROPIN_DIR"
sudo tee "$DROPIN_FILE" > /dev/null << UNIT_EOF
[Service]
Environment=PV_CERT=$CERT_PATH
Environment=PV_KEY=$KEY_PATH
UNIT_EOF

sudo systemctl daemon-reload
sudo systemctl restart voxlink.service

# --- Verify ---
sleep 2
if sudo systemctl is-active voxlink.service >/dev/null 2>&1; then
    echo
    echo "Voxlink restarted with TLS enabled."
    echo "Cert path: $CERT_PATH"
    echo "Expires:   $(sudo openssl x509 -in "$CERT_PATH" -noout -enddate | cut -d= -f2)"
    echo
    echo "Test from a client: wss://$DOMAIN:9090"
else
    echo "ERROR: voxlink failed to start after TLS setup. Check:"
    echo "  sudo journalctl -u voxlink.service -n 100"
    exit 1
fi

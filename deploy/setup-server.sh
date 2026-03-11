#!/bin/bash
# ============================================================
# Voxlink Server — Oracle Cloud Deployment Script
# ============================================================
#
# Run this ON the Oracle Cloud VM after SSH'ing in.
# It installs Rust, builds the server, and sets up auto-start.
#
# Usage:
#   chmod +x setup-server.sh
#   ./setup-server.sh
#
# After running, the server starts automatically on boot.
# ============================================================

set -e

echo ""
echo "========================================"
echo "  Voxlink Server Setup"
echo "========================================"
echo ""

# --- Create swap if needed (free-tier VMs have only 1GB RAM) ---
if [ ! -f /swapfile ]; then
    echo "[0] Creating 2GB swap (prevents OOM during build)..."
    sudo fallocate -l 2G /swapfile
    sudo chmod 600 /swapfile
    sudo mkswap /swapfile
    sudo swapon /swapfile
    echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab > /dev/null
    echo "  Swap enabled (2GB)"
    echo ""
fi

# --- Install Rust (if not present) ---
if ! command -v cargo &> /dev/null; then
    echo "[1/4] Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
    echo "  Rust $(rustc --version) installed"
else
    source "$HOME/.cargo/env" 2>/dev/null || true
    echo "[1/4] Rust already installed: $(rustc --version)"
fi
echo ""

# --- Install build dependencies ---
echo "[2/4] Installing build dependencies..."
if command -v apt-get &> /dev/null; then
    sudo apt-get update -qq
    sudo apt-get install -y -qq build-essential pkg-config cmake > /dev/null 2>&1
    echo "  Dependencies installed (apt)"
elif command -v dnf &> /dev/null; then
    sudo dnf install -y gcc gcc-c++ make cmake pkg-config > /dev/null 2>&1
    echo "  Dependencies installed (dnf)"
else
    echo "  WARNING: Unknown package manager. Ensure gcc, cmake, pkg-config are installed."
fi
echo ""

# --- Build the server ---
echo "[3/4] Building signaling server (release mode)..."
echo "  This may take a few minutes on a free-tier VM..."

# Only build the server binary (not the desktop app which needs GUI libs)
cargo build --release --bin signaling_server

BINARY="target/release/signaling_server"
if [ ! -f "$BINARY" ]; then
    echo "ERROR: Build failed — binary not found"
    exit 1
fi

SIZE=$(du -h "$BINARY" | cut -f1)
echo "  Built: $BINARY ($SIZE)"
echo ""

# --- Install binary and systemd service ---
echo "[4/4] Installing server..."

# Copy binary to /opt
sudo mkdir -p /opt/voxlink
sudo cp "$BINARY" /opt/voxlink/signaling_server
sudo chmod +x /opt/voxlink/signaling_server

# Create systemd service
sudo tee /etc/systemd/system/voxlink.service > /dev/null << 'UNIT'
[Unit]
Description=Voxlink Signaling Server
After=network.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/opt/voxlink/signaling_server
Environment=PV_ADDR=0.0.0.0:9090
Environment=RUST_LOG=info
Restart=always
RestartSec=3
# Run as non-root for security
User=nobody
Group=nogroup
# Harden the service
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true

[Install]
WantedBy=multi-user.target
UNIT

sudo systemctl daemon-reload
sudo systemctl enable voxlink
sudo systemctl restart voxlink

echo ""

# --- Open firewall port ---
# Oracle Linux / Ubuntu iptables
if command -v iptables &> /dev/null; then
    # Check if rule already exists
    if ! sudo iptables -C INPUT -p tcp --dport 9090 -j ACCEPT 2>/dev/null; then
        # Insert BEFORE the REJECT rule (position 5 on Oracle Cloud Ubuntu)
        sudo iptables -I INPUT 5 -p tcp --dport 9090 -j ACCEPT
        echo "  Firewall: opened TCP 9090 (iptables)"
    else
        echo "  Firewall: TCP 9090 already open (iptables)"
    fi
    # Persist the rule
    if command -v netfilter-persistent &> /dev/null; then
        sudo netfilter-persistent save 2>/dev/null || true
    elif [ -f /etc/iptables/rules.v4 ]; then
        sudo iptables-save | sudo tee /etc/iptables/rules.v4 > /dev/null
    fi
fi
echo ""

# --- Verify ---
sleep 2
if systemctl is-active --quiet voxlink; then
    echo "========================================"
    echo "  Voxlink Server is RUNNING"
    echo "========================================"
    echo ""
    # Get public IP
    PUBLIC_IP=$(curl -s ifconfig.me 2>/dev/null || curl -s icanhazip.com 2>/dev/null || echo "<your-server-ip>")
    echo "  Server address for clients:"
    echo ""
    echo "    ws://${PUBLIC_IP}:9090"
    echo ""
    echo "  Paste this into Voxlink > Server > Connect"
    echo ""
    echo "  IMPORTANT: You still need to open port 9090"
    echo "  in your Oracle Cloud Security List (see below)."
    echo ""
    echo "  Useful commands:"
    echo "    sudo systemctl status voxlink    # Check status"
    echo "    sudo journalctl -u voxlink -f    # View live logs"
    echo "    sudo systemctl restart voxlink   # Restart"
    echo "    sudo systemctl stop voxlink      # Stop"
    echo ""
else
    echo "WARNING: Server failed to start. Check logs:"
    echo "  sudo journalctl -u voxlink -e"
fi

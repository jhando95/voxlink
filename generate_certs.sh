#!/bin/bash
# Generate self-signed TLS certificates for Voxlink signaling server.
# Usage: ./generate_certs.sh [domain_or_ip]
#
# The server reads PV_CERT and PV_KEY env vars to enable TLS:
#   PV_CERT=certs/server.crt PV_KEY=certs/server.key cargo run --release -p signaling_server

set -e

DOMAIN="${1:-localhost}"
CERT_DIR="certs"
DAYS=365

mkdir -p "$CERT_DIR"

openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$CERT_DIR/server.key" \
  -out "$CERT_DIR/server.crt" \
  -days "$DAYS" \
  -subj "/CN=$DOMAIN" \
  -addext "subjectAltName=DNS:$DOMAIN,IP:127.0.0.1"

echo "Certificates generated in $CERT_DIR/"
echo ""
echo "To run the server with TLS:"
echo "  PV_CERT=$CERT_DIR/server.crt PV_KEY=$CERT_DIR/server.key cargo run --release -p signaling_server"
echo ""
echo "Clients connect via: wss://$DOMAIN:9090"

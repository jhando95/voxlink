# Voxlink TLS Setup

This guide walks through enabling `wss://` on a public Voxlink server using a free Let's Encrypt certificate. If you're running Voxlink only on a LAN, see the "Self-signed" section at the bottom.

## Prerequisites

1. **A domain name** (e.g., `voice.example.com`) with an A record pointing to your server's public IP.
2. **Port 80 open** to the public internet on the server. Certbot's HTTP-01 challenge connects on port 80. If you're on Oracle Cloud, open port 80 in the VCN security group.
3. **SSH access** to the server as a user who can `sudo`.
4. **Voxlink already deployed** (run `deploy/setup-server.sh` first or use `push-to-server.sh` without `--tls`).

## One command to enable TLS

From your local checkout:

```
./deploy/push-to-server.sh --tls voice.example.com ubuntu@<server-ip>
```

The script uploads source, builds on the server, then runs `setup-tls.sh`:

1. Installs `certbot` if missing.
2. Acquires a cert for the domain.
3. Sets ACLs so the `nobody` service user can read the cert.
4. Installs a renewal hook that re-applies ACLs and restarts Voxlink.
5. Writes a systemd drop-in at `/etc/systemd/system/voxlink.service.d/tls.conf` with `PV_CERT` / `PV_KEY`.
6. Restarts Voxlink.

After it finishes, connect from a client at `wss://voice.example.com:9090`.

## Verifying

On the server:

```
sudo journalctl -u voxlink.service -n 30 --no-pager
```

You should see:

```
TLS enabled (cert: /etc/letsencrypt/live/<domain>/fullchain.pem, key: ...)
TLS cert for CN=<domain> expires in 89 days
Signaling server listening on wss://0.0.0.0:9090
```

From your local machine:

```
openssl s_client -connect voice.example.com:9090 -servername voice.example.com </dev/null 2>/dev/null | openssl x509 -noout -issuer -subject -dates
```

You should see the Let's Encrypt issuer chain and "not after" date ~89-90 days out.

## Renewal

Certbot installs `certbot.timer` by default; it runs twice daily and renews certs with ≤30 days remaining. The deploy-hook at `/etc/letsencrypt/renewal-hooks/deploy/voxlink.sh` re-applies ACLs and restarts Voxlink on each successful renewal. Voice calls are interrupted for ~1 second during the restart.

To force a renewal for testing (only works if the cert is eligible — rate-limited):

```
sudo certbot renew --force-renewal
```

To see upcoming renewals:

```
sudo systemctl list-timers certbot.timer
```

## Troubleshooting

### DNS for `<domain>` does not resolve
Check that the A record is set and DNS has propagated. `dig +short <domain>` should return the server's public IP.

### Port 80 must be reachable
Check (from your local machine): `curl -v http://<domain>` — you should at least get a connection refused or a response from whatever is listening on port 80, not a hang. If you get a hang, port 80 is firewalled upstream (cloud provider security group).

On Oracle Cloud: VCN → Security Lists → Default Security List → add ingress rule for 0.0.0.0/0 TCP 80.

### Voxlink fails to start after setup
```
sudo journalctl -u voxlink.service -n 100 --no-pager
```

Common causes:
- **Permission denied on cert**: re-run `sudo setfacl -R -m u:nobody:rx /etc/letsencrypt/live /etc/letsencrypt/archive`.
- **Cert path wrong**: check the drop-in: `sudo cat /etc/systemd/system/voxlink.service.d/tls.conf`.
- **Port 9090 already in use**: `sudo ss -tlnp | grep 9090`.

### Client says "server's TLS certificate is invalid or expired"
If you're using Let's Encrypt, the cert is trusted by every platform automatically. This error usually means:
- Your client clock is wrong. `date` on your laptop.
- The server is serving a stale cert because it didn't restart after renewal. `sudo systemctl restart voxlink.service`.
- The domain in the URL doesn't match the cert. Check the URL has the exact hostname the cert was issued for.

## Self-signed (LAN / dev only)

For LAN deployments without a public hostname:

```
# On the server:
./generate_certs.sh voxlink.local
sudo cp voxlink.local.crt /etc/voxlink/cert.pem
sudo cp voxlink.local.key /etc/voxlink/key.pem
sudo setfacl -m u:nobody:r /etc/voxlink/key.pem
```

Then set `PV_CERT=/etc/voxlink/cert.pem` and `PV_KEY=/etc/voxlink/key.pem` in the voxlink systemd drop-in.

Clients will reject the self-signed cert by default. You can either:
- Install the generated `voxlink.local.crt` as a trusted root on each client machine.
- Or use `PV_ALLOW_INSECURE=1` on the server and let clients connect over plain `ws://` for LAN-only use.

The client surfaces a specific "TLS certificate is invalid or expired" error when it sees an untrusted cert, so users don't get confused by a generic connection failure.

# Design — Milestone 2: Production TLS for Public Server Deployment

**Date:** 2026-04-17
**Status:** Approved (pending spec review)
**Scope:** Make `wss://` deployments first-class. Automate Let's Encrypt provisioning and renewal. Keep self-signed as a fallback for LAN/dev.

## Context

Voxlink already has most of the TLS plumbing:

- Server: `crates/signaling_server/src/tls.rs` defines `ServerStream::{Plain,Tls}`, `load_tls_config`, `bind_requires_tls`, `allow_insecure_public_bind`. `main.rs` reads `PV_CERT` / `PV_KEY` env vars.
- Client: `crates/net_control` depends on `tokio-tungstenite` with the `rustls-tls-native-roots` feature, so `wss://` URLs already work against any cert chain the system trusts.
- Repo root has `generate_certs.sh` for dev self-signed certs.

What's missing:

1. Cert provisioning automation for the public server — operators do it by hand today.
2. Renewal — certbot's default timer runs, but nothing reloads the server after renewal.
3. Docs — no `TLS_SETUP.md`.
4. Integration test covering the TLS path.
5. Observability — server doesn't log cert expiry at startup, so a cert with 3 days left looks identical to a fresh one.
6. User-facing errors — a TLS handshake failure on the client currently reads as "connection closed".

Milestone 1 (signaling_server / shared_types split) is complete. This milestone ships on top of `refactor/m1-split`.

## Goals

1. One-command TLS provisioning for a new Oracle Cloud deployment via `./deploy/push-to-server.sh --tls <domain>`.
2. Automatic renewal that reloads the server without manual intervention.
3. Clear operator docs and troubleshooting.
4. End-to-end integration test that proves the TLS path actually works.
5. Surface cert health in server logs.
6. Translate TLS-handshake errors into actionable client-side messages.

## Non-goals

- **Auto-update binary signature verification.** Deferred until a download-and-apply flow exists; signing something nobody downloads is wasted work.
- **DTLS / SRTP for the UDP audio lane.** Bigger milestone on its own.
- **Mutual TLS / client certificates.**
- **Certificate pinning.**
- **DNS-01 challenges** (wildcard certs, DNS-API integration). Standalone HTTP-01 is simpler and covers the deployment model Voxlink actually uses.
- **In-process SIGHUP cert reload.** A 1-second `systemctl restart` every ~60 days is acceptable for a voice app. Adding hot-reload machinery (ArcSwap, signal handling, tests) would be YAGNI.

## Architecture

### Cert acquisition

`certbot --standalone` with the HTTP-01 challenge. Requires:

- A DNS hostname pointing at the VM's IPv4 address.
- Port 80 reachable from the public internet during acquisition.

Certbot writes cert + key to `/etc/letsencrypt/live/<domain>/{fullchain,privkey}.pem`. These paths become `PV_CERT` / `PV_KEY` in the systemd service.

### Permissions

Voxlink runs as `nobody:nogroup`. By default, `/etc/letsencrypt/{live,archive}/<domain>/` is `root:root` 0700 and the private key is 0600. Fix with a one-time `setfacl`:

```
setfacl -R -m u:nobody:rx /etc/letsencrypt/live /etc/letsencrypt/archive
```

After each renewal, certbot overwrites those directories, dropping the ACL. Re-apply via a deploy-hook (below).

### Renewal

Certbot installs a systemd timer (`certbot.timer`) that runs `certbot renew` twice daily. A cert that's ≤30 days from expiry gets renewed; otherwise the run is a no-op.

Add `/etc/letsencrypt/renewal-hooks/deploy/voxlink.sh`:

```bash
#!/bin/sh
# Runs after each successful renewal.
setfacl -R -m u:nobody:rx /etc/letsencrypt/live /etc/letsencrypt/archive
systemctl restart voxlink.service
```

The `systemctl restart` interrupts active WebSocket and UDP sessions for ~1 second. Acceptable because renewals happen roughly every 60 days on a 90-day cert.

### Self-signed fallback

If no domain is provided, the setup path falls back to generating a self-signed cert via the existing `generate_certs.sh`. The server logs a warning at startup:

```
TLS enabled with self-signed certificate at <path>. Clients will need to trust this cert manually or use PV_ALLOW_INSECURE=1.
```

The client, when seeing a TLS handshake error against a self-signed server, surfaces a specific message (see *Client error handling* below).

### Cert expiry observability

At startup, after `load_tls_config` succeeds, parse the not-after field of the leaf cert and log:

```
TLS cert for <CN/SAN> expires in N days (at <ISO-8601>)
```

If N ≤ 14, log at WARN; if N ≤ 0, log at ERROR and refuse to start. Uses `rustls-pki-types` (already a transitive dep) + a small ASN.1 parse, or a lightweight helper using `x509-parser`. Choose `x509-parser` — it's a stable, well-tested crate and avoids rolling ASN.1 by hand.

### Client error handling

When `net_control::Client::connect` fails, inspect the error chain. If any cause contains `"InvalidCertificate"`, `"UnknownCa"`, `"Expired"`, `"NotValidForName"`, or `"BadCertificate"`, surface a dedicated error variant (`ConnectError::TlsHandshake(reason: String)`) that `signal_handler::connection` renders as a toast:

> "Could not connect: server's TLS certificate is invalid or expired. Ask the server operator to run `./deploy/setup-tls.sh`."

Otherwise keep the existing generic error path.

## Components

| File / area | Create / Modify | Purpose |
|---|---|---|
| `deploy/setup-tls.sh` | Create | One-shot: install certbot, acquire cert, set ACLs, install deploy-hook, update `voxlink.service` env, restart service. Idempotent. |
| `deploy/push-to-server.sh` | Modify | Add `--tls <domain>` flag. When present, after uploading and building, run `setup-tls.sh <domain>` over SSH if not already configured. |
| `deploy/voxlink.service.template` | Modify | Add placeholders for `PV_CERT` / `PV_KEY`, left empty by default. `setup-tls.sh` substitutes values. |
| `crates/signaling_server/Cargo.toml` | Modify | Add `x509-parser` dep. |
| `crates/signaling_server/src/tls.rs` | Modify | New `pub(crate) fn cert_expiry(path: &str) -> Result<(String, i64)>` returning (subject, days_until_expiry). Call it from `load_tls_config`; log the expiry line. |
| `crates/signaling_server/src/main.rs` | Modify | At startup after TLS enabled, call the expiry logger. Refuse to start if expired. |
| `crates/net_control/src/error.rs` *(new or extend existing)* | Create/Modify | Add `ConnectError::TlsHandshake(String)` variant. |
| `crates/net_control/src/lib.rs` | Modify | Classify connection errors by inspecting error chain. |
| `crates/app_desktop/src/signal_handler/connection.rs` | Modify | Render the new `TlsHandshake` variant as a specific toast. |
| `crates/integration_tests/tests/tls_test.rs` | Create | Integration test: generate self-signed via `rcgen`, launch server with TLS, connect `wss://` client that trusts the generated root, send a ping, assert round trip. |
| `crates/integration_tests/Cargo.toml` | Modify | Add `rcgen` dev-dep. |
| `docs/TLS_SETUP.md` | Create | Operator walkthrough. |
| `docs/ARCHITECTURE.md` | Modify | Update the "Security / transport" section to reference TLS_SETUP.md. |

## Data flow

**Provisioning (one-time per domain):**

1. Operator runs locally: `./deploy/push-to-server.sh --tls voice.example.com ubuntu@<ip>`
2. Script uploads source, builds server remotely (existing behavior), then runs `setup-tls.sh voice.example.com` over SSH.
3. `setup-tls.sh` checks whether `/etc/letsencrypt/live/<domain>/fullchain.pem` exists. If not:
   - `apt-get install -y certbot`
   - `systemctl stop voxlink` (release port 80 if voxlink is using it — it's not by default but be safe)
   - `certbot certonly --standalone --non-interactive --agree-tos --email <op-email> -d <domain>`
   - `setfacl -R -m u:nobody:rx /etc/letsencrypt/{live,archive}`
   - Install deploy-hook at `/etc/letsencrypt/renewal-hooks/deploy/voxlink.sh`, `chmod 0755`
4. Write `PV_CERT=/etc/letsencrypt/live/<domain>/fullchain.pem` and `PV_KEY=/etc/letsencrypt/live/<domain>/privkey.pem` into the systemd unit (via `systemctl edit voxlink` or by regenerating the unit from template).
5. `systemctl restart voxlink`.
6. Script verifies via `curl -v https://<domain>:<port>/` (or a TLS handshake probe) that the server responds with a TLS handshake. Exits 0 on success.

**Steady state:**

- `certbot.timer` fires at 00:00 and 12:00. On renewal, deploy-hook runs, re-applies ACLs, restarts voxlink.
- Voxlink startup parses cert expiry, logs it. journalctl shows days remaining.

**Client:**

- User enters `wss://voice.example.com:<port>`. Tungstenite handshake succeeds via system root certs (Let's Encrypt ISRG Root X1 is trusted by default on every platform).
- If cert is invalid/expired/self-signed-untrusted, the connection error is classified and a specific toast shown.

## Error handling

| Scenario | Detection | Behavior |
|---|---|---|
| DNS doesn't resolve | `certbot` fails | `setup-tls.sh` prints "DNS for <domain> does not resolve to this server's public IP. See docs/TLS_SETUP.md §Prerequisites." Exits non-zero. Does not modify systemd unit. |
| Port 80 blocked | `certbot` fails with HTTP-01 timeout | Similar: "Port 80 must be reachable from the public internet for HTTP-01 challenge. See docs/TLS_SETUP.md §Troubleshooting." |
| Certbot not installable | `apt-get` fails | Print apt-get output, exit non-zero. |
| Cert exists but unreadable by nobody | `load_tls_config` fails at startup | Clear error: "Cannot read <path>: permission denied. Run `sudo setfacl -R -m u:nobody:rx /etc/letsencrypt`." |
| Cert expired | New `cert_expiry` check | Refuse to start. Error: "Certificate expired on <date>. Run `sudo certbot renew`." |
| Cert near expiry (≤14d) | Same check | Log WARN. Start normally. |
| Client sees TLS handshake error | Error chain inspection in `net_control` | `ConnectError::TlsHandshake(reason)` → dedicated UI toast. |

## Testing

### New integration test

`crates/integration_tests/tests/tls_test.rs`:

```text
- setup: generate self-signed root CA + leaf cert for "localhost" via rcgen
- write cert + key to tempdir
- set PV_CERT, PV_KEY env and launch signaling_server on a free port in a task
- configure a tungstenite client with a root store containing the generated root CA
- connect wss://localhost:<port>
- send a SignalMessage::Hello, await Welcome (or whatever the current handshake is)
- assert round trip works
- shut down server task
```

Uses the same test-server-harness pattern as existing `server_tests.rs`.

### Existing tests

All existing unit and integration tests must still pass. `cargo clippy` warnings count must not increase.

### Manual verification (for the operator doc)

- Fresh VM, DNS pointed at it, run the deploy command, verify `wss://` works from macOS client.
- Simulate renewal with `certbot renew --force-renewal` and confirm the deploy-hook runs and voxlink restarts.

## Risks

- **Port 80 collision.** If anything else is listening on port 80, `certbot --standalone` fails. Voxlink itself doesn't bind 80. The setup script explicitly stops voxlink before running certbot (harmless, voxlink doesn't use 80). Other processes are the operator's problem — surfaced via a clear error message.
- **Rate limit on Let's Encrypt.** 5 certs per week per registered domain. `setup-tls.sh` skips acquisition if a cert already exists. Re-runs are safe.
- **`nobody` user variance.** Some distros use `nogroup`, others `nobody`. The script uses `id -u nobody` to resolve the UID dynamically.
- **The `systemctl restart` blip.** A voice call loses ~1 second of audio every ~60 days. Documented as known; mitigated by running renewals during low-traffic hours (0:00 UTC default is fine for a small deployment).
- **Oracle Cloud security group blocks 80.** Requires the operator to open port 80 in Oracle's console. Documented in TLS_SETUP.md prerequisites.

## Commit strategy

Small commits, workspace compileable at every one:

1. `feat(tls): parse cert expiry and log at startup` (+ `x509-parser` dep, helper in `tls.rs`)
2. `feat(net_control): classify TLS handshake errors`
3. `feat(ui): surface TLS-handshake errors with actionable toast`
4. `test(tls): integration test for wss:// round trip` (+ `rcgen` dev-dep)
5. `deploy: add setup-tls.sh for Let's Encrypt provisioning`
6. `deploy: wire --tls flag into push-to-server.sh`
7. `docs: add TLS_SETUP.md and cross-link from ARCHITECTURE.md`

## Success criteria

1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets` — no new warnings vs M1 baseline (62).
3. All tests that pass on M1 still pass. The new `tls_test` passes.
4. `./deploy/push-to-server.sh --tls <domain> ubuntu@<ip>` works end-to-end on a fresh VM with DNS + port 80 available.
5. Server startup log shows `TLS cert for <domain> expires in N days`.
6. Client with `wss://<domain>:<port>` connects successfully.
7. Client with an intentionally broken cert sees the dedicated TLS error toast, not a generic failure.
8. Certbot timer with `--force-renewal` triggers a successful renewal and service restart.

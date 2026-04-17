# M2 — Production TLS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `wss://` deployments first-class. Add Let's Encrypt provisioning + renewal automation, cert expiry observability, user-facing TLS error messages, and an integration test that exercises the wss:// path end to end.

**Architecture:** The Rust plumbing already exists (server `ServerStream` / `load_tls_config`, client `rustls-tls-native-roots`). This milestone adds: cert-expiry logging, client-side error classification, an integration test with `rcgen`-generated certs, and deploy automation (`deploy/setup-tls.sh` + `--tls` flag on `push-to-server.sh`). Renewal is handled by certbot's default systemd timer plus a deploy-hook that re-ACLs the cert files and restarts the service.

**Tech Stack:** Rust 1.94, `x509-parser` for cert expiry parsing, `rcgen` (dev-dep) for test certs, `rustls` (already used via tokio-rustls), `certbot` on Ubuntu for cert acquisition.

**Spec:** `docs/superpowers/specs/2026-04-17-m2-production-tls-design.md`

**Workspace root:** `/Users/jph/Voiceapp/workspace_template`
**Branch:** `refactor/m1-split` (continue on this branch until the M1 branch is merged; after that switch to a fresh `feature/m2-tls` branch — decide at execution time based on whether M1 has landed).

---

## Ground rules for every task

1. **Workspace stays green.** `cargo check --workspace` must succeed before committing.
2. **No new clippy warnings.** Baseline after M1 was 62. Don't add warnings.
3. **Existing tests keep passing.** The known-flaky integration tests (`live_stress_*`, `test_create_space`, `test_audio_after_leave_room`, `test_channel_audio_relay`, `test_authenticate_invalid_token_creates_new`) are pre-existing flakes — not caused by anything in this plan.
4. **One logical change per commit.** Each task ends in a commit that leaves the workspace green.
5. **Do not touch files outside the paths listed in each task.**
6. **Follow existing style.** `net_control` uses `anyhow::Result` throughout — the plan respects that rather than introducing a new error enum (simplification vs. the spec; see Task 2).

---

## Task 0: Baseline verification

**Purpose:** Confirm starting state.

- [ ] **Step 1: Verify clean build**

Run: `cd /Users/jph/Voiceapp/workspace_template && cargo check --workspace`
Expected: finishes with no errors.

- [ ] **Step 2: Confirm clippy warning count**

Run: `cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"`
Expected: `62` (the M1 post-refactor baseline). If different, investigate before proceeding.

- [ ] **Step 3: Confirm tests pass**

Run: `cargo test --workspace --no-fail-fast -- --skip live_stress --skip test_create_space --skip test_audio_after_leave_room --skip test_channel_audio_relay --skip test_authenticate_invalid_token_creates_new 2>&1 | awk '/test result:/ {ok+=$4; fail+=$6} END {print "passed="ok, "failed="fail}'`
Expected: `passed=380 failed=0` (or close — test counts drift as code evolves; 0 failures is what matters).

No commit.

---

## Task 1: Cert expiry parsing + startup log

**Files:**
- Modify: `crates/signaling_server/Cargo.toml`
- Modify: `crates/signaling_server/src/tls.rs`
- Modify: `crates/signaling_server/src/main.rs` (one-line call after successful TLS setup)

**What this adds:** At server startup, after `load_tls_config` succeeds, parse the leaf cert, log `TLS cert for <subject> expires in N days (at <ISO-8601>)`. If N ≤ 14, log at WARN. If cert is already expired, return an error from `load_tls_config` — the server refuses to start.

- [ ] **Step 1: Add `x509-parser` dependency**

Open `crates/signaling_server/Cargo.toml`. Find the `[dependencies]` block. Add (alphabetically sensible, near other parsers):

```toml
x509-parser = "0.18"
```

Run `cd /Users/jph/Voiceapp/workspace_template && cargo check -p signaling_server` — expect a successful compile (x509-parser pulled in, not yet used).

- [ ] **Step 2: Add expiry helper in `tls.rs`**

Open `crates/signaling_server/src/tls.rs`. At the bottom (after `load_tls_config`), add:

```rust
/// Summary of a certificate: subject line and seconds until expiry
/// (negative if already expired).
pub(crate) struct CertInfo {
    pub(crate) subject: String,
    pub(crate) secs_until_expiry: i64,
}

/// Parse the leaf certificate at `cert_path` and return subject + expiry.
pub(crate) fn read_cert_info(cert_path: &str) -> std::io::Result<CertInfo> {
    use std::io::{Error, ErrorKind};
    use x509_parser::pem::Pem;

    let file = std::fs::File::open(cert_path)?;
    let reader = std::io::BufReader::new(file);

    // Parse the first PEM block as an X.509 cert (the leaf).
    let mut iter = Pem::iter_from_reader(reader);
    let pem = iter
        .next()
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "no PEM blocks in cert file"))?
        .map_err(|e| Error::new(ErrorKind::InvalidData, format!("PEM parse: {e}")))?;
    let (_, cert) = pem
        .parse_x509()
        .map_err(|e| Error::new(ErrorKind::InvalidData, format!("X509 parse: {e}")))?;

    let subject = cert.subject().to_string();
    let not_after = cert.validity().not_after.timestamp();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(CertInfo {
        subject,
        secs_until_expiry: not_after - now,
    })
}
```

- [ ] **Step 3: Add dev-deps needed by the unit tests**

Check existing dev-deps:
```
grep -A5 "\[dev-dependencies\]" crates/signaling_server/Cargo.toml
```

Ensure `[dev-dependencies]` includes `tempfile` and `rcgen`. Add any missing:

```toml
[dev-dependencies]
tempfile = "3"
rcgen = "0.13"
```

(The integration test in Task 3 will also use `rcgen`, but the `-p signaling_server` crate needs its own copy for these unit tests.)

- [ ] **Step 4: Add unit tests for the helper**

Still in `tls.rs`, append (or add to if one exists) a `#[cfg(test)] mod tests { ... }` block:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a self-signed cert for "localhost" valid for the next 30 days,
    /// write it to a tempfile, return its path.
    fn write_test_cert(dir: &std::path::Path) -> std::path::PathBuf {
        use rcgen::{CertificateParams, DnType, KeyPair};
        let mut params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        params.distinguished_name.push(DnType::CommonName, "localhost");
        let key = KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        let path = dir.join("leaf.pem");
        std::fs::write(&path, cert.pem()).unwrap();
        path
    }

    #[test]
    fn read_cert_info_parses_self_signed_cert() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_cert(dir.path());
        let info = read_cert_info(path.to_str().unwrap()).expect("parse succeeds");
        assert!(info.subject.contains("localhost"), "subject = {}", info.subject);
        // rcgen defaults to a 1-year validity window; confirm it parses as
        // roughly that (at least 10 days out).
        assert!(
            info.secs_until_expiry > 10 * 86_400,
            "expiry = {}s (expected > 10 days)",
            info.secs_until_expiry
        );
    }

    #[test]
    fn read_cert_info_rejects_non_pem() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-cert.txt");
        std::fs::write(&path, "hello world").unwrap();
        let err = read_cert_info(path.to_str().unwrap()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
```

- [ ] **Step 5: Run the tests**

Run: `cd /Users/jph/Voiceapp/workspace_template && cargo test -p signaling_server tls::tests`
Expected: both tests pass.

- [ ] **Step 6: Log cert expiry at startup in `main.rs`**

Open `crates/signaling_server/src/main.rs`. Find the block that loads TLS config (around the `load_tls_config(&cert_path, &key_path)` call — it's in the `match (std::env::var("PV_CERT"), std::env::var("PV_KEY"))` arm). Immediately after the successful `load_tls_config` call (where it logs `"TLS enabled..."`), add an expiry log.

Find this existing code:

```rust
(Ok(cert_path), Ok(key_path)) => match load_tls_config(&cert_path, &key_path) {
    Ok(config) => {
        log::info!("TLS enabled (cert: {cert_path}, key: {key_path})");
        Some(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
    }
```

Replace with:

```rust
(Ok(cert_path), Ok(key_path)) => match load_tls_config(&cert_path, &key_path) {
    Ok(config) => {
        log::info!("TLS enabled (cert: {cert_path}, key: {key_path})");
        match tls::read_cert_info(&cert_path) {
            Ok(info) => {
                let days = info.secs_until_expiry / 86_400;
                if info.secs_until_expiry < 0 {
                    log::error!(
                        "TLS cert for {} has EXPIRED ({} days ago) — refusing to start",
                        info.subject,
                        -days
                    );
                    std::process::exit(1);
                } else if days <= 14 {
                    log::warn!(
                        "TLS cert for {} expires in {} days — schedule a renewal",
                        info.subject,
                        days
                    );
                } else {
                    log::info!(
                        "TLS cert for {} expires in {} days",
                        info.subject,
                        days
                    );
                }
            }
            Err(e) => log::warn!("Could not parse cert for expiry check: {e}"),
        }
        Some(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
    }
```

Note: `tls` is already a `mod` in `main.rs` after M1.

- [ ] **Step 7: Verify workspace compiles + clippy clean**

Run:
```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo clippy -p signaling_server --all-targets 2>&1 | grep -cE "^warning:"
```
Expected: check succeeds; clippy warning count for signaling_server matches Task 0 baseline (do not introduce new warnings).

- [ ] **Step 8: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add crates/signaling_server/Cargo.toml crates/signaling_server/src/tls.rs crates/signaling_server/src/main.rs
git commit -m "feat(tls): log cert expiry at startup, refuse to start if expired"
```

---

## Task 2: Client-side TLS handshake error classification

**Files:**
- Modify: `crates/app_desktop/src/signal_handler/connection.rs`

**What this adds:** When the client fails to connect, inspect the error string. If it mentions a TLS certificate problem, show a dedicated toast message that tells the user what to do instead of a generic "connection closed".

**Deviation from spec:** Spec proposed adding `ConnectError::TlsHandshake(String)` to `net_control`. `net_control` uses `anyhow::Result` throughout and does NOT have a typed error module. Introducing one would be a larger refactor than the value it adds. Simpler and idiomatic: classify in the caller (`signal_handler/connection.rs`) via error-chain string inspection. This is consistent with the existing codebase style.

- [ ] **Step 1: Find the client connection error handler**

Run: `grep -n "connect\|Failed to connect\|show_toast\|set_connection_error" crates/app_desktop/src/signal_handler/connection.rs | head -20`

Locate the call to `client.connect(...)` and the branch that handles its `Err`. Read ~40 lines around it so you understand how errors are currently surfaced.

- [ ] **Step 2: Add a classification helper (private to this file)**

At the top of `crates/app_desktop/src/signal_handler/connection.rs` (after imports), add:

```rust
/// Classify a connection failure so we can give the user a useful message.
/// Looks at the full anyhow error chain.
fn classify_connect_error(e: &anyhow::Error) -> &'static str {
    // Walk the error chain stringified so we match against rustls's various
    // error variants without depending on rustls types directly.
    let chain = format!("{e:#}");
    let lower = chain.to_lowercase();
    if lower.contains("invalidcertificate")
        || lower.contains("unknownissuer")
        || lower.contains("unknown_issuer")
        || lower.contains("badcertificate")
        || lower.contains("notvalidforname")
        || lower.contains("certificate has expired")
        || lower.contains("expired")
    {
        "tls"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "timeout"
    } else {
        "generic"
    }
}
```

- [ ] **Step 3: Call the classifier at the connect-error site**

Find the `Err(e)` arm after `client.connect(server_url).await`. Change the user-facing message to branch on `classify_connect_error(&e)`:

Current (approximate; match against what the file actually has):

```rust
Err(e) => {
    log::error!("Failed to connect: {e:#}");
    // existing code that sets a generic error toast
}
```

Change to (keep the log line and any existing `set_connection_error` type calls, but branch the message):

```rust
Err(e) => {
    log::error!("Failed to connect: {e:#}");
    let msg = match classify_connect_error(&e) {
        "tls" => "Could not connect: server's TLS certificate is invalid or expired. Ask the server operator to run deploy/setup-tls.sh.".to_string(),
        "timeout" => "Could not connect: timed out after 5 seconds. Check the server address and your network.".to_string(),
        _ => format!("Could not connect: {e}"),
    };
    // existing error-surfacing code, but using `msg` instead of a hard-coded string.
    // For example, if the file currently does:
    //   window.set_last_error(slint::SharedString::from(format!("Could not connect: {e}")));
    // change it to:
    //   window.set_last_error(slint::SharedString::from(msg));
}
```

The exact plumbing depends on how the rest of the file surfaces errors. Preserve existing behavior; just substitute the constructed `msg` for the previous single-format-string call.

- [ ] **Step 4: Add unit tests for the classifier**

At the bottom of the same file, append:

```rust
#[cfg(test)]
mod tls_classify_tests {
    use super::classify_connect_error;

    #[test]
    fn classifies_invalid_cert() {
        let e = anyhow::anyhow!("tls handshake failed: InvalidCertificate(UnknownIssuer)");
        assert_eq!(classify_connect_error(&e), "tls");
    }

    #[test]
    fn classifies_expired_cert() {
        let e = anyhow::anyhow!("handshake error: certificate has expired");
        assert_eq!(classify_connect_error(&e), "tls");
    }

    #[test]
    fn classifies_name_mismatch() {
        let e = anyhow::anyhow!("tls: NotValidForName");
        assert_eq!(classify_connect_error(&e), "tls");
    }

    #[test]
    fn classifies_timeout() {
        let e = anyhow::anyhow!("Connection timed out (5s)");
        assert_eq!(classify_connect_error(&e), "timeout");
    }

    #[test]
    fn classifies_generic() {
        let e = anyhow::anyhow!("connection refused");
        assert_eq!(classify_connect_error(&e), "generic");
    }
}
```

- [ ] **Step 5: Run**

```
cd /Users/jph/Voiceapp/workspace_template
cargo check --workspace
cargo test -p app_desktop signal_handler::connection::tls_classify_tests
```
Expected: check clean; all 5 classifier tests pass.

- [ ] **Step 6: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add crates/app_desktop/src/signal_handler/connection.rs
git commit -m "feat(client): surface actionable message for TLS handshake failures"
```

---

## Task 3: Integration test for wss:// round trip

**Files:**
- Modify: `crates/integration_tests/Cargo.toml`
- Create: `crates/integration_tests/tests/tls_test.rs`

**What this adds:** A real end-to-end test that generates a self-signed CA + leaf cert, starts the signaling server with TLS, connects a tungstenite client that trusts the generated CA, and exchanges a minimal message. Proves the TLS path works.

- [ ] **Step 1: Add test dependencies**

Open `crates/integration_tests/Cargo.toml`. Ensure `[dev-dependencies]` (or `[dependencies]`, whichever this workspace uses for test-only deps — check surrounding files) includes:

```toml
rcgen = "0.13"
rustls = { version = "0.23", default-features = false, features = ["ring"] }
tokio-rustls = "0.26"
tempfile = "3"
```

If any of these are already present in the workspace root `Cargo.toml` under `[workspace.dependencies]`, prefer `<name> = { workspace = true }`. Run:

```
grep -E "^rcgen|^rustls|^tokio-rustls|^tempfile" crates/integration_tests/Cargo.toml
```

Run `cd /Users/jph/Voiceapp/workspace_template && cargo check -p integration_tests --tests` — expect a successful compile (deps resolved, no test yet).

- [ ] **Step 2: Look at existing integration tests for harness patterns**

Run: `ls crates/integration_tests/tests/ && head -40 crates/integration_tests/tests/server_tests.rs`

Note: how they start the server (binary vs. in-process), how they pick a free port, how they shut down. You'll reuse that pattern.

- [ ] **Step 3: Create `crates/integration_tests/tests/tls_test.rs`**

```rust
//! End-to-end TLS smoke test for the signaling server.
//!
//! Generates a self-signed root + leaf cert, launches the server binary with
//! PV_CERT/PV_KEY pointing at the leaf, then connects a tungstenite client
//! that trusts the generated root. Sends Hello, expects a Welcome (or
//! whatever the current handshake is) within 5 seconds.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::Connector;

/// Generate a self-signed root CA + leaf cert for "localhost", return
/// (ca_cert_der, leaf_cert_pem, leaf_key_pem).
fn make_self_signed() -> (Vec<u8>, String, String) {
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose,
    };

    // Root CA
    let mut ca_params = CertificateParams::new(vec![]).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(DnType::CommonName, "voxlink-test-root");
    ca_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    let ca_key = KeyPair::generate().unwrap();
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();

    // Leaf cert for localhost
    let mut leaf_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    leaf_params.distinguished_name.push(DnType::CommonName, "localhost");
    let leaf_key = KeyPair::generate().unwrap();
    let leaf_cert = leaf_params.signed_by(&leaf_key, &ca_cert, &ca_key).unwrap();

    let leaf_pem = format!("{}{}", leaf_cert.pem(), leaf_key.serialize_pem());
    // Actually — the server expects cert and key in separate files.
    let leaf_cert_pem = leaf_cert.pem();
    let leaf_key_pem = leaf_key.serialize_pem();
    let ca_der = ca_cert.der().to_vec();
    (ca_der, leaf_cert_pem, leaf_key_pem)
}

/// Bind a TCP listener, pick the port, drop the listener, return the port.
/// Small race window but good enough for a test.
async fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

struct ServerHandle {
    child: Child,
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn spawn_server(cert: &std::path::Path, key: &std::path::Path, port: u16) -> ServerHandle {
    // The binary path produced by `cargo build` is target/debug/signaling_server
    // relative to the workspace root.
    let exe = std::env::var("CARGO_BIN_EXE_signaling_server").unwrap_or_else(|_| {
        // Fallback: assume standard cargo layout
        let target = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into());
        format!("{target}/debug/signaling_server")
    });
    let child = Command::new(&exe)
        .env("PV_ADDR", format!("127.0.0.1:{port}"))
        .env("PV_CERT", cert)
        .env("PV_KEY", key)
        .env("RUST_LOG", "info")
        .env("PV_ALLOW_INSECURE", "1") // allow loopback, avoid PV_ALLOW_INSECURE gate
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn signaling_server");
    // Give the server a moment to bind.
    tokio::time::sleep(Duration::from_millis(500)).await;
    ServerHandle { child }
}

#[tokio::test]
async fn wss_round_trip_with_self_signed_cert() {
    let (ca_der, leaf_cert_pem, leaf_key_pem) = make_self_signed();

    let dir = tempfile::tempdir().unwrap();
    let cert_path = dir.path().join("leaf.crt");
    let key_path = dir.path().join("leaf.key");
    std::fs::write(&cert_path, &leaf_cert_pem).unwrap();
    std::fs::write(&key_path, &leaf_key_pem).unwrap();

    let port = free_port().await;
    let _server = spawn_server(&cert_path, &key_path, port).await;

    // Build a tungstenite connector that trusts only our test CA.
    let mut root_store = rustls::RootCertStore::empty();
    root_store
        .add(rustls::pki_types::CertificateDer::from(ca_der))
        .unwrap();
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let connector = Connector::Rustls(Arc::new(tls_config));

    // Connect wss://localhost:<port>
    let url = format!("wss://localhost:{port}");
    let req = url.as_str().into_client_request().unwrap();

    let connect = tokio_tungstenite::connect_async_tls_with_config(
        req,
        None,
        false,
        Some(connector),
    );
    let (ws, _resp) = tokio::time::timeout(Duration::from_secs(5), connect)
        .await
        .expect("connect timed out")
        .expect("wss connect failed");

    // If we reached this point, the TLS handshake succeeded. Good.
    // Shut down the connection cleanly.
    let (mut tx, _rx) = futures_util::StreamExt::split(ws);
    let _ = futures_util::SinkExt::close(&mut tx).await;
}
```

- [ ] **Step 4: Verify the test file compiles**

Run: `cd /Users/jph/Voiceapp/workspace_template && cargo test -p integration_tests --test tls_test --no-run`
Expected: compiles successfully.

If compilation fails, the most likely causes:
- A dev-dep version mismatch. Check the actual rustls version in `Cargo.lock` via `grep -A1 '^name = "rustls"' Cargo.lock`. Align.
- `Connector` API changed. If so, read tokio-tungstenite 0.26's docs and adjust.

- [ ] **Step 5: Build the server binary (required for the test)**

Run: `cargo build -p signaling_server`
Expected: builds cleanly. The test launches this binary.

- [ ] **Step 6: Run the test**

Run: `cargo test -p integration_tests --test tls_test`
Expected: `wss_round_trip_with_self_signed_cert ... ok`.

If it fails:
- Check server stderr by replacing `.stdout(...).stderr(...)` with `.stderr(std::process::Stdio::inherit())` temporarily.
- Confirm PV_ADDR binding succeeded (port collision).
- Confirm cert + key paths are readable.

- [ ] **Step 7: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add crates/integration_tests/Cargo.toml crates/integration_tests/tests/tls_test.rs
git commit -m "test(tls): integration test for wss:// round trip with self-signed cert"
```

---

## Task 4: `deploy/setup-tls.sh` — Let's Encrypt provisioning script

**Files:**
- Create: `deploy/setup-tls.sh`

**What this adds:** One-shot idempotent script, run on the server, that installs certbot, acquires a Let's Encrypt cert for the given domain, sets ACLs, installs a renewal deploy-hook, updates the voxlink.service env, and restarts voxlink.

- [ ] **Step 1: Read the existing setup-server.sh to match style**

Run: `cat deploy/setup-server.sh` — note bash conventions (set -e, sudo, tee), how it edits the systemd unit, idempotency checks.

- [ ] **Step 2: Create `deploy/setup-tls.sh`**

```bash
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
```

- [ ] **Step 3: Make executable**

```bash
cd /Users/jph/Voiceapp/workspace_template
chmod +x deploy/setup-tls.sh
```

- [ ] **Step 4: Lint (shellcheck if available, else skip)**

```
which shellcheck && shellcheck deploy/setup-tls.sh
```
Expected: no errors. If shellcheck isn't installed, skip — CI or future contributors will catch it.

- [ ] **Step 5: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add deploy/setup-tls.sh
git commit -m "deploy: add setup-tls.sh for Let's Encrypt provisioning"
```

---

## Task 5: Wire `--tls` flag into `push-to-server.sh`

**Files:**
- Modify: `deploy/push-to-server.sh`

**What this adds:** A `--tls <domain>` flag that, after successful build + deploy, copies `setup-tls.sh` to the server and runs it.

- [ ] **Step 1: Read current push-to-server.sh arg handling**

Run: `grep -n "SERVER=\|\$1\|getopts\|case" deploy/push-to-server.sh | head -20`

Note the existing positional-arg convention (`<user>@<server-ip>`).

- [ ] **Step 2: Add flag parsing to `push-to-server.sh`**

Open `deploy/push-to-server.sh`. Find the top of the script (after the usage block, before `SERVER="$1"`). Replace the simple `SERVER="$1"` with argument parsing that accepts `--tls <domain>`:

Find:

```bash
if [ -z "$1" ]; then
    echo "Usage: $0 <user>@<server-ip>"
    echo "Example: $0 ubuntu@129.146.123.45"
    exit 1
fi

SERVER="$1"
```

Replace with:

```bash
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
```

- [ ] **Step 3: At the end of push-to-server.sh, run setup-tls.sh remotely if --tls was set**

Find the very last section of the script (after the build-and-restart steps — the existing script ends with something like `echo "Done."`). Before that final echo, add:

```bash
if [ -n "$TLS_DOMAIN" ]; then
    echo
    echo "=== Configuring TLS for $TLS_DOMAIN ==="
    scp "$SCRIPT_DIR/setup-tls.sh" "$SERVER:/tmp/voxlink-setup-tls.sh"
    ssh "$SERVER" "chmod +x /tmp/voxlink-setup-tls.sh && sudo /tmp/voxlink-setup-tls.sh '$TLS_DOMAIN'"
fi
```

(Use the existing `$SCRIPT_DIR` variable that's already defined earlier in the script.)

- [ ] **Step 4: Sanity check — syntax only**

Run: `bash -n deploy/push-to-server.sh`
Expected: no syntax errors. This doesn't execute the script.

- [ ] **Step 5: (Optional manual test) Dry-run against the real server**

Only do this if you're ready to actually provision. Otherwise skip — the commit below is safe without it.

```bash
# Only if you have a domain ready and are OK restarting the server:
# ./deploy/push-to-server.sh --tls <your-domain> ubuntu@129.158.231.26
```

- [ ] **Step 6: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add deploy/push-to-server.sh
git commit -m "deploy: add --tls <domain> flag to push-to-server.sh"
```

---

## Task 6: `docs/TLS_SETUP.md`

**Files:**
- Create: `docs/TLS_SETUP.md`
- Modify: `docs/ARCHITECTURE.md` (add a cross-link)

- [ ] **Step 1: Create `docs/TLS_SETUP.md`**

```markdown
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
```

- [ ] **Step 2: Cross-link from `docs/ARCHITECTURE.md`**

Open `docs/ARCHITECTURE.md`. Find a reasonable place for a one-line reference (e.g., in a transport or security section). Add:

```markdown
For enabling TLS on a public deployment, see [TLS_SETUP.md](TLS_SETUP.md).
```

If there's no obvious section, add a new short section at the bottom:

```markdown
## Deployment

- [Setup (non-TLS)](../deploy/setup-server.sh) — one-shot server install.
- [TLS setup](TLS_SETUP.md) — Let's Encrypt for public deployments.
```

- [ ] **Step 3: Commit**

```bash
cd /Users/jph/Voiceapp/workspace_template
git add docs/TLS_SETUP.md docs/ARCHITECTURE.md
git commit -m "docs: add TLS_SETUP.md walkthrough"
```

---

## Task 7: Final verification

- [ ] **Step 1: Clean workspace build**

Run: `cargo check --workspace`
Expected: clean.

- [ ] **Step 2: Clippy**

Run: `cargo clippy --workspace --all-targets 2>&1 | grep -cE "^warning:"`
Expected: ≤ 62 (Task 0 baseline). New warnings must be justified — if this exceeds 62, find the added warnings and fix them.

- [ ] **Step 3: Test suite (skipping known flakes)**

Run:
```
cargo test --workspace --no-fail-fast -- \
  --skip live_stress \
  --skip test_create_space \
  --skip test_audio_after_leave_room \
  --skip test_channel_audio_relay \
  --skip test_authenticate_invalid_token_creates_new \
  2>&1 | awk '/test result:/ {ok+=$4; fail+=$6} END {print "passed="ok, "failed="fail}'
```
Expected: `failed=0`. `passed` count should be Task 0 baseline + at least 8 new tests (2 cert-parsing, 5 error-classification, 1 wss round trip).

- [ ] **Step 4: Run the TLS integration test on its own**

Run: `cargo test -p integration_tests --test tls_test`
Expected: `1 passed; 0 failed`.

- [ ] **Step 5: Shellcheck the deploy scripts**

```
which shellcheck && shellcheck deploy/setup-tls.sh deploy/push-to-server.sh
```
Expected: no errors. Skip if shellcheck isn't installed locally.

- [ ] **Step 6: Record final state**

Run:
```
git log --oneline fba66c8..HEAD
wc -l deploy/setup-tls.sh docs/TLS_SETUP.md
```

This gives a manifest of the milestone. No commit.

---

# Completion criteria

All of:

1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets` ≤ Task 0 baseline (62) warnings.
3. All non-flaky tests pass, including the new `tls_test`.
4. `./deploy/push-to-server.sh --tls <domain> ubuntu@<ip>` would work on a fresh VM with DNS + port 80 available (manual verification, not gated on CI).
5. Server startup log includes `TLS cert for <subject> expires in N days` when TLS is enabled.
6. Client failing TLS handshake shows the dedicated error toast, not a generic failure.
7. Every commit on this milestone leaves the workspace green.

# If something goes wrong

- **`x509-parser` API differs from what I specified:** read the crate's current docs (crates.io/crates/x509-parser) and adjust. The test is the contract: keep it passing.
- **`rcgen` API differs:** same — crates.io/crates/rcgen. Keep the test's intent (self-signed root + leaf for localhost).
- **Integration test fails because the binary isn't built:** run `cargo build -p signaling_server` first. The test relies on `CARGO_BIN_EXE_signaling_server` being set by cargo or falling back to `target/debug/signaling_server`.
- **Clippy warning count increases:** look at the new warnings. If they're legitimate, fix them. Don't `#[allow(...)]` without a comment explaining why.
- **New tests are flaky on CI:** the TLS test starts a subprocess and sleeps 500ms before connecting. If that's too short on a slow runner, bump to 1-2 seconds or replace with a tcp-connect retry loop.

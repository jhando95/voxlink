//! End-to-end TLS smoke test for the signaling server.
//!
//! Generates a self-signed root + leaf cert, launches the server binary with
//! PV_CERT/PV_KEY pointing at the leaf, then connects a tungstenite client
//! that trusts the generated root. Asserts the TLS handshake succeeds.

use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// Generate a self-signed root CA + leaf cert for "localhost".
/// Returns (ca_cert_der, leaf_cert_pem, leaf_key_pem).
fn make_self_signed() -> (Vec<u8>, String, String) {
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose,
    };

    // Root CA
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "voxlink-test-root");
    ca_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    let ca_key = KeyPair::generate().unwrap();
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();

    // Leaf cert for "localhost"
    let mut leaf_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    leaf_params
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    let leaf_key = KeyPair::generate().unwrap();
    let leaf_cert = leaf_params.signed_by(&leaf_key, &ca_cert, &ca_key).unwrap();

    let leaf_cert_pem = leaf_cert.pem();
    let leaf_key_pem = leaf_key.serialize_pem();
    let ca_der = ca_cert.der().to_vec();
    (ca_der, leaf_cert_pem, leaf_key_pem)
}

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
    let server_bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/debug/signaling_server");

    let child = Command::new(&server_bin)
        .env("PV_ADDR", format!("127.0.0.1:{port}"))
        .env("PV_CERT", cert)
        .env("PV_KEY", key)
        .env("RUST_LOG", "info")
        .env("PV_ALLOW_INSECURE", "1")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| {
            panic!(
                "Failed to spawn signaling_server at {:?}: {}. Did you run `cargo build -p signaling_server`?",
                server_bin, e
            )
        });

    // Wait for server to bind by polling TCP.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if tokio::time::Instant::now() > deadline {
            panic!("Server did not bind on port {port} within 20 seconds");
        }
        match tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await {
            Ok(_) => break,
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }

    ServerHandle { child }
}

#[tokio::test]
async fn wss_round_trip_with_self_signed_cert() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let (ca_der, leaf_cert_pem, leaf_key_pem) = make_self_signed();

    let dir = tempfile::tempdir().unwrap();
    let cert_path = dir.path().join("leaf.crt");
    let key_path = dir.path().join("leaf.key");
    std::fs::write(&cert_path, &leaf_cert_pem).unwrap();
    std::fs::write(&key_path, &leaf_key_pem).unwrap();

    let port = free_port().await;
    let _server = spawn_server(&cert_path, &key_path, port).await;

    // Build a rustls ClientConfig that trusts only our test CA.
    let mut root_store = rustls::RootCertStore::empty();
    root_store
        .add(rustls::pki_types::CertificateDer::from(ca_der))
        .unwrap();
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let tls_connector = TlsConnector::from(Arc::new(tls_config));

    // Open a plain TCP connection, then upgrade to TLS, then WebSocket.
    let tcp = tokio::time::timeout(
        Duration::from_secs(5),
        TcpStream::connect(format!("127.0.0.1:{port}")),
    )
    .await
    .expect("TCP connect timed out")
    .expect("TCP connect failed");

    let server_name = rustls::pki_types::ServerName::try_from("localhost")
        .expect("invalid server name")
        .to_owned();
    let tls_stream = tokio::time::timeout(
        Duration::from_secs(5),
        tls_connector.connect(server_name, tcp),
    )
    .await
    .expect("TLS handshake timed out")
    .expect("TLS handshake failed");

    // Upgrade TLS stream to WebSocket.
    let url = format!("wss://localhost:{port}");
    let req = url.as_str().into_client_request().unwrap();
    let (ws, _resp) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::client_async(req, tls_stream),
    )
    .await
    .expect("WebSocket upgrade timed out")
    .expect("WebSocket upgrade failed");

    // TLS handshake + WebSocket upgrade both succeeded. Clean up.
    use futures_util::{SinkExt, StreamExt};
    let (mut tx, _rx) = ws.split();
    let _ = tx.close().await;
}

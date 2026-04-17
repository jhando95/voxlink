use std::net::ToSocketAddrs;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

// ─── Server stream: either plain TCP or TLS ───

pub(crate) enum ServerStream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl AsyncRead for ServerStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            ServerStream::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            ServerStream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for ServerStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            ServerStream::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            ServerStream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            ServerStream::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            ServerStream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            ServerStream::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            ServerStream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

impl Unpin for ServerStream {}

pub(crate) fn bind_requires_tls(addr: &str) -> bool {
    match addr.to_socket_addrs() {
        Ok(addrs) => addrs
            .map(|socket_addr| socket_addr.ip())
            .any(|ip| !ip.is_loopback()),
        Err(_) => {
            !addr.starts_with("127.0.0.1:")
                && !addr.starts_with("[::1]:")
                && !addr.starts_with("localhost:")
        }
    }
}

pub(crate) fn allow_insecure_public_bind() -> bool {
    matches!(
        std::env::var("PV_ALLOW_INSECURE").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
    )
}

pub(crate) fn load_tls_config(
    cert_path: &str,
    key_path: &str,
) -> Result<tokio_rustls::rustls::ServerConfig, Box<dyn std::error::Error>> {
    let cert_file = std::fs::File::open(cert_path)?;
    let key_file = std::fs::File::open(key_path)?;

    let certs: Vec<_> = rustls_pemfile::certs(&mut std::io::BufReader::new(cert_file))
        .filter_map(|r| r.ok())
        .collect();

    let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(key_file))?
        .ok_or("No private key found in key file")?;

    let config = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    Ok(config)
}

/// Summary of a certificate: subject line and seconds until expiry
/// (negative if already expired).
#[derive(Debug)]
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
    let cert = pem
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a self-signed cert for "localhost" valid for the next ~1 year,
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
        // rcgen defaults to a validity window of about 1 year; confirm > 10 days.
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

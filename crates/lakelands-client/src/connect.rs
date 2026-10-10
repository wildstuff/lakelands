//! The connection under the client: plain TCP for `http`, rustls over TCP
//! for `https`, as a hyper-util connector so the pooled client reuses it.
//!
//! No proxy is used and nothing is read from the environment: `https`
//! trusts the certificate authorities of the configured bundle, or of the
//! first system bundle found at a well-known path, and a server name TLS
//! cannot verify is refused before anything is sent.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use hyper::Uri;
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::TokioIo;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::net::TcpStream;

/// Where systems keep their bundle of trusted certificate authorities,
/// tried in this order when the configuration names none.
const SYSTEM_BUNDLES: &[&str] = &[
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/ca-bundle.pem",
    "/etc/ssl/cert.pem",
    "/usr/local/etc/openssl/cert.pem",
];

/// A TLS configuration trusting the authorities of `ca_file`, or of the
/// first system bundle.
pub(crate) fn tls(ca_file: Option<&Path>) -> Result<Arc<rustls::ClientConfig>, String> {
    let bundle: PathBuf = match ca_file {
        Some(f) => f.to_owned(),
        None => SYSTEM_BUNDLES
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
            .ok_or("no certificate bundle was found at a well-known path; name one as `ca_file`")?,
    };
    let mut roots = rustls::RootCertStore::empty();
    let certs = CertificateDer::pem_file_iter(&bundle).map_err(|e| {
        format!(
            "the certificate bundle {} cannot be read: {e}",
            bundle.display()
        )
    })?;
    let (added, _) = roots.add_parsable_certificates(certs.filter_map(|c| c.ok()));
    if added == 0 {
        return Err(format!(
            "the certificate bundle {} holds no certificate",
            bundle.display()
        ));
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS cannot be set up: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Connects to the server the URL names.
#[derive(Clone)]
pub(crate) struct Connector {
    /// The TLS configuration, for an `https` server.
    tls: Option<Arc<rustls::ClientConfig>>,
    /// A connection a test holds the other end of, taken in place of the
    /// first one dialed.
    #[cfg(test)]
    over: Arc<std::sync::Mutex<Option<tokio::io::DuplexStream>>>,
}

impl Connector {
    pub(crate) fn new(tls: Option<Arc<rustls::ClientConfig>>) -> Connector {
        Connector {
            tls,
            #[cfg(test)]
            over: Arc::default(),
        }
    }

    /// A connector whose first connection is `conn`: no socket, so
    /// nothing in the exchange waits on the wall clock.
    #[cfg(test)]
    pub(crate) fn over(conn: tokio::io::DuplexStream) -> Connector {
        Connector {
            tls: None,
            over: Arc::new(std::sync::Mutex::new(Some(conn))),
        }
    }
}

/// One connection: plain, or TLS.
pub(crate) enum Conn {
    Plain(TokioIo<TcpStream>),
    Tls(Box<TokioIo<tokio_rustls::client::TlsStream<TcpStream>>>),
    /// A test's in-memory connection.
    #[cfg(test)]
    Duplex(TokioIo<tokio::io::DuplexStream>),
}

impl tower_service::Service<Uri> for Connector {
    type Response = Conn;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<Conn>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let tls = self.tls.clone();
        #[cfg(test)]
        let over = self.over.lock().unwrap_or_else(|e| e.into_inner()).take();
        Box::pin(async move {
            #[cfg(test)]
            if let Some(conn) = over {
                return Ok(Conn::Duplex(TokioIo::new(conn)));
            }
            let secure = uri.scheme_str() == Some("https");
            let host = uri
                .host()
                .ok_or_else(|| io::Error::other("the URL names no host"))?
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_owned();
            let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
            let tcp = TcpStream::connect((host.as_str(), port)).await?;
            let _ = tcp.set_nodelay(true);
            if !secure {
                return Ok(Conn::Plain(TokioIo::new(tcp)));
            }
            let config = tls.ok_or_else(|| io::Error::other("TLS is not set up"))?;
            let name = ServerName::try_from(host)
                .map_err(|e| io::Error::other(format!("not a name TLS can verify: {e}")))?;
            let stream = tokio_rustls::TlsConnector::from(config)
                .connect(name, tcp)
                .await?;
            Ok(Conn::Tls(Box::new(TokioIo::new(stream))))
        })
    }
}

impl Connection for Conn {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

impl Read for Conn {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Conn::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
            #[cfg(test)]
            Conn::Duplex(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl Write for Conn {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Conn::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Conn::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
            #[cfg(test)]
            Conn::Duplex(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(s) => Pin::new(s).poll_flush(cx),
            Conn::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
            #[cfg(test)]
            Conn::Duplex(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Conn::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Conn::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
            #[cfg(test)]
            Conn::Duplex(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

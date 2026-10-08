//! The one HTTP client the crate's outbound calls share: `POST` a JSON
//! body to one target and read a `2xx` reply's body.
//!
//! Plain `http` or `https` over rustls, HTTP/1.1, one connection per
//! request. No proxy is used and nothing is read from the environment:
//! `https` trusts the certificate authorities of the configured bundle,
//! or of the first system bundle found at a well-known path, and TLS is
//! set up before anything is sent to the host. A redirect is never
//! followed; any status but `2xx` fails. The timeout covers the whole
//! exchange, connecting included, and a reply larger than the limit
//! fails. A failure names the host, the port and the path it was sent
//! to, never the URL as configured, which may carry user information.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::header::{ACCEPT, CONTENT_TYPE, HOST, HeaderName, HeaderValue};
use hyper::{Request, Uri};
use hyper_util::rt::TokioIo;
use lakelands_api::error::{Detail, Exchange};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncRead, AsyncWrite};
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

/// Where one request goes: a URL taken apart, its user information
/// dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    secure: bool,
    /// The host, as the URL names it (an IPv6 address in brackets).
    pub(crate) host: String,
    /// The port, the scheme's own when the URL names none.
    pub(crate) port: u16,
    /// What the `Host` header says: the host, and the port if the URL
    /// names one.
    authority: String,
    /// The path and query sent.
    path: String,
    /// What a failure names: host, port and path, without the query.
    shown: String,
    /// Whether the URL carries user information, which is never sent.
    pub(crate) user_info: bool,
}

impl Target {
    /// The target of an `http` or `https` URL. A failure is the end of a
    /// sentence whose subject is the URL.
    pub(crate) fn parse(url: &str) -> Result<Target, String> {
        let uri: Uri = url.parse().map_err(|e| format!("is not a URL: {e}"))?;
        let secure = match uri.scheme_str() {
            Some("https") => true,
            Some("http") => false,
            _ => return Err("is neither an `http` nor an `https` URL".into()),
        };
        let host = uri.host().ok_or("names no host")?.to_owned();
        let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
        // Built from the host and an explicit port only: the URL's
        // authority may carry user information, which is never sent.
        let authority = match uri.port_u16() {
            Some(p) => format!("{host}:{p}"),
            None => host.clone(),
        };
        let path = uri.path_and_query().map_or("/", |p| p.as_str()).to_owned();
        let shown = format!("{host}:{port}{}", uri.path());
        let user_info = uri.authority().is_some_and(|a| a.as_str().contains('@'));
        Ok(Target {
            secure,
            host,
            port,
            authority,
            path,
            shown,
            user_info,
        })
    }

    /// Host, port and path, for a failure to name.
    pub(crate) fn shown(&self) -> &str {
        &self.shown
    }
}

/// Why a request was not answered.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Failure {
    /// Nothing was sent: the request cannot be made as configured.
    Refused(Exchange),
    /// Nothing was sent: TLS cannot be set up. The reason may name a
    /// certificate bundle's path on the server.
    Tls(String),
    /// The exchange took longer than the timeout.
    TimedOut(Duration),
    /// The host answered with a status other than `2xx`, a redirect
    /// included (never followed). Its body is not read.
    Status(hyper::StatusCode),
    /// The host answered `2xx`, and its body could not be read within the
    /// limit. The status has been given; what the body says is lost.
    Body(Exchange),
    /// The exchange failed, or its answer is not a `2xx` reply within the
    /// limit.
    Failed(Exchange),
}

/// The client: its trust, its timeout and its reply limit.
pub(crate) struct Transport {
    /// Who calls, as its failures name it: "the http backing".
    who: &'static str,
    ca_file: Option<PathBuf>,
    timeout: Duration,
    max_reply_bytes: u64,
    tls: Mutex<Option<Arc<rustls::ClientConfig>>>,
}

impl Failure {
    /// A status as the exchange a caller's failure carries.
    pub(crate) fn status(status: hyper::StatusCode) -> Exchange {
        let status_code = status.as_u16();
        if status.is_redirection() {
            Exchange::Redirect {
                status: status_code,
            }
        } else {
            Exchange::Status {
                status: status_code,
            }
        }
    }

    /// The failure as the exchange a caller's failure carries: TLS that
    /// cannot be set up as a bundle the operator names, since the reason
    /// may name a bundle's path on the server.
    pub(crate) fn exchange(self) -> Exchange {
        match self {
            Failure::Refused(x) | Failure::Body(x) | Failure::Failed(x) => x,
            Failure::Tls(_) => Exchange::NoBundle,
            Failure::TimedOut(t) => Exchange::TimedOut { secs: t.as_secs() },
            Failure::Status(s) => Failure::status(s),
        }
    }
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport")
            .field("ca_file", &self.ca_file)
            .field("timeout", &self.timeout)
            .field("max_reply_bytes", &self.max_reply_bytes)
            .finish()
    }
}

/// A TLS configuration trusting the authorities in one PEM bundle.
fn tls_config(bundle: &Path) -> Result<Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::empty();
    let certs = CertificateDer::pem_file_iter(bundle).map_err(|e| {
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

impl Transport {
    /// A client for `who`, trusting `ca_file` (or a system bundle), giving each
    /// exchange `timeout_secs` and accepting replies up to
    /// `max_reply_bytes`.
    pub(crate) fn new(
        who: &'static str,
        ca_file: Option<PathBuf>,
        timeout_secs: u64,
        max_reply_bytes: u64,
    ) -> Transport {
        Transport {
            who,
            ca_file,
            timeout: Duration::from_secs(timeout_secs),
            max_reply_bytes,
            tls: Mutex::new(None),
        }
    }

    /// The TLS configuration, made on the first `https` request and kept
    /// once made. A failure is not kept: a bundle installed later is found
    /// by the next request.
    fn tls(&self) -> Result<Arc<rustls::ClientConfig>, String> {
        let mut kept = self.tls.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(c) = kept.as_ref() {
            return Ok(c.clone());
        }
        let bundle: PathBuf = match &self.ca_file {
            Some(f) => f.clone(),
            None => SYSTEM_BUNDLES
                .iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
                .ok_or_else(|| {
                    format!(
                        "no certificate bundle was found at a well-known path; name one as \
                         `ca_file` in {}'s configuration",
                        self.who
                    )
                })?,
        };
        let made = tls_config(&bundle)?;
        *kept = Some(made.clone());
        Ok(made)
    }

    /// Check, before any request, that TLS can be set up for `target`:
    /// its host is a name TLS can verify and, when the configuration
    /// names a certificate bundle, the bundle holds a certificate. The
    /// configuration made is kept. Without a named bundle, the system's
    /// is still looked up at the first request.
    pub(crate) fn check_tls(&self, target: &Target) -> Result<(), String> {
        if !target.secure {
            return Ok(());
        }
        let address = target.host.trim_start_matches('[').trim_end_matches(']');
        ServerName::try_from(address.to_owned())
            .map_err(|e| format!("`{}` is not a name TLS can verify: {e}", target.host))?;
        if self.ca_file.is_some() {
            self.tls()?;
        }
        Ok(())
    }

    /// `POST` `body` as JSON to `target`, with `headers` besides `Host`,
    /// `Content-Type` and `Accept`; the reply body when the status is
    /// `2xx`.
    pub(crate) async fn post(
        &self,
        target: &Target,
        headers: &[(HeaderName, HeaderValue)],
        body: Vec<u8>,
    ) -> Result<Bytes, Failure> {
        let mut request = Request::post(target.path.as_str())
            .header(HOST, target.authority.as_str())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json");
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let request = request.body(Full::new(Bytes::from(body))).map_err(|e| {
            let detail = Detail::scrubbed(e);
            Failure::Refused(Exchange::CannotRequest { detail })
        })?;
        let host = target.host.as_str();
        let port = target.port;
        let address = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let limit = self.max_reply_bytes;
        // TLS is set up before anything is sent to the host.
        let tls = match target.secure {
            true => {
                let name = ServerName::try_from(address.clone()).map_err(|e| {
                    let (host, detail) = (host.to_owned(), Detail::scrubbed(e));
                    Failure::Refused(Exchange::NotVerifiable { host, detail })
                })?;
                let config = self.tls().map_err(Failure::Tls)?;
                Some((name, tokio_rustls::TlsConnector::from(config)))
            }
            false => None,
        };
        let exchange = async {
            let tcp = TcpStream::connect((address.as_str(), port))
                .await
                .map_err(|e| {
                    let (host, detail) = (host.to_owned(), Detail::io(&e));
                    Failure::Failed(Exchange::CannotConnect { host, port, detail })
                })?;
            match tls {
                Some((name, connector)) => {
                    let tls = connector.connect(name, tcp).await.map_err(|e| {
                        let (host, detail) = (host.to_owned(), Detail::io(&e));
                        Failure::Failed(Exchange::TlsFailed { host, port, detail })
                    })?;
                    send(tls, request, limit).await
                }
                None => send(tcp, request, limit).await,
            }
        };
        tokio::time::timeout(self.timeout, exchange)
            .await
            .map_err(|_| Failure::TimedOut(self.timeout))?
    }
}

/// One request over one connection; the reply body when the status is
/// `2xx`.
async fn send<S>(io: S, request: Request<Full<Bytes>>, limit: u64) -> Result<Bytes, Failure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io))
        .await
        .map_err(|e| {
            let detail = Detail::scrubbed(e);
            Failure::Failed(Exchange::Http { detail })
        })?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let reply = sender.send_request(request).await.map_err(|e| {
        let detail = Detail::scrubbed(e);
        Failure::Failed(Exchange::Http { detail })
    })?;
    let status = reply.status();
    if !status.is_success() {
        return Err(Failure::Status(status));
    }
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    Limited::new(reply.into_body(), limit)
        .collect()
        .await
        .map(|b| b.to_bytes())
        .map_err(|e| {
            Failure::Body(Exchange::Body {
                detail: Detail::scrubbed(format_args!("at most {limit} bytes: {e}")),
            })
        })
}

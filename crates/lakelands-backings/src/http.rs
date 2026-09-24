//! The `http` backing: `POST` the call as JSON to the URL a function
//! names, and read the reply body as the output.
//!
//! A function names its URL in its configuration (`config: { url: … }`),
//! `http` or `https`. The URL's host must be on the operator's list
//! ([`HttpConfig::hosts`]), as `host` or as `host:port`; any other is
//! refused before a connection is made. The body is
//! `{ "function", "inputs", "config" }`; a `2xx` reply's body, JSON, is
//! the output. Any other status fails the call, redirects included: a
//! redirect is never followed, since it could lead off the list. No proxy
//! is used, and nothing is read from the environment: `https` trusts the
//! certificate authorities of [`HttpConfig::ca_file`], or of the first
//! system bundle found at a well-known path. A call that takes longer than
//! the timeout, connecting included, fails.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::header::{ACCEPT, CONTENT_TYPE, HOST};
use hyper::{Request, Uri};
use hyper_util::rt::TokioIo;
use lakelands_api::Result;
use lakelands_api::seam::{FunctionBacking, FunctionCall};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use crate::{HttpConfig, body, failed, output, refused};

/// The backing kind.
pub const KIND: &str = "http";

/// Where systems keep their bundle of trusted certificate authorities,
/// tried in this order when the configuration names none.
const SYSTEM_BUNDLES: &[&str] = &[
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/ca-bundle.pem",
    "/etc/ssl/cert.pem",
    "/usr/local/etc/openssl/cert.pem",
];

/// The `http` backing.
pub struct Http {
    config: HttpConfig,
    tls: Mutex<Option<Arc<rustls::ClientConfig>>>,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http")
            .field("config", &self.config)
            .finish()
    }
}

/// A TLS configuration trusting the authorities in one PEM bundle.
fn tls_config(bundle: &Path) -> std::result::Result<Arc<rustls::ClientConfig>, String> {
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

impl Http {
    /// The backing, allowed the hosts of `config`.
    pub fn new(config: HttpConfig) -> Http {
        Http {
            config,
            tls: Mutex::new(None),
        }
    }

    /// The TLS configuration, made on the first `https` call and kept
    /// once made. A failure is not kept: a bundle installed later is found
    /// by the next call.
    fn tls(&self) -> std::result::Result<Arc<rustls::ClientConfig>, String> {
        let mut kept = self.tls.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(c) = kept.as_ref() {
            return Ok(c.clone());
        }
        let bundle: PathBuf = match &self.config.ca_file {
            Some(f) => f.clone(),
            None => SYSTEM_BUNDLES
                .iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
                .ok_or_else(|| {
                    "no certificate bundle was found at a well-known path; name one as \
                     `ca_file` in the http backing's configuration"
                        .to_owned()
                })?,
        };
        let made = tls_config(&bundle)?;
        *kept = Some(made.clone());
        Ok(made)
    }

    fn allows(&self, host: &str, port: u16) -> bool {
        let with_port = format!("{host}:{port}");
        self.config
            .hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(host) || h.eq_ignore_ascii_case(&with_port))
    }

    async fn post(&self, call: &FunctionCall) -> Result<Value> {
        let url = call.config["url"].as_str().ok_or_else(|| {
            refused(
                call,
                "an `http` function names its URL as `config.url`".into(),
            )
        })?;
        let uri: Uri = url
            .parse()
            .map_err(|e| refused(call, format!("`config.url` is not a URL: {e}")))?;
        let secure = match uri.scheme_str() {
            Some("https") => true,
            Some("http") => false,
            _ => {
                return Err(refused(
                    call,
                    "`config.url` is neither an `http` nor an `https` URL".into(),
                ));
            }
        };
        let host = uri
            .host()
            .ok_or_else(|| refused(call, "`config.url` names no host".into()))?;
        let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
        if !self.allows(host, port) {
            return Err(refused(
                call,
                format!("the host `{host}` (port {port}) is not one the operator allows"),
            ));
        }
        // Built from the host and an explicit port only: the URL's
        // authority may carry user information, which is never sent.
        let authority = match uri.port_u16() {
            Some(p) => format!("{host}:{p}"),
            None => host.to_owned(),
        };
        let path = uri.path_and_query().map_or("/", |p| p.as_str()).to_owned();
        // What a failure names: never the URL as declared, which may carry
        // user information and reaches every reader of the field's error.
        let target = format!("{host}:{port}{}", uri.path());
        let request = Request::post(path)
            .header(HOST, authority)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .body(Full::new(Bytes::from(body(call))))
            .map_err(|e| refused(call, format!("{target} cannot be requested: {e}")))?;
        let address = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let limit = self.config.max_reply_bytes;
        // TLS is set up before anything is sent to the host.
        let tls = match secure {
            true => {
                let name = ServerName::try_from(address.clone()).map_err(|e| {
                    refused(call, format!("`{host}` is not a name TLS can verify: {e}"))
                })?;
                let config = self.tls().map_err(|why| failed(call, why))?;
                Some((name, tokio_rustls::TlsConnector::from(config)))
            }
            false => None,
        };
        let exchange = async {
            let tcp = TcpStream::connect((address.as_str(), port))
                .await
                .map_err(|e| format!("cannot connect to {host}:{port}: {e}"))?;
            match tls {
                Some((name, connector)) => {
                    let tls = connector
                        .connect(name, tcp)
                        .await
                        .map_err(|e| format!("TLS with {host}:{port} failed: {e}"))?;
                    send(tls, request, limit).await
                }
                None => send(tcp, request, limit).await,
            }
        };
        let timeout = Duration::from_secs(self.config.timeout_secs);
        let reply = tokio::time::timeout(timeout, exchange)
            .await
            .map_err(|_| {
                failed(
                    call,
                    format!("{target} did not answer within {}s", timeout.as_secs()),
                )
            })?
            .map_err(|why| failed(call, why))?;
        output(call, &reply)
    }
}

/// One request over one connection; the reply body when the status is
/// `2xx`.
async fn send<S>(
    io: S,
    request: Request<Full<Bytes>>,
    limit: u64,
) -> std::result::Result<Bytes, String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io))
        .await
        .map_err(|e| format!("HTTP failed: {e}"))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let reply = sender
        .send_request(request)
        .await
        .map_err(|e| format!("HTTP failed: {e}"))?;
    let status = reply.status();
    if status.is_redirection() {
        return Err(format!(
            "it answered a redirect ({status}), which the http backing does not follow"
        ));
    }
    if !status.is_success() {
        return Err(format!("it answered {status}"));
    }
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    Limited::new(reply.into_body(), limit)
        .collect()
        .await
        .map(|b| b.to_bytes())
        .map_err(|e| format!("its reply could not be read (at most {limit} bytes): {e}"))
}

#[async_trait]
impl FunctionBacking for Http {
    fn kind(&self) -> &str {
        KIND
    }

    async fn call(&self, call: &FunctionCall) -> Result<Value> {
        self.post(call).await
    }
}

//! `https`: the server is verified against the configured authorities
//! and by its name, before anything is sent. A fake behind a test
//! certificate for `localhost` (`tests/tls/`) answers the head.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::error::Error;
use lakelands_api::id::Position;
use lakelands_api::location::Limits;
use lakelands_api::{LakeId, LakeStore};
use lakelands_client::{DEFAULT_MAX_ANSWER_BYTES, RemoteConfig, RemoteLake};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

fn tls_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/tls")
        .join(name)
}

/// A TLS server for `localhost` that answers every request with a `200`
/// carrying position 7.
async fn fake() -> SocketAddr {
    let certs = CertificateDer::pem_file_iter(tls_file("server.pem"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file(tls_file("server.key")).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                // A client that refuses the certificate ends the handshake.
                let Ok(mut s) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut head = Vec::new();
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && s.read(&mut b).await.unwrap_or(0) > 0 {
                    head.push(b[0]);
                }
                let body = "{}";
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                     lakelands-position: 7\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(reply.as_bytes()).await;
                let _ = s.shutdown().await;
            });
        }
    });
    addr
}

fn remote(url: String, ca: &str) -> RemoteLake {
    RemoteLake::connect(RemoteConfig {
        url,
        lake: LakeId::from("finance"),
        keys: BTreeMap::from([("owner".to_owned(), "k-owner".into())]),
        feed_key: Some("owner".into()),
        ca_file: Some(tls_file(ca)),
        // Generous for a loaded machine: it only ends a hang (lakelands#158).
        timeout_secs: 60,
        max_answer_bytes: DEFAULT_MAX_ANSWER_BYTES,
        limits: Limits::default(),
    })
    .expect("the client is configured")
}

/// The server's certificate, from the configured authority and for the
/// name the URL gives: the answer comes through.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_the_authority_vouches_for_under_its_name_is_answered() {
    let addr = fake().await;
    let lake = remote(format!("https://localhost:{}/", addr.port()), "ca.pem");
    assert_eq!(lake.head().await.unwrap(), Position(7));
}

/// An authority other than the configured one vouches for nothing, and
/// a name the certificate does not hold is refused, though the authority
/// is right: both are `Unavailable`, failing TLS, and no key is sent.
#[tokio::test(flavor = "multi_thread")]
async fn another_authority_or_another_name_fails_tls() {
    let addr = fake().await;
    for (url, ca) in [
        (
            format!("https://localhost:{}/", addr.port()),
            "other-ca.pem",
        ),
        (format!("https://127.0.0.1:{}/", addr.port()), "ca.pem"),
    ] {
        let e = remote(url.clone(), ca).head().await.unwrap_err();
        let Error::Unavailable { situation } = &e else {
            panic!("{url} with {ca}: expected unavailable, got {e:?}");
        };
        let message = situation.to_string();
        assert!(message.contains("failed TLS"), "{url} with {ca}: {message}");
        assert!(
            message.contains("the REST door of lake `finance`"),
            "{message}"
        );
        assert!(!message.contains("k-owner"), "{message}");
    }
}

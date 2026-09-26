//! The one door to the server: every connection the engine opens, and
//! every statement it runs, go through here.
//!
//! **Configuration only.** A connection is made from the location: its
//! URL names the server, the database and the credentials; the TLS mode,
//! the certificate authorities and a client certificate come from
//! `PgTls`; the timeouts from `PgTuning`. The driver is handed a parsed
//! configuration and reads no libpq variable and no `.pgpass`, and a URL
//! that names no user is refused, since the driver would otherwise take
//! the operating system's. A URL that
//! carries `sslmode`, `sslrootcert`, `sslcert` or `sslkey` is refused: TLS
//! is configured in one place, not two.
//!
//! **TLS** is rustls. `require` verifies the server's certificate and its
//! name against `ca_file`, or the first system bundle found at a
//! well-known path, and presents a client certificate when one is
//! configured; `disable` is plaintext.
//!
//! **The driver's runtime.** The synchronous client owns a runtime of its
//! own and blocks on it, which panics on a thread that is already driving
//! async tasks. [`driver`] runs each call where the client can block: as
//! is on a plain or blocking-pool thread, through `block_in_place` on a
//! multi-threaded runtime's worker, on a thread of its own on a
//! single-threaded runtime.
//!
//! **Prepared statements.** A connection prepares each statement text
//! once and keeps it for as long as it lives, so a statement run again
//! costs one round trip, not a parse and an execute. The texts are the
//! engine's own, the schema's name and fixed SQL with every value bound as
//! a parameter, so there are as many as the engine has statements. Each
//! run is planned for its values (`plan_cache_mode`), set as the
//! connection starts: a generic plan kept from when a table was small
//! would go on scanning it whole.
//!
//! **Session state.** The lease is a session-level lock, so a handle
//! opened for writing first checks, with [`session_keeping_state`], that
//! nothing between it and the server hands its session to anyone else.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lakelands_api::error::Result;
use lakelands_api::location::{PgTls, PgTlsMode, PgTuning};
use postgres::types::ToSql;
use postgres::{Client, NoTls, Row, SimpleQueryMessage, Statement};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::{invalid, sql, unavailable};

/// The oldest server the engine supports, as `server_version_num` says it.
pub const MIN_SERVER_VERSION: i32 = 160_000;

/// Where systems keep their bundle of trusted certificate authorities,
/// tried in this order when the configuration names none.
const SYSTEM_BUNDLES: &[&str] = &[
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/ca-bundle.pem",
    "/etc/ssl/cert.pem",
    "/usr/local/etc/openssl/cert.pem",
];

/// The URL keys that configure TLS, which the location configures instead.
const TLS_KEYS: [&str; 4] = ["sslmode", "sslrootcert", "sslcert", "sslkey"];

/// A lake's place on a server, as its location names it.
#[derive(Clone, Debug)]
pub(crate) struct Target {
    pub(crate) url: String,
    pub(crate) schema: String,
    pub(crate) tls: PgTls,
    pub(crate) tuning: PgTuning,
}

impl Target {
    /// The schema's name as SQL writes it: quoted, so it is taken exactly
    /// as written. Refused when an identifier cannot hold it.
    pub(crate) fn quoted_schema(&self) -> Result<String> {
        let s = &self.schema;
        if s.is_empty() || s.len() > 63 || s.contains('\0') {
            return Err(invalid(format!(
                "the schema `{s}` cannot be a PostgreSQL identifier: a name of 1 to 63 bytes \
                 without NUL is needed; name one in the location's `schema`"
            )));
        }
        Ok(format!("\"{}\"", s.replace('"', "\"\"")))
    }
}

/// Run one call of the synchronous client where it can block.
pub(crate) fn driver<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current().map(|h| h.runtime_flavor()) {
        Err(_) => f(),
        Ok(RuntimeFlavor::MultiThread) => tokio::task::block_in_place(f),
        Ok(_) => std::thread::scope(|s| {
            s.spawn(f)
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        }),
    }
}

/// One connection, every statement through [`driver`]; closed through it
/// too, since closing blocks on the driver's runtime as a statement does.
/// It keeps the statements it prepared, by their text, until it is closed.
pub(crate) struct Db {
    client: Option<Client>,
    prepared: HashMap<String, Statement>,
}

impl Drop for Db {
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            driver(move || drop(client));
        }
    }
}

type Params<'a> = &'a [&'a (dyn ToSql + Sync)];

impl Db {
    pub(crate) fn batch(&mut self, statements: &str) -> Result<()> {
        let client = self.client.as_mut().expect("a connection until dropped");
        driver(move || client.batch_execute(statements)).map_err(sql)
    }

    /// Run statements as one simple query, one round trip, and answer the
    /// first column of every row they return, as text.
    pub(crate) fn simple(&mut self, statements: &str) -> Result<Vec<Option<String>>> {
        let client = self.client.as_mut().expect("a connection until dropped");
        let messages = driver(move || client.simple_query(statements)).map_err(sql)?;
        Ok(messages
            .iter()
            .filter_map(|m| match m {
                SimpleQueryMessage::Row(row) => Some(row.get(0).map(str::to_owned)),
                _ => None,
            })
            .collect())
    }

    /// The connection, and the statement prepared for `text` on it:
    /// prepared now if it was not yet.
    fn prepared(&mut self, text: &str) -> Result<(&mut Client, Statement)> {
        let client = self.client.as_mut().expect("a connection until dropped");
        let statement = match self.prepared.get(text) {
            Some(statement) => statement.clone(),
            None => {
                let statement = driver(|| client.prepare(text)).map_err(sql)?;
                self.prepared.insert(text.to_owned(), statement.clone());
                statement
            }
        };
        Ok((client, statement))
    }

    pub(crate) fn execute(&mut self, statement: &str, params: Params<'_>) -> Result<u64> {
        let (client, statement) = self.prepared(statement)?;
        driver(move || client.execute(&statement, params)).map_err(sql)
    }

    pub(crate) fn query(&mut self, statement: &str, params: Params<'_>) -> Result<Vec<Row>> {
        let (client, statement) = self.prepared(statement)?;
        driver(move || client.query(&statement, params)).map_err(sql)
    }

    pub(crate) fn query_opt(&mut self, statement: &str, params: Params<'_>) -> Result<Option<Row>> {
        let (client, statement) = self.prepared(statement)?;
        driver(move || client.query_opt(&statement, params)).map_err(sql)
    }

    /// Run a query and hand each row to `f` as it arrives, holding one row
    /// at a time, never the answer.
    pub(crate) fn query_each(
        &mut self,
        statement: &str,
        params: Params<'_>,
        mut f: impl FnMut(&Row) -> Result<()> + Send,
    ) -> Result<()> {
        use postgres::fallible_iterator::FallibleIterator;
        let (client, statement) = self.prepared(statement)?;
        driver(move || {
            let mut rows = client
                .query_raw(&statement, params.iter().copied())
                .map_err(sql)?;
            while let Some(row) = rows.next().map_err(sql)? {
                f(&row)?;
            }
            Ok(())
        })
    }

    pub(crate) fn query_one(&mut self, statement: &str, params: Params<'_>) -> Result<Row> {
        let (client, statement) = self.prepared(statement)?;
        driver(move || client.query_one(&statement, params)).map_err(sql)
    }

    /// Run `f` in one transaction opened by `begin`: committed when it
    /// answers `Ok`, rolled back otherwise.
    pub(crate) fn transaction<R>(
        &mut self,
        begin: &str,
        f: impl FnOnce(&mut Db) -> Result<R>,
    ) -> Result<R> {
        self.batch(begin)?;
        match f(self) {
            Ok(r) => {
                self.batch("COMMIT")?;
                Ok(r)
            }
            Err(e) => {
                let _ = self.batch("ROLLBACK");
                Err(e)
            }
        }
    }
}

/// A PEM file's certificates.
fn certificates(path: &Path, what: &str) -> Result<Vec<CertificateDer<'static>>> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(path)
        .and_then(|i| i.collect::<std::result::Result<_, _>>())
        .map_err(|e| unavailable(format!("the {what} {} cannot be read: {e}", path.display())))?;
    if certs.is_empty() {
        return Err(unavailable(format!(
            "the {what} {} holds no certificate",
            path.display()
        )));
    }
    Ok(certs)
}

/// The rustls configuration `require` connects with.
fn tls_config(tls: &PgTls) -> Result<rustls::ClientConfig> {
    tls_config_with(tls, SYSTEM_BUNDLES)
}

/// As [`tls_config`], looking for the system's bundle at `bundles`.
fn tls_config_with(tls: &PgTls, bundles: &[&str]) -> Result<rustls::ClientConfig> {
    let bundle: PathBuf = match &tls.ca_file {
        Some(f) => f.clone(),
        None => bundles
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
            .ok_or_else(|| {
                unavailable(
                    "no certificate bundle was found at a well-known path; name one as the \
                     location's `tls.ca_file`, or set `tls.mode` to `disable` for a server you \
                     reach without TLS",
                )
            })?,
    };
    let mut roots = rustls::RootCertStore::empty();
    roots.add_parsable_certificates(certificates(&bundle, "certificate bundle")?);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| unavailable(format!("TLS cannot be set up: {e}")))?
        .with_root_certificates(roots);
    match (&tls.client_cert, &tls.client_key) {
        (None, None) => Ok(builder.with_no_client_auth()),
        (Some(cert), Some(key)) => {
            let chain = certificates(cert, "client certificate")?;
            let key_der = PrivateKeyDer::from_pem_file(key).map_err(|e| {
                unavailable(format!(
                    "the client key {} cannot be read: {e}",
                    key.display()
                ))
            })?;
            builder
                .with_client_auth_cert(chain, key_der)
                .map_err(|e| unavailable(format!("the client certificate cannot be used: {e}")))
        }
        _ => Err(invalid(
            "a client certificate needs both `tls.client_cert` and `tls.client_key`",
        )),
    }
}

/// Open one connection to the target's server.
pub(crate) fn connect(target: &Target) -> Result<Db> {
    for key in TLS_KEYS {
        if target.url.contains(&format!("{key}=")) {
            return Err(invalid(format!(
                "the location's URL carries `{key}`; TLS is configured by the location's `tls` \
                 alone, so remove it from the URL"
            )));
        }
    }
    let mut config: postgres::Config = target
        .url
        .parse()
        .map_err(|e| invalid(format!("the location's URL is not a PostgreSQL URL: {e}")))?;
    // Without a user the driver would take the operating system's: the
    // location names who connects.
    if config.get_user().is_none() {
        return Err(invalid(
            "the location's URL names no user; name the one that connects, as \
             `postgres://<user>@<host>/<database>`",
        ));
    }
    config.connect_timeout(Duration::from_secs(u64::from(
        target.tuning.connect_timeout_secs.max(1),
    )));
    // Every statement is planned for the values it runs with. A statement
    // a connection keeps would otherwise settle on a generic plan made
    // while the lake's tables were small, and scan them whole as they grow.
    let options = match config.get_options() {
        Some(o) => format!("{o} -c plan_cache_mode=force_custom_plan"),
        None => "-c plan_cache_mode=force_custom_plan".to_owned(),
    };
    config.options(&options);
    let client = match target.tls.mode {
        PgTlsMode::Disable => {
            config.ssl_mode(postgres::config::SslMode::Disable);
            driver(|| config.connect(NoTls))
        }
        PgTlsMode::Require => {
            config.ssl_mode(postgres::config::SslMode::Require);
            let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config(&target.tls)?);
            driver(|| config.connect(tls))
        }
    }
    .map_err(sql)?;
    let mut db = Db {
        client: Some(client),
        prepared: HashMap::new(),
    };
    let version: i32 = db
        .query_one("SELECT current_setting('server_version_num')::int4", &[])?
        .get(0);
    if version < MIN_SERVER_VERSION {
        return Err(unavailable(format!(
            "the server is PostgreSQL {}.{}; the engine needs PostgreSQL 16 or newer, and \
             nothing was created",
            version / 10_000,
            version % 10_000
        )));
    }
    if target.tuning.statement_timeout_secs > 0 {
        db.batch(&format!(
            "SET statement_timeout = '{}s'",
            target.tuning.statement_timeout_secs
        ))?;
    }
    Ok(db)
}

/// Open the connection that will hold the lease, checking on the way
/// that a session keeps what it sets and inherits nothing from another: a
/// setting made on it must survive to its next statement, and fresh
/// connections must not see it. A transaction-mode pooler fails one or the
/// other, and the lease, a session-level lock, would then be lost or handed
/// to someone else.
pub(crate) fn session_keeping_state(target: &Target) -> Result<Db> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mark = format!("probe-{}-{nanos}", std::process::id());
    let refuse = |what: &str| {
        unavailable(format!(
            "the connection does not keep its session ({what}); a transaction-mode connection \
             pooler seems to sit in front of the server. The writer lease is a session lock, \
             which such a pooler loses or hands to another client, so the lake was not opened \
             for writing. Connect directly or through a session-mode pooler."
        ))
    };
    let mut probe = connect(target)?;
    probe.batch(&format!("SET lakelands.probe = '{mark}'"))?;
    let kept: Option<String> = probe
        .query_one("SELECT current_setting('lakelands.probe', true)", &[])?
        .get(0);
    if kept.as_deref() != Some(mark.as_str()) {
        return Err(refuse("a setting was gone by the next statement"));
    }
    for _ in 0..2 {
        let seen: Option<String> = connect(target)?
            .query_one("SELECT current_setting('lakelands.probe', true)", &[])?
            .get(0);
        if seen.as_deref() == Some(mark.as_str()) {
            return Err(refuse(
                "a fresh connection inherited another session's setting",
            ));
        }
    }
    Ok(probe)
}

#[cfg(test)]
mod tests {
    use lakelands_api::Error;
    use lakelands_api::location::{PgTls, PgTlsMode};

    use super::tls_config_with;

    /// `require` with no `ca_file` on a system with no bundle at any
    /// well-known path refuses to connect as unavailable, and says what to
    /// name: the location's `tls.ca_file`.
    #[test]
    fn require_without_a_ca_file_or_a_system_bundle_is_unavailable_naming_ca_file() {
        let tls = PgTls {
            mode: PgTlsMode::Require,
            ..PgTls::default()
        };
        match tls_config_with(&tls, &[]) {
            Err(Error::Unavailable { message }) => assert!(
                message.contains("tls.ca_file"),
                "the refusal names what to set: {message}"
            ),
            Err(other) => panic!("refused as {other:?}, not as unavailable"),
            Ok(_) => panic!("TLS was set up with no certificate authority at all"),
        }
    }
}

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
//! once and keeps it, so a statement run again costs one round trip, not
//! a parse and an execute. The texts are the engine's own, the schema's
//! name and fixed SQL with every value bound as a parameter, so there are
//! as many as the engine has statements, for each lake a reader served.
//! At [`PREPARED_MOST`] a connection lets one go for each it prepares,
//! each time from the next place in its table, so no one statement goes
//! more often than the rest: a reader the process's lakes share does not
//! grow with every lake it ever read. Each run is planned for its values
//! (`plan_cache_mode`), set as the connection starts: a generic plan kept
//! from when a table was small would go on scanning it whole.
//!
//! **Session state.** The lease is a session-level lock, so a handle
//! opened for writing first checks, with [`session_keeping_state`], that
//! nothing between it and the server hands its session to anyone else.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use lakelands_api::error::{Case, Detail, Error, Result, Situation, Tls};
use lakelands_api::location::{PgTablespaces, PgTls, PgTlsMode, PgTuning};
use lakelands_api::record::StoredRecord;
use lakelands_core::engines::pool::Pool;
use postgres::types::{FromSql, ToSql};
use postgres::{Client, NoTls, Row, SimpleQueryMessage, Statement};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::{lock, sql};

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
    pub(crate) tablespaces: PgTablespaces,
}

impl Target {
    /// The schema's name as SQL writes it: quoted, so it is taken exactly
    /// as written. Refused when an identifier cannot hold it.
    pub(crate) fn quoted_schema(&self) -> Result<String> {
        let s = &self.schema;
        if s.is_empty() || s.len() > 63 || s.contains('\0') {
            let schema = s.clone();
            return Err(Error::invalid(Case::SchemaName { schema }));
        }
        Ok(Target::quote(s))
    }

    /// `name` as SQL writes an identifier: quoted, so it is taken exactly
    /// as written.
    pub(crate) fn quote(name: &str) -> String {
        format!("\"{}\"", name.replace('"', "\"\""))
    }

    /// The schema the projection a rebuild builds beside this one's lives
    /// in (`scratch.rs`): named from it by a digest, so it is the same for
    /// every handle onto the lake, fits an identifier however long the
    /// lake's schema is, and is found by no other lake.
    pub(crate) fn scratch_schema(&self) -> String {
        use sha2::{Digest as _, Sha256};
        let digest = Sha256::digest(self.schema.as_bytes());
        format!(
            "lakelands_rebuild_{}",
            lakelands_core::canonical::hex(&digest[..8])
        )
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

/// The most statements one connection keeps prepared.
const PREPARED_MOST: usize = 512;

/// One connection, every statement through [`driver`]; closed through it
/// too, since closing blocks on the driver's runtime as a statement does.
/// It keeps the statements it prepared, by their text.
pub(crate) struct Db {
    client: Option<Client>,
    prepared: HashMap<String, Statement>,
    /// How many [`Db::prepared`] let go: the next goes from the next place.
    let_go: usize,
    /// A `COMMIT` was sent since [`Db::transaction`] last began one.
    committing: bool,
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
    /// Whether the server or the network closed the connection: what was
    /// sent on it last failed, or no statement can be sent on it now.
    /// Whether a `COMMIT` was sent since [`Db::transaction`] last began:
    /// a connection that closes after it leaves the outcome unknown.
    pub(crate) fn committing(&self) -> bool {
        self.committing
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.client.as_ref().is_none_or(Client::is_closed)
    }

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
                if self.prepared.len() >= PREPARED_MOST {
                    self.let_go += 1;
                    let at = self.let_go % self.prepared.len();
                    if let Some(old) = self.prepared.keys().nth(at).cloned() {
                        self.prepared.remove(&old);
                    }
                }
                self.prepared.insert(text.to_owned(), statement.clone());
                statement
            }
        };
        Ok((client, statement))
    }

    pub(crate) fn execute(&mut self, statement: &str, params: Params<'_>) -> Result<()> {
        let (client, statement) = self.prepared(statement)?;
        driver(move || client.execute(&statement, params).map(drop)).map_err(sql)
    }

    pub(crate) fn query(&mut self, statement: &str, params: Params<'_>) -> Result<Vec<Row>> {
        let (client, statement) = self.prepared(statement)?;
        driver(move || client.query(&statement, params)).map_err(sql)
    }

    /// The first column of every row a query answers.
    pub(crate) fn column<T: for<'r> FromSql<'r>, C: FromIterator<T>>(
        &mut self,
        text: &str,
        args: Params<'_>,
    ) -> Result<C> {
        Ok(self.query(text, args)?.iter().map(|r| r.get(0)).collect())
    }

    /// The records a query's rows hold, each from its first column on as
    /// [`crate::projection::record_at`] reads it, joined to the rows of
    /// the vectors their states keep apart and of the bases they name in
    /// `names`' lake.
    pub(crate) fn records(
        &mut self,
        names: &crate::projection::Names,
        query: &str,
        params: Params<'_>,
    ) -> Result<Vec<StoredRecord>> {
        let rows = self.query(query, params)?;
        crate::projection::records_at(self, names, &rows, 0)
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
        self.committing = false;
        self.batch(begin)?;
        let answer = f(self);
        self.end(answer)
    }

    /// End the open transaction: commit it when `answer` is `Ok`, roll it
    /// back otherwise, and hand `answer` on.
    pub(crate) fn end<R>(&mut self, answer: Result<R>) -> Result<R> {
        match answer {
            Ok(r) => {
                self.committing = true;
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

/// TLS cannot be set up, for this reason.
fn tls_failed(problem: Tls, detail: Option<Detail>) -> Error {
    Error::unavailable(Situation::Tls { problem, detail })
}

/// A PEM file's certificates: the client's, or the authority's.
fn certificates(path: &Path, client: bool) -> Result<Vec<CertificateDer<'static>>> {
    let (unreadable, empty) = match client {
        true => (Tls::CertUnreadable, Tls::CertEmpty),
        false => (Tls::CaUnreadable, Tls::CaEmpty),
    };
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(path)
        .and_then(|i| i.collect::<std::result::Result<_, _>>())
        .map_err(|e| tls_failed(unreadable, Some(Detail::scrubbed(e))))?;
    if certs.is_empty() {
        return Err(tls_failed(empty, None));
    }
    Ok(certs)
}

/// The rustls configuration `require` connects with, the system's
/// bundle looked for at `bundles` when the location names none.
fn tls_config_with(tls: &PgTls, bundles: &[&str]) -> Result<rustls::ClientConfig> {
    let bundle: PathBuf = match &tls.ca_file {
        Some(f) => f.clone(),
        None => bundles
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
            .ok_or_else(|| tls_failed(Tls::NoBundle, None))?,
    };
    let mut roots = rustls::RootCertStore::empty();
    roots.add_parsable_certificates(certificates(&bundle, false)?);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| tls_failed(Tls::Setup, Some(Detail::scrubbed(e))))?
        .with_root_certificates(roots);
    match (&tls.client_cert, &tls.client_key) {
        (None, None) => Ok(builder.with_no_client_auth()),
        (Some(cert), Some(key)) => {
            let chain = certificates(cert, true)?;
            let key_der = PrivateKeyDer::from_pem_file(key)
                .map_err(|e| tls_failed(Tls::KeyUnreadable, Some(Detail::scrubbed(e))))?;
            builder
                .with_client_auth_cert(chain, key_der)
                .map_err(|e| tls_failed(Tls::CertUnusable, Some(Detail::scrubbed(e))))
        }
        _ => Err(Error::invalid(Case::ClientCertHalf)),
    }
}

/// The most readers a process's lakes on one server share. A lake's reads
/// still gained some past 16, but 16 leave a server's default 100
/// connections to some 40 writing lakes.
pub(crate) const SHARED_READERS: usize = 16;

/// The readers a process's lakes on one server share: one pool per
/// server, database, credentials, TLS and tuning, kept while a handle onto
/// one of its lakes is open. So a process holds at most [`SHARED_READERS`]
/// for each of those, however many lakes it opens there, and not fewer on
/// a machine of fewer cores: the reads run on the server's. Every
/// statement names its schema, so a reader serves any lake there.
pub(crate) fn shared_readers(target: &Target) -> Arc<Pool<Db>> {
    static POOLS: Mutex<Vec<(String, Weak<Pool<Db>>)>> = Mutex::new(Vec::new());
    let server = format!("{:?}", (&target.url, &target.tls, &target.tuning));
    let mut pools = lock(&POOLS);
    pools.retain(|(_, pool)| pool.strong_count() > 0);
    let found = pools.iter().find(|(s, _)| *s == server);
    if let Some(pool) = found.and_then(|(_, pool)| pool.upgrade()) {
        return pool;
    }
    let pool = Arc::new(Pool::new(Vec::new(), SHARED_READERS));
    pools.push((server, Arc::downgrade(&pool)));
    pool
}

/// Open one of the readers [`shared_readers`] keeps, named for the
/// process, not a lake.
pub(crate) fn reader(target: &Target) -> Result<Db> {
    open(target, &format!("lakelands readers {}", std::process::id()))
}

/// Open one connection to the target's server, for its lake.
pub(crate) fn connect(target: &Target) -> Result<Db> {
    open(target, &format!("lakelands {}", target.schema))
}

/// Open one connection to the target's server, named `name`.
fn open(target: &Target, name: &str) -> Result<Db> {
    for key in TLS_KEYS {
        if target.url.contains(&format!("{key}=")) {
            let key = key.to_owned();
            return Err(Error::invalid(Case::LocationUrlTls { key }));
        }
    }
    let mut config: postgres::Config = target.url.parse().map_err(|e| {
        let detail = Detail::scrubbed(e);
        Error::invalid(Case::LocationUrlUnparsed { detail })
    })?;
    // Without a user the driver would take the operating system's: the
    // location names who connects.
    if config.get_user().is_none() {
        return Err(Error::invalid(Case::LocationUrlNoUser));
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
    // An operator finds the lake's connections by name in
    // `pg_stat_activity`.
    config.application_name(name);
    let client = match target.tls.mode {
        PgTlsMode::Disable => {
            config.ssl_mode(postgres::config::SslMode::Disable);
            driver(|| config.connect(NoTls))
        }
        PgTlsMode::Require => {
            config.ssl_mode(postgres::config::SslMode::Require);
            let tls = tls_config_with(&target.tls, SYSTEM_BUNDLES)?;
            let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls);
            driver(|| config.connect(tls))
        }
    }
    .map_err(sql)?;
    let mut db = Db {
        client: Some(client),
        prepared: HashMap::new(),
        let_go: 0,
        committing: false,
    };
    let version: i32 = db
        .query_one("SELECT current_setting('server_version_num')::int4", &[])?
        .get(0);
    if version < MIN_SERVER_VERSION {
        let (major, minor) = ((version / 10_000) as u32, (version % 10_000) as u32);
        return Err(Error::unavailable(Situation::ServerTooOld { major, minor }));
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
    let refuse = || Error::unavailable(Situation::Pooler);
    let mut probe = connect(target)?;
    probe.batch(&format!("SET lakelands.probe = '{mark}'"))?;
    let kept: Option<String> = probe
        .query_one("SELECT current_setting('lakelands.probe', true)", &[])?
        .get(0);
    if kept.as_deref() != Some(mark.as_str()) {
        // A setting was gone by the next statement.
        return Err(refuse());
    }
    for _ in 0..2 {
        let seen: Option<String> = connect(target)?
            .query_one("SELECT current_setting('lakelands.probe', true)", &[])?
            .get(0);
        if seen.as_deref() == Some(mark.as_str()) {
            // A fresh connection inherited another session's setting.
            return Err(refuse());
        }
    }
    Ok(probe)
}

#[cfg(test)]
mod tests;

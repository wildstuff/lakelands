//! A lake another process serves, over its REST door (`docs/rest.md`):
//! the contract's [`LakeStore`], [`Catalog`] and [`ChangeFeed`] for a
//! caller that does not hold the lake itself.
//!
//! One contract, a second implementation beside the store:
//! the door checks run unchanged against it, behind `serve_http` over a
//! store. Each door is its REST route and nothing more; the client adds
//! no semantics of its own.
//!
//! - **Who acts.** Over REST the principal is what the server resolves
//!   from the key. The client holds one API key per principal id it may
//!   act as; a call as `who` sends the key of `who.id`. The grants inside
//!   `who` are neither sent nor trusted, so the server's answer is the
//!   one that holds. A `who` with no key, or of another lake, is refused
//!   as `not_granted` before anything is sent.
//! - **Keys** are given as values: the crate reads no file and nothing
//!   from the environment, and a key never appears in `Debug`, in an
//!   error or in a message.
//! - **Errors.** A refusal comes back as the contract's own [`Error`];
//!   a failure below it (connecting, TLS, no answer in time, a reply that
//!   is not the contract's) is `Unavailable`, naming the lake's REST door
//!   and what failed, never its address.
//! - **Host operations** — `verify`, `rebuild`, `restrict`, `unrestrict`,
//!   `checkpoint`, `age`, `purge_source` — belong to the host that holds
//!   the store, and no network door offers them: each is `Unbound`,
//!   naming the operation.
//! - **The feed** is the log itself, unmasked, so it is read as the
//!   configured `feed_key` principal, one that reads everything
//!   (`docs/decisions/037-the-feed-over-a-network-door.md`).

#![forbid(unsafe_code)]

mod connect;
mod doors;
mod feed;
mod wire;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lakelands_api::error::{Error, Result};
use lakelands_api::id::{LakeId, Position, RecordKey, SourceId, TypeName};
use lakelands_api::location::Limits;
use lakelands_api::seam::ChangeFeed;
use lakelands_api::time::Timestamp;
use lakelands_api::write::{
    AgeReport, CompactReport, ForgetReport, PurgeReport, RemoveReport, Selection,
};
use lakelands_api::{Catalog, LakeStore, Verification, WriteContext};

use crate::wire::Wire;

/// How long a feed stream may stay silent before it is taken for dead:
/// two of the server's fifteen-second keep-alives.
const SILENCE: Duration = Duration::from_secs(30);

/// A cap on one answer that holds any page the server's default limits
/// answer: 64 MiB.
pub const DEFAULT_MAX_ANSWER_BYTES: usize = 64 << 20;

/// An API key. Its `Debug` never shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// The key, for the one header that carries it.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for ApiKey {
    fn from(k: String) -> ApiKey {
        ApiKey(k)
    }
}

impl From<&str> for ApiKey {
    fn from(k: &str) -> ApiKey {
        ApiKey(k.to_owned())
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(…)")
    }
}

/// Where the lake is served, and who may act on it through this client.
#[derive(Clone, Debug)]
pub struct RemoteConfig {
    /// The server's base URL, `http` or `https`, without user-info; the
    /// lakes are under `<url>/lakes/`.
    pub url: String,
    /// The lake.
    pub lake: LakeId,
    /// One API key per principal id the caller may act as.
    pub keys: BTreeMap<String, ApiKey>,
    /// The principal id whose key reads the feed and the head; one that
    /// reads everything.
    pub feed_key: Option<String>,
    /// The certificate authorities an `https` server is verified
    /// against; the first system bundle at a well-known path when absent.
    pub ca_file: Option<PathBuf>,
    /// How long one exchange may take, from connecting to the whole
    /// answer (for a stream, to its head).
    pub timeout_secs: u64,
    /// The most bytes one answer may hold, and one line of an ingest's
    /// answer or one event of the feed; past it the answer is
    /// `Unavailable`, and no more of it is read. A page of records or an
    /// event is as large as the records in it, so this is the caller's
    /// to size; [`DEFAULT_MAX_ANSWER_BYTES`] suits most lakes.
    pub max_answer_bytes: usize,
    /// What [`LakeStore::limits`] answers. They are the caller's, not
    /// the server's: the server holds its own and its refusals win.
    pub limits: Limits,
}

/// Why a configuration cannot make a client. It never holds a key. It is
/// the host's own configuration, said to the host when the client is
/// made, never a lake's answer to a call, so it is not a case of the
/// refusal catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// One lake, served by another process.
pub struct RemoteLake {
    wire: Arc<Wire>,
    limits: Limits,
}

impl fmt::Debug for RemoteLake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteLake")
            .field("lake", &self.wire.lake)
            .field("at", &self.wire.place)
            .finish_non_exhaustive()
    }
}

impl RemoteLake {
    /// A client for the lake the configuration names. Nothing is sent
    /// yet; the URL, the keys and the TLS trust are checked here.
    pub fn connect(config: RemoteConfig) -> std::result::Result<RemoteLake, ConfigError> {
        let wire = Wire::new(&config, SILENCE)?;
        Ok(RemoteLake {
            wire: Arc::new(wire),
            limits: config.limits,
        })
    }

    /// As [`connect`](RemoteLake::connect), with another silence bound
    /// for the feed, so a test need not wait for the real one.
    #[cfg(test)]
    pub(crate) fn connect_with_silence(
        config: RemoteConfig,
        silence: Duration,
    ) -> std::result::Result<RemoteLake, ConfigError> {
        let wire = Wire::new(&config, silence)?;
        Ok(RemoteLake {
            wire: Arc::new(wire),
            limits: config.limits,
        })
    }

    /// As [`connect_with_silence`](RemoteLake::connect_with_silence),
    /// over a connection the test holds the server's end of, so a test
    /// on a paused clock measures the bound exactly.
    #[cfg(test)]
    pub(crate) fn connect_over(
        config: RemoteConfig,
        silence: Duration,
        conn: tokio::io::DuplexStream,
    ) -> std::result::Result<RemoteLake, ConfigError> {
        let wire = Wire::new(&config, silence)?.over(conn);
        Ok(RemoteLake {
            wire: Arc::new(wire),
            limits: config.limits,
        })
    }
}

/// A host operation, which no network door offers.
fn host_only(op: &str) -> Error {
    Error::unbound(lakelands_api::error::Seam::Operation { op: op.to_owned() })
}

#[async_trait]
impl LakeStore for RemoteLake {
    fn lake(&self) -> &LakeId {
        &self.wire.lake
    }

    /// The server's process holds the lease, and every write goes
    /// through it.
    fn is_writer(&self) -> bool {
        true
    }

    fn limits(&self) -> Limits {
        self.limits
    }

    async fn head(&self) -> Result<Position> {
        self.wire.head().await
    }

    /// The doors. The write context is the serving host's, never a
    /// caller's, so `context` is not sent.
    fn catalog(&self, _context: WriteContext) -> Arc<dyn Catalog> {
        Arc::new(doors::RemoteCatalog::new(self.wire.clone()))
    }

    fn feed(&self) -> Arc<dyn ChangeFeed> {
        Arc::new(feed::RemoteFeed::new(self.wire.clone()))
    }

    async fn verify(
        &self,
        _from: Option<Position>,
        _through: Option<Position>,
    ) -> Result<Verification> {
        Err(host_only("verify"))
    }

    async fn rebuild(&self) -> Result<Position> {
        Err(host_only("rebuild"))
    }

    async fn restrict(&self, _: &TypeName, _: &RecordKey, _: String) -> Result<Position> {
        Err(host_only("restrict"))
    }

    async fn unrestrict(&self, _: &TypeName, _: &RecordKey) -> Result<Position> {
        Err(host_only("unrestrict"))
    }

    async fn checkpoint(&self) -> Result<Position> {
        Err(host_only("checkpoint"))
    }

    async fn age(&self, _today: Timestamp) -> Result<AgeReport> {
        Err(host_only("age"))
    }

    async fn purge_source(
        &self,
        _source: &SourceId,
        _ty: Option<&TypeName>,
        _id: Option<&str>,
    ) -> Result<PurgeReport> {
        Err(host_only("purge_source"))
    }

    async fn purge_preflight(
        &self,
        _source: &SourceId,
        _ty: Option<&TypeName>,
    ) -> Result<PurgeReport> {
        Err(host_only("purge_preflight"))
    }

    async fn forget_source(&self, _source: &SourceId, _id: Option<&str>) -> Result<ForgetReport> {
        Err(host_only("forget_source"))
    }

    async fn forget_preflight(&self, _source: &SourceId) -> Result<ForgetReport> {
        Err(host_only("forget_preflight"))
    }

    async fn remove_values(
        &self,
        _source: &SourceId,
        _selection: &Selection,
        _shredded: bool,
        _id: Option<&str>,
    ) -> Result<RemoveReport> {
        Err(host_only("remove_values"))
    }

    async fn remove_values_preflight(
        &self,
        _source: &SourceId,
        _selection: &Selection,
    ) -> Result<RemoveReport> {
        Err(host_only("remove_values_preflight"))
    }

    async fn position_at(&self, _known_at: Timestamp) -> Result<Option<Position>> {
        Err(host_only("position_at"))
    }

    async fn vector_coverage(
        &self,
        _ty: &TypeName,
    ) -> Result<Vec<lakelands_api::read::VectorCoverage>> {
        Err(host_only("vector_coverage"))
    }

    async fn compact(&self) -> Result<CompactReport> {
        Err(host_only("compact"))
    }

    fn bind_embed(&self, _: Arc<dyn lakelands_api::seam::EmbedBackend>) -> Result<()> {
        Err(host_only("bind_embed"))
    }

    /// Nothing is held but pooled connections, which close with the
    /// last handle.
    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

//! A forwarding handle: a lake's handle in a process that does not hold
//! its writer lease, which reads its own engine and forwards every write
//! to the process that holds it
//! (`docs/decisions/076-a-lake-keeps-one-writer-and-a-dataset-that-outgrows-it-becomes-several-lakes.md`).
//!
//! - **The link.** A write goes over the writer's REST door as its host:
//!   with the host's own key, the principal it is made as named by id and
//!   judged by the writer under that principal's grants, and the write
//!   context it carries (`docs/hosting.md`).
//! - **The doors.** The location names the writer doors in order. A call
//!   starts at the one that last answered and goes once round them. A
//!   door that holds no lease, or that was not reached before anything
//!   was sent, passes the call to the next; every other answer is the
//!   call's. None took it: `NotForwarded`, and nothing was written.
//! - **A lost writer.** A call that was sent and then not answered is
//!   `ForwardLost` and is never sent again, so a write lands once at
//!   most; a retry under its idempotency key answers the first outcome.
//! - **Read-your-writes.** A read is answered by the handle's own engine,
//!   once it has folded the highest position a writer has answered this
//!   handle, waiting at most `read_wait_millis`; past it the read is
//!   `NotFoldedYet`, never one from before the handle's own write.
//! - **Host operations** that need the lease (`rebuild`, `restrict`,
//!   `unrestrict`, `checkpoint`, `age`, `purge_source`, the forgettings,
//!   `remove_values`, `compact`) are `ReadOnly`, the lease another's,
//!   whatever they would have done: they are run where the writer is.
//!   The others, the preflights among them, and the feed are the local
//!   handle's, once it has folded the handle's own writes.

use std::cell::Cell;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use lakelands_api::catalog::{CommitStream, SourceStream};
use lakelands_api::describe::{LakeDescription, TypeDescription};
use lakelands_api::error::{Error, Result, Situation};
use lakelands_api::id::{LakeId, Position, ProposalId, RecordKey, RelationName, SourceId};
use lakelands_api::id::{TypeName, VerbName, Version};
use lakelands_api::location::{EngineConfig, Limits, Location};
use lakelands_api::ontology::FieldDecl;
use lakelands_api::principal::Principal;
use lakelands_api::query::{Hits, Pattern, QueryResult, Search};
use lakelands_api::read::{Find, Found, Get, ReadAt, Reply, VectorCoverage};
use lakelands_api::record::{Patch, Record};
use lakelands_api::seam::{ChangeFeed, EmbedBackend, Reranker};
use lakelands_api::time::Timestamp;
use lakelands_api::write::{
    AgeReport, Change, CompactReport, Declaration, ForgetReport, ForgetSetReport, IngestOpts,
    Migration, MigrationReport, Outcome, Plan, Proposal, ProposalFilter, ProposedWrite,
    PurgeReport, Recompute, RemoveReport, Selection, ValueTypeMigration, ValueTypeReport,
    WriteOpts,
};
use lakelands_api::{Catalog, LakeStore, Verification, WriteContext};

use crate::doors::RemoteCatalog;
use crate::wire::{SENT, Sent, Wire};
use crate::{ApiKey, ConfigError, DEFAULT_MAX_ANSWER_BYTES, RemoteConfig, SILENCE};

/// How a forwarding handle reaches the writer doors its location names.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ForwardConfig {
    /// The host's key, one of a writer's configured `hosts`: never a
    /// principal's, and given here by value, never in the location.
    pub credential: ApiKey,
    /// How long one exchange may take, from connecting to the whole
    /// answer (for an ingest, to its head).
    pub timeout_secs: u64,
    /// How long connecting to a door may take, TLS included; less than
    /// the exchange's bound. A door not reached within it was sent
    /// nothing, and the call passes to the next.
    pub connect_timeout_millis: u64,
    /// The most bytes one answer may hold, as for a remote lake.
    pub max_answer_bytes: usize,
    /// How long a read waits for the handle's engine to fold what a
    /// writer has answered it; 0 waits not at all.
    pub read_wait_millis: u64,
}

impl ForwardConfig {
    /// Forwarding with `credential`, under the default bounds: 30 s an
    /// exchange, 2 s to connect, 2 s for a read to see the handle's
    /// writes.
    pub fn new(credential: impl Into<ApiKey>) -> ForwardConfig {
        ForwardConfig {
            credential: credential.into(),
            timeout_secs: 30,
            connect_timeout_millis: 2_000,
            max_answer_bytes: DEFAULT_MAX_ANSWER_BYTES,
            read_wait_millis: 2_000,
        }
    }
}

/// The writer doors, and which last answered.
struct Doors {
    lake: LakeId,
    wires: Vec<Arc<Wire>>,
    last: AtomicUsize,
}

impl Doors {
    /// The highest position a writer has answered this handle.
    fn floor(&self) -> Position {
        (self.wires.iter().map(|w| w.seen()).max()).unwrap_or(Position(0))
    }

    /// `f` at each door in turn, from the one that last answered, until
    /// one takes the call.
    async fn each<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: Fn(usize) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let n = self.wires.len();
        let from = self.last.load(Ordering::SeqCst);
        let mut last = None;
        for i in 0..n {
            let at = (from + i) % n;
            let (answer, sent) = SENT
                .scope(Cell::new(Sent::default()), async {
                    let answer = f(at).await;
                    (answer, SENT.with(Cell::get))
                })
                .await;
            match answer {
                Err(e) if sent.nothing() || holds_no_lease(&e) => last = Some(Box::new(e)),
                answer => {
                    self.last.store(at, Ordering::SeqCst);
                    return answer;
                }
            }
        }
        Err(Error::unavailable(Situation::NotForwarded {
            lake: self.lake.to_string(),
            doors: n as u32,
            last,
        }))
    }
}

/// Whether a door answered that it holds no lease, so did nothing: the
/// call passes to the next. Whether a door was sent nothing is the
/// wire's to say ([`Sent`]), never an answer's.
fn holds_no_lease(e: &Error) -> bool {
    matches!(
        e.situation(),
        Some(
            Situation::ReadOnly { .. }
                | Situation::LeaseStale { .. }
                | Situation::LeaseNotHeld { .. }
        )
    )
}

/// A lake's handle that reads its own engine and forwards every write to
/// the writer.
pub struct ForwardingLake {
    local: Arc<dyn LakeStore>,
    doors: Arc<Doors>,
    wait: Duration,
}

impl fmt::Debug for ForwardingLake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ForwardingLake")
            .field("lake", &self.doors.lake)
            .field("doors", &self.doors.wires.len())
            .finish_non_exhaustive()
    }
}

impl ForwardingLake {
    /// Wait, as a read does, until the handle's engine has folded what a
    /// writer has answered it: what the local handle answers is then of
    /// a lake that holds the handle's own writes.
    /// What a host operation that needs the lease answers: the handle
    /// never holds it, and another process does.
    fn read_only(&self) -> Error {
        Error::unavailable(Situation::ReadOnly {
            lake: Some(self.doors.lake.to_string()),
            leased_elsewhere: true,
        })
    }

    async fn caught_up(&self) -> Result<()> {
        (self.local.reaches(self.doors.floor(), self.wait).await).map(|_| ())
    }

    /// A forwarding handle over `local`, a handle without the lease
    /// opened from `location`, to the writer doors `location` names.
    /// Nothing is sent yet; the doors, the key and the bounds are checked
    /// here.
    pub fn new(
        local: Arc<dyn LakeStore>,
        location: &Location,
        config: ForwardConfig,
    ) -> std::result::Result<ForwardingLake, ConfigError> {
        let refuse = |why: &str| Err(ConfigError(why.into()));
        if matches!(location.engine, EngineConfig::Memory) {
            return refuse("a lake in memory is one process's; nothing can forward to its writer");
        }
        if local.lake() != &location.lake {
            return refuse("the local handle is of another lake than the location");
        }
        if local.is_writer() {
            return refuse("the local handle holds the writer lease; it writes itself");
        }
        if location.writers.is_empty() {
            return refuse("the location names no writer doors to forward to");
        }
        if config.connect_timeout_millis == 0
            || config.connect_timeout_millis >= config.timeout_secs.max(1) * 1000
        {
            return refuse(
                "`connect_timeout_millis` must be more than 0 and less than the timeout",
            );
        }
        let connect = Duration::from_millis(config.connect_timeout_millis);
        let wires = (location.writers.iter())
            .map(|door| {
                let remote = RemoteConfig {
                    url: door.url.clone(),
                    lake: location.lake.clone(),
                    keys: Default::default(),
                    feed_key: None,
                    ca_file: door.ca_file.clone(),
                    timeout_secs: config.timeout_secs,
                    max_answer_bytes: config.max_answer_bytes,
                    limits: location.limits,
                };
                let host = Some((config.credential.clone(), connect));
                Wire::build(&remote, SILENCE, host).map(Arc::new)
            })
            .collect::<std::result::Result<_, _>>()?;
        Ok(ForwardingLake {
            local,
            doors: Arc::new(Doors {
                lake: location.lake.clone(),
                wires,
                last: AtomicUsize::new(0),
            }),
            wait: Duration::from_millis(config.read_wait_millis),
        })
    }
}

/// The doors of a forwarding handle.
struct ForwardingCatalog {
    local: Arc<dyn Catalog>,
    store: Arc<dyn LakeStore>,
    doors: Arc<Doors>,
    remotes: Vec<Arc<RemoteCatalog>>,
    wait: Duration,
}

impl ForwardingCatalog {
    /// The local doors, once the handle's engine has folded its writes.
    async fn read(&self) -> Result<&dyn Catalog> {
        self.store.reaches(self.doors.floor(), self.wait).await?;
        Ok(self.local.as_ref())
    }

    /// The remote doors, from the one that last answered.
    async fn write<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: Fn(Arc<RemoteCatalog>) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        self.doors.each(|at| f(self.remotes[at].clone())).await
    }
}

/// A write at each door in turn: `$c` is the door, and each named value
/// is cloned for it.
macro_rules! forward {
    ($self:ident, $c:ident => $call:expr; $($owned:ident),*) => {
        $self
            .write(|$c| {
                $(let $owned = $owned.clone();)*
                async move { $call.await }
            })
            .await
    };
}

#[async_trait]
impl Catalog for ForwardingCatalog {
    fn lake(&self) -> &LakeId {
        &self.doors.lake
    }

    async fn describe(&self, who: &Principal, at: ReadAt) -> Result<Reply<LakeDescription>> {
        self.read().await?.describe(who, at).await
    }

    async fn describe_type(
        &self,
        who: &Principal,
        ty: &TypeName,
        at: ReadAt,
    ) -> Result<Reply<TypeDescription>> {
        self.read().await?.describe_type(who, ty, at).await
    }

    async fn find(&self, who: &Principal, ty: &TypeName, find: Find) -> Result<Reply<Found>> {
        self.read().await?.find(who, ty, find).await
    }

    async fn get(&self, who: &Principal, ty: &TypeName, get: Get) -> Result<Reply<Record>> {
        self.read().await?.get(who, ty, get).await
    }

    async fn upsert(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        forward!(self, c => c.upsert(who, ty, key, patch, opts); patch, opts)
    }

    async fn retire(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        forward!(self, c => c.retire(who, ty, key, opts); opts)
    }

    async fn link(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        relation: &RelationName,
        to: &RecordKey,
        fields: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        forward!(self, c => c.link(who, ty, key, relation, to, fields, opts); fields, opts)
    }

    async fn unlink(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        relation: &RelationName,
        to: &RecordKey,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        forward!(self, c => c.unlink(who, ty, key, relation, to, opts); opts)
    }

    async fn call(
        &self,
        who: &Principal,
        verb: &VerbName,
        key: Option<&RecordKey>,
        payload: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        forward!(self, c => c.call(who, verb, key, payload, opts); payload, opts)
    }

    async fn query(&self, who: &Principal, pattern: Pattern) -> Result<Reply<QueryResult>> {
        self.read().await?.query(who, pattern).await
    }

    async fn search(&self, who: &Principal, search: Search) -> Result<Reply<Hits>> {
        self.read().await?.search(who, search).await
    }

    async fn propose(&self, who: &Principal, write: ProposedWrite, why: String) -> Result<Outcome> {
        forward!(self, c => c.propose(who, write, why); write, why)
    }

    async fn accept(&self, who: &Principal, proposal: &ProposalId) -> Result<Outcome> {
        forward!(self, c => c.accept(who, proposal);)
    }

    async fn accept_planned(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        plan_id: &str,
    ) -> Result<Outcome> {
        forward!(self, c => c.accept_planned(who, proposal, plan_id);)
    }

    async fn reject(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        reason: String,
    ) -> Result<Outcome> {
        forward!(self, c => c.reject(who, proposal, reason); reason)
    }

    async fn proposals(
        &self,
        who: &Principal,
        filter: ProposalFilter,
    ) -> Result<Reply<Vec<Proposal>>> {
        self.read().await?.proposals(who, filter).await
    }

    async fn declare(&self, who: &Principal, declaration: Declaration) -> Result<Outcome> {
        forward!(self, c => c.declare(who, declaration); declaration)
    }

    async fn apply(&self, who: &Principal, declarations: Vec<Declaration>) -> Result<Outcome> {
        forward!(self, c => c.apply(who, declarations); declarations)
    }

    async fn recompute(&self, who: &Principal, what: Recompute) -> Result<Outcome> {
        forward!(self, c => c.recompute(who, what); what)
    }

    async fn add_field(&self, who: &Principal, ty: &TypeName, field: FieldDecl) -> Result<Outcome> {
        forward!(self, c => c.add_field(who, ty, field); field)
    }

    async fn migrate(
        &self,
        who: &Principal,
        migration: Migration,
    ) -> Result<Reply<MigrationReport>> {
        forward!(self, c => c.migrate(who, migration); migration)
    }

    async fn migrate_apply(
        &self,
        who: &Principal,
        migration: Migration,
        report_id: &str,
    ) -> Result<Outcome> {
        forward!(self, c => c.migrate_apply(who, migration, report_id); migration)
    }

    async fn plan(&self, who: &Principal, change: Change) -> Result<Reply<Plan>> {
        forward!(self, c => c.plan(who, change); change)
    }

    async fn enact(&self, who: &Principal, change: Change, plan_id: &str) -> Result<Outcome> {
        forward!(self, c => c.enact(who, change, plan_id); change)
    }

    async fn migrate_value_type(
        &self,
        who: &Principal,
        m: ValueTypeMigration,
    ) -> Result<Reply<ValueTypeReport>> {
        forward!(self, c => c.migrate_value_type(who, m); m)
    }

    async fn migrate_value_type_apply(
        &self,
        who: &Principal,
        m: ValueTypeMigration,
        report_id: &str,
    ) -> Result<Outcome> {
        forward!(self, c => c.migrate_value_type_apply(who, m, report_id); m)
    }

    async fn ingest(
        &self,
        who: &Principal,
        source: &SourceId,
        records: SourceStream,
    ) -> Result<CommitStream> {
        self.ingest_with(who, source, IngestOpts::default(), records)
            .await
    }

    /// The source is read only by the door that takes the call, once its
    /// answer's head says so: a door that holds no lease passes it on
    /// unread.
    async fn ingest_with(
        &self,
        who: &Principal,
        source: &SourceId,
        opts: IngestOpts,
        records: SourceStream,
    ) -> Result<CommitStream> {
        let slot = Arc::new(Mutex::new(Some(records)));
        forward!(self, c => c.ingest_held(who, source, opts, slot); opts, slot)
    }
}

#[async_trait]
impl LakeStore for ForwardingLake {
    fn lake(&self) -> &LakeId {
        &self.doors.lake
    }

    /// The writer is another process.
    fn is_writer(&self) -> bool {
        false
    }

    fn limits(&self) -> Limits {
        self.local.limits()
    }

    /// The local head, once it has folded this handle's writes.
    async fn head(&self) -> Result<Position> {
        self.local.reaches(self.doors.floor(), self.wait).await
    }

    async fn reaches(&self, at: Position, within: Duration) -> Result<Position> {
        self.local.reaches(at.max(self.doors.floor()), within).await
    }

    /// Reads by the local handle, writes by the writer, each carrying
    /// `context`.
    fn catalog(&self, context: WriteContext) -> Arc<dyn Catalog> {
        let remotes = (self.doors.wires.iter())
            .map(|w| Arc::new(RemoteCatalog::forwarding(w.clone(), &context)))
            .collect();
        Arc::new(ForwardingCatalog {
            local: self.local.catalog(context),
            store: self.local.clone(),
            doors: self.doors.clone(),
            remotes,
            wait: self.wait,
        })
    }

    fn feed(&self) -> Arc<dyn ChangeFeed> {
        self.local.feed()
    }

    async fn verify(
        &self,
        from: Option<Position>,
        through: Option<Position>,
    ) -> Result<Verification> {
        self.caught_up().await?;
        self.local.verify(from, through).await
    }

    async fn position_at(&self, known_at: Timestamp) -> Result<Option<Position>> {
        self.caught_up().await?;
        self.local.position_at(known_at).await
    }

    async fn vector_coverage(&self, ty: &TypeName) -> Result<Vec<VectorCoverage>> {
        self.caught_up().await?;
        self.local.vector_coverage(ty).await
    }

    async fn rebuild(&self) -> Result<Position> {
        Err(self.read_only())
    }

    fn reclaim_owed(&self) -> Option<String> {
        self.local.reclaim_owed()
    }

    fn rebuilt_at_open(&self) -> Option<String> {
        self.local.rebuilt_at_open()
    }

    async fn restrict(&self, _: &TypeName, _: &RecordKey, _: String) -> Result<Position> {
        Err(self.read_only())
    }

    async fn unrestrict(&self, _: &TypeName, _: &RecordKey) -> Result<Position> {
        Err(self.read_only())
    }

    async fn checkpoint(&self) -> Result<Position> {
        Err(self.read_only())
    }

    async fn age(&self, _: Timestamp) -> Result<AgeReport> {
        Err(self.read_only())
    }

    async fn purge_source(
        &self,
        _: &SourceId,
        _: Option<&TypeName>,
        _: Option<&str>,
    ) -> Result<PurgeReport> {
        Err(self.read_only())
    }

    async fn purge_preflight(
        &self,
        source: &SourceId,
        ty: Option<&TypeName>,
    ) -> Result<PurgeReport> {
        self.caught_up().await?;
        self.local.purge_preflight(source, ty).await
    }

    async fn upsert_preflight(
        &self,
        who: &Principal,
        context: WriteContext,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        expected: Option<Version>,
    ) -> Result<()> {
        self.caught_up().await?;
        (self.local)
            .upsert_preflight(who, context, ty, key, patch, expected)
            .await
    }

    async fn forget_source(&self, _: &SourceId, _: Option<&str>) -> Result<ForgetReport> {
        Err(self.read_only())
    }

    async fn forget_preflight(&self, source: &SourceId) -> Result<ForgetReport> {
        self.caught_up().await?;
        self.local.forget_preflight(source).await
    }

    async fn forget_sources(&self, _: &[SourceId], _: Option<&str>) -> Result<ForgetSetReport> {
        Err(self.read_only())
    }

    async fn forget_sources_preflight(&self, sources: &[SourceId]) -> Result<ForgetSetReport> {
        self.caught_up().await?;
        self.local.forget_sources_preflight(sources).await
    }

    async fn remove_values(
        &self,
        _: &SourceId,
        _: &Selection,
        _: bool,
        _: Option<&str>,
    ) -> Result<RemoveReport> {
        Err(self.read_only())
    }

    async fn remove_values_preflight(
        &self,
        source: &SourceId,
        selection: &Selection,
    ) -> Result<RemoveReport> {
        self.caught_up().await?;
        self.local.remove_values_preflight(source, selection).await
    }

    async fn compact(&self) -> Result<CompactReport> {
        Err(self.read_only())
    }

    fn bind_embed(&self, backend: Arc<dyn EmbedBackend>) -> Result<()> {
        self.local.bind_embed(backend)
    }

    fn bind_rerank(&self, backend: Arc<dyn Reranker>) -> Result<()> {
        self.local.bind_rerank(backend)
    }

    async fn close(&self) -> Result<()> {
        self.local.close().await
    }
}

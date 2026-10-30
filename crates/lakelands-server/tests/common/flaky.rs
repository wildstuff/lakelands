//! A lake that answers `Unavailable` a set number of times on `head`
//! and on `get`, can take its time before an ingest's first window,
//! counts the reads made through it, and otherwise is the lake it wraps.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use lakelands_api::catalog::{CommitStream, SourceStream, Verification};
use lakelands_api::describe::{LakeDescription, TypeDescription};
use lakelands_api::error::{Error, Result};
use lakelands_api::id::{
    LakeId, Position, ProposalId, RecordKey, RelationName, SourceId, TypeName, VerbName,
};
use lakelands_api::ontology::FieldDecl;
use lakelands_api::principal::{Principal, WriteContext};
use lakelands_api::quarantine::{QuarantineClose, QuarantineFilter, QuarantineId, Quarantined};
use lakelands_api::query::{Hits, Pattern, QueryResult, Search};
use lakelands_api::read::{Find, Found, Get, ReadAt, Reply};
use lakelands_api::record::{Patch, Record};
use lakelands_api::seam::{ChangeFeed, FeedBatch, FeedFilter};
use lakelands_api::write::{
    AgeReport, Declaration, Migration, MigrationReport, Outcome, Proposal, ProposalFilter,
    ProposedWrite, WriteOpts,
};
use lakelands_api::{Catalog, LakeStore};

/// How many more times `head`, `get` and a feed's `poll` fail, how many
/// more times `get` answers that its reads have not reached the position
/// yet, how many milliseconds an ingest's first window takes to commit,
/// and how many reads were made.
#[derive(Default)]
pub struct Faults {
    pub head: AtomicUsize,
    pub poll: AtomicUsize,
    /// Non zero: a poll tells all it reads as one append.
    pub one_append: AtomicUsize,
    /// How many milliseconds a `get` takes.
    pub get_millis: AtomicU64,
    pub get: AtomicUsize,
    pub behind: AtomicUsize,
    pub first_window_millis: AtomicU64,
    /// How many `get`s and `describe_type`s were asked of it.
    pub gets: AtomicUsize,
    pub describes: AtomicUsize,
}

/// An ingest's windows, the first one late: the body is not read while
/// it waits, as when a window takes long to commit.
struct Late {
    wait: Option<Pin<Box<tokio::time::Sleep>>>,
    inner: CommitStream,
}

impl futures_core::Stream for Late {
    type Item = <CommitStream as futures_core::Stream>::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(wait) = self.wait.as_mut() {
            if wait.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            self.wait = None;
        }
        self.inner.as_mut().poll_next(cx)
    }
}

/// Take one fault from `n` unless none is left: whether one was taken.
/// A compare-and-swap loop, since `fetch_update` is deprecated from 1.99
/// and its successor `try_update` is newer than the MSRV.
fn take(n: &AtomicUsize) -> bool {
    let mut left = n.load(Ordering::SeqCst);
    while let Some(after) = left.checked_sub(1) {
        match n.compare_exchange_weak(left, after, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(now) => left = now,
        }
    }
    false
}

fn behind(n: &AtomicUsize, at: Option<Position>) -> Result<()> {
    if !take(n) {
        return Ok(());
    }
    Err(Error::Refused {
        field: None,
        reason: lakelands_api::error::Refusal::OutOfRange {
            head: Position(at.map_or(0, |p| p.0.saturating_sub(1))),
        },
    })
}

fn fail(n: &AtomicUsize) -> Result<()> {
    if !take(n) {
        return Ok(());
    }
    let lake = "flaky".to_owned();
    Err(Error::unavailable(lakelands_api::error::Situation::Busy {
        lake,
    }))
}

pub struct Store {
    pub inner: Arc<dyn LakeStore>,
    pub faults: Arc<Faults>,
}

/// The lake's feed, its `poll` failing while `Faults::poll` says so.
struct Feed {
    inner: Arc<dyn ChangeFeed>,
    faults: Arc<Faults>,
}

#[async_trait]
impl ChangeFeed for Feed {
    async fn poll(&self, from: Position, filter: &FeedFilter, limit: u32) -> Result<FeedBatch> {
        fail(&self.faults.poll)?;
        let mut batch = self.inner.poll(from, filter, limit).await?;
        if self.faults.one_append.load(Ordering::SeqCst) != 0 {
            batch.appends = batch
                .events
                .last()
                .map(|e| e.position)
                .into_iter()
                .collect();
        }
        Ok(batch)
    }
    async fn ack(&self, consumer: &str, through: Position) -> Result<()> {
        self.inner.ack(consumer, through).await
    }
    async fn wait(&self, after: Position, within: Duration) -> Result<Position> {
        self.inner.wait(after, within).await
    }
}

struct Doors {
    inner: Arc<dyn Catalog>,
    faults: Arc<Faults>,
}

#[async_trait]
impl LakeStore for Store {
    fn lake(&self) -> &LakeId {
        self.inner.lake()
    }
    fn is_writer(&self) -> bool {
        self.inner.is_writer()
    }
    fn limits(&self) -> lakelands_api::location::Limits {
        self.inner.limits()
    }
    async fn head(&self) -> Result<Position> {
        fail(&self.faults.head)?;
        self.inner.head().await
    }
    fn catalog(&self, context: WriteContext) -> Arc<dyn Catalog> {
        Arc::new(Doors {
            inner: self.inner.catalog(context),
            faults: self.faults.clone(),
        })
    }
    fn feed(&self) -> Arc<dyn ChangeFeed> {
        Arc::new(Feed {
            inner: self.inner.feed(),
            faults: self.faults.clone(),
        })
    }
    async fn verify(
        &self,
        from: Option<Position>,
        through: Option<Position>,
    ) -> Result<Verification> {
        self.inner.verify(from, through).await
    }
    async fn rebuild(&self) -> Result<Position> {
        self.inner.rebuild().await
    }
    async fn restrict(&self, ty: &TypeName, key: &RecordKey, reason: String) -> Result<Position> {
        self.inner.restrict(ty, key, reason).await
    }
    async fn unrestrict(&self, ty: &TypeName, key: &RecordKey) -> Result<Position> {
        self.inner.unrestrict(ty, key).await
    }
    async fn checkpoint(&self) -> Result<Position> {
        self.inner.checkpoint().await
    }
    async fn age(&self, today: lakelands_api::time::Timestamp) -> Result<AgeReport> {
        self.inner.age(today).await
    }
    async fn purge_source(
        &self,
        source: &SourceId,
        ty: Option<&lakelands_api::TypeName>,
        id: Option<&str>,
    ) -> Result<lakelands_api::write::PurgeReport> {
        self.inner.purge_source(source, ty, id).await
    }
    async fn purge_preflight(
        &self,
        source: &SourceId,
        ty: Option<&lakelands_api::TypeName>,
    ) -> Result<lakelands_api::write::PurgeReport> {
        self.inner.purge_preflight(source, ty).await
    }
    async fn upsert_preflight(
        &self,
        who: &lakelands_api::Principal,
        context: lakelands_api::WriteContext,
        ty: &lakelands_api::TypeName,
        key: &lakelands_api::RecordKey,
        patch: lakelands_api::record::Patch,
        expected: Option<lakelands_api::Version>,
    ) -> lakelands_api::Result<()> {
        self.inner
            .upsert_preflight(who, context, ty, key, patch, expected)
            .await
    }
    async fn forget_source(
        &self,
        source: &lakelands_api::SourceId,
        id: Option<&str>,
    ) -> lakelands_api::Result<lakelands_api::write::ForgetReport> {
        self.inner.forget_source(source, id).await
    }
    async fn forget_preflight(
        &self,
        source: &lakelands_api::SourceId,
    ) -> lakelands_api::Result<lakelands_api::write::ForgetReport> {
        self.inner.forget_preflight(source).await
    }
    async fn forget_sources(
        &self,
        sources: &[lakelands_api::SourceId],
        id: Option<&str>,
    ) -> lakelands_api::Result<lakelands_api::write::ForgetSetReport> {
        self.inner.forget_sources(sources, id).await
    }
    async fn forget_sources_preflight(
        &self,
        sources: &[lakelands_api::SourceId],
    ) -> lakelands_api::Result<lakelands_api::write::ForgetSetReport> {
        self.inner.forget_sources_preflight(sources).await
    }
    async fn remove_values(
        &self,
        source: &lakelands_api::SourceId,
        selection: &lakelands_api::write::Selection,
        shredded: bool,
        id: Option<&str>,
    ) -> lakelands_api::Result<lakelands_api::write::RemoveReport> {
        self.inner
            .remove_values(source, selection, shredded, id)
            .await
    }
    async fn remove_values_preflight(
        &self,
        source: &lakelands_api::SourceId,
        selection: &lakelands_api::write::Selection,
    ) -> lakelands_api::Result<lakelands_api::write::RemoveReport> {
        self.inner.remove_values_preflight(source, selection).await
    }
    async fn compact(&self) -> Result<lakelands_api::write::CompactReport> {
        self.inner.compact().await
    }
    fn bind_embed(&self, backend: Arc<dyn lakelands_api::seam::EmbedBackend>) -> Result<()> {
        self.inner.bind_embed(backend)
    }
    async fn vector_coverage(
        &self,
        ty: &lakelands_api::TypeName,
    ) -> lakelands_api::Result<Vec<lakelands_api::read::VectorCoverage>> {
        self.inner.vector_coverage(ty).await
    }
    async fn position_at(
        &self,
        known_at: lakelands_api::Timestamp,
    ) -> lakelands_api::Result<Option<lakelands_api::Position>> {
        self.inner.position_at(known_at).await
    }
    async fn close(&self) -> Result<()> {
        self.inner.close().await
    }
}

#[async_trait]
impl Catalog for Doors {
    fn lake(&self) -> &LakeId {
        self.inner.lake()
    }
    async fn describe(&self, who: &Principal, at: ReadAt) -> Result<Reply<LakeDescription>> {
        self.inner.describe(who, at).await
    }
    async fn describe_type(
        &self,
        who: &Principal,
        ty: &TypeName,
        at: ReadAt,
    ) -> Result<Reply<TypeDescription>> {
        self.faults.describes.fetch_add(1, Ordering::SeqCst);
        self.inner.describe_type(who, ty, at).await
    }
    async fn find(&self, who: &Principal, ty: &TypeName, find: Find) -> Result<Reply<Found>> {
        self.inner.find(who, ty, find).await
    }
    async fn get(&self, who: &Principal, ty: &TypeName, get: Get) -> Result<Reply<Record>> {
        self.faults.gets.fetch_add(1, Ordering::SeqCst);
        let wait = self.faults.get_millis.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(wait)).await;
        fail(&self.faults.get)?;
        behind(&self.faults.behind, get.at.as_of)?;
        self.inner.get(who, ty, get).await
    }
    async fn upsert(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        self.inner.upsert(who, ty, key, patch, opts).await
    }
    async fn retire(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        self.inner.retire(who, ty, key, opts).await
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
        self.inner
            .link(who, ty, key, relation, to, fields, opts)
            .await
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
        self.inner.unlink(who, ty, key, relation, to, opts).await
    }
    async fn call(
        &self,
        who: &Principal,
        verb: &VerbName,
        key: Option<&RecordKey>,
        payload: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        self.inner.call(who, verb, key, payload, opts).await
    }
    async fn query(&self, who: &Principal, pattern: Pattern) -> Result<Reply<QueryResult>> {
        self.inner.query(who, pattern).await
    }
    async fn search(&self, who: &Principal, search: Search) -> Result<Reply<Hits>> {
        self.inner.search(who, search).await
    }
    async fn propose(&self, who: &Principal, write: ProposedWrite, why: String) -> Result<Outcome> {
        self.inner.propose(who, write, why).await
    }
    async fn accept(&self, who: &Principal, proposal: &ProposalId) -> Result<Outcome> {
        self.inner.accept(who, proposal).await
    }
    async fn reject(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        reason: String,
    ) -> Result<Outcome> {
        self.inner.reject(who, proposal, reason).await
    }
    async fn proposals(
        &self,
        who: &Principal,
        filter: ProposalFilter,
    ) -> Result<Reply<Vec<Proposal>>> {
        self.inner.proposals(who, filter).await
    }
    async fn quarantined(
        &self,
        who: &Principal,
        filter: QuarantineFilter,
    ) -> Result<Reply<Vec<Quarantined>>> {
        self.inner.quarantined(who, filter).await
    }
    async fn dismiss_quarantined(
        &self,
        who: &Principal,
        id: &QuarantineId,
    ) -> Result<QuarantineClose> {
        self.inner.dismiss_quarantined(who, id).await
    }
    async fn declare(&self, who: &Principal, declaration: Declaration) -> Result<Outcome> {
        self.inner.declare(who, declaration).await
    }
    async fn apply(&self, who: &Principal, declarations: Vec<Declaration>) -> Result<Outcome> {
        self.inner.apply(who, declarations).await
    }
    async fn add_field(&self, who: &Principal, ty: &TypeName, field: FieldDecl) -> Result<Outcome> {
        self.inner.add_field(who, ty, field).await
    }
    async fn recompute(
        &self,
        who: &Principal,
        what: lakelands_api::write::Recompute,
    ) -> Result<Outcome> {
        self.inner.recompute(who, what).await
    }
    async fn migrate(
        &self,
        who: &Principal,
        migration: Migration,
    ) -> Result<Reply<MigrationReport>> {
        self.inner.migrate(who, migration).await
    }
    async fn migrate_apply(
        &self,
        who: &Principal,
        migration: Migration,
        report_id: &str,
    ) -> Result<Outcome> {
        self.inner.migrate_apply(who, migration, report_id).await
    }
    async fn accept_planned(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        plan_id: &str,
    ) -> Result<Outcome> {
        self.inner.accept_planned(who, proposal, plan_id).await
    }
    async fn plan(
        &self,
        who: &Principal,
        change: lakelands_api::write::Change,
    ) -> Result<Reply<lakelands_api::write::Plan>> {
        self.inner.plan(who, change).await
    }
    async fn enact(
        &self,
        who: &Principal,
        change: lakelands_api::write::Change,
        plan_id: &str,
    ) -> Result<Outcome> {
        self.inner.enact(who, change, plan_id).await
    }
    async fn migrate_value_type(
        &self,
        who: &Principal,
        to: lakelands_api::write::ValueTypeMigration,
    ) -> Result<Reply<lakelands_api::write::ValueTypeReport>> {
        self.inner.migrate_value_type(who, to).await
    }
    async fn migrate_value_type_apply(
        &self,
        who: &Principal,
        to: lakelands_api::write::ValueTypeMigration,
        report_id: &str,
    ) -> Result<Outcome> {
        self.inner
            .migrate_value_type_apply(who, to, report_id)
            .await
    }
    async fn ingest_with(
        &self,
        who: &Principal,
        source: &SourceId,
        opts: lakelands_api::IngestOpts,
        records: SourceStream,
    ) -> Result<CommitStream> {
        if opts == lakelands_api::IngestOpts::default() {
            return self.ingest(who, source, records).await;
        }
        self.inner.ingest_with(who, source, opts, records).await
    }
    async fn ingest(
        &self,
        who: &Principal,
        source: &SourceId,
        records: SourceStream,
    ) -> Result<CommitStream> {
        let inner = self.inner.ingest(who, source, records).await?;
        let millis = self.faults.first_window_millis.load(Ordering::SeqCst);
        if millis == 0 {
            return Ok(inner);
        }
        Ok(Box::pin(Late {
            wait: Some(Box::pin(tokio::time::sleep(Duration::from_millis(millis)))),
            inner,
        }))
    }
}

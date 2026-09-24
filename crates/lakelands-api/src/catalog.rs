//! The doors: [`Catalog`], what every caller uses, and [`LakeStore`], the
//! store facade a host holds for one open lake.
//!
//! Both are `dyn`-compatible: the CLI and the MCP server hold
//! `Arc<dyn Catalog>`, a host holds `Arc<dyn LakeStore>`, and the mock,
//! the store and a remote client are interchangeable behind them.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;

use crate::describe::{LakeDescription, TypeDescription};
use crate::error::Result;
use crate::event::Digest;
use crate::id::{LakeId, Position, ProposalId, RecordKey, SourceId, TypeName, VerbName};
use crate::ontology::FieldDecl;
use crate::principal::{Principal, WriteContext};
use crate::query::{Hits, Pattern, QueryResult, Search};
use crate::read::{Find, Found, Get, ReadAt, Reply};
use crate::record::{Patch, Record, SourceRecord};
use crate::seam::ChangeFeed;
use crate::write::{
    Committed, Declaration, Migration, MigrationReport, Outcome, Proposal, ProposalFilter,
    ProposedWrite, WriteOpts,
};

/// A stream of records from one source, as `ingest` takes it.
pub type SourceStream = Pin<Box<dyn Stream<Item = SourceRecord> + Send>>;

/// The answer to `ingest`: one [`Committed`] per group-commit window, in
/// order, as each becomes durable.
pub type CommitStream = Pin<Box<dyn Stream<Item = Result<Committed>> + Send>>;

/// The doors of one lake, for any caller.
///
/// Every call names the [`Principal`] it is made as; its grants decide
/// what it sees and what it may write, on every call. Every read answers
/// a [`Reply`] carrying its position; every write answers an [`Outcome`]
/// carrying its position.
///
/// The per-type doors are `find`, `get`, `upsert` and `retire`; each
/// declared verb is reached through `call`. `describe`, `query` and
/// `search` are generic. Over MCP, each door becomes tools named by
/// [`crate::tools::tools`] and nothing else.
#[async_trait]
pub trait Catalog: Send + Sync {
    /// The lake these doors open onto.
    fn lake(&self) -> &LakeId;

    /// What the lake holds, as the principal may see it.
    async fn describe(&self, who: &Principal, at: ReadAt) -> Result<Reply<LakeDescription>>;

    /// One type, on its six axes.
    async fn describe_type(
        &self,
        who: &Principal,
        ty: &TypeName,
        at: ReadAt,
    ) -> Result<Reply<TypeDescription>>;

    /// Records of one type: filtered, ordered and paged, or aggregated.
    async fn find(&self, who: &Principal, ty: &TypeName, find: Find) -> Result<Reply<Found>>;

    /// One record by key.
    async fn get(&self, who: &Principal, ty: &TypeName, get: Get) -> Result<Reply<Record>>;

    /// Write authored fields of one record as a sparse patch: absent is
    /// unchanged, `null` clears. A field the principal may not write
    /// refuses the whole write.
    async fn upsert(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome>;

    /// Retire one record.
    async fn retire(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        opts: WriteOpts,
    ) -> Result<Outcome>;

    /// Call a declared verb on one record. It runs or is proposed by its
    /// declared risk.
    async fn call(
        &self,
        who: &Principal,
        verb: &VerbName,
        key: &RecordKey,
        payload: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome>;

    /// The one pattern query.
    async fn query(&self, who: &Principal, pattern: Pattern) -> Result<Reply<QueryResult>>;

    /// Ranked retrieval over one type.
    async fn search(&self, who: &Principal, search: Search) -> Result<Reply<Hits>>;

    /// Record a write as a proposal instead of applying it.
    async fn propose(&self, who: &Principal, write: ProposedWrite, why: String) -> Result<Outcome>;

    /// Accept a proposal: its write is applied, under the proposer's
    /// grants, caused by the acceptance.
    async fn accept(&self, who: &Principal, proposal: &ProposalId) -> Result<Outcome>;

    /// Reject a proposal. Nothing is written but the rejection, and the
    /// answer is `Applied` at the rejection's position, with no version.
    async fn reject(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        reason: String,
    ) -> Result<Outcome>;

    /// Proposals, newest first.
    async fn proposals(
        &self,
        who: &Principal,
        filter: ProposalFilter,
    ) -> Result<Reply<Vec<Proposal>>>;

    /// Add or revise a declaration that does not reshape stored records.
    async fn declare(&self, who: &Principal, declaration: Declaration) -> Result<Outcome>;

    /// Add one field to a type.
    async fn add_field(&self, who: &Principal, ty: &TypeName, field: FieldDecl) -> Result<Outcome>;

    /// The preflight of a migration: what it would do, changing nothing —
    /// every field whose values convert and how many would not, and in
    /// `dropped` every field the migration removes with how many values
    /// that discards.
    async fn migrate(
        &self,
        who: &Principal,
        migration: Migration,
    ) -> Result<Reply<MigrationReport>>;

    /// Apply a migration whose preflight the caller was shown. Refused if
    /// the lake moved since (the report differs) or if it is not
    /// lossless.
    async fn migrate_apply(
        &self,
        who: &Principal,
        migration: Migration,
        report_id: &str,
    ) -> Result<Outcome>;

    /// Feed the lake from a source. Records are chunked into group-commit
    /// windows; each window answers once it is durable. Content-addressed:
    /// a record delivered twice appends nothing the second time, so
    /// at-least-once delivery is safe. The source's own offset is the
    /// caller's; the answer carries the lake's positions.
    async fn ingest(
        &self,
        who: &Principal,
        source: &SourceId,
        records: SourceStream,
    ) -> Result<CommitStream>;
}

/// What verifying a lake's log found.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Verification {
    /// The first position checked.
    pub from: Position,
    /// The last position checked.
    pub through: Position,
    /// The hash of the event at `through`.
    pub hash: Digest,
}

/// The store facade: one open lake, as its host holds it.
///
/// The host opens a lake at a [`Location`](crate::location::Location)
/// (the store crate's `open`), holds this, hands `catalog()` to its
/// doors, and reads the change feed.
#[async_trait]
pub trait LakeStore: Send + Sync {
    /// The lake.
    fn lake(&self) -> &LakeId;

    /// Whether this process holds the lake's writer lease.
    fn is_writer(&self) -> bool;

    /// The position of the last event in the log.
    async fn head(&self) -> Result<Position>;

    /// The doors, for callers of this host. Writes through them carry
    /// `context` in the log; a door never lets a caller set it
    /// (`WriteContext` is the host's, never a caller's).
    fn catalog(&self, context: WriteContext) -> Arc<dyn Catalog>;

    /// The change feed: the log, read by kind from a position.
    fn feed(&self) -> Arc<dyn ChangeFeed>;

    /// Check the hash chain between two positions (all of it when both
    /// are absent), against the lake's checkpoints.
    async fn verify(
        &self,
        from: Option<Position>,
        through: Option<Position>,
    ) -> Result<Verification>;

    /// Delete the projection and rebuild it from the log. Answers the
    /// position it was rebuilt through.
    async fn rebuild(&self) -> Result<Position>;

    /// Seal the current log segment and write a checkpoint.
    async fn checkpoint(&self) -> Result<Position>;

    /// Release the lease and flush. The lake is closed when the last
    /// handle is dropped; this makes it explicit.
    async fn close(&self) -> Result<()>;
}

/// Compile-time proof that the doors are `dyn`-compatible.
#[allow(dead_code)]
fn doors_are_dyn_compatible(catalog: &dyn Catalog, store: &dyn LakeStore) -> Arc<dyn Catalog> {
    let _ = catalog.lake();
    store.catalog(WriteContext::default())
}

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
use crate::id::{
    LakeId, Position, ProposalId, RecordKey, RelationName, SourceId, TypeName, VerbName,
};
use crate::ontology::{FieldDecl, ValueTypeDecl};
use crate::principal::{Principal, WriteContext};
use crate::query::{Hits, Pattern, QueryResult, Search};
use crate::read::{Find, Found, Get, ReadAt, Reply};
use crate::record::{Patch, Record, SourceRecord};
use crate::seam::ChangeFeed;
use crate::write::{
    Committed, Declaration, Migration, MigrationReport, Outcome, Proposal, ProposalFilter,
    ProposedWrite, ValueTypeReport, WriteOpts,
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

    /// Write the link over `relation` from the record `key` of `ty` to the
    /// record `to` of the relation's target, for a relation carried by link
    /// records: the link's own fields as a sparse patch, absent unchanged,
    /// `null` cleared. Writing it again changes its fields; there is one
    /// link per pair. Both records must be live, the relation's
    /// cardinality holds over live links, and a field the principal may
    /// not write refuses the whole write
    /// (`docs/decisions/035-a-link-is-a-record.md`).
    #[allow(clippy::too_many_arguments)]
    async fn link(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        relation: &RelationName,
        to: &RecordKey,
        fields: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome>;

    /// Remove the link over `relation` from the record `key` of `ty` to the
    /// record `to`; its history stays.
    async fn unlink(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        relation: &RelationName,
        to: &RecordKey,
        opts: WriteOpts,
    ) -> Result<Outcome>;

    /// Call a declared verb: on one record, named by `key`, or, for a verb
    /// declared on its type, on the type, with no key. A key passed to a
    /// verb on its type, none passed to a verb on a record, or an expected
    /// version with no key, is refused as invalid, naming the verb. It runs
    /// or is proposed by its declared risk. A verb whose effect reaches outside the lake answers,
    /// when it runs, `Applied` with its executor's recorded `output` —
    /// `{"error": "<text>"}` when the executor failed, since the verb's
    /// position is real either way. A retry answered from its idempotency
    /// key carries no `output`; the recorded one is in the log.
    async fn call(
        &self,
        who: &Principal,
        verb: &VerbName,
        key: Option<&RecordKey>,
        payload: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome>;

    /// The one pattern query.
    async fn query(&self, who: &Principal, pattern: Pattern) -> Result<Reply<QueryResult>>;

    /// Ranked retrieval over one type.
    async fn search(&self, who: &Principal, search: Search) -> Result<Reply<Hits>>;

    /// Record a write as a proposal instead of applying it.
    async fn propose(&self, who: &Principal, write: ProposedWrite, why: String) -> Result<Outcome>;

    /// Accept a proposal: its write is applied, caused by the acceptance
    /// and written as the proposer. It is applied under the proposer's
    /// grants as they were when it was proposed: the write was checked
    /// whole then, and a reviewer applies exactly what was checked. At
    /// acceptance the store checks again only what can have moved since —
    /// the type's shape and the record's version. An accepted external
    /// verb answers its executor's recorded `output`, as `call` does.
    async fn accept(&self, who: &Principal, proposal: &ProposalId) -> Result<Outcome>;

    /// Reject a proposal. Nothing is written but the rejection, and the
    /// answer is `Applied` at the rejection's position, with no version.
    async fn reject(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        reason: String,
    ) -> Result<Outcome>;

    /// Proposals, newest first. A principal with the review grant lists
    /// every proposal; any other lists only its own, so a proposer can
    /// follow what became of what it proposed
    /// (`docs/decisions/013-verbs-run-by-declared-risk.md`).
    async fn proposals(
        &self,
        who: &Principal,
        filter: ProposalFilter,
    ) -> Result<Reply<Vec<Proposal>>>;

    /// Add or revise a declaration that does not reshape stored records. One
    /// that is the declaration in force is answered at the head and appends
    /// nothing (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`).
    async fn declare(&self, who: &Principal, declaration: Declaration) -> Result<Outcome>;

    /// Declare a set of declarations together, as a bundle is: validated as
    /// one — a reference resolves within the set or the lake, tool names
    /// are checked over the whole set, and one refusal names every problem —
    /// and appended in one append, or refused whole with nothing appended.
    /// The outcome names the position of the set's last declaration, from
    /// which all of it is in force. What is in force before the set is not
    /// appended again; a set wholly in force is answered at the head.
    async fn apply(&self, who: &Principal, declarations: Vec<Declaration>) -> Result<Outcome>;

    /// Add one field to a type.
    async fn add_field(&self, who: &Principal, ty: &TypeName, field: FieldDecl) -> Result<Outcome>;

    /// The preflight of a migration: what it would do, changing nothing —
    /// every field whose values convert and how many would not, and in
    /// `dropped` every field the migration removes with how many values
    /// that discards, and every relation carried by link records it
    /// removes with how many live links.
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

    /// The preflight of tightening a value type: every value stored under
    /// it, over every type and field that uses it, history included,
    /// checked against the new declaration. Appends nothing. A loosening is
    /// declared; a change of `kind` or `scale` is not a value-type migration
    /// (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`).
    async fn migrate_value_type(
        &self,
        who: &Principal,
        to: ValueTypeDecl,
    ) -> Result<Reply<ValueTypeReport>>;

    /// Put a tightened value type in force against the preflight the caller
    /// was shown. Refused if the lake moved since, or while any stored value
    /// would not fit; otherwise one `declared` at the next version, and no
    /// record changes.
    async fn migrate_value_type_apply(
        &self,
        who: &Principal,
        to: ValueTypeDecl,
        report_id: &str,
    ) -> Result<Outcome>;

    /// Feed the lake from a source. Records are chunked into group-commit
    /// windows; each window answers once it is durable. Content-addressed:
    /// a record delivered twice appends nothing the second time, so
    /// at-least-once delivery is safe. The source's own offset is the
    /// caller's; the answer carries the lake's positions. A caller holding
    /// the records already calls [`ingest_all`] instead.
    async fn ingest(
        &self,
        who: &Principal,
        source: &SourceId,
        records: SourceStream,
    ) -> Result<CommitStream>;
}

/// `ingest` for records already in hand: the records as the source
/// stream, and every window's answer collected, in order. A free function
/// over any [`Catalog`], not a door of its own. The first error ends it:
/// the windows before it are committed, and a caller that needs to know
/// which reads the stream `ingest` answers itself.
pub async fn ingest_all(
    cat: &dyn Catalog,
    who: &Principal,
    source: &SourceId,
    records: Vec<SourceRecord>,
) -> Result<Vec<Committed>> {
    let mut commits = cat
        .ingest(who, source, Box::pin(InHand(records.into_iter())))
        .await?;
    let mut out = Vec::new();
    while let Some(window) = std::future::poll_fn(|cx| commits.as_mut().poll_next(cx)).await {
        out.push(window?);
    }
    Ok(out)
}

/// Records in hand, as a source stream.
struct InHand(std::vec::IntoIter<SourceRecord>);

impl Stream for InHand {
    type Item = SourceRecord;

    fn poll_next(
        mut self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<SourceRecord>> {
        std::task::Poll::Ready(self.0.next())
    }
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

    /// The limits the lake was opened with: its location's.
    fn limits(&self) -> crate::location::Limits;

    /// The last position the projection has folded, which a read can be
    /// answered at at once: the last event in the log, except while a fold
    /// that failed has not been done again.
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

    /// The position the log had at an instant: the last position, no later
    /// than the head, whose event's `known_at` is at or before `known_at`;
    /// none for an instant before the lake's first event. `known_at`
    /// never decreases along the log (`docs/log-format.md` §1.3), so this
    /// is a binary search over positions.
    async fn position_at(&self, known_at: crate::time::Timestamp) -> Result<Option<Position>>;

    /// For each of a type's embedding fields, at the head: how many live
    /// records hold a vector, and how many hold none although they have
    /// text to embed. The host's, with no door; counting what is due walks
    /// the type's records without a vector, a page at a time.
    async fn vector_coverage(&self, ty: &TypeName) -> Result<Vec<crate::read::VectorCoverage>>;

    /// Delete the projection and rebuild it from the log. Answers the
    /// position it was rebuilt through.
    async fn rebuild(&self) -> Result<Position>;

    /// Withhold a record from principals without the `restricted` grant —
    /// a retention hold. The record and its history stay; reads by
    /// principals without the grant no longer see it. The reason is
    /// recorded in the log.
    async fn restrict(&self, ty: &TypeName, key: &RecordKey, reason: String) -> Result<Position>;

    /// Lift a hold `restrict` placed.
    async fn unrestrict(&self, ty: &TypeName, key: &RecordKey) -> Result<Position>;

    /// Seal the current log segment and write a checkpoint.
    async fn checkpoint(&self) -> Result<Position>;

    /// Run every live aging rule once for `today`, a date at UTC midnight;
    /// any other time is refused as invalid. Moves are records written by
    /// the store with the rule as their procedure, one append per rule,
    /// and a date aged twice is written once
    /// (`docs/decisions/036-aging-runs-by-date-once.md`).
    async fn age(&self, today: crate::time::Timestamp) -> Result<crate::write::AgeReport>;

    /// Retire, in one append, every live record the source brought in,
    /// across every type it reached, restricted ones included. A record
    /// another hand also wrote refuses the whole purge as invalid, naming
    /// it, and nothing is appended; a type that keeps no per-field
    /// provenance is not searched and is named in the report
    /// (`docs/decisions/039-a-source-is-retracted-as-one.md`). With the
    /// `report_id` of a preflight, it is refused as invalid if the lake
    /// moved since.
    async fn purge_source(
        &self,
        source: &SourceId,
        report_id: Option<&str>,
    ) -> Result<crate::write::PurgeReport>;

    /// What [`Self::purge_source`] would do, appending nothing: the same
    /// report, refused as the purge would be, under an id the purge may
    /// repeat.
    async fn purge_preflight(&self, source: &SourceId) -> Result<crate::write::PurgeReport>;

    /// Give back the storage the projection no longer uses. It appends
    /// nothing and changes no answer, no event and no byte of an export;
    /// it prunes no history and does not seal. Only the writer's handle
    /// compacts; writes wait while it runs
    /// (`docs/decisions/040-maintenance-never-changes-an-answer.md`).
    async fn compact(&self) -> Result<crate::write::CompactReport>;

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

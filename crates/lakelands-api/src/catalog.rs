//! The doors: [`Catalog`], what every caller uses, and [`LakeStore`], the
//! store facade a host holds for one open lake.
//!
//! Both are `dyn`-compatible: the CLI and the MCP server hold
//! `Arc<dyn Catalog>`, a host holds `Arc<dyn LakeStore>`, and the store
//! and a remote client are interchangeable behind them.

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
use crate::ontology::FieldDecl;
use crate::principal::{Principal, WriteContext};
use crate::query::{Hits, Pattern, QueryResult, Search};
use crate::read::{Find, Found, Get, ReadAt, Reply};
use crate::record::{Patch, Record, SourceRecord};
use crate::seam::ChangeFeed;
use crate::write::{
    Change, Committed, Declaration, IngestOpts, Migration, MigrationReport, Outcome, Plan,
    Proposal, ProposalFilter, ProposedWrite, Recompute, ValueTypeMigration, ValueTypeReport,
    WriteOpts,
};

/// A stream of records, as `ingest` takes it: each from the source it
/// names, else from the call's.
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
    /// the error's wire form when the executor failed, since the verb's
    /// position is real either way. A retry under its idempotency key runs
    /// nothing: it answers that recorded `output`, or, where none is
    /// recorded, is refused as `unanswered`.
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

    /// Record a write as a proposal instead of applying it. A proposal of
    /// declarations all in force is not recorded: its accept would change
    /// nothing, so it is answered `applied` at the head with how each
    /// stands, as `apply` answers it
    /// (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`).
    async fn propose(&self, who: &Principal, write: ProposedWrite, why: String) -> Result<Outcome>;

    /// Accept a proposal: its write is applied as it was logged, whole,
    /// caused by the acceptance and written in the proposer's name, not
    /// under its grant, which is not in the log. At acceptance the store
    /// checks the write again at the head, in the order of refusals. A
    /// proposal its reviewer is not listed is not found, a call whose verb
    /// reads a field the reviewer may not read among them (013, 006). An accepted external
    /// verb answers its executor's recorded `output`, as `call` does, and
    /// so does the accept sent again; any other accept sent again is
    /// `decided`. An accepted proposal of declarations answers how each
    /// stands after it, as `apply` does.
    async fn accept(&self, who: &Principal, proposal: &ProposalId) -> Result<Outcome>;

    /// `accept` of a proposed change to the ontology, only where the plan
    /// of its change made now, as the reviewer is shown it, has the id
    /// `plan_id`: what the reviewer accepts is what applies. Refused
    /// `plan_stale` otherwise, and `proposal_plans_nothing` for a proposal
    /// that changes no declaration
    /// (`docs/decisions/062-a-change-plans-before-it-acts.md`).
    async fn accept_planned(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        plan_id: &str,
    ) -> Result<Outcome>;

    /// Reject a proposal. Nothing is written but the rejection, and the
    /// answer is `Applied` at the rejection's position, with no version.
    async fn reject(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        reason: String,
    ) -> Result<Outcome>;

    /// Proposals, newest first. A principal with the review grant lists
    /// every proposal there to it, each as its view; any other lists only
    /// its own, so a proposer can follow what became of what it proposed
    /// (`docs/decisions/013-verbs-run-by-declared-risk.md`,
    /// `docs/decisions/006-grants-not-roles.md`).
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

    /// Compute again what a declaration in force computes: it is appended
    /// again as it stands, at its next version, and the fold does what it
    /// does at any `declared`: a function's fields are computed with the
    /// settings and the time of that event, a call whose recorded output is
    /// a failure is made again, a field that waits for its backend is made.
    /// A deliberate act the log shows; an identical `declare` does none of
    /// it (`docs/decisions/022-functions-are-declarations.md`).
    async fn recompute(&self, who: &Principal, what: Recompute) -> Result<Outcome>;

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
    /// lossless. One to the type in force that renames nothing is then
    /// answered at the head and appends nothing only where it fills no
    /// record (050).
    async fn migrate_apply(
        &self,
        who: &Principal,
        migration: Migration,
        report_id: &str,
    ) -> Result<Outcome>;

    /// The preflight of a value type's migration: every value stored under
    /// it, over every type and field that uses it, retired and restricted
    /// records included (not earlier versions), renamed as the migration
    /// renames it and checked against the new declaration. Appends nothing.
    /// A loosening without renames is declared; a change of `kind` or
    /// `scale` is not a value-type migration
    /// (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`).
    async fn migrate_value_type(
        &self,
        who: &Principal,
        migration: ValueTypeMigration,
    ) -> Result<Reply<ValueTypeReport>>;

    /// Put a value type's migration in force against the preflight the
    /// caller was shown. Refused if the lake moved since, or while any
    /// stored value would not fit; otherwise, without renames, one
    /// `declared` at the next version, and no record changes; with them,
    /// one `value_type_migrated`, and every record holding an old value
    /// holds the new one.
    async fn migrate_value_type_apply(
        &self,
        who: &Principal,
        migration: ValueTypeMigration,
        report_id: &str,
    ) -> Result<Outcome>;

    /// The plan of a change to the ontology: what it would do, in counts
    /// and the first keys its reader is answered, changing nothing. It is
    /// the act's own path up to its append, so it refuses what the act
    /// refuses (`docs/decisions/062-a-change-plans-before-it-acts.md`).
    async fn plan(&self, who: &Principal, change: Change) -> Result<Reply<Plan>>;

    /// Make a change whose plan the caller was shown: refused `plan_stale`
    /// where the plan made now, as the caller is shown it, has another id.
    /// Its outcome carries the plan that applied, as every change to the
    /// ontology's does.
    async fn enact(&self, who: &Principal, change: Change, plan_id: &str) -> Result<Outcome>;

    /// Feed the lake from a source. The call's source is the default; a
    /// record may name its own. Records are chunked into group-commit
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

    /// [`Catalog::ingest`] under `opts`: its records in the delivery
    /// `opts` names, which every `record_ingested` of the call and every
    /// [`Committed`] carries, sealed apart where `opts` asks it. A catalog
    /// that does not carry a delivery or the seal to the store refuses
    /// either, rather than drop it.
    async fn ingest_with(
        &self,
        who: &Principal,
        source: &SourceId,
        opts: IngestOpts,
        records: SourceStream,
    ) -> Result<CommitStream> {
        match (opts.delivery, opts.sealed_apart) {
            (Some(_), _) => Err(crate::Error::invalid(
                crate::error::Case::DeliveryNotCarried,
            )),
            (None, true) => Err(crate::Error::invalid(
                crate::error::Case::SealedApartNotCarried,
            )),
            (None, false) => self.ingest(who, source, records).await,
        }
    }
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
    ingest_all_with(
        cat,
        who,
        source,
        crate::write::IngestOpts::default(),
        records,
    )
    .await
}

/// [`ingest_all`] under the call's options: its delivery id.
pub async fn ingest_all_with(
    cat: &dyn Catalog,
    who: &Principal,
    source: &SourceId,
    opts: crate::write::IngestOpts,
    records: Vec<SourceRecord>,
) -> Result<Vec<Committed>> {
    let records = Box::pin(InHand(records.into_iter()));
    let mut commits = cat.ingest_with(who, source, opts, records).await?;
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
    /// are absent), against the lake's checkpoints. It reads the log, so
    /// it finds what an open does not look for: a damaged line of an
    /// engine's index of the log.
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
    /// position it was rebuilt through; a reclaim an act owed that fails
    /// again is not its failure, and stays in [`Self::reclaim_owed`].
    async fn rebuild(&self) -> Result<Position>;

    /// Why a forgetting's or a removal's reclaim is still owed, where the
    /// last one tried failed, the cause alone: the act, an import or a
    /// rebuild answered without it, and the writer's next open, the act's
    /// repeat or a compact that runs clears it
    /// (`docs/decisions/058-a-source-may-be-forgotten.md`).
    fn reclaim_owed(&self) -> Option<String> {
        None
    }

    /// Why the writer's open rebuilt the projection, where it did: an
    /// older build laid it out, and the open folded the whole log
    /// (`docs/decisions/004-the-change-feed-is-the-log.md`).
    fn rebuilt_at_open(&self) -> Option<String> {
        None
    }

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
    /// (`docs/decisions/036-aging-runs-by-date-once.md`). A read the engine
    /// fails ends the run as the outage it is, never a guard that failed;
    /// the rules appended before it stand.
    async fn age(&self, today: crate::time::Timestamp) -> Result<crate::write::AgeReport>;

    /// Retire, in one append, every live record the source brought in,
    /// across every type it reached, restricted ones included. A record
    /// another hand made or also wrote refuses the whole purge as invalid,
    /// naming it, and nothing is appended; a type that keeps no per-field
    /// provenance is not searched and is named in the report
    /// (`docs/decisions/039-a-source-is-retracted-as-one.md`). With `ty`,
    /// the purge is of what the source brought into that type and the
    /// link types of its relations, and all of this is said of that
    /// selection alone; a type that cannot be purged from is refused.
    /// With the `report_id` of a preflight, it is refused as invalid
    /// unless that report is of this purge at this head.
    async fn purge_source(
        &self,
        source: &SourceId,
        ty: Option<&TypeName>,
        report_id: Option<&str>,
    ) -> Result<crate::write::PurgeReport>;

    /// What [`Self::purge_source`] would do, appending nothing: the same
    /// report, refused as the purge would be, under an id the purge may
    /// repeat.
    async fn purge_preflight(
        &self,
        source: &SourceId,
        ty: Option<&TypeName>,
    ) -> Result<crate::write::PurgeReport>;

    /// Forget a source (`docs/decisions/058-a-source-may-be-forgotten.md`):
    /// append `source_forgotten` with the records it made and alone
    /// brought retired, destroy its keys, rebuild the projection, in which
    /// every value it sealed reads as erased at every position, and
    /// reclaim the storage, all under the writer's lock. Refused while the
    /// log holds its values of an erasable field in plaintext. With a
    /// preflight's id, refused unless that report is of this source at
    /// this head. A step a crash left owed is finished by the next
    /// writer's open; a reclaim that fails is not a failure of the act,
    /// which answers it in its report's `reclaim_owed`.
    async fn forget_source(
        &self,
        source: &SourceId,
        report_id: Option<&str>,
    ) -> Result<crate::write::ForgetReport>;

    /// What [`Self::forget_source`] would do, appending nothing, with the
    /// time it is expected to hold the writer's lock, under an id the act
    /// may repeat.
    async fn forget_preflight(&self, source: &SourceId) -> Result<crate::write::ForgetReport>;

    /// Forget several sources in one act, as [`Self::forget_source`]
    /// forgets one: one `source_forgotten` for each, appended together
    /// with the records the set's sources made and alone brought retired,
    /// their keys destroyed, one rebuild and one reclaim, under one hold
    /// of the writer. A set that is empty, names a source twice, names one the
    /// log holds no event of, or one whose values the log holds in
    /// plaintext, is refused whole with nothing appended; the first two
    /// before anything is read. With a preflight's id, refused unless
    /// that report is of this set at this head. A source of the set
    /// forgotten already
    /// appends nothing again; a set whose every source is forgotten
    /// finishes what is owed and answers it done, whatever report it names
    /// (`docs/decisions/058-a-source-may-be-forgotten.md`, extended).
    async fn forget_sources(
        &self,
        sources: &[SourceId],
        report_id: Option<&str>,
    ) -> Result<crate::write::ForgetSetReport> {
        let _ = (sources, report_id);
        Err(crate::Error::unbound(crate::error::Seam::Operation {
            op: "forget_sources".to_owned(),
        }))
    }

    /// What [`Self::forget_sources`] would do, appending nothing: each
    /// source's report and the time the one act is expected to hold the
    /// writer's lock, under one id the act may repeat.
    async fn forget_sources_preflight(
        &self,
        sources: &[SourceId],
    ) -> Result<crate::write::ForgetSetReport> {
        let _ = sources;
        Err(crate::Error::unbound(crate::error::Seam::Operation {
            op: "forget_sources_preflight".to_owned(),
        }))
    }

    /// Take back part of a source
    /// (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`): append
    /// `values_removed` with the records the source made left holding
    /// nothing of another hand retired, rebuild the projection, in which each value a selected
    /// event of the source wrote reads as erased at every position, and
    /// reclaim the storage, all under the writer's lock. A key field and a
    /// reference field are never taken. The versions of the source's key
    /// whose values it all selects, the longest prefix of them, are
    /// destroyed, and what they sealed is shredded; with
    /// `require_shredded`, refused while a value of an erasable field would
    /// be redacted only. With a preflight's id, refused unless that report
    /// is of this removal at this head. A step a crash left owed is
    /// finished by the next writer's open; a reclaim that fails is
    /// answered in the report's `reclaim_owed`, as for
    /// [`Self::forget_source`].
    async fn remove_values(
        &self,
        source: &SourceId,
        selection: &crate::write::Selection,
        require_shredded: bool,
        report_id: Option<&str>,
    ) -> Result<crate::write::RemoveReport>;

    /// What [`Self::remove_values`] would do, appending nothing, with the
    /// time it is expected to hold the writer's lock, under an id the act
    /// may repeat.
    async fn remove_values_preflight(
        &self,
        source: &SourceId,
        selection: &crate::write::Selection,
    ) -> Result<crate::write::RemoveReport>;

    /// Give back the storage the projection no longer uses. It appends
    /// nothing and changes no answer, no event and no byte of an export;
    /// it prunes no history and does not seal. Only the writer's handle
    /// compacts; writes wait while it runs
    /// (`docs/decisions/040-maintenance-never-changes-an-answer.md`). It
    /// is an act's reclaim too: one that runs clears
    /// [`Self::reclaim_owed`] and finishes the acts waiting on it.
    async fn compact(&self) -> Result<crate::write::CompactReport>;

    /// Bind the embedding backend the lake's declarations, writes and
    /// searches read from now on, in place of the one bound at open or
    /// by an earlier bind, whatever its model. Nothing is recorded: the
    /// binding is the host's configuration, not a fact of the log. What
    /// fell due while none of its model was bound is made in a task of
    /// the bind's own, as a writer's open with the backend makes it, a
    /// recorded failure aside, which `recompute` calls again;
    /// [`Self::vector_coverage`] counts it due until then
    /// (`docs/decisions/022-functions-are-declarations.md`, extended).
    fn bind_embed(&self, backend: Arc<dyn crate::seam::EmbedBackend>) -> Result<()>;

    /// Bind the reranker a search's second stage reads from now on, in
    /// place of the one bound at open or by an earlier bind. Nothing is
    /// recorded: the binding is the host's configuration, not a fact of
    /// the log. Like `bind_embed`, it is the host's alone; a remote
    /// handle answers it `Unbound` as a host operation
    /// (`docs/decisions/067-a-search-may-take-a-second-stage.md`).
    fn bind_rerank(&self, backend: Arc<dyn crate::seam::Reranker>) -> Result<()> {
        let _ = backend;
        Err(crate::Error::unbound(crate::error::Seam::Operation {
            op: "bind_rerank".to_owned(),
        }))
    }

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

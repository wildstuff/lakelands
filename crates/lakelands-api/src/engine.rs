//! The engine seam: how a lake's log and projection are kept.
//!
//! An engine stores events and gives them back byte-identically, holds
//! the writer lease, and keeps the projection the fold writes. It knows
//! nothing of grants, provenance rules or the expression grammar's
//! meaning beyond evaluating filters it is handed: the store's core
//! decides, the engine keeps. Every behaviour an engine promises is
//! checked by the conformance suite on every engine.
//!
//! The traits are synchronous and `dyn`-compatible: engines do blocking
//! storage work, and the store runs them off its async callers. A new
//! engine is a port, not a re-implementation; `docs/seams.md` states the
//! contract of each method.
//!
//! Two rules shape the whole seam:
//!
//! - **The store canonicalises; the engine keeps bytes.** The store
//!   produces every event's canonical bytes and hash (`docs/log-format.md`
//!   §1.2) and hands them to the engine as a [`CanonicalEvent`]. The
//!   engine stores the bytes and returns them unchanged; the store parses
//!   and verifies what it reads. Byte identity is therefore checkable
//!   through the seam, and the conformance suite checks it.
//! - **The projection keeps its history.** Every `put_*` of the fold adds
//!   a state valid from a position; it never destroys the state it
//!   supersedes. A view at an earlier position is a filter over kept
//!   states, not a replay, so "what did the agent see" is a cheap read.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::event::{Digest, EventId, SegmentDigest};
use crate::expr::Expr;
use crate::id::{FieldName, LakeId, Position, RecordKey, RelationName, TypeName};
use crate::location::Location;
use crate::query::{Edge, Walk};
use crate::read::Order;
use crate::record::{StoredRecord, Value};
use crate::time::Timestamp;
use crate::write::{Declaration, Proposal, ProposalFilter};

/// The writer lease one process holds on a lake
/// (`docs/decisions/008-one-writer-per-lake.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    /// The fencing token; greater than every token before it.
    pub fence: u64,
    /// Who holds it, for the operator.
    pub holder: String,
}

/// One event as it crosses the engine seam: its canonical bytes, and the
/// envelope members an engine needs to check an append without parsing
/// them. The store builds it; the engine stores `bytes` unchanged and
/// gives the same value back from `read`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalEvent {
    /// The event's position.
    pub position: Position,
    /// The event's identifier.
    pub id: EventId,
    /// The hash of the event before it.
    pub prev: Digest,
    /// The event's own hash.
    pub hash: Digest,
    /// The lease fence it was appended under.
    pub fence: u64,
    /// Its canonical bytes (`docs/log-format.md` §1.2), without the line
    /// feed that ends it in a segment file.
    pub bytes: Vec<u8>,
}

/// How a lake is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenMode {
    /// Read only. The handle never takes the lease: `take_lease` and
    /// `append` are refused. Any number of processes may open a lake so
    /// (`docs/decisions/008-one-writer-per-lake.md`).
    Read,
    /// Read and, once the lease is taken, write. The lake must exist.
    Write,
    /// As `Write`, creating the lake if it does not exist.
    Create,
    /// As `Write`, for a rebuild: the lake must exist, and its projection
    /// is opened whatever layout made it, since `clear` makes it anew at
    /// the engine's own before anything reads it. The store's open in this
    /// mode clears the projection under the lease and folds the whole log
    /// (`docs/decisions/004-the-change-feed-is-the-log.md`).
    Rebuild,
}

/// One lake's storage, as an engine keeps it.
pub trait LakeEngine: Send + Sync {
    /// The engine's name (`sqlite`, `memory`).
    fn name(&self) -> &str;

    /// The events a rebuild folds a second on this engine, as measured: a
    /// host is told from it how long an act that rebuilds holds the
    /// writer's lock. An engine that states none is taken as slow as the
    /// slowest measured here (PostgreSQL's).
    fn rebuild_rate(&self) -> u64 {
        2_500
    }

    /// The lake it keeps.
    fn lake(&self) -> &LakeId;

    /// Take the writer lease with a fencing token greater than any
    /// before. Fails at once, never waits, when another process holds
    /// it, with an `Unavailable` error whose message names the holder.
    /// Refused on a handle opened [`OpenMode::Read`].
    fn take_lease(&self, holder: &str) -> Result<Lease>;

    /// Give the lease up. A later `take_lease` gets a greater token.
    /// Releasing a lease that is no longer current changes nothing. A
    /// handle that is dropped releases the lease it took and still holds.
    fn release_lease(&self, lease: &Lease) -> Result<()>;

    /// The last event's position and hash; `None` for an empty lake.
    fn tip(&self) -> Result<Option<(Position, Digest)>>;

    /// Append events, all or none, durably before returning. Refused if
    /// `lease` is not the current lease or any event's `fence` differs
    /// from it, if the positions do not continue the tip without a gap,
    /// or if the `prev` of each event is not the hash before it. The
    /// engine stores each event's bytes unchanged.
    fn append(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()>;

    /// Append events carried over from another lake's log, unchanged, as
    /// an import does (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`):
    /// as [`append`](Self::append), except that an event's `fence` is the
    /// one it was written under in its own lake, never below the fence of
    /// the event before it and 0 at position 0, rather than `lease`'s.
    /// Refused unless the log holds nothing but imported events (it is
    /// empty, or every event in it came through `import`), so a lake that
    /// was ever appended to is never imported into. Afterwards the lease a
    /// later `take_lease` answers is greater than every fence imported.
    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()>;

    /// Events from `from` on, at most `limit`, exactly as appended: the
    /// same members, the same bytes.
    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>>;

    /// Close the active segment and begin the next with the events `first`
    /// answers for the closed segment's range and digest, in one step: both
    /// hold, or the log is as it was, so a segment after the first never
    /// exists without its first event (`docs/log-format.md` §4.2). The
    /// events enter as [`append`](Self::append)'s do, or as
    /// [`import`](Self::import)'s when `imported`, and are refused as
    /// invalid where those would be; so is none, and an error from `first`
    /// is the seal's own. `first` is asked once, after the lease is found
    /// current, and runs inside the step: it calls nothing of the engine.
    /// `None`, and `first` is not asked, if the active segment holds no
    /// event.
    fn seal_segment(
        &self,
        lease: &Lease,
        imported: bool,
        first: &mut dyn FnMut(&SegmentDigest) -> Result<Vec<CanonicalEvent>>,
    ) -> Result<Option<SegmentDigest>>;

    /// Every sealed segment, oldest first.
    fn segments(&self) -> Result<Vec<SegmentDigest>>;

    /// The projection's write side, for the fold.
    fn projection(&self) -> &dyn ProjectionStore;

    /// A read view of the projection: at the head, or as it was right
    /// after folding `as_of`. Served from the states the projection kept
    /// (see the module documentation), never by replaying the log. Refused
    /// as `OutOfRange` for a position the projection has not folded.
    fn view(&self, as_of: Option<Position>) -> Result<Box<dyn ProjectionView + '_>>;

    /// The rankings of one pull of a search over `ty`, each cut at `k`,
    /// all read from one state of the head's indexes, with the position
    /// the projection was folded through in that state. The indexes
    /// describe the head only, so the candidates are the records live
    /// there, restricted ones included; what a read at any position may
    /// see of them is the store's, which judges them at the position
    /// answered (`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`). An
    /// engine that keeps no vector index refuses a vector's ranking as
    /// unbound.
    fn ranked(&self, ty: &TypeName, asked: &Rankings, k: u32) -> Result<Ranked>;

    /// The engine's vector index, if it keeps one.
    fn vectors(&self) -> Option<&dyn VectorIndex>;
}

/// The projection's write side. The fold calls it inside one transaction
/// per chunk (an append for a write; for a rebuild or an open's catch-up a
/// page of the log, or a part of one: a page that computes a type's derived
/// fields again commits and begins again inside it); the transaction
/// commits with the position it folded through, so a crash leaves the
/// projection at a whole chunk.
pub trait ProjectionStore: Send + Sync {
    /// Start a transaction.
    fn begin(&self) -> Result<()>;

    /// Commit, recording that the projection now reflects the log
    /// through `through`.
    fn commit(&self, through: Position) -> Result<()>;

    /// Abandon the transaction.
    fn rollback(&self);

    /// The last position the projection reflects; `None` when empty.
    fn folded_through(&self) -> Result<Option<Position>>;

    /// Store a declaration in force from `at`.
    fn put_declaration(
        &self,
        declaration: &Declaration,
        version: u32,
        schema_hash: Option<&str>,
        at: Position,
    ) -> Result<()>;

    /// Store a record's whole state as of `record.position`. The state
    /// it supersedes is kept, valid up to that position, so an earlier
    /// view still sees it. The engine derives its own indexes, full-text
    /// entries and edges from it and the declarations in force.
    fn put_record(&self, record: &StoredRecord) -> Result<()>;

    /// End every key of the head-only name `name` ([`crate::seam::is_head_only`])
    /// at once, at `at`: whatever its keys held, put earlier in the
    /// transaction or before it, is deleted, and a key put after it in the
    /// same transaction is kept. Any other name is refused as invalid.
    fn end_head_only(&self, name: &TypeName, at: Position) -> Result<()>;

    /// Store a proposal's state as of its latest event, at `at`; earlier
    /// states are kept, so a view before `at` still sees the earlier one.
    fn put_proposal(&self, proposal: &Proposal, at: Position) -> Result<()>;

    /// Record whether a record is a member of an object set from `at` on;
    /// earlier membership is kept.
    fn put_membership(
        &self,
        set: &str,
        ty: &TypeName,
        key: &RecordKey,
        member: bool,
        at: Position,
    ) -> Result<()>;

    /// Store a seam's recorded output, for replay, valid from the event
    /// that recorded it at `at`.
    fn put_output(&self, seam: &str, input: &Digest, output: &Value, at: Position) -> Result<()>;

    /// Remember that the event `applied` names was applied at its
    /// position, so that a redelivery is recognised, and keep what the
    /// feed's index holds of it: its kind and what it is about, which
    /// [`ProjectionView::matching`] answers from.
    fn mark_applied(&self, applied: &Applied) -> Result<()>;

    /// Delete everything, kept history included, for a rebuild from the
    /// log: a disk engine drops the projection's tables and makes them
    /// again at its own layout, so a projection opened
    /// [`OpenMode::Rebuild`] whatever layout made it is read from here on.
    fn clear(&self) -> Result<()>;

    /// Give the storage the projection no longer uses back, and change
    /// nothing a view answers: no state, kept history included, no
    /// applied id, no recorded output. Called outside a transaction, under
    /// the writer's lock. A compact that fails leaves every answer as it
    /// was (`docs/decisions/040-maintenance-never-changes-an-answer.md`).
    fn compact(&self) -> Result<Reclaimed>;
}

/// What a compact of the projection measured: its storage in bytes before
/// and after, as the engine measures it. An engine that keeps nothing to
/// reclaim measures nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reclaimed {
    /// The projection's storage before.
    pub before: Option<u64>,
    /// The projection's storage after.
    pub after: Option<u64>,
}

impl Reclaimed {
    /// Nothing measured, nothing reclaimed.
    pub fn nothing() -> Reclaimed {
        Reclaimed::default()
    }
}

/// An event the fold applied, as the projection remembers it: its id,
/// and the kind and the types and keys the change feed's filter reads it
/// by (`docs/seams.md` §`ChangeFeed`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    /// The event's identifier.
    pub id: EventId,
    /// Its position.
    pub at: Position,
    /// Its kind, as [`crate::event::EventBody::kind`] names it.
    pub kind: &'static str,
    /// What it is about, one entry a type it concerns; never empty, so an
    /// event about nothing is one entry with neither.
    pub about: Vec<About>,
}

/// A type and a key an event is about, as a feed filter reads them: the
/// key is of that type, or of none for an event that names no type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct About {
    /// The type.
    pub ty: Option<TypeName>,
    /// The record's key.
    pub key: Option<RecordKey>,
}

/// The positions a feed filter admits in a range, as the projection's
/// index answers them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matched {
    /// The positions admitted, in order.
    pub positions: Vec<Position>,
    /// The last position decided: every position up to it that the filter
    /// admits is in `positions`. None where nothing is decided, as from a
    /// position a projection a rebuild cleared has not refolded: the store
    /// then reads the log.
    pub through: Option<Position>,
}

/// One declaration in force, as a view returns it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Declared {
    /// The declaration.
    pub declaration: Declaration,
    /// Its version.
    pub version: u32,
    /// For a type, its schema hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_hash: Option<String>,
    /// The position it has been in force since.
    pub since: Position,
}

/// Where a scan continues: the sort values and key of the last row of the
/// previous page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct After {
    /// The sort fields' values, in `order_by` order.
    pub values: Vec<Value>,
    /// The last row's key.
    pub key: RecordKey,
}

/// A scan of one type's records.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scan {
    /// Only records for which this holds. The engine applies it exactly.
    pub filter: Option<Expr>,
    /// Sort keys; the record key breaks ties.
    pub order_by: Vec<Order>,
    /// Continue after this row.
    pub after: Option<After>,
    /// At most this many.
    pub limit: u32,
    /// Only records valid at this instant, for a valid-timed type.
    pub valid_at: Option<Timestamp>,
    /// Include retired records.
    pub retired: bool,
    /// Include restricted records.
    pub restricted: bool,
    /// Only records whose field holds exactly this JSON value, pair by
    /// pair: identity of the stored value, not the expression's `=`. A
    /// derived relation's walk asks for its matches this way.
    pub equal: Vec<(FieldName, Value)>,
    /// Only records that hold one of these tuples, each exactly as
    /// `equal` holds its pairs; none, no such bound. A derived relation's
    /// walk asks for the matches of a page of records in one scan this
    /// way (lakelands#971).
    pub equal_any: Vec<Vec<(FieldName, Value)>>,
}

/// The rankings one pull of a search asks an engine for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rankings {
    /// A text, matched in the type's searchable fields, or in those of
    /// them it names.
    pub text: Option<(String, Option<Vec<FieldName>>)>,
    /// Each vector field asked, with its query vector.
    pub vectors: Vec<(String, Vec<f32>)>,
}

/// What [`LakeEngine::ranked`] answers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ranked {
    /// The position the projection was folded through in the state the
    /// lists were read from; `None` for an empty one.
    pub position: Option<Position>,
    /// One list for each ranking asked, the text's first, then each
    /// vector's: keys and scores, best first, equal scores by key. A
    /// text scores as `docs/seams.md` §`ranked` says, a vector by the
    /// cosine (higher is closer).
    pub lists: Vec<Vec<(RecordKey, f64)>>,
}

/// A read view of the projection at one position. Records come back
/// whole and unmasked; masking is the store's.
pub trait ProjectionView {
    /// The position the view reflects; `None` for an empty lake.
    fn position(&self) -> Option<Position>;

    /// Every declaration in force.
    fn declarations(&self) -> Result<Vec<Declared>>;

    /// Every state of a declaration that a later one replaced after
    /// `from` and by `to`, as it stood before. With those in force at
    /// `to`, they hold what was in force at every position from `from` on,
    /// one read for many (lakelands#1064). `None` from an engine that keeps
    /// no such read: the store then reads each position's declarations.
    fn replaced(&self, from: Position, to: Position) -> Result<Option<Vec<Declared>>> {
        let _ = (from, to);
        Ok(None)
    }

    /// One record, retired and restricted ones included.
    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>>;

    /// Records of one type.
    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>>;

    /// How many records of one type `scan` selects: its flags, `valid_at`,
    /// `equal` and `filter` as for [`ProjectionView::scan`], its order,
    /// keyset position and limit ignored. Counting holds no more than one
    /// batch of the type at a time.
    fn count(&self, ty: &TypeName, scan: &Scan) -> Result<u64>;

    /// Many records of one type; a key with no record is left out.
    /// Retired and restricted records included; ordered by key.
    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>>;

    /// The links of many records of one type, over one relation or all:
    /// one list per key, in the keys' order. A link between two of the
    /// keys is in both their lists.
    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Vec<Edge>>>;

    /// Proposals.
    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>>;

    /// Whether a record is a member of an object set.
    fn is_member(&self, set: &str, ty: &TypeName, key: &RecordKey) -> Result<bool>;

    /// The keys of the records of `ty` that are members of an object set:
    /// one read where a walk over the type would ask for each record.
    fn members(&self, set: &str, ty: &TypeName) -> Result<BTreeSet<RecordKey>>;

    /// The position an event id was applied at, if it was.
    fn applied(&self, id: &EventId) -> Result<Option<Position>>;

    /// [`ProjectionView::applied`] for many ids, one answer an id in their
    /// order: an ingest window asks its ids in one call.
    fn applied_many(&self, ids: &[EventId]) -> Result<Vec<Option<Position>>> {
        ids.iter().map(|id| self.applied(id)).collect()
    }

    /// A seam's recorded output for this input, if recorded, with the
    /// position it was recorded at.
    fn output(&self, seam: &str, input: &Digest) -> Result<Option<(Position, Value)>>;

    /// The positions from `from` through `through` whose events `filter`
    /// admits, by what [`ProjectionStore::mark_applied`] kept of them, in
    /// order and at most `limit` (0 is read as 1). Nothing past the view's
    /// position is answered, and the store asks for no range past it. Its
    /// `through` is the last position answered when `limit` is reached,
    /// else `through` or the view's position, whichever is first, or an
    /// earlier one where the engine bounds its work
    /// (`lakelands_core::feed::WINDOW`), and never past what the projection
    /// had folded where the index is read, in the same snapshot: a view
    /// taken before a rebuild's `clear` decides only what the refold has
    /// reached. It is at least `from`, or none where nothing from `from` on
    /// is folded yet, and the store then reads the log. A filter that names nothing admits every event, and the store
    /// does not ask the index for it.
    fn matching(
        &self,
        filter: &crate::seam::FeedFilter,
        from: Position,
        through: Position,
        limit: u32,
    ) -> Result<Matched>;
}

/// The engine's vector index, beside the projection: its write side and
/// its size. Vectors are indexed at the head, in the fold's transaction,
/// and read by [`LakeEngine::ranked`].
pub trait VectorIndex: Send + Sync {
    /// Index a record's vector for one field.
    fn put(&self, ty: &TypeName, key: &RecordKey, field: &str, vector: &[f32]) -> Result<()>;

    /// Remove a record's vector for one field.
    fn remove(&self, ty: &TypeName, key: &RecordKey, field: &str) -> Result<()>;

    /// How many vectors one field holds.
    fn count(&self, ty: &TypeName, field: &str) -> Result<u64>;
}

/// How an engine is found by name and opened at a location.
pub trait EngineOpener: Send + Sync {
    /// The name the engine answers to in [`crate::location::EngineConfig`].
    fn name(&self) -> &str;

    /// The first and the last event of the lake's log at `location`, read
    /// without opening the lake: nothing is made or changed, in the log's
    /// files or tables or in the projection, nothing of a lake is judged,
    /// and a place with no lake stays with nothing. `None` where there is
    /// no lake or its log is empty. An engine may refuse a place that
    /// holds something other than a lake, as its [`open`] does, and
    /// refuses as damaged what it can tell to be part of a lake without
    /// its events, which is not a place with no lake. The store
    /// asks this before every [`open`] and before an import, and opens
    /// only a lake written in the format it reads
    /// (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
    /// A tip that moves between the two calls was written by a writer
    /// that passed the same judgement, in a format the store reads; what
    /// a writer appends for its own open, an upgrade's marker among it,
    /// is decided from the tip it reads under its lease.
    ///
    /// [`open`]: EngineOpener::open
    fn log_ends(&self, location: &Location) -> Result<Option<(CanonicalEvent, CanonicalEvent)>>;

    /// Open the lake at `location`. Opening never takes the lease: a
    /// writer takes it afterwards with `take_lease`, and a reader opened
    /// [`OpenMode::Read`] never can.
    fn open(&self, location: &Location, mode: OpenMode) -> Result<Box<dyn LakeEngine>>;
}

/// Compile-time proof that the engine seam is `dyn`-compatible.
#[allow(dead_code)]
fn engine_seam_is_dyn_compatible(
    engine: &dyn LakeEngine,
    opener: &dyn EngineOpener,
) -> Option<Position> {
    let _: &dyn ProjectionStore = engine.projection();
    let _: Option<&dyn VectorIndex> = engine.vectors();
    let _ = opener.name();
    engine.view(None).ok()?.position()
}

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

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::event::{Digest, EventId, SegmentDigest};
use crate::expr::Expr;
use crate::id::{FieldName, LakeId, Position, RecordKey, RelationName, SourceId, TypeName};
use crate::location::Location;
use crate::query::{Edge, Walk};
use crate::read::{Order, ThinnedAct};
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

/// A frame of the log's stored form (`docs/log-format.md` §1.2): one line
/// holding the members the `events` events from `first` on share, which
/// their own lines leave out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The position of the first event it covers.
    pub first: Position,
    /// How many events it covers, its line's `events`.
    pub events: u32,
    /// Its line's canonical bytes, without the line feed that ends it.
    pub bytes: Vec<u8>,
}

/// One event as it crosses the engine seam: its line of the stored form
/// and the frame it shares, and the envelope members an engine needs to
/// check an append without parsing them. The store builds it, `bytes`
/// holding `id` as their own `id` member; the engine stores `bytes` and
/// `frame` unchanged and gives the same value back from `read`, where the
/// events one frame covers share it. It may keep the id only in `bytes`
/// and answer the one the line holds, so an event whose `bytes` hold
/// another id, or none, is outside the contract.
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
    /// Its line of the stored form (`docs/log-format.md` §1.2), without
    /// the line feed that ends it in a segment file: its canonical bytes
    /// less what its frame holds and what the event before it gives.
    pub bytes: Vec<u8>,
    /// The frame whose members its line leaves out, if any.
    pub frame: Option<std::sync::Arc<Frame>>,
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

    /// The events a rebuild folds a second on this engine, as measured on
    /// a plain type: a host is told from it how long an act that rebuilds
    /// holds the writer's lock until the handle has rebuilt once and
    /// measured its own lake. An engine that states none is taken as slow
    /// as the slowest measured here (PostgreSQL's).
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

    /// Whether no process holds the lease now, asked of any handle,
    /// read-only too: it takes nothing and changes nothing, and is stale
    /// as soon as it answers, so a caller that finds it free still takes
    /// the lease to know. A standby asks it before it opens the lake for
    /// writing. An engine that cannot tell answers `true`.
    fn lease_free(&self) -> Result<bool> {
        Ok(true)
    }

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

    /// The bytes the active segment has committed, as its events restore
    /// (each line's bytes and its line feed, each block's `raw`, never a
    /// block's stored bytes), as this handle last read them: what the
    /// store seals by size against `limits.seal_after_bytes`
    /// (`docs/decisions/074-a-segment-is-sealed-by-size.md`,
    /// `docs/decisions/098-a-lake-stores-what-it-cannot-derive.md`). An engine
    /// whose segment is nothing to move or back up on its own answers
    /// none, as this default does, and is never sealed by size.
    fn active_segment_bytes(&self) -> Option<u64> {
        None
    }

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
    /// there, restricted ones included, but a text's are those of the
    /// class `asked.text_stats` names, scored by BM25F over its
    /// statistics, kept per type, field and hold or counted by the store
    /// (`docs/seams.md` §`ranked`); what a read at any position may see
    /// of them is the store's, which judges them at the position answered
    /// (`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
    /// An engine that keeps no vector index refuses a vector's ranking as
    /// unbound.
    fn ranked(&self, ty: &TypeName, asked: &Rankings, k: u32) -> Result<Ranked>;

    /// The engine's vector index, if it keeps one.
    fn vectors(&self) -> Option<&dyn VectorIndex>;

    /// On a handle [`EngineOpener::open_scratch`] opened: mark its
    /// projection built through the position it is folded through, with the
    /// log's hash there, so that the scratch outlives this handle and the
    /// next to open it, or [`swap`](Self::swap) it in, checks it against the
    /// log (`docs/decisions/071-a-rebuild-builds-beside-the-projection.md`).
    /// The mark holds while the scratch stays folded through that position;
    /// a scratch folded on and not marked again is removed when its handle
    /// is dropped. An open transaction is abandoned. Answers the position
    /// marked, none for a scratch that folded nothing, which no mark keeps.
    /// Refused as unavailable on any other handle and where
    /// the log does not hold the position folded through; an engine that
    /// keeps no scratch refuses as unbound, as this default does.
    fn mark_built(&self) -> Result<Option<Position>> {
        Err(crate::Error::unbound(crate::error::Seam::Operation {
            op: "mark_built".to_owned(),
        }))
    }

    /// Put the projection built beside this lake's in its place, in one
    /// step, once it is checked: this handle holds `lease`, no handle has
    /// the scratch open, it is this lake's, laid out by this build, marked
    /// built, still folded through its mark, and the mark is the log's tip
    /// and its hash. Refused as unavailable otherwise, the projection and
    /// the scratch both left as they were. Afterwards this handle reads and
    /// folds the projection it put in place, the one it replaced is gone,
    /// and so is the scratch. An open transaction is abandoned, by a swap
    /// refused as well as one done, so none is held across it. Answers the
    /// position the projection is folded through. A disk engine may also
    /// refuse while another handle reads the projection it would replace.
    /// An engine that keeps no scratch refuses as unbound, as this default
    /// does.
    fn swap(&self, lease: &Lease) -> Result<Option<Position>> {
        let _ = lease;
        Err(crate::Error::unbound(crate::error::Seam::Operation {
            op: "swap".to_owned(),
        }))
    }

    /// Whether a [`swap`](Self::swap) on this handle would find no other
    /// handle reading the projection it would replace, asked before a
    /// rebuild folds the log beside it, so no fold is spent on a swap the
    /// engine refuses for a reader. A handle may open between the answer
    /// and the swap, which then refuses as it says. An engine whose swap
    /// waits for its readers, or that keeps no scratch, answers true, as
    /// this default does.
    fn may_swap(&self) -> Result<bool> {
        Ok(true)
    }
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
    /// view still sees it, except under a kept name
    /// ([`crate::seam::KEPT_PREFIX`]) or a head-only one, where the put
    /// deletes it. The engine is never asked to put a state at a position
    /// earlier than one its key already holds: the fold puts a key's states
    /// in the order it folds them. The engine derives its own indexes,
    /// full-text entries and edges from it and the declarations in force.
    fn put_record(&self, record: &StoredRecord) -> Result<()>;

    /// Put one record as [`ProjectionStore::put_record`] does, for a
    /// caller that says whether a setting it puts must be made whole at
    /// its event (`whole`: a fold from the log, a rebuild or an import) or
    /// may be built beside the writer (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`).
    /// A setting put beside marks what it asks for as building, and no
    /// read uses it until the build's `Keep`; an engine that keeps no
    /// index for it puts it whole either way.
    fn put_record_for(&self, record: &StoredRecord, whole: bool) -> Result<()> {
        let _ = whole;
        self.put_record(record)
    }

    /// End every key of the head-only name `name` ([`crate::seam::is_head_only`])
    /// at once, at `at`: whatever its keys held, put earlier in the
    /// transaction or before it, is deleted, and a key put after it in the
    /// same transaction is kept. Any other name is refused as invalid.
    fn end_head_only(&self, name: &TypeName, at: Position) -> Result<()>;

    /// Store the entry of the order `name` at `key`, or delete the one
    /// there where `entry` is none: what the fold keeps of an extreme's
    /// members ([`crate::seam::order_name`]). The bytes are the entry as
    /// the store wrote it, which an engine hands back unchanged: they hold
    /// the member's id and the member's value as the fold read it, and nothing
    /// that a
    /// read of the lake answers. An engine keeps no state of an order entry
    /// and nothing indexes it; a key put again holds the new entry alone.
    /// The name is not judged: the store names the order, and no door
    /// writes one.
    fn put_order(&self, name: &TypeName, key: &RecordKey, entry: Option<&[u8]>) -> Result<()>;

    /// Delete every entry of the order `name` at once: what was put before
    /// it in the transaction, or before the transaction, is gone, and what
    /// is put after it in the same transaction is kept.
    fn end_order(&self, name: &TypeName) -> Result<()>;

    /// Delete every state `of` names holds, kept history included, inside
    /// the transaction: what was put before it in the transaction or
    /// before the transaction is gone, and what is put after it is kept.
    /// A record's states take with them its memberships in every set and
    /// the entries the engine derived from them (its indexes, full-text
    /// entries and edges); its vectors are the lake's to keep. A scoped
    /// refold drops a record's states before it puts them again
    /// (`docs/decisions/058-a-source-may-be-forgotten.md`).
    fn drop_states(&self, of: StatesOf<'_>) -> Result<()>;

    /// The states `key` of `ty` holds whose positions lie after `after`
    /// and at or before `through`, oldest first, at most `limit`, the
    /// transaction's own puts included. A thinning act reads the states
    /// it is about to drop page by page, so a kept view's group a dropped
    /// state counted in is marked stranded rather than refolded short
    /// (`docs/decisions/102-a-thinned-series-keeps-its-roll-ups.md`).
    fn states(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        after: Option<Position>,
        through: Position,
        limit: u32,
    ) -> Result<Vec<StoredRecord>>;

    /// Store a thinning act's row: its type, the field and horizon it
    /// judged, the last position its parts had got through and how many
    /// records it has taken. `at` is the position the act began at; a row
    /// already begun keeps the first position it was put with, so each
    /// part moves `through` and `gone` alone
    /// (`docs/decisions/102-a-thinned-series-keeps-its-roll-ups.md`).
    fn put_thinned_act(&self, act: &ThinnedAct, at: Position) -> Result<()>;

    /// Store a thinned record's tombstone: its key, the horizon that took
    /// it and the position of the act's part that took it, so a correction
    /// is refused by one seek and a read names the horizon
    /// (`docs/decisions/102-a-thinned-series-keeps-its-roll-ups.md`).
    fn put_thinned(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        before: Timestamp,
        at: Position,
    ) -> Result<()>;

    /// Store a proposal's state as of its latest event, at `at`; earlier
    /// states are kept, so a view before `at` still sees the earlier one.
    fn put_proposal(&self, proposal: &Proposal, at: Position) -> Result<()>;

    /// Store a quarantined record's state as of its latest event, at `at`;
    /// earlier states are kept, as a proposal's are
    /// (`docs/decisions/085-a-refused-record-may-be-quarantined-until-it-is-run-again-dismissed-or-its-key-lands.md`).
    fn put_quarantined(&self, row: &crate::quarantine::Quarantined, at: Position) -> Result<()>;

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
    /// Deprecated in favour of [`Self::compact_step`], of which this is
    /// [`Compact::Whole`]; an engine implements one of the two.
    #[deprecated(
        since = "0.23.0",
        note = "`compact_step` takes the step; `Compact::Whole` is this compact"
    )]
    fn compact(&self) -> Result<Reclaimed> {
        self.compact_step(Compact::Whole)
    }

    /// Give the storage the projection no longer uses back, as much as
    /// `how` asks, under the same terms as [`Self::compact`], and under the
    /// writer's lock but for [`Compact::Beside`]. An act reclaims in these
    /// steps, a turn each
    /// (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`). An
    /// engine that takes no step keeps its whole compact as an act's
    /// reclaim: its journal step is the whole, and the others do nothing.
    fn compact_step(&self, how: Compact) -> Result<Reclaimed> {
        match how {
            #[allow(deprecated)]
            Compact::Whole | Compact::Journal => self.compact(),
            Compact::Beside | Compact::Pages(_) => Ok(Reclaimed::nothing()),
        }
    }

    /// One step of keeping an order index the lake chose beside the log
    /// (`docs/decisions/041-a-sorted-page-reads-its-page.md`). Called
    /// outside a transaction, under the writer's lock, and it changes no
    /// answer: a reader takes a field only once its step is [`Choose::Keep`].
    /// A step that names a field the type does not hold as the step needs
    /// does nothing, and so does a step naming a field the type's
    /// `sort_index.<type>` setting names, unless it is the setting's own
    /// build: the setting and the lake share the rows field by field.
    /// Answers the last key a [`Choose::Fill`] or [`Choose::DropChunk`]
    /// wrote, none once the type is filled or its field's rows are gone. An
    /// engine that keeps no order index refuses as unbound, as this default
    /// does.
    fn choose_order(&self, ty: &TypeName, step: &Choose) -> Result<Option<RecordKey>> {
        let _ = (ty, step);
        Err(crate::Error::unbound(crate::error::Seam::Operation {
            op: "choose_order".to_owned(),
        }))
    }

    /// One step of building the exact-text index a type's
    /// `lookup_index.<type>` setting asks for
    /// (`docs/decisions/057-an-exact-text-index-is-kept-where-it-is-looked-up.md`), beside
    /// the writer. Called outside a transaction, under the writer's lock,
    /// and it changes no answer: a reader takes a field only once its step
    /// is [`Look::Keep`]. Answers the last key a [`Look::Fill`] wrote, none
    /// once the type is filled. An engine that keeps no exact-text index
    /// has nothing to build, and answers none at every step, as this
    /// default does.
    fn look_index(&self, ty: &TypeName, step: &Look) -> Result<Option<RecordKey>> {
        let _ = (ty, step);
        Ok(None)
    }
}

/// What one [`ProjectionStore::drop_states`] deletes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatesOf<'a> {
    /// The record at a key of a type.
    Record(&'a TypeName, &'a RecordKey),
    /// A seam's output for one input.
    Output(&'a str, &'a Digest),
    /// Of these bases, which no record's state named at a view at the
    /// position ([`ProjectionView::unused_bases`]), those no state put
    /// after it names: a sweep's chunk
    /// (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`).
    Bases(&'a [u64], Position),
}

/// How much of its storage a [`ProjectionStore::compact_step`] gives back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compact {
    /// All of it, at once: the operator's compact, for a window of its own.
    Whole,
    /// What can be given back beside reads and writes, without the
    /// writer's lock; nothing on an engine whose reclaim needs it.
    Beside,
    /// The write-ahead journal truncated, where the engine keeps one.
    Journal,
    /// At most this many bytes of free pages.
    Pages(u64),
}

/// A step of building or dropping an order index the lake chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choose {
    /// Start keeping the field's rows as records are put. Reads do not use
    /// the field yet.
    Start(FieldName),
    /// Write the field's rows of the head records after `after`, at most
    /// `limit` of them, in key order.
    Fill {
        /// The field.
        field: FieldName,
        /// The key the last chunk ended at; none for the first.
        after: Option<RecordKey>,
        /// The most records the chunk writes.
        limit: u32,
    },
    /// The field's rows are whole: reads may use the index for it.
    Keep(FieldName),
    /// Stop keeping the field: its rows leave every record's in one step,
    /// no read looks at it meanwhile, and the step answers none.
    /// Deprecated in favour of [`Choose::DropChunk`], which clears them in
    /// parts beside the writer
    /// (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`).
    #[deprecated(
        since = "0.23.0",
        note = "`Choose::DropChunk` clears the field's rows in parts, a chunk a call"
    )]
    Drop(FieldName),
    /// Stop keeping the field, and delete its rows after `after`, at most
    /// `limit` of them, in key order: the mark `dropping` holds it, which
    /// no read looks at, and the last step removes the mark. A setting's
    /// write that takes its field back leaves the same mark, and the
    /// build's next [`Choose::Fill`] finds the mark gone and ends.
    DropChunk {
        /// The field.
        field: FieldName,
        /// The key the last chunk ended at; none for the first.
        after: Option<RecordKey>,
        /// The most records the chunk clears.
        limit: u32,
    },
}

/// A step of building the exact-text index a type's `lookup_index.<type>`
/// setting asks for (`docs/decisions/057-an-exact-text-index-is-kept-where-it-is-looked-up.md`).
/// A build is a job on the road, as an order index's is: reads take the
/// old road until [`Look::Keep`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Look {
    /// Write the rows of `fields` for the head records after `after`, at
    /// most `limit` of them, in key order.
    Fill {
        /// The fields being built.
        fields: Vec<FieldName>,
        /// The key the last chunk ended at; none for the first.
        after: Option<RecordKey>,
        /// The most records the chunk writes.
        limit: u32,
    },
    /// The fields' rows are whole: reads may use the index for them, and
    /// the rows of a field the setting no longer asks for go.
    Keep {
        /// The fields the build filled.
        fields: Vec<FieldName>,
    },
}

/// The fields of a type's order index the lake chose, and the builds of
/// the type's settings that are not whole, kept in the projection and
/// never in the log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chosen {
    /// The fields reads use the index for.
    pub kept: Vec<FieldName>,
    /// The fields being built: the fold keeps their rows, no read uses them.
    pub building: Vec<FieldName>,
    /// The fields a host's `sort_index.<type>` setting is having built:
    /// the fold keeps their rows, no read uses them.
    #[serde(default)]
    pub setting_building: Vec<FieldName>,
    /// The fields a host's `lookup_index.<type>` setting is having built:
    /// the fold keeps their rows, no read uses them until their `Keep`.
    #[serde(default)]
    pub looked_building: Vec<FieldName>,
    /// The fields the build will stop keeping, whose rows go at its
    /// `Keep`: reads keep using them until then.
    #[serde(default)]
    pub looked_going: Vec<FieldName>,
    /// The fields a `Drop` is clearing in parts: a setting that took a
    /// field back, or the lake letting one go. No read looks at them, and
    /// the last chunk removes them.
    #[serde(default)]
    pub dropping: Vec<FieldName>,
}

/// One pair of a `series_index.<type>` setting: the field naming the
/// series, then the field its records within the series are ordered by
/// (`docs/decisions/097-a-read-in-a-window-walks-its-window.md` §The
/// series index).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesPair {
    /// The field naming the series.
    pub series: FieldName,
    /// The order field within the series.
    pub order: FieldName,
}

/// The pairs a type's `series_index.<type>` setting asks for, and whether
/// each pair's rows are whole, kept in the projection and never in the
/// log. A pair is the host's, so it is kept here and not in [`Chosen`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesState {
    /// The pairs reads may use the index for.
    #[serde(default)]
    pub kept: Vec<SeriesPair>,
    /// The pairs being built: the fold keeps their rows, no read uses
    /// them.
    #[serde(default)]
    pub building: Vec<SeriesPair>,
    /// How many times a pair of this type has been started: a build reads
    /// it each turn, and one whose number moved fills its field from the
    /// first key again. A pair taken back and set again beside the writer
    /// is a new pair — its rows went with the take-back — and a build that
    /// went on from its cursor would skip the rows the take-back cleared
    /// and keep a pair that is not whole.
    #[serde(default)]
    pub started: u64,
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
    /// Whether free pages are left for a [`Compact::Pages`] to give back.
    #[serde(default)]
    pub more: bool,
}

impl Reclaimed {
    /// Nothing measured, nothing reclaimed.
    pub fn nothing() -> Reclaimed {
        Reclaimed::default()
    }
}

/// An event the fold applied, as the projection remembers it: its id,
/// the kind and the types and keys the change feed's filter reads it by
/// (`docs/seams.md` §`ChangeFeed`), and the source it is of, which
/// forgetting a source reads it by.
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
    /// The source it is of: a `record_ingested`'s is its envelope's; any
    /// other event's is the one its seal names, as a recorded output
    /// computed from a source's values does, else its envelope's.
    pub source: Option<SourceId>,
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

/// An interval on the key a scan reads in key order: a from, inclusive,
/// and an until, exclusive, each none for an end the read leaves open
/// (`docs/decisions/103-a-filter-on-a-kept-view-narrows-by-the-group-key.md`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyRange {
    /// The first key the scan hands.
    pub from: Option<RecordKey>,
    /// The first key the scan does not hand.
    pub until: Option<RecordKey>,
}

/// A scan of one type's records.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scan {
    /// Only records for which this holds. The engine applies it exactly.
    pub filter: Option<Expr>,
    /// Sort keys; the record key breaks ties.
    pub order_by: Vec<Order>,
    /// Only records of a kept name whose key is in this interval, on the
    /// key-ordered road. A read that asks for no other order narrows by
    /// it; every other read leaves it none.
    pub range: Option<KeyRange>,
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

/// A filter's bound on an ordered field: the inclusive interval of order
/// keys an engine's order index for `field` holds for the records a
/// windowed read may select
/// (`docs/decisions/097-a-read-in-a-window-walks-its-window.md`
/// §The window).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// The field the bound is on: the field of the order index the
    /// window is walked in.
    pub field: FieldName,
    /// The least order key inside the window.
    pub lo: Vec<u8>,
    /// The greatest order key inside the window.
    pub hi: Vec<u8>,
}

impl Window {
    /// Whether `key`, the sort key of an order-index row, lies inside the
    /// window.
    pub fn holds(&self, key: &[u8]) -> bool {
        key >= self.lo.as_slice() && key <= self.hi.as_slice()
    }
}

/// Where a walk of an order index stopped: the sort key and record key of
/// the last index row it passed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WindowAt {
    /// The row's sort key.
    pub sort_key: Vec<u8>,
    /// The record's key.
    pub key: RecordKey,
}

/// What [`ProjectionView::window_keys`] answers: the keys among the
/// index rows it passed that hold the scan's exact text, where it stopped,
/// and whether the window had no row past it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowBatch {
    /// The keys, in the index's order.
    pub keys: Vec<RecordKey>,
    /// The last index row passed; `None` where it passed none.
    pub after: Option<WindowAt>,
    /// The walk read fewer rows than it was asked for: the window has no
    /// row past `after`.
    pub done: bool,
}

/// What [`ProjectionView::window_sides`] counted for one round of the
/// choice between the window's road and today's
/// (`docs/decisions/097-a-read-in-a-window-walks-its-window.md`
/// §The smaller side).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowSides {
    /// The window's rows, counted up to the round's bound.
    pub inside: u64,
    /// Whether the window held no more rows than the round's bound, so
    /// `inside` is the whole window.
    pub whole: bool,
    /// The other side, counted up to four times `inside`.
    pub other: u64,
}

/// What [`ProjectionView::holders_inside`] answers: a batch of the exact
/// text's holders in key order, each with whether its order row lies
/// inside the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HolderBatch {
    /// The holders, in key order, each with whether its order row for the
    /// window's field lies inside the window.
    pub holders: Vec<(RecordKey, bool)>,
    /// The last holder passed; `None` where it passed none.
    pub after: Option<RecordKey>,
    /// The walk read fewer holders than it was asked for: the text has no
    /// holder past `after`.
    pub done: bool,
}

/// The rankings one pull of a search asks an engine for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rankings {
    /// A text, matched in the type's searchable fields, or in those of
    /// them it names.
    pub text: Option<(String, Option<Vec<FieldName>>)>,
    /// Each vector field asked, with its query vector.
    pub vectors: Vec<(String, Vec<f32>)>,
    /// The weight of a match of the text in each of these fields, under
    /// their names at the head; a field not named weighs 1.
    pub text_weights: BTreeMap<FieldName, f64>,
    /// The reader's selection. An engine that keeps an index streams an
    /// indexed field only from the graph of the class `seen.restricted`
    /// names, ranks exactly the records that hold `seen.equal`, and admits
    /// each vector candidate by `seen` on its head record: a vector list
    /// then holds the first `k` candidates of the field's sequence that it
    /// admits. Only its selection is read, not its order, `after` or
    /// `limit`. `None`: every live candidate, ranked as for a reader
    /// holding `restricted`.
    pub seen: Option<Scan>,
    /// Rank every vector list exactly, from no graph: for a reader whose
    /// records are no class.
    pub exact: bool,
    /// What the text's ranking is counted over (`docs/seams.md`
    /// §`ranked`). `None`: every live head, for a reader holding
    /// `restricted`.
    pub text_stats: Option<TextStats>,
}

/// What a text's ranking reads its statistics from: the live heads of a
/// reader's class, or what the store counted for a reader with no class.
#[derive(Clone, Debug, PartialEq)]
pub enum TextStats {
    /// The heads that are not held, as the engine keeps their counts: a
    /// reader without `restricted`. A held candidate is no hit.
    Unheld,
    /// What the store counted over the heads it admits a reader with no
    /// class, as it is told them: every live candidate ranks by these.
    Counted(TextCounts),
}

/// The statistics a lexical score reads (`docs/seams.md` §`ranked`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextCounts {
    /// Each searchable field's heads that hold a token in it, and the
    /// tokens they hold there, repeats counted.
    pub fields: BTreeMap<FieldName, (u64, u64)>,
    /// Each query token's heads that hold it in a searched field.
    pub df: BTreeMap<String, u64>,
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
    /// Whether the vector lists were judged by `seen`. An engine that
    /// answers `false` is judged by the store, at the cost of reading
    /// every candidate.
    pub judged: bool,
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

    /// The fields of `ty`'s order index the lake chose, at the head: none
    /// from an engine that keeps no such index.
    fn chosen(&self, ty: &TypeName) -> Result<Chosen> {
        let _ = ty;
        Ok(Chosen::default())
    }

    /// The series pairs `ty`'s `series_index.<type>` setting asks for, and
    /// whether each pair's rows are whole, at the head: none from an engine
    /// that keeps no such index
    /// (`docs/decisions/097-a-read-in-a-window-walks-its-window.md` §The
    /// series index).
    fn series(&self, ty: &TypeName) -> Result<SeriesState> {
        let _ = ty;
        Ok(SeriesState::default())
    }

    /// One record, retired and restricted ones included.
    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>>;

    /// The entry of the order `name` at `key`, as
    /// [`ProjectionStore::put_order`] wrote it, if it holds one.
    fn order(&self, name: &TypeName, key: &RecordKey) -> Result<Option<Vec<u8>>>;

    /// The entries of the order `name` after `after` in key order, at most
    /// `n` of them, each with its key: the page a seek over an extreme's
    /// order reads.
    fn order_page(
        &self,
        name: &TypeName,
        after: Option<&RecordKey>,
        n: u32,
    ) -> Result<Vec<(RecordKey, Vec<u8>)>>;

    /// The acts that thinned `ty` past a horizon, oldest first by the
    /// position each began at, at the head: every read says them, whatever
    /// its position, since a thinned record's states are gone at every
    /// position (`docs/decisions/102-a-thinned-series-keeps-its-roll-ups.md`).
    fn thinned_acts(&self, ty: &TypeName) -> Result<Vec<ThinnedAct>>;

    /// The horizon an act took `ty`'s `key` past, if one did, at the head.
    fn thinned_at(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<Timestamp>>;

    /// Records of one type.
    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>>;

    /// How many records of one type `scan` selects: its flags, `valid_at`,
    /// `equal` and `filter` as for [`ProjectionView::scan`], its order,
    /// keyset position and limit ignored. Counting holds no more than one
    /// batch of the type at a time.
    fn count(&self, ty: &TypeName, scan: &Scan) -> Result<u64>;

    /// At most `n` order-index rows of `window.field` inside the window,
    /// after the index position `after`, and the keys among them that hold
    /// the scan's exact text (the field and text `lakelands_core::engines`
    /// reads from the scan); the
    /// rows are walked in the index's order. `None` from an engine that
    /// keeps no order index for the field at the head, or no exact-text
    /// index where the scan holds a text: the read then takes today's
    /// road. Adds every index row passed to `rows_walked`, and none to
    /// `rows_read`: it hands on no record
    /// (`docs/decisions/097-a-read-in-a-window-walks-its-window.md`
    /// §The smaller side).
    fn window_keys(
        &self,
        ty: &TypeName,
        window: &Window,
        scan: &Scan,
        after: Option<&WindowAt>,
        n: u32,
    ) -> Result<Option<WindowBatch>> {
        let _ = (ty, window, scan, after, n);
        Ok(None)
    }

    /// One round of the choice between the window's road and today's: the
    /// window's rows counted up to `l`, whether that was the whole window,
    /// and the other side (the exact text's holders, or the type's order
    /// rows) counted up to four times the window's count. `None` from an
    /// engine that [`ProjectionView::window_keys`] answers `None` for.
    /// Both counts are index-only and add to `rows_walked`
    /// (`docs/decisions/097-a-read-in-a-window-walks-its-window.md`
    /// §The smaller side).
    fn window_sides(
        &self,
        ty: &TypeName,
        window: &Window,
        scan: &Scan,
        l: u64,
    ) -> Result<Option<WindowSides>> {
        let _ = (ty, window, scan, l);
        Ok(None)
    }

    /// At most `n` holders of the scan's exact text, in key order after
    /// `after`, each with whether its order row for `window.field` lies
    /// inside the window: the probe the other side reads under a text
    /// (`docs/decisions/097-a-read-in-a-window-walks-its-window.md`
    /// §The smaller side). `None` from an engine that keeps no exact-text
    /// index, or where the scan holds no exact text. Adds every holder
    /// passed to `rows_walked`, none to `rows_read`.
    fn holders_inside(
        &self,
        ty: &TypeName,
        window: &Window,
        scan: &Scan,
        after: Option<&RecordKey>,
        n: u32,
    ) -> Result<Option<HolderBatch>> {
        let _ = (ty, window, scan, after, n);
        Ok(None)
    }

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

    /// The quarantined records `filter` asks for, by their type and key as
    /// stored, in id order, at most its limit
    /// ([`crate::quarantine::QuarantineFilter::answer`]).
    fn quarantined(
        &self,
        filter: &crate::quarantine::QuarantineFilter,
    ) -> Result<Vec<crate::quarantine::Quarantined>>;

    /// A page of proposals newest first (lakelands#1063): of `filter`'s
    /// state alone, proposed before `before`, at most `filter.limit`; a
    /// proposal's position is its own. `None` where the engine keeps no
    /// such read, and the store reads [`Self::proposals`] whole.
    fn proposals_before(
        &self,
        _: &ProposalFilter,
        _: Option<Position>,
    ) -> Result<Option<Vec<Proposal>>> {
        Ok(None)
    }

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

    /// The ids of the bases no record's state names, ascending: what a
    /// sweep deletes in chunks ([`StatesOf::Bases`]). An engine that keeps
    /// no base apart lists none.
    fn unused_bases(&self) -> Result<Vec<u64>> {
        Ok(Vec::new())
    }

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

    /// The positions from `from` through `through` whose events are of
    /// `source`, by what [`ProjectionStore::mark_applied`] kept of them, in
    /// order and at most `limit` (0 is read as 1), decided as
    /// [`Self::matching`] decides them. An engine that keeps no source in
    /// its index answers that it decides nothing, and the store reads the
    /// log.
    fn of_source(
        &self,
        _source: &SourceId,
        _from: Position,
        _through: Position,
        _limit: u32,
    ) -> Result<Matched> {
        Ok(Matched {
            positions: Vec::new(),
            through: None,
        })
    }
}

/// The engine's vector index, beside the projection: its write side and
/// its size. Vectors are indexed at the head, in the fold's transaction,
/// and read by [`LakeEngine::ranked`]. An engine may index a state's
/// vectors as it puts the state; a put or a remove after it holds over
/// what the state holds, until the record's next state.
pub trait VectorIndex: Send + Sync {
    /// Index a record's vector for one field, replacing the one it held.
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
    /// a writer appends for its own open is decided from the tip it reads
    /// under its lease.
    ///
    /// [`open`]: EngineOpener::open
    fn log_ends(&self, location: &Location) -> Result<Option<(CanonicalEvent, CanonicalEvent)>>;

    /// Open the lake at `location`. Opening never takes the lease: a
    /// writer takes it afterwards with `take_lease`, and a reader opened
    /// [`OpenMode::Read`] never can.
    fn open(&self, location: &Location, mode: OpenMode) -> Result<Box<dyn LakeEngine>>;

    /// Open the lake at `location` to build its projection beside the one
    /// it answers from, into a scratch
    /// (`docs/decisions/071-a-rebuild-builds-beside-the-projection.md`).
    /// The handle reads the log as one opened [`OpenMode::Read`] does and
    /// never takes the lease or appends; its projection, view, rankings
    /// and vector index are the scratch's. The projection the lake answers
    /// from is not opened, whatever layout made it. A scratch marked built
    /// for this lake and this build's layout, still folded through its mark,
    /// whose mark the log holds, is opened as it is, so a fold continues
    /// from its mark; any other is removed and made anew, empty. Refused as
    /// unavailable while another handle has the scratch open. An engine
    /// that keeps no scratch refuses as unbound, as this default does.
    fn open_scratch(&self, location: &Location) -> Result<Box<dyn LakeEngine>> {
        let _ = location;
        Err(crate::Error::unbound(crate::error::Seam::Operation {
            op: "open_scratch".to_owned(),
        }))
    }

    /// How many bytes the projection the lake at `location` answers from
    /// takes where it is kept, which a scratch built beside it takes about
    /// as much of again
    /// (`docs/decisions/071-a-rebuild-builds-beside-the-projection.md`).
    /// Measured without opening the projection or waiting on a handle that
    /// holds it: an engine that cannot measure it so may answer what its
    /// catalogue last recorded. Zero where it keeps no projection yet. An
    /// engine that keeps no scratch refuses as unbound, as this default
    /// does.
    fn room(&self, location: &Location) -> Result<u64> {
        let _ = location;
        Err(crate::Error::unbound(crate::error::Seam::Operation {
            op: "room".to_owned(),
        }))
    }
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

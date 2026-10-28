//! The seams beside the engine: how Lakelands reaches anything outside
//! its deterministic core.
//!
//! Ten seams, this roster and no other: the engine seam
//! ([`crate::engine`]), [`PrincipalResolver`], [`FunctionBacking`],
//! [`EmbedBackend`], [`Reranker`], [`VerbExecutor`], [`ChangeFeed`],
//! [`SystemVocabulary`], [`KeyStore`] and [`Meter`]. Every seam has a
//! default, so a host that plugs in nothing still has a working store. Whatever must reproduce from the
//! log stays inside; whatever needs the outside world is here, and its
//! output is recorded in the log and read back on replay
//! (`docs/decisions/002-determinism-is-the-boundary.md`). Invariants —
//! grants, provenance, the `sys.` guard, idempotency — are never a seam's
//! to keep. `docs/seams.md` specifies each one.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::engine::EngineOpener;
use crate::error::{Case, Place, Refusal, Result, Said};
use crate::event::Event;
use crate::id::{KeyId, LakeId, Position, RecordKey, SourceId, TypeName, VerbName};
use crate::ontology::TypeDecl;
use crate::principal::{Principal, WriteContext};
use crate::record::{Patch, Record, Value};

/// What a caller presents to a door, before it is a principal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Credential {
    /// An API key.
    ApiKey(String),
    /// A bearer token from the host's identity provider.
    Bearer(String),
    /// The operating-system user running a local door (the CLI).
    LocalUser(String),
    /// A principal named by its id, for whom another host forwards a
    /// call over the link between hosts. The server has checked the
    /// host's own key and that the host may speak for the principal; the
    /// resolver resolves the id as one of its own principals, and the
    /// principal's grants judge the call
    /// (`docs/decisions/076-a-lake-keeps-one-writer-and-a-dataset-that-outgrows-it-becomes-several-lakes.md`).
    Forwarded {
        /// The forwarding host, as the server's configuration names it.
        host: String,
        /// The principal's id.
        principal: String,
    },
}

/// Turns a host's identities, roles and policies into one [`Principal`]
/// per call (`docs/decisions/006-grants-not-roles.md`). The default
/// resolves an API key or a static configuration entry to a principal.
#[async_trait]
pub trait PrincipalResolver: Send + Sync {
    /// The principal a credential stands for in one lake. Refused when
    /// the credential is unknown or holds nothing in that lake.
    async fn resolve(&self, lake: &LakeId, credential: &Credential) -> Result<Principal>;
}

/// One call of a function, as a backing receives it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    /// The lake.
    pub lake: LakeId,
    /// The function's declared name.
    pub function: String,
    /// Its inputs.
    pub inputs: Value,
    /// Its declared configuration for this backing.
    #[serde(default)]
    pub config: Value,
    /// The write that set it off. It is no part of the call: the same
    /// inputs are the same call, whoever caused them. Its recorded output
    /// fills every record that makes it, but for a function above `low`,
    /// whose call is its record's
    /// (`docs/decisions/064-an-outside-call-runs-by-its-declared-risk.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<CallCause>,
}

/// The write that made a function's call due, as its event names itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallCause {
    /// The event's position: the write, the `declared` of a declaration's
    /// sweep, or the `recompute`.
    pub position: Position,
    /// Its writer, `<kind>:<id>`.
    pub writer: String,
    /// The record the call is made for: its type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// And its key.
    pub key: RecordKey,
    /// The source, where the write came from one (an ingest's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceId>,
    /// The write's context.
    pub context: WriteContext,
    /// The accept that let the call be made, for a function above `low`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<Accepted>,
}

/// The accept of a proposed call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Accepted {
    /// The position of its `write_accepted`.
    pub position: Position,
    /// Who accepted it, `<kind>:<id>`.
    pub reviewer: String,
}

/// Computes a declared function. The backing kind is an open token:
/// `expr` (built in, evaluated in the store), `http` and `exec` (shipped
/// in `lakelands-backings`), or one a host adds. `expr` is deterministic and recomputed on
/// replay; for every other backing the store records the output as an
/// `OutputRecorded` event and reads it back on replay, never calling the
/// backing twice for one input.
#[async_trait]
pub trait FunctionBacking: Send + Sync {
    /// The backing kind this implementation answers to.
    fn kind(&self) -> &str;

    /// Compute one call's output.
    async fn call(&self, call: &FunctionCall) -> Result<Value>;
}

/// Turns text into vectors for search. Without one, `search` ranks
/// lexically and over vectors callers supply, and says so.
#[async_trait]
pub trait EmbedBackend: Send + Sync {
    /// The model's name, recorded as the procedure of every vector it
    /// makes.
    fn model(&self) -> &str;

    /// The length of the vectors it makes.
    fn dimensions(&self) -> u32;

    /// One vector per text, in order. `Invalid` refuses the request for
    /// what it carries, and the store asks again in halves to find the
    /// text; any other error is weather, recorded for every text of the
    /// batch not answered yet. Any failure reaches the caller of the search that
    /// asked as `Unavailable`, whatever error the backend answered: the
    /// store never falls back to lexical ranking for a backend that is
    /// bound.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// One candidate of a rerank call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RerankCandidate {
    /// The record's key: the host's own, and the reader is answered it anyway.
    pub key: RecordKey,
    /// Its searchable fields the reader reads, in declared order, joined
    /// by a blank line. Nothing the reader may not read is in it.
    pub text: String,
}

/// One call of the second stage, as a reranker receives it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RerankCall {
    /// The lake.
    pub lake: LakeId,
    /// The searched type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// What the candidates are ranked against: the search's text, or the
    /// `text` its `rerank` gave.
    pub query: String,
    /// The first-stage window, in first-stage order.
    pub candidates: Vec<RerankCandidate>,
}

/// Scores a window of a search's candidates against its query, the second
/// stage of a search (`docs/decisions/067-a-search-may-take-a-second-stage.md`).
/// Without one, a search that asks for the stage is refused as `Unbound`.
#[async_trait]
pub trait Reranker: Send + Sync {
    /// The model's name, told in the `rerank` axis of the answer.
    fn model(&self) -> &str;

    /// One score per candidate, in the call's order; higher ranks first.
    /// A call is a read: nothing it answers is recorded. A failure, or a
    /// count of scores other than the candidates', reaches the caller of
    /// the search as `Unavailable`, whatever error the reranker answered:
    /// the store never falls back to the first stage's order.
    async fn rerank(&self, call: &RerankCall) -> Result<Vec<f64>>;
}

/// One verb that reaches outside the lake, as its executor receives it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerbInvocation {
    /// The lake.
    pub lake: LakeId,
    /// The verb.
    pub verb: VerbName,
    /// The record it was called on, as it stood at `position`, where its
    /// call was judged: a later write, a retirement included, does not
    /// reach it. Whole and unmasked: the executor is the host's own code,
    /// not a caller, and receives the record as the store holds it by
    /// design. What it may release is the host's to decide. `None` for a
    /// verb called on its type, which has no record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<Record>,
    /// The payload.
    pub payload: Patch,
    /// The position of the `VerbCalled` (or `WriteAccepted`) event that
    /// made it due; with the lake, the executor's dedup key.
    pub position: Position,
}

/// Carries out verbs whose effect reaches outside the lake. Without an
/// executor bound under a verb's declared name, calling that verb is
/// refused as `Unbound`, never faked. The executor's answer is recorded.
#[async_trait]
pub trait VerbExecutor: Send + Sync {
    /// The executor name verbs declare (`effect: external, executor: …`).
    fn name(&self) -> &str;

    /// Carry out one invocation. Called at most once per position. An
    /// error is recorded as the call's output in its wire form; a call the
    /// host declines by its own policy is answered [`Error::declined`],
    /// never `not_granted` nor `unavailable`, and the host keeps its reason
    /// under the invocation's dedup key.
    ///
    /// [`Error::declined`]: crate::Error::declined
    async fn execute(&self, invocation: &VerbInvocation) -> Result<Value>;
}

/// Which events a feed reader wants.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedFilter {
    /// Only these kinds (as [`crate::event::EventBody::kind`] names them);
    /// all when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    /// Only events about these types; all when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<TypeName>,
    /// Only events about records with these keys (of the named types, or
    /// of any type when `types` is empty); all when empty. An event about
    /// a record is a `record_*` event, `trigger_fired`, `trigger_dropped`,
    /// `object_set_changed` or `verb_called`; no other event passes a key
    /// filter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<crate::id::RecordKey>,
}

/// One batch read from the feed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeedBatch {
    /// The matching events, in log order.
    pub events: Vec<Event>,
    /// The position to poll from next: one past the last position
    /// examined.
    pub next: Position,
    /// The head of the log when the batch was read.
    pub head: Position,
    /// The positions of `events`' reactions an act overruled, in order: a
    /// `trigger_fired`, `trigger_dropped` or `object_set_changed` whose
    /// condition read a value an act erased since, and that the history
    /// without that value does not make
    /// (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`). The
    /// events stay as the log holds them; a consumer that acts on a
    /// reaction passes these by.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overruled: Vec<Position>,
}

/// The most events one feed poll reads from the log and opens, matched or
/// not, whatever its `limit`: those the projection's index names for a
/// filter, and each event past the index's position or of a filter that
/// names nothing. A position the index decides without naming it is not
/// read. A consumer is caught up when a batch's `next` passes `head`.
pub const FEED_EXAMINED: u32 = 4096;

/// The change feed: the log filtered by kind, read from a position
/// (`docs/decisions/004-the-change-feed-is-the-log.md`). The consumer
/// keeps the position it has read up to; the dedup key of a change is its
/// lake and its position. (`cursor` is only ever the page token of a
/// list-shaped read; see `docs/glossary.md`.)
#[async_trait]
pub trait ChangeFeed: Send + Sync {
    /// Events from `from` on that match `filter`, at most `limit`, of at
    /// most [`FEED_EXAMINED`] read. Answers at once, possibly empty; a
    /// consumer that wants to wait polls again.
    async fn poll(&self, from: Position, filter: &FeedFilter, limit: u32) -> Result<FeedBatch>;

    /// Tell the feed a consumer has handled everything before `through`.
    /// The store's own feed keeps no consumer state and accepts this as a
    /// no-op; a feed that relays elsewhere (a remote client, a bus
    /// bridge) may use it to release what it holds.
    async fn ack(&self, consumer: &str, through: Position) -> Result<()>;

    /// The head, once the log is past `after` or once `within` has passed,
    /// whichever comes first: never past the head a `poll` would read, so
    /// a consumer caught up at `next` waits on `next - 1` and polls again.
    /// A feed that cannot wait answers at once, as this default does
    /// (`docs/decisions/004-the-change-feed-is-the-log.md`).
    async fn wait(&self, after: Position, within: std::time::Duration) -> Result<Position> {
        let _ = within;
        Ok(self
            .poll(after.next(), &FeedFilter::default(), 0)
            .await?
            .head)
    }
}

/// The `sys.` types a host adds, and where they may be held
/// (`docs/decisions/003-reservation-by-namespace.md`). Lakelands' own
/// reservation is the four types its fold reads; the default vocabulary
/// adds nothing and allows those four in every lake.
pub trait SystemVocabulary: Send + Sync {
    /// The host's own `sys.` types.
    fn types(&self) -> Vec<TypeDecl>;

    /// Whether a lake may hold a `sys.` type.
    fn may_hold(&self, lake: &LakeId, ty: &TypeName) -> bool;
}

/// A source's data key: 32 bytes for AES-256-GCM-SIV, wiped when dropped,
/// never in the lake (`docs/decisions/058-a-source-may-be-forgotten.md`).
#[derive(Clone)]
pub struct DataKey(pub [u8; 32]);

impl Drop for DataKey {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

impl std::fmt::Debug for DataKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DataKey(..)")
    }
}

/// The host's key store: one data key per source of a lake, in versions,
/// kept outside the lake (`docs/decisions/058-a-source-may-be-forgotten.md`).
/// Synchronous: the writer asks it on the store's blocking threads, at a
/// source's first sealed value of an open lake, and keeps the key in
/// memory while the lake is open.
pub trait KeyStore: Send + Sync {
    /// Make a new version of the source's key and answer its id.
    fn create(&self, lake: &LakeId, source: &SourceId) -> Result<KeyId>;

    /// The key of that version, or of that key [`Self::create_apart`]
    /// made, or none once it is destroyed.
    fn key(&self, lake: &LakeId, source: &SourceId, id: &KeyId) -> Result<Option<DataKey>>;

    /// Destroy every version of the source's key, and every key
    /// [`Self::create_apart`] made of it, and refuse it a new one: a
    /// source whose keys are all destroyed already, or that has none, is
    /// accepted and marked so.
    fn destroy(&self, lake: &LakeId, source: &SourceId) -> Result<()>;

    /// Destroy the version `id` of the source's key and every older one,
    /// never one alone: an output sealed under the oldest version its
    /// call reads then dies with the first of them to go (058). A version
    /// it does not hold is refused. A key store that destroys only whole
    /// sources refuses as unbound, as this default does.
    fn destroy_version(&self, lake: &LakeId, source: &SourceId, id: &KeyId) -> Result<()> {
        let _ = (lake, source, id);
        Err(crate::Error::unbound(crate::error::Seam::KeyVersions))
    }

    /// Whether [`Self::destroy_version`] destroys: a key store that only
    /// destroys whole sources answers false, as this default does, and an
    /// act that takes back part of a source then shreds nothing and
    /// redacts what it takes
    /// (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`).
    fn destroys_versions(&self) -> bool {
        false
    }

    /// The versions of the source's key, oldest first; none destroyed.
    fn ids(&self, lake: &LakeId, source: &SourceId) -> Result<Vec<KeyId>>;

    /// Make a key of the source outside its versions, for one delivery
    /// sealed apart, and answer its id: [`Self::key`] answers it, it is
    /// never among [`Self::ids`], and [`Self::destroy`] destroys it with
    /// the versions
    /// (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`). It
    /// answers once the key is kept as durably as any key. A key store
    /// that makes none refuses as unbound, as this default does.
    fn create_apart(&self, lake: &LakeId, source: &SourceId) -> Result<KeyId> {
        let _ = (lake, source);
        Err(crate::Error::unbound(crate::error::Seam::KeyApart))
    }

    /// [`Self::create_apart`] for each entry, in order, a source named
    /// once per key it needs: a window's new keys in one request. It
    /// answers once they are all kept as durably as any key, so a key
    /// store that keeps keys durably may keep them with one sync; this
    /// default asks [`Self::create_apart`] for each.
    fn create_apart_many(&self, lake: &LakeId, sources: &[SourceId]) -> Result<Vec<KeyId>> {
        (sources.iter())
            .map(|s| self.create_apart(lake, s))
            .collect()
    }

    /// Destroy the key `id` [`Self::create_apart`] made, and no other. One
    /// it does not hold is no change. A key store that makes none refuses
    /// as unbound, as this default does.
    fn destroy_apart(&self, lake: &LakeId, source: &SourceId, id: &KeyId) -> Result<()> {
        let _ = (lake, source, id);
        Err(crate::Error::unbound(crate::error::Seam::KeyApart))
    }
}

/// Where a number was measured: the lake and its engine, and nothing
/// else. No type, field, key, source or principal can be named, so a meter
/// has none to tell a scraper
/// (`docs/decisions/079-a-lake-is-measured-through-a-meter-the-host-binds.md`).
#[derive(Clone, Copy, Debug)]
pub struct Metered<'a> {
    /// The lake.
    pub lake: &'a LakeId,
    /// The engine's name.
    pub engine: &'a str,
}

/// One kind of number: each variant with the name a host exports it
/// under, and every variant in order.
macro_rules! numbers {
    ($(#[$m:meta])* $kind:ident { $($(#[$v:meta])* $variant:ident = $name:literal,)+ }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[non_exhaustive]
        pub enum $kind {
            $($(#[$v])* $variant,)+
        }

        impl $kind {
            /// Every number of the kind, in order.
            pub const ALL: &'static [$kind] = &[$($kind::$variant,)+];

            /// The name a host exports it under.
            pub fn name(self) -> &'static str {
                match self {
                    $($kind::$variant => $name,)+
                }
            }
        }
    };
}

numbers! {
    /// A number that only goes up.
    Count {
        /// Events this handle appended.
        Events = "lakelands_events_appended_total",
        /// Durable appends.
        Appends = "lakelands_appends_total",
        /// Appends the engine answered with an error.
        AppendFailures = "lakelands_append_failures_total",
        /// Record writes decided at the writer, batched or alone, whatever
        /// each answered: appended, refused, replayed or unchanged.
        Writes = "lakelands_writes_total",
        /// Segments sealed, by size or by a checkpoint.
        Seals = "lakelands_seals_total",
    }
}

numbers! {
    /// A number that stands at a value.
    Level {
        /// Entries waiting in the writer's queue.
        Queued = "lakelands_queue_depth",
        /// The log's last position, as the handle knows it.
        Tip = "lakelands_tip",
        /// The position the projection is committed through.
        FoldedThrough = "lakelands_folded_through",
        /// 1 while a rebuild folds, else 0.
        Rebuilding = "lakelands_rebuilding",
        /// The active segment's size in bytes, as a seal by size weighs it.
        ActiveSegmentBytes = "lakelands_active_segment_bytes",
        /// 1 while this handle holds the writer lease, else 0.
        LeaseHeld = "lakelands_lease_held",
        /// The lease's fencing token while it is held, else 0.
        LeaseFence = "lakelands_lease_fence",
    }
}

numbers! {
    /// A number observed many times: a duration in microseconds, or a
    /// batch's size in writes.
    Spread {
        /// The engine's durable append, a checkpoint's included.
        Append = "lakelands_append_seconds",
        /// The fold of an append, its commits included.
        Fold = "lakelands_fold_seconds",
        /// One commit of the projection.
        Commit = "lakelands_commit_seconds",
        /// One catalog read that answers, its view to its answer.
        Read = "lakelands_read_seconds",
        /// Record writes a turn decided together, whatever each answered; a
        /// write alone is one.
        BatchWrites = "lakelands_batch_writes",
    }
}

impl Spread {
    /// Whether it is observed in microseconds; otherwise in writes.
    pub fn micros(self) -> bool {
        self != Spread::BatchWrites
    }
}

/// The host's meter: what the lake measures, told as it happens
/// (`docs/decisions/079-a-lake-is-measured-through-a-meter-the-host-binds.md`).
/// Synchronous, and it returns at once: it is called on the store's
/// blocking threads, inside the writer's turn for the writer's numbers.
/// It records into memory and exports elsewhere. A meter that panics is dropped from that call
/// alone: the lake goes on as if it were not bound. None bound, the lake
/// measures nothing and reads no clock for it.
pub trait Meter: Send + Sync {
    /// `n` went up by `by`.
    fn count(&self, at: Metered<'_>, n: Count, by: u64);

    /// `n` stands at `value`.
    fn level(&self, at: Metered<'_>, n: Level, value: u64);

    /// One observation of `n`, in microseconds or writes ([`Spread::micros`]).
    fn observe(&self, at: Metered<'_>, n: Spread, value: u64);
}

/// The type holding a lake's named settings: `setting('key')` reads the
/// `value` of the live record keyed `key`.
pub const SETTING_TYPE: &str = "sys.setting";

/// The prefix of the name a view's kept records bear in an engine's
/// projection, `sys:kept.<view>`. It holds `:`, which no type name may,
/// so no declared type bears such a name. A kept record is put again at
/// every write to its group, so its key may hold thousands of states, and
/// an engine reads such a name by statements of its own: it tells one by
/// this prefix and by nothing looser (`docs/seams.md` §`ProjectionStore`).
pub const KEPT_PREFIX: &str = "sys:kept.";

/// The name the kept records of the view `view` are put and read under.
pub fn kept_name(view: &TypeName) -> TypeName {
    format!("{KEPT_PREFIX}{view}").into()
}

/// Whether records of this name are a view's kept records.
pub fn is_kept_name(ty: &TypeName) -> bool {
    ty.as_str().starts_with(KEPT_PREFIX)
}

/// The prefix of the name under which the fold keeps, for a view it keeps,
/// the order of each extreme's members, `sys:order.<view>`: an entry for
/// each member, keyed so that the first entry after a key is the next
/// holder. Such a name is read at the head only, and an engine keeps one
/// state for each of its keys (`docs/seams.md` §`ProjectionStore`).
pub const ORDER_PREFIX: &str = "sys:order.";

/// The name the extremes' orders of the view `view` are kept under.
pub fn order_name(view: &TypeName) -> TypeName {
    format!("{ORDER_PREFIX}{view}").into()
}

/// Whether an engine keeps one state for each key of this name, with no
/// row beside it: a put replaces the key's state, and an end deletes it.
pub fn is_head_only(ty: &TypeName) -> bool {
    ty.as_str().starts_with(ORDER_PREFIX)
}

/// The declaration of [`SETTING_TYPE`], which every lake holds from its
/// creation (`docs/seams.md` §`SystemVocabulary`). One definition, so every
/// implementation of the contract creates a lake the same way.
pub fn setting_type() -> TypeDecl {
    use crate::ontology::{FieldDecl, StorageKind};
    let field = |name: &str, kind| FieldDecl {
        required: true,
        ..FieldDecl::of(name, kind)
    };
    TypeDecl {
        key: vec!["key".into()],
        fields: vec![
            field("key", StorageKind::Text),
            field("value", StorageKind::Json),
            // The group the store stamps from the setting's declaration,
            // which a grant's `only` names (063, extended).
            FieldDecl::of("group", StorageKind::Text),
        ],
        field_meta: true,
        description: Some(
            "One named setting of the lake: `setting('key')` reads its value.".into(),
        ),
        ..TypeDecl::named(SETTING_TYPE)
    }
}

/// The key prefix of the setting that asks for a type's order index: the
/// setting `sort_index.<type>`, whose value is a JSON array of the type's
/// field names (`docs/decisions/041-a-sorted-page-reads-its-page.md`).
pub const SORT_INDEX_SETTING: &str = "sort_index.";

/// Whether an order index can keep a field of this kind: the scalar
/// kinds. JSON values, vectors, geometries and bytes are ordered by
/// sorting.
pub fn sortable(kind: &crate::ontology::StorageKind) -> bool {
    use crate::ontology::StorageKind as K;
    matches!(
        kind,
        K::Text | K::Number | K::Decimal | K::Boolean | K::DateTime | K::Date | K::Reference
    )
}

/// The key prefix of the setting that asks for a type's exact-text index
/// over more fields than its declarations look up by: the setting
/// `lookup_index.<type>`, whose value is a JSON array of the type's text
/// fields. A field the index keeps no rows for is found by a scan of the
/// type (`docs/decisions/057-an-exact-text-index-is-kept-where-it-is-looked-up.md`).
pub const LOOKUP_INDEX_SETTING: &str = "lookup_index.";

/// The key prefix of the setting that asks for a type's vector index: the
/// setting `vector_index.<type>`, whose value is a JSON array of the
/// type's vector fields. An indexed field's search answers the index's
/// set; on an engine that keeps no such index it is the exact ranking.
pub const VECTOR_INDEX_SETTING: &str = "vector_index.";

/// Whether a vector index can keep a field of this kind: vectors only.
pub fn indexable(kind: &crate::ontology::StorageKind) -> bool {
    matches!(kind, crate::ontology::StorageKind::Vector)
}

/// The key prefix of the setting that weighs a type's searches: the
/// setting `search_weights.<type>`, whose value is an object of
/// `fields`, vector fields of the type with their weights, `text_fields`,
/// searchable text fields with theirs, and `text_weight`, each optional
/// and one at least. A search that gives a ranking no weight takes the
/// setting's (lakelands#561); a text's ranking weighs a match by the
/// field it is in (lakelands#1230).
pub const SEARCH_WEIGHTS_SETTING: &str = "search_weights.";

/// The weights a `search_weights.<type>` setting of this value gives: a
/// vector field's, then the text's. A weight a search could not give gives
/// none: the setting is checked when it is written, and a log that came
/// another way (an import) holds no other weight for it.
pub fn search_weights(value: Option<&Value>) -> (BTreeMap<String, f64>, Option<f64>) {
    let text = value.and_then(|v| v.get("text_weight")?.as_f64());
    let text = text.filter(|w| crate::query::weight_fits(*w));
    (weighed(value, "fields"), text)
}

/// The weight of a match of the text in each searchable field a
/// `search_weights.<type>` setting of this value names in `text_fields`,
/// as [`search_weights`] reads them (lakelands#1230).
pub fn text_field_weights(value: Option<&Value>) -> BTreeMap<String, f64> {
    weighed(value, "text_fields")
}

/// The fields `member` of a `search_weights` value names, each with a
/// weight a search could give.
fn weighed(value: Option<&Value>, member: &str) -> BTreeMap<String, f64> {
    let fields = value.and_then(|v| v.get(member)?.as_object());
    let weight = |w: &Value| w.as_f64().filter(|w| crate::query::weight_fits(*w));
    (fields.into_iter().flatten())
        .filter_map(|(f, w)| Some((f.clone(), weight(w)?)))
        .collect()
}

/// The key of the setting that seals a source's values of each period
/// under a version of its key of their own: `key_period` for the lake,
/// `key_period.<source>` for a source, `day`, `week` or `month`
/// (`docs/decisions/058-a-source-may-be-forgotten.md`).
pub const KEY_PERIOD_SETTING: &str = "key_period";

/// The key prefix of the setting that asks for a view's records to be
/// kept as its sources are written: the setting `view_kept.<type>`, which
/// holds when its value is `true`.
pub const VIEW_KEPT_SETTING: &str = "view_kept.";

/// Whether a `view_kept.<type>` setting of this value holds.
pub fn view_kept(value: Option<&Value>) -> bool {
    value == Some(&Value::Bool(true))
}

/// Check a write of the setting `key` that leaves `value`, one of the
/// settings the store reads itself; any other setting passes. `declared`
/// answers a type's declaration at the head, and `shown` the type as
/// `describe_type` answers it to the writer: the judge tells a writer
/// nothing its description does not, so a type or a field it is not shown
/// is answered as one that is not declared. A refusal names the member it
/// is about (a named field, else `key` or `value`) as its field, and why,
/// said at the setting.
///
/// - `sort_index.<type>`, `vector_index.<type>` and `lookup_index.<type>`
///   name a declared stored type and hold an array of the names of fields
///   it declares with a kind that index keeps ([`sortable`],
///   [`indexable`], text) and not computed at the read; `None` or `null`
///   asks for no index. `sort_index` may name a declared view too: the
///   index is kept of its kept records.
/// - `search_weights.<type>` names a declared stored type and holds an
///   object of `fields` (each a vector field it declares, with a weight),
///   `text_fields` (each a searchable field it declares, with a weight)
///   and `text_weight`, one at least, every weight one a search may give
///   ([`crate::query::weight_fits`]); `None` or `null` weighs nothing.
/// - `view_kept.<type>` names a declared view and holds `true` or
///   `false`; under `true` the view must be one that can be kept
///   ([`crate::expr::keepable`]), and the refusal says why it is not to a
///   writer shown the view as declared, and to no other.
/// - `key_period` and `key_period.<source>`, a source id, hold `day`,
///   `week` or `month`; `None` or `null` names no period.
///
/// It lives in the contract, not in the core; the store is its one
/// caller.
pub fn check_setting<'a>(
    key: &str,
    value: Option<&Value>,
    declared: &dyn Fn(&crate::TypeName) -> Option<&'a TypeDecl>,
    shown: &dyn Fn(&crate::TypeName) -> Option<crate::describe::TypeDescription>,
) -> Result<()> {
    let at = || Place::Setting {
        key: key.to_owned(),
    };
    let refused = |member: &str, said: Said| {
        let reason = Refusal::Invalid(Box::new(said.within(at())));
        let field = Some(member.into());
        Err(crate::Error::refused(field, reason))
    };
    let known = |ty: &str| Some((shown(&ty.into())?, declared(&ty.into())?));
    if let Some(prefix) = crate::settings::unlisted(key) {
        return refused(
            "key",
            Case::SettingUnlisted {
                prefix: prefix.into(),
            }
            .into(),
        );
    }
    if let Some(rest) =
        (key.strip_prefix(KEY_PERIOD_SETTING)).filter(|r| r.is_empty() || r.starts_with('.'))
    {
        if rest
            .strip_prefix('.')
            .is_some_and(|s| s.is_empty() || s.contains('\0'))
        {
            return refused("key", Case::SourceIdForm.into());
        }
        let named = |p: &str| crate::Timestamp(0).period_start(p).is_some();
        return match value {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(p)) if named(p) => Ok(()),
            Some(_) => refused("value", Case::SettingKeyPeriod.into()),
        };
    }
    if let Some(ty) = key.strip_prefix(VIEW_KEPT_SETTING) {
        let Some((seen, view)) = known(ty).and_then(|(s, d)| Some((s, d.view.as_deref()?))) else {
            return refused("key", Case::NoView { ty: ty.into() }.into());
        };
        return match value {
            Some(Value::Bool(true)) => match crate::expr::keepable(view, declared) {
                Ok(()) => Ok(()),
                Err(why) if seen.view.is_some() => refused("key", why),
                Err(_) => refused("key", Case::ViewNotKeepable { ty: ty.into() }.into()),
            },
            None | Some(Value::Null | Value::Bool(false)) => Ok(()),
            Some(_) => refused("value", Case::SettingKeptHolds.into()),
        };
    }
    if let Some(ty) = key.strip_prefix(SEARCH_WEIGHTS_SETTING) {
        let Some((seen, _)) = known(ty).filter(|(_, d)| d.view.is_none()) else {
            return refused("key", Case::NoStoredType { ty: ty.into() }.into());
        };
        let shape = || refused("value", Case::SettingWeightsShape.into());
        let weighs = |member: &str, w: &Value, case: Case| match w.as_f64() {
            Some(w) if crate::query::weight_fits(w) => Ok(()),
            _ => refused(member, case.into()),
        };
        let members = match value {
            None | Some(Value::Null) => return Ok(()),
            Some(Value::Object(o)) if !o.is_empty() => o,
            Some(_) => return shape(),
        };
        for (member, w) in members {
            match (member.as_str(), w.as_object()) {
                ("text_weight", _) => weighs(member, w, Case::TextWeightRange)?,
                ("fields" | "text_fields", Some(fields)) if !fields.is_empty() => {
                    let text = member == "text_fields";
                    for (name, w) in fields {
                        let field = crate::id::FieldName::from(name.as_str());
                        let fault = match seen.fields.iter().find(|f| f.name == field) {
                            None => Case::UnknownField {
                                ty: ty.into(),
                                field,
                            },
                            Some(f) if text && !f.searchable => Case::NotSearchable {
                                ty: ty.into(),
                                field,
                            },
                            Some(f) if !text && !indexable(&f.ty.kind) => {
                                Case::NotVectorField { field }
                            }
                            Some(_) => {
                                weighs(name, w, Case::WeightRange { field })?;
                                continue;
                            }
                        };
                        return refused(name, fault.into());
                    }
                }
                _ => return shape(),
            }
        }
        return Ok(());
    }
    let text = |k: &crate::ontology::StorageKind| *k == crate::ontology::StorageKind::Text;
    let (ty, (order, lookup), keeps): (_, _, &dyn Fn(&crate::ontology::StorageKind) -> bool) =
        if let Some(ty) = key.strip_prefix(SORT_INDEX_SETTING) {
            (ty, (true, false), &sortable)
        } else if let Some(ty) = key.strip_prefix(VECTOR_INDEX_SETTING) {
            (ty, (false, false), &indexable)
        } else if let Some(ty) = key.strip_prefix(LOOKUP_INDEX_SETTING) {
            (ty, (false, true), &text)
        } else {
            return Ok(());
        };
    // An order index keeps a view's rows too; a vector index only a stored
    // type's.
    let Some((seen, _)) = known(ty).filter(|(_, d)| order || d.view.is_none()) else {
        let ty = ty.into();
        let case = match order {
            true => Case::NoStoredTypeOrView { ty },
            false => Case::NoStoredType { ty },
        };
        return refused("key", case.into());
    };
    let names = match value {
        None | Some(Value::Null) => return Ok(()),
        Some(Value::Array(names)) => names,
        Some(_) => return refused("value", Case::SettingIndexShape { ty: ty.into() }.into()),
    };
    for name in names {
        let Some(name) = name.as_str() else {
            let value = name.clone();
            return refused("value", Case::SettingFieldNames { value }.into());
        };
        let field = crate::id::FieldName::from(name);
        let fault = match seen.fields.iter().find(|f| f.name == field) {
            None => Case::UnknownField {
                ty: ty.into(),
                field,
            },
            Some(f) if !keeps(&f.ty.kind) => {
                let kind = f.ty.kind;
                match (order, lookup) {
                    (true, _) => Case::OrderIndexKind { field, kind },
                    (_, true) => Case::LookupIndexKind { field, kind },
                    _ => Case::VectorIndexKind { field, kind },
                }
            }
            // As it is shown: one told nothing of how a field is computed
            // is not told so here either.
            Some(f) if f.computed_at == Some(crate::ontology::ComputedAt::Read) => {
                match (order, lookup) {
                    (true, _) => Case::OrderIndexAtRead { field },
                    (_, true) => Case::LookupIndexAtRead { field },
                    _ => Case::VectorIndexAtRead { field },
                }
            }
            Some(_) => continue,
        };
        return refused(name, fault.into());
    }
    Ok(())
}

/// The system type whose records are a lake's aging rules
/// (`docs/decisions/036-aging-runs-by-date-once.md`).
pub const AGING_RULE_TYPE: &str = "sys.aging_rule";

/// The declaration of [`AGING_RULE_TYPE`], which every lake holds beside
/// [`SETTING_TYPE`] (`docs/seams.md` §`SystemVocabulary`): keyed by the
/// rule's `name`, moving `field` of the records of `on` from `from` to `to`
/// when `when` holds on the date aged.
pub fn aging_rule_type() -> TypeDecl {
    use crate::ontology::{FieldDecl, StorageKind};
    let field = |name: &str, kind, required| FieldDecl {
        required,
        ..FieldDecl::of(name, kind)
    };
    TypeDecl {
        key: vec!["name".into()],
        fields: vec![
            field("name", StorageKind::Text, true),
            field("on", StorageKind::Text, true),
            field("field", StorageKind::Text, true),
            field("from", StorageKind::Json, true),
            field("to", StorageKind::Json, true),
            field("when", StorageKind::Text, true),
            field("description", StorageKind::Text, false),
        ],
        field_meta: true,
        description: Some(
            "One aging rule: on a date, move `field` of every record of `on` from `from` to \
             `to` when `when` holds."
                .into(),
        ),
        ..TypeDecl::named(AGING_RULE_TYPE)
    }
}

/// The system types Lakelands itself reserves, because its own fold reads
/// them.
pub const RESERVED_SYSTEM_TYPES: [&str; 4] = [
    "sys.source",
    "sys.provenance",
    "sys.setting",
    "sys.aging_rule",
];

/// Everything a host plugs into a store when it opens a lake. Each slot
/// left empty takes its default. The principal resolver is the doors',
/// not the store's: it sits in the server configuration.
#[derive(Clone, Default)]
pub struct Seams {
    /// The engines the store may open a lake with. None is built in: a
    /// location whose engine no opener here serves is refused as
    /// `Unbound`.
    pub engines: Vec<Arc<dyn EngineOpener>>,
    /// Function backings beyond `expr`.
    pub backings: Vec<Arc<dyn FunctionBacking>>,
    /// The embedding backend, if any.
    pub embed: Option<Arc<dyn EmbedBackend>>,
    /// The reranker, if any.
    pub rerank: Option<Arc<dyn Reranker>>,
    /// Executors for external verbs, by name.
    pub executors: Vec<Arc<dyn VerbExecutor>>,
    /// The host's system vocabulary, if it adds one.
    pub vocabulary: Option<Arc<dyn SystemVocabulary>>,
    /// The key store a source's erasable values are sealed with, if any:
    /// without one such a value is refused as `Unbound`.
    pub keys: Option<Arc<dyn KeyStore>>,
    /// The meter, if any: without one the lake measures nothing.
    pub meter: Option<Arc<dyn Meter>>,
}

impl std::fmt::Debug for Seams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Seams")
            .field(
                "engines",
                &self.engines.iter().map(|e| e.name()).collect::<Vec<_>>(),
            )
            .field(
                "backings",
                &self.backings.iter().map(|b| b.kind()).collect::<Vec<_>>(),
            )
            .field("embed", &self.embed.as_ref().map(|e| e.model().to_owned()))
            .field(
                "rerank",
                &self.rerank.as_ref().map(|r| r.model().to_owned()),
            )
            .field(
                "executors",
                &self.executors.iter().map(|e| e.name()).collect::<Vec<_>>(),
            )
            .field("vocabulary", &self.vocabulary.is_some())
            .field("keys", &self.keys.is_some())
            .field("meter", &self.meter.is_some())
            .finish()
    }
}

/// Compile-time proof that every seam is `dyn`-compatible: one argument a
/// seam.
#[allow(dead_code, clippy::too_many_arguments)]
fn seams_are_dyn_compatible(
    _: &dyn PrincipalResolver,
    _: &dyn FunctionBacking,
    _: &dyn EmbedBackend,
    _: &dyn Reranker,
    _: &dyn VerbExecutor,
    _: &dyn ChangeFeed,
    _: &dyn SystemVocabulary,
    _: &dyn KeyStore,
    _: &dyn Meter,
) {
}

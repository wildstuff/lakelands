//! The seams beside the engine: how Lakelands reaches anything outside
//! its deterministic core.
//!
//! Seven seams, this roster and no other: the engine seam
//! ([`crate::engine`]), [`PrincipalResolver`], [`FunctionBacking`],
//! [`EmbedBackend`], [`VerbExecutor`], [`ChangeFeed`] and
//! [`SystemVocabulary`]. Every seam has a default, so a host that plugs
//! in nothing still has a working store. Whatever must reproduce from the
//! log stays inside; whatever needs the outside world is here, and its
//! output is recorded in the log and read back on replay
//! (`docs/decisions/002-determinism-is-the-boundary.md`). Invariants —
//! grants, provenance, the `sys.` guard, idempotency — are never a seam's
//! to keep. `docs/seams.md` specifies each one.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::engine::EngineOpener;
use crate::error::Result;
use crate::event::Event;
use crate::id::{LakeId, Position, TypeName, VerbName};
use crate::ontology::TypeDecl;
use crate::principal::Principal;
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

    /// One vector per text, in order. Any failure reaches the caller of
    /// the search that asked as `Unavailable`, whatever error the
    /// backend answered: the store never falls back to lexical ranking
    /// for a backend that is bound.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// One verb that reaches outside the lake, as its executor receives it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerbInvocation {
    /// The lake.
    pub lake: LakeId,
    /// The verb.
    pub verb: VerbName,
    /// The record it was called on, whole and unmasked: the executor is
    /// the host's own code, not a caller, and receives the record as the
    /// store holds it by design. What it may release is the host's to
    /// decide.
    pub record: Record,
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

    /// Carry out one invocation. Called at most once per position.
    async fn execute(&self, invocation: &VerbInvocation) -> Result<Value>;
}

/// Which events a feed reader wants.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// a record is a `record_*` event, `trigger_fired`, `object_set_changed`
    /// or `verb_called`; no other event passes a key filter.
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
}

/// The change feed: the log filtered by kind, read from a position
/// (`docs/decisions/004-the-change-feed-is-the-log.md`). The consumer
/// keeps the position it has read up to; the dedup key of a change is its
/// lake and its position. (`cursor` is only ever the page token of a
/// list-shaped read; see `docs/glossary.md`.)
#[async_trait]
pub trait ChangeFeed: Send + Sync {
    /// Events from `from` on that match `filter`, at most `limit`.
    /// Answers at once, possibly empty; a consumer that wants to wait
    /// polls again.
    async fn poll(&self, from: Position, filter: &FeedFilter, limit: u32) -> Result<FeedBatch>;

    /// Tell the feed a consumer has handled everything before `through`.
    /// The store's own feed keeps no consumer state and accepts this as a
    /// no-op; a feed that relays elsewhere (a remote client, a bus
    /// bridge) may use it to release what it holds.
    async fn ack(&self, consumer: &str, through: Position) -> Result<()>;
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

/// The type holding a lake's named settings: `setting('key')` reads the
/// `value` of the live record keyed `key`.
pub const SETTING_TYPE: &str = "sys.setting";

/// The declaration of [`SETTING_TYPE`], which every lake holds from its
/// creation (`docs/seams.md` §`SystemVocabulary`). One definition, so every
/// implementation of the contract creates a lake the same way.
pub fn setting_type() -> TypeDecl {
    use crate::ontology::{Authority, FieldDecl, FieldType, StorageKind};
    let field = |name: &str, kind| FieldDecl {
        name: name.into(),
        ty: FieldType::kind(kind),
        required: true,
        authority: Authority::Authored,
        derived: None,
        function: None,
        depends_on: vec![],
        on_input_change: Default::default(),
        sensitive: false,
        searchable: false,
        description: None,
        label: None,
        embed: None,
    };
    TypeDecl {
        name: SETTING_TYPE.into(),
        key: vec!["key".into()],
        fields: vec![
            field("key", StorageKind::Text),
            field("value", StorageKind::Json),
        ],
        relations: vec![],
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: Some(
            "One named setting of the lake: `setting('key')` reads its value.".into(),
        ),
        label: None,
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
        K::Text | K::Number | K::Decimal | K::Boolean | K::DateTime | K::Reference
    )
}

/// Check a write of the setting `key` that leaves `value` (`None` or
/// `null` asks for no index): a `sort_index.<type>` setting must name a
/// declared type (`decl`, its declaration at the head) and hold an array
/// of the names of fields it declares with a [`sortable`] kind. A refusal
/// names the field it is about (the named field, else `key` or `value`)
/// and why. Any other setting passes.
pub fn check_sort_index(
    key: &str,
    value: Option<&Value>,
    decl: Option<&TypeDecl>,
) -> std::result::Result<(), (crate::id::FieldName, String)> {
    let Some(ty) = key.strip_prefix(SORT_INDEX_SETTING) else {
        return Ok(());
    };
    let Some(decl) = decl.filter(|d| d.view.is_none()) else {
        return Err((
            "key".into(),
            format!("`{key}` asks for an order index of `{ty}`, which is no declared stored type"),
        ));
    };
    let names = match value {
        None | Some(Value::Null) => return Ok(()),
        Some(Value::Array(names)) => names,
        Some(_) => {
            return Err((
                "value".into(),
                format!("`{key}` holds an array of field names of `{ty}`, or null for no index"),
            ));
        }
    };
    for name in names {
        let Some(name) = name.as_str() else {
            return Err((
                "value".into(),
                format!("`{key}` holds field names, and {name} is not one"),
            ));
        };
        match decl.fields.iter().find(|f| f.name.as_str() == name) {
            None => {
                return Err((
                    name.into(),
                    format!("`{key}` names `{name}`, which `{ty}` does not declare"),
                ));
            }
            Some(f) if !sortable(&f.ty.kind) => {
                return Err((
                    name.into(),
                    format!(
                        "`{key}` names `{name}`, of kind {}; an order index keeps text, \
                         number, decimal, boolean, date-time and reference fields",
                        serde_json::to_value(f.ty.kind).unwrap_or_default()
                    ),
                ));
            }
            Some(_) => {}
        }
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
    use crate::ontology::{Authority, FieldDecl, FieldType, StorageKind};
    let field = |name: &str, kind, required| FieldDecl {
        name: name.into(),
        ty: FieldType::kind(kind),
        required,
        authority: Authority::Authored,
        derived: None,
        function: None,
        depends_on: vec![],
        on_input_change: Default::default(),
        sensitive: false,
        searchable: false,
        description: None,
        label: None,
        embed: None,
    };
    TypeDecl {
        name: AGING_RULE_TYPE.into(),
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
        relations: vec![],
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: Some(
            "One aging rule: on a date, move `field` of every record of `on` from `from` to \
             `to` when `when` holds."
                .into(),
        ),
        label: None,
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
    /// Executors for external verbs, by name.
    pub executors: Vec<Arc<dyn VerbExecutor>>,
    /// The host's system vocabulary, if it adds one.
    pub vocabulary: Option<Arc<dyn SystemVocabulary>>,
}

impl std::fmt::Debug for Seams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Seams")
            .field(
                "engines",
                &self
                    .engines
                    .iter()
                    .map(|e| e.name().to_owned())
                    .collect::<Vec<_>>(),
            )
            .field(
                "backings",
                &self
                    .backings
                    .iter()
                    .map(|b| b.kind().to_owned())
                    .collect::<Vec<_>>(),
            )
            .field("embed", &self.embed.as_ref().map(|e| e.model().to_owned()))
            .field(
                "executors",
                &self
                    .executors
                    .iter()
                    .map(|e| e.name().to_owned())
                    .collect::<Vec<_>>(),
            )
            .field("vocabulary", &self.vocabulary.is_some())
            .finish()
    }
}

/// Compile-time proof that every seam is `dyn`-compatible.
#[allow(dead_code)]
fn seams_are_dyn_compatible(
    _: &dyn PrincipalResolver,
    _: &dyn FunctionBacking,
    _: &dyn EmbedBackend,
    _: &dyn VerbExecutor,
    _: &dyn ChangeFeed,
    _: &dyn SystemVocabulary,
) {
}

//! Writes and what they answer: authored writes, proposals, schema
//! changes and the preflight a migration must pass.

use serde::{Deserialize, Serialize};

use crate::expr::Condition;
use crate::id::{
    Cursor, FieldName, IdempotencyKey, Position, ProposalId, RecordKey, RelationName, SourceId,
    TriggerName, TypeName, ValueTypeName, VerbName, Version,
};
use crate::ontology::{
    ANNOTATION_KEY_MAX_BYTES, ANNOTATIONS_MAX_BYTES, Annotations, FieldDecl, FieldType,
    RESERVED_ANNOTATION_NAMESPACES, TriggerDecl, TypeDecl, ValueTypeDecl, VerbDecl,
};

use crate::record::Patch;
use crate::time::Timestamp;

/// Options every authored write takes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteOpts {
    /// Refuse the write unless the record is at this version
    /// (compare-and-set). For a record that must not exist yet, pass
    /// `Version(0)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<Version>,
    /// Make a retry of this write a no-op: a second write with the same
    /// key from the same principal answers the first write's outcome and
    /// appends nothing. The key is recorded in the log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<IdempotencyKey>,
}

/// What a write did. A writer never has to know in advance whether its
/// write runs or waits for review; the answer says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// The write took effect.
    Applied {
        /// The position it is visible from.
        position: Position,
        /// The record's new version, for a record write.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<Version>,
        /// For a verb whose effect reaches outside the lake: its executor's
        /// recorded output, `{"error": "<text>"}` when the executor failed.
        /// The position is real either way; the verb is not run again for
        /// it. `None` for every other write.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<serde_json::Value>,
        /// What the write set off beyond the event it is visible from.
        #[serde(flatten)]
        set_off: SetOff,
    },
    /// The write was recorded as a proposal and waits for review.
    Proposed {
        /// The position of the `WriteProposed` event.
        position: Position,
        /// The proposal to accept or reject.
        proposal: ProposalId,
        /// Why it was not applied at once (the verb's declared risk, a
        /// policy of the lake).
        why: String,
    },
}

/// What a write set off: the reactions in its own append, and an external
/// verb's recorded output. Empty, and left out of the answer, when it set
/// off nothing, so such an answer reads as it always did
/// (`docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`,
/// extended).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetOff {
    /// The last position of what the write set off, when that lies after
    /// `position`: a read at or after it sees all of it. An external
    /// verb's output is recorded in a later append, so other writers'
    /// events may lie before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through: Option<Position>,
    /// The triggers it fired, in the order fired: only those `describe`
    /// lists to the caller.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fired: Vec<TriggerName>,
    /// The object sets its record joined, as `describe` lists them to the
    /// caller. For a set's declaration, the set, once, when it took
    /// records in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joined: Vec<String>,
    /// The object sets its record left, likewise; for a set's
    /// declaration, the set, once, when it let records go.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left: Vec<String>,
}

impl SetOff {
    /// Whether it names a trigger or an object set.
    pub fn names_any(&self) -> bool {
        !(self.fired.is_empty() && self.joined.is_empty() && self.left.is_empty())
    }
}

impl Outcome {
    /// The position the outcome was recorded at.
    pub fn position(&self) -> Position {
        match self {
            Outcome::Applied { position, .. } | Outcome::Proposed { position, .. } => *position,
        }
    }
}

/// A write that can be proposed instead of applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "write", rename_all = "snake_case")]
pub enum ProposedWrite {
    /// An `upsert`.
    Upsert {
        /// The record's type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The record's key.
        key: RecordKey,
        /// The fields it carries.
        patch: Patch,
        /// Its options.
        #[serde(default)]
        opts: WriteOpts,
    },
    /// A `retire`.
    Retire {
        /// The record's type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The record's key.
        key: RecordKey,
        /// Its options.
        #[serde(default)]
        opts: WriteOpts,
    },
    /// A verb call.
    Call {
        /// The verb.
        verb: VerbName,
        /// The record it is called on; absent for a verb called on its
        /// type (log format 8).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<RecordKey>,
        /// Its payload.
        payload: Patch,
        /// Its options.
        #[serde(default)]
        opts: WriteOpts,
    },
    /// Declarations, applied together as `apply` applies them (log format
    /// version 4).
    Declare {
        /// The declarations.
        declarations: Vec<Declaration>,
    },
    /// A type's migration, as `migrate_apply` applies it, with the report
    /// its preflight answered when it was proposed (log format version 4).
    Migrate {
        /// The migration.
        migration: Box<Migration>,
        /// The preflight's report at the proposal, for the reviewer. The
        /// store answers it; whatever a proposer sends is replaced.
        #[serde(default)]
        report: Box<MigrationReport>,
    },
}

impl ProposedWrite {
    /// Which kind of write it is.
    pub fn kind(&self) -> ProposalKind {
        match self {
            ProposedWrite::Upsert { .. } => ProposalKind::Upsert,
            ProposedWrite::Retire { .. } => ProposalKind::Retire,
            ProposedWrite::Call { .. } => ProposalKind::Call,
            ProposedWrite::Declare { .. } => ProposalKind::Declare,
            ProposedWrite::Migrate { .. } => ProposalKind::Migrate,
        }
    }

    /// Whether it changes the ontology: accepting it needs the `schema`
    /// grant as well as `review`.
    pub fn is_schema(&self) -> bool {
        matches!(
            self,
            ProposedWrite::Declare { .. } | ProposedWrite::Migrate { .. }
        )
    }
}

/// The kinds of write a proposal carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    /// An `upsert`.
    Upsert,
    /// A `retire`.
    Retire,
    /// A verb call.
    Call,
    /// Declarations.
    Declare,
    /// A migration.
    Migrate,
}

/// Where a proposal stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalState {
    /// Waiting for review.
    Pending,
    /// Accepted; its write was applied.
    Accepted,
    /// Rejected; nothing was written.
    Rejected,
}

/// A proposal, as the log recorded it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    /// Its identifier.
    pub id: ProposalId,
    /// The position of its `WriteProposed` event.
    pub position: Position,
    /// Who proposed it, as `<kind>:<id>`.
    pub proposer: String,
    /// When.
    pub proposed_at: Timestamp,
    /// What it would write.
    pub write: ProposedWrite,
    /// Why it waits.
    pub why: String,
    /// Where it stands.
    pub state: ProposalState,
    /// Who decided it, and the reason given, once decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    /// The reason given on rejection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Which proposals to list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProposalFilter {
    /// Only proposals in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ProposalState>,
    /// Only proposals touching this type.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub ty: Option<TypeName>,
    /// Only proposals of this kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ProposalKind>,
    /// At most this many.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Order: newest first unless stated (`proposed_at`, `position`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order_by: Vec<crate::read::Order>,
    /// Continue after an earlier page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// The time axes.
    #[serde(default, flatten)]
    pub at: crate::read::ReadAt,
}

/// An object set: the records of one type matching a condition. When a
/// write moves a record into or out of the set, the store appends
/// `ObjectSetChanged` in the same append.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectSetDecl {
    /// The set's name.
    pub name: String,
    /// The type of its members.
    pub of: TypeName,
    /// Which records are members.
    #[serde(rename = "where")]
    pub filter: Condition,
    /// What the set is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One declaration of a lake's ontology. It is read strictly: a member it
/// does not take, at any depth, is refused, naming the member and where it
/// sits (`docs/decisions/028-authoring-is-strict-and-whole.md`, extended).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "declare", rename_all = "snake_case")]
pub enum Declaration {
    /// A type, new or revised without reshaping stored records.
    Type(TypeDecl),
    /// A value type.
    ValueType(ValueTypeDecl),
    /// A verb.
    Verb(VerbDecl),
    /// A trigger.
    Trigger(TriggerDecl),
    /// An object set.
    ObjectSet(ObjectSetDecl),
    /// A function a backing computes.
    Function(FunctionDecl),
}

/// Why a declaration's annotations cannot be declared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnnotationFault {
    /// A key in a reserved namespace (`sys.`, `lakelands.`).
    Reserved {
        /// The key.
        key: String,
        /// Why, for a person: the declaration and where the key sits.
        message: String,
    },
    /// A key without a namespace or too long, a `null` value, an integer
    /// beyond ±(2^53 − 1), or more than [`ANNOTATIONS_MAX_BYTES`] in all.
    Invalid {
        /// Why, for a person: the declaration and where.
        message: String,
    },
}

impl Declaration {
    /// The declaration's kind and name, for a person
    /// (`the type declaration \`invoice\``).
    pub fn what(&self) -> String {
        let (kind, name) = match self {
            Declaration::Type(t) => ("type", t.name.as_str()),
            Declaration::ValueType(v) => ("value_type", v.name.as_str()),
            Declaration::Verb(v) => ("verb", v.name.as_str()),
            Declaration::Trigger(t) => ("trigger", t.name.as_str()),
            Declaration::ObjectSet(o) => ("object_set", o.name.as_str()),
            Declaration::Function(f) => ("function", f.name.as_str()),
        };
        format!("the {kind} declaration `{name}`")
    }

    /// Every annotation map the declaration holds, with where it sits
    /// (empty for the declaration's own): a type's, its fields', its
    /// relations' and their link fields', a verb's payload fields', a
    /// function's inputs'.
    pub fn annotation_maps(&self) -> Vec<(String, &Annotations)> {
        fn fields<'a>(at: &str, fs: &'a [FieldDecl], out: &mut Vec<(String, &'a Annotations)>) {
            for (i, f) in fs.iter().enumerate() {
                out.push((format!("{at}[{i}]"), &f.annotations));
            }
        }
        let mut out = Vec::new();
        match self {
            Declaration::Type(t) => {
                out.push((String::new(), &t.annotations));
                fields("fields", &t.fields, &mut out);
                for (i, r) in t.relations.iter().enumerate() {
                    out.push((format!("relations[{i}]"), &r.annotations));
                    fields(&format!("relations[{i}].fields"), &r.fields, &mut out);
                }
            }
            Declaration::Verb(v) => fields("payload", &v.payload, &mut out),
            Declaration::Function(f) => fields("inputs", &f.inputs, &mut out),
            Declaration::ValueType(_) | Declaration::Trigger(_) | Declaration::ObjectSet(_) => {}
        }
        out
    }

    /// Why its annotations cannot be declared, if they cannot: a key is
    /// two or more ASCII identifiers joined by `.`, at most
    /// [`ANNOTATION_KEY_MAX_BYTES`], outside the reserved namespaces; a
    /// value is any JSON but `null`, its integers within ±(2^53 − 1); and
    /// all the maps together hold at most [`ANNOTATIONS_MAX_BYTES`] of
    /// canonical bytes.
    pub fn annotation_fault(&self) -> Option<AnnotationFault> {
        let what = self.what();
        let mut bytes = 0;
        for (at, map) in self.annotation_maps() {
            if map.is_empty() {
                continue;
            }
            let at = if at.is_empty() {
                String::new()
            } else {
                format!(" at `{at}`")
            };
            for (key, value) in map {
                let segments: Vec<&str> = key.split('.').collect();
                if RESERVED_ANNOTATION_NAMESPACES.contains(&segments[0]) {
                    return Some(AnnotationFault::Reserved {
                        key: key.clone(),
                        message: format!(
                            "{what} has the annotation `{key}`{at} in the reserved namespace \
                             `{}.`",
                            segments[0]
                        ),
                    });
                }
                let invalid = |why: String| {
                    Some(AnnotationFault::Invalid {
                        message: format!("{what} has the annotation `{key}`{at}: {why}"),
                    })
                };
                if segments.len() < 2 || !segments.iter().all(|s| is_identifier(s)) {
                    return invalid(
                        "a key is a namespace and a name, ASCII identifiers joined by `.` \
                         (`acme.icon`)"
                            .into(),
                    );
                }
                if key.len() > ANNOTATION_KEY_MAX_BYTES {
                    return invalid(format!("a key is at most {ANNOTATION_KEY_MAX_BYTES} bytes"));
                }
                if value.is_null() {
                    return invalid(
                        "a value is not `null`; an annotation is taken away by leaving its key \
                         out"
                        .into(),
                    );
                }
                if let Some(n) = too_large(value) {
                    return invalid(format!(
                        "{n} is beyond ±(2^53 − 1), the largest integer every reader reads exactly"
                    ));
                }
            }
            bytes += crate::canonical::of(map).len();
        }
        (bytes > ANNOTATIONS_MAX_BYTES).then(|| AnnotationFault::Invalid {
            message: format!(
                "{what} holds {bytes} bytes of annotations, over the {ANNOTATIONS_MAX_BYTES} a \
                 declaration may hold"
            ),
        })
    }
}

impl From<AnnotationFault> for crate::error::Error {
    fn from(f: AnnotationFault) -> crate::error::Error {
        use crate::error::{Error, Refusal};
        match f {
            AnnotationFault::Reserved { key, .. } => Error::Refused {
                field: Some(key.into()),
                reason: Refusal::Reserved,
            },
            AnnotationFault::Invalid { message } => Error::Refused {
                field: None,
                reason: Refusal::Invalid { message },
            },
        }
    }
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// An integer anywhere in a value beyond ±(2^53 − 1), if there is one.
fn too_large(value: &serde_json::Value) -> Option<String> {
    const MAX: u64 = (1 << 53) - 1;
    match value {
        serde_json::Value::Number(n) => (n.as_u64().is_some_and(|u| u > MAX)
            || n.as_i64().is_some_and(|i| i.unsigned_abs() > MAX))
        .then(|| n.to_string()),
        serde_json::Value::Array(items) => items.iter().find_map(too_large),
        serde_json::Value::Object(m) => m.values().find_map(too_large),
        _ => None,
    }
}

const DECLARATION_KINDS: &[&str] = &[
    "type",
    "value_type",
    "verb",
    "trigger",
    "object_set",
    "function",
];

// Read by hand, not derived: a derived internally tagged enum buffers the
// declaration before it reads it, and a member refused inside the buffer
// cannot say where it sits.
impl<'de> Deserialize<'de> for Declaration {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Declaration, D::Error> {
        use serde::de::Error as _;
        let mut members = serde_json::Map::<String, serde_json::Value>::deserialize(d)?;
        let kind = match members.remove("declare") {
            Some(serde_json::Value::String(kind)) => kind,
            Some(_) => return Err(D::Error::custom("`declare` is not a string")),
            None => return Err(D::Error::missing_field("declare")),
        };
        let name = members
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let members = serde_json::Value::Object(members);
        let read = match kind.as_str() {
            "type" => serde_path_to_error::deserialize(members).map(Declaration::Type),
            "value_type" => serde_path_to_error::deserialize(members).map(Declaration::ValueType),
            "verb" => serde_path_to_error::deserialize(members).map(Declaration::Verb),
            "trigger" => serde_path_to_error::deserialize(members).map(Declaration::Trigger),
            "object_set" => serde_path_to_error::deserialize(members).map(Declaration::ObjectSet),
            "function" => serde_path_to_error::deserialize(members).map(Declaration::Function),
            other => return Err(D::Error::unknown_variant(other, DECLARATION_KINDS)),
        };
        read.map_err(|e| D::Error::custom(refusal(&kind, name.as_deref(), &e)))
    }
}

/// The sentence a declaration that cannot be read is refused with: the
/// declaration, then the member it does not take and where it sits, or
/// what else is wrong and where.
fn refusal(
    kind: &str,
    name: Option<&str>,
    e: &serde_path_to_error::Error<serde_json::Error>,
) -> String {
    let what = match name {
        Some(name) => format!("the {kind} declaration `{name}`"),
        None => format!("a {kind} declaration"),
    };
    let path = e.path().to_string();
    let inner = e.inner().to_string();
    if let Some((member, rest)) = inner
        .strip_prefix("unknown field `")
        .and_then(|rest| rest.split_once('`'))
    {
        let parent = path
            .strip_suffix(member)
            .map_or("", |p| p.trim_end_matches('.'));
        let at = if parent.is_empty() {
            String::new()
        } else {
            format!(" at `{parent}`")
        };
        let takes = rest
            .strip_prefix(", expected ")
            .map(|t| format!(" (it takes {t})"))
            .unwrap_or_default();
        return format!("{what} has no member `{member}`{at}{takes}");
    }
    if path == "." {
        format!("{what}: {inner}")
    } else {
        format!("{what}: at `{path}`: {inner}")
    }
}

/// A function: a named computation a [`FunctionBacking`](crate::seam::FunctionBacking)
/// performs. The backing kind is an open token; `expr` is evaluated in the
/// store from `expr`, any other kind is called through its seam and its
/// output recorded (`docs/decisions/002-determinism-is-the-boundary.md`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionDecl {
    /// The function's name.
    pub name: String,
    /// The backing kind that computes it (`expr`, `http`, `exec`, or a
    /// host's own).
    pub backing: String,
    /// Its inputs, typed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<crate::ontology::FieldDecl>,
    /// For the `expr` backing: the expression, over the inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr: Option<Condition>,
    /// The backing's own configuration.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub config: serde_json::Value,
    /// What the function computes, for people and agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A change of a type's shape that stored records must follow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Migration {
    /// The type's new declaration.
    pub to: TypeDecl,
    /// Fields renamed: old name to new name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renames: Vec<(FieldName, FieldName)>,
}

/// The preflight of a migration: what it would do to the stored records,
/// computed before anything changes. A migration is applied only when its
/// report is lossless, and only by a call that names the report it was
/// shown. The empty report (`Default`) is what a proposer sends with a
/// proposed migration, which the store replaces with the preflight's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationReport {
    /// The type migrated.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The schema hash before.
    pub from_schema_hash: String,
    /// The schema hash after.
    pub to_schema_hash: String,
    /// Records examined.
    pub records_scanned: u64,
    /// Per field whose type changes: how many values convert.
    pub fields: Vec<FieldConversion>,
    /// Fields the migration removes, and how many records hold a value
    /// that removing them discards. Removal is the migration's intent,
    /// named here before anything is applied; it does not make the
    /// report lossy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<(FieldName, u64)>,
    /// Per type a re-key moves: its records, and the new keys that
    /// collide or cannot be made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rekeyed: Vec<Rekeyed>,
    /// Per reference field, in any type, the values the re-key rewrites.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<ReferenceRewrite>,
    /// The first new keys more than one record would take, at most ten.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_collisions: Vec<RecordRef>,
    /// True when every stored value converts without loss, and every
    /// record a re-key moves takes a new key of its own.
    pub lossless: bool,
    /// Names this report; `migrate_apply` must repeat it.
    pub report_id: String,
}

/// How the values of one field convert to its new type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldConversion {
    /// The field.
    pub field: FieldName,
    /// Its type before.
    pub from: FieldType,
    /// Its type after.
    pub to: FieldType,
    /// Values that convert exactly.
    pub convertible: u64,
    /// Values that would lose information.
    pub not_convertible: u64,
    /// Up to five keys of records that would not convert. Never values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_keys: Vec<RecordKey>,
}

/// The acknowledgement of a batch `ingest` committed: one per
/// group-commit window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Committed {
    /// The position of the window's last event; everything in the
    /// window is visible from here.
    pub position: Position,
    /// How many records the window committed, duplicates excluded.
    pub records: u32,
    /// How many records were already in the log and appended nothing.
    pub duplicates: u32,
}

/// What `LakeStore::age` did for one date
/// (`docs/decisions/036-aging-runs-by-date-once.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgeReport {
    /// The date aged, at UTC midnight.
    pub date: crate::time::Timestamp,
    /// Every live rule, in name order, and what it did.
    pub rules: Vec<RuleRun>,
    /// The records moved, over all rules.
    pub transitions: u32,
    /// The conditions that could not be evaluated, over all rules.
    pub failed_guards: u32,
    /// The head after the run: the last append's position, or the head as
    /// it stood when nothing was appended.
    pub position: Position,
}

/// What one aging rule did for one date.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleRun {
    /// The rule's name.
    pub rule: String,
    /// The rule record's version, the procedure version of its moves.
    pub version: Version,
    /// The records it moved.
    pub moved: u32,
    /// The records whose condition could not be evaluated, left as they
    /// were.
    pub failed_guards: u32,
    /// Why the rule was not run, when it no longer fits the lake (its
    /// type or field gone or changed, its values or its condition no
    /// longer valid). A skipped rule moves nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// What `LakeStore::purge_source` retired, or what
/// `LakeStore::purge_preflight` says it would
/// (`docs/decisions/039-a-source-is-retracted-as-one.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgeReport {
    /// The source purged.
    pub source: SourceId,
    /// The records retired, over every type.
    pub retired: u32,
    /// Per type, in name order, the records retired in it; types with
    /// none are left out.
    pub types: Vec<TypeRetired>,
    /// The types that keep no per-field provenance, which a purge cannot
    /// read and does not search, in name order.
    pub unsearched: Vec<TypeName>,
    /// The head after the append, or the head as it stood when nothing
    /// was appended.
    pub position: Position,
    /// Per relation, in type then relation order, the live records outside
    /// the purge that refer to a record inside it; relations with none
    /// are left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<PurgeReference>,
    /// Names a preflight's report; `purge_source` may repeat it. Empty on
    /// a purge's own report.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub report_id: String,
}

/// The records outside a purge that refer into it over one relation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgeReference {
    /// The type that declares the relation.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The relation.
    pub relation: RelationName,
    /// How many live records refer into the purge over it.
    pub records: u64,
    /// The first of them in key order, at most ten: the referring records
    /// of a `via` relation, the link records of one carried by links.
    pub first: Vec<RecordRef>,
}

/// The records a purge retired in one type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeRetired {
    /// The type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// How many of its records were retired.
    pub retired: u32,
}

/// The preflight of a value type's migration: what its new declaration
/// would refuse among the values stored under it, computed before
/// anything changes (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueTypeReport {
    /// The value type migrated.
    pub value_type: ValueTypeName,
    /// Its version in force.
    pub from_version: u32,
    /// Stored records examined, over every type that uses it, retired
    /// and restricted ones included.
    pub records_scanned: u64,
    /// Per field that uses it: the values examined and those refused.
    pub fields: Vec<ValueTypeUse>,
    /// Values the new declaration would refuse, over every field.
    pub misfits: u64,
    /// The first misfits, at most ten. Never values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub first: Vec<Misfit>,
    /// True when every stored value fits the new declaration.
    pub fits: bool,
    /// Names this report; `migrate_value_type_apply` must repeat it.
    pub report_id: String,
}

/// One field that uses a value type, as a value-type preflight found it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueTypeUse {
    /// The type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The field.
    pub field: FieldName,
    /// Values stored in it.
    pub values_scanned: u64,
    /// Values the new declaration would refuse.
    pub misfits: u64,
}

/// A stored value a new declaration would refuse: where it is, never what.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Misfit {
    /// The record's type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The record's key.
    pub key: RecordKey,
    /// The field.
    pub field: FieldName,
}

/// One record key a re-keying migration moves (`migrated.rekeyed`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rekey {
    /// The record's type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// Its key before.
    pub from: RecordKey,
    /// Its key after.
    pub to: RecordKey,
}

/// What a re-key does to one type's records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rekeyed {
    /// The type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// Records whose key changes, retired and restricted included.
    pub records: u64,
    /// Records whose new key another record would take too.
    pub collisions: u64,
    /// Records a new key cannot be made for: a key field has no value.
    pub missing: u64,
}

/// The values of one reference field a re-key rewrites to new keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceRewrite {
    /// The type holding the field.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The field.
    pub field: FieldName,
    /// Values rewritten.
    pub values: u64,
}

/// A record named by its type and key; never its values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordRef {
    /// The type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The key.
    pub key: RecordKey,
}

/// What `LakeStore::compact` did to the projection's storage
/// (`docs/decisions/040-maintenance-never-changes-an-answer.md`). The
/// sizes are the engine's own measure, not a promise: the projection's
/// file and its journal on SQLite, its tables on PostgreSQL, none on the
/// memory engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactReport {
    /// The projection's storage in bytes before, if the engine measures it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<u64>,
    /// The projection's storage in bytes after, if the engine measures it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
    /// The lake's head, which a compact never moves.
    pub head: Position,
}

#[cfg(test)]
mod tests;

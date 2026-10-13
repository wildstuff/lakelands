//! Writes and what they answer: authored writes, proposals, schema
//! changes and the preflight a migration must pass.

use serde::{Deserialize, Serialize};

use crate::error::{Case, Place, Said};
use crate::expr::Condition;
use crate::id::{
    Cursor, FieldName, IdempotencyKey, Position, ProposalId, RecordKey, RelationName, SourceId,
    TriggerName, TypeName, ValueTypeName, VerbName, Version,
};
use crate::ontology::{
    ANNOTATION_KEY_MAX_BYTES, ANNOTATIONS_MAX_BYTES, Annotations, FieldDecl, FieldType,
    RESERVED_ANNOTATION_NAMESPACES, RelationDecl, StorageKind, TriggerDecl, TypeDecl,
    ValueTypeDecl, VerbDecl,
};

use crate::record::Patch;
use crate::time::Timestamp;

/// Options every authored write takes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteOpts {
    /// Refuse the write unless the record is at this version
    /// (compare-and-set). For a record that must not exist yet, pass
    /// `Version(0)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<Version>,
    /// Make a retry of this write a no-op: a second write with the same
    /// key from the same principal answers the first write's outcome and
    /// appends nothing. For a call of a verb that reaches outside the
    /// lake, that outcome carries its executor's recorded answer; while
    /// none is recorded the retry is refused as `unanswered`. The key is
    /// recorded in the log.
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
        /// it. `None` for every other write; an answer of `null` is
        /// `Some(Null)`, and read back so.
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        output: Option<serde_json::Value>,
        /// What the write set off beyond the event it is visible from.
        #[serde(flatten)]
        set_off: SetOff,
        /// For a migration: each field it renamed, old name and new, as the
        /// one answered may know of them. A grant by name restricts a field
        /// under every name it has borne; a host that grants a renamed
        /// field more names the new one (`docs/hosting.md`).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        renamed: Vec<(FieldName, FieldName)>,
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

/// A member that is there, `null` included: an executor's answer of `null`
/// is an answer, and only an absent `output` is none.
pub(crate) fn present<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<serde_json::Value>, D::Error> {
    serde::Deserialize::deserialize(d).map(Some)
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
#[serde(deny_unknown_fields, tag = "write", rename_all = "snake_case")]
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
        /// type.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<RecordKey>,
        /// Its payload.
        payload: Patch,
        /// Its options.
        #[serde(default)]
        opts: WriteOpts,
    },
    /// Declarations, applied together as `apply` applies them.
    Declare {
        /// The declarations.
        declarations: Vec<Declaration>,
    },
    /// A type's migration, as `migrate_apply` applies it, with the report
    /// its preflight answered when it was proposed.
    Migrate {
        /// The migration.
        migration: Box<Migration>,
        /// The preflight's report at the proposal, for the reviewer. The
        /// store answers it; whatever a proposer sends is replaced.
        #[serde(default)]
        report: Box<MigrationReport>,
    },
    /// A value type's migration, as `migrate_value_type_apply` applies it,
    /// with the report its preflight answered when it was proposed.
    MigrateValueType {
        /// The new declaration.
        to: Box<ValueTypeDecl>,
        /// The values it renames.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        renames: Vec<(String, String)>,
        /// The preflight's report at the proposal, for the reviewer. The
        /// store answers it; whatever a proposer sends is replaced.
        #[serde(default)]
        report: Box<ValueTypeReport>,
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
            ProposedWrite::MigrateValueType { .. } => ProposalKind::MigrateValueType,
        }
    }

    /// Whether it changes the ontology: accepting it needs the `schema`
    /// grant as well as `review`.
    pub fn is_schema(&self) -> bool {
        matches!(
            self,
            ProposedWrite::Declare { .. }
                | ProposedWrite::Migrate { .. }
                | ProposedWrite::MigrateValueType { .. }
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
    /// A value type's tightening.
    MigrateValueType,
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
    /// What it would write, as far as its reader may see it: absent in a
    /// listing for a reader who may see nothing of the write. A stored
    /// proposal always holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<ProposedWrite>,
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
    /// Whether the proposal holds more than this reader is shown: a
    /// listing says so, without a name. A stored proposal holds `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub more: bool,
}

/// Which proposals to list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

/// Why a declaration's annotations cannot be declared, said at the
/// declaration, where the map sits in it and the annotation.
#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationFault {
    /// A key in a reserved namespace (`sys.`, `lakelands.`).
    Reserved {
        /// The key.
        key: String,
        /// Its case, said where the key sits.
        said: Said,
    },
    /// A key without a namespace or too long, a `null` value, an integer
    /// beyond ±(2^53 − 1), or more than [`ANNOTATIONS_MAX_BYTES`] in all.
    Invalid {
        /// Its case, said where the annotation sits.
        said: Said,
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

    /// The declaration as the place a case is said at: `type `invoice``.
    pub fn place(&self) -> Place {
        match self {
            Declaration::Type(t) => Place::Type {
                name: t.name.clone(),
            },
            Declaration::ValueType(v) => Place::ValueType {
                name: v.name.clone(),
            },
            Declaration::Verb(v) => Place::Verb {
                name: v.name.clone(),
            },
            Declaration::Trigger(t) => Place::Trigger {
                name: t.name.clone(),
            },
            Declaration::ObjectSet(o) => Place::ObjectSet {
                name: o.name.clone(),
            },
            Declaration::Function(f) => Place::Function {
                name: f.name.clone(),
            },
        }
    }

    /// Every annotation map the declaration holds, with where it sits
    /// (empty for the declaration's own): a type's, its fields', its
    /// relations' and their link fields', a value type's, a verb's payload
    /// fields', a function's inputs'.
    pub fn annotation_maps(&self) -> Vec<(String, &Annotations)> {
        let maps = self.annotated().into_iter();
        maps.map(|(at, _, map)| (at, map)).collect()
    }

    /// Every annotation map with where it sits, as a path and as the
    /// places inside the declaration.
    fn annotated(&self) -> Vec<(String, Vec<Place>, &Annotations)> {
        type Out<'a> = Vec<(String, Vec<Place>, &'a Annotations)>;
        fn fields<'a>(at: &str, within: &[Place], fs: &'a [FieldDecl], out: &mut Out<'a>) {
            for (i, f) in fs.iter().enumerate() {
                let field = Place::Field {
                    name: f.name.clone(),
                };
                let places = [within, &[field]].concat();
                out.push((format!("{at}[{i}]"), places, &f.annotations));
            }
        }
        let mut out = Vec::new();
        match self {
            Declaration::Type(t) => {
                out.push((String::new(), Vec::new(), &t.annotations));
                fields("fields", &[], &t.fields, &mut out);
                for (i, r) in t.relations.iter().enumerate() {
                    let relation = [Place::Relation {
                        name: r.name.clone(),
                    }];
                    out.push((format!("relations[{i}]"), relation.to_vec(), &r.annotations));
                    fields(
                        &format!("relations[{i}].fields"),
                        &relation,
                        &r.fields,
                        &mut out,
                    );
                }
            }
            Declaration::Verb(v) => fields("payload", &[], &v.payload, &mut out),
            Declaration::Function(f) => fields("inputs", &[], &f.inputs, &mut out),
            Declaration::ValueType(v) => out.push((String::new(), Vec::new(), &v.annotations)),
            Declaration::Trigger(_) | Declaration::ObjectSet(_) => {}
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
        let mut bytes = 0;
        for (_, places, map) in self.annotated() {
            if map.is_empty() {
                continue;
            }
            for (key, value) in map {
                // The declaration, where the map sits, the annotation.
                let said = |case: Case| {
                    let mut said = Said::from(case).within(Place::Annotation { key: key.clone() });
                    for place in places.iter().rev() {
                        said = said.within(place.clone());
                    }
                    said.within(self.place())
                };
                let segments: Vec<&str> = key.split('.').collect();
                if RESERVED_ANNOTATION_NAMESPACES.contains(&segments[0]) {
                    let namespace = segments[0].to_owned();
                    let said = said(Case::AnnotationReserved { namespace });
                    let key = key.clone();
                    return Some(AnnotationFault::Reserved { key, said });
                }
                let invalid = |case| Some(AnnotationFault::Invalid { said: said(case) });
                if segments.len() < 2 || !segments.iter().all(|s| is_identifier(s)) {
                    return invalid(Case::AnnotationKeyForm);
                }
                if key.len() > ANNOTATION_KEY_MAX_BYTES {
                    let max = ANNOTATION_KEY_MAX_BYTES;
                    return invalid(Case::AnnotationKeyLong { max });
                }
                if value.is_null() {
                    return invalid(Case::AnnotationNull);
                }
                if let Some(n) = crate::canonical::integer_too_large(value) {
                    return invalid(Case::IntegerTooLarge { n: n.clone() });
                }
            }
            bytes += crate::canonical::of(map).len();
        }
        let max = ANNOTATIONS_MAX_BYTES;
        (bytes > max).then(|| AnnotationFault::Invalid {
            said: Said::from(Case::AnnotationsTooLarge { bytes, max }).within(self.place()),
        })
    }

    /// A declaration is written into the log as it was declared, so it
    /// is judged as a written value is (`docs/log-format.md` §1.2): a
    /// number anywhere in it written as an integer beyond ±(2^53 − 1) is
    /// said at the declaration and the member that holds it, by its path
    /// in the declaration as it is written: a function's `config.limits[0]`,
    /// the duration literal of an expression in typed form at
    /// `where.rhs.value.value`. An expression in text form is judged by
    /// the typed form it parses to, which is the form a schema hash is
    /// taken over, and said at the member that holds the text (`where`).
    pub fn integer_fault(&self) -> Option<Said> {
        use crate::canonical::{integer_too_large, integer_too_large_at};
        let written = serde_json::to_value(self).ok()?;
        let typed = integer_too_large_at(&written).map(|(path, n)| (path, n.clone()));
        let (path, n) = typed.or_else(|| {
            self.conditions().into_iter().find_map(|(path, c)| {
                let Condition::Text(text) = c else {
                    return None;
                };
                let typed = serde_json::to_value(crate::expr::parse(text).ok()?).ok()?;
                Some((path, integer_too_large(&typed)?.clone()))
            })
        })?;
        let said = Said::from(Case::IntegerTooLarge { n });
        Some(said.within(Place::Member { path }).within(self.place()))
    }

    /// Every expression the declaration holds, in either form, each with
    /// the path of the member that holds it in the declaration as it is
    /// written (`view.merge[0].where`).
    pub fn conditions(&self) -> Vec<(String, &Condition)> {
        let mut all: Vec<(String, Option<&Condition>)> = Vec::new();
        match self {
            Declaration::Type(t) => {
                for (i, f) in t.fields.iter().enumerate() {
                    all.push((format!("fields[{i}].derived"), f.derived.as_ref()));
                }
                for (i, r) in t.relations.iter().enumerate() {
                    for (j, f) in r.fields.iter().enumerate() {
                        let path = format!("relations[{i}].fields[{j}].derived");
                        all.push((path, f.derived.as_ref()));
                    }
                }
                let merged = t.view.iter().flat_map(|v| v.merge.iter().enumerate());
                let merged =
                    merged.map(|(i, m)| (format!("view.merge[{i}]"), &m.r#where, &m.measures));
                let own = t
                    .view
                    .iter()
                    .map(|v| ("view".to_owned(), &v.r#where, &v.measures));
                for (source, filter, measures) in own.chain(merged) {
                    all.push((format!("{source}.where"), filter.as_ref()));
                    for (i, m) in measures.iter().enumerate() {
                        all.push((format!("{source}.measures[{i}].expr"), Some(&m.expr)));
                    }
                }
            }
            Declaration::ValueType(v) => all.push(("constraint".into(), v.constraint.as_ref())),
            Declaration::Verb(v) => {
                all.push(("requires".into(), v.requires.as_ref()));
                if let crate::ontology::VerbEffect::Write { set } = &v.effect {
                    for (i, a) in set.iter().enumerate() {
                        all.push((format!("set[{i}].value"), Some(&a.value)));
                    }
                }
            }
            Declaration::Trigger(t) => all.push(("when".into(), t.when.as_ref())),
            Declaration::ObjectSet(s) => all.push(("where".into(), Some(&s.filter))),
            Declaration::Function(f) => all.push(("expr".into(), f.expr.as_ref())),
        }
        let held = all.into_iter().filter_map(|(path, c)| Some((path, c?)));
        held.collect()
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
            AnnotationFault::Invalid { said } => Error::invalid(said),
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

/// What a declaration that cannot be read says, as a parse error: the
/// declaration, and where in it the parser stopped. Which member it does
/// not take is found by the doors from the parse itself, never from these
/// words (lakelands#650).
fn refusal(
    kind: &str,
    name: Option<&str>,
    e: &serde_path_to_error::Error<serde_json::Error>,
) -> String {
    let what = match name {
        Some(name) => format!("the {kind} declaration `{name}`"),
        None => format!("a {kind} declaration"),
    };
    match e.path().to_string().as_str() {
        "." => format!("{what}: {}", e.inner()),
        path => format!("{what}: at `{path}`: {}", e.inner()),
    }
}

/// What `recompute` names: the declaration in force whose fields the lake
/// computes again (`docs/decisions/022-functions-are-declarations.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recompute {
    /// A function, whatever its backing: every type that names it.
    Function(String),
    /// A type with a field computed outside the lake.
    Type(TypeName),
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
    /// Relations renamed: old name to new name. A renamed relation keeps
    /// its edges, however it is carried.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relation_renames: Vec<(RelationName, RelationName)>,
}

impl Migration {
    /// The fields the migration renames, as pairs of the old name and the
    /// new. A pair names a field of `before` and one of the new shape, each
    /// once, and no new name is a field `before` keeps. Otherwise the case
    /// that says why. Judged against an author's view of the type, a field
    /// it may not know of is no field to it (lakelands#750). It lives in
    /// the contract; the store and the core's conversion call it.
    pub fn renamed_fields(&self, before: &TypeDecl) -> Result<Vec<(&FieldName, &FieldName)>, Case> {
        let has = |t: &TypeDecl, n: &FieldName| t.fields.iter().any(|f| &f.name == n);
        let mut out: Vec<(&FieldName, &FieldName)> = Vec::new();
        for (from, to) in &self.renames {
            if !has(before, from) {
                return Err(Case::RenameFromUnknown { from: from.clone() });
            }
            if !has(&self.to, to) {
                return Err(Case::RenameToUnknown { to: to.clone() });
            }
            if out.iter().any(|(f, _)| *f == from) {
                let field = from.clone();
                return Err(Case::RenameRepeated { field });
            }
            if out.iter().any(|(_, t)| *t == to) {
                return Err(Case::RenameRepeated { field: to.clone() });
            }
            out.push((from, to));
        }
        let renamed = |n: &FieldName| out.iter().any(|(f, _)| *f == n);
        let mut onto: Vec<&FieldName> = out.iter().map(|(_, t)| *t).collect();
        onto.sort();
        match onto.into_iter().find(|t| has(before, t) && !renamed(t)) {
            Some(to) => Err(Case::RenameCollides { to: to.clone() }),
            None => Ok(out),
        }
    }

    /// The relations the migration renames, each as it stood on `before`
    /// and as it stands. A pair names a relation of the type before and
    /// one of `to`, each once, and frees the old name. A rename moves edges
    /// and changes none: the carrier, the target, the cardinality and, for
    /// a relation carried by link records, its fields and who writes them
    /// stay. Otherwise the case that says why, said at the rename. It
    /// lives in the contract; the store is its one caller.
    pub fn renamed_relations<'a>(
        &'a self,
        before: &'a TypeDecl,
    ) -> std::result::Result<Vec<(&'a RelationDecl, &'a RelationDecl)>, Said> {
        let mut out = Vec::new();
        for (i, (old, new)) in self.relation_renames.iter().enumerate() {
            let wrong = |case: Case| {
                let (from, to) = (old.clone(), new.clone());
                Said::from(case).within(Place::RelationRename { from, to })
            };
            let of = |t: &'a TypeDecl, n| t.relations.iter().find(|r| &r.name == n);
            let (Some(was), Some(is)) = (of(before, old), of(&self.to, new)) else {
                return Err(wrong(Case::RelationRenameNamesNone));
            };
            let again = |(o, n): &(RelationName, RelationName)| o == old || n == new;
            if self.relation_renames[..i].iter().any(again)
                || of(before, new).is_some()
                || of(&self.to, old).is_some()
            {
                return Err(wrong(Case::RelationRenameNotOnce));
            }
            let kept = |r: &'a RelationDecl| {
                let carried = (&r.via, &r.derived, &r.target, r.cardinality);
                (carried, &r.fields, r.authority)
            };
            if kept(was) != kept(is) {
                return Err(wrong(Case::RelationRenameChanges));
            }
            out.push((was, is));
        }
        Ok(out)
    }
}

impl Migration {
    /// What `to` states otherwise than `before`, the declaration in force,
    /// for [`MigrationReport::restated`]: the two compared member by member
    /// as they serialise, a field with the one it takes its values from, a
    /// relation with the one it is renamed from, a link field as
    /// `relation.field`. It lives in the contract; the store is its one
    /// caller.
    pub fn restated(&self, before: &TypeDecl) -> Vec<Restated> {
        let mut out = Vec::new();
        let own = ["name", "fields", "relations"];
        differing(None, Some(before), &self.to, &own, &mut out);
        differing_fields("", &before.fields, &self.to.fields, &self.renames, &mut out);
        for r in &self.to.relations {
            let renamed = self.relation_renames.iter().find(|(_, new)| new == &r.name);
            let name = renamed.map_or(&r.name, |(old, _)| old);
            let was = before.relations.iter().find(|w| &w.name == name);
            // A rename is said by the report's `relations`, a link field below.
            let (of, said) = (format!("{}.", r.name), ["name", "fields"]);
            differing(Some(r.name.to_string()), was, r, &said, &mut out);
            let held = was.map_or(&[][..], |w| &w.fields);
            differing_fields(&of, held, &r.fields, &[], &mut out);
        }
        out
    }
}

/// Each field of `to` against the one of `was` it takes its values from,
/// by `renames` or by its name, named after `of`. A type that changes and
/// a `required` that tightens are a conversion, said by a report's `fields`.
fn differing_fields(
    of: &str,
    was: &[FieldDecl],
    to: &[FieldDecl],
    renames: &[(FieldName, FieldName)],
    out: &mut Vec<Restated>,
) {
    for f in to {
        let from = match renames.iter().find(|(_, new)| new == &f.name) {
            Some((old, _)) => Some(old),
            None => Some(&f.name).filter(|n| !renames.iter().any(|(old, _)| &old == n)),
        };
        let from = from.and_then(|n| was.iter().find(|w| &w.name == n));
        let retyped = from.is_some_and(|w| w.ty != f.ty).then_some("type");
        let tightened = (f.required && !from.is_some_and(|w| w.required)).then_some("required");
        let said: Vec<&str> = retyped.into_iter().chain(tightened).collect();
        differing(Some(format!("{of}{}", f.name)), from, f, &said, out);
    }
}

/// The members of `to` that differ from those of `was`, as the two
/// serialise, but those `said` elsewhere; of a declaration with no `was`,
/// each member it states otherwise than a bare one, from null.
fn differing<T: Serialize + Default>(
    field: Option<String>,
    was: Option<&T>,
    to: &T,
    said: &[&str],
    out: &mut Vec<Restated>,
) {
    let members = |d: &T| serde_json::to_value(d).unwrap_or_default();
    let (before, to) = (members(was.unwrap_or(&T::default())), members(to));
    let both = [&before, &to].into_iter().filter_map(|d| d.as_object());
    let names: std::collections::BTreeSet<&String> = both.flat_map(|d| d.keys()).collect();
    for member in names.into_iter().filter(|n| !said.contains(&n.as_str())) {
        if before[member] != to[member] {
            out.push(Restated {
                field: field.as_deref().map(FieldName::from),
                member: member.clone(),
                was: was.map_or(serde_json::Value::Null, |_| before[member].clone()),
                to: to[member].clone(),
            });
        }
    }
}

/// How a relation's edges are carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Carrier {
    /// By a reference field of the record, the relation's `via`.
    Via,
    /// By link records, of the relation's link type.
    Links,
    /// By matching values: a derived relation.
    Derived,
}

impl Carrier {
    /// How `r`'s edges are carried.
    pub fn of(r: &RelationDecl) -> Carrier {
        match (&r.via, &r.derived) {
            (Some(_), _) => Carrier::Via,
            (None, Some(_)) => Carrier::Derived,
            (None, None) => Carrier::Links,
        }
    }
}

/// A declaration a migration carries along: one that names a link type
/// whose records move, which the lake declares again under the new name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Carried {
    /// What is declared: `trigger`, `object_set`, `verb` or `type`.
    pub kind: String,
    /// Its name.
    pub name: String,
}

/// One relation a migration renames, in its preflight's report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationRename {
    /// The relation's name before.
    pub from: RelationName,
    /// Its name after.
    pub to: RelationName,
    /// How its edges are carried.
    pub carrier: Carrier,
    /// The link records that move to the new name's link type: none
    /// unless the relation is carried by links.
    pub links: u64,
}

/// One member of a declaration that a migration's `to` states otherwise
/// than the declaration in force, where the report's other lists do not
/// say it: a mark, a label, an expression, a relation's target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restated {
    /// What the member is of: a field, a relation by its name, a link
    /// field as `relation.field`. Left out for the type itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<FieldName>,
    /// The member, as a declaration serialises it.
    pub member: String,
    /// What the declaration in force states, null where it leaves the
    /// member out or declares no such field or relation.
    pub was: serde_json::Value,
    /// What `to` states, null where it leaves the member out.
    pub to: serde_json::Value,
}

/// A value type's migration: its new declaration, and the values it
/// renames in every stored record that holds them.
/// With no rename it changes no record, and is the tightening it was
/// before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueTypeMigration {
    /// The value type's new declaration.
    pub to: ValueTypeDecl,
    /// Values renamed: old value to new value, of a text value type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renames: Vec<(String, String)>,
}

impl From<ValueTypeDecl> for ValueTypeMigration {
    /// A migration that renames no value.
    fn from(to: ValueTypeDecl) -> ValueTypeMigration {
        ValueTypeMigration {
            to,
            renames: Vec::new(),
        }
    }
}

impl ValueTypeMigration {
    /// Refuse, with the case, renames that cannot be applied to `before`, the
    /// declaration in force: of a value type not text; an old value
    /// `before` does not accept, a new one `to` does not, a value renamed
    /// to itself or twice. A merge, a chain and a swap are allowed, unlike
    /// a type's relation renames: each pair maps a value as it stood before.
    /// The store is its one caller; `accepts` is the caller's
    /// check of a value.
    pub fn check_renames(
        &self,
        before: &ValueTypeDecl,
        accepts: impl Fn(&ValueTypeDecl, &str) -> bool,
    ) -> std::result::Result<(), Said> {
        if !self.renames.is_empty() && self.to.kind != StorageKind::Text {
            let value_type = self.to.name.clone();
            return Err(Case::ValueRenameNotText { value_type }.into());
        }
        for (i, (old, new)) in self.renames.iter().enumerate() {
            let wrong = |case: Case| {
                let (from, to) = (old.clone(), new.clone());
                Err(Said::from(case).within(Place::ValueRename { from, to }))
            };
            if old == new {
                return wrong(Case::ValueRenameSame);
            }
            if self.renames[..i].iter().any(|(o, _)| o == old) {
                return wrong(Case::ValueRenameTwice);
            }
            if !accepts(before, old) {
                return wrong(Case::ValueRenameOldRefused);
            }
            if !accepts(&self.to, new) {
                return wrong(Case::ValueRenameNewRefused);
            }
        }
        Ok(())
    }

    /// A stored value as the migration leaves it: renamed, or as it is.
    pub fn renamed<'a>(&'a self, value: &'a str) -> &'a str {
        let pair = self.renames.iter().find(|(old, _)| old == value);
        pair.map_or(value, |(_, new)| new)
    }
}

/// How many stored values one of a value type's renames rewrites in one
/// field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueRenamed {
    /// The old value.
    pub from: String,
    /// The new value.
    pub to: String,
    /// The records whose field holds the old value.
    pub records: u64,
}

/// The preflight of a migration: what it would do to the stored records
/// and to the declaration, computed before anything changes. Every
/// difference between the declaration in force and `to` is in it. A migration is applied only when its
/// report is lossless, and only by a call that names the report it was
/// shown. The empty report (`Default`) is what a proposer sends with a
/// proposed migration, which the store replaces with the preflight's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationReport {
    /// The type migrated. Empty, which no type's name is, in an answer to
    /// a reader who may not see the type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The schema hash before, of the shape the reader is shown, as
    /// `describe` answers it.
    pub from_schema_hash: String,
    /// The schema hash after, of the shape the reader is shown.
    pub to_schema_hash: String,
    /// Records examined.
    pub records_scanned: u64,
    /// Per field whose type changes: how many values convert.
    pub fields: Vec<FieldConversion>,
    /// Fields the migration removes, and how many records hold a value
    /// that removing them discards; and each relation it removes, by the
    /// relation's name, with the number of live links the apply retires,
    /// none for one not carried by link records. Removal is the migration's
    /// intent, named here before anything is applied; it does not make the
    /// report lossy. The apply discards what the counts count: a host that
    /// tests for that reads them, or their sum, [`MigrationReport::discards`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<(FieldName, u64)>,
    /// The relations the migration renames, each with how its edges are
    /// carried and the link records that move with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<RelationRename>,
    /// The declarations that name a link type a renamed relation's
    /// records move from: the lake declares each again under the new name,
    /// in the same append.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub carried: Vec<Carried>,
    /// Per type a re-key moves: its records, and the new keys that
    /// collide or cannot be made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rekeyed: Vec<Rekeyed>,
    /// Per reference field, in any type, the values the re-key rewrites.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<ReferenceRewrite>,
    /// The first new keys more than one record would take, in the order
    /// of the new key and at most ten, among those of which the reader is
    /// answered two takers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_collisions: Vec<RecordRef>,
    /// What `to` states otherwise than the declaration in force, beside
    /// the lists above: each member as the two declarations serialise it,
    /// but a field's type that changes and a `required` that tightens,
    /// which `fields` says. A field or a relation `to` adds is restated
    /// member by member, from null.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restated: Vec<Restated>,
    /// True when every stored value converts without loss, and every
    /// record a re-key moves takes a new key of its own. It speaks of
    /// stored values: a migration that only restates is lossless. It says
    /// nothing of what the migration removes, which is `dropped`.
    pub lossless: bool,
    /// Names this report; `migrate_apply` must repeat it. It is the id of
    /// the report as its reader is shown it and of the head it was made
    /// at: the SHA-256 of `{"report": …, "head": …}` in canonical form,
    /// the report with this member empty.
    pub report_id: String,
    /// Whether the report holds more than this reader is shown, or counts
    /// more than it names: an entry left out, a `not_convertible` larger
    /// than its `sample_keys`, `collisions` beyond `sample_collisions`. A
    /// logged and a stored report hold `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub more: bool,
}

impl MigrationReport {
    /// The stored values and live links the apply discards: the sum of
    /// `dropped`'s counts. Derived, so nothing on the wire carries it.
    pub fn discards(&self) -> u64 {
        self.dropped.iter().map(|(_, n)| n).sum()
    }
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
    /// The one type the purge was of; absent for a purge of the source
    /// whole.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<TypeName>,
    /// The records retired, over every type.
    pub retired: u32,
    /// Per type, in name order, the records retired in it; types with
    /// none are left out.
    pub types: Vec<TypeRetired>,
    /// The types that keep no per-field provenance, which a purge cannot
    /// read and does not search, in name order; empty for a purge of one
    /// type, which is refused instead.
    pub unsearched: Vec<TypeName>,
    /// The head after the append, or the head as it stood when nothing
    /// was appended.
    pub position: Position,
    /// Per relation, in type then relation order, the live records outside
    /// the purge that refer to a record inside it; relations with none
    /// are left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<PurgeReference>,
    /// Per type and field, in that order, the records retired whose field
    /// with a declared `initial` value an author had written: the purge
    /// takes them with what was written there. Fields with none are left
    /// out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moved: Vec<PurgeMoved>,
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

/// The records a purge takes whose one field with a declared `initial`
/// value an author had written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgeMoved {
    /// The type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The field.
    pub field: FieldName,
    /// How many of the purge's records an author wrote it on.
    pub records: u64,
    /// The first of them in key order, at most ten.
    pub first: Vec<RecordKey>,
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
/// The empty report (`Default`) is what a proposer sends with a proposed
/// value type's migration, which the store replaces with the preflight's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Values the new declaration would refuse, as the renames leave them,
    /// over every field, shown to the reader or not.
    pub misfits: u64,
    /// The first misfits of records the reader is answered, at most ten.
    /// Never values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub first: Vec<Misfit>,
    /// True when every stored value fits the new declaration, in every
    /// field, shown to the reader or not.
    pub fits: bool,
    /// The declarations in force that may name an old value the migration
    /// renames in a condition or an expression, each as its kind and name
    /// say it (set `open_tasks`), and the aging rules that move from or
    /// to one. They stay as they are: changing them is the author's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<String>,
    /// Whether a declaration the reader is not shown may name one too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub conditions_unshown: bool,
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
    /// Values the new declaration would refuse: stored ones, and the
    /// field's declared `initial`.
    pub misfits: u64,
    /// Per rename: how many stored values in the field it rewrites.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renamed: Vec<ValueRenamed>,
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

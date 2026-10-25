//! Writes and what they answer: authored writes, proposals, schema
//! changes and the preflight a migration must pass.

use serde::{Deserialize, Serialize};

use crate::error::{Case, Place, Said};
use crate::expr::Condition;
use crate::id::{
    ConstraintName, Cursor, DeliveryId, FieldName, IdempotencyKey, KeyId, Position, ProposalId,
    RecordKey, RelationName, SourceId, TriggerName, TypeName, ValueTypeName, VerbName, Version,
};
use crate::ontology::{
    ANNOTATION_KEY_MAX_BYTES, ANNOTATIONS_MAX_BYTES, Annotations, FieldDecl, FieldType,
    RESERVED_ANNOTATION_NAMESPACES, RelationDecl, Risk, StorageKind, TriggerDecl, TypeDecl,
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

/// Options an `ingest` takes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestOpts {
    /// The delivery its records came in, as the caller names it (a
    /// vendor's batch id); else the store's own, `d<first position>`.
    /// Not part of a record's ingest id: the same content under another
    /// delivery is a duplicate and stays with the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<DeliveryId>,
    /// Seal the call's values of erasable fields under a key of their own
    /// too, one per source, outside the source's versions, so that taking
    /// back the delivery shreds it alone
    /// (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`).
    /// Refused as unbound where the key store makes no such key. Off by
    /// default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sealed_apart: bool,
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
        /// recorded output, the error's wire form when the executor failed.
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
        /// For `declare` and `apply`: each declaration sent, in the order
        /// sent, as it stands in force after the call; so for a `propose`
        /// of declarations all in force, which files nothing, and for the
        /// `accept` of proposed declarations. Empty for every other write.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        declared: Vec<InForce>,
        /// For a change to the ontology: the plan that applied, as the one
        /// answered may know of it (062). `None` for every other write.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<Box<Plan>>,
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
        /// For a proposed change to the ontology: its plan, as the one
        /// answered may know of it (062).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<Box<Plan>>,
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
    /// The triggers it fired, each once, in the order first fired: only
    /// those `describe` lists to the caller.
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
        /// Their plan when they were proposed, as one who reads everything
        /// and holds no `restricted` was shown it; each reader is shown it
        /// as its listing shows a report. The store answers it; whatever a
        /// proposer sends is replaced
        /// (`docs/decisions/062-a-change-plans-before-it-acts.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<Box<Plan>>,
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
    /// One record's call of a function above `low` risk, which the store
    /// proposes when a write makes it due and no caller may propose
    /// (`docs/decisions/064-an-outside-call-runs-by-its-declared-risk.md`).
    Compute {
        /// The record's type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The record.
        key: RecordKey,
        /// The field its output fills.
        field: FieldName,
        /// The function.
        function: String,
        /// The inputs as the record holds them, by name, as a listing
        /// shows them to its reader. The log never holds them: a value
        /// sealed under a source's key would stay readable in the proposal
        /// once the source is forgotten (058).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inputs: Option<serde_json::Value>,
        /// The call's digest on its record, which keys its output: a
        /// decision holds for its record alone (064).
        input: crate::event::Digest,
    },
    /// The calls of a function above `low` risk that a declaration or
    /// `recompute` makes due over a type, proposed as one: its accept makes
    /// the calls the sweep owes then, and its reject records the rejection
    /// under each of them.
    ComputeSweep {
        /// The records' type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The function.
        function: String,
        /// How many calls it owed when proposed.
        count: u64,
        /// The first records it called for, in key order.
        first: Vec<RecordKey>,
    },
    /// A change to the records a condition selects, whole, as `enact`
    /// makes it: its accept enacts it against the plan its reviewer is
    /// shown, or is refused `plan_stale`
    /// (`docs/decisions/081-a-change-to-the-records-a-condition-selects-is-planned-before-it-acts.md`).
    Records {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The condition, in the expression grammar.
        #[serde(rename = "where")]
        filter: Condition,
        /// What it does to each record selected.
        to: ChangeTo,
        /// The proposer's `only` on the type, held beside the condition.
        /// The store answers it; whatever a proposer sends is replaced.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        only: std::collections::BTreeMap<FieldName, serde_json::Value>,
        /// Whether restricted records are left out: so where its proposer
        /// does not hold `restricted`. The store answers it; whatever a
        /// proposer sends is replaced.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        skip_restricted: bool,
        /// Its plan as its proposer was shown it. The store answers it;
        /// whatever a proposer sends is replaced.
        #[serde(default)]
        plan: Box<Plan>,
    },
}

impl ProposedWrite {
    /// The change it proposes, as `plan` plans it: to the ontology, or to
    /// the records a condition selects; `None` for a write of one record.
    pub fn change(&self) -> Option<Change> {
        match self {
            ProposedWrite::Declare { declarations, .. } => Some(Change::Declare {
                declarations: declarations.clone(),
            }),
            ProposedWrite::Migrate { migration, .. } => Some(Change::Migrate {
                migration: migration.clone(),
            }),
            ProposedWrite::MigrateValueType { to, renames, .. } => Some(Change::MigrateValueType {
                migration: Box::new(ValueTypeMigration {
                    to: (**to).clone(),
                    renames: renames.clone(),
                }),
            }),
            ProposedWrite::Records {
                ty,
                filter,
                to,
                only,
                skip_restricted,
                ..
            } => Some(Change::Records {
                ty: ty.clone(),
                filter: Box::new(filter.clone()),
                to: to.clone(),
                only: only.clone(),
                skip_restricted: *skip_restricted,
            }),
            _ => None,
        }
    }

    /// Which kind of write it is.
    pub fn kind(&self) -> ProposalKind {
        match self {
            ProposedWrite::Upsert { .. } => ProposalKind::Upsert,
            ProposedWrite::Retire { .. } => ProposalKind::Retire,
            ProposedWrite::Call { .. } => ProposalKind::Call,
            ProposedWrite::Declare { .. } => ProposalKind::Declare,
            ProposedWrite::Migrate { .. } => ProposalKind::Migrate,
            ProposedWrite::MigrateValueType { .. } => ProposalKind::MigrateValueType,
            ProposedWrite::Compute { .. } | ProposedWrite::ComputeSweep { .. } => {
                ProposalKind::Compute
            }
            ProposedWrite::Records { .. } => ProposalKind::Records,
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
    /// A function's outside calls.
    Compute,
    /// A change to the records a condition selects.
    Records,
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

/// An object set: the records of one type, or the groups of a view the
/// lake keeps, matching a condition. When a write moves a record or a
/// group into or out of the set, the store appends `ObjectSetChanged` in
/// the same append.
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
#[non_exhaustive]
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
    /// A host's own setting: its kind, its meaning and its group.
    Setting(SettingDecl),
    /// A declaration in force taken out of it.
    Withdraw(Withdrawal),
}

/// A type, a trigger, a verb, an object set or a function taken out of
/// force from the withdrawal's position on: `describe` no longer shows it,
/// and a read as of before still does. Declaring the same name again later
/// is a new declaration, whose version continues the withdrawn one's. A
/// type is withdrawn once it holds no live record and nothing in force
/// names it; the link types its relations carry leave with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Withdrawal {
    /// The kind of the declaration withdrawn.
    pub kind: DeclarationKind,
    /// Its name.
    pub name: String,
}

/// The kind of a declaration. A later kind is an addition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DeclarationKind {
    /// A type.
    Type,
    /// A value type.
    ValueType,
    /// A verb.
    Verb,
    /// A trigger.
    Trigger,
    /// An object set.
    ObjectSet,
    /// A function.
    Function,
    /// A host's own setting.
    Setting,
}

impl DeclarationKind {
    /// Every kind.
    pub(crate) const ALL: [DeclarationKind; 7] = [
        DeclarationKind::Type,
        DeclarationKind::ValueType,
        DeclarationKind::Verb,
        DeclarationKind::Trigger,
        DeclarationKind::ObjectSet,
        DeclarationKind::Function,
        DeclarationKind::Setting,
    ];

    /// What a refusal names a declaration of this kind.
    pub fn naming(self) -> crate::error::Naming {
        use crate::error::Naming;
        match self {
            DeclarationKind::Type => Naming::Type,
            DeclarationKind::ValueType => Naming::ValueType,
            DeclarationKind::Verb => Naming::Verb,
            DeclarationKind::Trigger => Naming::Trigger,
            DeclarationKind::ObjectSet => Naming::ObjectSet,
            DeclarationKind::Function => Naming::Function,
            DeclarationKind::Setting => Naming::Setting,
        }
    }

    /// Its name, as the declaration's `declare` member writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            DeclarationKind::Type => "type",
            DeclarationKind::ValueType => "value_type",
            DeclarationKind::Verb => "verb",
            DeclarationKind::Trigger => "trigger",
            DeclarationKind::ObjectSet => "object_set",
            DeclarationKind::Function => "function",
            DeclarationKind::Setting => "setting",
        }
    }
}

/// A declaration sent to `declare` or `apply`, or proposed, as it stands
/// in force after the call (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
/// extended).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InForce {
    /// Its kind.
    pub kind: DeclarationKind,
    /// Its name.
    pub name: String,
    /// Its version in force.
    pub version: u32,
    /// The position it has stood from: its `declared`, which a read
    /// `as_of` it sees.
    pub since: Position,
    /// Whether this call appended it; where not, it was in force already.
    pub appended: bool,
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
        let (kind, name) = self.named();
        format!("the {} declaration `{name}`", kind.as_str())
    }

    /// The declaration's kind and name.
    pub fn named(&self) -> (DeclarationKind, &str) {
        match self {
            Declaration::Type(t) => (DeclarationKind::Type, t.name.as_str()),
            Declaration::ValueType(v) => (DeclarationKind::ValueType, v.name.as_str()),
            Declaration::Verb(v) => (DeclarationKind::Verb, v.name.as_str()),
            Declaration::Trigger(t) => (DeclarationKind::Trigger, t.name.as_str()),
            Declaration::ObjectSet(o) => (DeclarationKind::ObjectSet, o.name.as_str()),
            Declaration::Function(f) => (DeclarationKind::Function, f.name.as_str()),
            Declaration::Setting(s) => (DeclarationKind::Setting, s.key.as_str()),
            Declaration::Withdraw(w) => (w.kind, w.name.as_str()),
        }
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
            Declaration::Setting(s) => Place::Setting { key: s.key.clone() },
            Declaration::Withdraw(w) => {
                let name = w.name.clone();
                match w.kind {
                    DeclarationKind::Type => Place::Type { name: name.into() },
                    DeclarationKind::ValueType => Place::ValueType { name: name.into() },
                    DeclarationKind::Verb => Place::Verb { name: name.into() },
                    DeclarationKind::Trigger => Place::Trigger { name: name.into() },
                    DeclarationKind::ObjectSet => Place::ObjectSet { name },
                    DeclarationKind::Function => Place::Function { name },
                    DeclarationKind::Setting => Place::Setting { key: name },
                }
            }
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
        fn fields<'a>(
            at: &str,
            within: &[Place],
            fs: impl IntoIterator<Item = &'a FieldDecl>,
            out: &mut Out<'a>,
        ) {
            for (i, f) in fs.into_iter().enumerate() {
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
            Declaration::Function(f) => {
                fields("inputs", &[], f.inputs.iter().map(|i| &i.field), &mut out)
            }
            Declaration::ValueType(v) => out.push((String::new(), Vec::new(), &v.annotations)),
            Declaration::Trigger(_)
            | Declaration::ObjectSet(_)
            | Declaration::Setting(_)
            | Declaration::Withdraw(_) => {}
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
                for (i, p) in v.preconditions.iter().enumerate() {
                    all.push((format!("preconditions[{i}].holds"), Some(&p.holds)));
                }
                if let crate::ontology::VerbEffect::Write { set } = &v.effect {
                    for (i, a) in set.iter().enumerate() {
                        all.push((format!("set[{i}].value"), Some(&a.value)));
                    }
                }
            }
            Declaration::Trigger(t) => all.push(("when".into(), t.when.as_ref())),
            Declaration::ObjectSet(s) => all.push(("where".into(), Some(&s.filter))),
            Declaration::Function(f) => all.push(("expr".into(), f.expr.as_ref())),
            Declaration::Setting(_) | Declaration::Withdraw(_) => {}
        }
        let held = all.into_iter().filter_map(|(path, c)| Some((path, c?)));
        held.collect()
    }
}

impl From<AnnotationFault> for crate::error::Error {
    fn from(f: AnnotationFault) -> crate::error::Error {
        use crate::error::{Error, Refusal};
        match f {
            AnnotationFault::Reserved { key, .. } => {
                Error::refused(Some(key.into()), Refusal::Reserved)
            }
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
    "setting",
    "withdraw",
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
        // A setting is named by its key.
        let named = if kind == "setting" { "key" } else { "name" };
        let name = members
            .get(named)
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
            "setting" => serde_path_to_error::deserialize(members).map(Declaration::Setting),
            "withdraw" => serde_path_to_error::deserialize(members).map(Declaration::Withdraw),
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
    /// Its inputs: each of the kind it states, or of any kind if it
    /// states none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<crate::ontology::FunctionInput>,
    /// For the `expr` backing: the expression, over the inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr: Option<Condition>,
    /// The backing's own configuration.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub config: serde_json::Value,
    /// What the function computes, for people and agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// How dangerous its backing's call is: above `low`, a call is
    /// proposed and made only when accepted, as a verb's is
    /// (`docs/decisions/064-an-outside-call-runs-by-its-declared-risk.md`).
    #[serde(default, skip_serializing_if = "Risk::is_low")]
    pub risk: Risk,
}

/// A setting a host declares (`docs/decisions/063-every-setting-is-in-one-list.md`,
/// extended): every write of its key is judged as a write of a field of
/// `type` is, `setting('<key>')` reads it as that kind, and only a
/// principal whose grant admits its `group` writes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingDecl {
    /// The setting's key under `sys.setting`.
    pub key: String,
    /// The kind of value it holds.
    #[serde(rename = "type")]
    pub ty: crate::ontology::FieldType,
    /// The unit its value is in, for people and agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// What it changes, for people and agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The group of settings it belongs to, which a grant's `only` on
    /// `sys.setting` names; none where absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
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
    /// Fields whose `initial`, declared in `to`, each live record that
    /// holds no value for them takes at the migration
    /// (`docs/decisions/050-a-field-declares-its-initial-value.md`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fill: Vec<FieldName>,
}

impl Migration {
    /// A migration to `to` that renames nothing and fills nothing.
    pub fn to(to: TypeDecl) -> Migration {
        let (renames, relation_renames, fill) = (Vec::new(), Vec::new(), Vec::new());
        Migration {
            to,
            renames,
            relation_renames,
            fill,
        }
    }

    /// The fields the migration renames, as pairs of the old name and the
    /// new. A pair names a field of `before` and one of the new shape, each
    /// once, and no new name is a field `before` keeps. Otherwise the case
    /// that says why. Judged against an author's view of the type, a field
    /// it may not know of is no field to it (lakelands#750). It lives in
    /// the contract; the store and the core's conversion call it.
    pub fn renamed_fields(&self, before: &TypeDecl) -> Result<Vec<(&FieldName, &FieldName)>, Case> {
        let has = |t: &TypeDecl, n: &FieldName| t.has_field(n);
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

    /// Whether `fill` names fields the migration may fill: each one `to`
    /// declares with an `initial`, once. A name `to` no longer has that a
    /// rename took away is told its new name. Otherwise the case that says
    /// why.
    pub fn fills(&self) -> Result<(), Case> {
        for (i, f) in self.fill.iter().enumerate() {
            let (field, declared) = (f.clone(), self.to.field(f));
            let case = match (declared, self.renames.iter().find(|(from, _)| from == f)) {
                (None, Some((_, to))) => Case::FillRenamed {
                    field,
                    to: to.clone(),
                },
                (None, None) => Case::FillUnknown { field },
                (Some(d), _) if d.initial.is_none() => Case::FillNoInitial { field },
                (Some(_), _) if self.fill[..i].contains(f) => Case::FillRepeated { field },
                (Some(_), _) => continue,
            };
            return Err(case);
        }
        Ok(())
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
            let of = |t: &'a TypeDecl, n| t.relation(n);
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
            let was = before.relation(name);
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
    /// Per field the migration fills (`Migration::fill`): the live records
    /// that hold no value for it and take its `initial`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filled: Vec<FieldFill>,
    /// Per constraint `to` declares: the live records that would break
    /// it as the migration leaves them (lakelands#1119). One that `to`
    /// refuses at ingest makes the migration lossy; one it admits is
    /// listed only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breaches: Vec<Breach>,
    /// The live records of the type the change computes again, as the
    /// fold does, whatever they then hold: a migration's count, 0 where it
    /// computes none again; a declaration's where its walk judges
    /// something. `None` where a declaration computes every record of the
    /// type again and nothing is judged: it is not counted
    /// (`docs/decisions/062-a-change-plans-before-it-acts.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recomputed: Option<u64>,
    /// Per reference field the change may move: the live records it would
    /// leave naming no live record. Told, and never refused: a change is
    /// its author's act (032, 062).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unlive: Vec<Unlive>,
    /// True when every stored value converts without loss, every record
    /// a re-key moves takes a new key of its own, and no record breaks a
    /// constraint that refuses. It speaks of
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

/// The live records a migration fills a field's `initial` into.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldFill {
    /// The field.
    pub field: FieldName,
    /// How many live records hold no value for it.
    pub records: u64,
    /// Up to five of their keys, the smallest its reader is answered.
    pub sample_keys: Vec<RecordKey>,
}

/// The live records a migration or a value type's migration would leave
/// breaking a constraint of their type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breach {
    /// The type whose constraint it is.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The constraint.
    pub constraint: ConstraintName,
    /// How many live records would break it.
    pub records: u64,
    /// Up to five of their keys, the smallest its reader is answered.
    pub sample_keys: Vec<RecordKey>,
    /// A field declared `required` that an act taking back part of a
    /// source leaves empty; `constraint` is then `required`
    /// (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<FieldName>,
}

/// The live records a change would leave whose reference names no live
/// record: one that does not exist, or is retired. A held record counts
/// as live, to every reader (062).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unlive {
    /// The reference field.
    pub field: FieldName,
    /// How many live records it would leave so. A held one is counted
    /// only for a reader who holds `restricted`.
    pub records: u64,
    /// Up to five of their keys, the smallest its reader is answered.
    pub sample_keys: Vec<RecordKey>,
}

/// A change to the ontology, as `plan` plans it and `enact` makes it
/// (`docs/decisions/062-a-change-plans-before-it-acts.md`): what the doors
/// `apply`, `add_field`, `recompute`, `migrate_apply` and
/// `migrate_value_type_apply` each make; or a change to the records a
/// condition selects
/// (`docs/decisions/081-a-change-to-the-records-a-condition-selects-is-planned-before-it-acts.md`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "change", rename_all = "snake_case")]
pub enum Change {
    /// Declarations, applied together as `apply` applies them.
    Declare {
        /// The declarations.
        declarations: Vec<Declaration>,
    },
    /// One field added to a type, as `add_field` adds it.
    AddField {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The field.
        field: Box<FieldDecl>,
    },
    /// What a declaration in force computes, computed again.
    Recompute {
        /// What is computed again.
        what: Recompute,
    },
    /// A type's migration.
    Migrate {
        /// The migration.
        migration: Box<Migration>,
    },
    /// A value type's migration.
    MigrateValueType {
        /// The migration.
        migration: Box<ValueTypeMigration>,
    },
    /// The same values set on, or a retirement of, every record of one
    /// type that a condition selects, as its caller's `find` with that
    /// condition answers them.
    Records {
        /// The type. On a link type, its links' own fields are set, or
        /// the links unlinked; no end is named.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The condition, in the expression grammar.
        #[serde(rename = "where")]
        filter: Box<Condition>,
        /// What it does to each record selected.
        to: ChangeTo,
        /// Text values a selected record's fields must hold, beside the
        /// condition: a proposal's carries its proposer's `only`, so its
        /// accept selects no record the proposer could not.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        only: std::collections::BTreeMap<FieldName, serde_json::Value>,
        /// Whether restricted records are left out, beside the condition:
        /// a proposal's is set where its proposer does not hold
        /// `restricted`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        skip_restricted: bool,
    },
}

/// What a change to records does to each record it selects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum ChangeTo {
    /// These values set, the same on every record: values, never an
    /// expression over the record.
    Set(Patch),
    /// Each record retired.
    Retire,
}

/// What a change would do, before it does it: told to its author, and
/// carried by its proposal to whoever accepts it
/// (`docs/decisions/062-a-change-plans-before-it-acts.md`). It is the act's
/// own path up to its append, so it refuses what the act refuses, and
/// `enact` acts only on the plan its caller was shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// Names this plan; `enact` must repeat it. As a migration's report id:
    /// the SHA-256 of `{"plan": …, "head": …}` in canonical form, the plan
    /// as its reader is shown it with this member empty.
    pub report_id: String,
    /// The head it was made at.
    pub at: Position,
    /// Each type whose stored records the change would change, by name: a
    /// migration's report, of the type migrated or of one a declaration
    /// computes again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<MigrationReport>,
    /// A value type's migration's report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub value_types: Vec<ValueTypeReport>,
    /// True when no section loses a value or breaks a constraint that
    /// refuses: the change can be made as planned. A proposal's plan of
    /// declarations only tells, and is judged on the records its proposer
    /// is told of; its accept still refuses on every record. A plan of
    /// records holds it while no record is refused.
    pub lossless: bool,
    /// A change to records' section: what it selects, changes and is
    /// refused. Its plan's id is made over it and over every selected
    /// key with the version read, never over the head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records: Option<Box<RecordsPlan>>,
}

/// What a change to records would do, as its caller is answered it: every
/// count and key is an answer to its read
/// (`docs/decisions/081-a-change-to-the-records-a-condition-selects-is-planned-before-it-acts.md`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordsPlan {
    /// The type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// How many records the condition selects.
    pub selected: u64,
    /// Up to five of their keys, the smallest.
    pub sample_keys: Vec<RecordKey>,
    /// Per field set, how many selected records it would change. A field
    /// its caller may write and not read is told as written on every one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldChanged>,
    /// Every refusal a selected record would meet, as that record's own
    /// `upsert` or `retire` is refused; one is enough that the change
    /// cannot be enacted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<RecordsRefused>,
}

/// How many selected records a change to records changes one field of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldChanged {
    /// The field.
    pub field: FieldName,
    /// How many records it would change.
    pub records: u64,
}

/// One refusal the records a change selects would meet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordsRefused {
    /// The refusal, as the first of them is refused it.
    pub refusal: Box<crate::Error>,
    /// How many records it refuses.
    pub records: u64,
    /// Up to five of their keys, the smallest.
    pub sample_keys: Vec<RecordKey>,
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Committed {
    /// The position of the window's last event; everything in the
    /// window is visible from here.
    pub position: Position,
    /// How many records the window committed, duplicates excluded.
    pub records: u32,
    /// How many records were already in the log and appended nothing.
    pub duplicates: u32,
    /// The call's delivery: the caller's, or the store's once a window
    /// appended; none before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<DeliveryId>,
}

/// How many refused moves a rule's run lists (`RuleRun::refusals`); the
/// count past it is kept in `refused`.
pub const AGE_REFUSALS_SHOWN: usize = 16;

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
    /// The conditions that could not be evaluated, and the moves the
    /// store refused for another reason than `refused` counts, over all
    /// rules.
    pub failed_guards: u32,
    /// The moves refused by a constraint or by the lake's
    /// `max_set_changes`, over all rules.
    #[serde(default)]
    pub refused: u32,
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
    /// The records whose condition could not be evaluated, or whose move
    /// the store refused for another reason than `refused` counts (a
    /// reference no longer live, say), left as they were.
    pub failed_guards: u32,
    /// The records whose move a constraint or the lake's
    /// `max_set_changes` refused, left as they were.
    #[serde(default)]
    pub refused: u32,
    /// The first [`AGE_REFUSALS_SHOWN`] of them, in scan order, each with
    /// its key and the refusal as the wire carries it (a constraint's
    /// names the constraint, the type and the key, never a value);
    /// `refused` counts them all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refusals: Vec<RefusedMove>,
    /// Why the rule was not run, when it no longer fits the lake (its
    /// type or field gone or changed, its values or its condition no
    /// longer valid). A skipped rule moves nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// A move an aging run did not make because the lake refused it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedMove {
    /// The record that stayed where it was.
    pub key: RecordKey,
    /// The refusal, `constraint` or `too_large`, in the form a refused
    /// error's `reason` takes: a kind this build does not know is read as
    /// `Refusal::Unknown`.
    #[serde(with = "crate::error::closed")]
    pub why: crate::error::Refusal,
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
    /// The set memberships and trigger firings the retirements set off
    /// across an edge, a set's `where` or a trigger's `when` reading a
    /// retired record: past `max_set_changes` the purge is refused whole
    /// (`docs/decisions/055-a-path-steps-through-a-reference.md`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub set_off: u32,
    /// The lake's `max_set_changes`, beside `set_off`.
    #[serde(default)]
    pub max_set_changes: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
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

/// What `LakeStore::forget_source` erased, or what
/// `LakeStore::forget_preflight` says it would
/// (`docs/decisions/058-a-source-may-be-forgotten.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetReport {
    /// The source forgotten.
    pub source: SourceId,
    /// Per type, in name order, the records holding a value of the source
    /// in an erasable field; types with none are left out.
    pub types: Vec<ForgetRecords>,
    /// Per type and field, in that order, how many values it erases.
    pub fields: Vec<ForgetField>,
    /// How many outputs a backing recorded under the source's key: they
    /// are erased with it.
    pub outputs: u64,
    /// Per type, in name order, the records it retires: those the source
    /// made and alone brought, as a purge retires them.
    pub retired: Vec<TypeRetired>,
    /// Per type, in name order, the records another hand made or wrote
    /// too: they stay, with that hand's values.
    pub co_owned: Vec<ForgetRecords>,
    /// The computed fields that read an erasable field holding the
    /// source's values, each computed again, in type then field order.
    pub recomputed: Vec<RecordField>,
    /// The views over a type holding the source's values, computed again.
    pub views: Vec<TypeName>,
    /// The source's values in erasable fields that the log holds in
    /// plaintext. Destroying its key cannot reach them, so while there
    /// are any the act is refused.
    pub plaintext: u64,
    /// The events the act's rebuild folds again: the log's length.
    pub events: u64,
    /// How long the act is expected to hold the writer's lock, in
    /// milliseconds: an estimate, `events` at the rate the last rebuild
    /// this handle ran folded (its own, an act's, or its open's), or
    /// before any at the engine's stated rate, and the time the last
    /// reclaim it ran took, where it ran one: a handle that has not
    /// reclaimed counts no reclaim; and the time the preflight's own walk
    /// of the log took, which the act walks again. The fold's cost is the ontology's and
    /// a reclaim's the projection's, so the act may still differ from it.
    pub expected_ms: u64,
    /// The head after the act, or the head the preflight read.
    pub position: Position,
    /// The projection's storage in bytes before and after the act's
    /// reclaim, where the engine measures it; absent on a preflight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaimed: Option<CompactReport>,
    /// Why the reclaim is still owed, where the last one tried failed:
    /// the act answers all the same, its values erased already, and the
    /// writer's next open, a repeat or a compact runs it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaim_owed: Option<String>,
    /// How the act restates the projection: the preflight's choice, or
    /// the way the act took. A repeat that finds the act done restates
    /// nothing, and answers the default.
    #[serde(default)]
    pub way: ForgetWay,
    /// Names a preflight's report; `forget_source` may repeat it. Empty
    /// on the act's own report.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub report_id: String,
}

/// What `LakeStore::forget_sources` erased, or what
/// `LakeStore::forget_sources_preflight` says it would: several sources
/// forgotten in one act, one log walk and one rebuild under one hold of
/// the writer (`docs/decisions/058-a-source-may-be-forgotten.md`,
/// extended).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetSetReport {
    /// Per source, in id order, what forgetting it takes, as
    /// `forget_source` answers it, the set's time, position and
    /// `reclaim_owed` in each; the act's reclaim and the preflight's id
    /// are the set's alone, below. A record the set's sources made and
    /// alone brought is retired under the first of them that brought it,
    /// and one another hand made or wrote too is co-owned.
    pub sources: Vec<ForgetReport>,
    /// The events the act's one rebuild folds again: the log's length.
    pub events: u64,
    /// How long the act is expected to hold the writer's lock, in
    /// milliseconds, as `ForgetReport::expected_ms` says it: one rebuild
    /// and one reclaim, and the preflight's own walk for the whole set.
    pub expected_ms: u64,
    /// The head after the act, or the head the preflight read.
    pub position: Position,
    /// The projection's storage in bytes before and after the act's
    /// reclaim, where the engine measures it; absent on a preflight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaimed: Option<CompactReport>,
    /// Why the reclaim is still owed, as `ForgetReport::reclaim_owed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaim_owed: Option<String>,
    /// How the act restates the projection, as `ForgetReport::way`.
    #[serde(default)]
    pub way: ForgetWay,
    /// Names a preflight's report over the set; `forget_sources` may
    /// repeat it. For a set of one it is the id `forget_preflight`
    /// answers. Empty on the act's own report.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub report_id: String,
}

/// How a forgetting restates the projection
/// (`docs/decisions/058-a-source-may-be-forgotten.md`): by a scoped
/// refold of the records its sources wrote, where nothing else the lake
/// keeps reads them and it is expected to take less than the rebuild;
/// otherwise by the rebuild, with why. Either leaves the projection as
/// the other would.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetWay {
    /// Whether the records are refolded alone; the rebuild otherwise.
    pub scoped: bool,
    /// Why the rebuild is chosen, each in a sentence; empty for the
    /// scoped refold.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub why: Vec<String>,
    /// The records the scoped refold restates.
    pub records: u64,
    /// The events it folds: those about its records, and the lake's own
    /// declarations, migrations and settings once per page, each record
    /// a migration walks of a type its records read counted as one.
    pub events: u64,
    /// How long the scoped refold is expected to take, in milliseconds,
    /// at the rate the last one this handle ran folded, or before any at
    /// the rebuild's, and the reclaim's time.
    pub scoped_ms: u64,
    /// How long the rebuild is expected to take, as `expected_ms` reckons
    /// it without the preflight's walk.
    pub rebuild_ms: u64,
}

/// Records of one type a forgetting reads, with the first keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetRecords {
    /// The type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// How many records.
    pub records: u64,
    /// The first ten keys, in key order.
    pub first: Vec<RecordKey>,
}

/// The values one field of one type loses to a forgetting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgetField {
    /// The type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The erasable field.
    pub field: FieldName,
    /// How many of its values the source wrote.
    pub values: u64,
}

/// A field of a type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RecordField {
    /// The type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The field.
    pub field: FieldName,
}

/// Which of a source's values `LakeStore::remove_values` takes back
/// (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    /// What it wrote before a time of the log's own clock.
    WrittenBefore(Timestamp),
    /// What it wrote in one delivery.
    Delivery(DeliveryId),
    /// What it wrote in any of a set of deliveries, in one act: as the acts
    /// taking each back would take them, with one walk and one rebuild.
    Deliveries(Vec<DeliveryId>),
    /// What it wrote of what it says it observed before a time: each event
    /// whose `seen_at` is before it, wherever in the log it stands.
    SeenBefore(Timestamp),
}

/// The values one field of one type loses to a removal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedField {
    /// The type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The field.
    pub field: FieldName,
    /// How many of its values read as erased at every position, their
    /// bytes still in the log.
    pub redacted: u64,
    /// How many of those were sealed under a version the act destroys.
    pub shredded: u64,
}

/// What `LakeStore::remove_values` took back, or what
/// `LakeStore::remove_values_preflight` says it would
/// (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveReport {
    /// The source.
    pub source: SourceId,
    /// The selection, as asked.
    pub selection: Selection,
    /// For a time of the log's clock, the first position it does not
    /// reach: every event of the source below it is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Position>,
    /// The source's `record_ingested` events selected.
    pub events: u64,
    /// For a delivery or a set of them, the source's events written
    /// before the log kept deliveries, which it cannot select.
    pub undelivered: u64,
    /// For a time the source observed, the source's events that state no
    /// `seen_at`, which it cannot select.
    #[serde(default)]
    pub unseen: u64,
    /// Per type, in name order, the records holding a value it removes.
    pub types: Vec<ForgetRecords>,
    /// Per type and field, in that order, the values it removes.
    pub fields: Vec<RemovedField>,
    /// Per type, in name order, the records the source made left holding
    /// nothing of another hand: retired, as a purge retires them.
    pub retired: Vec<TypeRetired>,
    /// Per type, in name order, the records reached that stay, with what
    /// another hand or the source outside the selection wrote.
    pub kept: Vec<ForgetRecords>,
    /// The computed fields that read a field it removes values of, each
    /// computed again, in type then field order.
    pub recomputed: Vec<RecordField>,
    /// The views over a type it removes values from, computed again.
    pub views: Vec<TypeName>,
    /// The constraints the records it keeps would be left breaking. The
    /// act is not refused for them.
    pub breaches: Vec<Breach>,
    /// Per relation, the live records left naming a record it retires.
    pub references: Vec<PurgeReference>,
    /// The versions of the source's key it destroys, oldest first.
    pub keys: Vec<KeyId>,
    /// The keys of the source's deliveries sealed apart it destroys, in id
    /// order: each whose values it all selects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apart: Vec<KeyId>,
    /// The values of an erasable field it redacts and does not shred: in
    /// plaintext, or sealed under a version that also seals values outside
    /// the selection. `require_shredded` refuses the act while any are.
    pub unshredded: u64,
    /// The events the act's rebuild folds again: the log's length.
    pub log_events: u64,
    /// How long the act is expected to hold the writer's lock, in
    /// milliseconds: an estimate, `log_events` at the rate the last
    /// rebuild this handle ran folded, the last reclaim's time and the
    /// preflight's own walk, as [`ForgetReport::expected_ms`].
    pub expected_ms: u64,
    /// The head after the act, or the head the preflight read.
    pub position: Position,
    /// The projection's storage in bytes before and after the act's
    /// reclaim, where the engine measures it; absent on a preflight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaimed: Option<CompactReport>,
    /// Why the reclaim is still owed, where the last one tried failed:
    /// the act answers all the same, its values erased already, and the
    /// writer's next open, a repeat or a compact runs it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reclaim_owed: Option<String>,
    /// Names a preflight's report; `remove_values` may repeat it. Empty
    /// on the act's own report.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub report_id: String,
}

impl RemoveReport {
    /// The report in sentences, for a host to tell its operator.
    pub fn said(&self) -> Vec<String> {
        let what = match (&self.selection, self.before) {
            (Selection::WrittenBefore(t), Some(p)) => {
                format!("written before {t} (below position {})", p.0)
            }
            (Selection::WrittenBefore(t), None) => format!("written before {t}"),
            (Selection::Delivery(d), _) => format!("of delivery `{d}`"),
            (Selection::Deliveries(ds), _) if ds.is_empty() => "of no delivery".to_owned(),
            (Selection::Deliveries(ds), _) => {
                let named: Vec<String> = ds.iter().take(5).map(|d| format!("`{d}`")).collect();
                let more = ds.len().saturating_sub(5);
                let more = if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                };
                let n = if ds.len() == 1 {
                    "delivery"
                } else {
                    "deliveries"
                };
                format!("of {} {n} ({}{more})", ds.len(), named.join(", "))
            }
            (Selection::SeenBefore(t), _) => format!("seen before {t}"),
        };
        let (events, are) = (counted(self.events, "event"), be(self.events));
        let mut said = vec![format!(
            "{events} of `{}` {what} {are} selected.",
            self.source
        )];
        if self.undelivered > 0 {
            let n = counted(self.undelivered, "older event");
            let keep = if self.undelivered == 1 {
                "keeps"
            } else {
                "keep"
            };
            said.push(format!("{n} {keep} no delivery and stay."));
        }
        if self.unseen > 0 {
            let n = counted(self.unseen, "event");
            let state = verb(self.unseen, "states", "state");
            let stay = verb(self.unseen, "stays", "stay");
            said.push(format!("{n} {state} no time seen and {stay}."));
        }
        for f in &self.fields {
            let (ty, field) = (&f.ty, &f.field);
            let (n, read) = (
                counted(f.redacted, "value"),
                verb(f.redacted, "reads", "read"),
            );
            said.push(format!(
                "{n} of `{ty}.{field}` {read} as erased at every position; the log keeps the bytes."
            ));
            if f.shredded > 0 {
                let s = f.shredded;
                said.push(format!(
                    "{s} of them {} shredded: the key that sealed {} is destroyed.",
                    be(s),
                    if s == 1 { "it" } else { "them" }
                ));
            }
        }
        if !self.keys.is_empty() {
            let keys: Vec<&str> = self.keys.iter().map(|k| k.as_str()).collect();
            let n = self.keys.len() as u64;
            said.push(format!(
                "{} {} of `{}`'s key {} destroyed.",
                if n == 1 { "Version" } else { "Versions" },
                keys.join(", "),
                self.source,
                be(n)
            ));
        }
        if !self.apart.is_empty() {
            let keys: Vec<&str> = self.apart.iter().map(|k| k.as_str()).collect();
            let n = self.apart.len() as u64;
            said.push(format!(
                "The {} {} of `{}`'s deliveries sealed apart {} destroyed.",
                if n == 1 { "key" } else { "keys" },
                keys.join(", "),
                self.source,
                be(n)
            ));
        }
        if self.unshredded > 0 {
            let n = self.unshredded;
            said.push(format!(
                "{} of erasable fields {} redacted only, its key still live or none.",
                counted(n, "value"),
                be(n)
            ));
        }
        for t in &self.retired {
            let (n, ty) = (u64::from(t.retired), &t.ty);
            said.push(format!(
                "{} of `{ty}` {} retired.",
                counted(n, "record"),
                be(n)
            ));
        }
        for k in &self.kept {
            let (n, ty) = (k.records, &k.ty);
            let stay = verb(n, "stays", "stay");
            said.push(format!(
                "{} of `{ty}` {stay}, with what others wrote.",
                counted(n, "record")
            ));
        }
        for f in &self.recomputed {
            said.push(format!("`{}.{}` is computed again.", f.ty, f.field));
        }
        for v in &self.views {
            said.push(format!("The view `{v}` is computed again."));
        }
        for b in &self.breaches {
            let (n, ty, c) = (b.records, &b.ty, &b.constraint);
            let records = counted(n, "record");
            said.push(match &b.field {
                Some(f) => format!(
                    "{records} of `{ty}` {} left without `{f}`, which it requires.",
                    be(n)
                ),
                None => format!("{records} of `{ty}` {} left breaking `{c}`.", be(n)),
            });
        }
        for r in &self.references {
            let (n, ty, rel) = (r.records, &r.ty, &r.relation);
            said.push(format!(
                "{} of `{ty}` {} left naming a retired one by `{rel}`.",
                counted(n, "record"),
                be(n)
            ));
        }
        let (n, ms) = (counted(self.log_events, "event"), self.expected_ms);
        said.push(format!(
            "The act refolds {n}, about {ms} ms under the writer's lock."
        ));
        said
    }
}

/// `n` and `noun`, plural but for one.
fn counted(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// "is" for one, "are" for any other count.
fn be(n: u64) -> &'static str {
    verb(n, "is", "are")
}

/// `one` for a count of one, `many` for any other.
fn verb(n: u64, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
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
    /// Per constraint that admits at ingest, the records the renames would
    /// leave breaking it; one that refuses refuses the migration
    /// (lakelands#1119).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breaches: Vec<Breach>,
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

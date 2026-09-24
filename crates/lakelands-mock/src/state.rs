//! The mock's projection: the log folded up to one position.
//!
//! Every read folds the events up to the position it is read at, so a
//! read `as_of` an earlier position sees exactly the state the lake held
//! there, and a rebuild has nothing to rebuild. The mock is small, so
//! the cost of folding on every read does not matter.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::Version;
use lakelands_api::event::{Event, EventBody, EventId};
use lakelands_api::id::{FieldName, IdempotencyKey, ProposalId, RecordKey, TypeName, VerbName};
use lakelands_api::ontology::{
    Authority, FieldDecl, OnInputChange, TriggerDecl, TypeDecl, ValueTypeDecl, VerbDecl,
};
use lakelands_api::principal::{Access, Principal};
use lakelands_api::record::{FieldMeta, Origin, Record, StoredRecord};
use lakelands_api::write::{Declaration, ObjectSetDecl, Outcome, Proposal, ProposalState};

/// One declared type, with its declaration's version and shape hash.
#[derive(Clone, Debug)]
pub(crate) struct DeclaredType {
    pub decl: TypeDecl,
    pub version: u32,
    pub schema_hash: String,
}

/// The lake as it stood at one position.
#[derive(Clone, Debug, Default)]
pub(crate) struct State {
    pub types: BTreeMap<TypeName, DeclaredType>,
    pub verbs: BTreeMap<VerbName, VerbDecl>,
    pub value_types: Vec<ValueTypeDecl>,
    pub triggers: Vec<TriggerDecl>,
    pub object_sets: Vec<ObjectSetDecl>,
    /// Declarations the mock keeps but gives no meaning.
    pub other_declarations: Vec<Declaration>,
    /// The version of every declaration but a type's, by kind and name.
    pub decl_versions: BTreeMap<String, u32>,
    pub records: BTreeMap<(TypeName, RecordKey), StoredRecord>,
    pub proposals: BTreeMap<ProposalId, Proposal>,
    /// The outcome each `(writer, idempotency key)` answered first.
    pub idempotent: BTreeMap<(String, IdempotencyKey), Outcome>,
    /// The ids of every ingested event, for content addressing.
    pub ingested: BTreeSet<EventId>,
}

impl State {
    /// Fold events, in log order.
    pub fn fold<'a>(events: impl IntoIterator<Item = &'a Event>) -> State {
        let mut st = State::default();
        for e in events {
            st.apply(e);
        }
        st
    }

    fn apply(&mut self, e: &Event) {
        let outcome = |version: Option<Version>| Outcome::Applied {
            position: e.position,
            version,
        };
        match &e.body {
            EventBody::Declared {
                declaration,
                schema_hash,
                version,
            } => {
                if let Some(k) = decl_key(declaration) {
                    self.decl_versions.insert(k, *version);
                }
                self.declare(declaration, schema_hash.clone(), *version);
            }
            EventBody::Migrated {
                migration,
                schema_hash,
                version,
                ..
            } => {
                let ty = &migration.to.name;
                for ((t, _), rec) in self.records.iter_mut() {
                    if t != ty {
                        continue;
                    }
                    for (from, to) in &migration.renames {
                        if let Some(v) = rec.fields.remove(from) {
                            rec.fields.insert(to.clone(), v);
                        }
                        if let Some(m) = rec.meta.remove(from) {
                            rec.meta.insert(to.clone(), m);
                        }
                    }
                    for f in &migration.to.fields {
                        if let Some(v) = rec.fields.get_mut(&f.name)
                            && let Some(c) = crate::value::convert(v, &f.ty)
                        {
                            *v = c;
                        }
                    }
                }
                self.types.insert(
                    ty.clone(),
                    DeclaredType {
                        decl: migration.to.clone(),
                        version: *version,
                        schema_hash: schema_hash.clone(),
                    },
                );
            }
            EventBody::RecordUpserted {
                ty,
                key,
                version,
                patch,
            }
            | EventBody::RecordIngested {
                ty,
                key,
                version,
                patch,
            } => {
                if matches!(e.body, EventBody::RecordIngested { .. }) {
                    self.ingested.insert(e.id.clone());
                }
                let decl = self.types.get(ty).map(|t| t.decl.clone());
                let rec = self
                    .records
                    .entry((ty.clone(), key.clone()))
                    .or_insert_with(|| StoredRecord {
                        ty: ty.clone(),
                        key: key.clone(),
                        version: Version(0),
                        fields: BTreeMap::new(),
                        meta: BTreeMap::new(),
                        position: e.position,
                        retired: false,
                        restricted: false,
                    });
                let field_meta = decl.as_ref().is_none_or(|d| d.field_meta);
                let mut changed = Vec::new();
                for (f, v) in patch {
                    if rec.fields.get(f) != Some(v) && !(v.is_null() && !rec.fields.contains_key(f))
                    {
                        changed.push(f.clone());
                    }
                    if v.is_null() {
                        rec.fields.remove(f);
                        rec.meta.remove(f);
                    } else {
                        rec.fields.insert(f.clone(), v.clone());
                        if field_meta {
                            rec.meta.insert(f.clone(), meta_of(e));
                        }
                    }
                }
                // A dependent field declared `clear` is cleared in the
                // same event that changes one of its inputs.
                if let Some(decl) = &decl {
                    for f in &decl.fields {
                        if f.on_input_change == OnInputChange::Clear
                            && !patch.contains_key(&f.name)
                            && f.depends_on.iter().any(|d| changed.contains(d))
                        {
                            rec.fields.remove(&f.name);
                            rec.meta.remove(&f.name);
                        }
                    }
                }
                rec.version = *version;
                rec.position = e.position;
                rec.retired = false;
                self.remember(e, outcome(Some(*version)));
            }
            EventBody::RecordRetired { ty, key, version } => {
                if let Some(rec) = self.records.get_mut(&(ty.clone(), key.clone())) {
                    rec.version = *version;
                    rec.position = e.position;
                    rec.retired = true;
                }
                self.remember(e, outcome(Some(*version)));
            }
            EventBody::WriteProposed {
                proposal,
                write,
                why,
            } => {
                self.proposals.insert(
                    proposal.clone(),
                    Proposal {
                        id: proposal.clone(),
                        position: e.position,
                        proposer: e.writer.clone(),
                        proposed_at: e.known_at,
                        write: write.clone(),
                        why: why.clone(),
                        state: ProposalState::Pending,
                        decided_by: None,
                        reason: None,
                    },
                );
                self.remember(
                    e,
                    Outcome::Proposed {
                        position: e.position,
                        proposal: proposal.clone(),
                        why: why.clone(),
                    },
                );
            }
            EventBody::WriteAccepted { proposal } => {
                if let Some(p) = self.proposals.get_mut(proposal) {
                    p.state = ProposalState::Accepted;
                    p.decided_by = Some(e.writer.clone());
                }
            }
            EventBody::WriteRejected { proposal, reason } => {
                if let Some(p) = self.proposals.get_mut(proposal) {
                    p.state = ProposalState::Rejected;
                    p.decided_by = Some(e.writer.clone());
                    p.reason = Some(reason.clone());
                }
            }
            _ => {}
        }
    }

    fn declare(&mut self, d: &Declaration, schema_hash: Option<String>, version: u32) {
        match d {
            Declaration::Type(t) => {
                self.types.insert(
                    t.name.clone(),
                    DeclaredType {
                        decl: t.clone(),
                        version,
                        schema_hash: schema_hash.unwrap_or_default(),
                    },
                );
            }
            Declaration::Verb(v) => {
                self.verbs.insert(v.name.clone(), v.clone());
            }
            Declaration::ValueType(v) => {
                self.value_types.retain(|x| x.name != v.name);
                self.value_types.push(v.clone());
            }
            Declaration::Trigger(t) => {
                self.triggers.retain(|x| x.name != t.name);
                self.triggers.push(t.clone());
            }
            Declaration::ObjectSet(s) => {
                self.object_sets.retain(|x| x.name != s.name);
                self.object_sets.push(s.clone());
            }
            #[allow(unreachable_patterns)]
            other => self.other_declarations.push(other.clone()),
        }
    }

    /// Remember the outcome an event answered under its idempotency key.
    fn remember(&mut self, e: &Event, outcome: Outcome) {
        if let Some(k) = &e.idempotency_key {
            self.idempotent
                .entry((e.writer.clone(), k.clone()))
                .or_insert(outcome);
        }
    }

    /// A live record.
    pub fn live(&self, ty: &TypeName, key: &RecordKey) -> Option<&StoredRecord> {
        self.records
            .get(&(ty.clone(), key.clone()))
            .filter(|r| !r.retired)
    }

    /// A record's current version; 0 when it was never written.
    pub fn version(&self, ty: &TypeName, key: &RecordKey) -> Version {
        self.records
            .get(&(ty.clone(), key.clone()))
            .map_or(Version(0), |r| r.version)
    }

    /// A type the principal may see: declared, and holding at least one
    /// grant for it. Anything else is absent, not forbidden.
    pub fn visible(&self, who: &Principal, ty: &TypeName) -> Option<&DeclaredType> {
        self.types.get(ty).filter(|t| {
            type_default(who, ty) != Access::None
                || t.decl
                    .fields
                    .iter()
                    .any(|f| who.grants.access(ty, &f.name) != Access::None)
        })
    }

    /// A record as one principal may read it.
    pub fn read(&self, who: &Principal, rec: &StoredRecord, with_meta: bool) -> Record {
        let decl = self.types.get(&rec.ty).map(|t| &t.decl);
        let mut out = Record {
            ty: rec.ty.clone(),
            key: rec.key.clone(),
            version: rec.version,
            fields: BTreeMap::new(),
            masked: Vec::new(),
            meta: BTreeMap::new(),
        };
        for (f, v) in &rec.fields {
            let granted = match decl.and_then(|d| field(d, f)) {
                Some(fd) => access(who, &rec.ty, fd),
                None => Access::None,
            };
            match granted {
                a if a.reads() => {
                    out.fields.insert(f.clone(), v.clone());
                    if let (true, Some(m)) = (with_meta, rec.meta.get(f)) {
                        out.meta.insert(f.clone(), m.clone());
                    }
                }
                Access::Masked => out.masked.push(f.clone()),
                _ => {}
            }
        }
        // A field the type no longer declares is not shown.
        if let Some(decl) = decl {
            let declared = |f: &FieldName| decl.fields.iter().any(|d| &d.name == f);
            out.fields.retain(|f, _| declared(f));
            out.meta.retain(|f, _| declared(f));
            out.masked.retain(|f| declared(f));
        }
        out
    }
}

/// The key a non-type declaration's version is kept under.
fn decl_key(d: &Declaration) -> Option<String> {
    match d {
        Declaration::Type(_) => None,
        Declaration::ValueType(v) => Some(format!("value_type:{}", v.name)),
        Declaration::Verb(v) => Some(format!("verb:{}", v.name)),
        Declaration::Trigger(t) => Some(format!("trigger:{}", t.name)),
        Declaration::ObjectSet(s) => Some(format!("object_set:{}", s.name)),
        #[allow(unreachable_patterns)]
        other => serde_json::to_value(other)
            .ok()
            .map(|v| format!("{}:{}", v["declare"], v["name"])),
    }
}

/// The access a principal has to one field: its grant, except that a
/// field declared sensitive is masked rather than dropped.
pub(crate) fn access(who: &Principal, ty: &TypeName, f: &FieldDecl) -> Access {
    match who.grants.access(ty, &f.name) {
        Access::None if f.sensitive => Access::Masked,
        a => a,
    }
}

/// The access a principal has to a type as a whole.
fn type_default(who: &Principal, ty: &TypeName) -> Access {
    who.grants
        .types
        .get(ty)
        .and_then(|t| t.default)
        .unwrap_or(who.grants.default)
}

fn meta_of(e: &Event) -> FieldMeta {
    FieldMeta {
        origin: e.origin,
        known_at: e.known_at,
        seen_at: e.seen_at,
        position: e.position,
        writer: e.writer.clone(),
        source: e.source.clone(),
        procedure_version: e.procedure.as_ref().map(|p| p.version.clone()),
    }
}

/// Whether a door of `origin` may carry a field of `authority`.
pub(crate) fn door_writes(origin: Origin, authority: Authority) -> bool {
    matches!(
        (origin, authority),
        (Origin::Authored, Authority::Authored | Authority::Shared)
            | (Origin::Source, Authority::Source | Authority::Shared)
    )
}

/// A field declaration by name.
pub(crate) fn field<'a>(decl: &'a TypeDecl, name: &FieldName) -> Option<&'a FieldDecl> {
    decl.fields.iter().find(|f| &f.name == name)
}

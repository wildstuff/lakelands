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
use lakelands_api::record::{FieldMeta, Origin, Record, StoredRecord, Value};
use lakelands_api::write::{
    Declaration, FunctionDecl, ObjectSetDecl, Outcome, Proposal, ProposalState,
};

/// One declared type, with its declaration's version and shape hash.
#[derive(Clone, Debug)]
pub(crate) struct DeclaredType {
    pub decl: TypeDecl,
    pub version: u32,
    pub schema_hash: String,
    /// Each field's `depends_on` as declared: the completed lists are made
    /// again from them.
    pub listed: Vec<Vec<FieldName>>,
}

/// A type's fields and its relations' link fields: all that may be computed.
fn computed(t: &mut TypeDecl) -> impl Iterator<Item = &mut FieldDecl> {
    let links = t.relations.iter_mut().flat_map(|r| r.fields.iter_mut());
    t.fields.iter_mut().chain(links)
}

/// The lake as it stood at one position.
#[derive(Clone, Debug, Default)]
pub(crate) struct State {
    pub types: BTreeMap<TypeName, DeclaredType>,
    pub verbs: BTreeMap<VerbName, VerbDecl>,
    pub value_types: Vec<ValueTypeDecl>,
    pub triggers: Vec<TriggerDecl>,
    pub object_sets: Vec<ObjectSetDecl>,
    /// Functions, kept for `describe`; the mock computes none of them.
    pub functions: Vec<FunctionDecl>,
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
            output: None,
            set_off: Default::default(),
            renamed: Vec::new(),
        };
        match &e.body {
            EventBody::Declared {
                declaration,
                schema_hash,
                version,
                ..
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
                // As the store: a renamed relation's link records move to
                // the link type of its new name, as they are.
                for (old, new) in &migration.relation_renames {
                    let (from, to) = (crate::links::name(ty, old), crate::links::name(ty, new));
                    let moved: Vec<_> = (self.records.keys().filter(|k| k.0 == from))
                        .cloned()
                        .collect();
                    for at in moved {
                        if let Some(mut r) = self.records.remove(&at) {
                            (r.ty, r.position) = (to.clone(), e.position);
                            self.records.insert((to.clone(), at.1), r);
                        }
                    }
                }
                let to = Declaration::Type(migration.to.clone());
                self.declare(&to, Some(schema_hash.clone()), *version);
            }
            EventBody::ValueTypeMigrated {
                declaration,
                version,
                renames,
                types,
                ..
            } => {
                // As the store: every field of the value type but one a
                // backed function computes takes the new value, at the
                // event's position, its version and provenance as they are.
                let reached: BTreeMap<TypeName, Vec<FieldName>> = (types.iter())
                    .filter_map(|t| self.types.get(t).map(|d| (t.clone(), d)))
                    .map(|(t, d)| {
                        let fields =
                            crate::valuetypes::renamed_fields(self, &d.decl, &declaration.name);
                        (t, fields)
                    })
                    .collect();
                for ((t, _), rec) in self.records.iter_mut() {
                    for f in reached.get(t).into_iter().flatten() {
                        let old = rec.fields.get(f).and_then(|v| v.as_str());
                        let new = old.and_then(|o| renames.iter().find(|(from, _)| from == o));
                        if let Some((_, new)) = new {
                            rec.fields.insert(f.clone(), new.as_str().into());
                            rec.position = e.position;
                        }
                    }
                }
                let decl = Declaration::ValueType(declaration.clone());
                if let Some(k) = decl_key(&decl) {
                    self.decl_versions.insert(k, *version);
                }
                self.declare(&decl, None, *version);
            }
            EventBody::RecordUpserted {
                ty,
                key,
                version,
                patch,
                made,
            }
            | EventBody::RecordIngested {
                ty,
                key,
                version,
                patch,
                made,
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
                // As the store: a record made again starts from no fields.
                if *made {
                    rec.fields.clear();
                    rec.meta.clear();
                }
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
                // As the store (050): the event that makes a record fills
                // each declared `initial` value its patch left out, with
                // the origin `initial` and nothing of a source.
                for f in decl.iter().flat_map(|d| &d.fields).filter(|_| *made) {
                    let Some(v) = f
                        .initial
                        .as_ref()
                        .filter(|_| !rec.fields.contains_key(&f.name))
                    else {
                        continue;
                    };
                    rec.fields
                        .insert(f.name.clone(), crate::value::canonical(&f.ty, v));
                    if field_meta {
                        let filled = FieldMeta {
                            origin: Origin::Initial,
                            upstream: None,
                            seen_at: None,
                            source: None,
                            procedure_version: None,
                            ..meta_of(e)
                        };
                        rec.meta.insert(f.name.clone(), filled);
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
                        write: Some(write.clone()),
                        why: why.clone(),
                        state: ProposalState::Pending,
                        decided_by: None,
                        reason: None,
                        more: false,
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

    /// Put a declaration into the ontology, as its `Declared` event does.
    pub(crate) fn declare(&mut self, d: &Declaration, schema_hash: Option<String>, version: u32) {
        match d {
            Declaration::Type(t) => {
                let mut decl = t.clone();
                let mut held = DeclaredType {
                    listed: computed(&mut decl).map(|f| f.depends_on.clone()).collect(),
                    decl,
                    version,
                    schema_hash: schema_hash.unwrap_or_default(),
                };
                self.complete(&mut held.decl);
                self.types.insert(t.name.clone(), held);
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
            // As the store: a function's inputs are what its fields are
            // computed from, so the types in force are completed again,
            // from their lists as declared.
            Declaration::Function(f) => {
                self.functions.retain(|x| x.name != f.name);
                self.functions.push(f.clone());
                for name in self.types.keys().cloned().collect::<Vec<_>>() {
                    let mut held = self.types.remove(&name).expect("a type in force");
                    for (f, declared) in computed(&mut held.decl).zip(&held.listed) {
                        f.depends_on.clone_from(declared);
                    }
                    self.complete(&mut held.decl);
                    self.types.insert(name, held);
                }
            }
        }
    }

    /// As the store: whether `d` is the declaration in force, a type in its
    /// completed form. Such a declaration appends nothing.
    pub(crate) fn in_force(&self, d: &Declaration) -> bool {
        match d {
            Declaration::Type(t) => {
                let mut t = t.clone();
                self.complete(&mut t);
                self.types.get(&t.name).is_some_and(|held| held.decl == t)
            }
            Declaration::Verb(v) => self.verbs.get(&v.name) == Some(v),
            Declaration::ValueType(v) => self.value_types.contains(v),
            Declaration::Trigger(t) => self.triggers.contains(t),
            Declaration::ObjectSet(s) => self.object_sets.contains(s),
            Declaration::Function(f) => self.functions.contains(f),
        }
    }

    /// As the store: a shape with every computed field's `depends_on`
    /// holding each field it is computed from, what its expression reads or
    /// its function's inputs; a field is read no further than those.
    pub(crate) fn complete(&self, t: &mut TypeDecl) {
        for f in computed(t).filter(|f| f.embed.is_none()) {
            let function = f.function.as_deref();
            let inputs = function.and_then(|n| self.functions.iter().find(|x| x.name == n));
            let mut reads: Vec<FieldName> = match (inputs, &f.derived) {
                (Some(func), _) => func.inputs.iter().map(|i| i.name.clone()).collect(),
                (None, Some(e)) => {
                    let mut paths = Vec::new();
                    if let Ok(e) = crate::expr::typed(e) {
                        crate::expr::paths(&e, &mut paths);
                    }
                    paths
                        .into_iter()
                        .filter_map(|p| p.first().cloned())
                        .collect()
                }
                (None, None) => Vec::new(),
            };
            reads.retain(|r| !f.depends_on.contains(r));
            reads.sort();
            reads.dedup();
            f.depends_on.extend(reads);
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

    /// A type the principal may see: declared, and its grants giving it
    /// some access to the type or to one of its fields, a `sensitive`
    /// field's raise not among them. Anything else is absent, not
    /// forbidden. As the store (006, extended), a type made of another is
    /// shown only with it: a link type with the type that declares its
    /// relation and that relation's target, and a view with each of its
    /// sources, whatever a grant on its own name says.
    pub fn visible(&self, who: &Principal, ty: &TypeName) -> Option<&DeclaredType> {
        // As the store: a type keyed by a field its principal may not know
        // of is not shown.
        let own = |t: &&DeclaredType| {
            let d = &t.decl;
            let given = |f| built_from(who, d, f, &mut Vec::new()).0 != Access::None;
            let mut key = d.fields.iter().filter(|f| d.key.contains(&f.name));
            key.all(|f| access(who, d, f) != Access::None)
                && (type_default(who, &d.name) != Access::None || d.fields.iter().any(given))
        };
        let shown = self.types.get(ty).filter(own)?;
        let (mut asked, mut next) = (vec![ty.clone()], vec![shown]);
        while let Some(t) = next.pop() {
            let owner = crate::links::parts(&t.decl.name).map(|p| p.0);
            // A link type's `to` names its relation's target.
            let to = (owner.iter())
                .flat_map(|_| &t.decl.fields)
                .find(|f| f.name.as_str() == "to");
            let target = to.and_then(|f| f.ty.target.as_ref());
            let sources = (t.decl.view.iter())
                .flat_map(|v| std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source)));
            for n in owner.iter().chain(target).chain(sources) {
                let Some(o) = self.types.get(n).filter(|_| !asked.contains(n)) else {
                    continue;
                };
                own(&o).then_some(())?;
                asked.push(n.clone());
                next.push(o);
            }
        }
        Some(shown)
    }

    /// As the store (051): whether `who`'s grants admit a stored record:
    /// it holds every pair of the grant's `only` on its type, a grant that
    /// cannot be applied there admitting nothing, and a link's end whose
    /// type the grant holds `only` on is admitted by its pairs too. The
    /// mock computes no view, so a view's `only` never reaches a record.
    pub fn admitted(&self, who: &Principal, r: &StoredRecord) -> bool {
        self.admits_fields(who, &r.ty, &r.fields)
    }

    /// [`Self::admitted`] for a record of `ty` holding `fields`, as a
    /// write would leave it.
    pub fn admits_fields(
        &self,
        who: &Principal,
        ty: &TypeName,
        fields: &BTreeMap<FieldName, Value>,
    ) -> bool {
        let Some(t) = self.types.get(ty) else {
            return true;
        };
        let pairs = |d: &TypeDecl, fields: &BTreeMap<FieldName, Value>| {
            only_applies(who, d) && admits(who, &d.name, fields)
        };
        pairs(&t.decl, fields)
            && held_ends(who, &t.decl).into_iter().all(|(f, end)| {
                let at = fields.get(f).and_then(Value::as_str);
                let e = at.and_then(|k| self.records.get(&(end.clone(), RecordKey::from(k))));
                e.zip(self.types.get(end))
                    .is_some_and(|(e, d)| pairs(&d.decl, &e.fields))
            })
    }

    /// Whether a view is answered to `who` from only some records (051):
    /// the grant holds `only` on it, or on what it reads, a link type's end
    /// type included.
    pub fn only_some(&self, who: &Principal, v: &TypeDecl, seen: &mut Vec<TypeName>) -> bool {
        let Some(view) = v.view.as_deref().filter(|_| !seen.contains(&v.name)) else {
            return only_holder(who, v);
        };
        seen.push(v.name.clone());
        let sources = std::iter::once(&view.source).chain(view.merge.iter().map(|m| &m.source));
        let mut sources = sources.filter_map(|s| self.types.get(s));
        let held = !own_only(who, &v.name).is_empty()
            || sources.any(|s| self.only_some(who, &s.decl, seen));
        seen.pop();
        held
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
            let granted = match decl.and_then(|d| Some((d, field(d, f)?))) {
                Some((d, fd)) => access(who, d, fd),
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
            // In the declaration's order, as the store lists them.
            out.masked
                .sort_by_key(|f| decl.fields.iter().position(|d| &d.name == f));
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

/// The access a principal has to one field of `decl`: its grant, read no
/// further than every field it is computed or embedded from, except that
/// a field declared sensitive is masked rather than dropped.
pub(crate) fn access(who: &Principal, decl: &TypeDecl, f: &FieldDecl) -> Access {
    access_seen(who, decl, f, &mut Vec::new())
}

fn access_seen<'d>(
    who: &Principal,
    decl: &'d TypeDecl,
    f: &'d FieldDecl,
    seen: &mut Vec<&'d FieldName>,
) -> Access {
    let (a, from_source) = built_from(who, decl, f, seen);
    // `sensitive` shows a field nobody granted or denied by name as masked;
    // a `none` given by name, or handed down by a source, hides it.
    let named = (who.grants.types.get(&decl.name)).is_some_and(|t| t.fields.contains_key(&f.name));
    match a {
        Access::None if f.sensitive && !named && !from_source => Access::Masked,
        a => a,
    }
}

/// A field's grant, lowered by each field it is built from that the
/// principal may not read, and whether a source hid it: what a type is
/// shown by, before `sensitive` raises anything.
fn built_from<'d>(
    who: &Principal,
    decl: &'d TypeDecl,
    f: &'d FieldDecl,
    seen: &mut Vec<&'d FieldName>,
) -> (Access, bool) {
    let mut a = who.grants.access(&decl.name, &f.name);
    let mut from_source = false;
    seen.push(&f.name);
    let sources = f
        .depends_on
        .iter()
        .chain(f.embed.iter().flat_map(|e| &e.from));
    for name in sources {
        if seen.contains(&name) {
            continue;
        }
        if let Some(source) = decl.fields.iter().find(|s| &s.name == name) {
            a = match access_seen(who, decl, source, seen) {
                s if s.reads() => a,
                // Its value stays hidden; its write stays the principal's.
                _ if a.writes() => Access::WriteOnly,
                Access::Masked if a.reads() => Access::Masked,
                Access::Masked => a,
                _ => {
                    from_source = true;
                    Access::None
                }
            };
        }
    }
    seen.pop();
    (a, from_source)
}

/// The pairs `who`'s grant on `ty` admits records by: its `only`, empty
/// where it admits every record (051).
pub(crate) fn own_only<'a>(who: &'a Principal, ty: &TypeName) -> &'a BTreeMap<FieldName, Value> {
    static EVERY: BTreeMap<FieldName, Value> = BTreeMap::new();
    who.grants.types.get(ty).map_or(&EVERY, |t| &t.only)
}

/// Whether `fields` hold every pair of `who`'s `only` on `ty`.
pub(crate) fn admits(who: &Principal, ty: &TypeName, fields: &BTreeMap<FieldName, Value>) -> bool {
    (own_only(who, ty).iter()).all(|(f, v)| fields.get(f) == Some(v))
}

/// Whether `who`'s `only` on `decl` can be applied: each pair a text
/// value of a `text` or `reference` field the type declares, `who` may
/// read and no read computes.
pub(crate) fn only_applies(who: &Principal, decl: &TypeDecl) -> bool {
    use lakelands_api::ontology::StorageKind;
    own_only(who, &decl.name).iter().all(|(f, v)| {
        field(decl, f).is_some_and(|d| {
            matches!(d.ty.kind, StorageKind::Text | StorageKind::Reference)
                && v.is_string()
                && access(who, decl, d).reads()
                && !d.at_read()
        })
    })
}

/// The ends of a link type `decl` whose type `who` holds `only` on, with
/// the link field that names each.
fn held_ends<'d>(who: &Principal, decl: &'d TypeDecl) -> Vec<(&'d FieldName, &'d TypeName)> {
    if !crate::links::is_link_type(&decl.name) {
        return Vec::new();
    }
    let ends = (decl.fields.iter())
        .filter(|f| [crate::links::FROM, crate::links::TO].contains(&f.name.as_str()));
    let ends = ends.filter_map(|f| Some((&f.name, f.ty.target.as_ref()?)));
    ends.filter(|(_, t)| !own_only(who, t).is_empty()).collect()
}

/// Whether `who`'s grant holds `only` on `decl`, or on an end type of a
/// link type: its writes reach those records alone, and none waits for
/// review.
pub(crate) fn only_holder(who: &Principal, decl: &TypeDecl) -> bool {
    !own_only(who, &decl.name).is_empty() || !held_ends(who, decl).is_empty()
}

/// As the store (051): whether `who` can make a record of `decl` that its
/// grant admits, writing every field its `only` names at the authored door.
pub(crate) fn creates(who: &Principal, decl: &TypeDecl) -> bool {
    own_only(who, &decl.name).iter().all(|(f, _)| {
        field(decl, f).is_some_and(|d| {
            matches!(d.authority, Authority::Authored | Authority::Shared)
                && access(who, decl, d).writes()
        })
    })
}

/// Whether `who`'s grant holds `only` on any type: such a principal
/// changes no shape.
pub(crate) fn holds_only(who: &Principal) -> bool {
    who.grants.types.values().any(|t| !t.only.is_empty())
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
        upstream: e.upstream.clone(),
        origin: e.origin,
        known_at: e.known_at,
        seen_at: e.seen_at,
        position: e.position,
        writer: e.writer.clone(),
        source: e.source.clone(),
        procedure_version: e.procedure.as_ref().map(|p| p.version.clone()),
        error: None,
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

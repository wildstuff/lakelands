//! The mock's [`Catalog`]: every door, over the log.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use lakelands_api::describe::{
    ExecutorDescription, FieldDescription, Kept, LakeDescription, LakeRights, NotKept, TypeAccess,
    TypeDescription, TypeLifecycle, TypeLineage, TypeRights, VerbDescription,
};
use lakelands_api::error::{
    Case, Error, Missing, Names, Place, Refusal, Result, Revision, Said, Seam, Unmocked,
};
use lakelands_api::event::{Event, EventBody, EventId};
use lakelands_api::expr::kind::Kind;
use lakelands_api::expr::{Condition, Expr};
use lakelands_api::id::{
    Cursor, FieldName, LakeId, Position, ProposalId, RecordKey, RelationName, SourceId, TypeName,
    VerbName, Version,
};
use lakelands_api::ontology::{
    Authority, FieldDecl, RelationDecl, Risk, TypeDecl, VerbDecl, VerbEffect, VerbScope, ViewDecl,
};
use lakelands_api::principal::{
    Access, Principal, PrincipalKind, Renamed, TypeGrants, WriteContext,
};
use lakelands_api::query::{
    Axis, Hit, Hits, Pattern, Projection, QueryResult, Search, SearchMode, Walk,
};
use lakelands_api::read::{After, Direction, Find, Found, Get, Order, OrderBy, ReadAt, Reply};
use lakelands_api::record::{Origin, Patch, Record, SourceRecord, StoredRecord, Value};
use lakelands_api::seam::{
    SEARCH_WEIGHTS_SETTING, SETTING_TYPE, SORT_INDEX_SETTING, VECTOR_INDEX_SETTING,
    VIEW_KEPT_SETTING, indexable, search_weights, sortable,
};
use lakelands_api::write::{
    Carrier, Committed, Declaration, FieldConversion, FunctionDecl, Migration, MigrationReport,
    ObjectSetDecl, Outcome, Proposal, ProposalFilter, ProposalState, ProposedWrite, Recompute,
    ValueTypeMigration, WriteOpts,
};
use lakelands_api::{Catalog, CommitStream, SourceStream};

use crate::expr::{self, Scope};

mod measures;
pub(crate) mod rules;
mod views;
use crate::links;
use crate::state::{
    DeclaredType, State, access, admits, creates, door_writes, field, holds_only, only_applies,
    only_holder, own_only, pinnable,
};
use crate::{Batch, Draft, Log, Shared, refused, stream, value};

pub(crate) struct MockCatalog {
    pub shared: Arc<Shared>,
    pub context: WriteContext,
}

fn on(field: &FieldName, reason: Refusal) -> Error {
    Error::Refused {
        field: Some(field.clone()),
        reason,
    }
}

/// A refusal of `field`'s value as not well-formed.
fn invalid_field(field: &FieldName, said: impl Into<Said>) -> Error {
    on(field, Refusal::Invalid(Box::new(said.into())))
}

/// A refusal of what a constraint does not allow, at `field`.
fn constraint(field: &FieldName, said: impl Into<Said>) -> Error {
    on(field, Refusal::Constraint(Box::new(said.into())))
}

/// `said`, inside the places `at`, outermost first.
fn at(at: &[Place], said: impl Into<Said>) -> Said {
    let mut said = said.into();
    said.at.splice(0..0, at.iter().cloned());
    said
}

/// The names a read binds against the view parameters `decl` declares:
/// none unknown and none missing, as the store refuses them. The mock
/// computes no view, so it checks no value's kind.
fn bound(decl: &TypeDecl, given: &lakelands_api::Params) -> Result<()> {
    let declared = decl.view.as_ref().map_or(&[][..], |v| &v.params[..]);
    if let Some(k) = given
        .keys()
        .find(|k| !declared.iter().any(|p| &p.name == *k))
    {
        let names: Names = declared.iter().map(|p| &p.name).collect();
        let declared = (!names.0.is_empty()).then_some(names);
        let (ty, name) = (decl.name.clone(), k.clone());
        return Err(invalid(Case::ParamUnknown { ty, name, declared }));
    }
    match declared.iter().find(|p| !given.contains_key(&p.name)) {
        Some(p) => {
            let (ty, name, kind) = (decl.name.clone(), p.name.clone(), p.kind);
            Err(invalid(Case::ParamMissing {
                ty,
                name,
                kind,
                unbound: false,
            }))
        }
        None => Ok(()),
    }
}

/// The key a call passes, held to the verb's scope, as the store holds
/// it before anything else is read: a verb on its type takes no key and
/// no record version to compare, and a verb on a record needs its key.
fn scoped<'k>(
    v: &VerbDecl,
    key: Option<&'k RecordKey>,
    opts: &WriteOpts,
) -> Result<Option<&'k RecordKey>> {
    let (verb, ty) = (v.name.clone(), v.on.clone());
    match (v.scope, key) {
        (VerbScope::Type, Some(_)) => Err(invalid(Case::VerbTypeTakesNoKey { verb, ty })),
        (VerbScope::Record, None) => Err(invalid(Case::VerbNeedsKey { verb, ty })),
        (VerbScope::Type, None) if opts.expected_version.is_some() => {
            Err(invalid(Case::VerbTypeNoVersion { verb, ty }))
        }
        (_, key) => Ok(key),
    }
}

/// The record key of a call that runs a write, which only a verb on a
/// record has: its scope was checked when it was called, and a verb on its
/// type with a write effect is not declared.
fn record_key<'k>(v: &VerbDecl, key: Option<&'k RecordKey>) -> Result<&'k RecordKey> {
    scoped(v, key, &WriteOpts::default())?.ok_or_else(|| {
        let on = [Place::Verb {
            name: v.name.clone(),
        }];
        let ty = v.on.clone();
        invalid(at(&on, Case::TypeVerbWrites { ty }))
    })
}

/// A refusal of what the caller asked for as not well-formed.
fn invalid(said: impl Into<Said>) -> Error {
    Error::invalid(said)
}

fn no_type(name: &TypeName) -> Error {
    Error::NotFound {
        missing: Missing::Type { name: name.clone() },
    }
}

fn no_record(ty: &TypeName, key: &RecordKey) -> Error {
    Error::NotFound {
        missing: Missing::Record {
            ty: ty.clone(),
            key: key.clone(),
        },
    }
}

/// The changes of a revision of a type that would reshape stored records,
/// so that it needs `migrate` (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`),
/// by the store's own reasons and in its order: a field removed, retyped or
/// made required, a new field required (a link's fields as well); the key
/// changed; a relation removed or retargeted, or its cardinality narrowed;
/// a stored type turned into a view. A change of meaning, an added optional
/// field or relation, a loosened requirement or cardinality, is a
/// declaration; and a view stores nothing, so every change of one is, a
/// view ceasing to be one included. Empty: it is declared.
fn reshaping(old: &TypeDecl, new: &TypeDecl) -> Vec<Revision> {
    use lakelands_api::ontology::Cardinality;
    let mut out = Vec::new();
    if old.view.is_some() {
        return out;
    }
    fields_reshaped(&old.fields, &new.fields, None, &mut out);
    if old.key != new.key {
        let (from, to) = (old.key.iter().collect(), new.key.iter().collect());
        out.push(Revision::KeyChanged { from, to });
    }
    // Whether each side of a relation may link many: (this side, target).
    let sides = |c: Cardinality| match c {
        Cardinality::OneToOne => (false, false),
        Cardinality::OneToMany => (false, true),
        Cardinality::ManyToOne => (true, false),
        Cardinality::ManyToMany => (true, true),
    };
    let mut relations: Vec<&RelationDecl> = old.relations.iter().collect();
    relations.sort_by(|a, b| a.name.cmp(&b.name));
    for o in relations {
        let relation = o.name.clone();
        let Some(n) = new.relations.iter().find(|n| n.name == o.name) else {
            out.push(Revision::RelationRemoved { relation });
            continue;
        };
        if n.target != o.target {
            let (from, to) = (o.target.clone(), n.target.clone());
            let relation = relation.clone();
            out.push(Revision::RelationRetargeted { relation, from, to });
        }
        let ((a, b), (c, d)) = (sides(o.cardinality), sides(n.cardinality));
        if !((!a || c) && (!b || d)) {
            let (from, to) = (o.cardinality, n.cardinality);
            let relation = relation.clone();
            out.push(Revision::CardinalityChanged { relation, from, to });
        }
        fields_reshaped(&o.fields, &n.fields, Some(&relation), &mut out);
    }
    if new.view.is_some() {
        out.push(Revision::BecameView);
    }
    out
}

/// The field changes that reshape stored records, in field-name order.
fn fields_reshaped(
    old: &[FieldDecl],
    new: &[FieldDecl],
    relation: Option<&RelationName>,
    out: &mut Vec<Revision>,
) {
    let by_name = |fs: &[FieldDecl]| {
        let mut fs: Vec<FieldDecl> = fs.to_vec();
        fs.sort_by(|a, b| a.name.cmp(&b.name));
        fs
    };
    let (old, new) = (by_name(old), by_name(new));
    let relation = || relation.cloned();
    for o in &old {
        let field = o.name.clone();
        let Some(n) = new.iter().find(|n| n.name == o.name) else {
            let relation = relation();
            out.push(Revision::FieldRemoved { field, relation });
            continue;
        };
        if n.ty != o.ty {
            let (from, to) = (o.ty.clone(), n.ty.clone());
            let (field, relation) = (field.clone(), relation());
            out.push(Revision::FieldRetyped {
                field,
                relation,
                from,
                to,
            });
        }
        if n.required && !o.required {
            let relation = relation();
            out.push(Revision::FieldRequired { field, relation });
        }
    }
    for n in new.iter().filter(|n| n.required) {
        if !old.iter().any(|o| o.name == n.name) {
            let (field, relation) = (n.name.clone(), relation());
            let required = true;
            out.push(Revision::FieldAdded {
                field,
                relation,
                required,
            });
        }
    }
}

/// Whether declaring a type of this name is reserved: a `sys.` type is
/// declared only by the system. Records of a `sys.` type are written by
/// whoever the grants allow, like any other.
fn reserved(name: &str, who: &Principal) -> bool {
    name.starts_with("sys.") && who.kind != PrincipalKind::System
}

/// A page token: the position the first page was read at, and how many
/// rows came before this page.
/// A cursor for the page at `offset` of an answer read at `p` through
/// `door` on `ty`, by the read whose digest is `of` ([`read_of`]): it
/// continues that answer and no other (018).
fn cursor(p: Position, offset: usize, door: &str, ty: Option<&TypeName>, of: &str) -> Cursor {
    let ty = ty.map_or("", TypeName::as_str);
    Cursor(format!("{}.{offset}.{door}.{of}.{ty}", p.0))
}

/// The digest of what decides a read's rows and their order, as the
/// store keeps one in its cursors; the mock's lives as long as its lake.
fn read_of(shape: serde_json::Result<String>) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    shape.unwrap_or_default().hash(&mut h);
    format!("{:016x}", h.finish())
}

/// A cursor's position and offset, when it was handed out for `door` on
/// `ty` by the read `of`; any other use is refused, naming what differs.
fn uncursor(c: &Cursor, door: &str, ty: Option<&TypeName>, of: &str) -> Result<(Position, usize)> {
    let mut parts = c.0.splitn(5, '.');
    let (Some(p), Some(o), Some(d), Some(f), Some(t)) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Err(invalid(Case::CursorForeign));
    };
    let (Ok(p), Ok(o)) = (p.parse(), o.parse()) else {
        return Err(invalid(Case::CursorForeign));
    };
    let here = ty.map_or("", TypeName::as_str);
    if d != door || t != here {
        let named = |t: &str| (!t.is_empty()).then(|| Box::new(TypeName::from(t)));
        let (was, was_ty) = (d.to_owned(), named(t));
        let (door, ty) = (door.to_owned(), ty.cloned().map(Box::new));
        return Err(invalid(Case::CursorOtherDoor {
            was,
            was_ty,
            door,
            ty,
        }));
    }
    if f != of {
        let door = door.to_owned();
        return Err(invalid(Case::CursorOtherRead { door }));
    }
    Ok((Position(p), o))
}

/// The one order of `find`: sort values in turn, a missing value last in
/// either direction, then the key ascending.
fn in_order(
    order: &[Order],
    (a, ak): (&[Option<Value>], &RecordKey),
    (b, bk): (&[Option<Value>], &RecordKey),
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    for (o, (x, y)) in order.iter().zip(a.iter().zip(b)) {
        let ord = match (x, y) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(x), Some(y)) => {
                let c = expr::compare(x, y).unwrap_or(Ordering::Equal);
                if o.dir == Direction::Desc {
                    c.reverse()
                } else {
                    c
                }
            }
        };
        if ord.is_ne() {
            return ord;
        }
    }
    ak.cmp(bk)
}

/// The sort position a caller's `after` names, refused as the store
/// refuses it: beside a cursor or an aggregate, with a value count other
/// than `order_by`'s, or with a value the key's field cannot hold.
fn resume_after(
    st: &State,
    t: &DeclaredType,
    find: &Find,
    a: &After,
) -> Result<(Vec<Option<Value>>, RecordKey)> {
    if find.cursor.is_some() {
        return Err(invalid(Case::AfterWithCursor));
    }
    if find.aggregate.is_some() {
        return Err(invalid(Case::AfterWithAggregate));
    }
    if a.values.len() != find.order_by.len() {
        let (values, keys) = (a.values.len(), find.order_by.len());
        return Err(invalid(Case::AfterCount { values, keys }));
    }
    let mut values = Vec::with_capacity(a.values.len());
    for (i, (v, o)) in a.values.iter().zip(&find.order_by).enumerate() {
        let unfit = |why: Case| {
            let (index, field, why) = (i, o.field.clone(), Box::new(why));
            invalid_field(&o.field, Case::AfterUnfit { index, field, why })
        };
        let v = match (o.by, v) {
            (_, Value::Null) => None,
            (OrderBy::Latest, Value::Number(n)) if n.is_u64() => Some(v.clone()),
            (OrderBy::Latest, _) => {
                return Err(unfit(Case::PositionNotInteger));
            }
            (OrderBy::KnownAt | OrderBy::SeenAt, Value::String(s)) => {
                let value = || Case::NotDateTime { value: s.clone() };
                let t = lakelands_api::Timestamp::parse(s).map_err(|_| unfit(value()))?;
                Some(t.to_string().into())
            }
            (OrderBy::KnownAt | OrderBy::SeenAt, _) => {
                return Err(unfit(Case::InstantNotText));
            }
            (OrderBy::Value, v) => match field(&t.decl, &o.field) {
                Some(f) => {
                    if let Some(why) = value::misfit(&f.ty, v, &st.value_types) {
                        return Err(unfit(why));
                    }
                    Some(value::canonical(&f.ty, v))
                }
                None => Some(v.clone()),
            },
        };
        values.push(v);
    }
    Ok((values, a.key.clone()))
}

/// A page of `rows` from `offset`, and the cursor after it.
fn page<T>(
    rows: Vec<T>,
    (at, door, ty, of): (Position, &str, Option<&TypeName>, &str),
    offset: usize,
    limit: usize,
) -> (Vec<T>, Option<Cursor>) {
    let more = rows.len() > offset + limit;
    let rows: Vec<T> = rows.into_iter().skip(offset).take(limit).collect();
    (rows, more.then(|| cursor(at, offset + limit, door, ty, of)))
}

impl MockCatalog {
    fn log(&self) -> std::sync::MutexGuard<'_, Log> {
        self.shared.log()
    }

    /// Refuse a principal of another lake.
    fn admit(&self, who: &Principal) -> Result<()> {
        if who.lake != self.shared.lake {
            return Err(refused(Refusal::NotGranted));
        }
        Ok(())
    }

    /// As the store: a principal with its grants held through the lake's
    /// renames (`Grants::follow`), for a read at `at` or a write at the head.
    /// The list is every `migrated` event's renames, in the log's order.
    /// Then each reference to a type it is not shown, by the lake at `at`,
    /// is held at `none` ([`State::hidden`]).
    fn followed(&self, who: &Principal, at: Option<Position>) -> Principal {
        let mut who = who.clone();
        who.grants.follow(&self.renames(), at);
        if !hides_some(&who) {
            return who;
        }
        let log = self.log();
        let st = (self.state_at(&log, at)).or_else(|_| self.state_at(&log, None));
        st.map_or(who.clone(), |(_, st)| st.hidden(&who))
    }

    /// As the store: the lake's list of renames, every `migrated` event's
    /// in the log's order: its field renames, then each link type a
    /// renamed relation moves.
    fn renames(&self) -> Vec<Renamed> {
        let renamed = |e: &Event| {
            let EventBody::Migrated { migration, .. } = &e.body else {
                return Vec::new();
            };
            let (at, ty) = (e.position, &migration.to.name);
            let fields = (!migration.renames.is_empty()).then(|| Renamed {
                at,
                ty: ty.clone(),
                renames: migration.renames.clone(),
                to: None,
            });
            let names = moved(migration).into_iter().map(|(ty, to)| Renamed {
                at,
                ty,
                to: Some(to),
                ..Renamed::default()
            });
            fields.into_iter().chain(names).collect()
        };
        self.log().events.iter().flat_map(renamed).collect()
    }

    /// Admit a write: also refuse a write deeper in a chain of reactions
    /// than the lake allows, whatever origin its context names.
    fn admit_write(&self, who: &Principal) -> Result<()> {
        self.admit(who)?;
        let max = self.shared.limits.max_reactive_depth;
        if self.context.depth > max {
            return Err(refused(Refusal::TooDeep { max }));
        }
        // A host's cause names an event the log holds; a trigger's firing
        // binds the reaction by that trigger's own ceiling too (021).
        if let Some(cause) = self.context.cause {
            let log = self.log();
            let head = Shared::head(&log);
            if cause > head {
                let head = Some(head);
                return Err(invalid(Case::CausePastHead { cause, head }));
            }
            if let EventBody::TriggerFired { trigger, .. } = &log.events[cause.0 as usize].body
                && let Some(m) = State::fold(&log.events)
                    .triggers
                    .iter()
                    .find(|t| &t.name == trigger)
                    .and_then(|t| t.max_depth)
                && self.context.depth > m
            {
                return Err(Error::Refused {
                    field: Some(trigger.as_str().into()),
                    reason: Refusal::TooDeep { max: m },
                });
            }
        }
        Ok(())
    }

    /// The position a read is answered at, and the lake folded up to it.
    fn state_at(&self, log: &Log, as_of: Option<Position>) -> Result<(Position, State)> {
        let head = Shared::head(log);
        let at = as_of.unwrap_or(head);
        if at > head {
            return Err(refused(Refusal::OutOfRange { head }));
        }
        Ok((at, State::fold(&log.events[..=at.0 as usize])))
    }

    /// The page a read asks for, refused outside the lake's bounds with
    /// the store's sentence; a measure's may be as large as the lake's
    /// `max_graph_nodes`, as the store's (026).
    fn limit(&self, asked: Option<u32>, measure: bool) -> Result<usize> {
        let l = &self.shared.limits;
        let max = if measure {
            l.max_limit.max(l.max_graph_nodes)
        } else {
            l.max_limit
        };
        let asked = asked.unwrap_or(l.default_limit.min(max));
        if asked == 0 || asked > max {
            return Err(invalid(Case::LimitRange { max }));
        }
        Ok(asked as usize)
    }

    fn commit(&self, log: &mut Log, batch: Batch) {
        self.shared.append(log, self.context, batch);
    }

    /// One declaration checked and appended to `log`, which the caller
    /// holds locked; answers its position.
    fn declare_in(
        &self,
        log: &mut Log,
        who: &Principal,
        declaration: Declaration,
    ) -> Result<Position> {
        let (head, st) = self.state_at(log, None)?;
        let mut declaration = declaration;
        redeclared(&st, who, std::slice::from_mut(&mut declaration))?;
        check_declaration(&st, who, &declaration)?;
        trigger_ceiling(&declaration, self.shared.limits.max_reactive_depth)?;
        // As the store: the declaration in force appends nothing.
        if st.in_force(&declaration) {
            return Ok(head);
        }
        let (schema_hash, version) = identity_version(&st, &declaration);
        let links = link_drafts(&st, who, &declaration);
        let mut batch = Batch::at(log);
        let position = batch.push(Draft::new(
            who,
            Origin::System,
            EventBody::Declared {
                declaration,
                schema_hash,
                version,
                with_the_set: false,
            },
        ));
        for d in links {
            batch.push(d);
        }
        self.commit(log, batch);
        Ok(position)
    }
}

/// The link types a type's declaration calls for that differ from those
/// the lake holds, declared in the same append.
fn link_drafts(st: &State, who: &Principal, d: &Declaration) -> Vec<Draft> {
    let Declaration::Type(t) = d else {
        return Vec::new();
    };
    links::of(t)
        .into_iter()
        .filter(|lt| st.types.get(&lt.name).map(|e| &e.decl) != Some(lt))
        .map(|lt| {
            let declaration = Declaration::Type(lt);
            let (schema_hash, version) = identity_version(st, &declaration);
            Draft::new(
                who,
                Origin::System,
                EventBody::Declared {
                    declaration,
                    schema_hash,
                    version,
                    with_the_set: false,
                },
            )
        })
        .collect()
}

/// The relation of `ty` carried by link records named `relation`.
fn link_relation(
    st: &State,
    who: &Principal,
    ty: &TypeName,
    relation: &RelationName,
) -> Result<lakelands_api::ontology::RelationDecl> {
    // As the store: an undeclared type, or one the principal is not shown,
    // is not found; a relation it does not carry by link records, or is
    // not shown, is invalid; one the principal may not write the links of
    // is not granted.
    let t = st.visible(who, ty).ok_or_else(|| no_type(ty))?;
    not_a_view(&t.decl)?;
    let r = t
        .decl
        .relations
        .iter()
        .find(|r| &r.name == relation && r.via.is_none() && r.derived.is_none())
        .filter(|r| relation_shown(st, who, &t.decl, r))
        .cloned()
        .ok_or_else(|| {
            let (ty, relation) = (ty.clone(), relation.clone());
            invalid(Case::NoLinkRelation { ty, relation })
        })?;
    if st.visible(who, &links::name(ty, relation)).is_none() {
        return Err(refused(Refusal::NotGranted));
    }
    Ok(r)
}

/// As the store: a view at a door that writes or holds a record is
/// refused in one sentence.
fn not_a_view(decl: &TypeDecl) -> Result<()> {
    match decl.view {
        Some(_) => Err(invalid(Case::IsAView {
            ty: decl.name.clone(),
        })),
        None => Ok(()),
    }
}

/// The relation's cardinality over live links, the link from `from` to
/// `to` among them; the other end named only where `who` is shown it.
fn link_cardinality(
    st: &State,
    who: &Principal,
    lt: &TypeName,
    r: &lakelands_api::ontology::RelationDecl,
    from: &RecordKey,
    to: &RecordKey,
) -> Result<()> {
    use lakelands_api::ontology::Cardinality;
    let (per_from, per_to) = match r.cardinality {
        Cardinality::ManyToOne => (true, false),
        Cardinality::OneToMany => (false, true),
        Cardinality::OneToOne => (true, true),
        Cardinality::ManyToMany => (false, false),
    };
    let end =
        |l: &StoredRecord, f: &str| l.fields.get(f).and_then(Value::as_str).map(str::to_owned);
    for ((t, _), l) in &st.records {
        if t != lt || l.retired {
            continue;
        }
        let (f, o) = (end(l, links::FROM), end(l, links::TO));
        if f.as_deref() == Some(from.as_str()) && o.as_deref() == Some(to.as_str()) {
            continue;
        }
        // The store's case, so a transcript reads the same on both.
        let (relation, cardinality) = (r.name.clone(), r.cardinality);
        let key = |ty: &TypeName, k: Option<String>| {
            let k = RecordKey::from(k.unwrap_or_default());
            let r = st.records.get(&(ty.clone(), k.clone()));
            let shown = r.is_some_and(|r| {
                (who.grants.restricted || !r.restricted) && admits(who, ty, &r.fields)
            });
            shown.then_some(k)
        };
        let why = if per_from && f.as_deref() == Some(from.as_str()) {
            let (from, to) = (from.clone(), key(&r.target, o));
            Some(Case::LinkedAlreadyFrom {
                from,
                relation,
                to,
                cardinality,
            })
        } else if per_to && o.as_deref() == Some(to.as_str()) {
            let ends = st
                .types
                .values()
                .find(|t| &links::name(&t.decl.name, &r.name) == lt);
            let ty = ends.map(|t| t.decl.name.clone()).unwrap_or_default();
            let (to, from) = (to.clone(), key(&ty, f));
            Some(Case::LinkedAlreadyTo {
                to,
                relation,
                from,
                cardinality,
            })
        } else {
            None
        };
        if let Some(why) = why {
            return Err(constraint(&FieldName::from(links::TO), why));
        }
    }
    Ok(())
}

/// A declaration's identity in a set: its kind and its name.
fn identity(d: &Declaration) -> String {
    let v = serde_json::to_value(d).unwrap_or_default();
    format!(
        "{} {}",
        v["declare"].as_str().unwrap_or_default(),
        v["name"].as_str().unwrap_or_default()
    )
}

/// The schema hash (for a type) and the version a declaration takes in
/// the lake as `st` holds it.
fn identity_version(st: &State, d: &Declaration) -> (Option<String>, u32) {
    match d {
        Declaration::Type(t) => (
            lakelands_api::shape::schema_hash(t).ok(),
            st.types.get(&t.name).map_or(1, |old| old.version + 1),
        ),
        other => {
            let v = serde_json::to_value(other).unwrap_or_default();
            let key = format!(
                "{}:{}",
                v["declare"].as_str().unwrap_or_default(),
                v["name"].as_str().unwrap_or_default()
            );
            (None, st.decl_versions.get(&key).map_or(1, |v| v + 1))
        }
    }
}

/// A type's embeddings: refused as invalid when declared against the
/// rules, as the store does first, and otherwise as unbound, since the
/// mock binds no embedding backend.
fn embeddings(t: &lakelands_api::ontology::TypeDecl) -> Result<()> {
    if let Some((field, why)) = t.embedding_problem() {
        return Err(invalid_field(&field, why));
    }
    if let Some((f, e)) = t.fields.iter().find_map(|f| Some((f, f.embed.as_ref()?))) {
        return Err(Error::unbound(Seam::Embedding {
            model: e.model.clone(),
            dimensions: f.ty.dimensions.unwrap_or(0),
            ty: t.name.clone(),
            field: f.name.to_string(),
            bound: None,
        }));
    }
    Ok(())
}

/// Check one declaration against the lake as `st` holds it: reserved
/// names, a revision that would reshape records, every type it refers to
/// declared (or itself), and every name able to become a tool.
/// A value type's new declaration, checked as a declaration of its own:
/// the one in force would refuse it as a tightening.
fn check_new_value_type(
    st: &State,
    who: &Principal,
    to: &lakelands_api::ontology::ValueTypeDecl,
) -> Result<()> {
    let mut probe = st.clone();
    probe.value_types.retain(|v| v.name != to.name);
    check_declaration(&probe, who, &Declaration::ValueType(to.clone()))
}

fn check_declaration(st: &State, who: &Principal, d: &Declaration) -> Result<()> {
    let mut types: Vec<TypeName> = st.types.keys().cloned().collect();
    let mut verbs: Vec<VerbName> = st.verbs.keys().cloned().collect();
    let known =
        |ty: &TypeName, own: Option<&TypeName>| own == Some(ty) || st.types.contains_key(ty);
    if let Some(fault) = d.annotation_fault() {
        return Err(fault.into());
    }
    rules::declaration(st, d)?;
    check_initial(st, d)?;
    if let Declaration::Type(t) = d {
        embeddings(t)?;
        let function = |n: &str| st.functions.iter().find(|f| f.name == n);
        if let Some((field, why)) = t.computed_at_problem(function) {
            return Err(invalid_field(&field, why));
        }
        read_as_stored(st, t)?;
        // As the store: a derived field's expression can be evaluated, and
        // what it computes can land in the field.
        for f in t.fields.iter().filter(|_| t.view.is_none()) {
            let Some(e) = f.derived.as_ref().and_then(|c| c.typed().ok()) else {
                continue;
            };
            let on = [of_type(&t.name), of_field(&f.name)];
            if let Some(kind) = evaluates(&e, t, &on)?.filter(|k| !k.lands(f.ty.kind)) {
                let (holds, duration) = (f.ty.kind, kind == Kind::Duration);
                let case = Case::NeverLands {
                    kind,
                    holds,
                    duration,
                };
                return Err(invalid_field(&f.name, at(&on, case)));
            }
        }
    }
    if let Declaration::ValueType(v) = d {
        crate::valuetypes::check_declared(st, v)?;
    }
    match d {
        // The mock binds no function backing; `expr` is computed inside.
        Declaration::Function(f) if f.backing != "expr" => {
            return Err(Error::unbound(Seam::FunctionBacking {
                kind: f.backing.clone(),
            }));
        }
        Declaration::Type(t) => {
            if reserved(t.name.as_str(), who) {
                return Err(refused(Refusal::Reserved));
            }
            shape_names(t)?;
            views::check(st, t)?;
            // A view is hashed over its conditions' typed form, so one that
            // does not parse has no schema hash and is not declared.
            lakelands_api::shape::schema_hash(t).map_err(invalid)?;
            if !st.types.contains_key(&t.name) {
                types.push(t.name.clone());
            }
            for f in &t.fields {
                if let Some(target) = &f.ty.target
                    && !known(target, Some(&t.name))
                {
                    let on = [of_type(&t.name), of_field(&f.name)];
                    let ty = target.clone();
                    return Err(invalid_field(&f.name, at(&on, Case::Undeclared { ty })));
                }
            }
            for r in &t.relations {
                let name = r.name.clone();
                let on = [of_type(&t.name), Place::Relation { name }];
                let wrong = |c: Case| invalid(at(&on, c));
                // As the store: a view's records are computed, so only a
                // relation derived from matching values starts from one.
                if t.view.is_some() && r.derived.is_none() {
                    let (ty, carrier) = (t.name.clone(), carrier(r));
                    return Err(wrong(Case::ViewRelation { ty, carrier }));
                }
                // As the store: a `via` names a field of the type.
                if let Some(via) = r
                    .via
                    .as_ref()
                    .filter(|v| !t.fields.iter().any(|f| &f.name == *v))
                {
                    let (ty, field) = (t.name.clone(), via.clone());
                    return Err(wrong(Case::UnknownField { ty, field }));
                }
                if r.via.is_some() && !r.fields.is_empty() {
                    return Err(wrong(Case::ViaWithFields));
                }
                if !matches!(r.authority, Authority::Authored | Authority::Source) {
                    let authority = r.authority;
                    return Err(wrong(Case::LinkAuthority { authority }));
                }
                if r.authority == Authority::Source
                    && let Some(f) = (r.fields.iter())
                        .find(|f| !matches!(f.authority, Authority::Source | Authority::Derived))
                {
                    let (field, authority) = (f.name.clone(), f.authority);
                    let case = Case::LinkFieldAuthority { field, authority };
                    return Err(invalid_field(&f.name, at(&on, case)));
                }
                if let Some(f) = (r.fields.iter())
                    .find(|f| f.name.as_str() == links::FROM || f.name.as_str() == links::TO)
                {
                    let field = f.name.clone();
                    return Err(wrong(Case::LinkFieldIsEnd { field }));
                }
                if let Some(before) = st
                    .types
                    .get(&t.name)
                    .and_then(|o| o.decl.relations.iter().find(|b| b.name == r.name))
                    && carrier(before) != carrier(r)
                {
                    let (from, to) = (carrier(before), carrier(r));
                    return Err(wrong(Case::CarrierChanged { from, to }));
                }
                if !known(&r.target, Some(&t.name)) {
                    let ty = r.target.clone();
                    return Err(wrong(Case::Undeclared { ty }));
                }
                if let Some(d) = &r.derived {
                    derived_pairs(st, t, r, d)?;
                }
            }
            // As the store: a reshape is judged after the shape's own checks,
            // so a migration that is forgiven it is still checked whole.
            if let Some(old) = st.types.get(&t.name) {
                let reasons = reshaping(&old.decl, t);
                if !reasons.is_empty() {
                    let ty = t.name.clone();
                    return Err(invalid(Case::Reshapes { ty, reasons }));
                }
            }
            // Its link types too, as the store: a relation dropped by a
            // migration and declared again with another target would
            // reshape the links it kept.
            for lt in links::of(t) {
                if let Some(old) = st.types.get(&lt.name) {
                    let reasons = reshaping(&old.decl, &lt);
                    if !reasons.is_empty() {
                        let (ty, links) = (t.name.clone(), lt.name.clone());
                        return Err(invalid(Case::ReshapesLinks { ty, links, reasons }));
                    }
                }
            }
        }
        other => {
            let target = match other {
                Declaration::Verb(v) => {
                    // As the store: a verb on its type has no record, so
                    // nothing for a write effect to set or `requires` to read.
                    if v.scope == VerbScope::Type {
                        let (on, ty) = ([d.place()], v.on.clone());
                        if matches!(v.effect, VerbEffect::Write { .. }) {
                            return Err(invalid(at(&on, Case::TypeVerbWrites { ty })));
                        }
                        if v.requires.is_some() {
                            return Err(invalid(at(&on, Case::TypeVerbRequires { ty })));
                        }
                    }
                    // A revision of a verb the lake holds is not a second name.
                    if !verbs.contains(&v.name) {
                        verbs.push(v.name.clone());
                    }
                    Some(&v.on)
                }
                Declaration::Trigger(t) => Some(&t.on),
                Declaration::ObjectSet(s) => Some(&s.of),
                _ => None,
            };
            // As the store: the link type of a relation carried by a
            // reference field does not exist; its links change with it.
            if let Declaration::Trigger(t) = other
                && let Some((owner, relation)) = links::parts(&t.on)
                && let Some(via) = st.types.get(&owner).and_then(|o| {
                    o.decl
                        .relations
                        .iter()
                        .find(|r| r.name == relation)
                        .and_then(|r| r.via.clone())
                })
            {
                let (ty, owner) = (t.on.clone(), Box::new(owner));
                let case = Case::WatchesViaLinks {
                    ty,
                    relation,
                    via,
                    owner,
                };
                return Err(invalid_field(&"on".into(), at(&[d.place()], case)));
            }
            if let Some(ty) = target {
                // As the store: a declaration that names an undeclared
                // type is invalid, naming both.
                if !st.types.contains_key(ty) {
                    let case = Case::Undeclared { ty: ty.clone() };
                    return Err(invalid(at(&[d.place()], case)));
                }
                // As the store: a view is never written, so a trigger on
                // one never fires, and an object set holds stored records.
                if st.types.get(ty).is_some_and(|o| o.decl.view.is_some()) {
                    let (on, ty) = ([d.place()], ty.clone());
                    match other {
                        Declaration::Trigger(_) => {
                            let case = at(&on, Case::WatchesView { ty });
                            return Err(invalid_field(&"on".into(), case));
                        }
                        Declaration::ObjectSet(_) => {
                            let case = at(&on, Case::SetOfView { ty });
                            return Err(invalid_field(&"of".into(), case));
                        }
                        // A verb on a record of a view could never act.
                        Declaration::Verb(v) if v.scope == VerbScope::Record => {
                            return Err(invalid(at(&on, Case::VerbOnView { ty })));
                        }
                        _ => {}
                    }
                }
                // As the store: a trigger may watch any declared type, and an
                // object set may hold its records, the link types of
                // relations carried by link records among them.
                let on_any = matches!(other, Declaration::Trigger(_) | Declaration::ObjectSet(_));
                if reserved(ty.as_str(), who) && !on_any {
                    return Err(refused(Refusal::Reserved));
                }
            }
        }
    }
    reads_its_type(st, d)?;
    let mut link_relations: Vec<(TypeName, RelationName)> = st
        .types
        .values()
        .filter(|o| !matches!(d, Declaration::Type(t) if t.name == o.decl.name))
        .flat_map(|o| {
            o.decl
                .relations
                .iter()
                .filter(|r| r.via.is_none() && r.derived.is_none())
                .map(|r| (o.decl.name.clone(), r.name.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    if let Declaration::Type(t) = d {
        link_relations.extend(
            t.relations
                .iter()
                .filter(|r| r.via.is_none() && r.derived.is_none())
                .map(|r| (t.name.clone(), r.name.clone())),
        );
    }
    let problems = lakelands_api::tools::name_problems(&types, &verbs, &link_relations);
    if !problems.is_empty() {
        return Err(invalid(Case::ToolNames { problems }));
    }
    Ok(())
}

// ---- describing ----

fn authored_writable(who: &Principal, t: &DeclaredType) -> bool {
    t.decl.fields.iter().any(|f| {
        matches!(f.authority, Authority::Authored | Authority::Shared)
            && access(who, &t.decl, f).writes()
    })
}

/// Whether a principal may retire records of a type: only if its grants
/// let it write every field of the type, whatever the field's authority.
fn may_retire(who: &Principal, t: &DeclaredType) -> bool {
    t.decl
        .fields
        .iter()
        .all(|f| who.grants.access(&t.decl.name, &f.name).writes())
}

fn callable(st: &State, who: &Principal, v: &VerbDecl) -> bool {
    // A holder of `only` calls no verb that waits for review and could
    // move its pairs (051).
    let held = |t: &&DeclaredType| only_holder(who, &t.decl) && !pinnable(who, &t.decl, Some(v));
    let Some(t) = (st.visible(who, &v.on)).filter(|t| !held(t) || v.risk == Risk::Low) else {
        return false;
    };
    match &v.effect {
        VerbEffect::Write { set } => set
            .iter()
            .all(|a| field(&t.decl, &a.field).is_some_and(|f| access(who, &t.decl, f).writes())),
        // Carrying it out writes nothing of the record, so it asks for
        // write access to the type, as the store does.
        VerbEffect::External { .. } => who
            .grants
            .types
            .get(&v.on)
            .and_then(|t| t.default)
            .unwrap_or(who.grants.default)
            .writes(),
    }
}

/// As the store: whether `who` is shown relation `r` of `owner`: both ends
/// shown, the reference field that carries it known, and of a derived
/// relation every matched field known on its side.
pub(crate) fn relation_shown(
    st: &State,
    who: &Principal,
    owner: &TypeDecl,
    r: &RelationDecl,
) -> bool {
    let Some(target) = st.visible(who, &r.target) else {
        return false;
    };
    let known = |t: &TypeDecl, n: &FieldName| {
        field(t, n).is_some_and(|f| access(who, t, f) != Access::None)
    };
    let mut pairs = r.derived.iter().flat_map(|d| &d.pairs);
    st.visible(who, &owner.name).is_some()
        && r.via.iter().all(|v| known(owner, v))
        && pairs.all(|p| known(owner, &p.from) && known(&target.decl, &p.to))
}

fn describe_type(st: &State, who: &Principal, t: &DeclaredType) -> TypeDescription {
    let d = &t.decl;
    let fields: Vec<FieldDescription> = d
        .fields
        .iter()
        .filter_map(|f| match access(who, d, f) {
            Access::None => None,
            a => Some(FieldDescription::of(f, a)),
        })
        .collect();
    // As the store: over the fields as shown.
    let searchable = fields.iter().any(|f| f.searchable);
    // As the store: a valid time over a field the principal may not know
    // of is not named.
    let named = |v: &FieldName| fields.iter().any(|f| &f.name == v);
    let (valid_from, valid_to) = (
        d.valid_from.clone().filter(named),
        d.valid_to.clone().filter(named),
    );
    let sourced =
        (fields.iter()).any(|f| matches!(f.authority, Authority::Source | Authority::Shared));
    let visible = |names: Vec<FieldName>| -> Vec<FieldName> {
        names
            .into_iter()
            .filter(|f| fields.iter().any(|d| d.name == *f))
            .collect()
    };
    let sorted = visible(index_fields(st, d, SORT_INDEX_SETTING, sortable));
    let indexed = visible(index_fields(st, d, VECTOR_INDEX_SETTING, indexable));
    // The mock keeps no view: one that is asked for and could be kept is
    // not kept here.
    let kept = d.view.as_deref().map(|v| {
        // As the store: to a holder of `only` a view is its own records,
        // never every owner's groups (051).
        if st.only_some(who, d, &mut Vec::new()) {
            return Kept::Computed {
                why: NotKept::OnlySome,
            };
        }
        let key = RecordKey::from(format!("{VIEW_KEPT_SETTING}{}", d.name));
        let setting = st.live(&TypeName::from(SETTING_TYPE), &key);
        let (value, shown) = (
            setting.and_then(|r| r.fields.get("value")),
            view_visible(st, who, v),
        );
        Kept::by_declaration(v, value, shown, |t| st.types.get(t).map(|t| &t.decl))
    });
    // As the store: an index is named where the view is kept, or asked to
    // be and can be, as `kept` tells this reader.
    let sorted = match &kept {
        Some(k) if !k.names_its_index() => Vec::new(),
        _ => sorted,
    };
    TypeDescription {
        annotations: d.annotations.clone(),
        name: d.name.clone(),
        description: d.description.clone(),
        label: d.label.clone(),
        language: d.language.clone(),
        view: d
            .view
            .clone()
            .filter(|v| view_visible(st, who, v))
            .map(|v| *v),
        fields,
        relations: d
            .relations
            .iter()
            .filter(|r| relation_shown(st, who, d, r))
            .map(|r| {
                // As the store: a relation is described where it is shown.
                // A link field the principal may not read is
                // described by its shape alone, as a type's own field is.
                let link = crate::links::link_type(&d.name, r);
                let mut r = r.clone();
                r.fields.retain(|f| access(who, &link, f) != Access::None);
                for f in &mut r.fields {
                    if !access(who, &link, f).reads() {
                        f.shape_alone();
                    }
                }
                r
            })
            .collect(),
        access: TypeAccess {
            key: d.key.clone(),
            valid_from,
            valid_to,
            searchable,
            field_meta: d.field_meta,
        },
        may: TypeRights {
            find: true,
            get: true,
            upsert: creates(who, d) && !links::is_link_type(&d.name) && authored_writable(who, t),
            retire: !links::is_link_type(&d.name) && may_retire(who, t),
            link: d
                .relations
                .iter()
                .filter(|r| {
                    r.via.is_none()
                        && r.derived.is_none()
                        && r.authority == Authority::Authored
                        && relation_shown(st, who, d, r)
                })
                .filter(|r| {
                    let lt = links::link_type(&d.name, r);
                    lt.fields[..2].iter().all(|f| access(who, &lt, f).writes())
                })
                .map(|r| r.name.clone())
                .collect(),
            // Told only where it applies, so it names no field its reader
            // may not read.
            only: if only_applies(who, d) {
                own_only(who, &d.name).clone()
            } else {
                Default::default()
            },
        },
        verbs: st
            .verbs
            .values()
            .filter(|v| v.on == d.name && callable(st, who, v))
            .map(|v| v.name.clone())
            .collect(),
        lineage: TypeLineage {
            sourced,
            view_of: d.view.as_ref().map(|v| v.source.clone()),
        },
        lifecycle: TypeLifecycle {
            version: t.version,
            schema_hash: shown_hash(
                (st, who),
                d,
                d.view.as_deref().is_none_or(|v| view_visible(st, who, v)),
                &t.schema_hash,
            ),
            sorted,
            indexed,
            kept,
        },
    }
}

/// Declarations listed in name order, as the store lists them.
fn by_name<T>(items: impl Iterator<Item = T>, name: impl Fn(&T) -> String) -> Vec<T> {
    let mut v: Vec<T> = items.collect();
    v.sort_by_key(|x| name(x));
    v
}

/// Whether `who` reads every one of `names` on type `ty`.
fn reads_all<'a>(
    st: &State,
    who: &Principal,
    ty: &TypeName,
    names: impl IntoIterator<Item = &'a FieldName>,
) -> bool {
    let Some(t) = st.types.get(ty) else {
        return false;
    };
    names
        .into_iter()
        .all(|n| field(&t.decl, n).is_some_and(|f| access(who, &t.decl, f).reads()))
}

/// As the store: a condition over `ty`'s records with the filter of each
/// set it names (`in_set('name')`) in the name's place. `set` answers a
/// set by its name, as its caller holds it.
fn expanded<'s>(
    set: &dyn Fn(&str) -> Option<&'s ObjectSetDecl>,
    ty: &TypeName,
    e: &Expr,
    on: &[Place],
) -> Result<Expr> {
    let named = |name: &str| {
        let set_name = name.to_owned();
        match set(name) {
            Some(s) if &s.of == ty => expr::typed(&s.filter).map_err(Said::from),
            Some(s) => {
                let (of, ty) = (s.of.clone(), ty.clone());
                Err(Case::SetOfOtherType {
                    set: set_name,
                    of,
                    ty,
                }
                .into())
            }
            None => Err(Case::SetUndeclared { set: set_name }.into()),
        }
    };
    lakelands_api::expr::expand(e, &named).map_err(|s| invalid(at(on, s)))
}

/// The fields a condition names: the first segment of each path, and what
/// the sets it names read, among `sets`. `None` when it does not parse or
/// names a set that is not among them.
fn named(sets: &[ObjectSetDecl], c: &Condition) -> Option<BTreeSet<FieldName>> {
    let filter = |n: &str| {
        let set = n.to_owned();
        let s = (sets.iter().find(|s| s.name == n)).ok_or(Case::SetUndeclared { set })?;
        expr::typed(&s.filter).map_err(Said::from)
    };
    let e = lakelands_api::expr::expand(&expr::typed(c).ok()?, &filter).ok()?;
    let mut paths = vec![];
    expr::paths(&e, &mut paths);
    Some(
        paths
            .into_iter()
            .filter_map(|p| p.first().cloned())
            .collect(),
    )
}

/// A trigger's guard or an object set's condition is shown to a principal
/// who reads every field it names, or holds the `schema` grant.
fn condition_visible(st: &State, who: &Principal, ty: &TypeName, c: Option<&Condition>) -> bool {
    let named = |c| named(&st.object_sets, c);
    who.grants.schema || c.is_none_or(|c| named(c).is_some_and(|n| reads_all(st, who, ty, &n)))
}

/// As the store: a read of a view probes what computing it reads, each
/// source in turn, its own first, and after each a source that is itself
/// a view by that view's probes (024). The refusal names the field to a
/// holder of the `schema` grant; to another reader, whom `describe` may
/// withhold the definition from, it names nothing.
fn view_readable(st: &State, who: &Principal, t: &DeclaredType) -> Result<()> {
    let Some(v) = t.decl.view.as_deref() else {
        return Ok(());
    };
    let unnamed = |e| match who.grants.schema {
        true => e,
        false => refused(Refusal::NotGranted),
    };
    for (source, reads) in view_reads(v) {
        let s = st.types.get(&source).ok_or_else(|| no_type(&source))?;
        for name in &reads {
            readable(st, who, s, name).map_err(unnamed)?;
        }
        view_readable(st, who, s)?;
    }
    Ok(())
}

/// A view is shown whole to a holder of the `schema` grant, or to one who
/// reads every field it reads of each of its sources.
fn view_visible(st: &State, who: &Principal, v: &ViewDecl) -> bool {
    who.grants.schema
        || view_reads(v)
            .iter()
            .all(|(source, reads)| reads_all(st, who, source, reads))
}

/// A function is shown when it computes some field and, on every type
/// where it does, the principal reads that field and every input.
fn function_visible(st: &State, who: &Principal, f: &FunctionDecl) -> bool {
    if who.grants.schema {
        return true;
    }
    let uses: Vec<(&TypeName, &FieldName)> = st
        .types
        .values()
        .flat_map(|t| {
            t.decl
                .fields
                .iter()
                .filter(|x| x.function.as_deref() == Some(f.name.as_str()))
                .map(move |x| (&t.decl.name, &x.name))
        })
        .collect();
    !uses.is_empty()
        && uses.iter().all(|(ty, x)| {
            st.visible(who, ty).is_some()
                && reads_all(
                    st,
                    who,
                    ty,
                    std::iter::once(*x).chain(f.inputs.iter().map(|i| &i.name)),
                )
        })
}

fn describe_lake(lake: &LakeId, st: &State, who: &Principal) -> LakeDescription {
    LakeDescription {
        lake: lake.clone(),
        types: st
            .types
            .keys()
            .filter_map(|n| st.visible(who, n))
            .map(|t| describe_type(st, who, t))
            .collect(),
        verbs: st
            .verbs
            .values()
            .filter(|v| callable(st, who, v))
            .map(|v| VerbDescription {
                name: v.name.clone(),
                on: v.on.clone(),
                scope: v.scope,
                payload: v
                    .payload
                    .iter()
                    .map(|f| FieldDescription::of(f, Access::Write))
                    .collect(),
                risk: v.risk,
                external: matches!(v.effect, VerbEffect::External { .. }),
                // The mock binds no executor.
                executor: match &v.effect {
                    VerbEffect::External { executor } => Some(ExecutorDescription {
                        name: executor.clone(),
                        bound: false,
                    }),
                    VerbEffect::Write { .. } => None,
                },
                description: v.description.clone(),
            })
            .collect(),
        value_types: st.value_types.clone(),
        functions: by_name(
            st.functions
                .iter()
                .filter(|f| function_visible(st, who, f))
                .cloned(),
            |f| f.name.clone(),
        ),
        triggers: by_name(
            st.triggers
                .iter()
                .filter(|t| {
                    st.visible(who, &t.on).is_some()
                        && condition_visible(st, who, &t.on, t.when.as_ref())
                })
                .cloned(),
            |t| t.name.to_string(),
        ),
        object_sets: by_name(
            st.object_sets
                .iter()
                .filter(|s| {
                    st.visible(who, &s.of).is_some()
                        && condition_visible(st, who, &s.of, Some(&s.filter))
                })
                .cloned(),
            |s| s.name.clone(),
        ),
        may: LakeRights {
            review: who.grants.review,
            schema: who.grants.schema,
            ingest: who.grants.ingest,
        },
    }
}

// ---- reading ----

/// As the store: the kind an expression certainly computes over a type's
/// records, or the refusal of one that can never be evaluated over them.
fn evaluates(e: &Expr, t: &TypeDecl, on: &[Place]) -> Result<Option<Kind>> {
    let kind = |n: &FieldName| field(t, n).map(|f| f.ty.kind);
    lakelands_api::expr::kind::of(e, &kind).map_err(|why| invalid(at(on, why)))
}

fn of_type(name: &TypeName) -> Place {
    let name = name.clone();
    Place::Type { name }
}

fn of_field(name: &FieldName) -> Place {
    let name = name.clone();
    Place::Field { name }
}

/// What carries a relation's edges: a `via` field, link records, or
/// nothing stored (derived).
fn carrier(r: &RelationDecl) -> Carrier {
    match (&r.via, &r.derived) {
        (_, Some(_)) => Carrier::Derived,
        (Some(_), None) => Carrier::Via,
        (None, None) => Carrier::Links,
    }
}

/// The typed filter, with every path it reads checked against the type
/// and the principal's grants.
fn filter(
    st: &State,
    who: &Principal,
    t: &DeclaredType,
    c: Option<&lakelands_api::expr::Condition>,
) -> Result<Option<Expr>> {
    let Some(c) = c else { return Ok(None) };
    // As the store: a set whose filter reads a field its reader is kept
    // from is, to that reader, a set that is not declared.
    let all = |n: &str| st.object_sets.iter().find(|s| s.name == n);
    let shown = |s: &&ObjectSetDecl| {
        let reads = named(&st.object_sets, &s.filter);
        reads.is_some_and(|names| reads_all(st, who, &s.of, &names))
    };
    let on = [Place::Where];
    let typed = expr::typed(c).map_err(|c| invalid(at(&on, c)))?;
    let e = expanded(&|n| all(n).filter(shown), &t.decl.name, &typed, &on)?;
    let mut paths = Vec::new();
    expr::paths(&e, &mut paths);
    for p in paths {
        // As the store: a path reads its first field, and one that walks
        // into what has no members is refused by what it evaluates.
        let name = match p.as_slice() {
            [ty, f] if ty.as_str() == t.decl.name.as_str() => f,
            [f, ..] => f,
            [] => continue,
        };
        readable(st, who, t, name)?;
    }
    // As the store: what can never be evaluated is refused, once the reader
    // is known to read the fields it names.
    evaluates(&e, &t.decl, &on)?;
    Ok(Some(e))
}

/// Refuse a field the type does not declare or the principal may not read.
fn readable(_: &State, who: &Principal, t: &DeclaredType, name: &FieldName) -> Result<()> {
    let f = field(&t.decl, name).ok_or_else(|| no_field(&t.decl.name, name))?;
    match access(who, &t.decl, f) {
        Access::None => Err(no_field(&t.decl.name, name)),
        a if !a.reads() => Err(on(name, Refusal::NotGranted)),
        _ => Ok(()),
    }
}

/// As the store (051): refuse a read that names `t` where `who`'s `only`
/// on it cannot be applied, naming the type and never the field.
fn check_only(who: &Principal, t: &DeclaredType) -> Result<()> {
    if only_applies(who, &t.decl) {
        return Ok(());
    }
    Err(invalid(Case::OnlyUnappliable {
        ty: t.decl.name.clone(),
    }))
}

/// The answer for a field the type does not declare, which is also the
/// answer for one the principal may not know of.
fn no_field(ty: &TypeName, name: &FieldName) -> Error {
    let (ty, field) = (ty.clone(), name.clone());
    invalid_field(name, Case::UnknownField { ty, field })
}

/// A read's valid time for a type, as the principal is shown it: the
/// instant, and whether each bound is shown.
type Valid = Option<(lakelands_api::Timestamp, bool, bool)>;

/// A read at a valid time filters on the type's valid-time fields, so it
/// probes them as a filter on them would (024). As the store, a bound over
/// a field the principal may not know of is not described, so it is
/// dropped, and only the bounds shown are probed.
fn valid_time_readable(st: &State, who: &Principal, t: &DeclaredType, at: ReadAt) -> Result<Valid> {
    let shown =
        |n: &FieldName| field(&t.decl, n).is_none_or(|f| access(who, &t.decl, f) != Access::None);
    let Some(at) = at.valid_at else {
        return Ok(None);
    };
    let (from, to) = (t.decl.valid_from.as_ref(), t.decl.valid_to.as_ref());
    for f in from.into_iter().chain(to).filter(|f| shown(f)) {
        readable(st, who, t, f)?;
    }
    Ok(Some((at, from.is_some_and(shown), to.is_some_and(shown))))
}

/// Whether a record is valid at a business instant, by the bounds shown.
fn valid_at(t: &DeclaredType, r: &StoredRecord, valid: Valid) -> bool {
    let (Some((at, from_shown, to_shown)), Some(from)) = (valid, &t.decl.valid_from) else {
        return true;
    };
    let ts = |f: &FieldName| {
        r.fields
            .get(f)
            .and_then(Value::as_str)
            .and_then(|s| lakelands_api::Timestamp::parse(s).ok())
    };
    let starts = !from_shown || ts(from).is_some_and(|s| s <= at);
    let to = t.decl.valid_to.as_ref().filter(|_| to_shown);
    let ends = to.and_then(ts).is_some_and(|e| e <= at);
    starts && !ends
}

/// The live records of a type a principal may read, its grant's `only`
/// admitting them (051), valid at `at`, matching a filter.
fn candidates<'a>(
    st: &'a State,
    who: &Principal,
    t: &DeclaredType,
    at: ReadAt,
    filter: Option<&Expr>,
) -> Result<Vec<&'a StoredRecord>> {
    let valid = valid_time_readable(st, who, t, at)?;
    let mut out = Vec::new();
    for r in st.records.values() {
        if r.ty != t.decl.name || r.retired || (r.restricted && !who.grants.restricted) {
            continue;
        }
        if !valid_at(t, r, valid) || !st.admitted(who, r) {
            continue;
        }
        if let Some(e) = filter {
            let scope = Scope {
                ty: t.decl.name.as_str(),
                fields: &r.fields,
                payload: None,
                now: None,
            };
            if !expr::holds(e, &scope)? {
                continue;
            }
        }
        out.push(r);
    }
    Ok(out)
}

// ---- writing ----

/// Check a patch for one door, failing closed: every field must be
/// declared, of the door's authority, granted for writing and a valid
/// value, or nothing is written.
fn check_patch<'a>(
    st: &'a State,
    who: &Principal,
    ty: &TypeName,
    patch: &Patch,
    door: Origin,
) -> Result<&'a DeclaredType> {
    // Records of a `sys.` type are written under grants like any other;
    // only declaring such a type is reserved.
    let t = st.visible(who, ty).ok_or_else(|| no_type(ty))?;
    not_a_view(&t.decl)?;
    for (name, v) in patch {
        let f = field(&t.decl, name).ok_or_else(|| no_field(ty, name))?;
        if access(who, &t.decl, f) == Access::None {
            return Err(no_field(&t.decl.name, name));
        }
        if !door_writes(door, f.authority) {
            return Err(on(name, Refusal::WrongDoor));
        }
        if !access(who, &t.decl, f).writes() {
            return Err(on(name, Refusal::NotGranted));
        }
        if !v.is_null()
            && let Some(why) = value::misfit(&f.ty, v, &st.value_types)
        {
            return Err(constraint(name, why));
        }
    }
    Ok(t)
}

/// A checked patch with every value in its one form.
fn canonical_patch(t: &DeclaredType, patch: Patch) -> Patch {
    patch
        .into_iter()
        .map(|(name, v)| {
            let v = match field(&t.decl, &name) {
                Some(f) if !v.is_null() => value::canonical(&f.ty, &v),
                _ => v,
            };
            (name, v)
        })
        .collect()
}

/// Refuse a reference that names no live record of its target type, nor
/// one written earlier in the same ingest window. A record its writer's
/// `only` does not admit is, to it, not there (051).
fn check_references(
    st: &State,
    who: &Principal,
    t: &DeclaredType,
    patch: &Patch,
    earlier: &BTreeMap<(TypeName, RecordKey), Version>,
) -> Result<()> {
    for (name, v) in patch {
        let Some(target) = field(&t.decl, name).and_then(|f| f.ty.target.as_ref()) else {
            continue;
        };
        let Some(key) = v.as_str() else { continue };
        let slot = (target.clone(), RecordKey::from(key));
        let live = (st.live(&slot.0, &slot.1)).filter(|r| admits(who, target, &r.fields));
        if live.is_none() && !earlier.contains_key(&slot) {
            let (field, (ty, key)) = (name.clone(), slot);
            return Err(constraint(name, Case::NotLive { field, ty, key }));
        }
    }
    Ok(())
}

/// The key rule (`docs/log-format.md` §2.3): on a create, the key fields a
/// patch leaves out are read from the key's components; and a key must
/// agree with the key fields the record then holds.
fn with_key_fields(
    t: &DeclaredType,
    st: &State,
    who: &Principal,
    live: Option<&StoredRecord>,
    key: &RecordKey,
    patch: &mut Patch,
    door: Origin,
) -> Result<()> {
    let fields = &t.decl.key;
    if fields.is_empty() {
        return Ok(());
    }
    if live.is_none() && !fields.iter().all(|f| patch.contains_key(f)) {
        let parts = lakelands_api::key::key_components(key);
        if parts.len() != fields.len() {
            let (ty, key) = (t.decl.name.clone(), key.clone());
            let (fields, parts) = (fields.len(), parts.len());
            return Err(invalid(Case::KeyComponents {
                ty,
                fields,
                key,
                parts,
            }));
        }
        for (name, part) in fields.iter().zip(parts) {
            if patch.contains_key(name) {
                continue;
            }
            let f = field(&t.decl, name).expect("a key names a field");
            let kind = f.ty.kind;
            let wrong = |part: &str| {
                let (part, field) = (part.to_owned(), name.clone());
                invalid(Case::KeyComponentKind { part, kind, field })
            };
            let v = match kind {
                lakelands_api::ontology::StorageKind::Number => part
                    .parse::<serde_json::Number>()
                    .map(Value::Number)
                    .map_err(|_| wrong(&part))?,
                lakelands_api::ontology::StorageKind::Boolean => match part.as_str() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    _ => return Err(wrong(&part)),
                },
                _ => Value::String(part),
            };
            let v = value::canonical(&f.ty, &v);
            if access(who, &t.decl, f) == Access::None {
                return Err(no_field(&t.decl.name, name));
            }
            if !door_writes(door, f.authority) {
                return Err(on(name, Refusal::WrongDoor));
            }
            if !access(who, &t.decl, f).writes() {
                return Err(on(name, Refusal::NotGranted));
            }
            if let Some(why) = value::misfit(&f.ty, &v, &st.value_types) {
                return Err(constraint(name, why));
            }
            patch.insert(name.clone(), v);
        }
    }
    let derived: Option<Vec<String>> = fields
        .iter()
        .map(|f| {
            patch
                .get(f)
                .or_else(|| live.and_then(|r| r.fields.get(f)))
                .and_then(lakelands_api::key::key_component)
        })
        .collect();
    match derived.map(|d| RecordKey::from(d.join(":"))) {
        Some(derived) if &derived != key => {
            let (key, ty) = (key.clone(), t.decl.name.clone());
            Err(invalid(Case::KeyDisagrees { key, ty, derived }))
        }
        _ => Ok(()),
    }
}

/// The fields a write leaves, as the store's fold makes them before it
/// computes: the patch over the fields before, a null clearing one, and
/// at a create each `initial` value the patch left out (050).
fn after_write(
    t: &DeclaredType,
    before: Option<&BTreeMap<FieldName, Value>>,
    patch: &Patch,
) -> BTreeMap<FieldName, Value> {
    let mut after = before.cloned().unwrap_or_default();
    for (f, v) in patch {
        match v {
            Value::Null => after.remove(f),
            v => after.insert(f.clone(), v.clone()),
        };
    }
    for f in t.decl.fields.iter().filter(|_| before.is_none()) {
        if let Some(v) = f.initial.as_ref().filter(|_| !after.contains_key(&f.name)) {
            after.insert(f.name.clone(), v.clone());
        }
    }
    after
}

/// Refuse a record that would lack a value for a required field, as the
/// store does: over the record whole as the write leaves it, whichever
/// door writes the field, with the `initial` values a create fills (050).
/// A derived field is judged by its expression where the mock evaluates
/// it; one computed by a function is not judged here.
fn check_required(
    t: &DeclaredType,
    before: Option<&BTreeMap<FieldName, Value>>,
    patch: &Patch,
) -> Result<()> {
    let after = after_write(t, before, patch);
    for f in t
        .decl
        .fields
        .iter()
        .filter(|f| f.required && f.function.is_none())
    {
        let scope = Scope {
            ty: t.decl.name.as_str(),
            fields: &after,
            payload: None,
            now: None,
        };
        let has = match f
            .derived
            .as_ref()
            .map(|c| expr::eval(&expr::typed(c).map_err(invalid)?, &scope))
        {
            Some(computed) => computed.map_or(true, |v| !v.is_null()),
            None => after.get(&f.name).is_some_and(|v| !v.is_null()),
        };
        if !has {
            let (ty, field) = (t.decl.name.clone(), f.name.clone());
            return Err(constraint(&f.name, Case::Required { ty, field }));
        }
    }
    Ok(())
}

/// As the store (050): `initial` stands on an authored field of a type's
/// own alone, with a value a write of the field would take.
fn check_initial(st: &State, d: &Declaration) -> Result<()> {
    let refuse = |owner: &[Place], f: &FieldDecl, why: Case| {
        let on = [owner, &[of_field(&f.name), Place::Initial]].concat();
        Err(invalid_field(&f.name, at(&on, why)))
    };
    let place = d.place();
    let elsewhere: Vec<(Vec<Place>, &FieldDecl)> = match d {
        Declaration::Type(t) => (t.relations.iter())
            .flat_map(|r| {
                let name = r.name.clone();
                let on = vec![place.clone(), Place::Relation { name }];
                r.fields.iter().map(move |f| (on.clone(), f))
            })
            .collect(),
        Declaration::Verb(v) => (v.payload.iter())
            .map(|f| (vec![place.clone(), Place::Payload], f))
            .collect(),
        Declaration::Function(f) => (f.inputs.iter())
            .map(|f| (vec![place.clone(), Place::Input], f))
            .collect(),
        _ => Vec::new(),
    };
    if let Some((on, f)) = elsewhere.into_iter().find(|(_, f)| f.initial.is_some()) {
        return refuse(&on, f, Case::InitialNotOfType);
    }
    let Declaration::Type(t) = d else {
        return Ok(());
    };
    let owner = [place];
    for f in &t.fields {
        let Some(v) = &f.initial else {
            continue;
        };
        let names = t.relations.iter().any(|r| r.via.as_ref() == Some(&f.name));
        let why = if t.view.is_some() {
            Case::InitialInView
        } else if f.authority != Authority::Authored || f.derived.is_some() {
            Case::InitialNotAuthored
        } else if t.key.contains(&f.name) {
            Case::InitialKey
        } else if f.ty.target.is_some() || names {
            Case::InitialReference
        } else if f.on_input_change == lakelands_api::ontology::OnInputChange::Clear {
            Case::InitialCleared
        } else {
            match value::misfit(&f.ty, v, &st.value_types) {
                Some(why) => return Err(constraint(&f.name, why)),
                None => continue,
            }
        };
        return refuse(&owner, f, why);
    }
    Ok(())
}

/// Refuse an aging rule the write would leave not fitting the lake, naming
/// the rule field at fault.
fn check_rule(st: &State, who: &Principal, key: &RecordKey, patch: &Patch) -> Result<()> {
    let ty = TypeName::from(crate::aging::RULE_TYPE);
    let mut fields = st
        .live(&ty, key)
        .map(|r| r.fields.clone())
        .unwrap_or_default();
    fields
        .entry("name".into())
        .or_insert_with(|| Value::String(key.to_string()));
    for (name, v) in patch {
        if v.is_null() {
            fields.remove(name);
        } else {
            fields.insert(name.clone(), v.clone());
        }
    }
    let rule =
        crate::aging::check(st, &fields).map_err(|(field, why)| invalid_field(&field, *why))?;
    // A rule moves every owner's records: never one a holder of `only`
    // on its type writes (051).
    if (st.types.get(&rule.on)).is_some_and(|t| only_holder(who, &t.decl)) {
        return Err(refused(Refusal::NotGranted));
    }
    // As the store: the author holds write on the field the rule moves,
    // and read on every field its condition probes.
    check_patch(
        st,
        who,
        &rule.on,
        &[(rule.field.clone(), rule.to.clone())].into(),
        Origin::Authored,
    )?;
    let t = st.types.get(&rule.on).ok_or_else(|| no_type(&rule.on))?;
    let mut read = Vec::new();
    expr::paths(&rule.when, &mut read);
    for name in read.iter().filter_map(|p| p.first()) {
        match field(&t.decl, name) {
            Some(f) if access(who, &t.decl, f).reads() => {}
            Some(_) => return Err(on(name, Refusal::NotGranted)),
            None => return Err(no_field(&rule.on, name)),
        }
    }
    Ok(())
}

/// Refuse a setting the store reads itself where the write would leave it
/// asking for what cannot be: an index of what that index cannot keep, or
/// a view kept that cannot be, naming the member at fault as its writer
/// is shown it. As the store's record plan, after the writer's grants.
fn check_setting(st: &State, who: &Principal, key: &RecordKey, patch: &Patch) -> Result<()> {
    let value = match patch.get("value") {
        Some(v) => Some(v.clone()),
        None => st
            .live(&TypeName::from(SETTING_TYPE), key)
            .and_then(|r| r.fields.get("value").cloned()),
    };
    let declared = |t: &TypeName| st.types.get(t).map(|t| &t.decl);
    let shown = |t: &TypeName| st.visible(who, t).map(|t| describe_type(st, who, t));
    lakelands_api::seam::check_setting(key.as_str(), value.as_ref(), &declared, &shown)
}

/// The fields of `t` the lake keeps an index for: those its
/// `<prefix><type>` setting (`sort_index.` or `vector_index.`) names that
/// it declares with a kind that index keeps.
fn index_fields(
    st: &State,
    t: &TypeDecl,
    prefix: &str,
    keeps: fn(&lakelands_api::ontology::StorageKind) -> bool,
) -> Vec<FieldName> {
    let key = RecordKey::from(format!("{prefix}{}", t.name));
    let Some(setting) = st.live(&TypeName::from(SETTING_TYPE), &key) else {
        return Vec::new();
    };
    let mut out: Vec<FieldName> = Vec::new();
    // As the store: a view's order index is its kept records'.
    if t.view.is_some() && prefix != SORT_INDEX_SETTING {
        return out;
    }
    for name in setting
        .fields
        .get("value")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !out.iter().any(|f| f.as_str() == name)
            && field(t, &FieldName::from(name)).is_some_and(|f| keeps(&f.ty.kind))
        {
            out.push(FieldName::from(name));
        }
    }
    out
}

fn check_version(st: &State, ty: &TypeName, key: &RecordKey, opts: &WriteOpts) -> Result<Version> {
    versioned(st.records.get(&(ty.clone(), key.clone())), opts)
}

/// The expected version checked against `own`, the record a write is
/// judged on; answers its version, 0 for none.
fn versioned(own: Option<&StoredRecord>, opts: &WriteOpts) -> Result<Version> {
    let actual = own.map_or(Version(0), |r| r.version);
    match opts.expected_version {
        Some(expected) if expected != actual => Err(Error::Conflict { expected, actual }),
        _ => Ok(actual),
    }
}

/// Who a planned write is made as, over which state, and the event that
/// caused it, if any.
struct By<'a> {
    st: &'a State,
    who: &'a Principal,
    cause: Option<Position>,
}

/// Plan an authored upsert; answer the record's new version.
fn plan_upsert(
    by: &By<'_>,
    batch: &mut Batch,
    ty: &TypeName,
    key: &RecordKey,
    patch: Patch,
    opts: &WriteOpts,
) -> Result<Version> {
    let By { st, who, cause } = *by;
    // The type first, as the store's record plan asks it: one not shown
    // it is told nothing of the write.
    st.visible(who, ty).ok_or_else(|| no_type(ty))?;
    if patch.is_empty() {
        return Err(invalid(Case::PatchEmpty));
    }
    // As the store's record plan: every door that writes a rule checks it,
    // `upsert`, a verb's write and an accepted proposal alike.
    if ty.as_str() == crate::aging::RULE_TYPE {
        check_rule(st, who, key, &patch)?;
    }
    let t = check_patch(st, who, ty, &patch, Origin::Authored)?;
    if ty.as_str() == SETTING_TYPE {
        check_setting(st, who, key, &patch)?;
    }
    // As the store (051): a holder is judged against the record it owns
    // at the key; another's is, to it, a key that is free, refused once
    // every check has passed.
    let own = (st.records.get(&(ty.clone(), key.clone()))).filter(|r| st.admitted(who, r));
    let live = own.filter(|r| !r.retired);
    let other = live.is_none() && st.live(ty, key).is_some();
    versioned(own, opts)?;
    let mut patch = canonical_patch(t, patch);
    with_key_fields(t, st, who, live, key, &mut patch, Origin::Authored)?;
    // After the key fields are filled: a key field may be a reference too.
    check_references(st, who, t, &patch, &BTreeMap::new())?;
    check_required(t, live.map(|r| &r.fields), &patch)?;
    let after = after_write(t, live.map(|r| &r.fields), &patch);
    if other || !st.admits_fields(who, ty, &after) {
        return Err(refused(Refusal::NotGranted));
    }
    let version = Version(st.version(ty, key).0 + 1);
    let mut d = Draft::new(
        who,
        Origin::Authored,
        EventBody::RecordUpserted {
            ty: ty.clone(),
            key: key.clone(),
            version,
            patch,
            made: st.live(ty, key).is_none(),
        },
    );
    d.cause = cause;
    d.key = opts.idempotency_key.clone();
    batch.push(d);
    Ok(version)
}

fn plan_retire(
    by: &By<'_>,
    batch: &mut Batch,
    ty: &TypeName,
    key: &RecordKey,
    opts: &WriteOpts,
) -> Result<Version> {
    let By { st, who, cause } = *by;
    let t = st.visible(who, ty).ok_or_else(|| no_type(ty))?;
    not_a_view(&t.decl)?;
    if !may_retire(who, t) {
        return Err(refused(Refusal::NotGranted));
    }
    // To a holder of `only`, another's record is not found (051).
    let own = st.live(ty, key).filter(|r| st.admitted(who, r));
    own.ok_or_else(|| no_record(ty, key))?;
    let actual = check_version(st, ty, key, opts)?;
    let version = Version(actual.0 + 1);
    let mut d = Draft::new(
        who,
        Origin::Authored,
        EventBody::RecordRetired {
            ty: ty.clone(),
            key: key.clone(),
            version,
        },
    );
    d.cause = cause;
    d.key = opts.idempotency_key.clone();
    batch.push(d);
    Ok(version)
}

/// The verb a principal may call, and the live record it is called on,
/// with the payload and the verb's precondition checked.
fn check_call<'a>(
    st: &'a State,
    who: &Principal,
    verb: &VerbName,
    key: Option<&RecordKey>,
    payload: &Patch,
    opts: &WriteOpts,
) -> Result<&'a VerbDecl> {
    // A verb on a type its caller is not shown is one that is not declared.
    let shown = |v: &&VerbDecl| st.visible(who, &v.on).is_some();
    let v = st
        .verbs
        .get(verb)
        .filter(shown)
        .ok_or_else(|| Error::NotFound {
            missing: Missing::Verb { name: verb.clone() },
        })?;
    // The key against the scope comes before anything else is read.
    let key = scoped(v, key, opts)?;
    // A record of a view is computed, never written or acted on.
    if key.is_some()
        && let Some(t) = st.types.get(&v.on)
    {
        not_a_view(&t.decl)?;
    }
    // A declared verb the caller may not call is refused, as the store
    // refuses it; only an undeclared one is not found.
    if !callable(st, who, v) {
        return Err(refused(Refusal::NotGranted));
    }
    // The mock binds no executor, so a verb that reaches outside the lake
    // is refused, as the store refuses one whose executor its host did not
    // bind: before the payload, and whether it runs or is proposed (013).
    if let VerbEffect::External { executor } = &v.effect {
        return Err(Error::unbound(Seam::Executor {
            name: executor.clone(),
        }));
    }
    // As the store: a member the verb does not declare before any value.
    let declared = |n: &&FieldName| v.payload.iter().any(|f| &f.name == *n);
    if let Some(name) = payload.keys().find(|n| !declared(n)) {
        let (verb, member) = (verb.clone(), name.clone());
        return Err(invalid_field(
            name,
            Case::PayloadUndeclared { verb, member },
        ));
    }
    for (name, val) in payload {
        let Some(f) = v.payload.iter().find(|f| &f.name == name) else {
            continue;
        };
        if !val.is_null()
            && let Some(why) = value::misfit(&f.ty, val, &st.value_types)
        {
            return Err(constraint(name, why));
        }
    }
    for f in v.payload.iter().filter(|f| f.required) {
        if payload.get(&f.name).is_none_or(Value::is_null) {
            let (verb, member) = (verb.clone(), f.name.clone());
            return Err(constraint(&f.name, Case::PayloadRequired { verb, member }));
        }
    }
    // The record after the payload; the version before `requires` is
    // judged on it, whether the call runs or is proposed (013 and 024,
    // extended).
    let rec = match key {
        Some(key) => Some(
            (st.live(&v.on, key).filter(|r| st.admitted(who, r)))
                .ok_or_else(|| no_record(&v.on, key))?,
        ),
        None => None,
    };
    if let (Some(_), Some(key)) = (rec, key) {
        check_version(st, &v.on, key, opts)?;
    }
    // A verb on its type declares no `requires`: declaring refuses it.
    if let (Some(req), Some(rec), Some(key)) = (&v.requires, rec, key) {
        let scope = Scope {
            ty: v.on.as_str(),
            fields: &rec.fields,
            payload: Some(payload),
            now: None,
        };
        if !expr::holds(&expr::typed(req).map_err(invalid)?, &scope)? {
            let (verb, ty, key) = (verb.clone(), v.on.clone(), key.clone());
            let case = Case::RequiresUnmet { verb, ty, key };
            return Err(refused(Refusal::Precondition(Box::new(case.into()))));
        }
    }
    Ok(v)
}

/// Plan a verb's effect as it runs: `VerbCalled`, then its write.
fn plan_run(
    by: &By<'_>,
    batch: &mut Batch,
    v: &VerbDecl,
    key: &RecordKey,
    payload: Patch,
    opts: &WriteOpts,
) -> Result<Version> {
    let By { st, who, cause } = *by;
    let set = match &v.effect {
        VerbEffect::Write { set } => set,
        VerbEffect::External { executor } => {
            return Err(Error::unbound(Seam::Executor {
                name: executor.clone(),
            }));
        }
    };
    let rec = st.live(&v.on, key).ok_or_else(|| no_record(&v.on, key))?;
    let scope = Scope {
        ty: v.on.as_str(),
        fields: &rec.fields,
        payload: Some(&payload),
        now: None,
    };
    let mut patch = Patch::new();
    for a in set {
        patch.insert(
            a.field.clone(),
            expr::eval(&expr::typed(&a.value).map_err(invalid)?, &scope)?,
        );
    }
    let mut called = Draft::new(
        who,
        Origin::Authored,
        EventBody::VerbCalled {
            verb: v.name.clone(),
            key: Some(key.clone()),
            payload,
        },
    );
    called.cause = cause;
    let called = batch.push(called);
    let by = By {
        st,
        who,
        cause: Some(called),
    };
    plan_upsert(&by, batch, &v.on, key, patch, opts)
}

/// The preflight's report of a value type's tightening, refused while a
/// stored value would not fit, with the count alone: a proposal names no
/// misfit, as the store's does not.
fn tightening(
    st: &State,
    who: &Principal,
    head: Position,
    m: &ValueTypeMigration,
) -> Result<lakelands_api::write::ValueTypeReport> {
    check_new_value_type(st, who, &m.to)?;
    let report = crate::valuetypes::preflight(st, head, m)?;
    renames_shown(st, who, m, &report)?;
    if !report.fits {
        let (value_type, misfits) = (m.to.name.clone(), report.misfits);
        let first = Vec::new();
        return Err(invalid(Case::ValueTypeNotTightened {
            value_type,
            misfits,
            first,
        }));
    }
    Ok(report)
}

/// As the store: a migration that renames values writes every field that
/// uses the value type, and is refused, naming nothing, to a caller not
/// shown one of them.
fn renames_shown(
    st: &State,
    who: &Principal,
    m: &ValueTypeMigration,
    report: &lakelands_api::write::ValueTypeReport,
) -> Result<()> {
    let read = |ty: &TypeName, f: &FieldName| {
        (st.visible(who, ty)).is_some_and(|t| readable(st, who, t, f).is_ok())
    };
    match m.renames.is_empty() || report.fields.iter().all(|u| read(&u.ty, &u.field)) {
        true => Ok(()),
        false => Err(refused(Refusal::NotGranted)),
    }
}

/// The events that put a value type's migration in force, as the store
/// appends them: without renames one `declared`; with them the
/// `value_type_migrated`, then the system's `declared` of each type whose
/// field's `initial` it renames, one set. Each is caused by `cause`.
fn value_type_drafts(
    st: &State,
    who: &Principal,
    m: ValueTypeMigration,
    report_id: String,
    cause: Option<Position>,
) -> Vec<Draft> {
    let declaration = Declaration::ValueType(m.to.clone());
    let (schema_hash, version) = identity_version(st, &declaration);
    let caused = |mut d: Draft| {
        d.cause = cause.or(d.cause);
        d
    };
    if m.renames.is_empty() {
        let body = EventBody::Declared {
            declaration,
            schema_hash,
            version,
            with_the_set: false,
        };
        return vec![caused(Draft::new(who, Origin::System, body))];
    }
    let initials = crate::valuetypes::initials(st, &m);
    let body = EventBody::ValueTypeMigrated {
        types: crate::valuetypes::using_types(st, &m.to.name),
        declaration: m.to,
        version,
        renames: m.renames,
        report_id,
    };
    let mut out = vec![caused(Draft::new(who, Origin::Authored, body))];
    let last = initials.len();
    for (i, t) in initials.into_iter().enumerate() {
        let declaration = Declaration::Type(t);
        let (schema_hash, version) = identity_version(st, &declaration);
        let body = EventBody::Declared {
            declaration,
            schema_hash,
            version,
            with_the_set: i + 1 < last,
        };
        out.push(caused(Draft::new(who, Origin::System, body)));
    }
    out
}

fn plan_propose(
    st: &State,
    batch: &mut Batch,
    who: &Principal,
    (write, pinned): (ProposedWrite, bool),
    why: String,
    key: Option<lakelands_api::IdempotencyKey>,
) -> Result<(Position, ProposalId)> {
    // A change of shape is read as its proposer's view, and logged so.
    let (mut write, mut author) = (write, None);
    match &mut write {
        ProposedWrite::Declare { declarations } => redeclared(st, who, declarations)?,
        ProposedWrite::Migrate { migration, .. } => author = authored(st, who, migration)?,
        _ => {}
    }
    // A proposal is checked as its write would be, so that an accept
    // cannot smuggle a field its proposer may not write.
    match &write {
        ProposedWrite::Upsert { ty, key, patch, .. } => {
            check_patch(st, who, ty, patch, Origin::Authored)?;
            if ty.as_str() == crate::aging::RULE_TYPE {
                check_rule(st, who, key, patch)?;
            }
            if ty.as_str() == SETTING_TYPE {
                check_setting(st, who, key, patch)?;
            }
        }
        ProposedWrite::Retire { ty, key, .. } => {
            let t = st.visible(who, ty).ok_or_else(|| no_type(ty))?;
            if !may_retire(who, t) {
                return Err(refused(Refusal::NotGranted));
            }
            st.live(ty, key).ok_or_else(|| no_record(ty, key))?;
        }
        ProposedWrite::Call {
            verb,
            key,
            payload,
            opts,
        } => {
            check_call(st, who, verb, key.as_ref(), payload, opts)?;
        }
        // A change of the ontology is checked as its door would check it;
        // the proposer needs no `schema` grant.
        ProposedWrite::Declare { declarations } => check_set(st, who, declarations)?,
        ProposedWrite::Migrate { .. } | ProposedWrite::MigrateValueType { .. } => {}
    }
    // As the store (051): a pinned proposal is judged as its write would
    // be now, against the record its proposer owns at the key.
    let (by, scratch) = (
        By {
            st,
            who,
            cause: None,
        },
        &mut Batch::new(batch.peek().0),
    );
    match &write {
        ProposedWrite::Upsert {
            ty,
            key,
            patch,
            opts,
        } if pinned => {
            plan_upsert(&by, scratch, ty, key, patch.clone(), opts)?;
        }
        ProposedWrite::Retire { ty, key, opts } if pinned => {
            plan_retire(&by, scratch, ty, key, opts)?;
        }
        ProposedWrite::Call {
            verb,
            key: Some(key),
            payload,
            opts,
        } if pinned => {
            let v = &st.verbs[verb];
            plan_run(&by, scratch, v, key, payload.clone(), opts)?;
        }
        _ => {}
    }
    let write = match write {
        ProposedWrite::Migrate { migration, .. } => {
            let head = Position(batch.peek().0.saturating_sub(1));
            let report = preflight(st, head, (&migration, author.as_ref()), who)?;
            if !report.lossless {
                return Err(lossy(st, who, &migration, head, report));
            }
            ProposedWrite::Migrate {
                migration,
                report: Box::new(report),
            }
        }
        ProposedWrite::MigrateValueType { to, renames, .. } => {
            let head = Position(batch.peek().0.saturating_sub(1));
            let m = ValueTypeMigration { to: *to, renames };
            let report = Box::new(tightening(st, who, head, &m)?);
            ProposedWrite::MigrateValueType {
                to: Box::new(m.to),
                renames: m.renames,
                report,
            }
        }
        other => other,
    };
    let id = ProposalId(value::proposal_id(who, key.as_ref(), batch.peek()));
    let mut d = Draft::new(
        who,
        Origin::Authored,
        EventBody::WriteProposed {
            proposal: id.clone(),
            write,
            why,
            pinned,
        },
    );
    d.key = key;
    Ok((batch.push(d), id))
}

/// A set of declarations checked as `apply` checks it, whoever applies
/// it: each against the lake with every other one of the set already in
/// it, every problem collected into one refusal.
fn check_set(st: &State, who: &Principal, declarations: &[Declaration]) -> Result<()> {
    if declarations.is_empty() {
        return Err(invalid(Case::SetEmpty));
    }
    let ids: Vec<String> = declarations.iter().map(identity).collect();
    let mut problems: Vec<(Place, Error)> = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        if ids[..i].contains(id) {
            let twice = invalid(Case::SetDeclaresTwice);
            problems.push((declarations[i].place(), twice));
        }
    }
    for (i, d) in declarations.iter().enumerate() {
        let mut probe = st.clone();
        for (j, other) in declarations.iter().enumerate() {
            if ids[j] != ids[i] {
                let (hash, version) = identity_version(&probe, other);
                probe.declare(other, hash, version);
            }
        }
        if let Err(e) = check_declaration(&probe, who, d) {
            problems.push((d.place(), e));
        }
    }
    match problems.len() {
        0 => Ok(()),
        1 => {
            let (at, e) = problems.remove(0);
            Err(e.within(at))
        }
        count => {
            let problems = problems.into_iter().map(|(at, e)| e.within(at)).collect();
            Err(invalid(Case::SetRefused { count, problems }))
        }
    }
}

/// As the store: the link types a migration's renamed relations move
/// their records between, old name to new. A rename keeps the carrier, so
/// it is read off the relation as the new shape declares it.
pub(crate) fn moved(m: &Migration) -> Vec<(TypeName, TypeName)> {
    let by_links = |new: &RelationName| {
        let is = m.to.relations.iter().find(|r| &r.name == new);
        is.is_some_and(|r| {
            lakelands_api::write::Carrier::of(r) == lakelands_api::write::Carrier::Links
        })
    };
    let pairs = m.relation_renames.iter().filter(|(_, new)| by_links(new));
    let ty = &m.to.name;
    pairs
        .map(|(old, new)| (links::name(ty, old), links::name(ty, new)))
        .collect()
}

/// As the store: the declarations that name a link type whose records a
/// migration moves, each as it is declared again under the new name.
fn carried(st: &State, m: &Migration) -> Vec<Declaration> {
    let ty = &m.to.name;
    let to = |t: &TypeName| {
        let (of, relation) = links::parts(t).filter(|(of, _)| of == ty)?;
        let renamed = m
            .relation_renames
            .iter()
            .find(|(old, _)| old == &relation)?;
        st.types
            .contains_key(t)
            .then(|| links::name(&of, &renamed.1))
    };
    let mut out = Vec::new();
    for t in &st.triggers {
        out.extend(to(&t.on).map(|on| {
            Declaration::Trigger(lakelands_api::ontology::TriggerDecl { on, ..t.clone() })
        }));
    }
    for v in st.verbs.values() {
        out.extend(to(&v.on).map(|on| Declaration::Verb(VerbDecl { on, ..v.clone() })));
    }
    for s in &st.object_sets {
        out.extend(to(&s.of).map(|of| {
            Declaration::ObjectSet(lakelands_api::write::ObjectSetDecl { of, ..s.clone() })
        }));
    }
    for t in st.types.values().map(|t| &t.decl) {
        let Some(mut view) = t.view.clone() else {
            continue;
        };
        let sources = view.merge.iter_mut().map(|s| &mut s.source);
        let mut reads = false;
        for source in std::iter::once(&mut view.source).chain(sources) {
            if let Some(new) = to(source) {
                (*source, reads) = (new, true);
            }
        }
        if reads {
            let view = Some(view);
            out.push(Declaration::Type(TypeDecl { view, ..t.clone() }));
        }
    }
    out
}

/// A carried declaration as the report lists it: its kind and its name.
fn carried_entry(d: &Declaration) -> lakelands_api::write::Carried {
    let v = serde_json::to_value(d).unwrap_or_default();
    let text = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
    lakelands_api::write::Carried {
        kind: text("declare"),
        name: text("name"),
    }
}

/// As the store: whether a migration changes nothing, its `to` the type in
/// force and no field renamed.
fn unchanged(st: &State, m: &Migration) -> bool {
    m.renames.is_empty() && st.in_force(&Declaration::Type(m.to.clone()))
}

/// The drafts that declare a checked set, each with the link types it
/// calls for; as the store, none for a declaration in force before it.
fn set_drafts(st: &State, who: &Principal, declarations: Vec<Declaration>) -> Vec<Draft> {
    let mut out = Vec::new();
    for declaration in declarations.into_iter().filter(|d| !st.in_force(d)) {
        let (schema_hash, version) = identity_version(st, &declaration);
        let links = link_drafts(st, who, &declaration);
        out.push(Draft::new(
            who,
            Origin::System,
            EventBody::Declared {
                declaration,
                schema_hash,
                version,
                with_the_set: false,
            },
        ));
        out.extend(links);
    }
    out
}

fn risk_word(r: Risk) -> &'static str {
    match r {
        Risk::Low => "low",
        Risk::Medium => "medium",
        Risk::High => "high",
    }
}

/// The fields a view reads from each source, its own first: its grouping,
/// what its `where` names and what its measures aggregate, as the store
/// counts them.
fn view_reads(v: &ViewDecl) -> Vec<(TypeName, Vec<FieldName>)> {
    std::iter::once((&v.source, v.r#where.as_ref(), &v.group_by, &v.measures))
        .chain(
            v.merge
                .iter()
                .map(|s| (&s.source, s.r#where.as_ref(), &s.group_by, &s.measures)),
        )
        .map(|(source, filter, group_by, measures)| {
            let mut reads: Vec<FieldName> = group_by.clone();
            if let Some(n) = filter.and_then(|c| named(&[], c)) {
                reads.extend(n);
            }
            for m in measures {
                if let Ok(e) = expr::typed(&m.expr) {
                    expr::aggregated(&e, false, &mut reads);
                }
            }
            (source.clone(), reads)
        })
        .collect()
}

/// The fields of `ty` a declaration reads, as the store counts them: what
/// it probes and the fields a verb sets.
fn declaration_reads(st: &State, d: &Declaration, ty: &TypeName) -> BTreeSet<FieldName> {
    let mut names = declaration_probes(st, d, ty);
    if let Declaration::Verb(v) = d
        && let VerbEffect::Write { set } = &v.effect
        && &v.on == ty
    {
        names.extend(set.iter().map(|a| a.field.clone()));
    }
    names
}

/// The fields of `ty` whose values a declaration reads, as the store counts
/// them: a derived field's expression on `ty` itself, the fields a derived
/// relation matches, its own and those of `ty` as its target, what a view
/// reads of `ty` as a source, a set's `where`, a trigger's `when`, and a
/// verb's `requires` and what it sets its fields from (where
/// `payload.<name>` is the call's).
fn declaration_probes(st: &State, d: &Declaration, ty: &TypeName) -> BTreeSet<FieldName> {
    let of = |c: &Condition| named(&st.object_sets, c).unwrap_or_default();
    match d {
        Declaration::Type(t) => {
            let own = t.fields.iter().filter(|_| &t.name == ty);
            let pairs = |r: &RelationDecl| r.derived.clone().map(|m| m.pairs).unwrap_or_default();
            let matching = (t.relations.iter().filter(|_| &t.name == ty)).flat_map(pairs);
            let matched = (t.relations.iter().filter(|r| &r.target == ty)).flat_map(pairs);
            let view = t.view.iter().flat_map(|v| view_reads(v));
            own.filter_map(|f| f.derived.as_ref())
                .flat_map(of)
                .chain(matching.map(|p| p.from))
                .chain(matched.map(|p| p.to))
                .chain(view.filter(|(source, _)| source == ty).flat_map(|v| v.1))
                .collect()
        }
        Declaration::ObjectSet(s) if &s.of == ty => of(&s.filter),
        Declaration::Trigger(t) if &t.on == ty => t.when.as_ref().map(of).unwrap_or_default(),
        Declaration::Verb(v) if &v.on == ty => {
            let mut names = v.requires.as_ref().map(of).unwrap_or_default();
            if let VerbEffect::Write { set } = &v.effect {
                for a in set {
                    let record = value_paths(&a.value).into_iter();
                    let record = record.filter(|p| expr::payload_member(p).is_none());
                    names.extend(record.filter_map(|p| p.first().cloned()));
                }
            }
            names
        }
        _ => BTreeSet::new(),
    }
}

/// The paths a verb's value reads; none when it does not parse.
fn value_paths(value: &Condition) -> Vec<Vec<FieldName>> {
    let mut paths = vec![];
    if let Ok(e) = expr::typed(value) {
        expr::paths(&e, &mut paths);
    }
    paths
}

/// As the store: a declaration reads only fields its type has, the link
/// type's when it names one, and a verb's value only members its payload
/// has.
fn reads_its_type(st: &State, d: &Declaration) -> Result<()> {
    if let Declaration::Verb(v) = d
        && let VerbEffect::Write { set } = &v.effect
    {
        for a in set {
            let paths = value_paths(&a.value);
            let has = |m: &&FieldName| v.payload.iter().any(|f| &f.name == *m);
            let members = paths.iter().filter_map(|p| expr::payload_member(p));
            if let Some(m) = members.into_iter().find(|m| !has(m)) {
                let on = [d.place(), of_field(&a.field)];
                let member = m.clone();
                return Err(invalid(at(&on, Case::PayloadNoMember { member })));
            }
        }
    }
    let on = match d {
        Declaration::Type(t) => Some(t),
        Declaration::Verb(v) => st.types.get(&v.on).map(|o| &o.decl),
        Declaration::Trigger(t) => st.types.get(&t.on).map(|o| &o.decl),
        Declaration::ObjectSet(s) => st.types.get(&s.of).map(|o| &o.decl),
        _ => None,
    };
    // As the store: a set is of one type; the sets a set's `where` and a
    // trigger's `when` name are declared, of its type and name it not in
    // turn; and what the condition reads can be walked.
    if let Declaration::ObjectSet(s) = d
        && let Some(was) = st
            .object_sets
            .iter()
            .find(|x| x.name == s.name && x.of != s.of)
    {
        let (was, ty) = (was.of.clone(), s.of.clone());
        let case = at(&[d.place()], Case::SetRetyped { was, ty });
        return Err(invalid_field(&"of".into(), case));
    }
    let condition = match d {
        Declaration::ObjectSet(s) => Some(&s.filter),
        Declaration::Trigger(t) => t.when.as_ref(),
        _ => None,
    };
    if let (Some(c), Some(on)) = (condition, on)
        && let Ok(e) = expr::typed(c)
    {
        let pending = |n: &str| match d {
            Declaration::ObjectSet(s) if s.name == n => Some(s),
            _ => st.object_sets.iter().find(|x| x.name == n),
        };
        let place = [d.place()];
        evaluates(&expanded(&pending, &on.name, &e, &place)?, on, &place)?;
    }
    if let Some(on) = on
        && let reads = declaration_reads(st, d, &on.name)
        && let Some(n) = (reads.iter()).find(|n| field(on, n).is_none())
    {
        let (ty, field) = (on.name.clone(), n.clone());
        return Err(invalid(at(&[d.place()], Case::UnknownField { ty, field })));
    }
    // As the store: a verb, a trigger and a set read a record as stored.
    if let Some(on) = on.filter(|_| !matches!(d, Declaration::Type(_)))
        && let reads = declaration_reads(st, d, &on.name)
        && let Some(n) = (reads.iter()).find(|n| field(on, n).is_some_and(|f| f.at_read()))
    {
        let (field, ty) = (n.clone(), on.name.clone());
        return Err(invalid(at(&[d.place()], Case::ReadsAtRead { field, ty })));
    }
    Ok(())
}

/// As the store: a view with parameters is bound only by a read that names
/// it, so a pattern's step that arrives at one other than its start is
/// refused before anything is read.
fn unbound_view(start: &TypeName, reached: &DeclaredType, step: Place) -> Result<()> {
    match reached.decl.view.as_deref().and_then(|v| v.params.first()) {
        Some(p) if &reached.decl.name != start => {
            let (ty, name, kind) = (reached.decl.name.clone(), p.name.clone(), p.kind);
            let case = Case::ParamMissing {
                ty,
                name,
                kind,
                unbound: true,
            };
            Err(invalid(Said::from(case).within(step)))
        }
        _ => Ok(()),
    }
}

/// Reads `to`, a migration's or a redeclaration's shape of the type
/// declared as `before`, as its author's view, as the store does: a field
/// `who` may not know of, or a relation it is not shown, is put back as
/// declared, at its declared place, and so are such link fields of each
/// relation `to` keeps or renames that it is shown. A shape that names one
/// declares it twice, found where the declaration is checked; a rename
/// from one is judged against the author's view ([`preflight`]); a field
/// one reads is refused on that field.
fn as_author(
    (st, who): (&State, &Principal),
    before: &TypeDecl,
    to: &mut TypeDecl,
    relations: &[(RelationName, RelationName)],
) -> Result<()> {
    let hidden = |t: &TypeDecl, f: &FieldDecl| access(who, t, f) == Access::None;
    let unshown = |r: &RelationDecl| !relation_shown(st, who, before, r);
    let put_back = |t: &TypeDecl, declared: &[FieldDecl], fields: &mut Vec<FieldDecl>| {
        for (i, f) in declared.iter().enumerate().filter(|(_, f)| hidden(t, f)) {
            fields.insert(i.min(fields.len()), f.clone());
        }
    };
    let unknown: Vec<&FieldDecl> = (before.fields.iter().filter(|f| hidden(before, f))).collect();
    put_back(before, &before.fields, &mut to.fields);
    // As the store: a valid time over a field the author may not know of,
    // left out, stands as declared.
    let unseen = |v: &Option<FieldName>| {
        v.as_ref()
            .is_some_and(|v| unknown.iter().any(|f| &f.name == v))
    };
    for (axis, sent, held) in [
        ("valid_from", &mut to.valid_from, &before.valid_from),
        ("valid_to", &mut to.valid_to, &before.valid_to),
    ] {
        if sent.is_none() && unseen(held) {
            sent.clone_from(held);
        } else if *sent != *held && unseen(held) && !unseen(sent) {
            // As the store: a field the author may name, sent where a bound
            // it may not know of stands, is refused as the bound taken.
            // A hidden field's name is left to the declaration.
            let (name, axis) = (to.name.clone(), axis.to_owned());
            let said = Said::from(Case::ValidTimeStands { axis });
            return Err(invalid(said.within(Place::Type { name })));
        }
    }
    for r in &mut to.relations {
        // As the store: a renamed relation is the one it was, and one its
        // author is not shown is none to rename.
        let was = relations.iter().find(|(_, new)| new == &r.name);
        let name = was.map_or(&r.name, |(old, _)| old);
        let shown = |o: &&RelationDecl| &o.name == name && !unshown(o);
        if let Some(old) = before.relations.iter().find(shown) {
            let link = links::link_type(&before.name, old);
            put_back(&link, &old.fields, &mut r.fields);
        }
    }
    for (i, r) in before
        .relations
        .iter()
        .enumerate()
        .filter(|(_, r)| unshown(r))
    {
        to.relations.insert(i.min(to.relations.len()), r.clone());
    }
    // What a relation it is not shown reads: its `via`, its match's own side.
    let hid = before.relations.iter().filter(|r| unshown(r));
    let matched = hid.flat_map(|r| {
        let pairs = r.derived.iter().flat_map(|d| &d.pairs);
        r.via.iter().chain(pairs.map(|p| &p.from))
    });
    let mut reads = (unknown.iter().copied())
        .flat_map(|f| (f.depends_on.iter()).chain(f.embed.iter().flat_map(|e| &e.from)))
        .chain(matched);
    match reads.find(|n| field(to, n).is_none()) {
        Some(n) => {
            let (field, ty) = (n.clone(), to.name.clone());
            Err(invalid_field(n, Case::FieldReadElsewhere { field, ty }))
        }
        None => Ok(()),
    }
}

/// A migration read as its author's view of its type, and that view, for
/// [`preflight`] to judge its names against, as the store does.
fn authored(st: &State, who: &Principal, m: &mut Migration) -> Result<Option<Authored>> {
    if st.types.contains_key(&m.to.name) && st.visible(who, &m.to.name).is_none() {
        return Err(no_type(&m.to.name));
    }
    reads_as_author(st, who, &[Declaration::Type(m.to.clone())], true)?;
    let Some(before) = st.types.get(&m.to.name) else {
        return Ok(None);
    };
    let (sent, mut known) = (m.clone(), before.decl.clone());
    known_fields((st, who), &mut known);
    as_author((st, who), &before.decl, &mut m.to, &m.relation_renames)?;
    Ok(Some(Authored { sent, known }))
}

/// A migration as its author sent it, and the type as its author sees it.
struct Authored {
    sent: Migration,
    known: TypeDecl,
}

/// As the store: a key and a valid time name fields of their type, and a
/// valid time a date-time or a date field, in the store's cases.
fn shape_names(t: &TypeDecl) -> Result<()> {
    let has = |f: &FieldName| t.fields.iter().any(|d| &d.name == f);
    let owner = [of_type(&t.name)];
    if let Some(k) = (t.key.iter().chain(&t.valid_from).chain(&t.valid_to)).find(|k| !has(k)) {
        let (ty, field) = (t.name.clone(), k.clone());
        return Err(invalid_field(
            k,
            at(&owner, Case::UnknownField { ty, field }),
        ));
    }
    for (axis, k) in [("valid_from", &t.valid_from), ("valid_to", &t.valid_to)] {
        let field = k
            .as_ref()
            .and_then(|k| t.fields.iter().find(|f| &f.name == k));
        use lakelands_api::ontology::StorageKind as K;
        if let Some(f) = field.filter(|f| !matches!(f.ty.kind, K::DateTime | K::Date)) {
            let (field, axis, kind) = (f.name.clone(), axis.to_owned(), f.ty.kind);
            let case = Case::ValidTimeNotTime { field, axis, kind };
            return Err(invalid_field(&f.name, at(&owner, case)));
        }
    }
    Ok(())
}

/// As the store: a declaration reads only what its author may read. The
/// set is checked as sent against the lake as its author is shown it, so a
/// field the author may not know of answers as one the type does not have;
/// then a field it knows of and may not read is refused as not granted,
/// where the declaration probes it and the one in force did not.
fn reads_as_author(
    st: &State,
    who: &Principal,
    sent: &[Declaration],
    reshapes: bool,
) -> Result<()> {
    // As the store: a type its author is not shown is a type that is not
    // declared, for whatever names it; to declare it is not granted.
    let shown = |ty: &TypeName| st.visible(who, ty).is_some();
    let hidden = |d: &Declaration| {
        matches!(d, Declaration::Type(t)
        if st.types.contains_key(&t.name) && !shown(&t.name))
    };
    if sent.iter().any(hidden) {
        return Err(refused(Refusal::NotGranted));
    }
    let mut seen = st.clone();
    seen.types.retain(|name, _| shown(name));
    for t in seen.types.values_mut() {
        known_fields((st, who), &mut t.decl);
    }
    // As the store: a valid time left out where the one in force names a
    // field outside the author's view stands as declared, left out of both
    // sides; a valid time sent, the one in force too, is read against the
    // view.
    let mut shaped = sent.to_vec();
    for d in &mut shaped {
        let Declaration::Type(t) = d else { continue };
        let (Some(was), Some(e)) = (st.types.get(&t.name), seen.types.get_mut(&t.name)) else {
            continue;
        };
        let held = &was.decl;
        let view: Vec<FieldName> = e.decl.fields.iter().map(|f| f.name.clone()).collect();
        let unseen = |f: &Option<FieldName>| f.as_ref().is_some_and(|f| !view.contains(f));
        for (sent, kept, was) in [
            (&mut t.valid_from, &mut e.decl.valid_from, &held.valid_from),
            (&mut t.valid_to, &mut e.decl.valid_to, &held.valid_to),
        ] {
            if sent.is_none() && unseen(was) {
                (*sent, *kept) = (None, None);
            }
        }
    }
    // A migration's shape is refused as a declaration, of the type or of
    // a relation's links.
    let reshaping = |e: &Error| {
        let said = e.case().map(|s| &s.case);
        reshapes
            && matches!(
                said,
                Some(Case::Reshapes { .. } | Case::ReshapesLinks { .. })
            )
    };
    check_set(&seen, who, &shaped).or_else(|e| if reshaping(&e) { Ok(()) } else { Err(e) })?;
    let mut after = st.clone();
    for d in sent {
        let (hash, version) = identity_version(&after, d);
        after.declare(d, hash, version);
    }
    for d in sent {
        let named = |x: &&lakelands_api::ontology::TriggerDecl| matches!(d, Declaration::Trigger(t) if t.name == x.name);
        let held = |x: &&lakelands_api::write::ObjectSetDecl| matches!(d, Declaration::ObjectSet(s) if s.name == x.name);
        let was = match d {
            Declaration::Type(t) => st
                .types
                .get(&t.name)
                .map(|t| Declaration::Type(t.decl.clone())),
            Declaration::Verb(v) => st.verbs.get(&v.name).cloned().map(Declaration::Verb),
            Declaration::Trigger(_) => st
                .triggers
                .iter()
                .find(named)
                .cloned()
                .map(Declaration::Trigger),
            Declaration::ObjectSet(_) => st
                .object_sets
                .iter()
                .find(held)
                .cloned()
                .map(Declaration::ObjectSet),
            _ => None,
        };
        // Only what its author adds or changes: of a type, the fields, the
        // relations and the view that do not stand as they are.
        let mut new = d.clone();
        if let Declaration::Type(t) = &mut new {
            after.complete(t);
        }
        match (&mut new, &was) {
            (Declaration::Type(t), Some(Declaration::Type(w))) => {
                t.fields.retain(|f| !w.fields.contains(f));
                t.relations.retain(|r| !w.relations.contains(r));
                t.view = t.view.take().filter(|v| Some(v) != w.view.as_ref());
            }
            (_, Some(w)) if w == d => continue,
            _ => {}
        }
        for t in st.types.values() {
            let mut names = declaration_probes(st, &new, &t.decl.name);
            // A function's inputs and what an embedding is made from.
            if let Declaration::Type(x) = &new
                && x.name == t.decl.name
            {
                for f in &x.fields {
                    names.extend(f.depends_on.iter().cloned());
                    names.extend(f.embed.iter().flat_map(|e| e.from.clone()));
                }
            }
            for n in names.iter().filter(|n| field(&t.decl, n).is_some()) {
                readable(st, who, t, n)?;
            }
        }
    }
    Ok(())
}

/// Each type of `declarations` the lake declares already, read as its
/// author's view of it.
fn redeclared(st: &State, who: &Principal, declarations: &mut [Declaration]) -> Result<()> {
    reads_as_author(st, who, declarations, false)?;
    for d in declarations {
        if let Declaration::Type(t) = d
            && let Some(before) = st.types.get(&t.name)
        {
            as_author((st, who), &before.decl, t, &[])?;
        }
    }
    Ok(())
}

/// `t` without the fields `who` may not know of, a relation's link fields
/// too, and without the relations it is not shown.
fn known_fields((st, who): (&State, &Principal), t: &mut TypeDecl) {
    let whole = t.clone();
    t.relations.retain(|r| relation_shown(st, who, &whole, r));
    t.fields.retain(|f| access(who, &whole, f) != Access::None);
    for r in &mut t.relations {
        let link = links::link_type(&whole.name, r);
        r.fields.retain(|f| access(who, &link, f) != Access::None);
    }
}

/// As the store: `t` as `describe` would show its fields to `who`, one it
/// may not read left with its shape alone; what a listing hands out.
fn described_fields((st, who): (&State, &Principal), t: &mut TypeDecl) {
    let whole = t.clone();
    t.relations.retain(|r| relation_shown(st, who, &whole, r));
    let shown = |of: &TypeDecl, fields: &mut Vec<FieldDecl>| {
        fields.retain(|f| access(who, of, f) != Access::None);
        let unread = fields.iter_mut().filter(|f| !access(who, of, f).reads());
        unread.for_each(FieldDecl::shape_alone);
    };
    shown(&whole, &mut t.fields);
    for r in &mut t.relations {
        shown(&links::link_type(&whole.name, r), &mut r.fields);
    }
    let held = |v: &FieldName| t.fields.iter().any(|f| &f.name == v);
    (t.valid_from, t.valid_to) = (
        t.valid_from.take().filter(held),
        t.valid_to.take().filter(held),
    );
}

/// As the store: whether `t` declares `name`, a field or a link field
/// (`relation.field`), as one `who` may not read.
fn unread(who: &Principal, t: &TypeDecl, name: &FieldName) -> bool {
    let link = |(r, f): (&str, _)| {
        let r = t.relations.iter().find(|x| x.name.as_str() == r)?;
        Some((links::link_type(&t.name, r), f))
    };
    let (of, name) = match name.as_str().split_once('.') {
        Some(named) => match link(named) {
            Some((of, name)) => (std::borrow::Cow::Owned(of), name),
            None => return false,
        },
        None => (std::borrow::Cow::Borrowed(t), name.as_str()),
    };
    let found = of.fields.iter().find(|f| f.name.as_str() == name);
    found.is_some_and(|f| !access(who, &of, f).reads())
}

/// Whether a shape holds what a migration's report names by `name`: a
/// field, a link field (`relation.field`), or a relation, which a report
/// lists as dropped by its own name.
fn holds(t: &TypeDecl, name: &FieldName) -> bool {
    let relation = |r: &str| t.relations.iter().find(|x| x.name.as_str() == r);
    match name.as_str().split_once('.') {
        Some((r, f)) => relation(r).is_some_and(|x| x.fields.iter().any(|x| x.name.as_str() == f)),
        None => t.fields.iter().any(|f| &f.name == name) || relation(name.as_str()).is_some(),
    }
}

/// As the store: a migration's report as `who` may know it, wherever one
/// is answered. An entry is shown where the reader may see the type and
/// know of the field, a member restated beyond the shape where it reads
/// the field in `to` and in the type it was made against (`stands`, a
/// listed proposal's as it stood, else the type as it stands, judged shown
/// by the caller as the store judges it); the totals stay, and `more`
/// says that something was left out ([`shown_hashes`] sets the hashes and
/// the id after it). The mock restricts no record, so every sample key is
/// of a record its reader is answered.
fn shown_report(
    st: &State,
    who: &Principal,
    m: &Migration,
    (report, stands): (&mut MigrationReport, &TypeDecl),
) {
    let whole = report.clone();
    let (mut was, mut to) = (stands.clone(), m.to.clone());
    known_fields((st, who), &mut was);
    known_fields((st, who), &mut to);
    let sourced = |new: &FieldName| {
        let mut from = m.renames.iter().filter(|(_, n)| n == new);
        from.all(|(old, _)| holds(&was, old))
    };
    let sees = |ty: &TypeName| st.visible(who, ty).map(|t| &t.decl);
    let known = |ty: &TypeName, f: &FieldName| {
        let found = sees(ty).and_then(|t| Some((t, field(t, f)?)));
        found.is_some_and(|(t, f)| access(who, t, f) != Access::None)
    };
    report
        .fields
        .retain(|c| holds(&to, &c.field) && sourced(&c.field));
    let read = |f: &FieldName| {
        let had = m.renames.iter().find(|(_, new)| new == f);
        !unread(who, &m.to, f) && !unread(who, stands, had.map_or(f, |(old, _)| old))
    };
    // A valid time over a field the reader may not know of is not named
    // (024, extended).
    let named =
        |v: &serde_json::Value, t: &TypeDecl| v.as_str().is_none_or(|f| holds(t, &f.into()));
    report.restated.retain(|r| {
        let shape = !FieldDecl::UNREAD.contains(&r.member.as_str());
        let shown = |f| holds(&to, f) && sourced(f) && (shape || read(f));
        let timed = r.field.is_none() && matches!(r.member.as_str(), "valid_from" | "valid_to");
        r.field.as_ref().is_none_or(shown) && (!timed || named(&r.was, &was) && named(&r.to, &to))
    });
    report.dropped.retain(|(f, _)| holds(&was, f));
    // As the store: a renamed relation as a dropped one, by the name it
    // had; a carried declaration where `describe` names it.
    (report.relations).retain(|r| holds(&was, &r.from.as_str().into()));
    let shown = describe_lake(&who.lake, st, who);
    let described = |c: &lakelands_api::write::Carried| match c.kind.as_str() {
        "trigger" => shown.triggers.iter().any(|t| t.name.as_str() == c.name),
        "verb" => shown.verbs.iter().any(|v| v.name.as_str() == c.name),
        "object_set" => shown.object_sets.iter().any(|s| s.name == c.name),
        _ => shown.types.iter().any(|t| t.name.as_str() == c.name),
    };
    report.carried.retain(described);
    report.rekeyed.retain(|r| sees(&r.ty).is_some());
    report.references.retain(|r| known(&r.ty, &r.field));
    report.sample_collisions.retain(|c| sees(&c.ty).is_some());
    report.more = *report != whole;
}

/// The refusal of a migration that is not lossless, with its report as
/// the caller may know it.
fn lossy(st: &State, who: &Principal, m: &Migration, head: Position, r: MigrationReport) -> Error {
    refused(Refusal::Migration {
        report: Box::new(shown(st, who, m, head, r)),
    })
}

/// As the store: a report as the caller may know it, its hashes and id
/// those of what it is shown, the report made from the type at the head.
fn shown(
    st: &State,
    who: &Principal,
    m: &Migration,
    head: Position,
    mut report: MigrationReport,
) -> MigrationReport {
    let made = st.types.get(&m.to.name).map_or(&m.to, |t| &t.decl);
    shown_report(st, who, m, (&mut report, made));
    shown_hashes((st, who), made, m, (&mut report, head));
    report
}

/// As the store: a proposed shape completed, as a shape in force is.
fn completed(st: &State, write: &mut ProposedWrite) {
    match write {
        ProposedWrite::Migrate { migration, .. } => st.complete(&mut migration.to),
        ProposedWrite::Declare { declarations } => {
            let mut after = st.clone();
            for d in declarations.iter() {
                let (hash, version) = identity_version(&after, d);
                after.declare(d, hash, version);
            }
            for d in declarations.iter_mut() {
                if let Declaration::Type(t) = d {
                    after.complete(t);
                }
            }
        }
        _ => {}
    }
}

/// As the store: a proposed write as its reader may know it, a value where
/// a read of the record would show it and a name where `describe` would.
/// False when the reader may see nothing of it. The proposal holds the
/// completed shape, so a relation's link fields are trimmed too.
fn known_shape(
    st: &State,
    who: &Principal,
    write: &mut ProposedWrite,
    (then, made, at): (&Principal, Option<&TypeDecl>, Position),
) -> bool {
    let seen = |ty: &TypeName| st.visible(who, ty).is_some();
    let read = |ty: &TypeName, f: &FieldName| {
        (st.visible(who, ty)).is_some_and(|t| readable(st, who, t, f).is_ok())
    };
    // As the store (051): a record its `only` does not admit, retired or
    // not, or after a proposed patch, is not answered.
    let stored = |ty: &TypeName, key: &RecordKey| st.records.get(&(ty.clone(), key.clone()));
    let answered = |ty: &TypeName, key: &RecordKey| {
        seen(ty) && stored(ty, key).is_none_or(|r| admits(who, ty, &r.fields))
    };
    match write {
        // A field the reader may not know of and a masked one both leave
        // the patch: it holds values, and no place for a name.
        ProposedWrite::Upsert { ty, key, patch, .. } => {
            let live = stored(ty, key).filter(|r| !r.retired);
            let mut after = live.map(|r| r.fields.clone()).unwrap_or_default();
            after.extend(patch.clone());
            patch.retain(|f, _| read(ty, f));
            answered(ty, key) && admits(who, ty, &after)
        }
        ProposedWrite::Retire { ty, key, .. } => answered(ty, key),
        // A payload is what the verb's fields become: whole to a reader who
        // may read each field the verb sets, and empty otherwise.
        ProposedWrite::Call {
            verb, key, payload, ..
        } => {
            let Some(v) = st.verbs.get(verb) else {
                return false;
            };
            if let VerbEffect::Write { set } = &v.effect
                && !set.iter().all(|a| read(&v.on, &a.field))
            {
                payload.clear();
            }
            key.as_ref().map_or(seen(&v.on), |k| answered(&v.on, k))
        }
        // The report names what the migration does to the type as it
        // stands; its totals stay whole, its hashes are of the shapes the
        // reader is shown, and its id is made at the head before the
        // proposal.
        ProposedWrite::Migrate { migration, report } => {
            let Some(was) = st.visible(who, &migration.to.name) else {
                return false;
            };
            shown_report(st, then, migration, (report, made.unwrap_or(&was.decl)));
            let before = Position(at.0.saturating_sub(1));
            shown_hashes(
                (st, then),
                made.unwrap_or(&was.decl),
                migration,
                (report, before),
            );
            known_migration(st, then, migration, made);
            true
        }
        // Each declaration as `describe` would show it once the bundle is
        // in force; one it would not show leaves the list.
        ProposedWrite::Declare { declarations } => {
            let mut after = st.clone();
            for d in declarations.iter() {
                let (hash, version) = identity_version(&after, d);
                after.declare(d, hash, version);
            }
            let who = &after.hidden(then);
            declarations.retain(|d| listed(&after, who, d));
            for d in declarations.iter_mut() {
                if let Declaration::Type(t) = d {
                    described_fields((&after, who), t);
                }
            }
            !declarations.is_empty()
        }
        // A use is shown only in a field the reader may read; the report's
        // totals stay whole, and a logged report fits, so it names no misfit.
        ProposedWrite::MigrateValueType { report, .. } => {
            report.fields.retain(|u| read(&u.ty, &u.field));
            true
        }
    }
}

/// As the store: a migration that is not in force as `who` may know it,
/// its shape judged with the grants held through its own renames, and a
/// rename only where `who` may know of the field under both names, the
/// old one in `made`, the type it was proposed against, else at the head.
fn known_migration(st: &State, who: &Principal, m: &mut Migration, made: Option<&TypeDecl>) {
    let then = following(who, m);
    let head = st.types.get(&m.to.name).map(|t| &t.decl);
    let mut was = made.or(head).cloned();
    if let Some(was) = &mut was {
        known_fields((st, who), was);
    }
    described_fields((st, &then), &mut m.to);
    let old = |n: &FieldName| was.as_ref().is_none_or(|w| holds(w, n));
    m.renames
        .retain(|(from, new)| old(from) && holds(&m.to, new));
}

/// As the store: `who` with its grants held through a migration's own
/// renames.
fn following(who: &Principal, m: &Migration) -> Principal {
    let (at, ty, renames) = (Position(u64::MAX), m.to.name.clone(), m.renames.clone());
    let mut list = vec![Renamed {
        at,
        ty,
        renames,
        to: None,
    }];
    list.extend(moved(m).into_iter().map(|(ty, to)| Renamed {
        at,
        ty,
        to: Some(to),
        ..Renamed::default()
    }));
    let mut then = who.clone();
    (then.grants).follow(&list, None);
    then
}

/// As the store: whether `who` may not know of some field, which only a
/// `none` its grants give hides.
pub(crate) fn hides_some(who: &Principal) -> bool {
    let (g, none) = (&who.grants, |a: &Access| *a == Access::None);
    let typed =
        |t: &TypeGrants| t.default.as_ref().is_some_and(none) || t.fields.values().any(none);
    none(&g.default) || g.types.values().any(typed)
}

/// As the store: the schema hash of `t` as `who` is shown it, `whole`
/// where nothing is left out.
fn shown_hash(
    (st, who): (&State, &Principal),
    t: &TypeDecl,
    view_shown: bool,
    whole: &str,
) -> String {
    let mut shown = t.clone();
    known_fields((st, who), &mut shown);
    shown.view = shown.view.filter(|_| view_shown);
    match shown == *t {
        true => whole.to_owned(),
        false => lakelands_api::shape::schema_hash(&shown).unwrap_or_default(),
    }
}

/// As the store: a report's hashes as `who` is shown them, set after
/// `more`, `from` of `made`, the type the report was made from; and its
/// id made again over what it shows at `head`.
fn shown_hashes(
    (st, who): (&State, &Principal),
    made: &TypeDecl,
    m: &Migration,
    (report, head): (&mut MigrationReport, Position),
) {
    if report.ty == m.to.name {
        report.from_schema_hash = shown_hash((st, who), made, true, &report.from_schema_hash);
        let then = following(who, m);
        report.to_schema_hash = shown_hash((st, &then), &m.to, true, &report.to_schema_hash);
    }
    report.report_id = report_id(report, head);
}

/// As the store: a report's id, the SHA-256 of what it says to its reader
/// and where the lake stood, in canonical form.
fn report_id(report: &MigrationReport, head: Position) -> String {
    let report = MigrationReport {
        report_id: String::new(),
        ..report.clone()
    };
    let report = serde_json::to_value(&report).unwrap_or_default();
    lakelands_api::canonical::sha256_hex(&serde_json::json!({"report": report, "head": head.0}))
}

/// As the store: whether a proposed declaration is listed to `who`, over
/// the state the proposal would leave: as `describe` would show it once in
/// force, but a verb by the fields it names, not by who may call it.
fn listed(st: &State, who: &Principal, d: &Declaration) -> bool {
    let described = |ty: &TypeName| st.visible(who, ty).is_some();
    let holds = |c: Option<&Condition>, ty: &TypeName| condition_visible(st, who, ty, c);
    match d {
        Declaration::Type(t) => {
            described(&t.name) && (t.view.as_deref()).is_none_or(|v| view_visible(st, who, v))
        }
        Declaration::Trigger(t) => described(&t.on) && holds(t.when.as_ref(), &t.on),
        Declaration::ObjectSet(s) => described(&s.of) && holds(Some(&s.filter), &s.of),
        Declaration::Verb(v) => {
            let VerbEffect::Write { set } = &v.effect else {
                return described(&v.on) && holds(v.requires.as_ref(), &v.on);
            };
            let t = st.types.get(&v.on).map(|t| &t.decl);
            let known = |n: &FieldName| {
                t.is_some_and(|t| field(t, n).is_some_and(|f| access(who, t, f) != Access::None))
            };
            let value = |c: &Condition| {
                let record = value_paths(c).into_iter();
                let record = record.filter(|p| expr::payload_member(p).is_none());
                let fields: Vec<FieldName> = record.filter_map(|p| p.first().cloned()).collect();
                reads_all(st, who, &v.on, &fields)
            };
            described(&v.on)
                && holds(v.requires.as_ref(), &v.on)
                && (who.grants.schema || set.iter().all(|a| known(&a.field) && value(&a.value)))
        }
        Declaration::Function(f) => function_visible(st, who, f),
        Declaration::ValueType(_) => true,
    }
}

/// The migration's preflight over the lake as it stands.
fn preflight(
    st: &State,
    head: Position,
    (m, author): (&Migration, Option<&Authored>),
    who: &Principal,
) -> Result<MigrationReport> {
    let ty = &m.to.name;
    let t = st.types.get(ty).ok_or_else(|| no_type(ty))?;
    // As the store: a view changes, or becomes a stored type, by being
    // declared again; a stored type never becomes a view.
    if t.decl.view.is_some() || m.to.view.is_some() {
        let ty = ty.clone();
        return Err(invalid(
            match (t.decl.view.is_some(), m.to.view.is_some()) {
                (true, true) => Case::MigrationOfView { ty },
                (true, false) => Case::MigrationViewToStored { ty },
                _ => Case::MigrationToView { ty },
            },
        ));
    }
    if t.decl.key != m.to.key {
        let what = Unmocked::Rekey;
        return Err(invalid(Case::MockCannot { what }));
    }
    // As the store: the shape is checked as a declaration, its reshape
    // forgiven, so a name it holds twice is found here.
    check_declaration(st, who, &Declaration::Type(m.to.clone())).or_else(|e| {
        match e.case().map(|s| &s.case) {
            Some(Case::Reshapes { .. } | Case::ReshapesLinks { .. }) => Ok(()),
            _ => Err(e),
        }
    })?;
    // The refusal of a migration that takes away a field a declaration
    // reads, said after the migration, at the reader where it is named.
    let after_it = |said: Said| {
        let name = ty.clone();
        invalid(said.within(Place::AfterMigration { name }))
    };
    let taken = |reader: Option<Place>, n: &FieldName, is: &TypeName| {
        let renamed = |(from, _): &&(FieldName, FieldName)| from == n && is == ty;
        let renamed = m.renames.iter().find(renamed).map(|(_, to)| to.clone());
        let (unnamed, field, ty) = (reader.is_none(), n.clone(), is.clone());
        let said = Said::from(Case::MigrationTakesRead {
            unnamed,
            field,
            ty,
            renamed,
        });
        after_it(match reader {
            Some(reader) => said.within(reader),
            None => said,
        })
    };
    // As the store: a view over the type, as its source or a merged one,
    // reads the shape the migration leaves: it is declared again against
    // it, and every field it reads (grouping, `where`, what its measures
    // aggregate) must remain.
    let mut after = st.clone();
    let to = Declaration::Type(m.to.clone());
    let (hash, version) = identity_version(&after, &to);
    after.declare(&to, hash, version);
    for o in st.types.values() {
        let Some(v) = o.decl.view.as_deref() else {
            continue;
        };
        if &v.source != ty && !v.merge.iter().any(|s| &s.source == ty) {
            continue;
        }
        let reads = view_reads(v);
        let gone = |reader: Option<Place>| {
            let of = reads.iter().filter(|(source, _)| source == ty);
            of.flat_map(|(_, names)| names)
                .find(|n| field(&t.decl, n).is_some() && field(&m.to, n).is_none())
                .map(|n| taken(reader, n, ty))
        };
        let after_place = Place::AfterMigration { name: ty.clone() };
        let view = Declaration::Type(o.decl.clone());
        let checked = check_declaration(&after, who, &view).map_err(|e| e.within(after_place));
        let name = o.decl.name.clone();
        let named = checked.and_then(|()| gone(Some(Place::View { name })).map_or(Ok(()), Err));
        let Err(named) = named else { continue };
        // As the store: the view is named only to an author `describe`
        // shows its definition to; another is told that a declaration
        // reads the field, or that one could not be declared.
        let seen = describe_lake(&who.lake, st, who);
        let shown = |s: &TypeDescription| s.name == o.decl.name && s.view.is_some();
        if seen.types.iter().any(shown) {
            return Err(named);
        }
        return Err(gone(None).unwrap_or_else(|| {
            let ty = ty.clone();
            after_it(Case::NoLongerDeclarable { ty }.into())
        }));
    }
    // As the store: so does every other declaration in force, in the
    // store's order (types, verbs, triggers, object sets, each by name):
    // it keeps the fields it reads of the type, and of each link type the
    // migration changes.
    let mut readers: Vec<Declaration> = (st.types.values())
        .map(|o| {
            Declaration::Type(if &o.decl.name == ty {
                m.to.clone()
            } else {
                o.decl.clone()
            })
        })
        .chain(st.verbs.values().cloned().map(Declaration::Verb))
        .collect();
    let (mut triggers, mut sets) = (st.triggers.clone(), st.object_sets.clone());
    triggers.sort_by(|a, b| a.name.cmp(&b.name));
    sets.sort_by(|a, b| a.name.cmp(&b.name));
    readers.extend(triggers.into_iter().map(Declaration::Trigger));
    readers.extend(sets.into_iter().map(Declaration::ObjectSet));
    let changed: Vec<(&TypeDecl, TypeDecl)> = links::of(&m.to)
        .into_iter()
        .filter_map(|is| Some((&st.types.get(&is.name)?.decl, is)))
        .collect();
    let kept = changed.iter().map(|(was, is)| (*was, is));
    for (was, is) in std::iter::once((&t.decl, &m.to)).chain(kept) {
        for d in &readers {
            let reads = declaration_reads(st, d, &was.name);
            let gone = |n: &&FieldName| field(was, n).is_some() && field(is, n).is_none();
            let Some(n) = reads.iter().find(gone) else {
                continue;
            };
            // As the store: an accepter who may not know of the field is told
            // neither it nor its reader.
            if field(was, n).is_some_and(|f| access(who, was, f) == Access::None) {
                return Err(after_it(Case::ReadsTakenAway.into()));
            }
            // As the store: the reader is named only to an author `describe`
            // shows it to; another is told that a declaration reads the
            // field, which is one it knows of (a hidden one was put back).
            let seen = describe_lake(&who.lake, st, who);
            let shown = match d {
                Declaration::Type(t) => (seen.types.iter())
                    .any(|s| s.name == t.name && (t.view.is_none() || s.view.is_some())),
                Declaration::Verb(v) => seen.verbs.iter().any(|s| s.name == v.name),
                Declaration::Trigger(t) => seen.triggers.iter().any(|s| s.name == t.name),
                Declaration::ObjectSet(o) => seen.object_sets.iter().any(|s| s.name == o.name),
                _ => true,
            };
            // A type by what it is shown of it: a relation it is not shown
            // reads nothing it is told of.
            let own = match d {
                Declaration::Type(t) if shown => {
                    let mut t = t.clone();
                    known_fields((st, who), &mut t);
                    let seen = declaration_reads(st, &Declaration::Type(t), &was.name);
                    seen.into_iter().find(|n| gone(&n))
                }
                _ => shown.then(|| n.clone()),
            };
            return Err(match own {
                Some(n) => taken(Some(d.place()), &n, &is.name),
                None => taken(None, n, &is.name),
            });
        }
    }
    // As the store: and the type it migrates to reads only fields it has.
    reads_its_type(st, &Declaration::Type(m.to.clone()))?;
    // As the store: the renames as their author sent them are judged
    // against the type as it sees it, then against the type.
    if let Some(a) = author {
        a.sent.renamed_fields(&a.known).map_err(invalid)?;
    }
    m.renamed_fields(&t.decl).map_err(invalid)?;
    let renamed = |old: &FieldName| -> FieldName {
        m.renames
            .iter()
            .find(|(from, _)| from == old)
            .map_or(old, |(_, to)| to)
            .clone()
    };
    let live: Vec<&StoredRecord> = st
        .records
        .values()
        .filter(|r| &r.ty == ty && !r.retired)
        .collect();
    // As the store: a field is checked when its type changes, and when it
    // becomes required or is added as required, where a record without a
    // value does not convert; in the new fields' name order.
    let mut fields = Vec::new();
    let mut to: Vec<&FieldDecl> = m.to.fields.iter().collect();
    to.sort_by(|a, b| a.name.cmp(&b.name));
    for new in to {
        let old = t
            .decl
            .fields
            .iter()
            .find(|old| renamed(&old.name) == new.name);
        let retyped = old.is_some_and(|old| old.ty != new.ty);
        if !retyped && !(new.required && !old.is_some_and(|old| old.required)) {
            continue;
        }
        let mut conv = FieldConversion {
            field: new.name.clone(),
            from: old.map_or(&new.ty, |old| &old.ty).clone(),
            to: new.ty.clone(),
            convertible: 0,
            not_convertible: 0,
            sample_keys: Vec::new(),
        };
        for r in &live {
            match old.and_then(|old| r.fields.get(&old.name)) {
                None if !new.required => {}
                Some(v) if value::convert(v, &new.ty).is_some() => conv.convertible += 1,
                _ => {
                    conv.not_convertible += 1;
                    if conv.sample_keys.len() < 5 {
                        conv.sample_keys.push(r.key.clone());
                    }
                }
            }
        }
        fields.push(conv);
    }
    // Fields the new declaration no longer has under any name: the
    // migration drops them, and the report says how many values that is.
    let mut dropped: Vec<(FieldName, u64)> = t
        .decl
        .fields
        .iter()
        .filter(|old| field(&m.to, &renamed(&old.name)).is_none())
        .map(|old| {
            let holding = live
                .iter()
                .filter(|r| r.fields.get(&old.name).is_some_and(|v| !v.is_null()))
                .count() as u64;
            (old.name.clone(), holding)
        })
        .collect();
    // As the store: a renamed relation keeps its edges; its link records
    // move to the new name's link type, which must hold none.
    if let Some(a) = author {
        a.sent.renamed_relations(&a.known).map_err(invalid)?;
    }
    let pairs = m.renamed_relations(&t.decl).map_err(invalid)?;
    let held = |lt: &TypeName| st.records.values().filter(|r| &r.ty == lt).count() as u64;
    let mut relations = Vec::new();
    for (was, is) in &pairs {
        let carrier = lakelands_api::write::Carrier::of(was);
        let by_links = carrier == lakelands_api::write::Carrier::Links;
        let wrong = |why: Case| {
            let (from, to) = (was.name.clone(), is.name.clone());
            invalid(Said::from(why).within(Place::RelationRename { from, to }))
        };
        if by_links && held(&links::name(ty, &is.name)) > 0 {
            return Err(wrong(Case::RenameNameHeld));
        }
        // As the store: a reference into the link type would name keys that
        // end, and the type that refers to it is named to one who sees it.
        let old = links::name(ty, &was.name);
        let refers = |d: &&DeclaredType| {
            let field = |f: &FieldDecl| f.ty.target.as_ref() == Some(&old);
            d.decl.fields.iter().any(field) || d.decl.relations.iter().any(|r| r.target == old)
        };
        let mut referring = st
            .types
            .values()
            .filter(|d| by_links && refers(d))
            .peekable();
        if referring.peek().is_some() {
            let named = referring.find(|d| st.visible(who, &d.decl.name).is_some());
            let (ty, links) = (named.map(|d| d.decl.name.clone()), old.clone());
            return Err(wrong(Case::RenameReferredLinks { ty, links }));
        }
        relations.push(lakelands_api::write::RelationRename {
            from: was.name.clone(),
            to: is.name.clone(),
            carrier,
            links: if by_links {
                held(&links::name(ty, &was.name))
            } else {
                0
            },
        });
    }
    let carried = carried(st, m).iter().map(carried_entry).collect();
    // A relation the migration removes is listed as the store lists it:
    // with the number of its live links at the head, none for one not
    // carried by link records.
    for r in &t.decl.relations {
        let lt = links::name(ty, &r.name);
        let live = st.records.values().filter(|x| x.ty == lt && !x.retired);
        if !m.to.relations.iter().any(|kept| kept.name == r.name)
            && !pairs.iter().any(|(was, _)| was.name == r.name)
        {
            dropped.push((FieldName::from(r.name.as_str()), live.count() as u64));
        }
    }
    // As the store: what `to` states otherwise, both shapes completed.
    let (mut was, mut is) = (t.decl.clone(), m.clone());
    st.complete(&mut was);
    st.complete(&mut is.to);
    let restated = is.restated(&was);
    let mut report = MigrationReport {
        ty: ty.clone(),
        from_schema_hash: t.schema_hash.clone(),
        to_schema_hash: lakelands_api::shape::schema_hash(&m.to).map_err(invalid)?,
        records_scanned: live.len() as u64,
        lossless: fields.iter().all(|f| f.not_convertible == 0),
        fields,
        dropped,
        rekeyed: Vec::new(),
        references: Vec::new(),
        sample_collisions: Vec::new(),
        report_id: String::new(),
        more: false,
        relations,
        carried,
        restated,
    };
    report.report_id = report_id(&report, head);
    Ok(report)
}

/// The content address of one ingested record.
/// An ingested record's id, from what the log format derives it from: its
/// source, type, key, the record's version before it and its values. When
/// it was seen is not part of it, so a re-export of the same values is the
/// same event.
fn content_id(
    source: &SourceId,
    r: &SourceRecord,
    version_before: Version,
    fields: &Patch,
) -> EventId {
    let bytes =
        serde_json::to_vec(&(source, &r.ty, &r.key, version_before.0, fields)).unwrap_or_default();
    EventId(value::hex(&bytes, 2))
}

/// The positions and versions an ingest window planned so far.
struct Window {
    batch: Batch,
    /// The fields each record holds after what the window planned so far.
    planned: BTreeMap<(TypeName, RecordKey), BTreeMap<FieldName, Value>>,
    versions: BTreeMap<(TypeName, RecordKey), Version>,
    ids: BTreeSet<EventId>,
    records: u32,
    duplicates: u32,
}

#[async_trait]
impl Catalog for MockCatalog {
    fn lake(&self) -> &LakeId {
        &self.shared.lake
    }

    async fn describe(&self, who: &Principal, at: ReadAt) -> Result<Reply<LakeDescription>> {
        self.admit(who)?;
        let who = &self.followed(who, at.as_of);
        let (position, st) = self.state_at(&self.log(), at.as_of)?;
        Ok(Reply {
            position,
            data: describe_lake(&self.shared.lake, &st, who),
        })
    }

    async fn describe_type(
        &self,
        who: &Principal,
        ty: &TypeName,
        at: ReadAt,
    ) -> Result<Reply<TypeDescription>> {
        self.admit(who)?;
        let who = &self.followed(who, at.as_of);
        let (position, st) = self.state_at(&self.log(), at.as_of)?;
        let t = st.visible(who, ty).ok_or_else(|| no_type(ty))?;
        Ok(Reply {
            position,
            data: describe_type(&st, who, t),
        })
    }

    async fn find(&self, who: &Principal, ty: &TypeName, find: Find) -> Result<Reply<Found>> {
        self.admit(who)?;
        let who = &self.followed(who, find.at.as_of);
        let of = read_of(serde_json::to_string(&(
            &find.filter,
            &find.order_by,
            find.at.valid_at,
        )));
        let (pinned, offset) = match &find.cursor {
            Some(c) => {
                let (p, o) = uncursor(c, "find", Some(ty), &of)?;
                (Some(p), o)
            }
            None => (find.at.as_of, 0),
        };
        let (position, st) = self.state_at(&self.log(), pinned)?;
        let t = st.visible(who, ty).ok_or_else(|| no_type(ty))?;
        check_only(who, t)?;
        bound(&t.decl, &find.params)?;
        view_readable(&st, who, t)?;
        let after = find
            .after
            .as_ref()
            .map(|a| resume_after(&st, t, &find, a))
            .transpose()?;
        if find.total && find.aggregate.is_some() {
            return Err(invalid(Case::TotalWithAggregate));
        }
        let skip = match find.offset {
            None => 0,
            Some(skip) => {
                let beside = [
                    (find.cursor.is_some(), "cursor"),
                    (find.after.is_some(), "after"),
                    (find.aggregate.is_some(), "aggregate"),
                ];
                if let Some((_, other)) = beside.iter().find(|(given, _)| *given) {
                    let other = (*other).to_owned();
                    return Err(invalid(Case::OffsetBeside { other }));
                }
                let max = self.shared.limits.max_offset;
                if skip > max {
                    return Err(on(&"offset".into(), Refusal::TooLarge { max }));
                }
                skip as usize
            }
        };
        if find.aggregate.is_some() {
            let what = Unmocked::Aggregate;
            return Err(invalid(Case::MockCannot { what }));
        }
        let e = filter(&st, who, t, find.filter.as_ref())?;
        for o in &find.order_by {
            if let Some((field, case)) = o.misfit() {
                let reason = Refusal::Invalid(Box::new(case.into()));
                return Err(Error::Refused { field, reason });
            }
            if o.by == OrderBy::Latest && t.decl.view.is_some() {
                let ty = Some(ty.clone());
                return Err(invalid(Case::LatestNoChange { ty }));
            }
            if o.by != OrderBy::Latest {
                readable(&st, who, t, &o.field)?;
                steady_order(&t.decl, o.field.as_str())?;
            }
        }
        let mut rows = candidates(&st, who, t, find.at, e.as_ref())?;
        let total = find.total.then_some(rows.len() as u64);
        let sort_key = |r: &StoredRecord| -> Vec<Option<Value>> {
            find.order_by
                .iter()
                .map(|o| match o.by {
                    OrderBy::Latest => Some(r.position.0.into()),
                    OrderBy::Value => r.fields.get(&o.field).cloned().filter(|v| !v.is_null()),
                    OrderBy::KnownAt => r.meta.get(&o.field).map(|m| m.known_at.to_string().into()),
                    OrderBy::SeenAt => r
                        .meta
                        .get(&o.field)
                        .and_then(|m| m.seen_at)
                        .map(|s| s.to_string().into()),
                })
                .collect()
        };
        rows.sort_by(|a, b| {
            in_order(
                &find.order_by,
                (&sort_key(a), &a.key),
                (&sort_key(b), &b.key),
            )
        });
        if let Some((values, key)) = &after {
            rows.retain(|r| {
                in_order(&find.order_by, (&sort_key(r), &r.key), (values, key)).is_gt()
            });
        }
        let rows: Vec<Record> = rows
            .into_iter()
            .map(|r| st.read(who, r, find.with_meta))
            .collect();
        let (rows, next_cursor) = page(
            rows,
            (position, "find", Some(ty), &of),
            offset + skip,
            self.limit(find.limit, false)?,
        );
        Ok(Reply {
            position,
            data: Found::Records {
                rows,
                next_cursor,
                total,
            },
        })
    }

    async fn get(&self, who: &Principal, ty: &TypeName, get: Get) -> Result<Reply<Record>> {
        self.admit(who)?;
        let who = &self.followed(who, get.at.as_of);
        let (position, st) = self.state_at(&self.log(), get.at.as_of)?;
        let t = st.visible(who, ty).ok_or_else(|| no_type(ty))?;
        check_only(who, t)?;
        bound(&t.decl, &get.params)?;
        view_readable(&st, who, t)?;
        let valid = valid_time_readable(&st, who, t, get.at)?;
        let r = st
            .live(ty, &get.key)
            .filter(|r| !r.restricted || who.grants.restricted)
            .filter(|r| valid_at(t, r, valid) && st.admitted(who, r))
            .ok_or_else(|| no_record(ty, &get.key))?;
        Ok(Reply {
            position,
            data: st.read(who, r, get.with_meta),
        })
    }

    async fn upsert(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        if links::is_link_type(ty) {
            return Err(refused(Refusal::WrongDoor));
        }
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        if let Some(o) = remembered(&st, who, &opts) {
            return Ok(o);
        }
        let mut batch = Batch::at(&log);
        let by = By {
            st: &st,
            who,
            cause: None,
        };
        let version = plan_upsert(&by, &mut batch, ty, key, patch, &opts)?;
        let position = batch.peek().0 - 1;
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position: Position(position),
            version: Some(version),
            output: None,
            set_off: Default::default(),
            renamed: Vec::new(),
        })
    }

    async fn retire(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        if links::is_link_type(ty) {
            return Err(refused(Refusal::WrongDoor));
        }
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        if let Some(o) = remembered(&st, who, &opts) {
            return Ok(o);
        }
        let mut batch = Batch::at(&log);
        let by = By {
            st: &st,
            who,
            cause: None,
        };
        let version = plan_retire(&by, &mut batch, ty, key, &opts)?;
        let position = Position(batch.peek().0 - 1);
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position,
            version: Some(version),
            output: None,
            set_off: Default::default(),
            renamed: Vec::new(),
        })
    }

    async fn link(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        relation: &RelationName,
        to: &RecordKey,
        fields: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        if let Some(o) = remembered(&st, who, &opts) {
            return Ok(o);
        }
        let r = link_relation(&st, who, ty, relation)?;
        if r.authority != Authority::Authored {
            return Err(refused(Refusal::WrongDoor));
        }
        if let Some(f) = [links::FROM, links::TO]
            .into_iter()
            .find(|f| fields.contains_key(*f))
        {
            let end = FieldName::from(f);
            return Err(invalid_field(
                &end,
                Case::LinkEndAsField { end: end.clone() },
            ));
        }
        let lt = links::name(ty, relation);
        let mut patch = fields;
        patch.insert(links::FROM.into(), Value::from(key.as_str()));
        patch.insert(links::TO.into(), Value::from(to.as_str()));
        let mut batch = Batch::at(&log);
        let by = By {
            st: &st,
            who,
            cause: None,
        };
        let version = plan_upsert(
            &by,
            &mut batch,
            &lt,
            &lakelands_api::key::link_key(key, to),
            patch,
            &opts,
        )?;
        // The cardinality reads other links: after the grants (024).
        link_cardinality(&st, who, &lt, &r, key, to)?;
        let position = batch.peek().0 - 1;
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position: Position(position),
            version: Some(version),
            output: None,
            set_off: Default::default(),
            renamed: Vec::new(),
        })
    }

    async fn unlink(
        &self,
        who: &Principal,
        ty: &TypeName,
        key: &RecordKey,
        relation: &RelationName,
        to: &RecordKey,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        if let Some(o) = remembered(&st, who, &opts) {
            return Ok(o);
        }
        link_relation(&st, who, ty, relation)?;
        let lt = links::name(ty, relation);
        let mut batch = Batch::at(&log);
        let by = By {
            st: &st,
            who,
            cause: None,
        };
        let version = plan_retire(
            &by,
            &mut batch,
            &lt,
            &lakelands_api::key::link_key(key, to),
            &opts,
        )?;
        let position = Position(batch.peek().0 - 1);
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position,
            version: Some(version),
            output: None,
            set_off: Default::default(),
            renamed: Vec::new(),
        })
    }

    async fn call(
        &self,
        who: &Principal,
        verb: &VerbName,
        key: Option<&RecordKey>,
        payload: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        if let Some(o) = remembered(&st, who, &opts) {
            return Ok(o);
        }
        let v = check_call(&st, who, verb, key, &payload, &opts)?.clone();
        // Past the executor, only a verb on a record is left.
        let key = record_key(&v, key)?;
        check_version(&st, &v.on, key, &opts)?;
        let mut batch = Batch::at(&log);
        let runs = v.risk == Risk::Low && matches!(v.effect, VerbEffect::Write { .. });
        let outcome = if runs {
            let by = By {
                st: &st,
                who,
                cause: None,
            };
            let version = plan_run(&by, &mut batch, &v, key, payload, &opts)?;
            Outcome::Applied {
                position: Position(batch.peek().0 - 1),
                version: Some(version),
                output: None,
                set_off: Default::default(),
                renamed: Vec::new(),
            }
        } else {
            let why = format!(
                "`{verb}` is declared {} risk; it runs when accepted",
                risk_word(v.risk)
            );
            let key_ = opts.idempotency_key.clone();
            let write = ProposedWrite::Call {
                verb: verb.clone(),
                key: Some(key.clone()),
                payload,
                opts,
            };
            let pinned = pinned(&st, who, &write)?;
            let (position, proposal) =
                plan_propose(&st, &mut batch, who, (write, pinned), why.clone(), key_)?;
            log.proposers.insert(proposal.clone(), who.clone());
            Outcome::Proposed {
                position,
                proposal,
                why,
            }
        };
        self.commit(&mut log, batch);
        Ok(outcome)
    }

    async fn query(&self, who: &Principal, pattern: Pattern) -> Result<Reply<QueryResult>> {
        self.admit(who)?;
        let who = &self.followed(who, pattern.at.as_of);
        let of = read_of(serde_json::to_string(&Pattern {
            cursor: None,
            limit: None,
            at: ReadAt {
                as_of: None,
                ..pattern.at
            },
            ..pattern.clone()
        }));
        let (pinned, offset) = match &pattern.cursor {
            Some(c) => {
                let (p, o) = uncursor(c, "query", Some(&pattern.start.ty), &of)?;
                (Some(p), o)
            }
            None => (pattern.at.as_of, 0),
        };
        let (position, st) = self.state_at(&self.log(), pinned)?;
        let start = &pattern.start;
        let t = st
            .visible(who, &start.ty)
            .ok_or_else(|| no_type(&start.ty))?;
        check_only(who, t)?;
        bound(&t.decl, &start.params)?;
        view_readable(&st, who, t)?;
        let measure = matches!(pattern.project, Projection::Degree | Projection::Components);
        let limit = self.limit(pattern.limit, measure)?;
        if start.search.is_some() {
            let what = Unmocked::PatternStart;
            return Err(invalid(Case::MockCannot { what }));
        }
        if pattern.to.is_some() && !measure {
            let what = Unmocked::PatternTo;
            return Err(invalid(Case::MockCannot { what }));
        }
        let step_at = |i: usize, relation: &RelationName| Place::Step {
            step: i + 1,
            relation: relation.clone(),
        };
        if measure {
            measures::check(&pattern)?;
            for (i, step) in pattern.steps.iter().enumerate() {
                let declared = st.types.values().any(|o| {
                    o.decl.relations.iter().any(|r| {
                        r.name == step.relation
                            && relation_shown(&st, who, &o.decl, r)
                            && (pattern.project == Projection::Components
                                || (step.walk != Walk::In && o.decl.name == t.decl.name)
                                || (step.walk != Walk::Out && r.target == t.decl.name))
                    })
                });
                if !declared {
                    let (ty, relation, walk) =
                        (t.decl.name.clone(), step.relation.clone(), step.walk);
                    return Err(invalid(Case::NoRelationToWalk { ty, relation, walk }));
                }
                // Either end of the relation the measure may arrive at.
                for o in st.types.values() {
                    for r in o.decl.relations.iter().filter(|r| r.name == step.relation) {
                        for end in [&o.decl.name, &r.target] {
                            if let Some(e) = st.types.get(end) {
                                unbound_view(&t.decl.name, e, step_at(i, &step.relation))?;
                            }
                        }
                    }
                }
            }
            let e = filter(&st, who, t, start.filter.as_ref())?;
            let mut seeds: Vec<(TypeName, RecordKey)> =
                candidates(&st, who, t, pattern.at, e.as_ref())?
                    .into_iter()
                    .filter(|r| start.key.as_ref().is_none_or(|k| &r.key == k))
                    .map(|r| (r.ty.clone(), r.key.clone()))
                    .collect();
            seeds.sort();
            let rows = measures::rows(
                &st,
                who,
                &pattern,
                seeds,
                self.shared.limits.max_graph_nodes,
            )?;
            let (rows, next_cursor) = page(
                rows,
                (position, "query", Some(&pattern.start.ty), &of),
                offset,
                limit,
            );
            return Ok(Reply {
                position,
                data: QueryResult::Rows { rows, next_cursor },
            });
        }
        let Projection::Rows { fields } = &pattern.project else {
            let what = Unmocked::PatternProjection;
            return Err(invalid(Case::MockCannot { what }));
        };
        // Rows are ordered by their paths, `type.field`: each probes its
        // field as `find`'s order does (024), on a type the principal is
        // shown; another reads nothing (006, extended). The mock answers
        // its rows in walk order whatever the order asks for.
        for o in &pattern.order_by {
            if let Some((ty, field)) = o.field.as_str().rsplit_once('.')
                && let Some(ot) = st.visible(who, &TypeName::from(ty))
            {
                readable(&st, who, ot, &FieldName::from(field))?;
                steady_order(&ot.decl, field)?;
            }
        }
        let e = filter(&st, who, t, start.filter.as_ref())?;
        let mut paths: Vec<Vec<&StoredRecord>> = candidates(&st, who, t, pattern.at, e.as_ref())?
            .into_iter()
            .filter(|r| start.key.as_ref().is_none_or(|k| &r.key == k))
            .map(|r| vec![r])
            .collect();
        let mut scope_ty = t;
        for (i, step) in pattern.steps.iter().enumerate() {
            // A step over a relation the type does not declare is
            // refused, not answered empty. A step naming a relation's
            // `inverse` walks that relation the other way; a relation
            // declared under the name wins.
            let shown = |o: &TypeDecl, r: &RelationDecl| relation_shown(&st, who, o, r);
            let forward = |named: &dyn Fn(&RelationDecl) -> bool| {
                let relations = scope_ty.decl.relations.iter();
                let r = relations
                    .filter(|r| shown(&scope_ty.decl, r))
                    .find(|r| named(r))?;
                let link = links::name(&scope_ty.decl.name, &r.name);
                Some((
                    r.target.clone(),
                    (r.via.clone(), r.derived.clone()),
                    true,
                    link,
                ))
            };
            let backward = |named: &dyn Fn(&RelationDecl) -> bool| {
                st.types.values().find_map(|o| {
                    let r = o
                        .decl
                        .relations
                        .iter()
                        .find(|r| r.target == scope_ty.decl.name && named(r))?;
                    // As the store: named from its target's end only to
                    // one shown the relation (006, extended).
                    shown(&o.decl, r).then_some(())?;
                    let link = links::name(&o.decl.name, &r.name);
                    Some((
                        o.decl.name.clone(),
                        (r.via.clone(), r.derived.clone()),
                        false,
                        link,
                    ))
                })
            };
            let name = |r: &RelationDecl| r.name == step.relation;
            let inverse = |r: &RelationDecl| r.inverse.as_ref() == Some(&step.relation);
            let walked = match step.walk {
                Walk::Out => forward(&name).or_else(|| backward(&inverse)),
                _ => backward(&name).or_else(|| forward(&inverse)),
            };
            let Some((target, via, forward, link_type)) = walked else {
                let (ty, relation) = (scope_ty.decl.name.clone(), step.relation.clone());
                let walk = step.walk;
                return Err(invalid(Case::NoRelationToWalk { ty, relation, walk }));
            };
            if step.depth.min != 1 || step.depth.max != 1 || step.walk == Walk::Both {
                let what = Unmocked::Hops;
                return Err(invalid(Case::MockCannot { what }));
            }
            // A relation without `via` is walked over its live link records
            // its walker's grants admit (051).
            let linked_by = |from: &RecordKey, to: &RecordKey| {
                st.live(&link_type, &lakelands_api::key::link_key(from, to))
                    .is_some_and(|l| st.admitted(who, l))
            };
            let next = st.visible(who, &target).ok_or_else(|| no_type(&target))?;
            view_readable(&st, who, next)?;
            unbound_view(&t.decl.name, next, step_at(i, &step.relation))?;
            if let Some(ty) = &step.ty
                && ty != &target
            {
                let (reached, ty) = (target.clone(), ty.clone());
                let said = Said::from(Case::StepReaches { reached, ty });
                return Err(invalid(said.within(step_at(i, &step.relation))));
            }
            // A relation probes the reference field that carries it, and a
            // derived one its matched fields on both sides (024, 044).
            let (owner, target) = if forward {
                (scope_ty, next)
            } else {
                (next, scope_ty)
            };
            if let (Some(v), _) = &via {
                readable(&st, who, owner, v)?;
            }
            if let (_, Some(d)) = &via {
                for p in &d.pairs {
                    readable(&st, who, owner, &p.from)?;
                    readable(&st, who, target, &p.to)?;
                }
            }
            let se = filter(&st, who, next, step.filter.as_ref())?;
            let reached = candidates(&st, who, next, pattern.at, se.as_ref())?;
            let mut out = Vec::new();
            for p in paths {
                let last = p[p.len() - 1];
                for r in &reached {
                    let linked = match (&via, forward) {
                        ((_, Some(d)), true) => derived_match(&d.pairs, last, r),
                        ((_, Some(d)), false) => derived_match(&d.pairs, r, last),
                        ((Some(via), None), true) => {
                            last.fields.get(via).and_then(Value::as_str) == Some(r.key.as_str())
                        }
                        ((Some(via), None), false) => {
                            r.fields.get(via).and_then(Value::as_str) == Some(last.key.as_str())
                        }
                        ((None, None), true) => linked_by(&last.key, &r.key),
                        ((None, None), false) => linked_by(&r.key, &last.key),
                    };
                    if linked {
                        let mut q = p.clone();
                        q.push(r);
                        out.push(q);
                    }
                }
            }
            paths = out;
            scope_ty = next;
        }
        let rows: Vec<BTreeMap<String, Value>> = paths
            .iter()
            .map(|p| {
                let mut row = BTreeMap::new();
                for r in p {
                    let read = st.read(who, r, false);
                    row.insert(format!("{}.key", r.ty), Value::from(r.key.as_str()));
                    for (f, v) in read.fields {
                        row.insert(format!("{}.{f}", r.ty), v);
                    }
                }
                if !fields.is_empty() {
                    row.retain(|k, _| fields.contains(k));
                }
                row
            })
            .collect();
        let (rows, next_cursor) = page(
            rows,
            (position, "query", Some(&pattern.start.ty), &of),
            offset,
            limit,
        );
        Ok(Reply {
            position,
            data: QueryResult::Rows { rows, next_cursor },
        })
    }

    async fn search(&self, who: &Principal, search: Search) -> Result<Reply<Hits>> {
        self.admit(who)?;
        let given = who;
        let who = &self.followed(who, search.at.as_of);
        let of = read_of(serde_json::to_string(&(
            &search.mode,
            &search.filter,
            search.at.valid_at,
        )));
        let (pinned, offset) = match &search.cursor {
            Some(c) => {
                let (p, o) = uncursor(c, "search", Some(&search.ty), &of)?;
                (Some(p), o)
            }
            None => (search.at.as_of, 0),
        };
        let (position, st) = self.state_at(&self.log(), pinned)?;
        let t = st
            .visible(who, &search.ty)
            .ok_or_else(|| no_type(&search.ty))?;
        check_only(who, t)?;
        if t.decl.view.is_some() {
            let ty = search.ty.clone();
            return Err(invalid(Case::ViewNotSearched { ty }));
        }
        if let Some((field, case)) = search.mode.misfit() {
            let reason = Refusal::Invalid(Box::new(case.into()));
            return Err(Error::Refused { field, reason });
        }
        // A search probes every searchable field, or the ones it names,
        // then each vector field it names, after the fields that vector
        // is embedded from; one the caller may not read refuses the
        // search whole (024), in the store's order. A named field is
        // judged readable before searchable.
        let text_fields = match &search.mode {
            SearchMode::Lexical { fields, .. } | SearchMode::Hybrid { fields, .. } => {
                fields.as_deref()
            }
            SearchMode::Vector { .. } => None,
        };
        let vectors = search.mode.vectors();
        let probed = vectors.iter().flat_map(|(v, ..)| {
            let embed = field(&t.decl, v).and_then(|f| f.embed.as_ref());
            embed.into_iter().flat_map(|e| e.from.iter()).chain([*v])
        });
        let searched: Vec<&FieldName> = match text_fields {
            // Naming none, it reads the searchable fields the caller reads;
            // one it may not read is never searched.
            None => t
                .decl
                .fields
                .iter()
                .filter(|f| f.searchable && access(who, &t.decl, f).reads())
                .map(|f| &f.name)
                .collect(),
            Some([]) => return Err(invalid(Case::SearchFieldsEmpty)),
            Some(named) => {
                let named: BTreeSet<&FieldName> = named.iter().collect();
                for name in &named {
                    readable(&st, who, t, name)?;
                    if !field(&t.decl, name).is_some_and(|f| f.searchable) {
                        let (field, ty) = ((*name).clone(), t.decl.name.clone());
                        return Err(invalid_field(name, Case::NotSearchable { field, ty }));
                    }
                }
                named.into_iter().collect()
            }
        };
        for name in searched.iter().copied().chain(probed) {
            readable(&st, who, t, name)?;
        }
        // As the store: a vector ranking reads a vector field, and a hybrid
        // search without a vector reads one a model fills.
        for (name, ..) in &vectors {
            if let Some(f) = field(&t.decl, name)
                .filter(|f| f.ty.kind != lakelands_api::ontology::StorageKind::Vector)
            {
                let (field, kind) = ((*name).clone(), f.ty.kind);
                return Err(invalid_field(name, Case::SearchNotVector { field, kind }));
            }
        }
        if let SearchMode::Hybrid {
            field: name,
            vector: None,
            ..
        } = &search.mode
            && field(&t.decl, name).is_some_and(|f| f.embed.is_none())
        {
            let case = Case::SearchNeedsVector {
                field: name.clone(),
            };
            return Err(invalid_field(name, case));
        }
        // As the store: a query vector is judged by the field's declared
        // length, never by a stored vector.
        let embedded = matches!(&search.mode, SearchMode::Hybrid { vector: None, .. });
        for (f, v, _) in vectors.iter().filter(|_| !embedded) {
            let d = field(&t.decl, f).and_then(|f| f.ty.dimensions);
            if let Some(d) = d.filter(|d| v.len() != *d as usize) {
                let case = Case::QueryDimensions {
                    query: v.len(),
                    held: d as usize,
                };
                let at = Said::from(case).within(Place::Field { name: (*f).clone() });
                let name = search.ty.clone();
                return Err(invalid(at.within(Place::Type { name })));
            }
        }
        let e = filter(&st, who, t, search.filter.as_ref())?;
        let mut rows = candidates(&st, who, t, search.at, e.as_ref())?;
        // As the store (051): a holder's search before the head keeps a
        // candidate only where the head admits it too, a link by its ends,
        // under the grant followed there and by the name its type bears
        // there, so that a ranking by the head's text and vectors finds no
        // other's record.
        if pinned.is_some() && only_holder(who, &t.decl) {
            // One lock at a time: `followed` reads the log too.
            let head = self.state_at(&self.log(), None)?.1;
            let now = self.followed(given, None);
            let mut name = t.decl.name.clone();
            for r in self.renames() {
                if let Some(to) = r.to.filter(|_| r.at > position && r.ty == name) {
                    name = to;
                }
            }
            rows.retain(|r| {
                let at_head = head.records.get(&(name.clone(), r.key.clone()));
                at_head.is_some_and(|h| !h.retired && head.admitted(&now, h))
            });
        }
        // A field the lake asks an index for at the head answers the
        // index's set, and says so; the mock's set is the exact ranking.
        let by_text = matches!(
            &search.mode,
            SearchMode::Lexical { .. } | SearchMode::Hybrid { vector: None, .. }
        );
        let indexed = !by_text && {
            let later;
            let head = match pinned {
                Some(_) => {
                    later = self.state_at(&self.log(), None)?.1;
                    &later
                }
                None => &st,
            };
            head.types.get(&search.ty).is_some_and(|h| {
                let asked = index_fields(head, &h.decl, VECTOR_INDEX_SETTING, indexable);
                vectors.iter().any(|(f, ..)| asked.contains(*f))
            })
        };
        // A stored vector is read back as the `f32`s it was written as.
        let nearest = |f: &FieldName, vector: &[f32]| -> Vec<(f64, &StoredRecord)> {
            let cos = |v: &Value| {
                let v: Vec<f32> = v
                    .as_array()?
                    .iter()
                    .filter_map(Value::as_f64)
                    .map(|x| x as f32)
                    .collect();
                Some(lakelands_api::query::cosine(&v, vector))
            };
            (rows.iter().copied())
                .filter_map(|r| Some((cos(r.fields.get(f)?)?, r)))
                .collect()
        };
        let matching = |text: &str| -> Vec<(f64, &StoredRecord)> {
            let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
            (rows.iter().copied())
                .filter_map(|r| {
                    let hay: String = searched
                        .iter()
                        .filter_map(|f| r.fields.get(*f)?.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                        .to_lowercase();
                    let score = words.iter().filter(|w| hay.contains(w.as_str())).count();
                    (score > 0).then_some((score as f64, r))
                })
                .collect()
        };
        fn best_first(mut l: Vec<(f64, &StoredRecord)>) -> Vec<(f64, &StoredRecord)> {
            l.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.key.cmp(&b.1.key)));
            l
        }
        // The rankings a search fuses, each with its weight, in the
        // order they are summed: a hybrid search's text, then each vector
        // field (046).
        let mut lists = Vec::new();
        // As the store: each axis that ranked, counted where it is fused.
        let axis = |field: Option<&FieldName>| Axis {
            axis: (if field.is_some() { "vector" } else { "lexical" }).into(),
            field: field.cloned(),
            candidates: None,
            weight: None,
        };
        // As the store: a ranking the search does not weigh takes its
        // type's `search_weights` setting at the read's position.
        let key = RecordKey::from(format!("{SEARCH_WEIGHTS_SETTING}{}", search.ty));
        let set = st.live(&TypeName::from(SETTING_TYPE), &key);
        let (field_weights, text_default) = search_weights(set.and_then(|r| r.fields.get("value")));
        let mut axes = Vec::new();
        if let (
            SearchMode::Hybrid {
                text, text_weight, ..
            },
            false,
        ) = (&search.mode, by_text)
        {
            lists.push((text_weight.or(text_default).unwrap_or(1.0), matching(text)));
            axes.push(axis(None));
        }
        for (f, vector, weight) in vectors.iter().filter(|_| !by_text) {
            let set = field_weights.get(f.as_str()).copied();
            lists.push((weight.or(set).unwrap_or(1.0), nearest(f, vector)));
            axes.push(axis(Some(f)));
        }
        if axes.is_empty() {
            axes.push(axis(None));
        }
        let (scored, ranked_by) = match &search.mode {
            SearchMode::Lexical { text, .. } | SearchMode::Hybrid { text, .. } if by_text => {
                (matching(text), "lexical")
            }
            // One list keeps its own score.
            SearchMode::Vector { also, .. } if also.is_empty() => {
                let by = if indexed { "index" } else { "vector" };
                (lists.remove(0).1, by)
            }
            mode => {
                // Fused by weighted reciprocal rank, each list over what
                // the read may see and to `max_limit` (046).
                let window = self.shared.limits.max_limit as usize;
                let mut fused: BTreeMap<&RecordKey, (f64, &StoredRecord)> = BTreeMap::new();
                for (a, (w, list)) in axes.iter_mut().zip(&lists) {
                    (a.candidates, a.weight) = (Some(list.len().min(window) as u64), Some(*w));
                }
                for (weight, list) in lists {
                    for (rank, (_, r)) in best_first(list).into_iter().take(window).enumerate() {
                        fused.entry(&r.key).or_insert((0.0, r)).0 += weight / (60.0 + rank as f64);
                    }
                }
                let by = match (mode, indexed) {
                    (SearchMode::Vector { .. }, true) => "index",
                    (SearchMode::Vector { .. }, false) => "vector",
                    (_, true) => "hybrid (index)",
                    (_, false) => "hybrid",
                };
                (fused.into_values().collect(), by)
            }
        };
        let hits: Vec<Hit> = best_first(scored)
            .into_iter()
            .map(|(score, r)| Hit {
                record: st.read(who, r, search.with_meta),
                score,
            })
            .collect();
        let (hits, next_cursor) = page(
            hits,
            (position, "search", Some(&search.ty), &of),
            offset,
            search.k.max(1) as usize,
        );
        Ok(Reply {
            position,
            data: Hits {
                hits,
                next_cursor,
                ranked_by: ranked_by.to_owned(),
                axes,
            },
        })
    }

    async fn propose(&self, who: &Principal, write: ProposedWrite, why: String) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        if let ProposedWrite::Upsert { ty, .. } | ProposedWrite::Retire { ty, .. } = &write
            && links::is_link_type(ty)
        {
            return Err(refused(Refusal::WrongDoor));
        }
        if let ProposedWrite::Declare { declarations } = &write {
            for d in declarations {
                trigger_ceiling(d, self.shared.limits.max_reactive_depth)?;
            }
        }
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        // A retry of a proposal is answered the proposal it made.
        let opts = match &write {
            ProposedWrite::Upsert { opts, .. }
            | ProposedWrite::Retire { opts, .. }
            | ProposedWrite::Call { opts, .. } => opts.clone(),
            _ => WriteOpts::default(),
        };
        if let Some(o) = remembered(&st, who, &opts) {
            return Ok(o);
        }
        let pinned = pinned(&st, who, &write)?;
        let key = opts.idempotency_key;
        let mut batch = Batch::at(&log);
        let write = (write, pinned);
        let (position, proposal) = plan_propose(&st, &mut batch, who, write, why.clone(), key)?;
        log.proposers.insert(proposal.clone(), who.clone());
        self.commit(&mut log, batch);
        Ok(Outcome::Proposed {
            position,
            proposal,
            why,
        })
    }

    async fn accept(&self, who: &Principal, proposal: &ProposalId) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        let p = decidable(&st, who, proposal)?;
        let write = p.write.clone().expect("a held proposal holds its write");
        if write.is_schema() && !who.grants.schema {
            return Err(refused(Refusal::NotGranted));
        }
        let (_, then) = self.state_at(&log, Some(p.position))?;
        let logged = log.events.get(p.position.0 as usize);
        unmoved(&st, &then, logged, p.position)?;
        let proposer = log.proposers.get(proposal).cloned().ok_or_else(|| {
            let what = Unmocked::Proposer;
            invalid(Case::MockCannot { what })
        })?;
        let mut batch = Batch::at(&log);
        let accepted = batch.push(Draft::new(
            who,
            Origin::Authored,
            EventBody::WriteAccepted {
                proposal: proposal.clone(),
            },
        ));
        // The write is applied under the proposer's grants, caused by
        // the acceptance.
        let by = By {
            st: &st,
            who: &proposer,
            cause: Some(accepted),
        };
        let mut renamed = Vec::new();
        let version = match write {
            ProposedWrite::Upsert {
                ty,
                key,
                patch,
                opts,
            } => {
                let version = plan_upsert(&by, &mut batch, &ty, &key, patch, &opts)?;
                reviewed(&st, who, &batch, &ty, &key)?;
                Some(version)
            }
            ProposedWrite::Retire { ty, key, opts } => {
                Some(plan_retire(&by, &mut batch, &ty, &key, &opts)?)
            }
            ProposedWrite::Call {
                verb,
                key,
                payload,
                opts,
            } => {
                // The scope is checked again at the head: the verb may have
                // been declared again while the proposal waited. Declared
                // differently since, it is not what was checked (013).
                if let Some(now) = st.verbs.get(&verb) {
                    scoped(now, key.as_ref(), &opts)?;
                    if then.verbs.get(&verb) != Some(now) {
                        let verb = verb.clone();
                        return Err(invalid(Case::VerbRedeclared { verb }));
                    }
                }
                let v = check_call(&st, &proposer, &verb, key.as_ref(), &payload, &opts)?.clone();
                let key = record_key(&v, key.as_ref())?;
                check_version(&st, &v.on, key, &opts)?;
                let version = plan_run(&by, &mut batch, &v, key, payload, &opts)?;
                reviewed(&st, who, &batch, &v.on, key)?;
                Some(version)
            }
            // Checked again on the head; a refusal leaves the proposal
            // pending, since nothing is committed.
            ProposedWrite::Declare { declarations } => {
                check_set(&st, &proposer, &declarations)?;
                for d in &declarations {
                    trigger_ceiling(d, self.shared.limits.max_reactive_depth)?;
                }
                for mut d in set_drafts(&st, &proposer, declarations) {
                    d.cause = Some(accepted);
                    batch.push(d);
                }
                None
            }
            ProposedWrite::Migrate {
                migration,
                report: shown,
            } => {
                // The shape is the type's whole: on a type changed since
                // the proposal it would remove what was added since.
                let ty = &migration.to.name;
                let on = st.types.get(ty).map(|t| &t.schema_hash);
                let changed = || invalid(Case::ChangedSinceProposed { ty: ty.clone() });
                if on != Some(&shown.from_schema_hash) {
                    return Err(changed());
                }
                let head = Position(accepted.0.saturating_sub(1));
                let report = preflight(&st, head, (&migration, None), who)?;
                // As the store: nor does it take away a mark set since.
                if report.restated != shown.restated {
                    return Err(changed());
                }
                if !report.lossless {
                    return Err(lossy(&st, who, &migration, head, report));
                }
                let mut known = (*migration).clone();
                known_migration(&st, who, &mut known, None);
                renamed = known.renames;
                let version = st
                    .types
                    .get(&migration.to.name)
                    .map_or(1, |t| t.version + 1);
                let same = unchanged(&st, &migration);
                let mut d = Draft::new(
                    &proposer,
                    Origin::System,
                    EventBody::Migrated {
                        schema_hash: report.to_schema_hash.clone(),
                        migration: *migration,
                        version,
                        report_id: report.report_id,
                        rekeyed: Vec::new(),
                    },
                );
                d.cause = Some(accepted);
                if !same {
                    batch.push(d);
                }
                None
            }
            ProposedWrite::MigrateValueType {
                to,
                renames,
                report: shown,
            } => {
                // As a migration: accepted only on the declaration it was
                // proposed on, and only while every stored value fits.
                let declaration = Declaration::ValueType((*to).clone());
                let (_, version) = identity_version(&st, &declaration);
                let name = to.name.clone();
                let changed = || {
                    let value_type = name.clone();
                    invalid(Case::ValueTypeChangedSinceProposed { value_type })
                };
                if version != shown.from_version + 1 {
                    return Err(changed());
                }
                let head = Position(accepted.0.saturating_sub(1));
                let m = ValueTypeMigration { to: *to, renames };
                let report = tightening(&st, &proposer, head, &m)?;
                // As the store: renames reach only the fields the report
                // named at the proposal.
                let uses = |r: &lakelands_api::write::ValueTypeReport| {
                    (r.fields.iter())
                        .map(|u| (u.ty.clone(), u.field.clone()))
                        .collect::<Vec<_>>()
                };
                if !m.renames.is_empty() && uses(&report) != uses(&shown) {
                    return Err(changed());
                }
                let id = report.report_id;
                for d in value_type_drafts(&st, &proposer, m, id, Some(accepted)) {
                    batch.push(d);
                }
                None
            }
        };
        let position = Position(batch.peek().0 - 1);
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position,
            version,
            output: None,
            set_off: Default::default(),
            renamed,
        })
    }

    async fn reject(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        reason: String,
    ) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        decidable(&st, who, proposal)?;
        let mut batch = Batch::at(&log);
        let position = batch.push(Draft::new(
            who,
            Origin::Authored,
            EventBody::WriteRejected {
                proposal: proposal.clone(),
                reason,
            },
        ));
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position,
            version: None,
            output: None,
            set_off: Default::default(),
            renamed: Vec::new(),
        })
    }

    async fn proposals(
        &self,
        who: &Principal,
        filter: ProposalFilter,
    ) -> Result<Reply<Vec<Proposal>>> {
        self.admit(who)?;
        let asked = who;
        let limit = self.limit(filter.limit, false)?;
        let (pinned, offset) = match &filter.cursor {
            Some(c) => {
                let (p, o) = uncursor(c, "proposals", None, "")?;
                (Some(p), o)
            }
            None => (filter.at.as_of, 0),
        };
        let (position, st) = self.state_at(&self.log(), pinned)?;
        // As the store: the reader's grants followed to the listing, its
        // references hidden by the lake there (024, extended).
        let renames = self.renames();
        let mut here = asked.clone();
        here.grants.follow(&renames, pinned);
        let who = &st.hidden(&here);
        let touches = |w: &ProposedWrite, ty: &TypeName| match w {
            ProposedWrite::Upsert { ty: t, .. } | ProposedWrite::Retire { ty: t, .. } => t == ty,
            ProposedWrite::Call { verb, .. } => st.verbs.get(verb).is_some_and(|v| &v.on == ty),
            ProposedWrite::Declare { declarations } => declarations.iter().any(|d| match d {
                Declaration::Type(t) => &t.name == ty,
                Declaration::Verb(v) => &v.on == ty,
                Declaration::Trigger(t) => &t.on == ty,
                Declaration::ObjectSet(s) => &s.of == ty,
                Declaration::ValueType(_) | Declaration::Function(_) => false,
            }),
            ProposedWrite::Migrate { migration, .. } => &migration.to.name == ty,
            ProposedWrite::MigrateValueType { .. } => false,
        };
        // A reviewer sees every proposal; anyone else sees their own, so
        // that a proposer can follow what became of it.
        let writer = who.writer();
        let mut rows: Vec<Proposal> = st
            .proposals
            .values()
            .filter(|p| who.grants.review || p.proposer == writer)
            .filter(|p| filter.state.is_none_or(|s| p.state == s))
            // A migration's report counts every owner's records: a holder
            // of `only` is listed none (051).
            .filter(|p| !holds_only(who) || !p.write.as_ref().is_some_and(migrates))
            // As the store (lakelands#762): one on a type its reader is not
            // shown is not there to it.
            .filter(|p| {
                !p.write
                    .as_ref()
                    .is_some_and(|w| hidden_by_type(&st, who, w))
            })
            .cloned()
            .collect();
        // Proposals are made in log order, so `proposed_at` and
        // `position` order them alike; newest first unless stated.
        let asc = filter
            .order_by
            .first()
            .is_some_and(|o| o.dir == Direction::Asc);
        rows.sort_by_key(|p| p.position);
        if !asc {
            rows.reverse();
        }
        // As the store: the entry as its reader is shown it, and the kind
        // and the type asked of that.
        for p in &mut rows {
            // `more` is what the trim left out, not what the completion added.
            let mut logged = p.write.take();
            logged.iter_mut().for_each(|w| completed(&st, w));
            let shown = logged.clone();
            // As the store: a proposed migration is judged with the grants
            // followed to the position it was proposed at, and a report made
            // from a type changed since by the type it was made from, where
            // those grants hide a field.
            // Its references are held as the type it was proposed against
            // shows them; a bundle's, as the bundle in force would, from the
            // grants as asked.
            let mut then = asked.clone();
            then.grants.follow(&renames, Some(p.position));
            let made = match &logged {
                Some(ProposedWrite::Migrate { migration, report }) => {
                    let ty = &migration.to.name;
                    let head = st.types.get(ty).map(|t| &t.schema_hash);
                    let stale = head != Some(&report.from_schema_hash) && hides_some(&then);
                    let at = stale.then(|| self.state_at(&self.log(), Some(p.position)).ok());
                    let at = at.flatten().map(|(_, at)| at);
                    then = at.as_ref().unwrap_or(&st).hidden(&then);
                    let made = at.and_then(|at| at.types.get(ty).cloned());
                    made.map(|t| t.decl)
                }
                Some(ProposedWrite::Declare { .. }) => {
                    then = here.clone();
                    None
                }
                _ => None,
            };
            let listing = (&then, made.as_ref(), p.position);
            p.write = shown.and_then(|mut w| known_shape(&st, who, &mut w, listing).then_some(w));
            p.more = p.write != logged;
        }
        rows.retain(|p| {
            let (kind, ty) = (filter.kind, filter.ty.as_ref());
            kind.is_none_or(|k| p.write.as_ref().is_some_and(|w| w.kind() == k))
                && ty.is_none_or(|ty| p.write.as_ref().is_some_and(|w| touches(w, ty)))
        });
        let (rows, _) = page(rows, (position, "proposals", None, ""), offset, limit);
        Ok(Reply {
            position,
            data: rows,
        })
    }

    async fn declare(&self, who: &Principal, declaration: Declaration) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        if !who.grants.schema || holds_only(who) {
            return Err(refused(Refusal::NotGranted));
        }
        let mut log = self.log();
        let position = self.declare_in(&mut log, who, declaration)?;
        Ok(Outcome::Applied {
            position,
            version: None,
            output: None,
            set_off: Default::default(),
            renamed: Vec::new(),
        })
    }

    /// `apply`, minimally: the declarations in order under one lock, as
    /// `declare` takes them, and none of them when one is refused. It does
    /// not yet resolve a reference to a declaration later in the set.
    async fn apply(&self, who: &Principal, declarations: Vec<Declaration>) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        if !who.grants.schema || holds_only(who) {
            return Err(refused(Refusal::NotGranted));
        }
        let mut log = self.log();
        let (mut position, st) = self.state_at(&log, None)?;
        let mut declarations = declarations;
        redeclared(&st, who, &mut declarations)?;
        check_set(&st, who, &declarations)?;
        for d in &declarations {
            trigger_ceiling(d, self.shared.limits.max_reactive_depth)?;
        }
        // One append at consecutive positions; the answer names the last,
        // or the head where the whole set is in force already.
        let mut batch = Batch::at(&log);
        for d in set_drafts(&st, who, declarations) {
            position = batch.push(d);
        }
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position,
            version: None,
            output: None,
            set_off: Default::default(),
            renamed: Vec::new(),
        })
    }

    /// `recompute`, as the store: the declaration in force appended again
    /// at its next version. The mock binds no backing and no embedder, so
    /// no type of its has a field computed outside the lake.
    async fn recompute(&self, who: &Principal, what: Recompute) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        if !who.grants.schema || holds_only(who) {
            return Err(refused(Refusal::NotGranted));
        }
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        let declaration = match what {
            Recompute::Function(name) => match st.functions.iter().find(|f| f.name == name) {
                Some(f) => Declaration::Function(f.clone()),
                None => {
                    let missing = lakelands_api::Missing::Function { name };
                    return Err(Error::NotFound { missing });
                }
            },
            Recompute::Type(name) => {
                st.visible(who, &name).ok_or_else(|| no_type(&name))?;
                return Err(invalid(Case::NothingComputedOutside { ty: name }));
            }
        };
        let (schema_hash, version) = identity_version(&st, &declaration);
        let mut batch = Batch::at(&log);
        let body = EventBody::Declared {
            declaration,
            schema_hash,
            version,
            with_the_set: false,
        };
        let position = batch.push(Draft::new(who, Origin::System, body));
        self.commit(&mut log, batch);
        let (version, output, set_off, renamed) = Default::default();
        Ok(Outcome::Applied {
            position,
            version,
            output,
            set_off,
            renamed,
        })
    }

    async fn add_field(&self, who: &Principal, ty: &TypeName, f: FieldDecl) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        if !who.grants.schema || holds_only(who) {
            return Err(refused(Refusal::NotGranted));
        }
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        if reserved(ty.as_str(), who) {
            return Err(refused(Refusal::Reserved));
        }
        let t = st.visible(who, ty).ok_or_else(|| no_type(ty))?;
        // As the store: the field is added to its author's view of the
        // type, and the type with it is declared as `declare` declares it.
        let mut decl = t.decl.clone();
        known_fields((&st, who), &mut decl);
        if field(&decl, &f.name).is_some() {
            let (ty, field) = (ty.clone(), f.name.clone());
            return Err(invalid_field(&f.name, Case::FieldExists { ty, field }));
        }
        decl.fields.push(f);
        let position = self.declare_in(&mut log, who, Declaration::Type(decl))?;
        Ok(Outcome::Applied {
            position,
            version: None,
            output: None,
            set_off: Default::default(),
            renamed: Vec::new(),
        })
    }

    async fn migrate(
        &self,
        who: &Principal,
        migration: Migration,
    ) -> Result<Reply<MigrationReport>> {
        self.admit(who)?;
        let who = &self.followed(who, None);
        if !who.grants.schema || holds_only(who) {
            return Err(refused(Refusal::NotGranted));
        }
        let log = self.log();
        let (position, st) = self.state_at(&log, None)?;
        let mut migration = migration;
        let author = authored(&st, who, &mut migration)?;
        let data = preflight(&st, position, (&migration, author.as_ref()), who)?;
        let data = shown(&st, who, &migration, position, data);
        Ok(Reply { position, data })
    }

    async fn migrate_value_type(
        &self,
        who: &Principal,
        m: ValueTypeMigration,
    ) -> Result<Reply<lakelands_api::write::ValueTypeReport>> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        if !who.grants.schema || holds_only(who) {
            return Err(refused(Refusal::NotGranted));
        }
        let log = self.log();
        let (head, st) = self.state_at(&log, None)?;
        check_new_value_type(&st, who, &m.to)?;
        // As the store: a use and a misfit are shown where the caller is
        // shown the type and reads the field; the totals, `fits` and the id
        // stay, and renames are not granted to one shown less.
        let mut data = crate::valuetypes::preflight(&st, head, &m)?;
        renames_shown(&st, who, &m, &data)?;
        let read = |ty: &TypeName, f: &FieldName| {
            (st.visible(who, ty)).is_some_and(|t| readable(&st, who, t, f).is_ok())
        };
        data.fields.retain(|u| read(&u.ty, &u.field));
        data.first.retain(|m| read(&m.ty, &m.field));
        Ok(Reply {
            position: head,
            data,
        })
    }

    async fn migrate_value_type_apply(
        &self,
        who: &Principal,
        m: ValueTypeMigration,
        report_id: &str,
    ) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        if !who.grants.schema || holds_only(who) {
            return Err(refused(Refusal::NotGranted));
        }
        let mut log = self.log();
        let (head, st) = self.state_at(&log, None)?;
        check_new_value_type(&st, who, &m.to)?;
        let report = crate::valuetypes::preflight(&st, head, &m)?;
        if report.report_id != report_id {
            return Err(invalid(Case::MigrationStale));
        }
        renames_shown(&st, who, &m, &report)?;
        if !report.fits {
            // As the store: the first misfits the caller is shown are named.
            let read = |ty: &TypeName, f: &FieldName| {
                (st.visible(who, ty)).is_some_and(|t| readable(&st, who, t, f).is_ok())
            };
            let first = (report.first.iter().filter(|m| read(&m.ty, &m.field)))
                .map(|m| lakelands_api::error::RecordName::FieldOf {
                    ty: m.ty.clone(),
                    key: m.key.clone(),
                    field: m.field.clone(),
                })
                .collect();
            let (value_type, misfits) = (m.to.name.clone(), report.misfits);
            return Err(invalid(Case::ValueTypeNotTightened {
                value_type,
                misfits,
                first,
            }));
        }
        let mut batch = Batch::at(&log);
        let mut drafts = value_type_drafts(&st, who, m, report.report_id, None).into_iter();
        let position = drafts.next().map(|d| batch.push(d)).unwrap_or(head);
        drafts.for_each(|d| {
            batch.push(d);
        });
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position,
            version: None,
            output: None,
            set_off: Default::default(),
            renamed: Vec::new(),
        })
    }

    async fn migrate_apply(
        &self,
        who: &Principal,
        migration: Migration,
        report_id: &str,
    ) -> Result<Outcome> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        if !who.grants.schema || holds_only(who) {
            return Err(refused(Refusal::NotGranted));
        }
        let mut log = self.log();
        let (head, st) = self.state_at(&log, None)?;
        let mut migration = migration;
        let author = authored(&st, who, &mut migration)?;
        let report = preflight(&st, head, (&migration, author.as_ref()), who)?;
        if shown(&st, who, &migration, head, report.clone()).report_id != report_id {
            return Err(invalid(Case::MigrationStale));
        }
        if !report.lossless {
            return Err(lossy(&st, who, &migration, head, report));
        }
        // As the store: a migration to the type in force appends nothing.
        if unchanged(&st, &migration) {
            let (version, output, set_off, renamed) = Default::default();
            return Ok(Outcome::Applied {
                position: head,
                version,
                output,
                set_off,
                renamed,
            });
        }
        let version = st
            .types
            .get(&migration.to.name)
            .map_or(1, |t| t.version + 1);
        let mut batch = Batch::at(&log);
        let link_types = links::of(&migration.to);
        let carry = carried(&st, &migration);
        let report_id = report.report_id;
        // The answer names each rename as the one answered may know it.
        let mut known = migration.clone();
        known_migration(&st, who, &mut known, None);
        let position = batch.push(Draft::new(
            who,
            Origin::System,
            EventBody::Migrated {
                schema_hash: report.to_schema_hash.clone(),
                migration,
                version,
                report_id: report_id.clone(),
                rekeyed: Vec::new(),
            },
        ));
        // As the store: each link type in force whose derivation changes
        // migrates with the type, then a relation new with it gets its link
        // type declared, in the same append.
        for lt in &link_types {
            if let Some(old) = st.types.get(&lt.name).filter(|old| &old.decl != lt) {
                let migration = Migration {
                    to: lt.clone(),
                    renames: Vec::new(),
                    relation_renames: Vec::new(),
                };
                batch.push(Draft::new(
                    who,
                    Origin::System,
                    EventBody::Migrated {
                        schema_hash: lakelands_api::shape::schema_hash(lt).unwrap_or_default(),
                        migration,
                        version: old.version + 1,
                        report_id: report_id.clone(),
                        rekeyed: Vec::new(),
                    },
                ));
            }
        }
        for lt in link_types
            .into_iter()
            .filter(|lt| !st.types.contains_key(&lt.name))
        {
            let declaration = Declaration::Type(lt);
            let (schema_hash, version) = identity_version(&st, &declaration);
            let body = EventBody::Declared {
                declaration,
                schema_hash,
                version,
                with_the_set: false,
            };
            batch.push(Draft::new(who, Origin::System, body));
        }
        // As the store: what names a link type whose records move is
        // declared again under the new name.
        for declaration in carry {
            let (schema_hash, version) = identity_version(&st, &declaration);
            let body = EventBody::Declared {
                declaration,
                schema_hash,
                version,
                with_the_set: false,
            };
            batch.push(Draft::new(who, Origin::System, body));
        }
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position,
            version: None,
            output: None,
            set_off: Default::default(),
            renamed: known.renames,
        })
    }

    async fn ingest(
        &self,
        who: &Principal,
        source: &SourceId,
        mut records: SourceStream,
    ) -> Result<CommitStream> {
        self.admit_write(who)?;
        let who = &self.followed(who, None);
        if !who.grants.ingest {
            return Err(refused(Refusal::NotGranted));
        }
        let records = stream::collect(&mut records).await;
        let window = self.shared.limits.commit_window_events.max(1) as usize;
        let mut acks = Vec::new();
        for chunk in records.chunks(window) {
            let mut log = self.log();
            let (head, st) = self.state_at(&log, None)?;
            match plan_window(&st, &log, who, source, chunk) {
                Ok(w) => {
                    let position = if w.batch.drafts.is_empty() {
                        head
                    } else {
                        Position(w.batch.peek().0 - 1)
                    };
                    self.commit(&mut log, w.batch);
                    acks.push(Ok(Committed {
                        position,
                        records: w.records,
                        duplicates: w.duplicates,
                    }));
                }
                Err(e) => {
                    acks.push(Err(e));
                    break;
                }
            }
        }
        Ok(stream::of(acks))
    }
}

fn plan_window(
    st: &State,
    log: &Log,
    who: &Principal,
    source: &SourceId,
    chunk: &[SourceRecord],
) -> Result<Window> {
    let mut w = Window {
        batch: Batch::at(log),
        planned: BTreeMap::new(),
        versions: BTreeMap::new(),
        ids: BTreeSet::new(),
        records: 0,
        duplicates: 0,
    };
    for r in chunk {
        st.visible(who, &r.ty).ok_or_else(|| no_type(&r.ty))?;
        if r.fields.is_empty() {
            return Err(invalid(Case::PatchEmpty));
        }
        let t = check_patch(st, who, &r.ty, &r.fields, Origin::Source)?;
        let checked = canonical_patch(t, r.fields.clone());
        let slot = (r.ty.clone(), r.key.clone());
        let before = w
            .versions
            .get(&slot)
            .copied()
            .unwrap_or_else(|| st.version(&r.ty, &r.key));
        let id = content_id(source, r, before, &checked);
        // A record that would change no field is a duplicate too, so a full
        // re-import of unchanged data appends nothing.
        // As the store (051): a record a holder is not admitted is, to it,
        // a key that is free: not compared, and a live one refused once
        // every check has passed.
        let planned = (w.planned.get(&slot)).map(|f| !st.admits_fields(who, &r.ty, f));
        let not_own = |s: &StoredRecord| !st.admitted(who, s);
        let other = planned.unwrap_or_else(|| st.live(&r.ty, &r.key).is_some_and(not_own));
        let unowned = planned.unwrap_or_else(|| st.records.get(&slot).is_some_and(not_own));
        let now = w
            .planned
            .get(&slot)
            .cloned()
            .or_else(|| st.live(&r.ty, &r.key).map(|l| l.fields.clone()))
            .filter(|_| !other);
        let unchanged = now.as_ref().is_some_and(|fields| {
            checked.iter().all(|(f, v)| match fields.get(f) {
                Some(stored) => stored == v,
                None => v.is_null(),
            })
        });
        if unchanged || !unowned && (st.ingested.contains(&id) || !w.ids.insert(id.clone())) {
            w.duplicates += 1;
            continue;
        }
        let mut fields = checked;
        with_key_fields(
            t,
            st,
            who,
            st.live(&r.ty, &r.key).filter(|_| !other),
            &r.key,
            &mut fields,
            Origin::Source,
        )?;
        // As the store: a link type no relation carries takes no link.
        let carried = st.types.values().any(|o| {
            let by_links =
                (o.decl.relations.iter()).filter(|x| x.via.is_none() && x.derived.is_none());
            by_links
                .into_iter()
                .any(|x| links::name(&o.decl.name, &x.name) == r.ty)
        });
        if links::is_link_type(&r.ty) && !carried {
            let ty = r.ty.clone();
            return Err(invalid(Case::UnclaimedLinkType { ty }));
        }
        // A record earlier in this window counts as there.
        check_references(st, who, t, &fields, &w.versions)?;
        check_required(t, now.as_ref(), &fields)?;
        // As the store: a record this window made is there for its next one.
        let made = now.is_none();
        let after = after_write(t, now.as_ref(), &fields);
        if other || !st.admits_fields(who, &r.ty, &after) {
            return Err(refused(Refusal::NotGranted));
        }
        // A link holds its relation's cardinality, over the lake's live
        // links and those earlier in this window, judged once the record
        // is admitted, as the store judges it.
        if links::is_link_type(&r.ty)
            && let Some(rel) = st.types.values().find_map(|o| {
                o.decl
                    .relations
                    .iter()
                    .find(|x| {
                        x.via.is_none()
                            && x.derived.is_none()
                            && links::name(&o.decl.name, &x.name) == r.ty
                    })
                    .cloned()
            })
        {
            let end = |f: &str| fields.get(f).and_then(Value::as_str).map(RecordKey::from);
            if let (Some(from), Some(to)) = (end(links::FROM), end(links::TO)) {
                link_cardinality(st, who, &r.ty, &rel, &from, &to)?;
                for ((lt, k), planned) in &w.planned {
                    if lt != &r.ty || k == &r.key {
                        continue;
                    }
                    let pe = |f: &str| planned.get(f).and_then(Value::as_str).map(str::to_owned);
                    let (per_from, per_to) = match rel.cardinality {
                        lakelands_api::ontology::Cardinality::ManyToOne => (true, false),
                        lakelands_api::ontology::Cardinality::OneToMany => (false, true),
                        lakelands_api::ontology::Cardinality::OneToOne => (true, true),
                        lakelands_api::ontology::Cardinality::ManyToMany => (false, false),
                    };
                    let (f, o) = (pe(links::FROM), pe(links::TO));
                    if (per_from && f.as_deref() == Some(from.as_str()))
                        || (per_to && o.as_deref() == Some(to.as_str()))
                    {
                        let key = |k: Option<String>| RecordKey::from(k.unwrap_or_default());
                        let (from, to) = (key(f), key(o));
                        let (relation, cardinality) = (rel.name.clone(), rel.cardinality);
                        let case = Case::WindowLinksAlready {
                            from,
                            to,
                            relation,
                            cardinality,
                        };
                        return Err(constraint(&FieldName::from(links::TO), case));
                    }
                }
            }
        }
        w.planned.insert(slot.clone(), after);
        let version = Version(before.0 + 1);
        w.versions.insert(slot, version);
        let mut d = Draft::new(
            who,
            Origin::Source,
            EventBody::RecordIngested {
                ty: r.ty.clone(),
                key: r.key.clone(),
                version,
                patch: fields,
                made,
            },
        );
        d.source = Some(source.clone());
        d.seen_at = r.seen_at;
        d.upstream = r.upstream.clone();
        d.id = Some(id);
        w.batch.push(d);
        w.records += 1;
    }
    Ok(w)
}

/// The outcome an idempotency key already answered for this principal.
fn remembered(st: &State, who: &Principal, opts: &WriteOpts) -> Option<Outcome> {
    let key = opts.idempotency_key.as_ref()?;
    st.idempotent.get(&(who.writer(), key.clone())).cloned()
}

/// As the store (051): whether `who`'s proposal of `write` is pinned, a
/// write to a record of a type it holds `only` on; a change of shape and a
/// write that anything but itself could move off its pairs are refused. A
/// type it is not shown is answered as not there.
fn pinned(st: &State, who: &Principal, write: &ProposedWrite) -> Result<bool> {
    if !holds_only(who) {
        return Ok(false);
    }
    let (ty, verb) = match write {
        ProposedWrite::Upsert { ty, .. } | ProposedWrite::Retire { ty, .. } => (ty, None),
        ProposedWrite::Call { verb, .. } => {
            let shown = (st.verbs.get(verb)).filter(|v| st.visible(who, &v.on).is_some());
            let missing = || Error::NotFound {
                missing: Missing::Verb { name: verb.clone() },
            };
            let v = shown.ok_or_else(missing)?;
            (&v.on, Some(v))
        }
        _ => return Err(refused(Refusal::NotGranted)),
    };
    let t = st.visible(who, ty).ok_or_else(|| no_type(ty))?;
    let pinned = only_holder(who, &t.decl);
    if pinned && !pinnable(who, &t.decl, verb) {
        return Err(refused(Refusal::NotGranted));
    }
    Ok(pinned)
}

/// As the store (051): whether a reviewer decides the proposal logged at
/// `at`, writing `write`: one holding `only` decides no change of shape,
/// and no write to a type it holds `only` on that its listing would not
/// show it whole.
fn decides(st: &State, who: &Principal, write: &ProposedWrite, at: Position) -> bool {
    if !holds_only(who) {
        return true;
    }
    let ty = match write {
        ProposedWrite::Upsert { ty, .. } | ProposedWrite::Retire { ty, .. } => ty,
        ProposedWrite::Call { verb, .. } => match st.verbs.get(verb) {
            Some(v) => &v.on,
            None => return false,
        },
        _ => return false,
    };
    if st.types.get(ty).is_some_and(|t| !only_holder(who, &t.decl)) {
        return true;
    }
    let mut shown = write.clone();
    known_shape(st, who, &mut shown, (who, None, at)) && shown == *write
}

/// As the store (051): the accept of a pinned proposal, `logged`, made at
/// `at`, is refused once the record it writes or its type changed after
/// it; `then` is the lake at `at`.
fn unmoved(st: &State, then: &State, logged: Option<&Event>, at: Position) -> Result<()> {
    let Some(EventBody::WriteProposed {
        write,
        pinned: true,
        ..
    }) = logged.map(|e| &e.body)
    else {
        return Ok(());
    };
    let (ty, key) = match write {
        ProposedWrite::Upsert { ty, key, .. } | ProposedWrite::Retire { ty, key, .. } => {
            (ty, Some(key))
        }
        ProposedWrite::Call { verb, key, .. } => match st.verbs.get(verb) {
            Some(v) => (&v.on, key.as_ref()),
            None => return Ok(()),
        },
        _ => return Ok(()),
    };
    let version = |s: &State| s.types.get(ty).map(|t| t.version);
    let moved_at = |ty: &TypeName, k: &RecordKey| {
        let stored = st.records.get(&(ty.clone(), k.clone()));
        stored.is_some_and(|r| r.position > at)
    };
    let mut moved = key.is_some_and(|k| moved_at(ty, k));
    // As the store: a link is its holder's by its end records too, and a
    // change to an end moves no link.
    let ends = st.types.values().find_map(|o| {
        let carried = (o.decl.relations.iter()).find(|x| {
            x.via.is_none() && x.derived.is_none() && links::name(&o.decl.name, &x.name) == *ty
        });
        carried.map(|x| [o.decl.name.clone(), x.target.clone()])
    });
    if let (Some(key), Some(ends)) = (key, ends) {
        let parts = lakelands_api::key::key_components(key);
        for (end, k) in ends.iter().zip(&parts) {
            moved = moved || moved_at(end, &RecordKey::from(k.as_str()));
        }
    }
    if !moved && version(st) == version(then) {
        return Ok(());
    }
    let (ty, key) = (ty.clone(), key.cloned());
    Err(invalid(Case::WriteChangedSinceProposed { ty, key }))
}

/// As the store (051): what an accepted write leaves of the record `key`
/// of `ty` is judged by the reviewer's pairs too.
fn reviewed(
    st: &State,
    who: &Principal,
    batch: &Batch,
    ty: &TypeName,
    key: &RecordKey,
) -> Result<()> {
    let Some(t) = st.types.get(ty) else {
        return Ok(());
    };
    for d in &batch.drafts {
        if let EventBody::RecordUpserted {
            ty: on,
            key: at,
            patch,
            ..
        } = &d.body
            && (on, at) == (ty, key)
        {
            let after = after_write(t, st.live(ty, key).map(|r| &r.fields), patch);
            if !st.admits_fields(who, ty, &after) {
                return Err(refused(Refusal::NotGranted));
            }
        }
    }
    Ok(())
}

/// Whether a proposed write is a migration, of a type or a value type.
fn migrates(w: &ProposedWrite) -> bool {
    matches!(
        w,
        ProposedWrite::Migrate { .. } | ProposedWrite::MigrateValueType { .. }
    )
}

/// As the store (lakelands#762): whether a proposed write names a type
/// `who` is not shown, one the lake declares: the type it writes or a
/// called verb is on; of a migration's `to` and of each type a bundle
/// declares, what [`TypeDecl::names_against`] names against the type in force,
/// the shape completed as the mock keeps it; and the type each verb,
/// trigger and object set of a bundle is on.
fn hidden_by_type(st: &State, who: &Principal, write: &ProposedWrite) -> bool {
    let shape = |t: &TypeDecl| {
        let mut t = t.clone();
        st.complete(&mut t);
        let held = st.types.get(&t.name).map(|d| &d.decl);
        t.names_against(held)
    };
    let mut named: Vec<TypeName> = Vec::new();
    match write {
        ProposedWrite::Upsert { ty, .. } | ProposedWrite::Retire { ty, .. } => {
            named.push(ty.clone())
        }
        ProposedWrite::Call { verb, .. } => named.extend(st.verbs.get(verb).map(|v| v.on.clone())),
        ProposedWrite::Migrate { migration, .. } => named.extend(shape(&migration.to)),
        ProposedWrite::Declare { declarations } => {
            for d in declarations {
                match d {
                    Declaration::Type(t) => named.extend(shape(t)),
                    Declaration::Verb(v) => named.push(v.on.clone()),
                    Declaration::Trigger(t) => named.push(t.on.clone()),
                    Declaration::ObjectSet(s) => named.push(s.of.clone()),
                    Declaration::ValueType(_) | Declaration::Function(_) => {}
                }
            }
        }
        ProposedWrite::MigrateValueType { .. } => {}
    }
    named
        .iter()
        .any(|ty| st.types.contains_key(ty) && st.visible(who, ty).is_none())
}

/// A pending proposal a reviewer may decide.
fn decidable<'a>(st: &'a State, who: &Principal, id: &ProposalId) -> Result<&'a Proposal> {
    if !who.grants.review {
        return Err(refused(Refusal::NotGranted));
    }
    let absent = || Error::NotFound {
        missing: Missing::Proposal { id: id.clone() },
    };
    let p = st.proposals.get(id).ok_or_else(absent)?;
    // As the store (lakelands#762): a proposal on a type its reviewer is
    // not shown is not there.
    if p.write.as_ref().is_some_and(|w| hidden_by_type(st, who, w)) {
        return Err(absent());
    }
    // Before it is told decided: a holder of `only` decides only what its
    // listing shows it whole (051).
    if p.write
        .as_ref()
        .is_some_and(|w| !decides(st, who, w, p.position))
    {
        return Err(refused(Refusal::NotGranted));
    }
    if p.state != ProposalState::Pending {
        return Err(refused(Refusal::Decided));
    }
    Ok(p)
}

/// Whether `owner` and `target` match over a derived relation's pairs:
/// each holds the same non-null value (044).
fn derived_match(
    pairs: &[lakelands_api::ontology::MatchPair],
    owner: &StoredRecord,
    target: &StoredRecord,
) -> bool {
    pairs.iter().all(|p| {
        owner
            .fields
            .get(&p.from)
            .is_some_and(|v| !v.is_null() && target.fields.get(&p.to) == Some(v))
    })
}

/// A derived relation's match, as the store checks it: no carrier of its
/// own, one or more pairs, each of two fields of the same keyable kind
/// (044).
fn derived_pairs(
    st: &State,
    t: &TypeDecl,
    r: &lakelands_api::ontology::RelationDecl,
    d: &lakelands_api::ontology::Derived,
) -> Result<()> {
    let name = r.name.clone();
    let on = [of_type(&t.name), Place::Relation { name }];
    let wrong = |c: Case| Err(invalid(at(&on, c)));
    if r.via.is_some() || !r.fields.is_empty() || r.authority != Authority::Authored {
        return wrong(Case::DerivedStoresEdge);
    }
    if d.pairs.is_empty() {
        return wrong(Case::DerivedNoPair);
    }
    let target = if r.target == t.name {
        Some(t)
    } else {
        st.types.get(&r.target).map(|o| &o.decl)
    };
    for p in &d.pairs {
        let (from, to) = (p.from.clone(), p.to.clone());
        if d.pairs.iter().filter(|q| *q == p).count() > 1 {
            return wrong(Case::PairTwice { from, to });
        }
        let left = t.fields.iter().find(|f| f.name == p.from);
        let right = target.and_then(|u| u.fields.iter().find(|f| f.name == p.to));
        let (Some(left), Some(right)) = (left, right) else {
            let (ty, target) = (t.name.clone(), Box::new(r.target.clone()));
            return wrong(Case::PairUnknown {
                from,
                to,
                ty,
                target,
            });
        };
        let kind = left.ty.kind;
        if kind != right.ty.kind {
            let right = right.ty.kind;
            return wrong(Case::PairKinds {
                from,
                to,
                left: kind,
                right,
            });
        }
        if !lakelands_api::key::keyable(kind) {
            return wrong(Case::PairUnkeyable { from, to, kind });
        }
        if right.at_read() {
            let target = r.target.clone();
            return wrong(Case::PairAtRead { from, to, target });
        }
    }
    Ok(())
}

/// As the store: a field a trigger, a verb, an object set or another
/// type's derived relation in force reads as stored is not taken to the
/// read; the refusal names the field alone.
fn read_as_stored(st: &State, t: &TypeDecl) -> Result<()> {
    if !t.fields.iter().any(FieldDecl::at_read) {
        return Ok(());
    }
    let verbs = st
        .verbs
        .values()
        .filter(|v| v.on == t.name)
        .map(|v| Declaration::Verb(v.clone()));
    let triggers = (st.triggers.iter())
        .filter(|x| x.on == t.name)
        .map(|x| Declaration::Trigger(x.clone()));
    let sets = (st.object_sets.iter())
        .filter(|x| x.of == t.name)
        .map(|x| Declaration::ObjectSet(x.clone()));
    let mut read = BTreeSet::new();
    for d in verbs.chain(triggers).chain(sets) {
        read.extend(declaration_reads(st, &d, &t.name));
    }
    let others = st.types.values().filter(|e| e.decl.name != t.name);
    let matching = others
        .flat_map(|e| &e.decl.relations)
        .filter(|r| r.target == t.name);
    read.extend(
        matching
            .flat_map(|r| r.derived.iter().flat_map(|d| &d.pairs))
            .map(|p| p.to.clone()),
    );
    match t
        .fields
        .iter()
        .find(|f| f.at_read() && read.contains(&f.name))
    {
        Some(f) => {
            let on = [of_type(&t.name), of_field(&f.name)];
            Err(invalid_field(&f.name, at(&on, Case::ReadAsStored)))
        }
        None => Ok(()),
    }
}

/// As the store: an order by a field computed at the read is refused, once
/// the reader is known to read it.
fn steady_order(decl: &TypeDecl, field: &str) -> Result<()> {
    if decl
        .fields
        .iter()
        .any(|f| f.name.as_str() == field && f.at_read())
    {
        let (field, ty) = (field.into(), decl.name.clone());
        return Err(invalid(Case::OrderAtRead { field, ty }));
    }
    Ok(())
}

/// A trigger's `max_depth` lies within the lake's ceiling (021).
fn trigger_ceiling(d: &Declaration, ceiling: u32) -> Result<()> {
    match d {
        Declaration::Trigger(t) if t.max_depth.is_some_and(|m| m > ceiling) => {
            let (depth, max) = (t.max_depth.unwrap_or_default(), ceiling);
            let said = Said::from(Case::TriggerTooDeep { depth, max });
            let name = t.name.clone();
            Err(invalid(said.within(Place::Trigger { name })))
        }
        _ => Ok(()),
    }
}

//! The mock's [`Catalog`]: every door, over the log.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use lakelands_api::describe::{
    FieldDescription, LakeDescription, LakeRights, TypeAccess, TypeDescription, TypeLifecycle,
    TypeLineage, TypeRights, VerbDescription,
};
use lakelands_api::error::{Error, Missing, Refusal, Result};
use lakelands_api::event::{EventBody, EventId};
use lakelands_api::expr::{Condition, Expr};
use lakelands_api::id::{
    Cursor, FieldName, LakeId, Position, ProposalId, RecordKey, RelationName, SourceId, TypeName,
    VerbName, Version,
};
use lakelands_api::ontology::{
    Authority, FieldDecl, Risk, TypeDecl, VerbDecl, VerbEffect, VerbScope, ViewDecl,
};
use lakelands_api::principal::{Access, Principal, PrincipalKind, WriteContext};
use lakelands_api::query::{Hit, Hits, Pattern, Projection, QueryResult, Search, SearchMode, Walk};
use lakelands_api::read::{After, Direction, Find, Found, Get, Order, OrderBy, ReadAt, Reply};
use lakelands_api::record::{Origin, Patch, Record, SourceRecord, StoredRecord, Value};
use lakelands_api::seam::{
    SETTING_TYPE, SORT_INDEX_SETTING, VECTOR_INDEX_SETTING, indexable, sortable,
};
use lakelands_api::write::{
    Committed, Declaration, FieldConversion, FunctionDecl, Migration, MigrationReport, Outcome,
    Proposal, ProposalFilter, ProposalState, ProposedWrite, WriteOpts,
};
use lakelands_api::{Catalog, CommitStream, SourceStream};

use crate::expr::{self, Scope};

mod measures;
use crate::links;
use crate::state::{DeclaredType, State, access, door_writes, field};
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

/// The names a read binds against the view parameters `decl` declares:
/// none unknown and none missing, as the store refuses them. The mock
/// computes no view, so it checks no value's kind.
fn bound(decl: &TypeDecl, given: &lakelands_api::Params) -> Result<()> {
    let declared = decl.view.as_ref().map_or(&[][..], |v| &v.params[..]);
    if let Some(k) = given
        .keys()
        .find(|k| !declared.iter().any(|p| &p.name == *k))
    {
        return Err(invalid(format!(
            "`{}` declares no parameter `{k}`",
            decl.name
        )));
    }
    match declared.iter().find(|p| !given.contains_key(&p.name)) {
        Some(p) => Err(invalid(format!(
            "view `{}` needs parameter `{}`",
            decl.name, p.name
        ))),
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
    match (v.scope, key) {
        (VerbScope::Type, Some(_)) => Err(invalid(format!(
            "verb `{}` is called on the type `{}`, not on a record: it takes no key",
            v.name, v.on
        ))),
        (VerbScope::Type, None) if opts.expected_version.is_some() => Err(invalid(format!(
            "verb `{}` is called on the type `{}`: there is no record version to compare",
            v.name, v.on
        ))),
        (VerbScope::Record, None) => Err(invalid(format!(
            "verb `{}` is called on a `{}` record: it needs that record's key",
            v.name, v.on
        ))),
        (_, key) => Ok(key),
    }
}

/// The record key of a call that runs a write, which only a verb on a
/// record has: its scope was checked when it was called.
fn record_key<'k>(v: &VerbDecl, key: Option<&'k RecordKey>) -> Result<&'k RecordKey> {
    scoped(v, key, &WriteOpts::default())?.ok_or_else(|| {
        invalid(format!(
            "verb `{}` is called on the type `{}`; only a verb on a record writes",
            v.name, v.on
        ))
    })
}

fn invalid(message: impl Into<String>) -> Error {
    refused(Refusal::Invalid {
        message: message.into(),
    })
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

/// Whether a revision of a type would reshape stored records, so that it
/// needs `migrate` (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`),
/// by the store's own reasons: a field removed, retyped or made required;
/// a new field required; the key changed; a relation removed or
/// retargeted, or its cardinality narrowed; a stored type turned into a
/// view. A change of meaning, an added optional field or relation, a
/// loosened requirement or cardinality, a view ceasing to be one, is a
/// declaration; and between two views nothing is stored, so every change
/// is.
fn reshapes(
    old: &lakelands_api::ontology::TypeDecl,
    new: &lakelands_api::ontology::TypeDecl,
) -> bool {
    use lakelands_api::ontology::Cardinality;
    if old.view.is_some() && new.view.is_some() {
        return false;
    }
    if old.view.is_none() && new.view.is_some() {
        return true;
    }
    if old.key != new.key {
        return true;
    }
    let changed = old.fields.iter().any(|o| match field(new, &o.name) {
        None => true,
        Some(n) => n.ty != o.ty || (n.required && !o.required),
    });
    let required_added = new
        .fields
        .iter()
        .any(|n| n.required && field(old, &n.name).is_none());
    // Whether each side of a relation may link many: (this side, target).
    let sides = |c: Cardinality| match c {
        Cardinality::OneToOne => (false, false),
        Cardinality::OneToMany => (false, true),
        Cardinality::ManyToOne => (true, false),
        Cardinality::ManyToMany => (true, true),
    };
    let relation_reshaped =
        old.relations
            .iter()
            .any(|o| match new.relations.iter().find(|n| n.name == o.name) {
                None => true,
                Some(n) => {
                    let ((a, b), (c, d)) = (sides(o.cardinality), sides(n.cardinality));
                    n.target != o.target || !((!a || c) && (!b || d))
                }
            });
    changed || required_added || relation_reshaped
}

/// Whether declaring a type of this name is reserved: a `sys.` type is
/// declared only by the system. Records of a `sys.` type are written by
/// whoever the grants allow, like any other.
fn reserved(name: &str, who: &Principal) -> bool {
    name.starts_with("sys.") && who.kind != PrincipalKind::System
}

/// A page token: the position the first page was read at, and how many
/// rows came before this page.
fn cursor(p: Position, offset: usize) -> Cursor {
    Cursor(format!("{}.{offset}", p.0))
}

fn uncursor(c: &Cursor) -> Result<(Position, usize)> {
    c.0.split_once('.')
        .and_then(|(p, o)| Some((Position(p.parse().ok()?), o.parse().ok()?)))
        .ok_or_else(|| invalid("the cursor was not issued by this lake"))
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
        return Err(invalid(
            "`after` and `cursor` both name where a page starts; give one",
        ));
    }
    if find.aggregate.is_some() {
        return Err(invalid(
            "`after` pages records; an `aggregate` has no pages",
        ));
    }
    if a.values.len() != find.order_by.len() {
        return Err(invalid(format!(
            "`after.values` holds {} value(s) and `order_by` has {} key(s); give one value per key",
            a.values.len(),
            find.order_by.len()
        )));
    }
    let mut values = Vec::with_capacity(a.values.len());
    for (i, (v, o)) in a.values.iter().zip(&find.order_by).enumerate() {
        let unfit = |why: String| Error::Refused {
            field: Some(o.field.clone()),
            reason: Refusal::Invalid {
                message: format!("`after.values[{i}]` does not fit `{}`: {why}", o.field),
            },
        };
        let v = match (o.by, v) {
            (_, Value::Null) => None,
            (OrderBy::Latest, Value::Number(n)) if n.is_u64() => Some(v.clone()),
            (OrderBy::Latest, _) => {
                return Err(unfit("a position is a non-negative integer".into()));
            }
            (OrderBy::KnownAt | OrderBy::SeenAt, Value::String(s)) => Some(
                lakelands_api::Timestamp::parse(s)
                    .map_err(|e| unfit(e.to_string()))?
                    .to_string()
                    .into(),
            ),
            (OrderBy::KnownAt | OrderBy::SeenAt, _) => {
                return Err(unfit("an instant is given as text".into()));
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
fn page<T>(rows: Vec<T>, at: Position, offset: usize, limit: usize) -> (Vec<T>, Option<Cursor>) {
    let more = rows.len() > offset + limit;
    let rows: Vec<T> = rows.into_iter().skip(offset).take(limit).collect();
    (rows, more.then(|| cursor(at, offset + limit)))
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
                return Err(refused(Refusal::Invalid {
                    message: format!("the cause {cause} is past the lake's head {head}"),
                }));
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

    fn limit(&self, asked: Option<u32>) -> usize {
        let l = &self.shared.limits;
        asked.unwrap_or(l.default_limit).clamp(1, l.max_limit) as usize
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
        let (_, st) = self.state_at(log, None)?;
        check_declaration(&st, who, &declaration)?;
        trigger_ceiling(&declaration, self.shared.limits.max_reactive_depth)?;
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
    // As the store: an undeclared type is not found; a relation it does
    // not carry by link records is invalid; one the principal may not
    // write the links of is not granted.
    let t = st.types.get(ty).ok_or_else(|| no_type(ty))?;
    let r = t
        .decl
        .relations
        .iter()
        .find(|r| &r.name == relation && r.via.is_none() && r.derived.is_none())
        .cloned()
        .ok_or_else(|| {
            invalid(format!(
                "`{ty}` has no relation `{relation}` carried by link records"
            ))
        })?;
    if st.visible(who, &links::name(ty, relation)).is_none() {
        return Err(refused(Refusal::NotGranted));
    }
    Ok(r)
}

/// The relation's cardinality over live links, the link from `from` to
/// `to` among them.
fn link_cardinality(
    st: &State,
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
        // The store's words, so a transcript reads the same on both.
        let why = if per_from && f.as_deref() == Some(from.as_str()) {
            Some(format!(
                "`{from}` already links over `{}` to `{}`",
                r.name,
                o.as_deref().unwrap_or_default()
            ))
        } else if per_to && o.as_deref() == Some(to.as_str()) {
            Some(format!(
                "`{to}` is already linked over `{}` from `{}`",
                r.name,
                f.as_deref().unwrap_or_default()
            ))
        } else {
            None
        };
        if let Some(why) = why {
            return Err(on(
                &FieldName::from(links::TO),
                Refusal::Constraint {
                    message: format!(
                        "{why}, and `{}` allows one ({:?}); unlink it first",
                        r.name, r.cardinality
                    ),
                },
            ));
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
    if let Some((field, message)) = t.embedding_problem() {
        return Err(on(&field, Refusal::Invalid { message }));
    }
    if let Some((f, e)) = t.fields.iter().find_map(|f| Some((f, f.embed.as_ref()?))) {
        return Err(Error::Unbound {
            seam: format!(
                "the embedding model `{}` with {} dimensions, for `{}.{}` (none is bound)",
                e.model,
                f.ty.dimensions.unwrap_or(0),
                t.name,
                f.name
            ),
        });
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
    if let Declaration::Type(t) = d {
        embeddings(t)?;
    }
    if let Declaration::ValueType(v) = d {
        crate::valuetypes::check_declared(st, v)?;
    }
    match d {
        // The mock binds no function backing; `expr` is computed inside.
        Declaration::Function(f) if f.backing != "expr" => {
            return Err(Error::Unbound {
                seam: format!(
                    "the `{}` function backing (the host bound none of that kind)",
                    f.backing
                ),
            });
        }
        Declaration::Type(t) => {
            if reserved(t.name.as_str(), who) {
                return Err(refused(Refusal::Reserved));
            }
            // A view is hashed over its conditions' typed form, so one that
            // does not parse has no schema hash and is not declared.
            lakelands_api::shape::schema_hash(t).map_err(|e| invalid(e.to_string()))?;
            if let Some(old) = st.types.get(&t.name) {
                if reshapes(&old.decl, t) {
                    return Err(invalid(format!(
                        "this declaration reshapes the records of `{}`; change it with `migrate`",
                        t.name
                    )));
                }
            } else {
                types.push(t.name.clone());
            }
            // Its link types too, as the store: a relation dropped by a
            // migration and declared again with another target would
            // reshape the links it kept.
            for lt in links::of(t) {
                if let Some(old) = st.types.get(&lt.name)
                    && reshapes(&old.decl, &lt)
                {
                    return Err(invalid(format!(
                        "this declaration reshapes the links `{}` holds; that is a migration of `{}`",
                        lt.name, t.name
                    )));
                }
            }
            for f in &t.fields {
                if let Some(target) = &f.ty.target
                    && !known(target, Some(&t.name))
                {
                    return Err(on(
                        &f.name,
                        Refusal::Invalid {
                            message: format!(
                                "type `{}`, field `{}`: its target `{target}` is not a declared type",
                                t.name, f.name
                            ),
                        },
                    ));
                }
            }
            for r in &t.relations {
                if !known(&r.target, Some(&t.name)) {
                    return Err(invalid(format!(
                        "type `{}`, relation `{}`: `{}` is not a declared type",
                        t.name, r.name, r.target
                    )));
                }
                if r.via.is_some() && !r.fields.is_empty() {
                    return Err(invalid(format!(
                        "type `{}`, relation `{}`: a relation carried by `via` has no fields of \
                         its own",
                        t.name, r.name
                    )));
                }
                if !matches!(r.authority, Authority::Authored | Authority::Source)
                    || r.fields
                        .iter()
                        .any(|f| f.name.as_str() == links::FROM || f.name.as_str() == links::TO)
                {
                    return Err(invalid(format!(
                        "type `{}`, relation `{}`: its links are written by `authored` or \
                         `source`, and its fields do not name `from` or `to`",
                        t.name, r.name
                    )));
                }
                if let Some(before) = st
                    .types
                    .get(&t.name)
                    .and_then(|o| o.decl.relations.iter().find(|b| b.name == r.name))
                    && (before.via.is_some() != r.via.is_some()
                        || before.derived.is_some() != r.derived.is_some())
                {
                    return Err(invalid(format!(
                        "type `{}`, relation `{}`: a relation keeps its carrier",
                        t.name, r.name
                    )));
                }
                if let Some(d) = &r.derived {
                    derived_pairs(st, t, r, d)?;
                }
            }
        }
        other => {
            let target = match other {
                Declaration::Verb(v) => {
                    // As the store: a verb on its type has no record, so
                    // nothing for a write effect to set or `requires` to read.
                    if v.scope == VerbScope::Type {
                        if matches!(v.effect, VerbEffect::Write { .. }) {
                            return Err(invalid(format!(
                                "verb `{}` is declared on the type `{}`, and a write effect sets fields of a record it does not have",
                                v.name, v.on
                            )));
                        }
                        if v.requires.is_some() {
                            return Err(invalid(format!(
                                "verb `{}` is declared on the type `{}`; `requires` reads a record it does not have",
                                v.name, v.on
                            )));
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
                return Err(Error::Refused {
                    field: Some("on".into()),
                    reason: Refusal::Invalid {
                        message: format!(
                            "`{relation}` is carried by `{via}`; watch `{owner}` with `changed({via})`"
                        ),
                    },
                });
            }
            if let Some(ty) = target {
                // As the store: a declaration that names an undeclared
                // type is invalid, naming both.
                if !st.types.contains_key(ty) {
                    return Err(invalid(match other {
                        Declaration::Verb(v) => {
                            format!("verb `{}` acts on `{ty}`, which is not declared", v.name)
                        }
                        Declaration::Trigger(t) => {
                            format!("trigger `{}` watches `{ty}`, which is not declared", t.name)
                        }
                        Declaration::ObjectSet(s) => format!(
                            "object set `{}` holds `{ty}`, which is not declared",
                            s.name
                        ),
                        _ => format!("`{ty}` is not declared"),
                    }));
                }
                // As the store: a trigger may watch any declared type, the
                // link types of relations carried by link records among them.
                if reserved(ty.as_str(), who) && !matches!(other, Declaration::Trigger(_)) {
                    return Err(refused(Refusal::Reserved));
                }
            }
        }
    }
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
        return Err(invalid(format!(
            "these names cannot all become tools: {}",
            serde_json::to_string(&problems).unwrap_or_default()
        )));
    }
    Ok(())
}

// ---- describing ----

fn authored_writable(who: &Principal, t: &DeclaredType) -> bool {
    t.decl.fields.iter().any(|f| {
        matches!(f.authority, Authority::Authored | Authority::Shared)
            && access(who, &t.decl.name, f).writes()
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
    let Some(t) = st.visible(who, &v.on) else {
        return false;
    };
    match &v.effect {
        VerbEffect::Write { set } => set
            .iter()
            .all(|a| field(&t.decl, &a.field).is_some_and(|f| access(who, &v.on, f).writes())),
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

fn describe_type(st: &State, who: &Principal, t: &DeclaredType) -> TypeDescription {
    let d = &t.decl;
    let fields: Vec<FieldDescription> = d
        .fields
        .iter()
        .filter_map(|f| match access(who, &d.name, f) {
            Access::None => None,
            a => Some(FieldDescription::of(f, a)),
        })
        .collect();
    let visible = |names: Vec<FieldName>| -> Vec<FieldName> {
        names
            .into_iter()
            .filter(|f| fields.iter().any(|d| d.name == *f))
            .collect()
    };
    let sorted = visible(index_fields(st, d, SORT_INDEX_SETTING, sortable));
    let indexed = visible(index_fields(st, d, VECTOR_INDEX_SETTING, indexable));
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
            .filter(|r| st.visible(who, &r.target).is_some())
            .map(|r| {
                // A link field the principal may not read is described by
                // its shape alone, as a type's own field is.
                let link = crate::links::name(&d.name, &r.name);
                let mut r = r.clone();
                for f in &mut r.fields {
                    if !access(who, &link, f).reads() {
                        f.derived = None;
                        f.function = None;
                        f.depends_on.clear();
                        f.searchable = false;
                        f.description = None;
                        f.label = None;
                        f.embed = None;
                        f.annotations.clear();
                    }
                }
                r
            })
            .collect(),
        access: TypeAccess {
            key: d.key.clone(),
            valid_from: d.valid_from.clone(),
            valid_to: d.valid_to.clone(),
            searchable: d.fields.iter().any(|f| f.searchable),
            field_meta: d.field_meta,
        },
        may: TypeRights {
            find: true,
            get: true,
            upsert: !links::is_link_type(&d.name) && authored_writable(who, t),
            retire: !links::is_link_type(&d.name) && may_retire(who, t),
            link: d
                .relations
                .iter()
                .filter(|r| {
                    r.via.is_none()
                        && r.derived.is_none()
                        && r.authority == Authority::Authored
                        && st.visible(who, &r.target).is_some()
                })
                .filter(|r| {
                    let lt = links::link_type(&d.name, r);
                    lt.fields[..2]
                        .iter()
                        .all(|f| access(who, &lt.name, f).writes())
                })
                .map(|r| r.name.clone())
                .collect(),
        },
        verbs: st
            .verbs
            .values()
            .filter(|v| v.on == d.name && callable(st, who, v))
            .map(|v| v.name.clone())
            .collect(),
        lineage: TypeLineage {
            sourced: d
                .fields
                .iter()
                .any(|f| matches!(f.authority, Authority::Source | Authority::Shared)),
            view_of: d.view.as_ref().map(|v| v.source.clone()),
        },
        lifecycle: TypeLifecycle {
            version: t.version,
            schema_hash: t.schema_hash.clone(),
            sorted,
            indexed,
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
        .all(|n| field(&t.decl, n).is_some_and(|f| access(who, ty, f).reads()))
}

/// The fields a condition names: the first segment of each path.
fn named(c: &Condition) -> Option<BTreeSet<FieldName>> {
    let e = expr::typed(c).ok()?;
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
    who.grants.schema || c.is_none_or(|c| named(c).is_some_and(|n| reads_all(st, who, ty, &n)))
}

/// A view is shown whole on the same rule, over its source's fields.
fn view_visible(st: &State, who: &Principal, v: &ViewDecl) -> bool {
    if who.grants.schema {
        return true;
    }
    let mut names: BTreeSet<FieldName> = v.group_by.iter().cloned().collect();
    for c in v.r#where.iter().chain(v.measures.iter().map(|m| &m.expr)) {
        match named(c) {
            Some(n) => names.extend(n),
            None => return false,
        }
    }
    reads_all(st, who, &v.source, &names)
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
            reads_all(
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

/// The typed filter, with every path it reads checked against the type
/// and the principal's grants.
fn filter(
    st: &State,
    who: &Principal,
    t: &DeclaredType,
    c: Option<&lakelands_api::expr::Condition>,
) -> Result<Option<Expr>> {
    let Some(c) = c else { return Ok(None) };
    let e = expr::typed(c)?;
    let mut paths = Vec::new();
    expr::paths(&e, &mut paths);
    for p in paths {
        let name = match p.as_slice() {
            [f] => f,
            [ty, f] if ty.as_str() == t.decl.name.as_str() => f,
            _ => {
                return Err(invalid(format!(
                    "`{}` does not name a field of `{}`",
                    p.iter()
                        .map(FieldName::as_str)
                        .collect::<Vec<_>>()
                        .join("."),
                    t.decl.name
                )));
            }
        };
        readable(st, who, t, name)?;
    }
    Ok(Some(e))
}

/// Refuse a field the type does not declare or the principal may not read.
fn readable(_: &State, who: &Principal, t: &DeclaredType, name: &FieldName) -> Result<()> {
    let f = field(&t.decl, name).ok_or_else(|| {
        on(
            name,
            Refusal::Invalid {
                message: format!("`{}` has no field `{name}`", t.decl.name),
            },
        )
    })?;
    if !access(who, &t.decl.name, f).reads() {
        return Err(on(name, Refusal::NotGranted));
    }
    Ok(())
}

/// Whether a record is valid at a business instant.
fn valid_at(t: &DeclaredType, r: &StoredRecord, at: Option<lakelands_api::Timestamp>) -> bool {
    let (Some(at), Some(from)) = (at, &t.decl.valid_from) else {
        return true;
    };
    let ts = |f: &FieldName| {
        r.fields
            .get(f)
            .and_then(Value::as_str)
            .and_then(|s| lakelands_api::Timestamp::parse(s).ok())
    };
    let starts = ts(from).is_some_and(|s| s <= at);
    let ends = t
        .decl
        .valid_to
        .as_ref()
        .and_then(ts)
        .is_some_and(|e| e <= at);
    starts && !ends
}

/// The live records of a type a principal may read, valid at `at`,
/// matching a filter.
fn candidates<'a>(
    st: &'a State,
    who: &Principal,
    t: &DeclaredType,
    at: ReadAt,
    filter: Option<&Expr>,
) -> Result<Vec<&'a StoredRecord>> {
    let mut out = Vec::new();
    for r in st.records.values() {
        if r.ty != t.decl.name || r.retired || (r.restricted && !who.grants.restricted) {
            continue;
        }
        if !valid_at(t, r, at.valid_at) {
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
    if t.decl.view.is_some() {
        return Err(invalid(format!("`{ty}` is a view and is never written")));
    }
    for (name, v) in patch {
        let f = field(&t.decl, name).ok_or_else(|| {
            on(
                name,
                Refusal::Invalid {
                    message: format!("`{ty}` has no field `{name}`"),
                },
            )
        })?;
        if !door_writes(door, f.authority) {
            return Err(on(name, Refusal::WrongDoor));
        }
        if !access(who, ty, f).writes() {
            return Err(on(name, Refusal::NotGranted));
        }
        if !v.is_null()
            && let Some(message) = value::misfit(&f.ty, v, &st.value_types)
        {
            return Err(on(name, Refusal::Constraint { message }));
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
/// one written earlier in the same ingest window.
fn check_references(
    st: &State,
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
        if st.live(&slot.0, &slot.1).is_none() && !earlier.contains_key(&slot) {
            return Err(on(
                name,
                Refusal::Constraint {
                    message: format!(
                        "`{name}` names `{target}` `{key}`, which is not a live record"
                    ),
                },
            ));
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
            return Err(invalid(format!(
                "`{}` is keyed by {} field(s), and the key `{key}` has {} component(s)",
                t.decl.name,
                fields.len(),
                parts.len()
            )));
        }
        for (name, part) in fields.iter().zip(parts) {
            if patch.contains_key(name) {
                continue;
            }
            let f = field(&t.decl, name).expect("a key names a field");
            let v = match f.ty.kind {
                lakelands_api::ontology::StorageKind::Number => part
                    .parse::<serde_json::Number>()
                    .map(Value::Number)
                    .map_err(|_| invalid(format!("the key component `{part}` is not a number")))?,
                lakelands_api::ontology::StorageKind::Boolean => match part.as_str() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    _ => {
                        return Err(invalid(format!(
                            "the key component `{part}` is not a boolean"
                        )));
                    }
                },
                _ => Value::String(part),
            };
            let v = value::canonical(&f.ty, &v);
            if !door_writes(door, f.authority) {
                return Err(on(name, Refusal::WrongDoor));
            }
            if !access(who, &t.decl.name, f).writes() {
                return Err(on(name, Refusal::NotGranted));
            }
            if let Some(message) = value::misfit(&f.ty, &v, &st.value_types) {
                return Err(on(name, Refusal::Constraint { message }));
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
    match derived.map(|d| d.join(":")) {
        Some(d) if d != key.as_str() => Err(invalid(format!(
            "the key `{key}` and the key fields of `{}` (which make `{d}`) disagree",
            t.decl.name
        ))),
        _ => Ok(()),
    }
}

/// Refuse a record that would lack a required field its door writes.
fn check_required(
    t: &DeclaredType,
    before: Option<&StoredRecord>,
    patch: &Patch,
    door: Origin,
) -> Result<()> {
    for f in &t.decl.fields {
        if !f.required || !door_writes(door, f.authority) {
            continue;
        }
        let has = match patch.get(&f.name) {
            Some(v) => !v.is_null(),
            None => before.is_some_and(|r| r.fields.contains_key(&f.name)),
        };
        if !has {
            return Err(on(
                &f.name,
                Refusal::Constraint {
                    message: format!("`{}` is required", f.name),
                },
            ));
        }
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
    let rule = crate::aging::check(st, &fields)
        .map_err(|(field, message)| on(&field, Refusal::Invalid { message }))?;
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
            Some(f) if access(who, &rule.on, f).reads() => {}
            Some(_) => return Err(on(name, Refusal::NotGranted)),
            None => {
                return Err(on(
                    name,
                    Refusal::Invalid {
                        message: format!("`{}` has no field `{name}`", rule.on),
                    },
                ));
            }
        }
    }
    Ok(())
}

/// Refuse a `sort_index.<type>` or `vector_index.<type>` setting the write
/// would leave naming what that index cannot keep, naming the field at
/// fault.
fn check_index_setting(st: &State, key: &RecordKey, patch: &Patch) -> Result<()> {
    let value = match patch.get("value") {
        Some(v) => Some(v.clone()),
        None => st
            .live(&TypeName::from(SETTING_TYPE), key)
            .and_then(|r| r.fields.get("value").cloned()),
    };
    let decl = lakelands_api::seam::index_setting_type(key.as_str())
        .and_then(|t| st.types.get(&TypeName::from(t)))
        .map(|t| &t.decl);
    lakelands_api::seam::check_index_setting(key.as_str(), value.as_ref(), decl)
        .map_err(|(field, message)| on(&field, Refusal::Invalid { message }))
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
    if t.view.is_some() {
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
    let actual = st.version(ty, key);
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
    if patch.is_empty() {
        return Err(invalid("nothing to write: the patch carries no field"));
    }
    // As the store's record plan: every door that writes a rule checks it,
    // `upsert`, a verb's write and an accepted proposal alike.
    if ty.as_str() == crate::aging::RULE_TYPE {
        check_rule(st, who, key, &patch)?;
    }
    if ty.as_str() == SETTING_TYPE {
        check_index_setting(st, key, &patch)?;
    }
    let t = check_patch(st, who, ty, &patch, Origin::Authored)?;
    let actual = check_version(st, ty, key, opts)?;
    let mut patch = canonical_patch(t, patch);
    with_key_fields(
        t,
        st,
        who,
        st.live(ty, key),
        key,
        &mut patch,
        Origin::Authored,
    )?;
    // After the key fields are filled: a key field may be a reference too.
    check_references(st, t, &patch, &BTreeMap::new())?;
    check_required(t, st.live(ty, key), &patch, Origin::Authored)?;
    let version = Version(actual.0 + 1);
    let mut d = Draft::new(
        who,
        Origin::Authored,
        EventBody::RecordUpserted {
            ty: ty.clone(),
            key: key.clone(),
            version,
            patch,
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
    if !may_retire(who, t) {
        return Err(refused(Refusal::NotGranted));
    }
    st.live(ty, key).ok_or_else(|| no_record(ty, key))?;
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
    let v = st.verbs.get(verb).ok_or_else(|| Error::NotFound {
        missing: Missing::Verb { name: verb.clone() },
    })?;
    // The key against the scope comes before anything else is read.
    let key = scoped(v, key, opts)?;
    // A declared verb the caller may not call is refused, as the store
    // refuses it; only an undeclared one is not found.
    if !callable(st, who, v) {
        return Err(refused(Refusal::NotGranted));
    }
    let rec = match key {
        Some(key) => Some(st.live(&v.on, key).ok_or_else(|| no_record(&v.on, key))?),
        None => None,
    };
    for (name, val) in payload {
        let f = v.payload.iter().find(|f| &f.name == name).ok_or_else(|| {
            on(
                name,
                Refusal::Invalid {
                    message: format!("`{verb}` takes no `{name}`"),
                },
            )
        })?;
        if !val.is_null()
            && let Some(message) = value::misfit(&f.ty, val, &st.value_types)
        {
            return Err(on(name, Refusal::Constraint { message }));
        }
    }
    for f in v.payload.iter().filter(|f| f.required) {
        if payload.get(&f.name).is_none_or(Value::is_null) {
            return Err(on(
                &f.name,
                Refusal::Constraint {
                    message: format!("`{verb}` needs `{}`", f.name),
                },
            ));
        }
    }
    // A verb on its type declares no `requires`: declaring refuses it.
    if let (Some(req), Some(rec), Some(key)) = (&v.requires, rec, key) {
        let scope = Scope {
            ty: v.on.as_str(),
            fields: &rec.fields,
            payload: Some(payload),
            now: None,
        };
        if !expr::holds(&expr::typed(req)?, &scope)? {
            return Err(refused(Refusal::Precondition {
                message: format!("`{verb}` does not apply to `{key}` as it stands"),
            }));
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
            return Err(Error::Unbound {
                seam: format!("the verb executor `{executor}`"),
            });
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
            expr::eval(&expr::typed(&a.value)?, &scope)?,
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

fn plan_propose(
    st: &State,
    batch: &mut Batch,
    who: &Principal,
    write: ProposedWrite,
    why: String,
    key: Option<lakelands_api::IdempotencyKey>,
) -> Result<(Position, ProposalId)> {
    // A proposal is checked as its write would be, so that an accept
    // cannot smuggle a field its proposer may not write.
    match &write {
        ProposedWrite::Upsert { ty, key, patch, .. } => {
            check_patch(st, who, ty, patch, Origin::Authored)?;
            if ty.as_str() == crate::aging::RULE_TYPE {
                check_rule(st, who, key, patch)?;
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
            // The mock binds no executor: a call that could not run when
            // accepted is not proposed.
            if let VerbEffect::External { executor } =
                &check_call(st, who, verb, key.as_ref(), payload, opts)?.effect
            {
                return Err(Error::Unbound {
                    seam: format!("the verb executor `{executor}`"),
                });
            }
        }
        // A change of the ontology is checked as its door would check it;
        // the proposer needs no `schema` grant.
        ProposedWrite::Declare { declarations } => check_set(st, who, declarations)?,
        ProposedWrite::Migrate { .. } => {}
    }
    let write = match write {
        ProposedWrite::Migrate { migration, .. } => {
            let head = Position(batch.peek().0.saturating_sub(1));
            let report = preflight(st, head, &migration)?;
            if !report.lossless {
                return Err(refused(Refusal::Migration {
                    report: Box::new(report),
                }));
            }
            ProposedWrite::Migrate {
                migration,
                report: Box::new(report),
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
        return Err(invalid(
            "nothing to apply: the set holds no declaration".to_owned(),
        ));
    }
    let ids: Vec<String> = declarations.iter().map(identity).collect();
    let mut problems: Vec<(String, Error)> = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        if ids[..i].contains(id) {
            problems.push((
                id.clone(),
                invalid(format!("the set declares {id} more than once")),
            ));
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
            problems.push((ids[i].clone(), e));
        }
    }
    match problems.len() {
        0 => Ok(()),
        1 => Err(problems.remove(0).1),
        n => {
            let all: Vec<String> = problems
                .iter()
                .map(|(id, e)| format!("{id}: {e}"))
                .collect();
            Err(invalid(format!(
                "the set is refused whole, with {n} problems: {}",
                all.join("; ")
            )))
        }
    }
}

/// The drafts that declare a checked set, each with the link types it
/// calls for.
fn set_drafts(st: &State, who: &Principal, declarations: Vec<Declaration>) -> Vec<Draft> {
    let mut out = Vec::new();
    for declaration in declarations {
        let (schema_hash, version) = identity_version(st, &declaration);
        let links = link_drafts(st, who, &declaration);
        out.push(Draft::new(
            who,
            Origin::System,
            EventBody::Declared {
                declaration,
                schema_hash,
                version,
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

/// The migration's preflight over the lake as it stands.
fn preflight(st: &State, head: Position, m: &Migration) -> Result<MigrationReport> {
    let ty = &m.to.name;
    let t = st.types.get(ty).ok_or_else(|| no_type(ty))?;
    if t.decl.key != m.to.key {
        return Err(invalid(format!(
            "the in-memory mock does not re-key: `{ty}` keeps its key fields here"
        )));
    }
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
    let mut fields = Vec::new();
    for old in &t.decl.fields {
        let Some(new) = field(&m.to, &renamed(&old.name)) else {
            continue;
        };
        if new.ty == old.ty {
            continue;
        }
        let mut conv = FieldConversion {
            field: new.name.clone(),
            from: old.ty.clone(),
            to: new.ty.clone(),
            convertible: 0,
            not_convertible: 0,
            sample_keys: Vec::new(),
        };
        for r in &live {
            match r.fields.get(&old.name) {
                None => {}
                Some(v) if value::convert(v, &new.ty).is_some() => conv.convertible += 1,
                Some(_) => {
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
    let dropped: Vec<(FieldName, u64)> = t
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
    let mut report = MigrationReport {
        ty: ty.clone(),
        from_schema_hash: t.schema_hash.clone(),
        to_schema_hash: lakelands_api::shape::schema_hash(&m.to)
            .map_err(|e| invalid(e.to_string()))?,
        records_scanned: live.len() as u64,
        lossless: fields.iter().all(|f| f.not_convertible == 0),
        fields,
        dropped,
        rekeyed: Vec::new(),
        references: Vec::new(),
        sample_collisions: Vec::new(),
        report_id: String::new(),
    };
    let seed = serde_json::to_vec(&(&report, head.0, m)).unwrap_or_default();
    report.report_id = value::hex(&seed, 1);
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
        let (position, st) = self.state_at(&self.log(), at.as_of)?;
        let t = st.visible(who, ty).ok_or_else(|| no_type(ty))?;
        Ok(Reply {
            position,
            data: describe_type(&st, who, t),
        })
    }

    async fn find(&self, who: &Principal, ty: &TypeName, find: Find) -> Result<Reply<Found>> {
        self.admit(who)?;
        let (pinned, offset) = match &find.cursor {
            Some(c) => {
                let (p, o) = uncursor(c)?;
                (Some(p), o)
            }
            None => (find.at.as_of, 0),
        };
        let (position, st) = self.state_at(&self.log(), pinned)?;
        let t = st.visible(who, ty).ok_or_else(|| no_type(ty))?;
        bound(&t.decl, &find.params)?;
        let after = find
            .after
            .as_ref()
            .map(|a| resume_after(&st, t, &find, a))
            .transpose()?;
        if find.aggregate.is_some() {
            return Err(invalid("the in-memory mock does not aggregate"));
        }
        let e = filter(&st, who, t, find.filter.as_ref())?;
        for o in &find.order_by {
            if let Some((field, message)) = o.misfit() {
                return Err(Error::Refused {
                    field,
                    reason: Refusal::Invalid { message },
                });
            }
            if o.by == OrderBy::Latest && t.decl.view.is_some() {
                return Err(invalid(format!(
                    "`order_by` `latest` orders records by their latest change, and `{ty}` is a \
                     view, whose records are computed and have none"
                )));
            }
            if o.by != OrderBy::Latest {
                readable(&st, who, t, &o.field)?;
            }
        }
        let mut rows = candidates(&st, who, t, find.at, e.as_ref())?;
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
        let (rows, next_cursor) = page(rows, position, offset, self.limit(find.limit));
        Ok(Reply {
            position,
            data: Found::Records { rows, next_cursor },
        })
    }

    async fn get(&self, who: &Principal, ty: &TypeName, get: Get) -> Result<Reply<Record>> {
        self.admit(who)?;
        let (position, st) = self.state_at(&self.log(), get.at.as_of)?;
        let t = st.visible(who, ty).ok_or_else(|| no_type(ty))?;
        bound(&t.decl, &get.params)?;
        let r = st
            .live(ty, &get.key)
            .filter(|r| !r.restricted || who.grants.restricted)
            .filter(|r| valid_at(t, r, get.at.valid_at))
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
            return Err(on(
                &FieldName::from(f),
                Refusal::Invalid {
                    message: format!("a link's `{f}` is given as `key` and `to`"),
                },
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
        link_cardinality(&st, &lt, &r, key, to)?;
        let position = batch.peek().0 - 1;
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position: Position(position),
            version: Some(version),
            output: None,
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
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        if let Some(o) = remembered(&st, who, &opts) {
            return Ok(o);
        }
        let v = check_call(&st, who, verb, key, &payload, &opts)?.clone();
        // The mock binds no executor, so a verb that reaches outside the
        // lake is refused when called, as the store refuses one whose
        // executor its host did not bind.
        if let VerbEffect::External { executor } = &v.effect {
            return Err(Error::Unbound {
                seam: format!("the verb executor `{executor}`"),
            });
        }
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
            let (position, proposal) =
                plan_propose(&st, &mut batch, who, write, why.clone(), key_)?;
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
        let (pinned, offset) = match &pattern.cursor {
            Some(c) => {
                let (p, o) = uncursor(c)?;
                (Some(p), o)
            }
            None => (pattern.at.as_of, 0),
        };
        let (position, st) = self.state_at(&self.log(), pinned)?;
        let start = &pattern.start;
        let t = st
            .visible(who, &start.ty)
            .ok_or_else(|| no_type(&start.ty))?;
        bound(&t.decl, &start.params)?;
        if start.search.is_some() || pattern.to.is_some() {
            return Err(invalid(
                "the in-memory mock starts a pattern from a key or a filter only",
            ));
        }
        if matches!(pattern.project, Projection::Degree | Projection::Components) {
            measures::check(&pattern)?;
            for step in &pattern.steps {
                let declared = st.types.values().any(|o| {
                    o.decl.relations.iter().any(|r| {
                        r.name == step.relation
                            && (pattern.project == Projection::Components
                                || (step.walk != Walk::In && o.decl.name == t.decl.name)
                                || (step.walk != Walk::Out && r.target == t.decl.name))
                    })
                });
                if !declared {
                    return Err(invalid(format!(
                        "`{}` has no relation `{}` to walk",
                        t.decl.name, step.relation
                    )));
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
            let (rows, next_cursor) = page(rows, position, offset, self.limit(pattern.limit));
            return Ok(Reply {
                position,
                data: QueryResult::Rows { rows, next_cursor },
            });
        }
        let Projection::Rows { fields } = &pattern.project else {
            return Err(invalid("the in-memory mock projects patterns as rows only"));
        };
        let e = filter(&st, who, t, start.filter.as_ref())?;
        let mut paths: Vec<Vec<&StoredRecord>> = candidates(&st, who, t, pattern.at, e.as_ref())?
            .into_iter()
            .filter(|r| start.key.as_ref().is_none_or(|k| &r.key == k))
            .map(|r| vec![r])
            .collect();
        let mut scope_ty = t;
        for step in &pattern.steps {
            // A step over a relation the type does not declare is
            // refused, not answered empty.
            let (target, via, forward, link_type) = match step.walk {
                Walk::Out => {
                    let rel = scope_ty
                        .decl
                        .relations
                        .iter()
                        .find(|r| r.name == step.relation);
                    match rel {
                        Some(r) => (
                            r.target.clone(),
                            (r.via.clone(), r.derived.clone()),
                            true,
                            links::name(&scope_ty.decl.name, &r.name),
                        ),
                        None => {
                            return Err(invalid(format!(
                                "`{}` declares no relation `{}`",
                                scope_ty.decl.name, step.relation
                            )));
                        }
                    }
                }
                _ => {
                    let back = st.types.values().find_map(|o| {
                        o.decl
                            .relations
                            .iter()
                            .find(|r| {
                                r.target == scope_ty.decl.name
                                    && (r.inverse.as_ref() == Some(&step.relation)
                                        || r.name == step.relation)
                            })
                            .map(|r| {
                                (
                                    o.decl.name.clone(),
                                    (r.via.clone(), r.derived.clone()),
                                    links::name(&o.decl.name, &r.name),
                                )
                            })
                    });
                    match back {
                        Some((ty, via, lt)) => (ty, via, false, lt),
                        None => {
                            return Err(invalid(format!(
                                "no relation `{}` reaches `{}`",
                                step.relation, scope_ty.decl.name
                            )));
                        }
                    }
                }
            };
            if step.depth.min != 1 || step.depth.max != 1 || step.walk == Walk::Both {
                return Err(invalid(
                    "the in-memory mock walks one hop, in one direction, per step",
                ));
            }
            // A relation without `via` is walked over its live link records.
            let linked_by = |from: &RecordKey, to: &RecordKey| {
                st.live(&link_type, &lakelands_api::key::link_key(from, to))
                    .is_some()
            };
            let next = st.visible(who, &target).ok_or_else(|| no_type(&target))?;
            if let Some(ty) = &step.ty
                && ty != &target
            {
                return Err(invalid(format!(
                    "`{}` reaches `{target}`, not `{ty}`",
                    step.relation
                )));
            }
            // A derived relation probes its matched fields on both sides
            // (024, 044).
            if let (_, Some(d)) = &via {
                let (owner, target) = if forward {
                    (scope_ty, next)
                } else {
                    (next, scope_ty)
                };
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
        let (rows, next_cursor) = page(rows, position, offset, self.limit(pattern.limit));
        Ok(Reply {
            position,
            data: QueryResult::Rows { rows, next_cursor },
        })
    }

    async fn search(&self, who: &Principal, search: Search) -> Result<Reply<Hits>> {
        self.admit(who)?;
        let (pinned, offset) = match &search.cursor {
            Some(c) => {
                let (p, o) = uncursor(c)?;
                (Some(p), o)
            }
            None => (search.at.as_of, 0),
        };
        let (position, st) = self.state_at(&self.log(), pinned)?;
        let t = st
            .visible(who, &search.ty)
            .ok_or_else(|| no_type(&search.ty))?;
        if t.decl.view.is_some() {
            return Err(invalid(format!(
                "`{}` is a view, and a view is not searched",
                search.ty
            )));
        }
        // A search probes every searchable field, or the ones it names,
        // then the vector field it names and the fields that vector is
        // embedded from; one the caller may not read refuses the search
        // whole (024), in the store's order. A named field is judged
        // readable before searchable.
        let text_fields = match &search.mode {
            SearchMode::Lexical { fields, .. } | SearchMode::Hybrid { fields, .. } => {
                fields.as_deref()
            }
            SearchMode::Vector { .. } => None,
        };
        let vector_field = match &search.mode {
            SearchMode::Vector { field, .. } | SearchMode::Hybrid { field, .. } => Some(field),
            SearchMode::Lexical { .. } => None,
        };
        let embedded_from = vector_field
            .and_then(|v| field(&t.decl, v))
            .and_then(|f| f.embed.as_ref())
            .into_iter()
            .flat_map(|e| e.from.iter());
        let searched: Vec<&FieldName> = match text_fields {
            None => t
                .decl
                .fields
                .iter()
                .filter(|f| f.searchable)
                .map(|f| &f.name)
                .collect(),
            Some([]) => return Err(invalid("`fields` names no field")),
            Some(named) => {
                let named: BTreeSet<&FieldName> = named.iter().collect();
                for name in &named {
                    readable(&st, who, t, name)?;
                    if !field(&t.decl, name).is_some_and(|f| f.searchable) {
                        return Err(on(
                            name,
                            Refusal::Invalid {
                                message: format!("`{name}` of `{}` is not searchable", t.decl.name),
                            },
                        ));
                    }
                }
                named.into_iter().collect()
            }
        };
        for name in searched
            .iter()
            .copied()
            .chain(vector_field)
            .chain(embedded_from)
        {
            readable(&st, who, t, name)?;
        }
        let e = filter(&st, who, t, search.filter.as_ref())?;
        let rows = candidates(&st, who, t, search.at, e.as_ref())?;
        // A field the lake asks an index for at the head answers the
        // index's set, and says so; the mock's set is the exact ranking.
        let indexed = match &search.mode {
            SearchMode::Vector { field, .. }
            | SearchMode::Hybrid {
                field,
                vector: Some(_),
                ..
            } => {
                let later;
                let head = match pinned {
                    Some(_) => {
                        later = self.state_at(&self.log(), None)?.1;
                        &later
                    }
                    None => &st,
                };
                head.types.get(&search.ty).is_some_and(|h| {
                    index_fields(head, &h.decl, VECTOR_INDEX_SETTING, indexable).contains(field)
                })
            }
            SearchMode::Lexical { .. } | SearchMode::Hybrid { .. } => false,
        };
        let (scored, ranked_by): (Vec<(f64, &StoredRecord)>, &str) = match &search.mode {
            SearchMode::Vector { field: f, vector }
            | SearchMode::Hybrid {
                field: f,
                vector: Some(vector),
                ..
            } => {
                readable(&st, who, t, f)?;
                let cos = |v: &Value| {
                    let v: Vec<f64> = v.as_array()?.iter().filter_map(Value::as_f64).collect();
                    let dot: f64 = v.iter().zip(vector).map(|(a, b)| a * f64::from(*b)).sum();
                    let n = |x: f64| x.sqrt();
                    let na = n(v.iter().map(|a| a * a).sum());
                    let nb = n(vector.iter().map(|b| f64::from(*b).powi(2)).sum());
                    (na > 0.0 && nb > 0.0).then(|| dot / (na * nb))
                };
                let s = rows
                    .into_iter()
                    .filter_map(|r| Some((cos(r.fields.get(f)?)?, r)))
                    .collect();
                let by = match (&search.mode, indexed) {
                    (SearchMode::Vector { .. }, true) => "index",
                    (SearchMode::Hybrid { .. }, true) => "hybrid (index)",
                    _ => "vector",
                };
                (s, by)
            }
            SearchMode::Lexical { text, .. } | SearchMode::Hybrid { text, .. } => {
                let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
                let s = rows
                    .into_iter()
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
                    .collect();
                (s, "lexical")
            }
        };
        let mut scored = scored;
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.key.cmp(&b.1.key)));
        let hits: Vec<Hit> = scored
            .into_iter()
            .map(|(score, r)| Hit {
                record: st.read(who, r, search.with_meta),
                score,
            })
            .collect();
        let (hits, next_cursor) = page(hits, position, offset, search.k.max(1) as usize);
        Ok(Reply {
            position,
            data: Hits {
                hits,
                next_cursor,
                ranked_by: ranked_by.to_owned(),
            },
        })
    }

    async fn propose(&self, who: &Principal, write: ProposedWrite, why: String) -> Result<Outcome> {
        self.admit_write(who)?;
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
        let mut batch = Batch::at(&log);
        let (position, proposal) = plan_propose(&st, &mut batch, who, write, why.clone(), None)?;
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
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        let p = decidable(&st, who, proposal)?;
        if p.write.is_schema() && !who.grants.schema {
            return Err(refused(Refusal::NotGranted));
        }
        let proposer = log
            .proposers
            .get(proposal)
            .cloned()
            .ok_or_else(|| invalid("the mock does not know who made this proposal"))?;
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
        let version = match p.write.clone() {
            ProposedWrite::Upsert {
                ty,
                key,
                patch,
                opts,
            } => Some(plan_upsert(&by, &mut batch, &ty, &key, patch, &opts)?),
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
                // been declared again while the proposal waited.
                let v = check_call(&st, &proposer, &verb, key.as_ref(), &payload, &opts)?.clone();
                let key = record_key(&v, key.as_ref())?;
                check_version(&st, &v.on, key, &opts)?;
                Some(plan_run(&by, &mut batch, &v, key, payload, &opts)?)
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
            ProposedWrite::Migrate { migration, .. } => {
                let head = Position(accepted.0.saturating_sub(1));
                let report = preflight(&st, head, &migration)?;
                if !report.lossless {
                    return Err(refused(Refusal::Migration {
                        report: Box::new(report),
                    }));
                }
                let version = st
                    .types
                    .get(&migration.to.name)
                    .map_or(1, |t| t.version + 1);
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
                batch.push(d);
                None
            }
        };
        let position = Position(batch.peek().0 - 1);
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position,
            version,
            output: None,
        })
    }

    async fn reject(
        &self,
        who: &Principal,
        proposal: &ProposalId,
        reason: String,
    ) -> Result<Outcome> {
        self.admit_write(who)?;
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
        })
    }

    async fn proposals(
        &self,
        who: &Principal,
        filter: ProposalFilter,
    ) -> Result<Reply<Vec<Proposal>>> {
        self.admit(who)?;
        let (pinned, offset) = match &filter.cursor {
            Some(c) => {
                let (p, o) = uncursor(c)?;
                (Some(p), o)
            }
            None => (filter.at.as_of, 0),
        };
        let (position, st) = self.state_at(&self.log(), pinned)?;
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
        };
        // A reviewer sees every proposal; anyone else sees their own, so
        // that a proposer can follow what became of it.
        let writer = who.writer();
        let mut rows: Vec<Proposal> = st
            .proposals
            .values()
            .filter(|p| who.grants.review || p.proposer == writer)
            .filter(|p| filter.state.is_none_or(|s| p.state == s))
            .filter(|p| filter.kind.is_none_or(|k| p.write.kind() == k))
            .filter(|p| filter.ty.as_ref().is_none_or(|ty| touches(&p.write, ty)))
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
        let (rows, _) = page(rows, position, offset, self.limit(filter.limit));
        Ok(Reply {
            position,
            data: rows,
        })
    }

    async fn declare(&self, who: &Principal, declaration: Declaration) -> Result<Outcome> {
        self.admit_write(who)?;
        if !who.grants.schema {
            return Err(refused(Refusal::NotGranted));
        }
        let mut log = self.log();
        let position = self.declare_in(&mut log, who, declaration)?;
        Ok(Outcome::Applied {
            position,
            version: None,
            output: None,
        })
    }

    /// `apply`, minimally: the declarations in order under one lock, as
    /// `declare` takes them, and none of them when one is refused. It does
    /// not yet resolve a reference to a declaration later in the set.
    async fn apply(&self, who: &Principal, declarations: Vec<Declaration>) -> Result<Outcome> {
        self.admit_write(who)?;
        if !who.grants.schema {
            return Err(refused(Refusal::NotGranted));
        }
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        check_set(&st, who, &declarations)?;
        for d in &declarations {
            trigger_ceiling(d, self.shared.limits.max_reactive_depth)?;
        }
        // One append at consecutive positions; the answer names the last.
        let mut batch = Batch::at(&log);
        let mut position = batch.peek();
        for d in set_drafts(&st, who, declarations) {
            position = batch.push(d);
        }
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position,
            version: None,
            output: None,
        })
    }

    async fn add_field(&self, who: &Principal, ty: &TypeName, f: FieldDecl) -> Result<Outcome> {
        self.admit_write(who)?;
        if !who.grants.schema {
            return Err(refused(Refusal::NotGranted));
        }
        let mut log = self.log();
        let (_, st) = self.state_at(&log, None)?;
        if reserved(ty.as_str(), who) {
            return Err(refused(Refusal::Reserved));
        }
        let t = st.types.get(ty).ok_or_else(|| no_type(ty))?;
        if field(&t.decl, &f.name).is_some() {
            return Err(on(
                &f.name,
                Refusal::Invalid {
                    message: format!("`{ty}` already has a field `{}`", f.name),
                },
            ));
        }
        if f.required && st.records.values().any(|r| &r.ty == ty && !r.retired) {
            return Err(on(
                &f.name,
                Refusal::Constraint {
                    message: format!(
                        "`{}` cannot be required: records of `{ty}` have no value for it",
                        f.name
                    ),
                },
            ));
        }
        let mut decl = t.decl.clone();
        decl.fields.push(f);
        embeddings(&decl)?;
        let mut batch = Batch::at(&log);
        let position = batch.push(Draft::new(
            who,
            Origin::System,
            EventBody::Declared {
                schema_hash: Some(
                    lakelands_api::shape::schema_hash(&decl).map_err(|e| invalid(e.to_string()))?,
                ),
                version: t.version + 1,
                declaration: Declaration::Type(decl),
            },
        ));
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position,
            version: None,
            output: None,
        })
    }

    async fn migrate(
        &self,
        who: &Principal,
        migration: Migration,
    ) -> Result<Reply<MigrationReport>> {
        self.admit(who)?;
        if !who.grants.schema {
            return Err(refused(Refusal::NotGranted));
        }
        let log = self.log();
        let (position, st) = self.state_at(&log, None)?;
        Ok(Reply {
            position,
            data: preflight(&st, position, &migration)?,
        })
    }

    async fn migrate_value_type(
        &self,
        who: &Principal,
        to: lakelands_api::ontology::ValueTypeDecl,
    ) -> Result<Reply<lakelands_api::write::ValueTypeReport>> {
        self.admit_write(who)?;
        if !who.grants.schema {
            return Err(refused(Refusal::NotGranted));
        }
        let log = self.log();
        let (head, st) = self.state_at(&log, None)?;
        check_new_value_type(&st, who, &to)?;
        Ok(Reply {
            position: head,
            data: crate::valuetypes::preflight(&st, head, &to)?,
        })
    }

    async fn migrate_value_type_apply(
        &self,
        who: &Principal,
        to: lakelands_api::ontology::ValueTypeDecl,
        report_id: &str,
    ) -> Result<Outcome> {
        self.admit_write(who)?;
        if !who.grants.schema {
            return Err(refused(Refusal::NotGranted));
        }
        let mut log = self.log();
        let (head, st) = self.state_at(&log, None)?;
        check_new_value_type(&st, who, &to)?;
        let report = crate::valuetypes::preflight(&st, head, &to)?;
        if report.report_id != report_id {
            return Err(invalid(
                "the lake moved since this report was made; run the preflight again",
            ));
        }
        if !report.fits {
            return Err(invalid(format!(
                "value type `{}` is not tightened: {} stored value(s) would not fit it",
                to.name, report.misfits
            )));
        }
        let declaration = Declaration::ValueType(to);
        let (schema_hash, version) = identity_version(&st, &declaration);
        let mut batch = Batch::at(&log);
        let position = batch.push(Draft::new(
            who,
            Origin::System,
            EventBody::Declared {
                declaration,
                schema_hash,
                version,
            },
        ));
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position,
            version: None,
            output: None,
        })
    }

    async fn migrate_apply(
        &self,
        who: &Principal,
        migration: Migration,
        report_id: &str,
    ) -> Result<Outcome> {
        self.admit_write(who)?;
        if !who.grants.schema {
            return Err(refused(Refusal::NotGranted));
        }
        let mut log = self.log();
        let (head, st) = self.state_at(&log, None)?;
        let report = preflight(&st, head, &migration)?;
        if report.report_id != report_id {
            return Err(invalid(
                "the lake moved since this report was made; run the preflight again",
            ));
        }
        if !report.lossless {
            return Err(refused(Refusal::Migration {
                report: Box::new(report),
            }));
        }
        let version = st
            .types
            .get(&migration.to.name)
            .map_or(1, |t| t.version + 1);
        let mut batch = Batch::at(&log);
        let position = batch.push(Draft::new(
            who,
            Origin::System,
            EventBody::Migrated {
                schema_hash: report.to_schema_hash.clone(),
                migration,
                version,
                report_id: report.report_id,
                rekeyed: Vec::new(),
            },
        ));
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position,
            version: None,
            output: None,
        })
    }

    async fn ingest(
        &self,
        who: &Principal,
        source: &SourceId,
        mut records: SourceStream,
    ) -> Result<CommitStream> {
        self.admit_write(who)?;
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
        if r.fields.is_empty() {
            return Err(invalid("nothing to write: the patch carries no field"));
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
        let now = w
            .planned
            .get(&slot)
            .cloned()
            .or_else(|| st.live(&r.ty, &r.key).map(|l| l.fields.clone()));
        let unchanged = now.as_ref().is_some_and(|fields| {
            checked.iter().all(|(f, v)| match fields.get(f) {
                Some(stored) => stored == v,
                None => v.is_null(),
            })
        });
        if unchanged || st.ingested.contains(&id) || !w.ids.insert(id.clone()) {
            w.duplicates += 1;
            continue;
        }
        let mut fields = checked;
        with_key_fields(
            t,
            st,
            who,
            st.live(&r.ty, &r.key),
            &r.key,
            &mut fields,
            Origin::Source,
        )?;
        // A link holds its relation's cardinality, over the lake's live
        // links and those earlier in this window.
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
                link_cardinality(st, &r.ty, &rel, &from, &to)?;
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
                    if (per_from && pe(links::FROM).as_deref() == Some(from.as_str()))
                        || (per_to && pe(links::TO).as_deref() == Some(to.as_str()))
                    {
                        return Err(on(
                            &FieldName::from(links::TO),
                            Refusal::Constraint {
                                message: format!(
                                    "the window links over `{}` already, and it allows one",
                                    rel.name
                                ),
                            },
                        ));
                    }
                }
            }
        }
        // A record earlier in this window counts as there.
        check_references(st, t, &fields, &w.versions)?;
        check_required(t, st.live(&r.ty, &r.key), &fields, Origin::Source)?;
        let mut after = now.unwrap_or_default();
        for (f, v) in &fields {
            if v.is_null() {
                after.remove(f);
            } else {
                after.insert(f.clone(), v.clone());
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

/// A pending proposal a reviewer may decide.
fn decidable<'a>(st: &'a State, who: &Principal, id: &ProposalId) -> Result<&'a Proposal> {
    if !who.grants.review {
        return Err(refused(Refusal::NotGranted));
    }
    let p = st.proposals.get(id).ok_or_else(|| Error::NotFound {
        missing: Missing::Proposal { id: id.clone() },
    })?;
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
    let at = |why: String| invalid(format!("type `{}`, relation `{}`: {why}", t.name, r.name));
    if r.via.is_some() || !r.fields.is_empty() || r.authority != Authority::Authored {
        return Err(at(
            "a derived relation has no `via`, fields or link authority".into(),
        ));
    }
    if d.pairs.is_empty() {
        return Err(at("a derived relation matches at least one pair".into()));
    }
    let target = if r.target == t.name {
        Some(t)
    } else {
        st.types.get(&r.target).map(|o| &o.decl)
    };
    for p in &d.pairs {
        let from = t.fields.iter().find(|f| f.name == p.from);
        let to = target.and_then(|u| u.fields.iter().find(|f| f.name == p.to));
        let (Some(from), Some(to)) = (from, to) else {
            return Err(at(format!(
                "the pair `{}` = `{}` names a field not declared",
                p.from, p.to
            )));
        };
        if from.ty.kind != to.ty.kind {
            return Err(at(format!(
                "the pair `{}` = `{}` matches fields of two kinds",
                p.from, p.to
            )));
        }
        if !lakelands_api::key::keyable(from.ty.kind) {
            return Err(at(format!(
                "the pair `{}` = `{}` is of a kind no key may be",
                p.from, p.to
            )));
        }
    }
    Ok(())
}

/// A trigger's `max_depth` lies within the lake's ceiling (021).
fn trigger_ceiling(d: &Declaration, ceiling: u32) -> Result<()> {
    match d {
        Declaration::Trigger(t) if t.max_depth.is_some_and(|m| m > ceiling) => {
            Err(invalid(format!(
                "trigger `{}`: its `max_depth` {} is above the lake's `max_reactive_depth` {ceiling}",
                t.name,
                t.max_depth.unwrap_or_default()
            )))
        }
        _ => Ok(()),
    }
}

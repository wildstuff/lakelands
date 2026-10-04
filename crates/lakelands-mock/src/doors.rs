//! The mock's [`Catalog`]: every door, over the log.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use lakelands_api::describe::{
    ExecutorDescription, FieldDescription, LakeDescription, LakeRights, TypeAccess,
    TypeDescription, TypeLifecycle, TypeLineage, TypeRights, VerbDescription,
};
use lakelands_api::error::{Error, Missing, Refusal, Result};
use lakelands_api::event::{EventBody, EventId};
use lakelands_api::expr::{Condition, Expr};
use lakelands_api::id::{
    Cursor, FieldName, LakeId, Position, ProposalId, RecordKey, RelationName, SourceId, TypeName,
    VerbName, Version,
};
use lakelands_api::ontology::{
    Authority, FieldDecl, RelationDecl, Risk, TypeDecl, VerbDecl, VerbEffect, VerbScope, ViewDecl,
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
/// loosened requirement or cardinality, is a declaration; and a view
/// stores nothing, so every change of one is, a view ceasing to be one
/// included.
fn reshapes(
    old: &lakelands_api::ontology::TypeDecl,
    new: &lakelands_api::ontology::TypeDecl,
) -> bool {
    use lakelands_api::ontology::Cardinality;
    if old.view.is_some() {
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
        return Err(invalid("the cursor was not issued by this lake"));
    };
    let (Ok(p), Ok(o)) = (p.parse(), o.parse()) else {
        return Err(invalid("the cursor was not issued by this lake"));
    };
    let on = |door: &str, ty: &str| match ty {
        "" => format!("`{door}`"),
        ty => format!("`{door}` on `{ty}`"),
    };
    let here = ty.map_or("", TypeName::as_str);
    if d != door || t != here {
        return Err(invalid(format!(
            "the cursor was handed out for {}, not {}; continue it there",
            on(d, t),
            on(door, here)
        )));
    }
    if f != of {
        return Err(invalid(format!(
            "the cursor was handed out for another `{door}`; continue it with the read its \
             first page was read with"
        )));
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
        let asked = asked.unwrap_or(l.default_limit);
        if asked == 0 || asked > max {
            return Err(invalid(format!("a limit is between 1 and {max}")));
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
        let (_, st) = self.state_at(log, None)?;
        let mut declaration = declaration;
        redeclared(&st, who, std::slice::from_mut(&mut declaration))?;
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
    not_a_view(&t.decl)?;
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

/// As the store: a view at a door that writes or holds a record is
/// refused in one sentence.
fn not_a_view(decl: &TypeDecl) -> Result<()> {
    match decl.view {
        Some(_) => Err(invalid(format!(
            "`{}` is a view; it is computed, never written",
            decl.name
        ))),
        None => Ok(()),
    }
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
            // As the store: a view reads only fields its sources have.
            if let Some(v) = t.view.as_deref() {
                for (from, reads) in view_reads(v) {
                    let Some(source) = st.types.get(&from) else {
                        continue;
                    };
                    if let Some(n) = reads
                        .iter()
                        .find(|n| !source.decl.fields.iter().any(|f| &f.name == *n))
                    {
                        return Err(invalid(format!(
                            "type `{}`: its view reads `{n}` of `{from}`, which `{from}` does not \
                             have",
                            t.name
                        )));
                    }
                }
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
                // As the store: a view's records are computed, so only a
                // relation derived from matching values starts from one.
                if t.view.is_some() && r.derived.is_none() {
                    let carrier = if r.via.is_some() {
                        "a `via` field"
                    } else {
                        "link records"
                    };
                    return Err(invalid(format!(
                        "type `{}`, relation `{}`: `{}` is a view, whose records are computed \
                         and never written, so a relation carried by {carrier} could never hold \
                         an edge; a relation on a view is derived from matching values",
                        t.name, r.name, t.name
                    )));
                }
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
                // As the store: a view is never written, so a trigger on
                // one never fires, and an object set holds stored records.
                if st.types.get(ty).is_some_and(|o| o.decl.view.is_some()) {
                    match other {
                        Declaration::Trigger(t) => {
                            return Err(Error::Refused {
                                field: Some("on".into()),
                                reason: Refusal::Invalid {
                                    message: format!(
                                        "trigger `{}` watches `{ty}`, a view; a view is never \
                                         written, so no write of it fires a trigger",
                                        t.name
                                    ),
                                },
                            });
                        }
                        Declaration::ObjectSet(s) => {
                            return Err(Error::Refused {
                                field: Some("of".into()),
                                reason: Refusal::Invalid {
                                    message: format!(
                                        "object set `{}` holds `{ty}`, a view; a view's records \
                                         are computed on read, and an object set holds stored \
                                         records only",
                                        s.name
                                    ),
                                },
                            });
                        }
                        // A verb on a record of a view could never act.
                        Declaration::Verb(v) if v.scope == VerbScope::Record => {
                            return Err(invalid(format!(
                                "verb `{}` is declared on a record of `{ty}`, a view, whose \
                                 records are computed and never written",
                                v.name
                            )));
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
    let Some(t) = st.visible(who, &v.on) else {
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
                let link = crate::links::link_type(&d.name, r);
                let mut r = r.clone();
                r.fields.retain(|f| access(who, &link, f) != Access::None);
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
                    lt.fields[..2].iter().all(|f| access(who, &lt, f).writes())
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
        .all(|n| field(&t.decl, n).is_some_and(|f| access(who, &t.decl, f).reads()))
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

/// As the store: a read of a view probes what its definition reads, so a
/// reader `describe` withholds the definition from is not granted it, and
/// no field of its source is named.
fn view_readable(st: &State, who: &Principal, t: &DeclaredType) -> Result<()> {
    match t.decl.view.as_deref() {
        Some(v) if !view_visible(st, who, v) => Err(refused(Refusal::NotGranted)),
        _ => Ok(()),
    }
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
    let f = field(&t.decl, name).ok_or_else(|| no_field(&t.decl.name, name))?;
    match access(who, &t.decl, f) {
        Access::None => Err(no_field(&t.decl.name, name)),
        a if !a.reads() => Err(on(name, Refusal::NotGranted)),
        _ => Ok(()),
    }
}

/// The answer for a field the type does not declare, which is also the
/// answer for one the principal may not know of.
fn no_field(ty: &TypeName, name: &FieldName) -> Error {
    on(
        name,
        Refusal::Invalid {
            message: format!("`{ty}` has no field `{name}`"),
        },
    )
}

/// A read at a valid time filters on the type's valid-time fields, so it
/// probes them as a filter on them would (024).
fn valid_time_readable(st: &State, who: &Principal, t: &DeclaredType, at: ReadAt) -> Result<()> {
    if at.valid_at.is_some() {
        for f in t.decl.valid_from.iter().chain(&t.decl.valid_to) {
            readable(st, who, t, f)?;
        }
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
    valid_time_readable(st, who, t, at)?;
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
    not_a_view(&t.decl)?;
    for (name, v) in patch {
        let f = field(&t.decl, name).ok_or_else(|| {
            on(
                name,
                Refusal::Invalid {
                    message: format!("`{ty}` has no field `{name}`"),
                },
            )
        })?;
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
            if access(who, &t.decl, f) == Access::None {
                return Err(no_field(&t.decl.name, name));
            }
            if !door_writes(door, f.authority) {
                return Err(on(name, Refusal::WrongDoor));
            }
            if !access(who, &t.decl, f).writes() {
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
            Some(f) if access(who, &t.decl, f).reads() => {}
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
    not_a_view(&t.decl)?;
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
    // The version before `requires` is judged on the record, whether the
    // call runs or is proposed (013 and 024, extended).
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

/// The preflight's report of a value type's tightening, refused while a
/// stored value would not fit, with the count alone: a proposal names no
/// misfit, as the store's does not.
fn tightening(
    st: &State,
    who: &Principal,
    head: Position,
    to: &lakelands_api::ontology::ValueTypeDecl,
) -> Result<lakelands_api::write::ValueTypeReport> {
    check_new_value_type(st, who, to)?;
    let report = crate::valuetypes::preflight(st, head, to)?;
    if !report.fits {
        return Err(invalid(format!(
            "value type `{}` is not tightened: {} stored value(s) would not fit it; change \
             them first.",
            to.name, report.misfits
        )));
    }
    Ok(report)
}

fn plan_propose(
    st: &State,
    batch: &mut Batch,
    who: &Principal,
    write: ProposedWrite,
    why: String,
    key: Option<lakelands_api::IdempotencyKey>,
) -> Result<(Position, ProposalId)> {
    // A change of shape is read as its proposer's view, and logged so.
    let mut write = write;
    match &mut write {
        ProposedWrite::Declare { declarations } => redeclared(st, who, declarations)?,
        ProposedWrite::Migrate { migration, .. } => authored(st, who, migration)?,
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
        ProposedWrite::Migrate { .. } | ProposedWrite::MigrateValueType { .. } => {}
    }
    let write = match write {
        ProposedWrite::Migrate { migration, .. } => {
            let head = Position(batch.peek().0.saturating_sub(1));
            let report = preflight(st, head, &migration, who)?;
            if !report.lossless {
                return Err(lossy(st, who, &migration, report));
            }
            ProposedWrite::Migrate {
                migration,
                report: Box::new(report),
            }
        }
        ProposedWrite::MigrateValueType { to, .. } => {
            let head = Position(batch.peek().0.saturating_sub(1));
            let report = Box::new(tightening(st, who, head, &to)?);
            ProposedWrite::MigrateValueType { to, report }
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
            if let Some(n) = filter.and_then(named) {
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
fn declaration_reads(d: &Declaration, ty: &TypeName) -> BTreeSet<FieldName> {
    let mut names = declaration_probes(d, ty);
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
fn declaration_probes(d: &Declaration, ty: &TypeName) -> BTreeSet<FieldName> {
    let of = |c: &Condition| named(c).unwrap_or_default();
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
                return Err(invalid(format!(
                    "verb `{}` sets `{}` from `payload.{m}`, which its payload does not have",
                    v.name, a.field
                )));
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
    if let Some(on) = on
        && let reads = declaration_reads(d, &on.name)
        && let Some(n) = (reads.iter()).find(|n| field(on, n).is_none())
    {
        return Err(invalid(format!(
            "{} reads `{n}`, which `{}` does not have",
            d.what(),
            on.name
        )));
    }
    Ok(())
}

/// As the store: a view with parameters is bound only by a read that names
/// it, so a pattern's step that arrives at one other than its start is
/// refused before anything is read.
fn unbound_view(start: &TypeName, reached: &DeclaredType, relation: &RelationName) -> Result<()> {
    match reached.decl.view.as_deref().and_then(|v| v.params.first()) {
        Some(param) if &reached.decl.name != start => Err(invalid(format!(
            "a step over `{relation}` arrives at view `{}`, which needs parameter `{}`, and only \
             a read that names the view binds it",
            reached.decl.name, param.name
        ))),
        _ => Ok(()),
    }
}

/// Reads `to`, a migration's or a redeclaration's shape of the type
/// declared as `before`, as its author's view, as the store does: a field
/// `who` may not know of is put back as declared, at its declared place,
/// and so are such link fields of each relation `to` keeps. A shape that
/// names one declares it twice, a rename from one is from no field, and a
/// field one is built from is refused on that field.
fn as_author(
    who: &Principal,
    before: &TypeDecl,
    to: &mut TypeDecl,
    renames: &[(FieldName, FieldName)],
) -> Result<()> {
    let hidden = |t: &TypeDecl, f: &FieldDecl| access(who, t, f) == Access::None;
    let on = |f: &FieldName, message: String| Error::Refused {
        field: Some(f.clone()),
        reason: Refusal::Invalid { message },
    };
    let put_back = |t: &TypeDecl, declared: &[FieldDecl], fields: &mut Vec<FieldDecl>| {
        for (i, f) in declared.iter().enumerate().filter(|(_, f)| hidden(t, f)) {
            fields.insert(i.min(fields.len()), f.clone());
        }
    };
    let unknown: Vec<&FieldDecl> = (before.fields.iter().filter(|f| hidden(before, f))).collect();
    for f in &unknown {
        if renames.iter().any(|(from, _)| from == &f.name) {
            let name = &f.name;
            return Err(invalid(format!(
                "rename from `{name}`: no such field before the migration"
            )));
        }
        if field(to, &f.name).is_some() {
            return Err(on(
                &f.name,
                format!("type `{}` declares `{}` twice", to.name, f.name),
            ));
        }
    }
    put_back(before, &before.fields, &mut to.fields);
    for r in &mut to.relations {
        if let Some(old) = before.relations.iter().find(|o| o.name == r.name) {
            let link = links::link_type(&before.name, old);
            put_back(&link, &old.fields, &mut r.fields);
        }
    }
    let mut reads = (unknown.iter().copied())
        .flat_map(|f| (f.depends_on.iter()).chain(f.embed.iter().flat_map(|e| &e.from)));
    match reads.find(|n| field(to, n).is_none()) {
        Some(n) => {
            let ty = &to.name;
            Err(on(
                n,
                format!("`{n}` is read by another field of `{ty}` and cannot be removed"),
            ))
        }
        None => Ok(()),
    }
}

/// A migration read as its author's view of its type.
fn authored(st: &State, who: &Principal, m: &mut Migration) -> Result<()> {
    reads_as_author(st, who, &[Declaration::Type(m.to.clone())], true)?;
    match st.types.get(&m.to.name) {
        Some(before) => as_author(who, &before.decl, &mut m.to, &m.renames),
        None => Ok(()),
    }
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
    let mut seen = st.clone();
    for t in seen.types.values_mut() {
        known_fields(who, &mut t.decl);
    }
    // As the store: what a type's shape names stands as declared, known to
    // its author or not: a key field, a valid time, a relation's `via`.
    let mut shaped = sent.to_vec();
    for d in &mut shaped {
        let Declaration::Type(t) = d else { continue };
        let (Some(was), Some(e)) = (st.types.get(&t.name), seen.types.get_mut(&t.name)) else {
            continue;
        };
        let via = t.relations.iter().filter_map(|r| r.via.as_ref());
        let named = t
            .key
            .iter()
            .chain(&t.valid_from)
            .chain(&t.valid_to)
            .chain(via);
        let named: Vec<FieldName> = named.cloned().collect();
        let declared = was.decl.fields.iter().enumerate();
        for (i, f) in declared.filter(|(_, f)| named.contains(&f.name)) {
            for fields in [&mut t.fields, &mut e.decl.fields] {
                if !fields.iter().any(|x| x.name == f.name) {
                    fields.insert(i.min(fields.len()), f.clone());
                }
            }
        }
    }
    // A migration's shape is refused as a declaration, of the type or of
    // a relation's links.
    let reshaping = |e: &Error| {
        let why = e.to_string();
        reshapes && (why.contains("change it with `migrate`") || why.contains("is a migration"))
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
            let mut names = declaration_probes(&new, &t.decl.name);
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
            as_author(who, &before.decl, t, &[])?;
        }
    }
    Ok(())
}

/// `t` without the fields `who` may not know of, a relation's link fields
/// too.
fn known_fields(who: &Principal, t: &mut TypeDecl) {
    let whole = t.clone();
    t.fields.retain(|f| access(who, &whole, f) != Access::None);
    for r in &mut t.relations {
        let link = links::link_type(&whole.name, r);
        r.fields.retain(|f| access(who, &link, f) != Access::None);
    }
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
/// know of the field; the totals, the hashes and the id stay, and `more`
/// says that something was left out. The mock restricts no record, so
/// every sample key is of a record its reader is answered.
fn shown_report(st: &State, who: &Principal, m: &Migration, report: &mut MigrationReport) {
    // As the store: one who may not see the migrated type is answered no
    // name at all.
    let Some(was) = st.visible(who, &m.to.name) else {
        (report.ty, report.more) = (TypeName::default(), true);
        report.fields.clear();
        report.dropped.clear();
        report.rekeyed.clear();
        report.references.clear();
        report.sample_collisions.clear();
        return;
    };
    let whole = report.clone();
    let (mut was, mut to) = (was.decl.clone(), m.to.clone());
    known_fields(who, &mut was);
    known_fields(who, &mut to);
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
    report.dropped.retain(|(f, _)| holds(&was, f));
    report.rekeyed.retain(|r| sees(&r.ty).is_some());
    report.references.retain(|r| known(&r.ty, &r.field));
    report.sample_collisions.retain(|c| sees(&c.ty).is_some());
    report.more = *report != whole;
}

/// The refusal of a migration that is not lossless, with its report as
/// the caller may know it.
fn lossy(st: &State, who: &Principal, m: &Migration, mut report: MigrationReport) -> Error {
    shown_report(st, who, m, &mut report);
    refused(Refusal::Migration {
        report: Box::new(report),
    })
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
fn known_shape(st: &State, who: &Principal, write: &mut ProposedWrite) -> bool {
    let seen = |ty: &TypeName| st.visible(who, ty).is_some();
    let read = |ty: &TypeName, f: &FieldName| {
        (st.visible(who, ty)).is_some_and(|t| readable(st, who, t, f).is_ok())
    };
    match write {
        // A field the reader may not know of and a masked one both leave
        // the patch: it holds values, and no place for a name.
        ProposedWrite::Upsert { ty, patch, .. } => {
            patch.retain(|f, _| read(ty, f));
            seen(ty)
        }
        ProposedWrite::Retire { ty, .. } => seen(ty),
        // A payload is what the verb's fields become: whole to a reader who
        // may read each field the verb sets, and empty otherwise.
        ProposedWrite::Call { verb, payload, .. } => {
            let Some(v) = st.verbs.get(verb) else {
                return false;
            };
            if let VerbEffect::Write { set } = &v.effect
                && !set.iter().all(|a| read(&v.on, &a.field))
            {
                payload.clear();
            }
            seen(&v.on)
        }
        // The report names what the migration does to the type as it
        // stands; its totals, hashes and id stay whole.
        ProposedWrite::Migrate { migration, report } => {
            let Some(was) = st.visible(who, &migration.to.name) else {
                return false;
            };
            shown_report(st, who, migration, report);
            let (mut was, Migration { to, renames }) = (was.decl.clone(), &mut **migration);
            known_fields(who, &mut was);
            known_fields(who, to);
            renames.retain(|(old, new)| holds(&was, old) && holds(to, new));
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
            declarations.retain(|d| listed(&after, who, d));
            for d in declarations.iter_mut() {
                if let Declaration::Type(t) = d {
                    known_fields(who, t);
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
    m: &Migration,
    who: &Principal,
) -> Result<MigrationReport> {
    let ty = &m.to.name;
    let t = st.types.get(ty).ok_or_else(|| no_type(ty))?;
    // As the store: a view changes, or becomes a stored type, by being
    // declared again; a stored type never becomes a view.
    if t.decl.view.is_some() || m.to.view.is_some() {
        return Err(invalid(
            match (t.decl.view.is_some(), m.to.view.is_some()) {
                (true, true) => format!(
                    "`{ty}` is a view, and a view changes by being declared again, never by a \
                 migration"
                ),
                (true, false) => format!(
                    "`{ty}` is a view, and it becomes a stored type by being declared as one, never \
                 by a migration"
                ),
                _ => format!(
                    "the migration would turn the stored type `{ty}` into a view; declare the view \
                 under another name (`{ty}` keeps its name and its records)"
                ),
            },
        ));
    }
    if t.decl.key != m.to.key {
        return Err(invalid(format!(
            "the in-memory mock does not re-key: `{ty}` keeps its key fields here"
        )));
    }
    // As the store: a view over the type, as its source or a merged one,
    // reads the shape the migration leaves, so every field it reads
    // (grouping, `where`, what its measures aggregate) must remain.
    for o in st.types.values() {
        let Some(v) = o.decl.view.as_deref() else {
            continue;
        };
        for (source, reads) in view_reads(v) {
            if &source != ty {
                continue;
            }
            if let Some(gone) = reads
                .iter()
                .find(|n| !m.to.fields.iter().any(|f| &f.name == *n))
            {
                let how = match m.renames.iter().find(|(from, _)| from == gone) {
                    Some((_, to)) => format!("renames to `{to}`"),
                    None => "drops".to_owned(),
                };
                // As the store: the view is named only to an author
                // `describe` shows its definition to.
                let seen = describe_lake(&who.lake, st, who);
                let shown = |s: &TypeDescription| s.name == o.decl.name && s.view.is_some();
                let reader = match seen.types.iter().any(shown) {
                    true => format!("view `{}`", o.decl.name),
                    false => "a declaration".to_owned(),
                };
                return Err(invalid(format!(
                    "after the migration of `{ty}`, {reader} reads `{gone}` of `{ty}`, which \
                     the migration {how}"
                )));
            }
        }
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
            let reads = declaration_reads(d, &was.name);
            let gone = |n: &&FieldName| field(was, n).is_some() && field(is, n).is_none();
            let said = |reader: String, n: &FieldName| {
                let renamed = |(from, _): &&(FieldName, FieldName)| from == n && &is.name == ty;
                let how = match m.renames.iter().find(renamed) {
                    Some((_, to)) => format!("renames to `{to}`"),
                    None => "drops".to_owned(),
                };
                invalid(format!(
                    "after the migration of `{ty}`, {reader} reads `{n}` of `{}`, which the \
                     migration {how}",
                    is.name
                ))
            };
            let Some(n) = reads.iter().find(gone) else {
                continue;
            };
            // As the store: an accepter who may not know of the field is told
            // neither it nor its reader.
            if field(was, n).is_some_and(|f| access(who, was, f) == Access::None) {
                return Err(invalid(format!(
                    "after the migration of `{ty}`, a declaration reads a field the migration \
                     takes away"
                )));
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
            let reader = if shown {
                d.what()
            } else {
                "a declaration".to_owned()
            };
            return Err(said(reader, n));
        }
    }
    // As the store: and the type it migrates to reads only fields it has.
    reads_its_type(st, &Declaration::Type(m.to.clone()))?;
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
    // A relation the migration removes takes its live links from the
    // head: it is listed with their number, as the store lists it.
    for lt in links::of(&t.decl) {
        let live = st
            .records
            .values()
            .filter(|r| r.ty == lt.name && !r.retired);
        let relation = links::parts(&lt.name).map(|(_, r)| r);
        if let (Some(r), n @ 1..) = (relation, live.count() as u64)
            && !m.to.relations.iter().any(|kept| kept.name == r)
        {
            dropped.push((FieldName::from(r.as_str()), n));
        }
    }
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
        more: false,
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
        bound(&t.decl, &find.params)?;
        view_readable(&st, who, t)?;
        let after = find
            .after
            .as_ref()
            .map(|a| resume_after(&st, t, &find, a))
            .transpose()?;
        if find.total && find.aggregate.is_some() {
            return Err(invalid(
                "`total` counts the records a page lists; an `aggregate` answers groups, so give one",
            ));
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
                    return Err(invalid(format!(
                        "`offset` and `{other}` both say where the answer starts; give one"
                    )));
                }
                let max = self.shared.limits.max_offset;
                if skip > max {
                    return Err(on(&"offset".into(), Refusal::TooLarge { max }));
                }
                skip as usize
            }
        };
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
        let (position, st) = self.state_at(&self.log(), get.at.as_of)?;
        let t = st.visible(who, ty).ok_or_else(|| no_type(ty))?;
        bound(&t.decl, &get.params)?;
        view_readable(&st, who, t)?;
        valid_time_readable(&st, who, t, get.at)?;
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
            set_off: Default::default(),
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
            set_off: Default::default(),
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
            set_off: Default::default(),
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
            set_off: Default::default(),
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
                set_off: Default::default(),
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
        bound(&t.decl, &start.params)?;
        view_readable(&st, who, t)?;
        let measure = matches!(pattern.project, Projection::Degree | Projection::Components);
        let limit = self.limit(pattern.limit, measure)?;
        if start.search.is_some() || pattern.to.is_some() {
            return Err(invalid(
                "the in-memory mock starts a pattern from a key or a filter only",
            ));
        }
        if measure {
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
                // Either end of the relation the measure may arrive at.
                for o in st.types.values() {
                    for r in o.decl.relations.iter().filter(|r| r.name == step.relation) {
                        for end in [&o.decl.name, &r.target] {
                            if let Some(e) = st.types.get(end) {
                                unbound_view(&t.decl.name, e, &step.relation)?;
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
            return Err(invalid("the in-memory mock projects patterns as rows only"));
        };
        // Rows are ordered by their paths, `type.field`: each probes its
        // field as `find`'s order does (024). The mock answers its rows in
        // walk order whatever the order asks for.
        for o in &pattern.order_by {
            if let Some((ty, field)) = o.field.as_str().rsplit_once('.')
                && let Some(ot) = st.types.get(&TypeName::from(ty))
            {
                readable(&st, who, ot, &FieldName::from(field))?;
            }
        }
        let e = filter(&st, who, t, start.filter.as_ref())?;
        let mut paths: Vec<Vec<&StoredRecord>> = candidates(&st, who, t, pattern.at, e.as_ref())?
            .into_iter()
            .filter(|r| start.key.as_ref().is_none_or(|k| &r.key == k))
            .map(|r| vec![r])
            .collect();
        let mut scope_ty = t;
        for step in &pattern.steps {
            // A step over a relation the type does not declare is
            // refused, not answered empty. A step naming a relation's
            // `inverse` walks that relation the other way; a relation
            // declared under the name wins.
            let forward = |named: &dyn Fn(&RelationDecl) -> bool| {
                let r = scope_ty.decl.relations.iter().find(|r| named(r))?;
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
                return Err(invalid(match step.walk {
                    Walk::Out => format!(
                        "`{}` declares no relation `{}`",
                        scope_ty.decl.name, step.relation
                    ),
                    _ => format!(
                        "no relation `{}` reaches `{}`",
                        step.relation, scope_ty.decl.name
                    ),
                }));
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
            view_readable(&st, who, next)?;
            unbound_view(&t.decl.name, next, &step.relation)?;
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
        if t.decl.view.is_some() {
            return Err(invalid(format!(
                "`{}` is a view, and a view is not searched",
                search.ty
            )));
        }
        if let Some((field, message)) = search.mode.misfit() {
            let reason = Refusal::Invalid { message };
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
            // Naming none, it reads the searchable fields the caller knows
            // of; one it may not know of is never searched.
            None => t
                .decl
                .fields
                .iter()
                .filter(|f| f.searchable && access(who, &t.decl, f) != Access::None)
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
        for name in searched.iter().copied().chain(probed) {
            readable(&st, who, t, name)?;
        }
        let e = filter(&st, who, t, search.filter.as_ref())?;
        let rows = candidates(&st, who, t, search.at, e.as_ref())?;
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
        if let (
            SearchMode::Hybrid {
                text, text_weight, ..
            },
            false,
        ) = (&search.mode, by_text)
        {
            lists.push((text_weight.unwrap_or(1.0), matching(text)));
        }
        for (f, vector, weight) in vectors.iter().filter(|_| !by_text) {
            lists.push((weight.unwrap_or(1.0), nearest(f, vector)));
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
        let write = p.write.clone().expect("a held proposal holds its write");
        if write.is_schema() && !who.grants.schema {
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
        let version = match write {
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
                // been declared again while the proposal waited. Declared
                // differently since, it is not what was checked (013).
                if let Some(now) = st.verbs.get(&verb) {
                    scoped(now, key.as_ref(), &opts)?;
                    let (_, then) = self.state_at(&log, Some(p.position))?;
                    if then.verbs.get(&verb) != Some(now) {
                        return Err(invalid(format!(
                            "verb `{verb}` was declared again, differently, after this call \
                             was proposed; propose the call again"
                        )));
                    }
                }
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
            ProposedWrite::Migrate {
                migration,
                report: shown,
            } => {
                // The shape is the type's whole: on a type changed since
                // the proposal it would remove what was added since.
                let ty = &migration.to.name;
                let on = st.types.get(ty).map(|t| &t.schema_hash);
                if on != Some(&shown.from_schema_hash) {
                    return Err(invalid(format!(
                        "`{ty}` changed since this migration was proposed; propose it again"
                    )));
                }
                let head = Position(accepted.0.saturating_sub(1));
                let report = preflight(&st, head, &migration, who)?;
                if !report.lossless {
                    return Err(lossy(&st, who, &migration, report));
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
            ProposedWrite::MigrateValueType { to, report: shown } => {
                // As a migration: accepted only on the declaration it was
                // proposed on, and only while every stored value fits.
                let declaration = Declaration::ValueType((*to).clone());
                let (schema_hash, version) = identity_version(&st, &declaration);
                if version != shown.from_version + 1 {
                    return Err(invalid(format!(
                        "value type `{}` changed since this was proposed; propose it again",
                        to.name
                    )));
                }
                let head = Position(accepted.0.saturating_sub(1));
                tightening(&st, &proposer, head, &to)?;
                let body = EventBody::Declared {
                    declaration,
                    schema_hash,
                    version,
                };
                let mut d = Draft::new(&proposer, Origin::System, body);
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
            set_off: Default::default(),
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
            set_off: Default::default(),
        })
    }

    async fn proposals(
        &self,
        who: &Principal,
        filter: ProposalFilter,
    ) -> Result<Reply<Vec<Proposal>>> {
        self.admit(who)?;
        let limit = self.limit(filter.limit, false)?;
        let (pinned, offset) = match &filter.cursor {
            Some(c) => {
                let (p, o) = uncursor(c, "proposals", None, "")?;
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
            p.write = shown.and_then(|mut w| known_shape(&st, who, &mut w).then_some(w));
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
        if !who.grants.schema {
            return Err(refused(Refusal::NotGranted));
        }
        let mut log = self.log();
        let position = self.declare_in(&mut log, who, declaration)?;
        Ok(Outcome::Applied {
            position,
            version: None,
            output: None,
            set_off: Default::default(),
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
        let mut declarations = declarations;
        redeclared(&st, who, &mut declarations)?;
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
            set_off: Default::default(),
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
            set_off: Default::default(),
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
        let mut migration = migration;
        authored(&st, who, &mut migration)?;
        let mut data = preflight(&st, position, &migration, who)?;
        shown_report(&st, who, &migration, &mut data);
        Ok(Reply { position, data })
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
            set_off: Default::default(),
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
        let mut migration = migration;
        authored(&st, who, &mut migration)?;
        let report = preflight(&st, head, &migration, who)?;
        if report.report_id != report_id {
            return Err(invalid(
                "the lake moved since this report was made; run the preflight again",
            ));
        }
        if !report.lossless {
            return Err(lossy(&st, who, &migration, report));
        }
        let version = st
            .types
            .get(&migration.to.name)
            .map_or(1, |t| t.version + 1);
        let mut batch = Batch::at(&log);
        let link_types = links::of(&migration.to);
        let report_id = report.report_id;
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
            };
            batch.push(Draft::new(who, Origin::System, body));
        }
        self.commit(&mut log, batch);
        Ok(Outcome::Applied {
            position,
            version: None,
            output: None,
            set_off: Default::default(),
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

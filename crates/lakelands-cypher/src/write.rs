//! The translation of a statement that writes into the one door call it
//! makes, and nothing else: an `upsert` for a node's `CREATE` or
//! `MERGE`, a `link` for a relationship's, and for `MATCH … SET`,
//! `REMOVE` or `DELETE` the plan, or with its id the enactment, of a
//! change to the records the `MATCH` selects.
//!
//! The statement alone is translated first, every refusal it earns
//! made before the store is asked anything. Then the types it names are
//! described, which reads the ontology and no record, to key a node by
//! its type's declared key and to refuse a field the lake computes. A
//! right, a ceiling or a position is the door's to judge, never this.

use lakelands_api::error::{Case, Construct, Error, Result};
use lakelands_api::expr::{CmpOp, Condition, Expr, Literal};
use lakelands_api::key::{FROM, TO, key_component, link_type_name};
use lakelands_api::ontology::Authority;
use lakelands_api::query::{Depth, StatementAnswer, Walk};
use lakelands_api::record::Patch;
use lakelands_api::write::{Change, ChangeTo, WriteOpts};
use lakelands_api::{Catalog, FieldName, Principal, RecordKey, RelationName, TypeName, Version};
use lakelands_api::{ReadAt, Value};

use crate::parse::{Assign, Kind, Match, Name, Node, Part, Query, Where, literal, outside};
use crate::read::map_paths;

type Parsed<T> = std::result::Result<T, Case>;

/// A statement that writes, translated as far as its text says: the
/// door it calls, before the types it names are described.
#[derive(Clone, Debug, PartialEq)]
pub struct Write(pub(crate) Op);

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Op {
    /// `CREATE` or `MERGE` of one node: an `upsert`.
    Put { node: End, create: bool, set: Patch },
    /// `CREATE` or `MERGE` of one relationship between two matched
    /// nodes: a `link` of its owner's relation.
    Link {
        from: End,
        relation: RelationName,
        to: End,
        create: bool,
        fields: Patch,
    },
    /// A change to the records of `ty` that `filter` selects; on a link
    /// type, each end named by its node's key, where its map gives one.
    Change {
        ty: TypeName,
        filter: Vec<Expr>,
        ends: Option<(End, End)>,
        to: ChangeTo,
    },
}

/// A node by its type, the values its map gives, and where it stands.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct End {
    ty: TypeName,
    map: Patch,
    at: usize,
}

/// What a node's map may hold beside its key.
#[derive(Clone, Copy)]
enum Only {
    /// Any field: a `CREATE`'s.
    Any,
    /// The key alone: a `MERGE`'s.
    Merge,
    /// The key alone, naming a link's end.
    End(usize),
}

/// The construct an updating clause is.
fn construct(kind: &Kind) -> Construct {
    match kind {
        Kind::Put { merge: true, .. } => Construct::Merge,
        Kind::Put { .. } => Construct::Create,
        Kind::Set { remove: true, .. } => Construct::Remove,
        Kind::Set { .. } => Construct::Set,
        _ => Construct::Delete,
    }
}

fn second(construct: Construct, offset: usize) -> Case {
    Case::CypherSecondWrite { construct, offset }
}

fn unbound(name: &Name) -> Case {
    let (name, offset) = (name.name.clone(), name.at);
    Case::CypherUnbound { name, offset }
}

/// A node's type and its map's values, each a literal or a parameter.
fn end(node: Node) -> Parsed<End> {
    let label = node
        .label
        .ok_or(Case::CypherStartType { offset: node.at })?;
    let mut map = Patch::new();
    for p in node.props {
        let value = literal(&p.value).ok_or(outside(Construct::ComputedValue, p.at))?;
        map.insert(FieldName::from(p.key), value);
    }
    Ok(End {
        ty: TypeName::from(label),
        map,
        at: node.at,
    })
}

/// The statement's one door call, from its clauses: its `MATCH`, then
/// its updating clauses, of which the first names the door and every
/// other adds to the same call or is refused as a second.
pub(crate) fn write(q: Query) -> Parsed<Op> {
    let mut matches = Vec::new();
    let mut writes = Vec::new();
    for clause in q.clauses {
        match clause.kind {
            Kind::Match(_) if !writes.is_empty() => {
                return Err(outside(Construct::WriteSelection, clause.at));
            }
            Kind::Match(m) => matches.push((clause.at, m)),
            kind => writes.push((clause.at, kind)),
        }
    }
    let mut writes = writes.into_iter();
    let (_, first) = writes.next().expect("a write statement writes");
    let made = construct(&first);
    let (mut op, written) = match first {
        Kind::Put { mut parts, .. } => {
            if let Some(p) = parts.get(1) {
                return Err(second(made, p.nodes[0].at));
            }
            put(matches, parts.remove(0), made)?
        }
        kind => changed(matches, kind)?,
    };
    for (at, kind) in writes {
        let (
            Kind::Set { items, .. },
            Op::Put { set, .. }
            | Op::Link { fields: set, .. }
            | Op::Change {
                to: ChangeTo::Set(set),
                ..
            },
        ) = (&kind, &mut op)
        else {
            return Err(second(construct(&kind), at));
        };
        assign(&kind, items, written.as_deref(), &mut |f, v| {
            set.insert(f, v);
        })?;
    }
    Ok(op)
}

/// Each item of a `SET` or `REMOVE` on the name `written`, given to
/// `put`; one on another name is a second write.
fn assign(
    kind: &Kind,
    items: &[Assign],
    written: Option<&str>,
    put: &mut dyn FnMut(FieldName, Value),
) -> Parsed<()> {
    for a in items {
        if Some(a.name.as_str()) != written {
            return Err(second(construct(kind), a.at));
        }
        put(FieldName::from(a.field.clone()), a.value.clone());
    }
    Ok(())
}

/// `CREATE` or `MERGE` of `part`: a node alone, or one relationship
/// between two nodes the `MATCH` binds by their maps alone.
fn put(
    matches: Vec<(usize, Match)>,
    mut part: Part,
    made: Construct,
) -> Parsed<(Op, Option<String>)> {
    let create = made == Construct::Create;
    if part.rels.is_empty() {
        if let Some((at, _)) = matches.first() {
            return Err(outside(Construct::WriteSelection, *at));
        }
        let node = part.nodes.remove(0);
        let written = node.var.clone();
        let op = Op::Put {
            node: end(node)?,
            create,
            set: Patch::new(),
        };
        return Ok((op, written));
    }
    if let Some(rel) = part.rels.get(1) {
        return Err(second(made, rel.at));
    }
    let rel = part.rels.remove(0);
    if rel.walk == Walk::Both {
        return Err(outside(Construct::UndirectedWrite, rel.at));
    }
    if rel.depth != Depth::default() {
        return Err(outside(Construct::WriteSelection, rel.at));
    }
    // Each end a name the `MATCH` binds, by a node of its own.
    let mut bound = Vec::new();
    for (at, m) in matches {
        if m.optional || m.filter.is_some() {
            return Err(outside(Construct::WriteSelection, at));
        }
        for mut p in m.parts {
            if let Some(r) = p.rels.first() {
                return Err(outside(Construct::WriteSelection, r.at));
            }
            bound.push(p.nodes.remove(0));
        }
    }
    let mut ends = Vec::new();
    for node in part.nodes {
        if node.label.is_some() || !node.props.is_empty() {
            return Err(second(made, node.at));
        }
        let name = node.var.clone().unwrap_or_default();
        let i = bound.iter().position(|b| b.var.as_deref() == Some(&name));
        let name = Name { name, at: node.at };
        ends.push(i.ok_or_else(|| unbound(&name))?);
    }
    let mut ends: Vec<End> = (ends.into_iter().rev())
        .map(|i| end(bound.remove(i)))
        .collect::<Parsed<_>>()?;
    if rel.walk == Walk::Out {
        ends.reverse();
    }
    let (from, to) = (ends.remove(0), ends.remove(0));
    let mut fields = Patch::new();
    for p in rel.props.map(|(_, p)| p).unwrap_or_default() {
        if !create {
            let ty = link_type_name(&from.ty, &RelationName::from(rel.relation));
            let field = FieldName::from(p.key);
            return Err(Case::CypherMergeOn { ty, field });
        }
        let value = literal(&p.value).ok_or(outside(Construct::ComputedValue, p.at))?;
        fields.insert(FieldName::from(p.key), value);
    }
    let written = rel.var.map(|v| v.name);
    let relation = RelationName::from(rel.relation);
    let op = Op::Link {
        from,
        relation,
        to,
        create,
        fields,
    };
    Ok((op, written))
}

/// `MATCH … SET`, `REMOVE` or `DELETE` of the one name the `MATCH`
/// writes: its one node, or its one relationship between two.
fn changed(mut matches: Vec<(usize, Match)>, kind: Kind) -> Parsed<(Op, Option<String>)> {
    let first = match &kind {
        Kind::Set { items, .. } => Name {
            name: items[0].name.clone(),
            at: items[0].at,
        },
        Kind::Delete(names) => names[0].clone(),
        Kind::Put { .. } | Kind::Match(_) | Kind::Return(_) => unreachable!("a change's clause"),
    };
    if matches.is_empty() {
        return Err(unbound(&first));
    }
    if let Some((at, _)) = matches.get(1) {
        return Err(outside(Construct::WriteSelection, *at));
    }
    let (at_match, m) = matches.remove(0);
    if m.optional {
        return Err(outside(Construct::WriteSelection, at_match));
    }
    if let Some(p) = m.parts.get(1) {
        return Err(outside(Construct::WriteSelection, p.nodes[0].at));
    }
    let mut part = m.parts.into_iter().next().expect("a MATCH has a part");
    if let Some(n) = part.nodes.get(2) {
        return Err(outside(Construct::WriteSelection, n.at));
    }
    let mut filter = Vec::new();
    let mut others = Vec::new();
    let (ty, written, ends) = if part.rels.is_empty() {
        let node = part.nodes.remove(0);
        let label = node
            .label
            .ok_or(Case::CypherStartType { offset: node.at })?;
        for p in node.props {
            filter.push(prop(p)?);
        }
        (TypeName::from(label), node.var, None)
    } else {
        let rel = part.rels.remove(0);
        let (b, a) = (part.nodes.remove(1), part.nodes.remove(0));
        let (from, to) = match rel.walk {
            Walk::Out => (a, b),
            Walk::In => (b, a),
            Walk::Both => return Err(outside(Construct::UndirectedWrite, rel.at)),
        };
        if rel.depth != Depth::default() {
            return Err(outside(Construct::WriteSelection, rel.at));
        }
        let names = [&from, &to].map(|n| n.var.clone());
        let keyed = |n: Node| match (n.label.is_some(), n.props.first()) {
            (false, Some(p)) => Err(outside(Construct::WriteSelection, p.at)),
            (false, None) => Ok(End {
                ty: TypeName::from(""),
                map: Patch::new(),
                at: n.at,
            }),
            _ => end(n),
        };
        let ends = (keyed(from)?, keyed(to)?);
        if ends.0.ty.as_str().is_empty() {
            return Err(Case::CypherStartType { offset: rel.at });
        }
        let at = rel.at;
        others.extend(names.into_iter().flatten().map(|name| Name { name, at }));
        for p in rel.props.map(|(_, p)| p).unwrap_or_default() {
            filter.push(prop(p)?);
        }
        let relation = RelationName::from(rel.relation);
        let written = rel.var.map(|v| v.name);
        (link_type_name(&ends.0.ty, &relation), written, Some(ends))
    };
    let written = match written {
        Some(w) if w == first.name => w,
        _ if others.iter().any(|o| o.name == first.name) => {
            return Err(outside(Construct::WriteSelection, first.at));
        }
        _ => return Err(unbound(&first)),
    };
    if let Some(w) = m.filter {
        filter.push(condition(w, &written, &others)?);
    }
    let to = match &kind {
        Kind::Delete(names) => {
            if let Some(n) = names.get(1) {
                let bound = n.name == written || others.iter().any(|o| o.name == n.name);
                return Err(if bound {
                    second(Construct::Delete, n.at)
                } else {
                    unbound(n)
                });
            }
            ChangeTo::Retire
        }
        Kind::Set { items, .. } => {
            let mut patch = Patch::new();
            let stray = items.iter().find(|a| a.name != written);
            if let Some(a) = stray.filter(|a| !others.iter().any(|o| o.name == a.name)) {
                let (name, at) = (a.name.clone(), a.at);
                return Err(unbound(&Name { name, at }));
            }
            assign(&kind, items, Some(&written), &mut |f, v| {
                patch.insert(f, v);
            })?;
            ChangeTo::Set(patch)
        }
        _ => unreachable!("a change's clause"),
    };
    let op = Op::Change {
        ty,
        filter,
        ends,
        to,
    };
    Ok((op, Some(written)))
}

/// A map's property as a condition on the record it is the map of: its
/// value reads no name.
fn prop(p: crate::parse::Prop) -> Parsed<Expr> {
    if let Some(n) = p.names.first() {
        return Err(outside(Construct::WriteSelection, n.at));
    }
    Ok(eq(&p.key, p.value))
}

fn eq(field: &str, value: Expr) -> Expr {
    Expr::Cmp {
        cmp: CmpOp::Eq,
        lhs: Box::new(Expr::Path {
            path: vec![FieldName::from(field)],
        }),
        rhs: Box::new(value),
    }
}

/// A `WHERE` on the name `written`, said of its own record: a name
/// `others` binds is a second name the condition cannot say.
fn condition(w: Where, written: &str, others: &[Name]) -> Parsed<Expr> {
    if let Some(a) = w.absent.first() {
        return Err(outside(Construct::WriteSelection, a.at));
    }
    if let Some(n) = w.names.iter().find(|n| n.name != written) {
        let bound = others.iter().any(|o| o.name == n.name);
        return Err(match bound {
            true => outside(Construct::WriteSelection, n.at),
            false => unbound(n),
        });
    }
    let own = |path: Vec<FieldName>| match path.len() {
        1 => path,
        _ => path[1..].to_vec(),
    };
    Ok(map_paths(w.expr, &own))
}

/// The statement's door called as `who`: a change planned, or enacted
/// with `plan_id`; every other write called once.
pub(crate) async fn run(
    catalog: &dyn Catalog,
    who: &Principal,
    op: Op,
    plan_id: Option<&str>,
) -> Result<StatementAnswer> {
    let described = |ty: TypeName| async move {
        let at = ReadAt::default();
        Ok::<_, Error>(catalog.describe_type(who, &ty, at).await?.data)
    };
    if let (Some(_), Op::Put { .. } | Op::Link { .. }) = (plan_id, &op) {
        let member = "plan_id".to_owned();
        return Err(Error::invalid(Case::CypherMember { member }));
    }
    let expect = |create: bool| WriteOpts {
        expected_version: create.then_some(Version(0)),
        idempotency_key: None,
    };
    match op {
        Op::Put { node, create, set } => {
            let d = described(node.ty.clone()).await?;
            let only = if create { Only::Any } else { Only::Merge };
            let (key, mut patch) = keyed(&d, node.map, only)?;
            patch.extend(set);
            computed(&d, &patch)?;
            let out = catalog.upsert(who, &node.ty, &key, patch, expect(create));
            Ok(StatementAnswer::Written(out.await?))
        }
        Op::Link {
            from,
            relation,
            to,
            create,
            fields,
        } => {
            let only = Only::End(from.at);
            let from_key = keyed(&described(from.ty.clone()).await?, from.map, only)?.0;
            let to_key = keyed(&described(to.ty).await?, to.map, Only::End(to.at))?.0;
            if !fields.is_empty() {
                let ty = link_type_name(&from.ty, &relation);
                computed(&described(ty).await?, &fields)?;
            }
            let out = catalog.link(
                who,
                &from.ty,
                &from_key,
                &relation,
                &to_key,
                fields,
                expect(create),
            );
            Ok(StatementAnswer::Written(out.await?))
        }
        Op::Change {
            ty,
            mut filter,
            ends,
            to,
        } => {
            if let ChangeTo::Set(patch) = &to {
                computed(&described(ty.clone()).await?, patch)?;
            }
            for (field, end) in ends.into_iter().flat_map(|(f, t)| [(FROM, f), (TO, t)]) {
                if !end.map.is_empty() {
                    let only = Only::End(end.at);
                    let key = keyed(&described(end.ty).await?, end.map, only)?.0;
                    filter.push(eq(
                        field,
                        Expr::Lit {
                            value: Literal::Text(key.to_string()),
                        },
                    ));
                }
            }
            let filter = match filter.len() {
                0 => Expr::Lit {
                    value: Literal::Bool(true),
                },
                1 => filter.remove(0),
                _ => Expr::And { args: filter },
            };
            let change = Change::Records {
                ty,
                filter: Box::new(Condition::Expr(filter)),
                to,
                only: Default::default(),
                skip_restricted: false,
            };
            Ok(match plan_id {
                None => StatementAnswer::Planned(catalog.plan(who, change).await?),
                Some(id) => StatementAnswer::Written(catalog.enact(who, change, id).await?),
            })
        }
    }
}

/// The key a node's map gives a record of the type `d` describes, and
/// the patch it writes: its declared key fields, each given, or for a
/// type keyed by its writer the map's `key`, which no field holds.
/// `only` says what else the map may hold.
fn keyed(
    d: &lakelands_api::describe::TypeDescription,
    mut map: Patch,
    only: Only,
) -> Result<(RecordKey, Patch)> {
    let ty = d.name.clone();
    let writer = d.access.key.is_empty();
    let key_fields = match writer {
        true => vec![FieldName::from("key")],
        false => d.access.key.clone(),
    };
    if let Some(field) = map.keys().find(|f| !key_fields.contains(f)) {
        let field = field.clone();
        match only {
            Only::Any => {}
            Only::Merge => return Err(Error::invalid(Case::CypherMergeOn { ty, field })),
            Only::End(offset) => {
                let construct = Construct::WriteSelection;
                return Err(Error::invalid(Case::CypherOutside { construct, offset }));
            }
        }
    }
    let mut parts = Vec::new();
    for field in key_fields {
        let missing = || {
            let (ty, field) = (ty.clone(), field.clone());
            Error::invalid(Case::CypherKeyMissing { ty, field })
        };
        let value = map.get(&field).ok_or_else(missing)?;
        parts.push(match (writer, value) {
            (true, Value::String(s)) => s.clone(),
            _ => key_component(value).ok_or_else(missing)?,
        });
    }
    if writer {
        map.remove(&FieldName::from("key"));
    }
    Ok((RecordKey::from(parts.join(":")), map))
}

/// Refuses a field of `patch` the lake computes, by the type `d`
/// describes, before any door is called.
fn computed(d: &lakelands_api::describe::TypeDescription, patch: &Patch) -> Result<()> {
    let computes = |f: &&lakelands_api::describe::FieldDescription| {
        matches!(f.authority, Authority::Derived | Authority::Writer)
            || f.derived.is_some()
            || f.function.is_some()
            || f.embed.is_some()
    };
    let found = d
        .fields
        .iter()
        .filter(computes)
        .find(|f| patch.contains_key(&f.name));
    match found {
        Some(f) => {
            let (ty, field) = (d.name.clone(), f.name.clone());
            Err(Error::invalid(Case::CypherComputed { ty, field }))
        }
        None => Ok(()),
    }
}

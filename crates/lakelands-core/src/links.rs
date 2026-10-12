//! Link records: the one definition of the records that carry a relation
//! (`docs/decisions/035-a-link-is-a-record.md`, `docs/log-format.md` §2.3).
//!
//! A relation `r` of type `t` with neither `via` nor `derived` is carried by records
//! of its **link type**, `sys.link.t.r`, which this module derives from the
//! relation, the same way for the store and every engine:
//!
//! - its key fields are `from` (a `reference` to `t`) and `to` (a
//!   `reference` to the relation's target), both required, so a link's key
//!   is `<from key>:<to key>` by the key rule and one relation holds one
//!   link per pair;
//! - its other fields are the relation's own `fields`, as declared;
//! - `from` and `to` take the relation's `authority`: `authored` links are
//!   written through the link doors, `source` links by ingest.
//!
//! [`link_edges`] is the rule every engine applies to answer a record's
//! edges over such relations: from the live link records at the view's
//! position, each an [`Edge`] carrying the link's own fields.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::Position;
use lakelands_api::id::{RecordKey, RelationName, TypeName};
use lakelands_api::ontology::{Authority, FieldDecl, RelationDecl, StorageKind, TypeDecl};
use lakelands_api::principal::Renamed;
use lakelands_api::query::{Edge, Walk};
use lakelands_api::record::{StoredRecord, Value};

/// The prefix of every link type's name.
pub const PREFIX: &str = "sys.link.";

pub use lakelands_api::key::{FROM, TO, link_key};

/// The name of the link type of relation `r` on type `t`.
pub fn link_type_name(t: &TypeName, r: &RelationName) -> TypeName {
    TypeName::from(format!("{PREFIX}{t}.{r}"))
}

/// Whether a type is a link type.
pub fn is_link_type(name: &TypeName) -> bool {
    name.as_str().starts_with(PREFIX)
}

/// The type and relation a link type's name spells, whether or not the
/// relation exists or is carried by link records.
pub fn link_type_parts(name: &TypeName) -> Option<(TypeName, RelationName)> {
    let (t, r) = name.as_str().strip_prefix(PREFIX)?.rsplit_once('.')?;
    (!t.is_empty() && !r.is_empty()).then(|| (TypeName::from(t), RelationName::from(r)))
}

/// The link types a migration's renamed relations move their records
/// between, old name to new: those of the relations carried by link
/// records. A rename keeps the carrier, so it is read off the relation as
/// the migration's own shape declares it, whatever the type has become
/// since the migration was written (035, extended).
pub fn moved(
    m: &lakelands_api::write::Migration,
) -> std::collections::BTreeMap<TypeName, TypeName> {
    let carried = |(_, new): &&(RelationName, RelationName)| {
        (m.to.relations.iter()).any(|r| &r.name == new && r.carried_by_links())
    };
    let link = |r| link_type_name(&m.to.name, r);
    (m.relation_renames.iter().filter(carried))
        .map(|(old, new)| (link(old), link(new)))
        .collect()
}

/// The entries a migration adds to the lake's list of renames at `at`, as
/// the fold records them: its renamed fields, if it renames any, and each
/// link type a renamed relation moves.
pub fn renamed(m: &lakelands_api::write::Migration, at: Position) -> Vec<Renamed> {
    let fields = Renamed {
        at,
        ty: m.to.name.clone(),
        renames: m.renames.clone(),
        to: None,
    };
    let types = moved(m).into_iter().map(|(ty, to)| Renamed {
        at,
        ty,
        to: Some(to),
        ..Renamed::default()
    });
    let fields = (!m.renames.is_empty()).then_some(fields);
    fields.into_iter().chain(types).collect()
}

fn key_field(name: &str, target: &TypeName, authority: Authority) -> FieldDecl {
    let mut f = FieldDecl::of(name, StorageKind::Reference);
    f.ty.target = Some(target.clone());
    f.required = true;
    f.authority = authority;
    f
}

/// The link type of relation `r` on type `t`.
pub fn link_type(t: &TypeName, r: &RelationDecl) -> TypeDecl {
    let mut fields = vec![
        key_field(FROM, t, r.authority),
        key_field(TO, &r.target, r.authority),
    ];
    fields.extend(r.fields.iter().cloned());
    TypeDecl {
        key: vec![FROM.into(), TO.into()],
        fields,
        field_meta: true,
        description: Some(format!(
            "The links of relation `{}` of `{t}` to `{}`.",
            r.name, r.target
        )),
        ..TypeDecl::named(link_type_name(t, &r.name))
    }
}

/// The link types a type's relations call for, in declaration order.
pub fn link_types_of(t: &TypeDecl) -> Vec<TypeDecl> {
    t.relations
        .iter()
        .filter(|r| r.carried_by_links())
        .map(|r| link_type(&t.name, r))
        .collect()
}

/// The relation a link type carries, found among `types`: the type whose
/// link types include it, and the relation.
pub fn relation_of<'a>(
    types: impl IntoIterator<Item = &'a TypeDecl>,
    link_type: &TypeName,
) -> Option<(&'a TypeDecl, &'a RelationDecl)> {
    types.into_iter().find_map(|t| {
        t.relations
            .iter()
            .filter(|r| r.carried_by_links())
            .find(|r| link_type_name(&t.name, &r.name) == *link_type)
            .map(|r| (t, r))
    })
}

/// The edge a live link record stands for.
fn edge_of(t: &TypeName, r: &RelationDecl, link: &StoredRecord) -> Option<Edge> {
    let from = link.fields.get(FROM)?.as_str()?;
    let to = link.fields.get(TO)?.as_str()?;
    let fields = link
        .fields
        .iter()
        .filter(|(name, _)| name.as_str() != FROM && name.as_str() != TO)
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    Some(Edge {
        from: (t.clone(), RecordKey::from(from)),
        relation: r.name.clone(),
        to: (r.target.clone(), RecordKey::from(to)),
        fields,
    })
}

/// The edges of many records of one type over the relations carried by
/// link records, one list per key: out, the links of `ty`'s relations
/// whose `from` is the key; in, the links of every type's relations to
/// `ty` whose `to` is; over `relation` or all.
/// `links_where(link_type, field, keys)` answers, once for all the keys,
/// the link type's records at the view's position whose `field` (`from`
/// or `to`) holds one of `keys`, found through an index, or the link
/// type's records there, whole; this gives each link to the key its end
/// names and decides which count: only live links, each once per key.
/// What it is handed is counted (`engines::links_read`).
pub fn link_edges<'a, E>(
    ty: &TypeName,
    keys: &[RecordKey],
    relation: Option<&RelationName>,
    walk: Walk,
    types: impl IntoIterator<Item = &'a TypeDecl>,
    mut links_where: impl FnMut(&TypeName, &str, &[RecordKey]) -> Result<Vec<StoredRecord>, E>,
) -> Result<Vec<Vec<Edge>>, E> {
    let out = matches!(walk, Walk::Out | Walk::Both);
    let inn = matches!(walk, Walk::In | Walk::Both);
    let mut index: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        index.entry(k.as_str()).or_default().push(i);
    }
    let mut edges = vec![Vec::new(); keys.len()];
    let mut seen: BTreeSet<(usize, TypeName, RecordKey)> = BTreeSet::new();
    for t in types {
        for r in t
            .relations
            .iter()
            .filter(|r| r.carried_by_links() && relation.is_none_or(|want| *want == r.name))
        {
            let lt = link_type_name(&t.name, &r.name);
            let mut wanted: Vec<&str> = Vec::new();
            if out && t.name == *ty {
                wanted.push(FROM);
            }
            if inn && r.target == *ty {
                wanted.push(TO);
            }
            for field in wanted {
                let links = links_where(&lt, field, keys)?;
                crate::engines::count_links(links.len());
                for link in links {
                    let at = link.fields.get(field).and_then(Value::as_str);
                    let Some(ends) = at.and_then(|at| index.get(at)) else {
                        continue;
                    };
                    if link.retired {
                        continue;
                    }
                    for &i in ends {
                        if seen.insert((i, lt.clone(), link.key.clone()))
                            && let Some(e) = edge_of(&t.name, r, &link)
                        {
                            edges[i].push(e);
                        }
                    }
                }
            }
        }
    }
    Ok(edges)
}

#[cfg(test)]
mod tests;

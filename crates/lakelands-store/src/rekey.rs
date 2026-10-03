//! Re-keying a type: which record keys a migration moves, and what moves
//! with them (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! `docs/log-format.md` §2.3 and §6).
//!
//! A record's key is its name everywhere in the lake, so a migration that
//! changes how a type's key is made moves more than that type's records:
//!
//! - every record of the type, retired and restricted ones included, whose
//!   key the new key fields make differently;
//! - the **closure**: every type whose key fields reference a re-keyed
//!   type is re-keyed too, its references to the moved keys substituted
//!   before its own key is made again. Link types, keyed by their two
//!   ends, are this case. It is computed with a visited set, so it ends;
//! - every reference field, in any type, whose value is a moved key: it is
//!   rewritten to the new key.
//!
//! [`plan`] computes the moves and what the preflight reports about them;
//! the fold applies exactly the moves the `migrated` event lists, and
//! derives the reference rewrites from them.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::ProjectionView;
use lakelands_api::ontology::{StorageKind, TypeDecl};
use lakelands_api::write::{RecordRef, ReferenceRewrite, Rekey, Rekeyed};
use lakelands_api::{FieldName, RecordKey, Result, TypeName, Value};
use lakelands_core::{invalid, links};

use crate::lake::every;
use crate::ontology::Ontology;

/// How many colliding keys a report names.
const NAMED: usize = 10;

/// The moves of one re-key, per type: old key to new key.
pub type Moves = BTreeMap<TypeName, BTreeMap<RecordKey, RecordKey>>;

/// What a re-key does, as the preflight reports it.
#[derive(Default)]
pub struct Plan {
    /// Every record key moved, per type.
    pub moves: Moves,
    /// Per type: records moved, collisions, missing new keys.
    pub rekeyed: Vec<Rekeyed>,
    /// Per reference field: values rewritten.
    pub references: Vec<ReferenceRewrite>,
    /// The first colliding new keys.
    pub collisions: Vec<RecordRef>,
    /// Whether every moved record takes a key of its own.
    pub lossless: bool,
}

impl Plan {
    /// The moves as the `migrated` event lists them.
    pub fn listed(&self) -> Vec<Rekey> {
        self.moves
            .iter()
            .flat_map(|(ty, m)| {
                m.iter().map(|(from, to)| Rekey {
                    ty: ty.clone(),
                    from: from.clone(),
                    to: to.clone(),
                })
            })
            .collect()
    }
}

/// The reference fields of `decl` that name a type with moves.
fn moved_references<'a>(decl: &'a TypeDecl, moves: &Moves) -> Vec<(&'a FieldName, &'a TypeName)> {
    decl.fields
        .iter()
        .filter(|f| f.ty.kind == StorageKind::Reference)
        .filter_map(|f| {
            let target = f.ty.target.as_ref()?;
            moves.contains_key(target).then_some((&f.name, target))
        })
        .collect()
}

/// A record's fields with every reference to a moved key rewritten.
pub fn rewritten(
    decl: &TypeDecl,
    fields: &BTreeMap<FieldName, Value>,
    moves: &Moves,
) -> BTreeMap<FieldName, Value> {
    let mut out = fields.clone();
    for (f, target) in moved_references(decl, moves) {
        if let Some(Value::String(old)) = fields.get(f)
            && let Some(new) = moves
                .get(target)
                .and_then(|m| m.get(&RecordKey::from(old.as_str())))
        {
            out.insert(f.clone(), Value::String(new.to_string()));
        }
    }
    out
}

/// Tally one type's new keys into `plan`: the moves, the collisions, the
/// missing.
fn tally(ty: &TypeName, keyed: Vec<(RecordKey, Option<RecordKey>)>, plan: &mut Plan) {
    let mut taken: BTreeMap<RecordKey, u64> = BTreeMap::new();
    for (_, new) in &keyed {
        if let Some(k) = new {
            *taken.entry(k.clone()).or_default() += 1;
        }
    }
    let mut moves = BTreeMap::new();
    let mut row = Rekeyed {
        ty: ty.clone(),
        records: 0,
        collisions: 0,
        missing: 0,
    };
    for (old, new) in keyed {
        match new {
            None => row.missing += 1,
            Some(new) => {
                if taken.get(&new).copied().unwrap_or(0) > 1 {
                    row.collisions += 1;
                    if plan.collisions.len() < NAMED
                        && !plan.collisions.iter().any(|c| c.ty == *ty && c.key == new)
                    {
                        plan.collisions.push(RecordRef {
                            ty: ty.clone(),
                            key: new.clone(),
                        });
                    }
                }
                if new != old {
                    row.records += 1;
                    moves.insert(old, new);
                }
            }
        }
    }
    if row.records > 0 || row.collisions > 0 || row.missing > 0 {
        plan.rekeyed.push(row);
    }
    if !moves.is_empty() {
        plan.moves.insert(ty.clone(), moves);
    }
}

/// Plan a re-key. `after` is the ontology with the migrated type's new
/// declaration in force; `converted` are that type's records, retired and
/// restricted included, with their fields as the migration converts them.
pub fn plan(
    after: &Ontology,
    view: &dyn ProjectionView,
    migrated: &TypeName,
    converted: Vec<(RecordKey, BTreeMap<FieldName, Value>)>,
) -> Result<Plan> {
    let mut plan = Plan::default();
    let decl = after.ty(migrated)?;
    let keyed = converted
        .into_iter()
        .map(|(old, fields)| {
            let new = lakelands_core::log::record_key(decl, &fields);
            (old, new)
        })
        .collect();
    tally(migrated, keyed, &mut plan);
    // The closure: types keyed by a reference to a re-keyed type.
    let mut visited: BTreeSet<TypeName> = BTreeSet::from([migrated.clone()]);
    loop {
        let next = after.types.iter().find(|(name, e)| {
            !visited.contains(*name)
                && e.decl.key.iter().any(|k| {
                    e.decl.fields.iter().any(|f| {
                        &f.name == k
                            && f.ty.kind == StorageKind::Reference
                            && f.ty
                                .target
                                .as_ref()
                                .is_some_and(|t| plan.moves.contains_key(t))
                    })
                })
        });
        let Some((name, entry)) = next else { break };
        let (name, decl) = (name.clone(), entry.decl.clone());
        visited.insert(name.clone());
        if decl.view.is_some() {
            return Err(invalid(format!(
                "re-keying `{migrated}` would re-key the view `{name}`, which is keyed by it; a \
                 view's rows are computed and cannot be moved, so that is not supported"
            )));
        }
        if name.as_str().starts_with("sys.") && !links::is_link_type(&name) {
            return Err(invalid(format!(
                "re-keying `{migrated}` would re-key the system type `{name}`, which is keyed \
                 by it; that is not supported"
            )));
        }
        let keyed = view
            .scan(&name, &every(true))?
            .into_iter()
            .filter(|r| !r.is_ended())
            .map(|r| {
                let fields = rewritten(&decl, &r.fields, &plan.moves);
                let new = lakelands_core::log::record_key(&decl, &fields);
                (r.key, new)
            })
            .collect();
        tally(&name, keyed, &mut plan);
    }
    // The reference fields, in any type, that hold a moved key.
    for (name, entry) in &after.types {
        if entry.decl.view.is_some() {
            continue;
        }
        let refs = moved_references(&entry.decl, &plan.moves);
        if refs.is_empty() {
            continue;
        }
        let records: Vec<_> = view
            .scan(name, &every(true))?
            .into_iter()
            .filter(|r| !r.is_ended())
            .collect();
        for (f, target) in refs {
            let values = records
                .iter()
                .filter(|r| {
                    r.fields
                        .get(f)
                        .and_then(Value::as_str)
                        .is_some_and(|v| plan.moves[target].contains_key(&RecordKey::from(v)))
                })
                .count() as u64;
            if values > 0 {
                plan.references.push(ReferenceRewrite {
                    ty: name.clone(),
                    field: f.clone(),
                    values,
                });
            }
        }
    }
    plan.lossless = plan
        .rekeyed
        .iter()
        .all(|r| r.collisions == 0 && r.missing == 0);
    Ok(plan)
}

#[cfg(test)]
mod tests;

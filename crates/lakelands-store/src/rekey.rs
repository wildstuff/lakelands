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
//!   ends, are this case. A type's keys are made after those of every
//!   type its key references, once, from keys that no longer move; a
//!   re-key that would move keys made of each other is refused;
//! - every reference field, in any type, whose value is a moved key: it is
//!   rewritten to the new key.
//!
//! [`plan`] computes the moves and what the preflight reports about them;
//! the fold applies exactly the moves the `migrated` event lists, and
//! derives the reference rewrites from them.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::ProjectionView;
use lakelands_api::error::Case;
use lakelands_api::ontology::{StorageKind, TypeDecl};
use lakelands_api::write::{RecordRef, ReferenceRewrite, Rekey, Rekeyed};
use lakelands_api::{FieldName, RecordKey, Result, TypeName, Value};
use lakelands_core::{invalid, links};

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
pub(crate) fn moved_references<'a>(
    decl: &'a TypeDecl,
    moves: &Moves,
) -> Vec<(&'a FieldName, &'a TypeName)> {
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

/// The types the key fields of `decl` reference: only a reference names a
/// target.
fn key_targets(decl: &TypeDecl) -> impl Iterator<Item = &TypeName> {
    let keys = (decl.fields.iter()).filter(|f| decl.key_fields().contains(&f.name));
    keys.filter_map(|f| f.ty.target.as_ref())
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
/// declaration in force; `keyed` are that type's records, retired and
/// restricted included and moved or not, each with its key and the key its
/// fields make as the migration converts them (`None` where they make
/// none); `renames` are the migration's, each an old name and its new one.
/// Every other type is walked a page at a time; what is kept is keys.
pub fn plan(
    after: &Ontology,
    view: &dyn ProjectionView,
    migrated: &TypeName,
    keyed: Vec<(RecordKey, Option<RecordKey>)>,
    renames: &[(FieldName, FieldName)],
) -> Result<Plan> {
    let mut plan = Plan::default();
    tally(migrated, keyed, &mut plan);
    // The closure: types keyed by a reference to a re-keyed type. Each one
    // it can reach waits until every other type its key references has its
    // moves, and is scanned only if one of them has any.
    let types = || after.types.iter().map(|(name, e)| (name, &e.decl));
    let mut waiting: BTreeSet<&TypeName> = BTreeSet::new();
    while let Some((name, _)) = types().find(|(n, d)| {
        let reached = |t| t == migrated || waiting.contains(t);
        *n != migrated && !waiting.contains(n) && key_targets(d).any(reached)
    }) {
        waiting.insert(name);
    }
    let moving = |d: &TypeDecl, plan: &Plan| key_targets(d).any(|t| plan.moves.contains_key(t));
    while let Some((name, decl)) = types().find(|(n, d)| {
        waiting.contains(n) && key_targets(d).all(|t| t == *n || !waiting.contains(t))
    }) {
        waiting.remove(name);
        if !moving(decl, &plan) {
            continue;
        }
        let (name, decl) = (name.clone(), decl.clone());
        if decl.view.is_some() {
            let (ty, view) = (migrated.clone(), name);
            return Err(invalid(Case::RekeyView { ty, view }));
        }
        if name.as_str().starts_with("sys.") && !links::is_link_type(&name) {
            let (ty, system) = (migrated.clone(), name);
            return Err(invalid(Case::RekeySystem { ty, system }));
        }
        let mut keyed = Vec::new();
        crate::lake::walk(view, &name, true, |r| {
            let fields = rewritten(&decl, &r.fields, &plan.moves);
            keyed.push((r.key, lakelands_core::log::record_key(&decl, &fields)));
            Ok(())
        })?;
        tally(&name, keyed, &mut plan);
    }
    // No order makes keys that are made of each other: those of a moved
    // type whose key reads its own keys (the migrated type's: any that moved
    // after it), and those of the types left waiting for each other.
    let own = |n: &&TypeName| {
        let later = |t| *n == migrated && plan.moves.contains_key(t);
        key_targets(&after.types[*n].decl).any(|t| t == *n || later(t))
    };
    let mut ring: Vec<&TypeName> = plan.moves.keys().filter(own).collect();
    if waiting.iter().any(|n| moving(&after.types[*n].decl, &plan)) {
        ring.extend(waiting);
    }
    if !ring.is_empty() {
        let (ty, ring) = (migrated.clone(), ring.into_iter().collect());
        return Err(invalid(Case::RekeyRing { ty, ring }));
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
        // The migrated type's records hold a renamed field under its old name.
        let held = |f: &FieldName| {
            let renamed = |(_, new): &&(FieldName, FieldName)| name == migrated && new == f;
            renames
                .iter()
                .find(renamed)
                .map_or(f, |(old, _)| old)
                .clone()
        };
        let held: Vec<FieldName> = refs.iter().map(|(f, _)| held(f)).collect();
        // Counted over the stored fields, one walk for all the type's fields.
        let mut counts = vec![0u64; refs.len()];
        crate::lake::walk(view, name, true, |r| {
            for (((_, target), held), n) in refs.iter().zip(&held).zip(&mut counts) {
                let names = |v| plan.moves[*target].contains_key(&RecordKey::from(v));
                *n += u64::from(
                    r.fields
                        .get(held)
                        .and_then(Value::as_str)
                        .is_some_and(names),
                );
            }
            Ok(())
        })?;
        for ((f, _), values) in refs.into_iter().zip(counts) {
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

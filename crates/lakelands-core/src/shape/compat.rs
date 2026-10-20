//! Declare or migrate: whether a revision of a type reshapes the records
//! already stored.
//!
//! The schema hash says only *whether* a shape changed. Before a new
//! declaration of a type enters the log, the store needs to know *how*:
//! a `declared` event must never reshape a stored record (log format
//! §2.2), so every revision under which some stored record could stop
//! matching its type goes through a migration, with its preflight, and
//! everything else is simply declared.
//!
//! The rule:
//!
//! - **Declared** — nothing stored has to change: a change of meaning
//!   only (descriptions, labels, authority, indexing marks); a new
//!   optional field; a required field made optional; a new relation; a
//!   relation whose cardinality only widens; any change to a view, which
//!   stores nothing, a view becoming a stored type included.
//! - **Migrated** — a stored record could stop matching: a field or
//!   relation removed; a field's type changed (its kind, value type,
//!   reference target or vector length); an optional field made
//!   required; a required field added; the key changed, in its fields or
//!   their order; a relation retargeted or its cardinality narrowed; a
//!   stored type turned into a view. The same rules hold for the fields
//!   a relation carries on its links.
//!
//! A rename is not visible here: comparing two declarations, a renamed
//! field reads as one field removed and another added, which needs a
//! migration, and the migration names the rename (`Migration::renames`)
//! so that the values move instead of vanishing.

use std::collections::BTreeMap;

use lakelands_api::ontology::{Cardinality, FieldDecl, RelationDecl, TypeDecl};
use lakelands_api::shape::view_shape;
use lakelands_api::{FieldName, RelationName};
use serde::Serialize;

/// What a revision of a type needs.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum Change {
    /// The revision may be declared: no stored record is reshaped.
    Declare {
        /// The shape changes it makes, none of which reshapes a stored
        /// record. Empty when only meaning changed.
        changes: Vec<Reason>,
    },
    /// The revision needs a migration.
    Migrate {
        /// Every change that reshapes stored records. Never empty.
        reasons: Vec<Reason>,
    },
}

impl Change {
    /// Whether the revision needs a migration.
    pub fn needs_migration(&self) -> bool {
        matches!(self, Change::Migrate { .. })
    }
}

/// One difference between two declarations of a type, a field's naming
/// the relation whose links carry it; each case says whether it reshapes.
pub use lakelands_api::error::Revision as Reason;

/// Whether this change, on a stored type, reshapes stored records: an
/// added field when it is required, a cardinality when it narrows.
pub fn reshapes(reason: &Reason) -> bool {
    match reason {
        Reason::FieldAdded { required, .. } => *required,
        Reason::CardinalityChanged { from, to, .. } => !widens(*from, *to),
        Reason::FieldRemoved { .. }
        | Reason::FieldRetyped { .. }
        | Reason::FieldRequired { .. }
        | Reason::KeyChanged { .. }
        | Reason::RelationRemoved { .. }
        | Reason::RelationRetargeted { .. }
        | Reason::BecameView => true,
        _ => false,
    }
}

/// Compare two declarations of one type: may `after` be declared over
/// `before`, or does it need a migration?
///
/// Only shape is compared, exactly the members the schema hash covers,
/// so a revision whose schema hash is unchanged is always
/// `Declare { changes: [] }`. When `before` is a view nothing is stored, so
/// every change is declared, a view becoming a stored type included.
pub fn classify(before: &TypeDecl, after: &TypeDecl) -> Change {
    let all = differences(before, after);
    // A view stores nothing, whatever it becomes: no change of it reshapes
    // a stored record.
    let stores_nothing = before.view.is_some();
    let (reasons, changes): (Vec<Reason>, Vec<Reason>) = all
        .into_iter()
        .partition(|r| !stores_nothing && reshapes(r));
    if reasons.is_empty() {
        Change::Declare { changes }
    } else {
        Change::Migrate { reasons }
    }
}

/// Every difference in shape between two declarations of one type, those
/// that reshape and those that do not, fields first.
pub fn differences(before: &TypeDecl, after: &TypeDecl) -> Vec<Reason> {
    let mut all = Vec::new();
    diff_fields(&before.fields, &after.fields, None, &mut all);
    if before.key_fields() != after.key_fields() {
        all.push(Reason::KeyChanged {
            from: before.key_fields().iter().collect(),
            to: after.key_fields().iter().collect(),
        });
    }
    diff_relations(&before.relations, &after.relations, &mut all);
    match (&before.view, &after.view) {
        (None, None) => {}
        (None, Some(_)) => all.push(Reason::BecameView),
        (Some(_), None) => all.push(Reason::StoppedBeingView),
        (Some(a), Some(b)) => {
            let same = match (view_shape(a), view_shape(b)) {
                (Ok(x), Ok(y)) => x == y,
                _ => a == b,
            };
            if !same {
                all.push(Reason::ViewChanged);
            }
        }
    }
    all
}

fn by_name(fields: &[FieldDecl]) -> BTreeMap<&FieldName, &FieldDecl> {
    fields.iter().map(|f| (&f.name, f)).collect()
}

/// The field differences, in field-name order.
fn diff_fields(
    before: &[FieldDecl],
    after: &[FieldDecl],
    relation: Option<&RelationName>,
    out: &mut Vec<Reason>,
) {
    let old = by_name(before);
    let new = by_name(after);
    let relation = || relation.cloned();
    for (name, o) in &old {
        let Some(n) = new.get(name) else {
            out.push(Reason::FieldRemoved {
                field: (*name).clone(),
                relation: relation(),
            });
            continue;
        };
        if o.ty != n.ty {
            out.push(Reason::FieldRetyped {
                field: (*name).clone(),
                relation: relation(),
                from: o.ty.clone(),
                to: n.ty.clone(),
            });
        }
        match (o.required, n.required) {
            (false, true) => out.push(Reason::FieldRequired {
                field: (*name).clone(),
                relation: relation(),
            }),
            (true, false) => out.push(Reason::FieldOptional {
                field: (*name).clone(),
                relation: relation(),
            }),
            _ => {}
        }
    }
    for (name, n) in &new {
        if !old.contains_key(name) {
            out.push(Reason::FieldAdded {
                field: (*name).clone(),
                relation: relation(),
                required: n.required,
            });
        }
    }
}

/// The relation differences, in relation-name order.
fn diff_relations(before: &[RelationDecl], after: &[RelationDecl], out: &mut Vec<Reason>) {
    let old: BTreeMap<&RelationName, &RelationDecl> = before.iter().map(|r| (&r.name, r)).collect();
    let new: BTreeMap<&RelationName, &RelationDecl> = after.iter().map(|r| (&r.name, r)).collect();
    for (name, o) in &old {
        let Some(n) = new.get(name) else {
            out.push(Reason::RelationRemoved {
                relation: (*name).clone(),
            });
            continue;
        };
        if o.target != n.target {
            out.push(Reason::RelationRetargeted {
                relation: (*name).clone(),
                from: o.target.clone(),
                to: n.target.clone(),
            });
        }
        if o.cardinality != n.cardinality {
            out.push(Reason::CardinalityChanged {
                relation: (*name).clone(),
                from: o.cardinality,
                to: n.cardinality,
            });
        }
        diff_fields(&o.fields, &n.fields, Some(name), out);
    }
    for name in new.keys() {
        if !old.contains_key(name) {
            out.push(Reason::RelationAdded {
                relation: (*name).clone(),
            });
        }
    }
}

/// Whether each side of a relation may link many: (this type's side,
/// the target's side). `many_to_one` means many records of this type
/// link one target, so each record links at most one target.
fn sides(c: Cardinality) -> (bool, bool) {
    match c {
        Cardinality::OneToOne => (false, false),
        Cardinality::OneToMany => (false, true),
        Cardinality::ManyToOne => (true, false),
        Cardinality::ManyToMany => (true, true),
    }
}

/// Whether moving from `from` to `to` only allows more links: no side
/// that allowed many allows one afterwards.
fn widens(from: Cardinality, to: Cardinality) -> bool {
    let (a, b) = sides(from);
    let (c, d) = sides(to);
    (!a || c) && (!b || d)
}

#[cfg(test)]
mod tests;

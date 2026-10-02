//! Grants applied: what one principal may see of a record, what a read may
//! probe, and what a write may carry
//! (`docs/decisions/006-grants-not-roles.md`).
//!
//! Three rules, each enforced here and nowhere else:
//!
//! - **Reading.** A field the principal may read is returned in full. A
//!   field it may not read is absent — or, if the field is declared
//!   `sensitive` or the grant says `masked`, listed as masked so the reader
//!   knows a value exists without seeing it. A restricted record is
//!   withheld from a principal without the `restricted` grant; a retired
//!   record from everyone.
//! - **Probing.** A read that filters on a field, orders by it or
//!   aggregates over it learns something about its values even if the
//!   field is masked in the answer: a filter answers "is there a record
//!   whose salary is above X" record by record, and a sum over one record
//!   is that record's value. So a read that probes a field the principal
//!   may not read is refused outright, never answered with masking.
//! - **Writing.** A write names the door it comes through — authored
//!   (`upsert`, a verb) or source (`ingest`). Every field it carries must
//!   exist, be written by that door's authority, and be granted to the
//!   principal; one field that fails refuses the whole write
//!   (`docs/decisions/012-writes-touch-only-their-fields.md`).

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::ontology::{Authority, FieldDecl, TypeDecl};
use lakelands_api::record::StoredRecord;
use lakelands_api::{Access, Error, FieldName, Principal, Record, Refusal, TypeName, Value};

/// The prefix every system type's name carries
/// (`docs/decisions/003-reservation-by-namespace.md`).
pub const SYSTEM_PREFIX: &str = "sys.";

/// Whether a type name is reserved for system types.
pub fn is_system(ty: &TypeName) -> bool {
    ty.as_str().starts_with(SYSTEM_PREFIX)
}

/// One principal's access to one declared field: the grant, with a
/// `sensitive` field shown masked rather than dropped when the grant gives
/// no read.
pub fn field_access(who: &Principal, ty: &TypeName, field: &FieldDecl) -> Access {
    match who.grants.access(ty, &field.name) {
        Access::None if field.sensitive => Access::Masked,
        a => a,
    }
}

/// Whether the principal may retire records of a type: every field of the
/// type is writable as its grants resolve it, field by field, whatever the
/// field's authority. Retiring removes every field of a record, so a
/// principal who may write only some of them may not; one granted each
/// field one by one may. A type with no field falls back to its type-level
/// grant. A view's records are computed, and are never retired.
pub fn may_retire(who: &Principal, decl: &TypeDecl) -> bool {
    if decl.view.is_some() {
        return false;
    }
    if decl.fields.is_empty() {
        return type_access(who, &decl.name).writes();
    }
    decl.fields
        .iter()
        .all(|f| who.grants.access(&decl.name, &f.name).writes())
}

/// The principal's access to a type as a whole: its type-level grant, or
/// the lake-wide default.
pub fn type_access(who: &Principal, ty: &TypeName) -> Access {
    who.grants
        .types
        .get(ty)
        .and_then(|t| t.default)
        .unwrap_or(who.grants.default)
}

/// A stored record as one principal may see it; `None` when the record is
/// withheld from it (retired, or restricted without the grant).
pub fn visible(
    who: &Principal,
    decl: &TypeDecl,
    stored: &StoredRecord,
    with_meta: bool,
) -> Option<Record> {
    if stored.retired || (stored.restricted && !who.grants.restricted) {
        return None;
    }
    let mut fields = BTreeMap::new();
    let mut masked = Vec::new();
    let mut meta = BTreeMap::new();
    for f in &decl.fields {
        let Some(value) = stored.fields.get(&f.name).filter(|v| !v.is_null()) else {
            // A computed field whose function failed holds no value; its
            // metadata says why, to whoever may read the field. An
            // embedding with nothing to embed keeps metadata without an
            // error, which says so.
            if with_meta
                && let Some(m) = stored
                    .meta
                    .get(&f.name)
                    .filter(|m| m.error.is_some() || f.embed.is_some())
                && field_access(who, &decl.name, f).reads()
            {
                meta.insert(f.name.clone(), m.clone());
            }
            continue;
        };
        match field_access(who, &decl.name, f) {
            a if a.reads() => {
                fields.insert(f.name.clone(), value.clone());
                if with_meta && let Some(m) = stored.meta.get(&f.name) {
                    meta.insert(f.name.clone(), m.clone());
                }
            }
            Access::Masked => masked.push(f.name.clone()),
            _ => {}
        }
    }
    Some(Record {
        ty: stored.ty.clone(),
        key: stored.key.clone(),
        version: stored.version,
        fields,
        masked,
        meta,
    })
}

/// Refuse a read that probes a field the principal may not read, or a
/// field the type does not declare. `probed` are the fields a filter,
/// an ordering or an aggregate touches.
pub fn check_probe<'a>(
    who: &Principal,
    decl: &TypeDecl,
    probed: impl IntoIterator<Item = &'a FieldName>,
) -> Result<(), Error> {
    for name in probed {
        let field = field_of(decl, name)?;
        if !field_access(who, &decl.name, field).reads() {
            return Err(refused(Some(name), Refusal::NotGranted));
        }
    }
    Ok(())
}

/// Refuse a search the principal may not make: a lexical search probes
/// every searchable field of the type at once, so each must be readable;
/// a vector search probes the vector field it names and, since a vector
/// embedded from text encodes that text, every field it is embedded from.
///
/// A lexical or hybrid search that names `text_fields` probes those
/// instead of every searchable field, in name order; the vector half of a
/// hybrid search is not narrowed. A named field must be declared, readable
/// and searchable, judged in that order: an unreadable field is refused as
/// not granted before its searchability is looked at, so the refusal never
/// tells a principal whether a field it may not read is searchable.
pub fn check_search(
    who: &Principal,
    decl: &TypeDecl,
    text_fields: Option<&[FieldName]>,
    vector_field: Option<&FieldName>,
) -> Result<(), Error> {
    let embedded_from = vector_field
        .and_then(|v| decl.fields.iter().find(|f| &f.name == v))
        .and_then(|f| f.embed.as_ref())
        .into_iter()
        .flat_map(|e| e.from.iter());
    let Some(named) = text_fields else {
        let searched = decl.fields.iter().filter(|f| f.searchable).map(|f| &f.name);
        return check_probe(who, decl, searched.chain(vector_field).chain(embedded_from));
    };
    if named.is_empty() {
        return Err(crate::invalid(
            "`fields` names no field; leave it out to search every searchable field",
        ));
    }
    let named: BTreeSet<&FieldName> = named.iter().collect();
    for name in named {
        check_probe(who, decl, [name])?;
        if !decl.fields.iter().any(|f| &f.name == name && f.searchable) {
            let message = format!("`{name}` of `{}` is not searchable", decl.name);
            return Err(refused(Some(name), Refusal::Invalid { message }));
        }
    }
    check_probe(who, decl, vector_field.into_iter().chain(embedded_from))
}

/// The door a write comes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Door {
    /// `upsert`, a verb, an accepted proposal.
    Authored,
    /// `ingest`.
    Source,
}

impl Door {
    /// Whether a field of this authority may be written through this door.
    pub fn admits(self, authority: Authority) -> bool {
        matches!(
            (self, authority),
            (_, Authority::Shared)
                | (Door::Authored, Authority::Authored)
                | (Door::Source, Authority::Source)
        )
    }
}

/// Refuse a write, whole, unless every field it carries exists, is written
/// through this door, and is granted to the principal. Fields are checked
/// in name order, so the refusal names the same field every time.
pub fn check_write(
    who: &Principal,
    decl: &TypeDecl,
    door: Door,
    patch: &BTreeMap<FieldName, Value>,
) -> Result<(), Error> {
    if door == Door::Source && !who.grants.ingest {
        return Err(refused(None, Refusal::NotGranted));
    }
    not_a_view(decl)?;
    for name in patch.keys() {
        let field = field_of(decl, name)?;
        if !door.admits(field.authority) {
            return Err(refused(Some(name), Refusal::WrongDoor));
        }
        if !who.grants.access(&decl.name, name).writes() {
            return Err(refused(Some(name), Refusal::NotGranted));
        }
    }
    Ok(())
}

/// Refuse a view at a door that writes or holds a record: its records are
/// computed, never written. Every such door says it in these words.
pub fn not_a_view(decl: &TypeDecl) -> Result<(), Error> {
    if decl.view.is_some() {
        return Err(crate::invalid(format!(
            "`{}` is a view; it is computed, never written",
            decl.name
        )));
    }
    Ok(())
}

fn refused(field: Option<&FieldName>, reason: Refusal) -> Error {
    let field = field.cloned();
    Error::Refused { field, reason }
}

/// The field `decl` declares as `name`, or a refusal naming it.
fn field_of<'d>(decl: &'d TypeDecl, name: &FieldName) -> Result<&'d FieldDecl, Error> {
    let message = || format!("`{}` has no field `{name}`", decl.name);
    let field = decl.fields.iter().find(|f| &f.name == name);
    field.ok_or_else(|| refused(Some(name), Refusal::Invalid { message: message() }))
}

#[cfg(test)]
mod tests;

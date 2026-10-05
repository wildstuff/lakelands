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
use lakelands_api::write::{Migration, MigrationReport};
use lakelands_api::{
    Access, Error, FieldName, Position, Principal, Record, RecordKey, Refusal, TypeName, Value,
};

/// The prefix every system type's name carries
/// (`docs/decisions/003-reservation-by-namespace.md`).
pub const SYSTEM_PREFIX: &str = "sys.";

/// Whether a type name is reserved for system types.
pub fn is_system(ty: &TypeName) -> bool {
    ty.as_str().starts_with(SYSTEM_PREFIX)
}

/// One principal's access to one declared field of `decl`: the grant,
/// read no further than every field it is computed or embedded from
/// (`depends_on`, `embed.from`, and theirs in turn), since such a field
/// hands out what it is built from
/// (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`).
/// A field that resolves to none does not exist for the principal. A
/// `sensitive` field nobody granted or denied by name is shown masked
/// rather than dropped; a `none` given by name, or handed down by a
/// source, hides it (`docs/decisions/006-grants-not-roles.md`).
pub fn field_access(who: &Principal, decl: &TypeDecl, field: &FieldDecl) -> Access {
    access_of(who, decl, field, &mut Vec::new())
}

fn access_of<'d>(
    who: &Principal,
    decl: &'d TypeDecl,
    field: &'d FieldDecl,
    seen: &mut Vec<&'d FieldName>,
) -> Access {
    let (access, from_source) = built_from(who, decl, field, seen);
    let named =
        (who.grants.types.get(&decl.name)).is_some_and(|t| t.fields.contains_key(&field.name));
    match access {
        Access::None if field.sensitive && !named && !from_source => Access::Masked,
        a => a,
    }
}

/// A field's grant, lowered by each field it is built from that the
/// principal may not read: to masked by a masked one, to none by any
/// other, and whether a source hid it. `seen` guards a cycle a
/// declaration could not hold anyway.
fn built_from<'d>(
    who: &Principal,
    decl: &'d TypeDecl,
    field: &'d FieldDecl,
    seen: &mut Vec<&'d FieldName>,
) -> (Access, bool) {
    let mut access = who.grants.access(&decl.name, &field.name);
    let mut from_source = false;
    seen.push(&field.name);
    let sources = field
        .depends_on
        .iter()
        .chain(field.embed.iter().flat_map(|e| &e.from));
    for name in sources {
        if seen.contains(&name) {
            continue;
        }
        let Some(source) = decl.fields.iter().find(|f| &f.name == name) else {
            continue;
        };
        access = match access_of(who, decl, source, seen) {
            s if s.reads() => access,
            // The field's value stays hidden; its write stays the
            // principal's, as the store's write check grants it.
            _ if access.writes() => Access::WriteOnly,
            Access::Masked if access.reads() => Access::Masked,
            Access::Masked => access,
            _ => {
                from_source = true;
                Access::None
            }
        };
    }
    seen.pop();
    (access, from_source)
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
                && field_access(who, decl, f).reads()
            {
                meta.insert(f.name.clone(), m.clone());
            }
            continue;
        };
        match field_access(who, decl, f) {
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
        match field_access(who, decl, field) {
            Access::None => return Err(no_field(decl, name)),
            a if !a.reads() => return Err(refused(Some(name), Refusal::NotGranted)),
            _ => {}
        }
    }
    Ok(())
}

/// Refuse a search the principal may not make: a lexical search probes
/// every searchable field of the type at once, so each must be readable;
/// a vector search probes each vector field it names, in the order named,
/// and, since a vector embedded from text encodes that text, before each
/// every field it is embedded from.
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
    vector_fields: &[&FieldName],
) -> Result<(), Error> {
    let vectors = vector_fields.iter().flat_map(|v| {
        let field = decl.fields.iter().find(|f| &f.name == *v);
        let embed = field.and_then(|f| f.embed.as_ref());
        embed.into_iter().flat_map(|e| e.from.iter()).chain([*v])
    });
    let Some(named) = text_fields else {
        return check_probe(who, decl, searched_fields(who, decl).chain(vectors));
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
    check_probe(who, decl, vectors)
}

/// The searchable fields a search that names none reads: those the
/// principal knows of. A field it may not know of is never searched, so
/// the search neither refuses nor names it; a masked one is still probed.
pub fn searched_fields<'d>(
    who: &Principal,
    decl: &'d TypeDecl,
) -> impl Iterator<Item = &'d FieldName> {
    (decl.fields.iter())
        .filter(move |f| f.searchable && field_access(who, decl, f) != Access::None)
        .map(|f| &f.name)
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
        if field_access(who, decl, field) == Access::None {
            return Err(no_field(decl, name));
        }
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
    let field = decl.fields.iter().find(|f| &f.name == name);
    field.ok_or_else(|| no_field(decl, name))
}

/// The answer for a field `decl` does not declare, which is also the
/// answer for one the principal may not know of.
fn no_field(decl: &TypeDecl, name: &FieldName) -> Error {
    let message = format!("`{}` has no field `{name}`", decl.name);
    refused(Some(name), Refusal::Invalid { message })
}

/// A migration that is not in force as `who` may know it: the shape it
/// leaves without a field `who` may not know of, judged with its grants
/// held through the migration's own renames, of fields and of the link
/// types its renamed relations move; and a rename only where
/// `who` may know of the field under both names, the old one in `was`,
/// the type as it stands.
pub fn known_migration(who: &Principal, was: &TypeDecl, m: &mut Migration) {
    // The migration's own renames, as the fold would record them.
    let list = crate::links::renamed(m, Position(u64::MAX));
    let (mut then, mut was) = (who.clone(), was.clone());
    (then.grants).follow(&list, None);
    known_fields(who, &mut was);
    known_fields(&then, &mut m.to);
    (m.renames).retain(|(old, new)| holds(&was, old) && holds(&m.to, new));
}

/// `t` without the fields `who` may not know of, a relation's link fields
/// too: a shape as `describe` would show it.
pub fn known_fields(who: &Principal, t: &mut TypeDecl) {
    let whole = t.clone();
    t.fields
        .retain(|f| field_access(who, &whole, f) != Access::None);
    for r in &mut t.relations {
        let link = crate::links::link_type(&whole.name, r);
        r.fields
            .retain(|f| field_access(who, &link, f) != Access::None);
    }
}

/// Whether a shape holds what a migration's report names by `name`: a
/// field, a link field (`relation.field`), or a relation, which a report
/// lists as dropped by its own name.
pub fn holds(t: &TypeDecl, name: &FieldName) -> bool {
    let relation = |r: &str| t.relations.iter().find(|x| x.name.as_str() == r);
    match name.as_str().split_once('.') {
        Some((r, f)) => relation(r).is_some_and(|x| x.fields.iter().any(|x| x.name.as_str() == f)),
        None => t.fields.iter().any(|f| &f.name == name) || relation(name.as_str()).is_some(),
    }
}

/// A migration's report as `who` may know it (013, 024 and 027, extended):
/// every place that answers one calls this. `was` is the type as it
/// stands, `sees` a type the reader may see, `answered` whether it is
/// answered a record. An entry is shown where the reader may see the type
/// and know of the field: a conversion in the shape the migration leaves,
/// and by the name it took its values from where it was renamed, as is a
/// rewritten reference of the migrated type itself; a dropped field in
/// the shape as it stands, and a renamed relation likewise; a sample key
/// where the record is answered; a declaration the migration carries
/// where `describe` names it, over `declared`, the declarations in force. The totals, the hashes and the id, which names the whole
/// report, stay; `more` says that something was left out.
pub fn shown_report<'a, D: std::borrow::Borrow<lakelands_api::describe::LakeDescription>>(
    who: &Principal,
    was: &TypeDecl,
    m: &Migration,
    report: &mut MigrationReport,
    described: impl FnOnce() -> D,
    sees: impl Fn(&TypeName) -> Option<&'a TypeDecl>,
    answered: impl Fn(&TypeName, &RecordKey) -> bool,
) {
    // One who may not see the migrated type is answered no name at all:
    // the totals, the hashes and the id, and that there is more.
    if sees(&m.to.name).is_none() {
        (report.ty, report.more) = (TypeName::default(), true);
        report.fields.clear();
        report.dropped.clear();
        report.relations.clear();
        report.carried.clear();
        report.rekeyed.clear();
        report.references.clear();
        report.sample_collisions.clear();
        return;
    }
    let whole = report.clone();
    let (mut was, mut to) = (was.clone(), m.to.clone());
    known_fields(who, &mut was);
    known_fields(who, &mut to);
    let sourced = |new: &FieldName| {
        let mut from = m.renames.iter().filter(|(_, n)| n == new);
        from.all(|(old, _)| holds(&was, old))
    };
    let known = |ty: &TypeName, field: &FieldName| {
        let found = sees(ty).and_then(|t| Some((t, t.fields.iter().find(|f| &f.name == field)?)));
        found.is_some_and(|(t, f)| field_access(who, t, f) != Access::None)
    };
    report
        .fields
        .retain(|c| holds(&to, &c.field) && sourced(&c.field));
    for c in &mut report.fields {
        // A link field's sample keys are of its link type's records.
        let of = match c.field.as_str().split_once('.') {
            Some((r, _)) => crate::links::link_type_name(&m.to.name, &r.into()),
            None => m.to.name.clone(),
        };
        c.sample_keys.retain(|key| answered(&of, key));
    }
    report.dropped.retain(|(f, _)| holds(&was, f));
    // A renamed relation is shown as a dropped one is: by the name it had.
    let named = |r: &lakelands_api::write::RelationRename| holds(&was, &r.from.as_str().into());
    report.relations.retain(named);
    // A declaration the migration carries is named as `describe` names it:
    // a trigger, a verb or a set to one who is described it, a view to one
    // who is described the view. The lake is described only for a report
    // that carries something.
    if !report.carried.is_empty() {
        let shown = described();
        let shown = shown.borrow();
        report.carried.retain(|c| match c.kind.as_str() {
            "trigger" => shown.triggers.iter().any(|t| t.name.as_str() == c.name),
            "verb" => shown.verbs.iter().any(|v| v.name.as_str() == c.name),
            "object_set" => shown.object_sets.iter().any(|s| s.name == c.name),
            _ => shown.types.iter().any(|t| t.name.as_str() == c.name),
        });
    }
    report.rekeyed.retain(|r| sees(&r.ty).is_some());
    report.references.retain(|r| match r.ty == m.to.name {
        true => holds(&to, &r.field) && sourced(&r.field),
        false => known(&r.ty, &r.field),
    });
    report.sample_collisions.retain(|c| sees(&c.ty).is_some());
    report.more = *report != whole;
}

#[cfg(test)]
mod tests;

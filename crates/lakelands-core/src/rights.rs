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

use lakelands_api::error::Case;
use lakelands_api::ontology::{Authority, FieldDecl, TypeDecl, VerbDecl, VerbEffect};
use lakelands_api::principal::TypeGrants;
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

/// What the grants give a field, before `sensitive` raises it: a type
/// is shown by this, and `sensitive` shows a field of a type that is
/// shown, never the type.
pub fn granted_access(who: &Principal, decl: &TypeDecl, field: &FieldDecl) -> Access {
    built_from(who, decl, field, &mut Vec::new()).0
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
    let measured = decl.measure_reads(&field.name);
    let sources = (field.depends_on.iter())
        .chain(field.embed.iter().flat_map(|e| &e.from))
        .chain(&measured);
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
            // principal's, as the store's write check grants it. A view's
            // fields are never written (lakelands#782).
            _ if access.writes() && decl.view.is_none() => Access::WriteOnly,
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

/// A stored record as a sight's principal may see it; `None` when the
/// record is retired, or held from it ([`Sight::withheld`]).
pub fn visible(
    sees: &crate::sight::Sight,
    decl: &TypeDecl,
    stored: &StoredRecord,
    with_meta: bool,
) -> Option<Record> {
    if stored.retired || sees.withheld(stored) {
        return None;
    }
    let who = sees.who();
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

/// The pairs `who`'s grant on `ty` admits records by: its `only`, empty
/// where it admits every record.
pub(crate) fn own_only(who: &Principal, ty: &TypeName) -> Vec<(FieldName, Value)> {
    let only = who.grants.types.get(ty).map(|t| &t.only);
    only.into_iter()
        .flatten()
        .map(|(f, v)| (f.clone(), v.clone()))
        .collect()
}

/// The one effective `only` of a type for `who`, and whether its records
/// are computed for it; [`admit`] is made of it, and every door asks
/// that. A stored type's is its own pairs. A view's are its own,
/// plus, where it has one stored source, no merge, and groups by every
/// field the source's pairs name, the source's pairs on its group fields:
/// each group's records then share those values, so the admitted records
/// are the groups that hold them. Otherwise a view that reads a type held
/// to some records, or a link type whose end type is, is computed from
/// the admitted records alone, and then held to the view's own pairs. A
/// kept name answers for its view. `find` answers a type by its name.
pub(crate) fn only<'a>(
    who: &Principal,
    ty: &TypeName,
    find: &dyn Fn(&TypeName) -> Option<&'a TypeDecl>,
) -> (Vec<(FieldName, Value)>, bool) {
    only_seen(who, ty, find, &mut Vec::new())
}

fn only_seen<'a>(
    who: &Principal,
    ty: &TypeName,
    find: &dyn Fn(&TypeName) -> Option<&'a TypeDecl>,
    seen: &mut Vec<TypeName>,
) -> (Vec<(FieldName, Value)>, bool) {
    let kept = ty.as_str().strip_prefix(lakelands_api::seam::KEPT_PREFIX);
    let ty = kept.map_or_else(|| ty.clone(), TypeName::from);
    let mut pairs = own_only(who, &ty);
    let Some(v) = find(&ty).and_then(|d| d.view.as_deref()) else {
        return (pairs, false);
    };
    if seen.contains(&ty) {
        return (pairs, true);
    }
    seen.push(ty);
    let sources = std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source));
    let held = |s: &TypeName, seen: &mut Vec<TypeName>| {
        let (p, computed) = only_seen(who, s, find, seen);
        let whole = p.is_empty() && !computed;
        (!whole || find(s).is_some_and(|d| ends_held(who, d))).then_some((p, computed))
    };
    let held: Vec<_> = sources.filter_map(|s| Some((s, held(s, seen)?))).collect();
    seen.pop();
    let Some((source, (p, computed))) = held.first() else {
        return (pairs, false);
    };
    let stored = find(source).is_some_and(|d| d.view.is_none() && !ends_held(who, d));
    let grouped = p.iter().all(|(f, _)| v.group_by.contains(f));
    let carried = !computed && v.merge.is_empty() && stored && grouped;
    pairs.extend(p.iter().filter(|_| carried).cloned());
    (pairs, !carried)
}

/// What a grant admits of a type's records for one principal (051): the
/// one answer every door reads, and it fails closed.
#[derive(Clone, Debug, PartialEq)]
pub enum Admit {
    /// Every record.
    All,
    /// The records whose fields hold every pair.
    Only(Vec<(FieldName, Value)>),
    /// A view's records, computed for this principal from the admitted
    /// records of what it reads and then held to these pairs.
    Computed(Vec<(FieldName, Value)>),
    /// No record: a grant that cannot be applied, whatever a record holds
    /// (lakelands#771).
    Nothing,
}

impl Admit {
    /// Whether a record holding `fields` is admitted.
    pub fn holds(&self, fields: &BTreeMap<FieldName, Value>) -> bool {
        self.pairs().is_some_and(|p| admitted(p, fields))
    }

    /// The pairs an admitted record holds, for an engine to filter by;
    /// none where no record is admitted.
    pub fn pairs(&self) -> Option<&[(FieldName, Value)]> {
        match self {
            Admit::All => Some(&[]),
            Admit::Only(p) | Admit::Computed(p) => Some(p),
            Admit::Nothing => None,
        }
    }
}

/// Whether the records of the view `ty` are computed for `who` from the
/// admitted records of what it reads ([`only`]), a grant that cannot be
/// applied included.
pub fn computed_only<'a>(
    who: &Principal,
    ty: &TypeName,
    find: &dyn Fn(&TypeName) -> Option<&'a TypeDecl>,
) -> bool {
    only(who, ty, find).1
}

/// What `who`'s grants admit of the records of `ty`, a kept name's as its
/// view's ([`only`]): nothing where the grant on the type, or on the
/// source a view reads, cannot be applied ([`check_only`]).
pub fn admit<'a>(
    who: &Principal,
    ty: &TypeName,
    find: &dyn Fn(&TypeName) -> Option<&'a TypeDecl>,
) -> Admit {
    if !holds_only(who) {
        return Admit::All;
    }
    let kept = ty.as_str().strip_prefix(lakelands_api::seam::KEPT_PREFIX);
    let decl = find(&kept.map_or_else(|| ty.clone(), TypeName::from));
    let source = decl.and_then(|d| find(&d.view.as_ref()?.source));
    if !decl
        .into_iter()
        .chain(source)
        .all(|d| check_only(who, d).is_ok())
    {
        return Admit::Nothing;
    }
    match only(who, ty, find) {
        (p, true) => Admit::Computed(p),
        (p, false) if p.is_empty() => Admit::All,
        (p, false) if decl.is_some() => Admit::Only(p),
        _ => Admit::Nothing,
    }
}

/// Whether a stored record holds every pair.
pub(crate) fn admitted(pairs: &[(FieldName, Value)], fields: &BTreeMap<FieldName, Value>) -> bool {
    pairs.iter().all(|(f, v)| fields.get(f) == Some(v))
}

/// The end types of a link type `decl` whose records `who` is admitted
/// only some of, with the link field that names each: a link record is
/// withheld unless both its ends are admitted.
pub(crate) fn held_ends<'d>(
    who: &Principal,
    decl: &'d TypeDecl,
) -> Vec<(&'d FieldName, &'d TypeName)> {
    if !crate::links::is_link_type(&decl.name) {
        return Vec::new();
    }
    let ends = decl
        .fields
        .iter()
        .filter(|f| f.name.as_str() == crate::links::FROM || f.name.as_str() == crate::links::TO);
    let ends = ends.filter_map(|f| Some((&f.name, f.ty.target.as_ref()?)));
    ends.filter(|(_, t)| !own_only(who, t).is_empty()).collect()
}

fn ends_held(who: &Principal, decl: &TypeDecl) -> bool {
    !held_ends(who, decl).is_empty()
}

/// Whether `who`'s grant admits only some records of `decl`, or of an end
/// type of a link type: its writes reach those records alone, and none
/// waits for review (051).
pub fn only_holder(who: &Principal, decl: &TypeDecl) -> bool {
    !own_only(who, &decl.name).is_empty() || ends_held(who, decl)
}

/// Whether a holder of `only` on `decl` may propose a write there, or a
/// call of `verb`: no field its `only` names is computed, and the verb
/// assigns none. Its accept applies the write as it was proposed, so
/// only such a write leaves the pairs where the proposer was judged to
/// leave them (051).
pub fn pinnable(who: &Principal, decl: &TypeDecl, verb: Option<&VerbDecl>) -> bool {
    let computed = |d: &FieldDecl| {
        d.authority == Authority::Derived || d.function.is_some() || d.embed.is_some()
    };
    let assigned = |f: &FieldName| match verb.map(|v| &v.effect) {
        Some(VerbEffect::Write { set }) => set.iter().any(|a| &a.field == f),
        _ => false,
    };
    own_only(who, &decl.name)
        .iter()
        .all(|(f, _)| !assigned(f) && !decl.fields.iter().any(|d| &d.name == f && computed(d)))
}

/// Whether `who` may read every field `verb` reads, each judged as a
/// filter on it is: a verb that reads one would answer its value, by
/// running or not, or copy it whole (024, extended).
pub fn reads_what_verb_reads(who: &Principal, decl: &TypeDecl, verb: &VerbDecl) -> bool {
    let reads = |n: &FieldName| {
        let field = decl.fields.iter().find(|f| &f.name == n);
        field.is_none_or(|f| field_access(who, decl, f).reads())
    };
    verb.fields_read()
        .is_some_and(|read| read.iter().all(reads))
}

/// Whether `who` can make a record of `decl` that its grant admits: it
/// writes every field its `only` names at the authored door (051).
pub fn creates(who: &Principal, decl: &TypeDecl) -> bool {
    own_only(who, &decl.name).iter().all(|(f, _)| {
        let field = decl.fields.iter().find(|d| &d.name == f);
        field.is_some_and(|d| {
            matches!(d.authority, Authority::Authored | Authority::Shared)
                && field_access(who, decl, d).writes()
        })
    })
}

/// Whether `who`'s grant admits only some records of any type: such a
/// principal changes no shape.
pub fn holds_only(who: &Principal) -> bool {
    who.grants.types.values().any(|t| !t.only.is_empty())
}

/// Refuse a read that names `decl` where `who`'s `only` on it cannot be
/// applied: a pair on a field the type does not declare, of a kind other
/// than `text` or `reference`, with a value that is not text, that the
/// holder may not read, or that is computed at the read, which an engine
/// does not hold to filter by. The refusal names the type, never the field, so a
/// field the holder may not know of is not told.
pub fn check_only(who: &Principal, decl: &TypeDecl) -> Result<(), Error> {
    use lakelands_api::ontology::StorageKind;
    let applies = |(f, v): &(FieldName, Value)| {
        let field = decl.fields.iter().find(|d| &d.name == f);
        field.is_some_and(|d| {
            matches!(d.ty.kind, StorageKind::Text | StorageKind::Reference)
                && v.is_string()
                && field_access(who, decl, d).reads()
                && !d.at_read()
        })
    };
    if own_only(who, &decl.name).iter().all(applies) {
        return Ok(());
    }
    let ty = decl.name.clone();
    Err(crate::invalid(Case::OnlyUnappliable { ty }))
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

/// Refuse a name of a field that `decl` does not declare, or one the
/// principal may not know of, alike. A masked field is not refused: a
/// projection that names it reads it as the record does.
pub fn check_known<'a>(
    who: &Principal,
    decl: &TypeDecl,
    named: impl IntoIterator<Item = &'a FieldName>,
) -> Result<(), Error> {
    for name in named {
        if field_access(who, decl, field_of(decl, name)?) == Access::None {
            return Err(no_field(decl, name));
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
        return Err(crate::invalid(Case::SearchFieldsEmpty));
    }
    let named: BTreeSet<&FieldName> = named.iter().collect();
    for name in named {
        check_probe(who, decl, [name])?;
        if !decl.fields.iter().any(|f| &f.name == name && f.searchable) {
            let (field, ty) = (name.clone(), decl.name.clone());
            let said = Box::new(Case::NotSearchable { field, ty }.into());
            return Err(refused(Some(name), Refusal::Invalid(said)));
        }
    }
    check_probe(who, decl, vectors)
}

/// The searchable fields a search that names none reads: those the
/// principal reads. A field it may not read is never searched, so the
/// search neither refuses, names nor ranks by it.
pub fn searched_fields<'d>(
    who: &Principal,
    decl: &'d TypeDecl,
) -> impl Iterator<Item = &'d FieldName> {
    (decl.fields.iter())
        .filter(move |f| f.searchable && field_access(who, decl, f).reads())
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
        let ty = decl.name.clone();
        return Err(crate::invalid(Case::IsAView { ty }));
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
    let (ty, field) = (decl.name.clone(), name.clone());
    let said = Box::new(Case::UnknownField { ty, field }.into());
    refused(Some(name), Refusal::Invalid(said))
}

/// A migration that is not in force as `who` may know it: the shape it
/// leaves as [`described_fields`] shows it, judged with its grants
/// held through the migration's own renames, of fields and of the link
/// types its renamed relations move; and a rename only where
/// `who` may know of the field under both names, the old one in `was`,
/// the type it was written against. A proposal that waited is judged by
/// the names it was written with: `who` is its reader with the grants
/// followed to the proposal's position, so a restriction on a name taken
/// since reaches the name it bore then.
pub fn known_migration<'a>(who: &Principal, was: &TypeDecl, m: &mut Migration, find: Find<'_, 'a>) {
    let (then, mut was) = (following(who, m), was.clone());
    known_fields(who, &mut was, find);
    described_fields(&then, &mut m.to, find);
    (m.renames).retain(|(old, new)| holds(&was, old) && holds(&m.to, new));
}

/// `who` with its grants held through a migration's own renames, as the
/// fold would record them.
fn following(who: &Principal, m: &Migration) -> Principal {
    let mut then = who.clone();
    (then.grants).follow(&crate::links::renamed(m, Position(u64::MAX)), None);
    then
}

/// The schema hash of `t` as `who` is shown it (024, extended): without
/// the fields it may not know of, the relations it is not shown and a
/// view it is not described; `whole`, the stored hash, where nothing is
/// left out.
pub fn shown_hash<'a>(
    who: &Principal,
    t: &TypeDecl,
    view_shown: bool,
    whole: &str,
    find: Find<'_, 'a>,
) -> String {
    let mut shown = t.clone();
    known_fields(who, &mut shown, find);
    if !view_shown {
        // Its key stays the one its records carry, which `describe` names.
        (shown.key, shown.view) = (shown.key_fields().to_vec(), None);
    }
    keep_known_measures(&mut shown);
    match shown == *t {
        true => whole.to_owned(),
        false => crate::shape::schema_hash(&shown).unwrap_or_default(),
    }
}

/// A report's hashes as `who` is shown them, set after [`shown_report`]
/// decided `more`: `from` of `made`, the type it was made from, `to` with
/// the grants held through the renames; whole to one not shown the type.
/// `find` is the lookup [`known_migration`] lists `to` with, so the `to`
/// hash is that of the listed `to`.
pub fn shown_hashes<'a>(
    who: &Principal,
    made: &TypeDecl,
    m: &Migration,
    report: &mut MigrationReport,
    find: Find<'_, 'a>,
) {
    if report.ty != m.to.name {
        return;
    }
    report.from_schema_hash = shown_hash(who, made, true, &report.from_schema_hash, find);
    let then = following(who, m);
    report.to_schema_hash = shown_hash(&then, &m.to, true, &report.to_schema_hash, find);
}

/// `t` as `describe` would show its fields to `who`, a relation's link
/// fields too (024): without those it may not know of and the relations
/// it is not shown, and a field it may not read left with its shape
/// alone. A listing hands a declaration out so; an author's view of a
/// type is made by [`known_fields`], and holds a field as declared.
pub fn described_fields<'a>(who: &Principal, t: &mut TypeDecl, find: Find<'_, 'a>) {
    let whole = t.clone();
    (t.relations).retain(|r| crate::describe::relation_shown(&whole, r, find, who));
    let shown = |of: &TypeDecl, fields: &mut Vec<FieldDecl>| {
        fields.retain(|f| field_access(who, of, f) != Access::None);
        let unread = fields
            .iter_mut()
            .filter(|f| !field_access(who, of, f).reads());
        unread.for_each(FieldDecl::shape_alone);
    };
    shown(&whole, &mut t.fields);
    for r in &mut t.relations {
        shown(&crate::links::link_type(&whole.name, r), &mut r.fields);
    }
    // A valid time over a field it may not know of is not named.
    let held = |v: &FieldName| t.fields.iter().any(|f| &f.name == v);
    (t.valid_from, t.valid_to) = (
        t.valid_from.take().filter(held),
        t.valid_to.take().filter(held),
    );
    // The key its records carry, as `describe` names it (lakelands#782).
    t.key = t.key_fields().to_vec();
    keep_known_measures(t);
}

/// `t`'s view without the measures that name a field `t` no longer holds.
fn keep_known_measures(t: &mut TypeDecl) {
    if let Some(v) = &mut t.view {
        v.keep_measures(|n| t.fields.iter().any(|f| &f.name == n));
    }
}

/// Whether `t` declares `name`, a field or a link field
/// (`relation.field`), as one `who` may not read.
fn unread(who: &Principal, t: &TypeDecl, name: &FieldName) -> bool {
    let link = |(r, f): (&str, _)| {
        let r = t.relations.iter().find(|x| x.name.as_str() == r)?;
        Some((crate::links::link_type(&t.name, r), f))
    };
    let (of, field) = match name.as_str().split_once('.') {
        Some(named) => match link(named) {
            Some((of, field)) => (std::borrow::Cow::Owned(of), field),
            None => return false,
        },
        None => (std::borrow::Cow::Borrowed(t), name.as_str()),
    };
    let field = of.fields.iter().find(|f| f.name.as_str() == field);
    field.is_some_and(|f| !field_access(who, &of, f).reads())
}

/// `t` without the fields `who` may not know of, a relation's link fields
/// too, and without the relations it is not shown. `find` answers a type
/// by its name.
pub fn known_fields<'a>(who: &Principal, t: &mut TypeDecl, find: Find<'_, 'a>) {
    let whole = t.clone();
    (t.relations).retain(|r| crate::describe::relation_shown(&whole, r, find, who));
    t.fields
        .retain(|f| field_access(who, &whole, f) != Access::None);
    for r in &mut t.relations {
        let link = crate::links::link_type(&whole.name, r);
        r.fields
            .retain(|f| field_access(who, &link, f) != Access::None);
    }
}

/// A type by its name, as the caller's ontology answers it.
pub type Find<'f, 'a> = &'f dyn Fn(&TypeName) -> Option<&'a TypeDecl>;

/// `who` with each reference to a type it is not shown held at `none`, a
/// type's own fields and its relations' link fields alike, whatever the
/// grant gave them (024 and 051, extended): a type kept from a principal
/// is not learned through what names it. To a fixed point, since a field
/// so hidden can leave another type unshown (006, extended). A field the
/// effective `only` of its type names stays as granted, its value the one
/// the host gave. `force` are the declarations in force. Answers the
/// fields hidden, in order. A sight is made by it, and nothing else asks.
pub(crate) fn hide_unshown_references(
    who: &mut Principal,
    force: &dyn crate::sight::InForce,
) -> Vec<(TypeName, FieldName)> {
    let mut hidden = Vec::new();
    if !hides_some(who) {
        return hidden;
    }
    let (types, find) = (force.types(), &|n: &TypeName| force.ty(n));
    loop {
        let unshown =
            |ty: &TypeName| find(ty).is_some_and(|d| !crate::describe::type_shown(d, find, who));
        let mut more = Vec::new();
        for t in &types {
            let (only, _) = only(who, &t.name, find);
            let links = (t.relations.iter()).map(|r| {
                (
                    crate::links::link_type_name(&t.name, &r.name),
                    &r.fields[..],
                    false,
                )
            });
            for (ty, fields, own) in
                std::iter::once((t.name.clone(), &t.fields[..], true)).chain(links)
            {
                let held = |f: &FieldName| {
                    let none = who.grants.types.get(&ty).and_then(|g| g.fields.get(f));
                    none == Some(&Access::None) || own && only.iter().any(|(n, _)| n == f)
                };
                let refers = fields
                    .iter()
                    .filter(|f| f.ty.target.as_ref().is_some_and(unshown));
                more.extend(
                    refers
                        .filter(|f| !held(&f.name))
                        .map(|f| (ty.clone(), f.name.clone())),
                );
            }
        }
        if more.is_empty() {
            return hidden;
        }
        for (ty, f) in &more {
            let grants = who.grants.types.entry(ty.clone()).or_default();
            grants.fields.insert(f.clone(), Access::None);
        }
        hidden.extend(more);
    }
}

/// Whether `who` may not know of some field: only a `none` its grants
/// give, to the lake, a type or a field, hides one.
pub fn hides_some(who: &Principal) -> bool {
    let (g, none) = (&who.grants, |a: &Access| *a == Access::None);
    let typed =
        |t: &TypeGrants| t.default.as_ref().is_some_and(none) || t.fields.values().any(none);
    none(&g.default) || g.types.values().any(typed)
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
/// every place that answers one calls this. `was` is the type the report
/// was made against: as it stands, or, for a listed proposal, as it stood
/// when proposed, read with `who`'s grants then. `sees` is a type the
/// reader may see, `answered` whether it is
/// answered a record. An entry is shown where the reader may see the type
/// and know of the field: a conversion in the shape the migration leaves,
/// and by the name it took its values from where it was renamed, as is a
/// rewritten reference of the migrated type itself, and a member restated,
/// with what was declared and what `to` states, as `describe` shows a
/// field member by member: one of [`FieldDecl::UNREAD`] only where the
/// reader reads the field in `to` and as the type stands, under the name
/// it had, since a field's access is built from what it is computed from
/// and a migration may change that with no rename (a link field bears
/// the name it had: a relation renamed keeps its link fields); a dropped
/// field in the shape as it stands, and a renamed relation likewise; a sample key
/// where the record is answered; a declaration the migration carries
/// where `described`, the lake as `describe` shows it to the reader,
/// names it, asked only for a report that carries one. The totals stay;
/// `more` says that something was left out. The hashes are
/// [`shown_hashes`]'s, and the id is made again over what is shown. The
/// reader is shown the migrated type: one that is not is answered no
/// migration at all, by every door that would answer a report
/// (lakelands#762).
pub fn shown_report<'a, D: std::borrow::Borrow<lakelands_api::describe::LakeDescription>>(
    who: &Principal,
    was: &TypeDecl,
    m: &Migration,
    report: &mut MigrationReport,
    described: impl FnOnce() -> D,
    sees: impl Fn(&TypeName) -> Option<&'a TypeDecl>,
    answered: impl Fn(&TypeName, &RecordKey) -> bool,
) {
    let whole = report.clone();
    let (stands, mut was, mut to) = (was, was.clone(), m.to.clone());
    known_fields(who, &mut was, &sees);
    known_fields(who, &mut to, &sees);
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
    // A member restated is the type's, or shown as its field's conversion
    // is, and one beyond the shape where the field is read in both shapes.
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

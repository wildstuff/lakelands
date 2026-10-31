//! Grants applied: what one principal may see of a record, what a read may
//! probe, and what a write may carry
//! (`docs/decisions/006-grants-not-roles.md`).
//!
//! Three rules, each enforced here and nowhere else:
//!
//! - **Reading.** A readable field is returned in full. An unreadable one
//!   is absent, or listed as masked if declared `sensitive` or granted
//!   `masked`. A restricted record is withheld from a principal without
//!   the `restricted` grant; a retired record from everyone.
//! - **Probing.** A filter, ordering or aggregate on a field learns its
//!   values even if the answer masks it ("is any salary above X", a sum
//!   over one record). A read that probes a field the principal may not
//!   read is refused outright, never answered with masking.
//! - **Writing.** A write names its door: authored (`upsert`, a verb) or
//!   source (`ingest`). Every field it carries must exist, be written by
//!   that door's authority and be granted; one failing field refuses the
//!   whole write
//!   (`docs/decisions/012-writes-touch-only-their-fields.md`).
//!
//! The module is the crate's own: a door asks these rules of its principal's
//! [`Sight`], and the `rights-private` gate keeps it so.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use lakelands_api::error::Case;
use lakelands_api::ontology::{Authority, FieldDecl, RetiredBy, TypeDecl, VerbDecl, VerbEffect};
use lakelands_api::principal::{HiddenWhere, TypeGrants};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::{Migration, MigrationReport};
use lakelands_api::{
    Access, Error, FieldName, Origin, Position, Principal, Record, Refusal, SourceId, TypeName,
    Value,
};

use crate::sight::Sight;

/// The prefix every system type's name carries
/// (`docs/decisions/003-reservation-by-namespace.md`).
const SYSTEM_PREFIX: &str = "sys.";

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
pub(crate) fn field_access(who: &Principal, decl: &TypeDecl, field: &FieldDecl) -> Access {
    settled(who, decl, field).1
}

/// What the grants give a field, before `sensitive` raises it: a type
/// is shown by this, and `sensitive` shows a field of a type that is
/// shown, never the type.
pub(crate) fn granted_access(who: &Principal, decl: &TypeDecl, field: &FieldDecl) -> Access {
    settled(who, decl, field).0
}

/// What one walk through the fields a field is built from has asked: a
/// field's access once answered, none while the walk stands on it, so a
/// shared source is asked once (lakelands#926). `before` holds the last
/// walk's answers, `met` whether this one came back to a field it stands on.
#[derive(Default)]
struct Asked<'d> {
    now: BTreeMap<&'d FieldName, Option<Access>>,
    before: BTreeMap<&'d FieldName, Access>,
    met: bool,
}

/// What the grants give `field`, and what it is shown as. Fields may list
/// each other in a cycle: a field the walk stands on answers as the walk
/// before left it, and the walk is repeated until no answer moves. So a
/// field has one answer however the walk comes round: the widest under
/// which each is read no further than what it is built from.
fn settled<'d>(who: &Principal, decl: &'d TypeDecl, field: &'d FieldDecl) -> (Access, Access) {
    let mut asked = Asked::default();
    loop {
        let built = built_from(who, decl, field, &mut asked);
        let shown = shown_as(who, decl, field, built);
        if !asked.met {
            return (built.0, shown);
        }
        // Only the field asked for is still stood on.
        let now = (asked.now.iter()).map(|(f, a)| (*f, a.unwrap_or(shown)));
        let now: BTreeMap<_, _> = now.collect();
        if now == asked.before {
            return (built.0, shown);
        }
        asked = Asked {
            before: now,
            ..Asked::default()
        };
    }
}

fn access_of<'d>(
    who: &Principal,
    decl: &'d TypeDecl,
    field: &'d FieldDecl,
    asked: &mut Asked<'d>,
) -> Access {
    let built = built_from(who, decl, field, asked);
    shown_as(who, decl, field, built)
}

/// A field's access as its sources left it, a `sensitive` one nobody
/// named and no source hid raised to masked.
fn shown_as(
    who: &Principal,
    decl: &TypeDecl,
    field: &FieldDecl,
    (access, from_source): (Access, bool),
) -> Access {
    let named =
        (who.grants.types.get(&decl.name)).is_some_and(|t| t.fields.contains_key(&field.name));
    match access {
        Access::None if field.sensitive && !named && !from_source => Access::Masked,
        a => a,
    }
}

/// A field's grant, lowered by each field it is built from that the
/// principal may not read: to masked by a masked one, to none by any
/// other, and whether a source hid it. A source the walk stands on is not
/// asked again ([`settled`]).
fn built_from<'d>(
    who: &Principal,
    decl: &'d TypeDecl,
    field: &'d FieldDecl,
    asked: &mut Asked<'d>,
) -> (Access, bool) {
    let mut access = who.grants.access(&decl.name, &field.name);
    let mut from_source = false;
    asked.now.insert(&field.name, None);
    let measured = decl.measure_reads(&field.name);
    let sources = (field.depends_on.iter())
        .chain(field.embed.iter().flat_map(|e| &e.from))
        .chain(&measured);
    for name in sources {
        let Some(source) = decl.field(name) else {
            continue;
        };
        let of_source = match asked.now.get(&source.name) {
            Some(Some(answered)) => *answered,
            Some(None) => {
                asked.met = true;
                match asked.before.get(&source.name) {
                    Some(left) => *left,
                    None => continue,
                }
            }
            None => {
                let answered = access_of(who, decl, source, asked);
                asked.now.insert(&source.name, Some(answered));
                answered
            }
        };
        access = lowered(decl, access, of_source, &mut from_source);
    }
    // One paired at the read is built from the fields of the other type it
    // pairs by and reads, as that type's grant resolves them (052, extended).
    for (_, e) in decl
        .in_force()
        .into_iter()
        .filter(|(f, _)| f.name == field.name)
    {
        for p in e.pairings().unwrap_or_default() {
            let ty = TypeName::from(p.ty.as_str());
            for name in p.theirs() {
                let of = who.grants.access(&ty, name);
                access = lowered(decl, access, of, &mut from_source);
            }
        }
    }
    (access, from_source)
}

/// `access` lowered by a field it is built from, of access `of_source`.
fn lowered(decl: &TypeDecl, access: Access, of_source: Access, from_source: &mut bool) -> Access {
    match of_source {
        s if s.reads() => access,
        // The field's value stays hidden; its write stays the
        // principal's, as the store's write check grants it. A view's
        // fields are never written (lakelands#782).
        _ if access.writes() && decl.view.is_none() => Access::WriteOnly,
        Access::Masked if access.reads() => Access::Masked,
        Access::Masked => access,
        _ => {
            *from_source = true;
            Access::None
        }
    }
}

/// The principal's access to a type as a whole: its type-level grant, or
/// the lake-wide default.
pub(crate) fn type_access(who: &Principal, ty: &TypeName) -> Access {
    who.grants
        .types
        .get(ty)
        .and_then(|t| t.default)
        .unwrap_or(who.grants.default)
}

impl Sight<'_> {
    /// Whether its principal may retire records of `decl`: every field is
    /// writable as its grants resolve it, whatever its authority, since
    /// retiring removes them all. A type with no field falls back to its
    /// type-level grant. A view's records are computed, never retired.
    pub fn may_retire(&self, decl: &TypeDecl) -> bool {
        let who = self.who();
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

    /// `stored` as its principal may see it; `None` when the record is
    /// retired, or held from it ([`Sight::withheld`]), or a setting it is
    /// not told ([`Sight::told`]).
    pub fn visible(
        &self,
        decl: &TypeDecl,
        stored: &StoredRecord,
        with_meta: bool,
    ) -> Option<Record> {
        if stored.retired || self.withheld(stored) {
            return None;
        }
        // A field a hiding lowers on the record is answered as its access
        // there says, masked whether or not it holds a value (006).
        let lowered = self.lowered(&decl.name, &stored.fields);
        let stored = self.told(stored)?;
        let who = self.who();
        let access = |f: &FieldDecl| match lowered.get(&f.name) {
            Some(a) => *a,
            None => field_access(who, decl, f),
        };
        let mut fields = BTreeMap::new();
        let mut masked = Vec::new();
        let mut meta = BTreeMap::new();
        for f in &decl.fields {
            let Some(value) = stored.fields.get(&f.name).filter(|v| !v.is_null()) else {
                // A computed field whose function failed holds no value; its
                // metadata says why, to whoever may read the field. An
                // embedding with nothing to embed keeps metadata without an
                // error, which says so, and an erased value says it was
                // written (058).
                if with_meta
                    && let Some(m) = stored
                        .meta
                        .get(&f.name)
                        .filter(|m| m.error.is_some() || m.erased || f.embed.is_some())
                    && access(f).reads()
                {
                    meta.insert(f.name.clone(), m.clone());
                }
                if lowered.get(&f.name) == Some(&Access::Masked) {
                    masked.push(f.name.clone());
                }
                continue;
            };
            match access(f) {
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
}

/// The pairs `who`'s grant on `ty` admits records by: its `only`, and a
/// pair for each set of its `only_any`, its value the set's values as an
/// array, which a record holds by holding one of them; empty where it
/// admits every record. An array given in `only`, and a set on a field
/// `only` names too, is null, a pair that cannot be applied.
pub(crate) fn own_only(who: &Principal, ty: &TypeName) -> Vec<(FieldName, Value)> {
    let Some(t) = who.grants.types.get(ty) else {
        return Vec::new();
    };
    let any = t
        .only_any
        .iter()
        .map(|(f, set)| match t.only.contains_key(f) {
            true => (f.clone(), Value::Null),
            false => (f.clone(), set.iter().cloned().map(Value::String).collect()),
        });
    one_each(&t.only).chain(any).collect()
}

/// The pairs `who`'s grant on `ty` admits records to write by: its
/// `writes_only`, empty where it may write every record it reads.
pub(crate) fn own_writes_only(who: &Principal, ty: &TypeName) -> Vec<(FieldName, Value)> {
    let pairs = who.grants.types.get(ty).map(|t| one_each(&t.writes_only));
    pairs.into_iter().flatten().collect()
}

/// The pairs of a grant that names one value a field: an array, which
/// would read as a set, is null.
fn one_each(pairs: &BTreeMap<FieldName, Value>) -> impl Iterator<Item = (FieldName, Value)> + '_ {
    let one = |v: &Value| if v.is_array() { Value::Null } else { v.clone() };
    pairs.iter().map(move |(f, v)| (f.clone(), one(v)))
}

/// The one effective `only` of a type for `who`, and whether its records are
/// computed for it; [`Sight::admit`] is made of it. A stored type's is its own
/// pairs. A view's are its own plus, where it has one stored source, no merge,
/// and groups by every field the source's pairs name, the source's pairs: each
/// group's records share those values, so the admitted records are the groups
/// that hold them. Otherwise a view that reads a type held to some records, or
/// a link type whose end type is, is computed from the admitted records alone,
/// then held to the view's own pairs. A kept name answers for its view. `find`
/// answers a type by its name.
pub(crate) fn only<'a>(
    who: &Principal,
    ty: &TypeName,
    find: &dyn Fn(&TypeName) -> Option<&'a TypeDecl>,
) -> (Vec<(FieldName, Value)>, bool) {
    only_seen(who, ty, find, &mut Vec::new(), &mut BTreeMap::new())
}

/// What [`only`] answers of a type: its pairs, and whether it is computed.
type OnlyAnswer = (Vec<(FieldName, Value)>, bool);

/// Pairs of a field and the value a record must hold there.
pub type Pairs = Vec<(FieldName, Value)>;

/// [`only`] of `ty`, `seen` the views on the walk's path and `known` what
/// each view walked before answered, so a shared view is asked once a call
/// (lakelands#942). The answer is the same from any path: a view on a cycle
/// of views, which the read refuses, answers its own pairs, computed.
fn only_seen<'a>(
    who: &Principal,
    ty: &TypeName,
    find: &dyn Fn(&TypeName) -> Option<&'a TypeDecl>,
    seen: &mut Vec<TypeName>,
    known: &mut BTreeMap<TypeName, OnlyAnswer>,
) -> OnlyAnswer {
    let kept = ty.as_str().strip_prefix(lakelands_api::seam::KEPT_PREFIX);
    let ty = kept.map_or_else(|| ty.clone(), TypeName::from);
    let mut pairs = own_only(who, &ty);
    let Some(v) = find(&ty).and_then(|d| d.view.as_deref()) else {
        return (pairs, false);
    };
    if seen.contains(&ty) {
        return (pairs, true);
    }
    if let Some(answer) = known.get(&ty) {
        return answer.clone();
    }
    seen.push(ty.clone());
    let sources = std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source));
    let ends_held = |d: &TypeDecl| !held_ends(who, d, find).is_empty();
    let mut held = Vec::new();
    for s in sources {
        let (p, computed) = only_seen(who, s, find, seen, known);
        let whole = p.is_empty() && !computed;
        if !whole || find(s).is_some_and(ends_held) {
            held.push((s, (p, computed)));
        }
    }
    seen.pop();
    let answer = match held.first() {
        None => (pairs, false),
        Some((source, (p, computed))) => {
            let stored = find(source).is_some_and(|d| d.view.is_none() && !ends_held(d));
            let grouped = p.iter().all(|(f, _)| v.group_by.contains(f));
            let carried = !computed && v.merge.is_empty() && stored && grouped;
            pairs.extend(p.iter().filter(|_| carried).cloned());
            (pairs, !carried)
        }
    };
    known.insert(ty, answer.clone());
    answer
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

    /// The pairs an admitted record holds, a set's as an array of which
    /// it holds one; none where no record is admitted.
    pub fn pairs(&self) -> Option<&[(FieldName, Value)]> {
        match self {
            Admit::All => Some(&[]),
            Admit::Only(p) | Admit::Computed(p) => Some(p),
            Admit::Nothing => None,
        }
    }

    /// Whether every record the pairs `only` select is one this admits:
    /// each pair of this is named in `only` by a value it holds, or by a
    /// set of values it holds every one of.
    pub fn within(&self, only: &BTreeMap<FieldName, Value>) -> bool {
        let named = |(f, v): &(FieldName, Value)| only.get(f).is_some_and(|o| Self::inside(o, v));
        self.pairs().is_some_and(|p| p.iter().all(named))
    }

    /// Whether `selected`, a value or a set of values a selection names,
    /// holds only values the pair value `held` admits.
    pub fn inside(selected: &Value, held: &Value) -> bool {
        match selected.as_array() {
            Some(set) => set.iter().all(|s| holds_value(Some(s), held)),
            None => holds_value(Some(selected), held),
        }
    }

    /// The pairs as an engine's scan selects by them: `equal`, each pair
    /// of one value, and `equal_any`, a tuple for each choice of one value
    /// from every set; none where no record is admitted.
    pub fn scanned(&self) -> Option<(Pairs, Vec<Pairs>)> {
        let (sets, equal): (Vec<_>, Vec<_>) = self.pairs()?.iter().partition(|p| p.1.is_array());
        let mut any: Vec<Pairs> = Vec::new();
        for (f, values) in sets.iter().filter_map(|(f, v)| Some((f, v.as_array()?))) {
            if values.is_empty() {
                return None;
            }
            let had = std::mem::take(&mut any);
            for t in if had.is_empty() {
                vec![Vec::new()]
            } else {
                had
            } {
                any.extend(
                    values
                        .iter()
                        .map(|v| [&t[..], &[(f.clone(), v.clone())]].concat()),
                );
            }
        }
        Some((equal.into_iter().cloned().collect(), any))
    }
}

impl Sight<'_> {
    /// Whether the records of the view `ty` are computed for its principal
    /// from the admitted records of what it reads (`only`), a grant that
    /// cannot be applied included. `find` is the lookup the caller reads
    /// types with, not the sight's own, so a read judges the shape it reads.
    pub fn computed_only(&self, ty: &TypeName, find: Find<'_, '_>) -> bool {
        only(self.who(), ty, find).1
    }

    /// What its principal's grants admit of the records of `ty`, a kept
    /// name's as its view's (`only`): nothing where the grant on the type,
    /// or on the source a view reads, cannot be applied
    /// ([`Self::check_only`]) (051).
    pub fn admit(&self, ty: &TypeName) -> Admit {
        if !self.holds_only() {
            return Admit::All;
        }
        let find = &|n: &TypeName| self.force.ty(n);
        let kept = ty.as_str().strip_prefix(lakelands_api::seam::KEPT_PREFIX);
        let decl = find(&kept.map_or_else(|| ty.clone(), TypeName::from));
        let source = decl.and_then(|d| find(&d.view.as_ref()?.source));
        if !decl
            .into_iter()
            .chain(source)
            .all(|d| self.check_only(d).is_ok())
        {
            return Admit::Nothing;
        }
        match only(self.who(), ty, find) {
            // A set that lists no value admits no record.
            (p, _)
                if p.iter()
                    .any(|(_, v)| v.as_array().is_some_and(Vec::is_empty)) =>
            {
                Admit::Nothing
            }
            (p, true) => Admit::Computed(p),
            (p, false) if p.is_empty() => Admit::All,
            (p, false) if decl.is_some() => Admit::Only(p),
            _ => Admit::Nothing,
        }
    }
}

/// Whether a stored record holds every pair, one of a set's values for a
/// set ([`own_only`]).
pub(crate) fn admitted(pairs: &[(FieldName, Value)], fields: &BTreeMap<FieldName, Value>) -> bool {
    pairs.iter().all(|(f, v)| holds_value(fields.get(f), v))
}

/// Whether a field holding `held` holds a pair's value: the value, or, of a
/// set, one of its texts.
pub(crate) fn holds_value(held: Option<&Value>, v: &Value) -> bool {
    match v {
        Value::Array(set) => held.is_some_and(|h| h.is_string() && set.contains(h)),
        v => held == Some(v),
    }
}

/// The end types of a link type `decl` whose records `who` is admitted
/// only some of, with the link field that names each: a link record is
/// withheld unless both its ends are admitted. An end that is a link is
/// held where an end of its own is, as deep as they go, each link type
/// asked once: a migration can make two each other's end.
pub(crate) fn held_ends<'d, 'a>(
    who: &Principal,
    decl: &'d TypeDecl,
    find: &dyn Fn(&TypeName) -> Option<&'a TypeDecl>,
) -> Vec<(&'d FieldName, &'d TypeName)> {
    ends_held_by(who, decl, find, own_only)
}

/// [`held_ends`] of `writes_only`: the end types of a link type whose
/// records `who` may write only some of, as deep as they go.
pub(crate) fn write_held_ends<'d, 'a>(
    who: &Principal,
    decl: &'d TypeDecl,
    find: &dyn Fn(&TypeName) -> Option<&'a TypeDecl>,
) -> Vec<(&'d FieldName, &'d TypeName)> {
    ends_held_by(who, decl, find, own_writes_only)
}

/// The end types of `decl` that hold pairs of `own`, or whose own ends do.
fn ends_held_by<'d, 'a>(
    who: &Principal,
    decl: &'d TypeDecl,
    find: &dyn Fn(&TypeName) -> Option<&'a TypeDecl>,
    own: fn(&Principal, &TypeName) -> Vec<(FieldName, Value)>,
) -> Vec<(&'d FieldName, &'d TypeName)> {
    if !crate::links::is_link_type(&decl.name) {
        return Vec::new();
    }
    let ends = decl
        .fields
        .iter()
        .filter(|f| f.name.as_str() == crate::links::FROM || f.name.as_str() == crate::links::TO);
    let ends = ends.filter_map(|f| Some((&f.name, f.ty.target.as_ref()?)));
    let find = |n: &TypeName| find(n).filter(|d| d.name != decl.name);
    let held = |t: &TypeName| find(t).is_some_and(|d| !ends_held_by(who, d, &find, own).is_empty());
    ends.filter(|(_, t)| !own(who, t).is_empty() || held(t))
        .collect()
}

impl Sight<'_> {
    /// Whether its principal's grant admits only some records of `decl`, or
    /// of an end type of a link type, as deep as its ends go
    /// (`held_ends`): its writes reach those records alone, and none
    /// waits for review (051).
    pub fn only_holder(&self, decl: &TypeDecl) -> bool {
        let (who, find) = (self.who(), &|n: &TypeName| self.force.ty(n));
        !own_only(who, &decl.name).is_empty() || !held_ends(who, decl, find).is_empty()
    }

    /// Whether its principal, holding `only` or `writes_only` on `decl`, may
    /// propose a write there, or a call of `verb`: no field their pairs
    /// name is computed, and the verb assigns none. Its accept applies the write as it was
    /// proposed, so only such a write leaves the pairs where the proposer
    /// was judged to leave them (051).
    pub fn pinnable(&self, decl: &TypeDecl, verb: Option<&VerbDecl>) -> bool {
        let computed = |d: &FieldDecl| {
            d.authority == Authority::Derived || d.function.is_some() || d.embed.is_some()
        };
        let assigned = |f: &FieldName| match verb.map(|v| &v.effect) {
            Some(VerbEffect::Write { set }) => set.iter().any(|a| &a.field == f),
            _ => false,
        };
        let who = self.who();
        let pairs = own_only(who, &decl.name).into_iter();
        pairs
            .chain(own_writes_only(who, &decl.name))
            .all(|(f, _)| !assigned(&f) && !decl.fields.iter().any(|d| d.name == f && computed(d)))
    }

    /// Whether its principal holds the grant `verb` asks of its own: none
    /// for a verb not declared `granted`, else its name among its verbs on
    /// its type. No other grant stands in for it, the owner's included
    /// (070).
    pub fn verb_granted(&self, verb: &VerbDecl) -> bool {
        let types = &self.who().grants.types;
        let named = || {
            types
                .get(&verb.on)
                .is_some_and(|t| t.verbs.contains(&verb.name))
        };
        !verb.granted || named()
    }

    /// Whether its principal may read every field `verb` reads, each judged
    /// as a filter on it is: a verb that reads one would answer its value,
    /// by running or not, or copy it whole (024, extended). So is each
    /// field it reads of another record, across a reference, on the type
    /// `ty` answers. A path through a relation's link is its principal's
    /// only where nothing of the link type and its end types is held from
    /// it: it holds `restricted`, no `only` there, and is shown the
    /// relation (055, extended). So is one that begins at a derived
    /// relation, of its target and, for a link-type target, its ends: no
    /// record it matches can be one the caller is not answered.
    pub fn reads_what_verb_reads<'d>(
        &self,
        decl: &'d TypeDecl,
        verb: &VerbDecl,
        ty: Find<'_, 'd>,
    ) -> bool {
        let who = self.who();
        let reads = |n: &FieldName| {
            let field = decl.field(n);
            field.is_none_or(|f| field_access(who, decl, f).reads())
        };
        let far = verb.reads_across(decl, ty);
        let free = |p: &Vec<FieldName>| {
            let field = decl.has_field(&p[0]);
            let r = decl
                .relations
                .iter()
                .find(|r| !field && r.name.as_str() == p[0].as_str());
            // A derived relation's far side is its target: nothing of the
            // target or the verb's own type is held from the caller (044 and
            // 055, extended).
            if let Some(r) = r.filter(|r| r.derived.is_some()) {
                // A target that is a link type is held by its ends too (035).
                let ends = ty(&r.target).is_some_and(|t| !held_ends(who, t, ty).is_empty());
                let held = ends
                    || !own_only(who, &r.target).is_empty()
                    || !own_only(who, &decl.name).is_empty();
                return who.grants.restricted
                    && !held
                    && crate::describe::relation_shown(decl, r, ty, who);
            }
            let lt = r.and_then(|r| ty(&crate::links::link_type_name(&decl.name, &r.name)));
            r.zip(lt).is_none_or(|(r, lt)| {
                let held =
                    !own_only(who, &lt.name).is_empty() || !held_ends(who, lt, ty).is_empty();
                who.grants.restricted && !held && crate::describe::relation_shown(decl, r, ty, who)
            })
        };
        let paths = verb.record_paths();
        verb.fields_read()
            .is_some_and(|read| read.iter().all(reads))
            && paths.is_some_and(|p| p.iter().all(free))
            && far.is_some_and(|far| far.iter().all(|(t, f)| field_access(who, t, f).reads()))
    }
}

/// Whether nothing `paths`, over `decl`, read across an edge is held from
/// `who`: it holds `restricted`, no hiding on `decl` or a far type, no
/// `only` on a far type nor, of a link type, on its ends, and reads every
/// field on the way, the first of each
/// path with them. A set's `where` and a trigger's `when` are decided for
/// no reader, so what they hold for a record tells what they read there
/// (055, extended). With no path across, nothing is asked.
pub(crate) fn holds_nothing_across<'d>(
    who: &Principal,
    decl: &'d TypeDecl,
    paths: &[Vec<FieldName>],
    ty: &dyn Fn(&TypeName) -> Option<&'d TypeDecl>,
) -> bool {
    // A path leaves the record where it steps through a reference or a
    // relation; into a value of its own it does not.
    let related = |n: &FieldName| {
        !decl.has_field(n) && decl.relations.iter().any(|r| r.name.as_str() == n.as_str())
    };
    let leaves = |p: &&Vec<FieldName>| related(&p[0]) || decl.stepped(p, ty).len() > 1;
    let paths: Vec<Vec<FieldName>> = paths.iter().filter(leaves).cloned().collect();
    if paths.is_empty() {
        return true;
    }
    let far = decl.reads_across(&paths, ty);
    let first = |p: &Vec<FieldName>| match decl.field(&p[0]) {
        Some(f) => field_access(who, decl, f).reads(),
        None => (decl.relations.iter())
            .filter(|r| r.name.as_str() == p[0].as_str())
            .all(|r| crate::describe::relation_shown(decl, r, ty, who)),
    };
    let bare = |t: &TypeName| {
        who.grants
            .types
            .get(t)
            .is_none_or(|g| g.hide_where.is_empty())
    };
    let unheld = |(t, f): &(&TypeDecl, &FieldDecl)| {
        own_only(who, &t.name).is_empty()
            && held_ends(who, t, ty).is_empty()
            && field_access(who, t, f).reads()
            && bare(&t.name)
    };
    who.grants.restricted && bare(&decl.name) && paths.iter().all(first) && far.iter().all(unheld)
}

/// Whether `who` can make a record of `decl` that its grant admits: a
/// create of `who` holds every pair its `only` and its `writes_only` name
/// ([`create_holds`]).
pub(crate) fn creates(who: &Principal, decl: &TypeDecl) -> bool {
    let pairs = own_only(who, &decl.name).into_iter();
    pairs
        .chain(own_writes_only(who, &decl.name))
        .all(|(f, v)| create_holds(who, decl, &f, &v))
}

/// Whether a create of `decl` by `who` can hold `field` at `value`: it
/// writes the field at the authored door (051); or the field is one only
/// verbs write whose `initial` is `value`; or the store fills it with the
/// creating writer (084) and `value` is `who`'s.
pub(crate) fn create_holds(
    who: &Principal,
    decl: &TypeDecl,
    field: &FieldName,
    value: &Value,
) -> bool {
    decl.field(field).is_some_and(|d| match d.authority {
        Authority::Authored | Authority::Shared => field_access(who, decl, d).writes(),
        Authority::Verb => holds_value(d.initial.as_ref(), value),
        Authority::Writer => holds_value(Some(&Value::String(who.writer())), value),
        _ => false,
    })
}

impl Sight<'_> {
    /// Whether its principal's grant admits only some records of any type:
    /// such a principal changes no shape.
    pub fn holds_only(&self) -> bool {
        let mut types = self.who().grants.types.values();
        types.any(|t| !t.only.is_empty() || !t.only_any.is_empty())
    }

    /// Refuse a read that names `decl` where its principal's `only` on it
    /// cannot be applied: a pair on an undeclared field, of a kind other
    /// than `text` or `reference`, with a non-text value, that the holder
    /// may not read, or computed at the read (an engine cannot filter by
    /// it). The refusal names the type, never the field, so an unknown
    /// field is not told.
    pub fn check_only(&self, decl: &TypeDecl) -> Result<(), Error> {
        if own_only(self.who(), &decl.name)
            .iter()
            .all(|p| self.applies(decl, p))
        {
            return Ok(());
        }
        let ty = decl.name.clone();
        Err(Error::invalid(Case::OnlyUnappliable { ty }))
    }

    /// Refuse a read that computes over the records of `ty` as a set (a
    /// count, an aggregate, a view) where its principal's grant admits only
    /// some of them and holds `whole_sets_only` there, so a part is never
    /// answered as the whole; of a view, so of every type it reads, as deep
    /// as it goes. The refusal names the type so narrowed.
    pub fn check_whole_set(&self, ty: &TypeName) -> Result<(), Error> {
        let (who, mut seen) = (self.who(), BTreeSet::new());
        let mut next = vec![ty.clone()];
        while let Some(ty) = next.pop() {
            let kept = ty.as_str().strip_prefix(lakelands_api::seam::KEPT_PREFIX);
            let ty = kept.map_or(ty.clone(), TypeName::from);
            if !seen.insert(ty.clone()) {
                continue;
            }
            // A link type answers only the links whose held ends are
            // admitted, so it is narrowed by them, and their own setting
            // holds over it too.
            let find = &|n: &TypeName| self.force.ty(n);
            let decl = self.force.ty(&ty);
            let ends: Vec<TypeName> = decl.map_or_else(Vec::new, |d| {
                let held = held_ends(who, d, find).into_iter();
                held.map(|(_, end)| end.clone()).collect()
            });
            let whole = who.grants.types.get(&ty).is_none_or(|t| !t.whole_sets_only);
            if !whole && (!own_only(who, &ty).is_empty() || !ends.is_empty()) {
                return Err(Error::invalid(Case::WholeSetNeeded { ty }));
            }
            next.extend(ends);
            if let Some(v) = decl.and_then(|d| d.view.as_deref()) {
                next.push(v.source.clone());
                next.extend(v.merge.iter().map(|m| m.source.clone()));
            }
        }
        Ok(())
    }

    /// Refuse a write of `decl` where its principal's `writes_only` on it
    /// cannot be applied, each pair judged as one of `only` is
    /// ([`Self::check_only`]), naming the type and never the field.
    pub fn check_writes_only(&self, decl: &TypeDecl) -> Result<(), Error> {
        let pairs = own_writes_only(self.who(), &decl.name);
        if pairs.iter().all(|p| self.applies(decl, p)) {
            return Ok(());
        }
        let ty = decl.name.clone();
        Err(Error::invalid(Case::WritesOnlyUnappliable { ty }))
    }

    /// Whether a pair of a grant on `decl` can be applied: a declared field
    /// of kind `text` or `reference` its holder reads, not computed at the
    /// read, with a text value or a set of them.
    fn applies(&self, decl: &TypeDecl, (f, v): &(FieldName, Value)) -> bool {
        use lakelands_api::ontology::StorageKind;
        let texts = |a: &Vec<Value>| a.iter().all(Value::is_string);
        decl.field(f).is_some_and(|d| {
            matches!(d.ty.kind, StorageKind::Text | StorageKind::Reference)
                && (v.is_string() || v.as_array().is_some_and(texts))
                && field_access(self.who(), decl, d).reads()
                && !d.at_read()
        })
    }

    /// Whether its principal's grant admits only some records of any type
    /// to write: such a principal changes no shape but by a proposal.
    pub fn holds_writes_only(&self) -> bool {
        let mut types = self.who().grants.types.values();
        types.any(|t| !t.writes_only.is_empty())
    }

    /// What its principal's grants admit of the records of `decl` to write
    /// (`writes_only`), the one answer every write door reads: every record
    /// where neither the type nor an end of a link type holds pairs; none
    /// where the type's pairs cannot be applied, or where an end holds some
    /// and the link type none, which no grant covers; else those holding
    /// the type's pairs (006).
    pub fn writes(&self, decl: &TypeDecl) -> Admit {
        let (who, find) = (self.who(), &|n: &TypeName| self.force.ty(n));
        let pairs = own_writes_only(who, &decl.name);
        if pairs.is_empty() && write_held_ends(who, decl, find).is_empty() {
            return Admit::All;
        }
        match pairs.is_empty() || self.check_writes_only(decl).is_err() {
            true => Admit::Nothing,
            false => Admit::Only(pairs),
        }
    }

    /// Whether its principal may write only some records of `decl`
    /// ([`Self::writes`]).
    pub fn write_holder(&self, decl: &TypeDecl) -> bool {
        self.writes(decl) != Admit::All
    }

    /// Whether a call of `verb` on `decl` can be its principal's write: on
    /// every record where it holds no `writes_only` there; else where its
    /// pairs apply and the verb moves none of them ([`Self::pinnable`]).
    pub fn writes_by(&self, decl: &TypeDecl, verb: &VerbDecl) -> bool {
        match self.writes(decl) {
            Admit::All => true,
            Admit::Nothing => false,
            _ => self.pinnable(decl, Some(verb)),
        }
    }

    /// Whether a write of `decl` that finds `before` at its key, live or
    /// retired, and leaves `after` is its principal's to make: both hold
    /// its pairs ([`Self::writes`]).
    pub fn writes_record(
        &self,
        decl: &TypeDecl,
        before: Option<&BTreeMap<FieldName, Value>>,
        after: Option<&BTreeMap<FieldName, Value>>,
    ) -> bool {
        let admit = self.writes(decl);
        admit != Admit::Nothing && before.into_iter().chain(after).all(|f| admit.holds(f))
    }

    /// Whether its principal may create a link record of `decl` between
    /// the end records `from` and `to` (absent where none is found): one
    /// held end is enough, unless the other is to-one by the relation's
    /// cardinality or a computed field of the other's type reads across the
    /// relation, judged against the declarations in force; then both must
    /// be held. An end is held where its type carries its principal's
    /// `writes_only` and the record holds those pairs; an end of a type
    /// carrying none is held by no grant, so a link with no end held is
    /// refused, never defaulted open (006). A type that is no link type is
    /// not judged.
    pub fn links(
        &self,
        decl: &TypeDecl,
        from: Option<&BTreeMap<FieldName, Value>>,
        to: Option<&BTreeMap<FieldName, Value>>,
    ) -> bool {
        use lakelands_api::ontology::Cardinality::*;
        let Some((owner, rel)) = crate::links::link_type_parts(&decl.name) else {
            return true;
        };
        let owner = self.force.ty(&owner);
        let Some((owner, r)) = owner.and_then(|o| Some((o, o.relation(&rel)?))) else {
            return false;
        };
        let target = self.force.ty(&r.target);
        let ends = [
            (
                Some(owner),
                from,
                matches!(r.cardinality, ManyToOne | OneToOne),
            ),
            (target, to, matches!(r.cardinality, OneToMany | OneToOne)),
        ];
        type End<'e> = (
            Option<&'e TypeDecl>,
            Option<&'e BTreeMap<FieldName, Value>>,
            bool,
        );
        let held = |(d, fields, ..): &End<'_>| {
            d.is_some_and(|d| match self.writes(d) {
                admit @ Admit::Only(_) => fields.is_some_and(|f| admit.holds(f)),
                _ => false,
            })
        };
        let freed = |(d, _, one): &End<'_>| !one && d.is_some();
        match ends.iter().map(held).collect::<Vec<_>>()[..] {
            [true, true] => true,
            [true, false] => freed(&ends[1]),
            [false, true] => freed(&ends[0]),
            _ => false,
        }
    }

    /// Refuse a read that names `decl` where a hiding its principal's grant
    /// holds on it cannot be applied ([`Self::lowered`]), naming the type
    /// and never a field, as [`Self::check_only`] does.
    pub fn check_hidings(&self, decl: &TypeDecl) -> Result<(), Error> {
        match !self.hides(&decl.name) || self.lowering(decl, None).0 {
            true => Ok(()),
            false => Err(Error::invalid(Case::HidingUnappliable {
                ty: decl.name.clone(),
            })),
        }
    }

    /// The fields of `ty` its principal's hidings lower on a record holding
    /// `fields`, each with the access it has there; none on a record no
    /// hiding selects (006). A field built from a lowered one is lowered as
    /// a type-wide grant would lower it. A hiding applies where its field
    /// is of kind `text` or `reference`, read by its holder, not computed
    /// at the read and lowered by no hiding, its values are not empty, and
    /// each field it lowers is no key, no valid time, none `only` names and
    /// not its own, to `none` or `masked`. Where one does not, every hiding
    /// on the type selects every record: it fails closed.
    pub fn lowered(&self, ty: &TypeName, fields: &BTreeMap<FieldName, Value>) -> Lowered {
        let hidings = self.hidings(ty);
        if hidings.is_empty() {
            return Lowered::default();
        }
        let Some(decl) = self.force.ty(ty) else {
            return Lowered::default();
        };
        let applies = self.lowering(decl, None).0;
        let holds = |h: &HiddenWhere| match fields.get(&h.field) {
            Some(Value::String(v)) => h.values.contains(v),
            _ => false,
        };
        let picked: Vec<bool> = hidings.iter().map(|h| !applies || holds(h)).collect();
        match picked.contains(&true) {
            true => self.lowering(decl, Some(picked)).1,
            false => Lowered::default(),
        }
    }

    /// The fields of `decl` lowered on a record each hiding `picked` names
    /// selects, worked out once a sight; with none, whether the hidings
    /// apply.
    pub(crate) fn lowering(&self, decl: &TypeDecl, picked: Option<Vec<bool>>) -> (bool, Lowered) {
        let key = (decl.name.clone(), picked.clone().unwrap_or_default());
        if let Some(known) = self.lowerings.borrow().get(&key) {
            return known.clone();
        }
        let (who, hidings) = (self.who(), self.hidings(&decl.name));
        // Hidings that cannot be applied hide each field they name.
        let fails = picked.is_some() && !self.lowering(decl, None).0;
        let low = |picked: &[bool]| {
            let mut low = who.clone();
            let grant = low.grants.types.entry(decl.name.clone()).or_default();
            for (h, _) in hidings.iter().zip(picked).filter(|p| *p.1) {
                for (f, a) in &h.fields {
                    let a = if fails { Access::None } else { *a };
                    let was = (grant.fields.get(f).copied())
                        .unwrap_or_else(|| who.grants.access(&decl.name, f));
                    grant.fields.insert(f.clone(), was.lowest(a));
                }
            }
            low
        };
        let known = match &picked {
            Some(picked) => {
                let low = low(picked);
                let lowered = (decl.fields.iter())
                    .map(|f| (f, field_access(&low, decl, f)))
                    .filter(|(f, a)| *a != field_access(who, decl, f));
                (
                    true,
                    Rc::new(lowered.map(|(f, a)| (f.name.clone(), a)).collect()),
                )
            }
            None => {
                let all = low(&vec![true; hidings.len()]);
                let only = own_only(who, &decl.name);
                let fixed = |f: &FieldName| {
                    decl.key_fields().contains(f)
                        || [&decl.valid_from, &decl.valid_to].contains(&&Some(f.clone()))
                        || only.iter().any(|(o, _)| o == f)
                };
                let applies = |h: &HiddenWhere| {
                    let g = decl.field(&h.field).filter(|g| {
                        use lakelands_api::ontology::StorageKind::{Reference, Text};
                        matches!(g.ty.kind, Text | Reference)
                            && field_access(who, decl, g).reads()
                            && field_access(&all, decl, g).reads()
                            && !g.at_read()
                    });
                    let lowers = |(f, a): (&FieldName, &Access)| {
                        matches!(a, Access::None | Access::Masked) && *f != h.field && !fixed(f)
                    };
                    g.is_some() && !h.values.is_empty() && h.fields.iter().all(lowers)
                };
                let records = decl.view.is_none();
                (records && hidings.iter().all(applies), Lowered::default())
            }
        };
        self.lowerings.borrow_mut().insert(key, known.clone());
        known
    }

    /// Refuse a write whose patch carries a field its principal's hidings
    /// lower on the record before the write or after it, naming the field
    /// (012).
    pub fn check_hidden_write(
        &self,
        ty: &TypeName,
        records: [Option<&BTreeMap<FieldName, Value>>; 2],
        sent: &[FieldName],
    ) -> Result<(), Error> {
        for fields in records.into_iter().flatten() {
            let lowered = self.lowered(ty, fields);
            if let Some(f) = sent.iter().find(|f| lowered.contains_key(*f)) {
                return Err(Error::refused(Some(f.clone()), Refusal::NotGranted));
            }
        }
        Ok(())
    }

    /// What its principal is shown of a quarantined record's `cells`,
    /// under their names at the head of `decl`, and which it is shown
    /// masked (085). A cell is read as its field, `sensitive` raising one
    /// that was so when it came (`sensitive`); one of no field by the
    /// type's default. A hiding lowers it where it selects the record the
    /// cells would leave, or `live`, the record at the key: a `G` cell
    /// that is there and holds no text selects, as no value can be told
    /// not to.
    pub fn quarantined_cells(
        &self,
        decl: &TypeDecl,
        cells: &BTreeMap<FieldName, Value>,
        sensitive: &[FieldName],
        live: Option<&BTreeMap<FieldName, Value>>,
    ) -> (BTreeMap<FieldName, Value>, Vec<FieldName>) {
        let (who, hidings) = (self.who(), self.hidings(&decl.name));
        let applies = hidings.is_empty() || self.lowering(decl, None).0;
        let selects = |h: &HiddenWhere| match cells.get(&h.field) {
            Some(Value::String(v)) => h.values.contains(v),
            Some(Value::Null) | None => false,
            Some(_) => true,
        };
        let on_live = |h: &HiddenWhere| match live.and_then(|l| l.get(&h.field)) {
            Some(Value::String(v)) => h.values.contains(v),
            _ => false,
        };
        let picked: Vec<bool> = (hidings.iter())
            .map(|h| !applies || selects(h) || on_live(h))
            .collect();
        let lowered = match picked.contains(&true) {
            true => self.lowering(decl, Some(picked)).1,
            false => Lowered::default(),
        };
        let (mut shown, mut masked) = (BTreeMap::new(), Vec::new());
        for (name, value) in cells {
            let raised = sensitive.contains(name);
            let access = match decl.field(name) {
                Some(f) => {
                    let mut f = f.clone();
                    f.sensitive |= raised;
                    let a = field_access(who, decl, &f);
                    lowered.get(name).map_or(a, |l| a.lowest(*l))
                }
                None => match type_access(who, &decl.name) {
                    Access::None if raised => Access::Masked,
                    a => a,
                },
            };
            match access {
                a if a.reads() => _ = shown.insert(name.clone(), value.clone()),
                Access::Masked => masked.push(name.clone()),
                _ => {}
            }
        }
        (shown, masked)
    }

    /// Whether its principal's hidings lower `f` of `decl` on some records.
    pub fn lowers_somewhere(&self, decl: &TypeDecl, f: &FieldName) -> bool {
        let all = vec![true; self.hidings(&decl.name).len()];
        self.hides(&decl.name) && self.lowering(decl, Some(all)).1.contains_key(f)
    }

    /// The hidings its principal's grant on `ty` holds.
    pub(crate) fn hidings(&self, ty: &TypeName) -> &[HiddenWhere] {
        let held = self.who().grants.types.get(ty);
        held.map_or(&[], |t| t.hide_where.as_slice())
    }

    /// Whether its principal's grant hides a field of `ty` on some records.
    pub fn hides(&self, ty: &TypeName) -> bool {
        !self.hidings(ty).is_empty()
    }

    /// Each field of `decl` its principal's hidings lower on some records,
    /// with the field and values of each hiding that does, as `describe`
    /// tells them: none where they cannot be applied, so it names no field
    /// its reader may not read.
    pub fn hidden_where(
        &self,
        decl: &TypeDecl,
    ) -> BTreeMap<FieldName, Vec<(FieldName, BTreeSet<String>)>> {
        let hidings = self.hidings(&decl.name);
        let mut out: BTreeMap<FieldName, Vec<_>> = BTreeMap::new();
        if hidings.is_empty() || !self.lowering(decl, None).0 {
            return out;
        }
        for (i, h) in hidings.iter().enumerate() {
            let picked = (0..hidings.len()).map(|j| j == i).collect();
            for f in self.lowering(decl, Some(picked)).1.keys() {
                let held = out.entry(f.clone()).or_default();
                held.push((h.field.clone(), h.values.clone()));
            }
        }
        out
    }

    /// Refuse a read that probes a field its principal may not read, or a
    /// field the type does not declare. `probed` are the fields a filter,
    /// an ordering or an aggregate touches.
    pub fn check_probe<'a>(
        &self,
        decl: &TypeDecl,
        probed: impl IntoIterator<Item = &'a FieldName>,
    ) -> Result<(), Error> {
        for name in probed {
            let field = field_of(decl, name)?;
            match field_access(self.who(), decl, field) {
                Access::None => return Err(no_field(decl, name)),
                a if !a.reads() => {
                    return Err(Error::refused(Some(name.clone()), Refusal::NotGranted));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Refuse `name`, a field `decl` does not declare or one its principal
    /// may not know of, alike. A masked field is not refused: a projection
    /// that names it reads it as the record does.
    pub fn check_known(&self, decl: &TypeDecl, name: &FieldName) -> Result<(), Error> {
        match field_access(self.who(), decl, field_of(decl, name)?) {
            Access::None => Err(no_field(decl, name)),
            _ => Ok(()),
        }
    }

    /// Refuse a search its principal may not make: a lexical search probes
    /// every searchable field at once, so each must be readable; a vector
    /// search probes each vector field it names, in order, and before each
    /// every field it is embedded from.
    ///
    /// A lexical or hybrid search naming `fields` probes those instead, in
    /// name order; the vector half is not narrowed. A named field must be
    /// declared, readable and searchable, judged in that order, so the
    /// refusal never tells a principal whether a field it may not read is
    /// searchable.
    pub fn check_search(
        &self,
        decl: &TypeDecl,
        fields: Option<&[FieldName]>,
        vector_fields: &[&FieldName],
    ) -> Result<(), Error> {
        let vectors = vector_fields.iter().flat_map(|v| {
            let field = decl.field(v);
            let embed = field.and_then(|f| f.embed.as_ref());
            embed.into_iter().flat_map(|e| e.from.iter()).chain([*v])
        });
        let Some(named) = fields else {
            return self.check_probe(decl, self.searched_fields(decl).chain(vectors));
        };
        if named.is_empty() {
            return Err(Error::invalid(Case::SearchFieldsEmpty));
        }
        let named: BTreeSet<&FieldName> = named.iter().collect();
        for name in named {
            self.check_probe(decl, [name])?;
            if !decl.fields.iter().any(|f| &f.name == name && f.searchable) {
                let (field, ty) = (name.clone(), decl.name.clone());
                return Err(Error::invalid_at(name, Case::NotSearchable { field, ty }));
            }
        }
        self.check_probe(decl, vectors)
    }

    /// Refuse a search's own text weights its principal could not give,
    /// judged as `search_weights`' `text_fields` is: each field, in name
    /// order, probed as a searched field (undeclared and unshown answer
    /// alike), then searchable, then its weight one a search may give. An
    /// empty map is refused as an empty `fields` is.
    pub fn check_text_weights(
        &self,
        decl: &TypeDecl,
        weights: &BTreeMap<FieldName, f64>,
    ) -> Result<(), Error> {
        if weights.is_empty() {
            return Err(Error::invalid(Case::SearchTextFieldsEmpty));
        }
        for (name, w) in weights {
            self.check_probe(decl, [name])?;
            let (field, ty) = (name.clone(), decl.name.clone());
            let case = if !decl.fields.iter().any(|f| &f.name == name && f.searchable) {
                Case::NotSearchable { field, ty }
            } else if !lakelands_api::query::weight_fits(*w) {
                Case::WeightRange { field }
            } else {
                continue;
            };
            return Err(Error::invalid_at(name, case));
        }
        Ok(())
    }

    /// The searchable fields of `decl` a search that names none reads:
    /// those its principal reads. A field it may not read is never
    /// searched, so the search neither refuses, names nor ranks by it.
    pub fn searched_fields<'d>(&self, decl: &'d TypeDecl) -> impl Iterator<Item = &'d FieldName> {
        let who = self.who();
        (decl.fields.iter())
            .filter(move |f| f.searchable && field_access(who, decl, f).reads())
            .map(|f| &f.name)
    }

    /// Refuse its principal's write of `patch` to `decl`, whole, unless
    /// every field it carries exists, is written through `door`, from
    /// `source` where it is `ingest`, and is granted to it. Fields are
    /// checked in name order, so the refusal names the same field every
    /// time.
    pub fn check_write(
        &self,
        decl: &TypeDecl,
        (door, source): (Door, Option<&SourceId>),
        patch: &BTreeMap<FieldName, Value>,
    ) -> Result<(), Error> {
        let who = self.who();
        if door == Door::Source && !who.grants.ingest {
            return Err(Error::not_granted());
        }
        not_a_view(decl)?;
        for name in patch.keys() {
            let field = field_of(decl, name)?;
            if field_access(who, decl, field) == Access::None {
                return Err(no_field(decl, name));
            }
            if !door.admits(field, source) {
                return Err(Error::refused(Some(name.clone()), Refusal::WrongDoor));
            }
            if !who.grants.access(&decl.name, name).writes() {
                return Err(Error::refused(Some(name.clone()), Refusal::NotGranted));
            }
        }
        Ok(())
    }
}

/// The fields a record's hidings lower, each with its access there
/// ([`Sight::lowered`]).
pub type Lowered = Rc<BTreeMap<FieldName, Access>>;

/// Whether a type's hidings apply, and what they lower, by which of them
/// select a record.
pub(crate) type Lowerings = BTreeMap<(TypeName, Vec<bool>), (bool, Lowered)>;

/// The door a write comes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Door {
    /// `upsert`, `link`, an accepted proposal of either.
    Authored,
    /// `ingest`.
    Source,
    /// A declared verb's write, called or accepted, and what the store
    /// writes as declared: an aging rule's, the fold of an author's write.
    Verb,
}

impl Door {
    /// Whether `field` may be written through this door, from `source`
    /// where it is `ingest`: by its authority, and an authored field by a
    /// source it admits (069).
    pub fn admits(self, field: &FieldDecl, source: Option<&SourceId>) -> bool {
        let admitted = |s: &SourceId| field.admits.contains(s);
        match (self, field.authority) {
            (Door::Source, Authority::Authored) => source.is_some_and(admitted),
            (_, Authority::Shared)
            | (Door::Authored | Door::Verb, Authority::Authored)
            | (Door::Verb, Authority::Verb)
            | (Door::Source, Authority::Source) => true,
            _ => false,
        }
    }

    /// Whether a record of a type its `retired_by` names may be retired
    /// through this door: a type only its verbs retire, by a verb alone
    /// (lakelands#1148).
    pub fn admits_retirement(self, by: RetiredBy) -> bool {
        by.is_authors() || self == Door::Verb
    }
}

/// Refuse `source`'s write of an authored field it is admitted to where the
/// record as it stands, `live`, holds another hand's write there: an author's
/// value or clear, or another source's. Its own write and a field no hand wrote
/// are written over, and the field's `initial` on a record the source made,
/// which `made` answers, asked only for an `initial` (069). Fields are checked
/// in name order, as [`Sight::check_write`] checks them.
pub fn check_admitted(
    decl: &TypeDecl,
    (live, source): (Option<&StoredRecord>, &SourceId),
    patch: &BTreeMap<FieldName, Value>,
    made: impl FnOnce() -> Result<bool, Error>,
) -> Result<(), Error> {
    let Some(live) = live else {
        return Ok(());
    };
    let admitted = |n: &FieldName| {
        (decl.fields.iter()).any(|f| &f.name == n && f.authority == Authority::Authored)
    };
    let initial = |n: &FieldName| {
        live.meta
            .get(n)
            .is_some_and(|m| m.origin == Origin::Initial)
    };
    let made = match patch.keys().any(|n| admitted(n) && initial(n)) {
        true => made()?,
        false => false,
    };
    // A clear keeps its writer's provenance, so the field's last hand is
    // read there.
    let others = |n: &FieldName| match live.meta.get(n) {
        Some(m) if m.origin == Origin::Source => m.source.as_ref() != Some(source),
        Some(m) => m.origin != Origin::Initial || !made,
        None => live.fields.contains_key(n),
    };
    match patch.keys().find(|n| admitted(n) && others(n)) {
        Some(name) => Err(Error::refused(Some(name.clone()), Refusal::WrongDoor)),
        None => Ok(()),
    }
}

/// Refuse a view at a door that writes or holds a record: its records are
/// computed, never written. Every such door says it in these words.
pub fn not_a_view(decl: &TypeDecl) -> Result<(), Error> {
    if decl.view.is_some() {
        let ty = decl.name.clone();
        return Err(Error::invalid(Case::IsAView { ty }));
    }
    Ok(())
}

/// The field `decl` declares as `name`, or a refusal naming it.
fn field_of<'d>(decl: &'d TypeDecl, name: &FieldName) -> Result<&'d FieldDecl, Error> {
    let field = decl.field(name);
    field.ok_or_else(|| no_field(decl, name))
}

/// The answer for a field `decl` does not declare, which is also the
/// answer for one the principal may not know of.
fn no_field(decl: &TypeDecl, name: &FieldName) -> Error {
    let (ty, field) = (decl.name.clone(), name.clone());
    Error::invalid_at(name, Case::UnknownField { ty, field })
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
pub(crate) fn shown_hash<'a>(
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

/// `t` as `describe` would show its fields to `who`, a relation's link
/// fields too (024): without those it may not know of and the relations
/// it is not shown, and a field it may not read left with its shape
/// alone. A listing hands a declaration out so; an author's view of a
/// type is made by [`known_fields`], and holds a field as declared.
pub(crate) fn described_fields<'a>(who: &Principal, t: &mut TypeDecl, find: Find<'_, 'a>) {
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
pub(crate) fn known_fields<'a>(who: &Principal, t: &mut TypeDecl, find: Find<'_, 'a>) {
    if !keeps_from(who, t, find) {
        return;
    }
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

/// Whether [`known_fields`] takes anything out of `t` for `who`: a
/// relation it is not shown, or a field or a link field it may not know
/// of. Asked before a type is copied to take it out (lakelands#943).
pub(crate) fn keeps_from(who: &Principal, t: &TypeDecl, find: Find<'_, '_>) -> bool {
    let none = |d: &TypeDecl, f: &FieldDecl| field_access(who, d, f) == Access::None;
    let relation = |r: &lakelands_api::ontology::RelationDecl| {
        let link = crate::links::link_type(&t.name, r);
        !crate::describe::relation_shown(t, r, find, who) || r.fields.iter().any(|f| none(&link, f))
    };
    t.fields.iter().any(|f| none(t, f)) || t.relations.iter().any(relation)
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
        let unshown = |ty: &TypeName| {
            find(ty).is_some_and(|d| !crate::describe::type_shown(d, find, who, false))
        };
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

/// `who` as a read sees the lake (092): each type a write is shown and a
/// read is not, keyed by a field it may only write or granted nothing of
/// it but writes, held at `none`, every field by name, so that every read
/// door answers it as a type that is not declared, and each reference to
/// it is held at `none` in turn ([`hide_unshown_references`]), to a fixed
/// point. Only a `write_only` grant parts the two, so one who holds none
/// is left as it is. Asked of `who` once [`hide_unshown_references`] has
/// held it; answers `None` where no type is so held, and otherwise the
/// references hidden besides, in order.
pub(crate) fn hide_unread_types(
    who: &mut Principal,
    force: &dyn crate::sight::InForce,
) -> Option<Vec<(TypeName, FieldName)>> {
    if !writes_blind(who) {
        return None;
    }
    let (types, find) = (force.types(), &|n: &TypeName| force.ty(n));
    let mut hidden: Option<Vec<_>> = None;
    loop {
        let shown = |d, reading| crate::describe::type_shown(d, find, who, reading);
        let unread: Vec<&TypeDecl> = (types.iter().copied())
            .filter(|d| shown(d, false) && !shown(d, true))
            .collect();
        if unread.is_empty() {
            return hidden;
        }
        for t in unread {
            let grants = who.grants.types.entry(t.name.clone()).or_default();
            grants.default = Some(Access::None);
            for f in &t.fields {
                grants.fields.insert(f.name.clone(), Access::None);
            }
        }
        let more = hide_unshown_references(who, force);
        hidden.get_or_insert_default().extend(more);
    }
}

/// Whether `who` holds a `write_only` grant, to the lake, a type or a
/// field: only such a grant shows a type to a write and not to a read.
pub(crate) fn writes_blind(who: &Principal) -> bool {
    let (g, blind) = (&who.grants, |a: &Access| *a == Access::WriteOnly);
    let typed =
        |t: &TypeGrants| t.default.as_ref().is_some_and(blind) || t.fields.values().any(blind);
    blind(&g.default) || g.types.values().any(typed)
}

/// `who` as a write to a type a read is not shown sees it (092): each
/// field of each such type it may write held `write_only`, every other at
/// `none`, so its description names what it may write and nothing it
/// reads; a view, which no one writes, wholly at `none`. `read` is `who`
/// as a read sees the lake.
pub(crate) fn writing_unread(
    who: &Principal,
    read: &Principal,
    force: &dyn crate::sight::InForce,
) -> Principal {
    let mut out = who.clone();
    let find = &|n: &TypeName| force.ty(n);
    for t in force.types() {
        let shown = |p| crate::describe::type_shown(t, find, p, false);
        if !shown(who) || shown(read) {
            continue;
        }
        // No one writes a view, so one a read is not shown is not there.
        let writes = |f: &FieldDecl| who.grants.access(&t.name, &f.name).writes();
        let write_side = |f: &FieldDecl| match t.view.is_none() && writes(f) {
            true => Access::WriteOnly,
            false => Access::None,
        };
        let fields = t.fields.iter().map(|f| (f.name.clone(), write_side(f)));
        let grants = out.grants.types.entry(t.name.clone()).or_default();
        grants.default = Some(Access::None);
        grants.fields = fields.collect();
    }
    out
}

/// Whether `who` may not know of some field: only a `none` its grants
/// give, to the lake, a type or a field, hides one.
pub(crate) fn hides_some(who: &Principal) -> bool {
    let (g, none) = (&who.grants, |a: &Access| *a == Access::None);
    let typed =
        |t: &TypeGrants| t.default.as_ref().is_some_and(none) || t.fields.values().any(none);
    none(&g.default) || g.types.values().any(typed)
}

/// Whether a shape holds what a migration's report names by `name`: a
/// field, a link field (`relation.field`), or a relation, which a report
/// lists as dropped by its own name.
pub(crate) fn holds(t: &TypeDecl, name: &FieldName) -> bool {
    let relation = |r: &str| t.relations.iter().find(|x| x.name.as_str() == r);
    match name.as_str().split_once('.') {
        Some((r, f)) => relation(r).is_some_and(|x| x.fields.iter().any(|x| x.name.as_str() == f)),
        None => t.has_field(name) || relation(name.as_str()).is_some(),
    }
}

/// Whether `who` may know what a migration's report names by `name` in a
/// shape: a field or a relation `known`, the shape as `who` knows it,
/// holds; or a link field `relation.field` of a relation `known` holds,
/// judged on its link type as `whole` declares it, under the grants on
/// that link type, its ends `from` and `to` included (lakelands#1092).
fn link_known(who: &Principal, whole: &TypeDecl, known: &TypeDecl, name: &FieldName) -> bool {
    let Some((r, f)) = name.as_str().split_once('.') else {
        return holds(known, name);
    };
    let shown = known.relations.iter().any(|x| x.name.as_str() == r);
    let relation = whole.relations.iter().find(|x| x.name.as_str() == r);
    shown
        && relation.is_some_and(|relation| {
            let link = crate::links::link_type(&whole.name, relation);
            let field = link.fields.iter().find(|x| x.name.as_str() == f);
            field.is_some_and(|x| field_access(who, &link, x) != Access::None)
        })
}

impl Sight<'_> {
    /// `m`, a migration not in force, as its principal may know it: the shape
    /// it leaves as [`Self::described_fields`] shows it, judged with grants held
    /// through the migration's own renames (of fields, and of the link types
    /// its renamed relations move); a rename only where it may know the field
    /// under both names, the old one in `was`, the type it was written against.
    /// A waiting proposal is judged by the names it was written with, so a
    /// restriction on a name taken since reaches the name it bore then.
    pub fn known_migration(&self, was: &TypeDecl, m: &mut Migration, find: Find<'_, '_>) {
        let who = self.who();
        let (then, mut was) = (following(who, m), was.clone());
        known_fields(who, &mut was, find);
        described_fields(&then, &mut m.to, find);
        (m.renames).retain(|(old, new)| holds(&was, old) && holds(&m.to, new));
        m.fill.retain(|f| holds(&m.to, f));
    }

    /// A report's hashes as its principal is shown them, set after
    /// [`Self::shown_report`] decided `more`: `from` of `made`, the type it was
    /// made from, `to` with the grants held through the renames; whole to one
    /// not shown the type. `find` is the lookup [`Self::known_migration`] lists
    /// `to` with, so the `to` hash is that of the listed `to`.
    pub fn shown_hashes(
        &self,
        made: &TypeDecl,
        m: &Migration,
        report: &mut MigrationReport,
        find: Find<'_, '_>,
    ) {
        let who = self.who();
        if report.ty != m.to.name {
            return;
        }
        report.from_schema_hash = shown_hash(who, made, true, &report.from_schema_hash, find);
        let then = following(who, m);
        report.to_schema_hash = shown_hash(&then, &m.to, true, &report.to_schema_hash, find);
    }

    /// Whether its principal reads every field of every type: no grant below
    /// `read`, no `only` and no hiding. Such a principal is shown the lake whole,
    /// `sensitive` fields too, since only a `none` masks one.
    pub fn reads_all_fields(&self) -> bool {
        let who = self.who();
        let reads = |a: &Access| a.reads();
        let typed = |t: &TypeGrants| {
            t.default.as_ref().is_none_or(reads)
                && t.fields.values().all(reads)
                && t.only.is_empty()
                && t.only_any.is_empty()
                && t.hide_where.is_empty()
        };
        reads(&who.grants.default) && who.grants.types.values().all(typed)
    }

    /// Whether its principal reads the field `f` of the type `m` leaves, there
    /// and as `was` declares it, under the name it had.
    pub fn reads_both(&self, was: &TypeDecl, m: &Migration, f: &FieldName) -> bool {
        let who = self.who();
        let had = m.renames.iter().find(|(_, new)| new == f);
        !unread(who, &m.to, f) && !unread(who, was, had.map_or(f, |(old, _)| old))
    }

    /// A migration's report as its principal may know it (013, 024 and 027,
    /// extended): every place that answers one calls this. `was` is the type
    /// the report was made against: as it stands, or, for a listed proposal, as
    /// it stood when proposed, read with its grants then. `sees` is a type the
    /// reader may see. An entry is shown where the reader may see the type and
    /// know of the field: a conversion in the shape the migration leaves, and
    /// by the name it took its values from where renamed, as is a rewritten
    /// reference of the migrated type itself; a member restated (a link field's
    /// entry, `relation.field`, judged on its link type by `link_known`), one
    /// of [`FieldDecl::UNREAD`] only where the reader reads the field in `to`
    /// and as the type stands, under the name it had, since a migration may
    /// change a field's access with no rename; a dropped field or renamed
    /// relation in the shape as it stands; a declaration the migration carries
    /// where `described`, the lake as `describe` shows it, names it, asked only
    /// for a report that carries one. The totals stay; `more` says something
    /// was left out here, or a count is larger than the keys named beside it,
    /// so it tells nothing the report does not. The hashes are
    /// [`Self::shown_hashes`]'s, and the id is made again over what is shown. A
    /// reader not shown the migrated type is answered no migration at all, by
    /// every door (lakelands#762).
    pub fn shown_report<'a, D: std::borrow::Borrow<lakelands_api::describe::LakeDescription>>(
        &self,
        was: &TypeDecl,
        m: &Migration,
        report: &mut MigrationReport,
        described: impl FnOnce() -> D,
        sees: impl Fn(&TypeName) -> Option<&'a TypeDecl>,
    ) {
        let who = self.who();
        let whole = report.clone();
        let (stands, mut was, mut to) = (was, was.clone(), m.to.clone());
        known_fields(who, &mut was, &sees);
        known_fields(who, &mut to, &sees);
        let sourced = |new: &FieldName| {
            let mut from = m.renames.iter().filter(|(_, n)| n == new);
            from.all(|(old, _)| holds(&was, old))
        };
        let known = |ty: &TypeName, field: &FieldName| {
            let found = sees(ty).and_then(|t| Some((t, t.field(field)?)));
            found.is_some_and(|(t, f)| field_access(who, t, f) != Access::None)
        };
        // A link field's row is judged on its link type (lakelands#1092).
        let in_to = |f: &FieldName| link_known(who, &m.to, &to, f);
        report
            .fields
            .retain(|c| in_to(&c.field) && sourced(&c.field));
        // A member restated is the type's, or shown as its field's conversion
        // is, and one beyond the shape where the field is read in both shapes.
        let read = |f: &FieldName| self.reads_both(stands, m, f);
        // A valid time over a field the reader may not know of is not named
        // (024, extended).
        let named =
            |v: &serde_json::Value, t: &TypeDecl| v.as_str().is_none_or(|f| holds(t, &f.into()));
        // A constraint is shown to one who reads every field it reads, in both
        // shapes, as `describe_type` tells it (lakelands#1119).
        let told = |c: &serde_json::Value| {
            let c = serde_json::from_value::<lakelands_api::ontology::ConstraintDecl>(c.clone());
            let e = c.ok().and_then(|c| c.holds.typed().ok());
            e.is_some_and(|e| e.fields_read().into_iter().all(&read))
        };
        for r in
            (report.restated.iter_mut()).filter(|r| r.field.is_none() && r.member == "constraints")
        {
            for v in [&mut r.was, &mut r.to] {
                if let Some(cs) = v.as_array_mut() {
                    cs.retain(told);
                }
            }
        }
        let empty = |v: &serde_json::Value| v.as_array().is_none_or(Vec::is_empty);
        report.restated.retain(|r| {
            r.member != "constraints" || r.was != r.to && !(empty(&r.was) && empty(&r.to))
        });
        report.restated.retain(|r| {
            let shape = !FieldDecl::UNREAD.contains(&r.member.as_str());
            let shown = |f| in_to(f) && sourced(f) && (shape || read(f));
            let timed = r.field.is_none() && matches!(r.member.as_str(), "valid_from" | "valid_to");
            r.field.as_ref().is_none_or(shown)
                && (!timed || named(&r.was, &was) && named(&r.to, &to))
        });
        report
            .dropped
            .retain(|(f, _)| link_known(who, stands, &was, f));
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
        report.filled.retain(|f| in_to(&f.field));
        // A breach to one who reads every field its constraint reads: of the
        // migrated type in both shapes, of another as it stands (lakelands#1119).
        let constraint = |b: &lakelands_api::write::Breach| {
            let own = b.ty == m.to.name;
            let t = if own { Some(&m.to) } else { sees(&b.ty) };
            let c = t.and_then(|t| t.constraints.iter().find(|c| c.name == b.constraint));
            let e = c.and_then(|c| c.holds.typed().ok());
            let reads = |f: &FieldName| match own {
                true => read(f),
                false => sees(&b.ty)
                    .and_then(|t| Some((t, t.field(f)?)))
                    .is_some_and(|(t, x)| field_access(who, t, x).reads()),
            };
            e.is_some_and(|e| e.fields_read().into_iter().all(reads))
        };
        report.breaches.retain(constraint);
        // A reference left naming no live record, to one who reads it in both
        // shapes and is shown its target type (062).
        let target = |f: &FieldName| (m.to.field(f))?.ty.target.clone();
        report
            .unlive
            .retain(|u| read(&u.field) && target(&u.field).is_some_and(|t| sees(&t).is_some()));
        report.references.retain(|r| match r.ty == m.to.name {
            true => holds(&to, &r.field) && sourced(&r.field),
            false => known(&r.ty, &r.field),
        });
        let unnamed = |c: &lakelands_api::write::FieldConversion| {
            c.not_convertible > c.sample_keys.len() as u64
        };
        let collide: u64 = report.rekeyed.iter().map(|r| r.collisions).sum();
        let unfilled = |f: &lakelands_api::write::FieldFill| f.records > f.sample_keys.len() as u64;
        // What a reference's count left out is told by the plan, from the
        // declarations alone, never by what the count held (062).
        let without = |r: &MigrationReport| MigrationReport {
            unlive: Vec::new(),
            ..r.clone()
        };
        report.more = without(report) != without(&whole)
            || report.fields.iter().any(unnamed)
            || report.filled.iter().any(unfilled)
            || (report.breaches.iter()).any(|b| b.records > b.sample_keys.len() as u64)
            || (report.unlive.iter()).any(|u| u.records > u.sample_keys.len() as u64)
            || collide > report.sample_collisions.len() as u64;
    }
}
#[cfg(test)]
mod tests;

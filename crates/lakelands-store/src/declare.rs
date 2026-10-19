//! `declare`, `add_field` and the migration preflight's checks: what a
//! declaration must satisfy before it enters the log.
//!
//! The bundle checks the same rules when it loads, but the door is the
//! invariant: a declaration that reaches a lake by any path is checked
//! here, whole, and refused whole.
//!
//! - **Names** are ASCII identifiers; a type name is identifiers joined by
//!   `.` (`lakelands_core::names`). `sys.` names are reserved
//!   (`docs/decisions/003-reservation-by-namespace.md`): no caller declares
//!   one; the store declares the system types itself.
//! - **Field types** name what their kind needs and nothing else: a
//!   `reference` its target (a declared type), a `vector` its dimensions,
//!   other kinds neither. A value type a field names exists, over the
//!   field's kind.
//! - **Expressions** parse and check in the place they are evaluated:
//!   constraints and filters over one record, guards over a write, view
//!   measures over a group (`docs/decisions/023-one-evaluator-decides-what-an-expression-means.md`).
//! - **Derived fields** can be ordered: no field reads itself, no cycle.
//! - **Keys** are fields of a kind with a string form (text, reference,
//!   decimal, number, boolean, date-time, date), and so are a view's `group_by`
//!   fields, which key its records (`docs/log-format.md` §2.3).
//! - **References** name what exists: keys, `valid_from` / `valid_to`,
//!   `depends_on`, `via`, a view's source and `group_by`, a relation's
//!   target, a verb's type and assignments, a trigger's or object set's
//!   type, a derived field's function.
//! - **Tool names** the lake would offer stay unique, non-empty and short
//!   (`docs/decisions/014-four-doors-per-type-one-name-generator.md`).
//! - **Shape.** A revision of a type is declared only when it reshapes no
//!   stored record; otherwise it needs `migrate`
//!   (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`). A
//!   value type's `one_of`, `constraint`, `scale` and `format` constrain
//!   stored values, so changing them is not a declaration either.

use std::collections::BTreeSet;

use lakelands_api::error::{Beyond, Case, Names, Naming, Place, Said};
use lakelands_api::expr::kind::{self, Kind};
use lakelands_api::expr::{Condition, Expr, Func};
use lakelands_api::ontology::{
    Authority, Cardinality, ConstraintDecl, Derived, FieldDecl, FunctionInput, OnInputChange,
    RelationDecl, StorageKind, TypeDecl, ValueTypeDecl, VerbEffect, VerbScope, ViewDecl,
    lands_in_the_fold,
};
use lakelands_api::tools::{NameProblem, name_problems};
use lakelands_api::write::{
    Carrier, Declaration, DeclarationKind, FunctionDecl, ObjectSetDecl, SettingDecl, Withdrawal,
};
use lakelands_api::{Error, FieldName, Refusal, RelationName, Result, TypeName, VerbName};
use lakelands_core::expr::{Place as Eval, check, parse};
use lakelands_core::rights::is_system;
use lakelands_core::shape::{self, Change};
use lakelands_core::{derive, invalid, links, names, value};

use crate::ontology::Ontology;

/// A refusal of `field`'s value as not well-formed.
pub(crate) fn invalid_field(field: &FieldName, said: impl Into<Said>) -> Error {
    Error::Refused {
        field: Some(field.clone()),
        reason: Refusal::Invalid(Box::new(said.into())),
    }
}

/// `said`, inside the places `at`, outermost first.
pub(crate) fn at(at: &[Place], said: impl Into<Said>) -> Said {
    let mut said = said.into();
    said.at.splice(0..0, at.iter().cloned());
    said
}

fn name(what: Naming, n: &str) -> Result<()> {
    names::fault(what, n).map_or(Ok(()), |c| Err(invalid(c)))
}

fn of_type(name: &TypeName) -> Place {
    let name = name.clone();
    Place::Type { name }
}

fn of_field(name: &FieldName) -> Place {
    let name = name.clone();
    Place::Field { name }
}

/// Whether `t`'s view comes back to `t` by following every source a view
/// reads, its `source` and its merged ones, through `o`, in which `t`
/// stands for its declared self: the chain, `t` first and last, if it
/// does. A cycle that does not pass through `t` is an earlier one, the
/// reader's to refuse.
fn source_cycle(o: &Ontology, t: &TypeDecl, v: &ViewDecl) -> std::result::Result<(), Names> {
    fn walk<'a>(
        o: &'a Ontology,
        t: &'a TypeDecl,
        v: &'a ViewDecl,
        path: &mut Vec<&'a TypeName>,
        seen: &mut BTreeSet<&'a TypeName>,
    ) -> bool {
        let sources = std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source));
        for next in sources {
            if *next == t.name {
                path.push(&t.name);
                return true;
            }
            if !seen.insert(next) {
                continue;
            }
            let Some(view) = o.ty(next).ok().and_then(|u| u.view.as_deref()) else {
                continue;
            };
            path.push(next);
            if walk(o, t, view, path, seen) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut chain: Vec<&TypeName> = vec![&t.name];
    if !walk(o, t, v, &mut chain, &mut BTreeSet::new()) {
        return Ok(());
    }
    Err(chain.into_iter().collect())
}

/// The link types of a ring `t` closes, from the link type of one of its
/// relations to the one whose end it is, each the end of the one before
/// (lakelands#919). A link of a ring needs a live link of the next one as
/// its end, so none can be written: the declaration that closes it is
/// refused, whether by a type, a set or a migration.
fn link_ring(o: &Ontology, t: &TypeDecl) -> Option<Names> {
    for r in &t.relations {
        let start = links::link_type_name(&t.name, &r.name);
        let (mut ring, mut end) = (vec![start.clone()], r.target.clone());
        while let Some((owner, relation)) = links::link_type_parts(&end) {
            if end == start {
                return Some(ring.into_iter().collect());
            }
            if ring.contains(&end) {
                break;
            }
            ring.push(end.clone());
            let owner = (owner == t.name).then_some(t).or_else(|| o.ty(&owner).ok());
            let next = owner.and_then(|d| d.relations.iter().find(|x| x.name == relation));
            let Some(next) = next else { break };
            end = next.target.clone();
        }
    }
    None
}

/// The field `g` of `source` a view groups by, one a key may be, since the
/// grouping values are the view record's key; refused naming it at `at`.
fn grouped_field<'a>(source: &'a TypeDecl, g: &FieldName, on: &[Place]) -> Result<&'a FieldDecl> {
    let f = source.fields.iter().find(|f| &f.name == g).ok_or_else(|| {
        let (ty, field) = (source.name.clone(), g.clone());
        invalid(at(on, Case::UnknownField { ty, field }))
    })?;
    if !lakelands_core::log::keyable(f.ty.kind) {
        let field = g.clone();
        return Err(invalid(at(on, Case::GroupUnkeyable { field })));
    }
    Ok(f)
}

/// The declared type a declaration reads, acts on, watches or holds:
/// refused at `on`, naming the type, when it is not declared.
fn declared<'a>(o: &'a Ontology, name: &TypeName, on: &[Place]) -> Result<&'a TypeDecl> {
    let ty = name.clone();
    o.ty(name)
        .map_err(|_| invalid(at(on, Case::Undeclared { ty })))
}

/// A view's source may not be a view with parameters: only a read that
/// names a view binds them, and a view reading another names it to no one.
fn unbound_source(on: &[Place], source: &TypeDecl) -> Result<()> {
    match source.view.as_deref().and_then(|v| v.params.first()) {
        Some(p) => {
            let (source, param) = (source.name.clone(), p.name.clone());
            Err(invalid(at(on, Case::ReadsParamView { source, param })))
        }
        None => Ok(()),
    }
}

/// A view's merged sources: each declared, read once and not as the view's
/// own source, not a view with parameters, grouped by as many fields as the
/// view, each of the kind of the main source's field it lines up with and
/// one a key may be, with measures named uniquely in the view. The
/// parameters their expressions read are added to `read`.
fn merged_sources(
    o: &Ontology,
    owner: &[Place],
    v: &ViewDecl,
    main: &TypeDecl,
    read: &mut BTreeSet<String>,
) -> Result<()> {
    let mut names: Vec<&FieldName> = v.measures.iter().map(|m| &m.name).collect();
    for (i, m) in v.merge.iter().enumerate() {
        let merged = Place::MergedSource {
            name: m.source.clone(),
        };
        let on = [owner, &[merged]].concat();
        if m.source == v.source || v.merge[..i].iter().any(|n| n.source == m.source) {
            let source = m.source.clone();
            return Err(invalid(at(&on, Case::SourceTwice { source })));
        }
        let source = declared(o, &m.source, &on)?;
        unbound_source(owner, source)?;
        if m.group_by.len() != v.group_by.len() {
            let (given, view) = (m.group_by.len(), v.group_by.len());
            return Err(invalid(at(&on, Case::MergedGroupCount { given, view })));
        }
        for (g, to) in m.group_by.iter().zip(&v.group_by) {
            let kind = grouped_field(source, g, &on)?.ty.kind;
            if main
                .fields
                .iter()
                .find(|f| &f.name == to)
                .map(|f| f.ty.kind)
                != Some(kind)
            {
                let (field, view_field) = (g.clone(), to.clone());
                return Err(invalid(at(
                    &on,
                    Case::MergedGroupKind { field, view_field },
                )));
            }
        }
        if let Some(w) = &m.r#where {
            let on = [&on[..], &[Place::Where]].concat();
            let e = record_stands(source.view.is_none(), condition(w, Eval::View, &on)?, &on)?;
            lakelands_core::expr::params_read(&e, read);
        }
        for x in &m.measures {
            name(Naming::Measure, x.name.as_str())?;
            let name = x.name.clone();
            let on = [&on[..], &[Place::Measure { name }]].concat();
            let e = condition(&x.expr, Eval::Group, &on)?;
            let e = record_stands(source.view.is_none(), e, &on)?;
            // A merged measure measures its source's group; one with no
            // aggregate would read nothing of it.
            if !lakelands_core::expr::calls(&e, &Func::is_aggregate) {
                let source = m.source.clone();
                return Err(invalid(at(&on, Case::MeasuresNothing { source })));
            }
            lakelands_core::expr::params_read(&e, read);
        }
        names.extend(m.measures.iter().map(|x| &x.name));
    }
    if let Some(n) = names
        .iter()
        .enumerate()
        .find(|(i, n)| names[..*i].contains(n))
        .map(|(_, n)| n)
    {
        let name = (*n).clone();
        return Err(invalid(at(owner, Case::MeasureTwice { name })));
    }
    Ok(())
}

/// A view's `key` names what its records are keyed by, its `group_by`,
/// field for field in order, or is left out. Any other would describe
/// records the view does not make, and a rule that reads a type's key
/// would judge a field no record of the view carries as one.
fn view_key(owner: &[Place], t: &TypeDecl, v: &ViewDecl) -> Result<()> {
    if t.key.is_empty() || t.key == v.group_by {
        return Ok(());
    }
    let group_by = (!v.group_by.is_empty()).then(|| v.group_by.iter().collect());
    Err(invalid(at(owner, Case::ViewKey { group_by })))
}

/// A view's declared fields are exactly what it computes: a field for each
/// `group_by` field and each measure, in which its value lands, and no
/// field that nothing fills (028). A read answers declared fields only, so
/// a computed value with no field would be dropped without a word.
fn view_fields(owner: &[Place], t: &TypeDecl, v: &ViewDecl) -> Result<()> {
    let computed: Vec<&FieldName> = (v.group_by.iter())
        .chain(v.measures.iter().map(|m| &m.name))
        .chain(
            v.merge
                .iter()
                .flat_map(|m| m.measures.iter().map(|x| &x.name)),
        )
        .collect();
    if let Some(n) = computed
        .iter()
        .find(|n| !t.fields.iter().any(|f| &f.name == **n))
    {
        let field = (*n).clone();
        return Err(invalid_field(
            n,
            at(owner, Case::ViewValueNoField { field }),
        ));
    }
    if let Some(f) = t.fields.iter().find(|f| !computed.contains(&&f.name)) {
        let field = f.name.clone();
        return Err(invalid_field(
            &f.name,
            at(owner, Case::ViewFieldUnfilled { field }),
        ));
    }
    Ok(())
}

/// A view's parameters: each named by the rule every name follows, once,
/// of a scalar kind, and read by its `where` or a measure; and every
/// `param('name')` they hold, `read`, names one of them.
fn view_params(owner: &[Place], v: &ViewDecl, read: &BTreeSet<String>) -> Result<()> {
    for (i, p) in v.params.iter().enumerate() {
        name(Naming::Parameter, p.name.as_str())?;
        if v.params[..i].iter().any(|q| q.name == p.name) {
            let (what, name) = (Naming::Parameter, p.name.to_string());
            return Err(invalid(at(owner, Case::DeclaredTwice { what, name })));
        }
        if !matches!(
            p.kind,
            StorageKind::Text
                | StorageKind::Number
                | StorageKind::Decimal
                | StorageKind::Boolean
                | StorageKind::DateTime
                | StorageKind::Date
        ) {
            let (name, kind) = (p.name.clone(), p.kind);
            return Err(invalid(at(owner, Case::ParamKind { name, kind })));
        }
        if !read.contains(p.name.as_str()) {
            let name = p.name.clone();
            return Err(invalid(at(owner, Case::ParamUnread { name })));
        }
    }
    if let Some(unknown) = read
        .iter()
        .find(|n| !v.params.iter().any(|p| p.name.as_str() == n.as_str()))
    {
        let declared = Some(v.params.iter().map(|p| &p.name).collect::<Names>());
        let declared = declared.filter(|d| !d.0.is_empty());
        let name = unknown.clone();
        return Err(invalid(at(owner, Case::ParamUndeclared { name, declared })));
    }
    Ok(())
}

/// A condition in its typed form, checked for its place, refused said at
/// `on`.
pub fn condition(c: &Condition, place: Eval, on: &[Place]) -> Result<Expr> {
    let e = typed(c, place).map_err(|c| invalid(at(on, c)))?;
    // A set is named only where its filter is put in the name's place.
    if lakelands_core::expr::calls(&e, &|f| f == Func::InSet) {
        return Err(invalid(at(on, Case::InSetHere)));
    }
    Ok(e)
}

/// `e`, checked at `on`, refused where it reads a record's provenance and
/// none `stands`: over a value, a function's inputs or a view's records,
/// declared or read (028, extended).
pub(crate) fn record_stands(stands: bool, e: Expr, on: &[Place]) -> Result<Expr> {
    let reads = |f| matches!(f, Func::KnownAt | Func::SeenAt);
    match stands || !lakelands_core::expr::calls(&e, &reads) {
        true => Ok(e),
        false => Err(invalid(at(on, Case::NoRecordsProvenance))),
    }
}

/// Whether the filter of `s` names the set `target`, itself or through
/// the sets it names, as `o` holds them.
pub fn names_set(o: &Ontology, s: &ObjectSetDecl, target: &str) -> bool {
    let asked = std::cell::Cell::new(false);
    let lookup = |n: &str| {
        asked.set(asked.get() || n == target);
        o.set(n)
    };
    let _ = set_filter(&lookup, s, &[]);
    asked.get()
}

/// A condition parsed and checked for its place, the sets it names left
/// as it names them.
pub(crate) fn typed(c: &Condition, place: Eval) -> std::result::Result<Expr, Case> {
    let e = match c {
        Condition::Expr(e) => e.clone(),
        Condition::Text(t) => parse(t).map_err(|e| {
            let (text, why) = (t.clone(), Box::new(e.into()));
            Case::NotParsed { text, why }
        })?,
    };
    check(&e, place)?;
    Ok(e)
}

/// A condition over the records of `ty` that may name object sets
/// (`in_set('name')`): each named set's filter stands in the call's place.
/// `set` answers a set by its name, as its caller holds it. A set it does
/// not answer is one that is not declared; one of another type and one
/// that names itself through others are refused. Only a set's `where`, a
/// trigger's `when` and a read's `where` are read this way; anywhere else
/// `in_set` evaluates to its refusal.
pub fn filter<'s>(
    set: &dyn Fn(&str) -> Option<&'s ObjectSetDecl>,
    ty: &TypeName,
    c: &Condition,
    place: Eval,
    on: &[Place],
) -> Result<Expr> {
    filter_kept(set, &|_| false, ty, c, place, on)
}

/// [`filter`], each set `kept` names left as `in_set('name')`, answered by
/// its kept membership where the condition is evaluated: a read's `in_set`
/// of a set across an edge (055, extended).
pub fn filter_kept<'s>(
    set: &dyn Fn(&str) -> Option<&'s ObjectSetDecl>,
    kept: &dyn Fn(&str) -> bool,
    ty: &TypeName,
    c: &Condition,
    place: Eval,
    on: &[Place],
) -> Result<Expr> {
    let e = typed(c, place).map_err(|c| invalid(at(on, c)))?;
    let named = |name: &str| {
        let set_name = name.to_owned();
        match set(name) {
            Some(s) if &s.of == ty => typed(&s.filter, place).map_err(Said::from),
            Some(s) => {
                let (of, ty) = (s.of.clone(), ty.clone());
                Err(Case::SetOfOtherType {
                    set: set_name,
                    of,
                    ty,
                }
                .into())
            }
            None => Err(Case::SetUndeclared { set: set_name }.into()),
        }
    };
    lakelands_api::expr::expand_kept(&e, &named, kept).map_err(|s| invalid(at(on, s)))
}

/// A set's `where`, the sets it names standing in it, as a write decides
/// a membership: in a verb's place, where `exists` has meaning and a path
/// steps across an edge (055, extended).
pub fn set_filter<'s>(
    set: &dyn Fn(&str) -> Option<&'s ObjectSetDecl>,
    s: &ObjectSetDecl,
    on: &[Place],
) -> Result<Expr> {
    filter(set, &s.of, &s.filter, Eval::Verb, on)
}

/// The kind an expression certainly computes over `decl`'s records, or the
/// refusal of one that can never be evaluated over them (028, extended).
/// A failure that depends on one record's value is not known here, and
/// stays that record's: it does not match, or its value is null.
pub fn evaluates(e: &Expr, decl: &TypeDecl, on: &[Place]) -> Result<Option<Kind>> {
    evaluates_across(&|_| None, e, decl, on)
}

/// As [`evaluates`], where a path steps through a reference to a type
/// `far` answers, as a verb's `requires` and values read.
fn evaluates_across<'d>(
    far: &dyn Fn(&TypeName) -> Option<&'d TypeDecl>,
    e: &Expr,
    decl: &'d TypeDecl,
    on: &[Place],
) -> Result<Option<Kind>> {
    let field = |p: &[FieldName]| {
        let derived = decl.derived_step(&p[0]).and_then(|r| far(&r.target));
        let read = match derived {
            Some(t) => t.stepped(&p[1..], far),
            None => decl.stepped(p, far),
        };
        let steps = read.len() + usize::from(derived.is_some());
        read.last().map(|(_, f)| (f.ty.kind, steps))
    };
    kind::of(e, &field).map_err(|why| invalid(at(on, why)))
}

/// A declared `initial` value stands on an authored field of a type's own
/// alone, and is a value a write of the field would take
/// (`docs/decisions/050-a-field-declares-its-initial-value.md`).
fn initial(o: &Ontology, owner: &[Place], f: &FieldDecl, of: Option<&TypeDecl>) -> Result<()> {
    let Some(v) = &f.initial else {
        return Ok(());
    };
    let names = |t: &TypeDecl| t.relations.iter().any(|r| r.via.as_ref() == Some(&f.name));
    let why = match of {
        None => Case::InitialNotOfType,
        Some(t) if t.view.is_some() => Case::InitialInView,
        Some(_)
            if !matches!(f.authority, Authority::Authored | Authority::Verb)
                || f.derived.is_some() =>
        {
            Case::InitialNotAuthored
        }
        Some(t) if t.key.contains(&f.name) => Case::InitialKey,
        Some(t) if f.ty.kind == StorageKind::Reference || names(t) => Case::InitialReference,
        Some(_) if f.on_input_change == OnInputChange::Clear => Case::InitialCleared,
        Some(t) => {
            let patch = [(f.name.clone(), v.clone())].into();
            return crate::writes::check_values(o, t, &patch, std::slice::from_ref(f)).map(drop);
        }
    };
    let on = [owner, &[of_field(&f.name), Place::Initial]].concat();
    Err(invalid_field(&f.name, at(&on, why)))
}

/// `erasable` stands on a type's own field that a source writes and that
/// is neither a key nor names a record, and a field once erasable stays
/// so (`docs/decisions/058-a-source-may-be-forgotten.md`).
fn erasable(o: &Ontology, owner: &[Place], f: &FieldDecl, of: Option<&TypeDecl>) -> Result<()> {
    let was = |t: &TypeDecl| {
        let now = o.types.get(&t.name).map(|c| &c.decl.fields);
        now.is_some_and(|fs| fs.iter().any(|g| g.name == f.name && g.erasable))
    };
    let names = |t: &TypeDecl| t.relations.iter().any(|r| r.via.as_ref() == Some(&f.name));
    let why = match of {
        Some(t) if !f.erasable && was(t) => Case::ErasableDropped,
        _ if !f.erasable => return Ok(()),
        None => Case::ErasableNotOfType,
        Some(_) if !matches!(f.authority, Authority::Source | Authority::Shared) => {
            Case::ErasableAuthored
        }
        Some(t) if t.key.contains(&f.name) => Case::ErasableKey,
        Some(t) if f.ty.kind == StorageKind::Reference || names(t) => Case::ErasableReference,
        Some(_) => return Ok(()),
    };
    let on = [owner, &[of_field(&f.name), Place::Erasable]].concat();
    Err(invalid_field(&f.name, at(&on, why)))
}

/// A field only verbs write (lakelands#1130) is no key, required only with
/// an `initial`, as a verb writes a record that stands, and not computed.
fn verb_written(owner: &[Place], f: &FieldDecl, of: Option<&TypeDecl>) -> Result<()> {
    let why = match of {
        _ if f.authority != Authority::Verb => return Ok(()),
        _ if f.derived.is_some() => Case::VerbComputed,
        Some(t) if t.key.contains(&f.name) => Case::VerbKey,
        Some(_) if f.required && f.initial.is_none() => Case::VerbRequired,
        _ => return Ok(()),
    };
    let on = [owner, &[of_field(&f.name), Place::Authority]].concat();
    Err(invalid_field(&f.name, at(&on, why)))
}

/// `admits` stands on an authored field of a type's own records that keeps
/// per-field provenance, naming each source once, in its form (069).
fn admitted(owner: &[Place], f: &FieldDecl, of: Option<&TypeDecl>) -> Result<()> {
    let mut named = BTreeSet::new();
    let misnamed =
        (f.admits.iter()).find_map(|s| match (crate::writes::formed(s), named.insert(s)) {
            (false, _) => Some(Case::SourceIdForm),
            (true, false) => Some(Case::AdmitsTwice { source: s.clone() }),
            (true, true) => None,
        });
    let why = match of {
        _ if f.admits.is_empty() => return Ok(()),
        None => Case::AdmitsNotOfType,
        Some(t) if t.view.is_some() => Case::AdmitsNotOfType,
        Some(_) if f.authority != Authority::Authored => Case::AdmitsNotAuthored,
        Some(t) if !t.field_meta => Case::AdmitsNoProvenance,
        Some(_) => match misnamed {
            Some(case) => case,
            None => return Ok(()),
        },
    };
    let on = [owner, &[of_field(&f.name), Place::Admits]].concat();
    Err(invalid_field(&f.name, at(&on, why)))
}

/// Refuse `computed_at` on a field no record holds: a verb's payload's, a
/// function's input.
fn computed_here(on: &[Place], f: &FieldDecl) -> Result<()> {
    match f.computed_at {
        Some(_) => {
            let on = [on, &[of_field(&f.name)]].concat();
            Err(invalid_field(&f.name, at(&on, Case::ComputedAtNoRecord)))
        }
        None => Ok(()),
    }
}

/// `of` is the type a field is one of, `None` for a relation's field, a
/// verb's payload and a function's input, which declare no `initial`.
fn field_type(
    o: &Ontology,
    owner: &[Place],
    f: &FieldDecl,
    pending: &TypeName,
    of: Option<&TypeDecl>,
) -> Result<()> {
    name(Naming::Field, f.name.as_str())?;
    initial(o, owner, f, of)?;
    erasable(o, owner, f, of)?;
    verb_written(owner, f, of)?;
    admitted(owner, f, of)?;
    let ty = &f.ty;
    let on = [owner, &[of_field(&f.name)]].concat();
    let wrong = |c: Case| invalid_field(&f.name, at(&on, c));
    match (ty.kind, &ty.target, ty.dimensions) {
        (StorageKind::Reference, None, _) => return Err(wrong(Case::ReferenceNoTarget)),
        (StorageKind::Reference, Some(t), _) if t != pending && !o.types.contains_key(t) => {
            return Err(wrong(Case::Undeclared { ty: t.clone() }));
        }
        (StorageKind::Vector, _, None | Some(0)) => return Err(wrong(Case::VectorDimensions)),
        (k, Some(_), _) if k != StorageKind::Reference => {
            return Err(wrong(Case::TargetNotReference));
        }
        (k, _, Some(_)) if k != StorageKind::Vector => {
            return Err(wrong(Case::DimensionsNotVector));
        }
        _ => {}
    }
    if ty.value_type.is_some() {
        value::conformance(f, &o.value_type_list()).map_err(wrong)?;
    }
    if let Some(d) = &f.derived {
        condition(d, Eval::Write, &on)?;
    }
    if let Some(func) = &f.function {
        if f.derived.is_some() {
            return Err(wrong(Case::DerivedAndFunction));
        }
        if !o.functions.contains_key(func.as_str()) {
            return Err(wrong(Case::FunctionUndeclared { name: func.clone() }));
        }
        if f.authority != lakelands_api::ontology::Authority::Derived {
            return Err(wrong(Case::FunctionNotDerived));
        }
    }
    Ok(())
}

fn type_decl(o: &Ontology, t: &TypeDecl) -> Result<()> {
    name(Naming::Type, t.name.as_str())?;
    let owner = [of_type(&t.name)];
    let has = |f: &FieldName| t.fields.iter().any(|d| &d.name == f);
    let mut seen = BTreeSet::new();
    // A view is not searched: its records are computed on read, and the
    // indexes a search reads hold stored records only (018).
    if t.view.is_some()
        && let Some(f) = t
            .fields
            .iter()
            .find(|f| f.searchable || f.embed.is_some() || f.ty.kind == StorageKind::Vector)
    {
        let (searchable, embedded) = (f.searchable, f.embed.is_some());
        let name = t.name.clone();
        let on = [Place::View { name }, of_field(&f.name)];
        let case = Case::ViewSearched {
            searchable,
            embedded,
        };
        return Err(invalid_field(&f.name, at(&on, case)));
    }
    // A view's records are computed and never retired, so the mark of who
    // retires them means nothing there (lakelands#1148).
    if t.view.is_some() && !t.retired_by.is_authors() {
        return Err(invalid(at(&owner, Case::RetiredByView)));
    }
    if let Some((field, why)) = t.embedding_problem() {
        return Err(invalid_field(&field, why));
    }
    if let Some((field, why)) = t.computed_at_problem(|n| o.functions.get(n).map(|e| &e.decl)) {
        return Err(invalid_field(&field, why));
    }
    for f in &t.fields {
        if !seen.insert(&f.name) {
            let (what, name) = (Naming::Field, f.name.to_string());
            return Err(invalid_field(
                &f.name,
                at(&owner, Case::DeclaredTwice { what, name }),
            ));
        }
        field_type(o, &owner, f, &t.name, Some(t))?;
        let on = [of_type(&t.name), of_field(&f.name)];
        if let Some(func) = f
            .function
            .as_ref()
            .and_then(|n| o.functions.get(n.as_str()))
        {
            // A function computes a field from inputs the type holds, each
            // of the kind the function declares for it, or of any kind for
            // an input that declares none.
            for input in &func.decl.inputs {
                let (function, ty) = (func.decl.name.clone(), t.name.clone());
                let why = match t.fields.iter().find(|d| d.name == *input.name()) {
                    Some(_) if input_kind_held(t, input).is_none() => continue,
                    Some(d) => {
                        let (input, kind, held) =
                            (input.name().clone(), input.field.ty.kind, d.ty.kind);
                        at(
                            &on,
                            Case::FunctionInputKind {
                                function,
                                input,
                                kind,
                                ty,
                                held,
                            },
                        )
                    }
                    None => {
                        let field = input.name().clone();
                        let on = [&on[..], &[Place::Function { name: function }]].concat();
                        at(&on, Case::UnknownField { ty, field })
                    }
                };
                return Err(invalid_field(&f.name, why));
            }
        }
        for d in &f.depends_on {
            if !has(d) {
                let (ty, field) = (t.name.clone(), d.clone());
                return Err(invalid_field(
                    &f.name,
                    at(&on, Case::UnknownField { ty, field }),
                ));
            }
        }
        if f.ty.kind == StorageKind::Json
            && (o.types.contains_key(f.name.as_str()) || t.name.as_str() == f.name.as_str())
        {
            return Err(invalid_field(&f.name, at(&owner, Case::JsonNamedAsType)));
        }
    }
    for k in t.key.iter().chain(&t.valid_from).chain(&t.valid_to) {
        if !has(k) {
            let (ty, field) = (t.name.clone(), k.clone());
            return Err(invalid_field(
                k,
                at(&owner, Case::UnknownField { ty, field }),
            ));
        }
    }
    for k in &t.key {
        let kind = t.fields.iter().find(|f| &f.name == k).map(|f| f.ty.kind);
        if kind.is_some_and(|kind| !lakelands_core::log::keyable(kind)) {
            let field = k.clone();
            return Err(invalid_field(k, at(&owner, Case::KeyUnkeyable { field })));
        }
    }
    // A valid time is a stored instant or date (028, extended): a field of
    // another kind would set a record's validity by a value that only reads
    // as one.
    for (axis, k) in [("valid_from", &t.valid_from), ("valid_to", &t.valid_to)] {
        let kind = |k: &FieldName| t.fields.iter().find(|f| &f.name == k).map(|f| f.ty.kind);
        if let Some((k, kind)) = k.as_ref().and_then(|k| Some((k, kind(k)?)))
            && !matches!(kind, StorageKind::DateTime | StorageKind::Date)
        {
            let (field, axis) = (k.clone(), axis.to_owned());
            let case = Case::ValidTimeNotTime { field, axis, kind };
            return Err(invalid_field(k, at(&owner, case)));
        }
    }
    // A valid time has a start (028, extended): a read at `valid_at`
    // filters by `valid_to` only beside one. A type in force that holds
    // one alone, as its author is shown it, keeps one.
    if let (None, Some(to)) = (&t.valid_from, &t.valid_to) {
        let held = o.types.get(&t.name).map(|e| &e.decl);
        if !held.is_some_and(|h| h.valid_from.is_none() && h.valid_to.is_some()) {
            let case = Case::ValidToAlone { field: to.clone() };
            return Err(invalid_field(to, at(&owner, case)));
        }
    }
    // Before any relation is judged: one its author is not shown, put back
    // under a name the author sends again, is refused as any relation
    // declared twice (024, extended). Its inverse is judged below.
    let mut named = BTreeSet::new();
    if let Some(r) = t.relations.iter().find(|r| !named.insert(&r.name)) {
        let (what, name) = (Naming::Relation, r.name.to_string());
        return Err(invalid(at(&owner, Case::DeclaredTwice { what, name })));
    }
    for r in &t.relations {
        name(Naming::Relation, r.name.as_str())?;
        let name = r.name.clone();
        let on = [of_type(&t.name), Place::Relation { name }];
        let wrong = |c: Case| invalid(at(&on, c));
        // A view's records are computed, never written: no link names one
        // as its live `from`, and it has no stored field for `via` to
        // read. Only a relation derived from matching values reaches out.
        if t.view.is_some() && r.derived.is_none() {
            let (ty, carrier) = (t.name.clone(), carrier(r));
            return Err(wrong(Case::ViewRelation { ty, carrier }));
        }
        // A relation is carried by its `via` field or by link records,
        // never both (`docs/decisions/035-a-link-is-a-record.md`).
        if r.via.is_some() && !r.fields.is_empty() {
            return Err(wrong(Case::ViaWithFields));
        }
        if !matches!(r.authority, Authority::Authored | Authority::Source) {
            let authority = r.authority;
            return Err(wrong(Case::LinkAuthority { authority }));
        }
        if r.authority == Authority::Source
            && let Some(f) = r
                .fields
                .iter()
                .find(|f| !matches!(f.authority, Authority::Source | Authority::Derived))
        {
            let (field, authority) = (f.name.clone(), f.authority);
            let case = Case::LinkFieldAuthority { field, authority };
            return Err(invalid_field(&f.name, at(&on, case)));
        }
        if let Some(f) = r
            .fields
            .iter()
            .find(|f| f.name.as_str() == links::FROM || f.name.as_str() == links::TO)
        {
            let field = f.name.clone();
            return Err(wrong(Case::LinkFieldIsEnd { field }));
        }
        if let Some(before) = o
            .types
            .get(&t.name)
            .and_then(|e| e.decl.relations.iter().find(|b| b.name == r.name))
            && carrier(before) != carrier(r)
        {
            let (from, to) = (carrier(before), carrier(r));
            return Err(wrong(Case::CarrierChanged { from, to }));
        }
        if r.target != t.name && !o.types.contains_key(&r.target) {
            let ty = r.target.clone();
            return Err(wrong(Case::Undeclared { ty }));
        }
        if let Some(via) = &r.via
            && !has(via)
        {
            let (ty, field) = (t.name.clone(), via.clone());
            return Err(wrong(Case::UnknownField { ty, field }));
        }
        if let Some(d) = &r.derived {
            derived(o, t, r, d, &on)?;
        }
        for f in &r.fields {
            let pending = links::link_type_name(&t.name, &r.name);
            field_type(o, &on, f, &pending, None)?;
        }
    }
    // An inverse names one relation onto its target (044, extended): a
    // step by it from there is read as that relation walked the other way.
    // A second is refused at the type its author sent, naming the inverse
    // and the target it sent and no relation: the other may be one the
    // author is not shown, on this type or another, and that one exists
    // is told on purpose. A relation the type holds as it stands is not
    // judged again.
    let others = o.types.values().filter(|e| e.decl.name != t.name);
    let theirs: Vec<_> = others.flat_map(|e| &e.decl.relations).collect();
    let held = o.types.get(&t.name).map(|e| &e.decl.relations);
    for r in &t.relations {
        let same = |x: &RelationDecl| (&x.inverse, &x.target) == (&r.inverse, &r.target);
        let stood = held.is_some_and(|h| h.iter().any(|x| x.name == r.name && same(x)));
        let mine = t.relations.iter().filter(|x| x.name != r.name);
        let twice = !stood && mine.chain(theirs.iter().copied()).any(same);
        if let Some(inverse) = r.inverse.clone().filter(|_| twice) {
            let target = r.target.clone();
            return Err(invalid(at(&owner, Case::InverseTwice { inverse, target })));
        }
    }
    if let Some(ring) = link_ring(o, t) {
        return Err(invalid(at(&owner, Case::LinkTypeRing { ring })));
    }
    if let Some(v) = &t.view {
        source_cycle(o, t, v).map_err(|chain| invalid(at(&owner, Case::ViewCycle { chain })))?;
        let source = declared(o, &v.source, &owner)?;
        unbound_source(&owner, source)?;
        let mut read = BTreeSet::new();
        let stored = source.view.is_none();
        if let Some(w) = &v.r#where {
            let on = [of_type(&t.name), Place::Where];
            let e = record_stands(stored, condition(w, Eval::View, &on)?, &on)?;
            lakelands_core::expr::params_read(&e, &mut read);
        }
        for g in &v.group_by {
            grouped_field(source, g, &owner)?;
        }
        for m in &v.measures {
            name(Naming::Measure, m.name.as_str())?;
            let name = m.name.clone();
            let on = [of_type(&t.name), Place::Measure { name }];
            let e = record_stands(stored, condition(&m.expr, Eval::Group, &on)?, &on)?;
            lakelands_core::expr::params_read(&e, &mut read);
        }
        merged_sources(o, &owner, v, source, &mut read)?;
        // Every field the view reads from a source, in its `where` or in
        // what its measures aggregate, is one that source has (028): the
        // same rule `describe` and the migration preflight read it by.
        for (from, names) in lakelands_core::describe::view_reads(v).unwrap_or_default() {
            let Ok(s) = o.ty(&from) else {
                continue;
            };
            if let Some(n) = names
                .iter()
                .find(|n| !s.fields.iter().any(|f| &f.name == *n))
            {
                let (ty, field) = (from.clone(), n.clone());
                return Err(invalid(at(&owner, Case::UnknownField { ty, field })));
            }
        }
        view_key(&owner, t, v)?;
        view_fields(&owner, t, v)?;
        view_kinds(o, &owner, t, v)?;
        view_params(&owner, v, &read)?;
    }
    // A derived field's expression can be evaluated, and what it computes
    // can land in the field: one that never could would be taken and never
    // filled (028, extended).
    for f in t
        .fields
        .iter()
        .filter(|f| t.view.is_none() && f.derived.is_some())
    {
        let on = [of_type(&t.name), of_field(&f.name)];
        let e = condition(f.derived.as_ref().expect("derived"), Eval::Write, &on)?;
        lands(f, evaluates(&e, t, &on)?, &on)?;
    }
    // Order the fields as the fold will compute them: a field computed by
    // a function reads what that function's expression reads, so it is
    // ordered, and checked for cycles, with the function resolved.
    derive::order(&crate::record::resolve_functions(t, o)).map_err(|e| invalid(at(&owner, e)))?;
    // Its computed fields, derived or computed by a function, read one another
    // without a cycle, so a chain of fills within one record ends
    // (`docs/decisions/022-functions-are-declarations.md`, extended).
    crate::backed::computed_order(o, t).map_err(|e| invalid(at(&owner, e)))?;
    Ok(())
}

/// What an expression certainly computes, `k`, can land in the field `f`:
/// one that never could would be taken and never filled (028, extended).
fn lands(f: &FieldDecl, k: Option<Kind>, on: &[Place]) -> Result<()> {
    let Some(kind) = k.filter(|k| !k.lands(f.ty.kind)) else {
        return Ok(());
    };
    let (holds, duration) = (f.ty.kind, kind == Kind::Duration);
    let case = Case::NeverLands {
        kind,
        holds,
        duration,
    };
    Err(invalid_field(&f.name, at(on, case)))
}

/// What a view computes lands in the field declared for it, as a derived
/// field's value does: each measure, read over its source's fields, and
/// each group value. A name a measure answers under is read as nothing
/// certain, since it may be a measure's and not the field's.
fn view_kinds(o: &Ontology, owner: &[Place], t: &TypeDecl, v: &ViewDecl) -> Result<()> {
    let field = |n: &FieldName| t.fields.iter().find(|f| &f.name == n);
    let merged = v.merge.iter().map(|m| (&m.source, &m.measures));
    let measured: BTreeSet<&FieldName> = (v.measures.iter())
        .chain(v.merge.iter().flat_map(|m| &m.measures))
        .map(|m| &m.name)
        .collect();
    for (source, measures) in std::iter::once((&v.source, &v.measures)).chain(merged) {
        let Ok(s) = o.ty(source) else {
            continue;
        };
        let read = |n: &FieldName| {
            let f = s
                .fields
                .iter()
                .find(|f| &f.name == n && !measured.contains(n));
            f.map(|f| f.ty.kind)
        };
        let by = (v.group_by.iter()).filter(|_| source == &v.source);
        let by = by.map(|g| {
            let on = [owner, &[Place::Group { name: g.clone() }]].concat();
            (g, on, Ok(path(g)))
        });
        let measures = measures.iter().map(|m| {
            let on = [
                owner,
                &[Place::Measure {
                    name: m.name.clone(),
                }],
            ]
            .concat();
            let e = condition(&m.expr, Eval::Group, &on);
            (&m.name, on, e)
        });
        for (name, on, e) in by.chain(measures) {
            if let Some(f) = field(name) {
                let first = |p: &[FieldName]| read(p.first()?).map(|k| (k, 1));
                lands(f, kind::of(&e?, &first).ok().flatten(), &on)?;
            }
        }
    }
    Ok(())
}

/// The expression that reads the field `name`.
fn path(name: &FieldName) -> Expr {
    Expr::Path {
        path: vec![name.clone()],
    }
}

/// A type's relations carried by link records, as the tool generator
/// names their doors.
pub(crate) fn link_relations(t: &TypeDecl) -> Vec<(TypeName, RelationName)> {
    t.relations
        .iter()
        .filter(|r| r.carried_by_links())
        .map(|r| (t.name.clone(), r.name.clone()))
        .collect()
}

/// Whether a host vocabulary type may be declared under its name
/// (`docs/decisions/003-reservation-by-namespace.md`). A name under
/// `sys.link.` belongs to the link types the store derives from relations
/// (035), and the four names of `RESERVED_SYSTEM_TYPES` to the store's own
/// readers; a vocabulary that names one is refused, naming it.
pub fn check_vocabulary_type(t: &TypeDecl) -> Result<()> {
    let link = lakelands_core::links::is_link_type(&t.name);
    if !link && !lakelands_api::seam::RESERVED_SYSTEM_TYPES.contains(&t.name.as_str()) {
        return Ok(());
    }
    let ty = t.name.clone();
    Err(invalid(Case::VocabularyReserved { ty, link }))
}

/// Whether `ty` is a type of a host's vocabulary: a system type that is
/// neither a link type nor one of the store's own four. No caller declares
/// a system type, and the writer's open refuses those from a vocabulary,
/// so the declarations in force tell it, whoever opened the lake.
fn vocabulary_type(ty: &TypeName) -> bool {
    let own = lakelands_api::seam::RESERVED_SYSTEM_TYPES.contains(&ty.as_str());
    is_system(ty) && !links::is_link_type(ty) && !own
}

/// What carries a relation's edges: a `via` field, link records, or
/// nothing stored (derived, 044).
fn carrier(r: &RelationDecl) -> Carrier {
    match (&r.via, &r.derived) {
        (_, Some(_)) => Carrier::Derived,
        (Some(_), None) => Carrier::Via,
        (None, None) => Carrier::Links,
    }
}

/// A derived relation's match: one or more pairs, each a field of the
/// owner and a field of the target of the same storage kind, one a key
/// may be, so that identity of the stored values is what connects them;
/// and no carrier of its own (`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`).
fn derived(o: &Ontology, t: &TypeDecl, r: &RelationDecl, d: &Derived, on: &[Place]) -> Result<()> {
    let wrong = |c: Case| Err(invalid(at(on, c)));
    if r.via.is_some() || !r.fields.is_empty() || r.authority != Authority::Authored {
        return wrong(Case::DerivedStoresEdge);
    }
    if d.pairs.is_empty() {
        return wrong(Case::DerivedNoPair);
    }
    let target = if r.target == t.name {
        Some(t)
    } else {
        o.ty(&r.target).ok()
    };
    for p in &d.pairs {
        let (from, to) = (p.from.clone(), p.to.clone());
        if d.pairs.iter().filter(|q| *q == p).count() > 1 {
            return wrong(Case::PairTwice { from, to });
        }
        let left = t.fields.iter().find(|f| f.name == p.from);
        let right = target.and_then(|u| u.fields.iter().find(|f| f.name == p.to));
        let (Some(left), Some(right)) = (left, right) else {
            let (ty, target) = (t.name.clone(), Box::new(r.target.clone()));
            return wrong(Case::PairUnknown {
                from,
                to,
                ty,
                target,
            });
        };
        let kind = left.ty.kind;
        if kind != right.ty.kind {
            let right = right.ty.kind;
            return wrong(Case::PairKinds {
                from,
                to,
                left: kind,
                right,
            });
        }
        if !lakelands_core::log::keyable(kind) {
            return wrong(Case::PairUnkeyable { from, to, kind });
        }
        if right.at_read() {
            let target = r.target.clone();
            return wrong(Case::PairAtRead { from, to, target });
        }
    }
    // The condition reads a stored field of either record, one step each,
    // and what a read's `where` may call of the record and the clock.
    let Some(c) = &d.filter else { return Ok(()) };
    let e = condition(c, Eval::Record, on)?;
    let mut paths = Vec::new();
    lakelands_api::expr::paths(&e, &mut paths);
    for p in paths {
        let side = match (p.first().map(FieldName::as_str), p.len()) {
            (Some(links::FROM), 2) => Some(t),
            (Some(links::TO), 2) => target,
            _ => {
                let path = p
                    .iter()
                    .map(FieldName::as_str)
                    .collect::<Vec<_>>()
                    .join(".");
                return wrong(Case::DerivedWherePath { path });
            }
        };
        let (Some(side), field) = (side, &p[1]) else {
            continue;
        };
        let ty = side.name.clone();
        match side.fields.iter().find(|f| &f.name == field) {
            None => {
                return wrong(Case::UnknownField {
                    ty,
                    field: field.clone(),
                });
            }
            Some(f) if f.at_read() => {
                return wrong(Case::ReadsAtRead {
                    field: field.clone(),
                    ty,
                });
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// What a declaration reads of its type, the type has (028, extended): the
/// reads the migration preflight keeps a field by.
pub fn reads_its_type(o: &Ontology, d: &Declaration) -> Result<()> {
    let on = match d {
        Declaration::Type(t) => Some(t),
        Declaration::Verb(v) => o.ty(&v.on).ok(),
        Declaration::Trigger(t) => o.ty(&t.on).ok(),
        Declaration::ObjectSet(s) => o.ty(&s.of).ok(),
        _ => None,
    };
    // A verb's path may begin at a relation of its type and go on to the
    // link's fields (055, extended); the relation is judged below. One
    // carried by links begins a path only where `o` holds its link type's
    // `from`: to an author who may not know of the link type, judged over
    // the lake as it is shown, the path names a field the type lacks.
    let link_known = |on: &TypeDecl, r: &RelationDecl| {
        let lt = o.ty(&links::link_type_name(&on.name, &r.name));
        !r.carried_by_links() || lt.is_ok_and(|lt| lt.fields.iter().any(|f| f.name == links::FROM))
    };
    // A set's `where` and a trigger's `when` take the paths a verb's
    // condition takes (055, extended); `exists(r)` reads the relation alone.
    let (paths, exists) = match d {
        Declaration::Verb(v) => (v.record_paths().unwrap_or_default(), v.exists_relations()),
        Declaration::Trigger(_) | Declaration::ObjectSet(_) => {
            let conditions = conditions_of(d).into_iter().filter_map(|c| c.typed().ok());
            let (mut paths, mut exists) = (Vec::new(), Vec::new());
            for e in conditions {
                let mut read = Vec::new();
                lakelands_api::expr::paths(&e, &mut read);
                paths.extend(read.into_iter().map(<[FieldName]>::to_vec));
                lakelands_api::ontology::exists_args(&e, &mut exists);
            }
            (paths, exists)
        }
        _ => Default::default(),
    };
    // A relation's name alone, beside the `exists` that names it, stays
    // a field the type does not have.
    let bare = |n: &FieldName| {
        let alone = paths.iter().filter(|p| p.len() == 1 && &p[0] == n).count();
        alone > exists.iter().filter(|e| *e == n).count()
    };
    let begins_at_relation = |n: &FieldName| match on {
        Some(on) => {
            paths.iter().any(|p| {
                let long = p.len() > 1 || exists.contains(n);
                long && &p[0] == n && related(on, n).is_some_and(|r| link_known(on, r))
            }) && !bare(n)
        }
        None => false,
    };
    // `exists` over a field is no question of a relation; over a name the
    // type has neither as a field nor as a relation it is the unknown
    // field a path through it would be.
    if let Some(on) = on
        && let Some(n) = exists.iter().find(|n| related(on, n).is_none())
    {
        let case = match on.fields.iter().any(|f| &f.name == n) {
            true => Case::ExistsHere,
            false => Case::UnknownField {
                ty: on.name.clone(),
                field: n.clone(),
            },
        };
        return Err(invalid(at(&[d.place()], case)));
    }
    if let Some(on) = on
        && let reads = lakelands_core::describe::declaration_reads(d, &on.name, &|_| None)
        && let Some(n) = (reads.iter())
            .find(|n| !on.fields.iter().any(|f| &f.name == *n) && !begins_at_relation(n))
    {
        let (ty, field) = (on.name.clone(), n.clone());
        return Err(invalid(at(&[d.place()], Case::UnknownField { ty, field })));
    }
    // A verb, a trigger and a set read a record as stored, at a write.
    if let Some(on) = on.filter(|_| !matches!(d, Declaration::Type(_)))
        && let reads = lakelands_core::describe::declaration_reads(d, &on.name, &|_| None)
        && let Some(n) =
            (reads.iter()).find(|n| on.fields.iter().any(|f| &f.name == *n && f.at_read()))
    {
        let (field, ty) = (n.clone(), on.name.clone());
        return Err(invalid(at(&[d.place()], Case::ReadsAtRead { field, ty })));
    }
    // What a verb, a trigger and a set read across a reference, the record
    // it reaches has, as stored.
    let across = matches!(
        d,
        Declaration::Verb(_) | Declaration::Trigger(_) | Declaration::ObjectSet(_)
    );
    let far = |n: &TypeName| o.ty(n).ok().filter(|_| across);
    if let Some(on) = on.filter(|_| across) {
        let wrong = |c| Err(invalid(at(&[d.place()], c)));
        for path in paths.iter().cloned() {
            // A relation reads the one link a record holds under it, so one
            // whose records each hold one link record at most.
            let long = path.len() > 1 || exists.contains(&path[0]);
            if let (Some(r), true) = (related(on, &path[0]), long) {
                let (relation, cardinality) = (r.name.clone(), r.cardinality);
                let many = matches!(
                    cardinality,
                    Cardinality::OneToMany | Cardinality::ManyToMany
                );
                // A derived relation reaches one record at most where its
                // match names every key field of its target, whatever its
                // cardinality says (044 enforces none).
                if let Some(d) = &r.derived {
                    let target = o.ty(&r.target).ok().filter(|t| t.view.is_none());
                    let to: Vec<&FieldName> = d.pairs.iter().map(|p| &p.to).collect();
                    let Some(target) = target
                        .filter(|t| !t.key.is_empty() && t.key.iter().all(|k| to.contains(&k)))
                    else {
                        let target = r.target.clone();
                        return wrong(Case::MatchNotKey { relation, target });
                    };
                    let read = target.stepped(&path[1..], &far);
                    let next = read.last().and_then(|l| l.1.ty.target.as_ref());
                    let next = if read.is_empty() {
                        Some(target)
                    } else {
                        next.and_then(far)
                    };
                    if let Some((t, field)) = next.zip(path.get(1 + read.len())) {
                        let (ty, field) = (t.name.clone(), field.clone());
                        return wrong(Case::UnknownField { ty, field });
                    }
                    if let Some((t, f)) = read.iter().find(|r| r.1.at_read()) {
                        let (field, ty) = (f.name.clone(), t.name.clone());
                        return wrong(Case::ReadsAtRead { field, ty });
                    }
                    continue;
                }
                let case = match (&r.derived, &r.via) {
                    (Some(_), _) => None,
                    (_, Some(field)) => Some(Case::RelationViaField {
                        relation,
                        field: field.clone(),
                    }),
                    _ => many.then_some(Case::RelationHoldsMany {
                        relation,
                        cardinality,
                    }),
                };
                if let Some(case) = case {
                    return wrong(case);
                }
            }
            let read = on.stepped(&path, &far);
            // The next segment is the link's where the path began at a
            // relation, else the far record's of the last reference read.
            let target = read.last().and_then(|l| l.1.ty.target.as_ref());
            let next = match read.as_slice() {
                [(link, _)] if link.name != on.name => Some(*link),
                _ => target.and_then(far),
            };
            if let Some((t, field)) = next.zip(path.get(read.len())) {
                let (ty, field) = (t.name.clone(), field.clone());
                return wrong(Case::UnknownField { ty, field });
            }
            if let Some((t, f)) = read.iter().skip(1).find(|r| r.1.at_read()) {
                let (field, ty) = (f.name.clone(), t.name.clone());
                return wrong(Case::ReadsAtRead { field, ty });
            }
        }
        // A set and a trigger are decided again at a write of the far
        // record, which a value computed outside the lake, and every field
        // computed from one at any depth, lands without: a fill writes no
        // record a path is decided at.
        let function = |n: &str| o.functions.get(n).map(|e| &e.decl);
        let lands = |(t, f): &&(&TypeDecl, &FieldDecl)| {
            lakelands_api::ontology::lands_in_the_fold(&t.fields, f, &function, &mut Vec::new())
        };
        for p in paths.iter().filter(|_| !matches!(d, Declaration::Verb(_))) {
            // A derived relation's match reads the condition's own record
            // first; what follows is the far record's.
            let own = on.derived_step(&p[0]).and_then(|r| r.derived.as_ref());
            let own = own.map_or(0, |d| {
                let [from, _] = d.reads();
                from.iter()
                    .filter(|n| on.fields.iter().any(|f| &f.name == *n))
                    .count()
            });
            let read = on.reads_across(std::slice::from_ref(p), &far);
            if let Some((t, f)) = read.iter().skip(own).find(lands) {
                let (field, ty) = (f.name.clone(), t.name.clone());
                return wrong(Case::SetReadsComputed { field, ty });
            }
        }
    }
    // And each of its conditions can be evaluated over that type's records.
    for c in conditions_of(d) {
        let typed = match c {
            Condition::Expr(e) => Some(e.clone()),
            Condition::Text(t) => parse(t).ok(),
        };
        if let (Some(on), Some(e)) = (on, typed) {
            evaluates_across(&far, &e, on, &[d.place()])?;
        }
    }
    Ok(())
}

/// The conditions of a declaration over its type's records.
fn conditions_of(d: &Declaration) -> Vec<&Condition> {
    match d {
        Declaration::Trigger(t) => t.when.iter().collect(),
        Declaration::ObjectSet(s) => vec![&s.filter],
        Declaration::Verb(v) => match &v.effect {
            VerbEffect::Write { set } => (v.requires.iter())
                .chain(set.iter().map(|a| &a.value))
                .collect(),
            _ => v.requires.iter().collect(),
        },
        _ => vec![],
    }
}

/// The relation of `on` a path's first segment `n` names, where `on` has
/// no field of that name.
fn related<'d>(on: &'d TypeDecl, n: &FieldName) -> Option<&'d RelationDecl> {
    let field = on.fields.iter().any(|f| &f.name == n);
    (on.relations.iter()).find(|r| !field && r.name.as_str() == n.as_str())
}

/// The fields of `ty` a declaration reads
/// ([`lakelands_core::describe::declaration_reads`]), with those a verb
/// reads of it across a reference.
pub fn reads_of(o: &Ontology, d: &Declaration, ty: &TypeName) -> BTreeSet<FieldName> {
    let mut reads = lakelands_core::describe::declaration_reads(d, ty, &|_| None);
    if let Declaration::Verb(v) = d
        && let Ok(on) = o.ty(&v.on)
    {
        let far = v.reads_across(on, &|n| o.ty(n).ok());
        let of_ty = far.iter().flatten().filter(|r| &r.0.name == ty);
        reads.extend(of_ty.map(|r| r.1.name.clone()));
    }
    reads
}

/// Refuse taking to the read a field that a trigger, a verb, an object set
/// or another type's derived relation in force reads: each reads the
/// record as stored. The refusal names the field alone, not what reads it,
/// which its declarer may not be shown.
fn read_as_stored(o: &Ontology, t: &TypeDecl) -> Result<()> {
    if !t.fields.iter().any(FieldDecl::at_read) {
        return Ok(());
    }
    let mut read = BTreeSet::new();
    for d in o.declared().iter().map(|d| &d.declaration) {
        if let Declaration::Verb(_) | Declaration::Trigger(_) | Declaration::ObjectSet(_) = d {
            read.extend(reads_of(o, d, &t.name));
        }
    }
    let others = o.types.values().filter(|e| e.decl.name != t.name);
    let matching = others
        .flat_map(|e| &e.decl.relations)
        .filter(|r| r.target == t.name);
    read.extend(matching.flat_map(|r| r.derived.iter().flat_map(|d| d.reads()[1].clone())));
    match t
        .fields
        .iter()
        .find(|f| f.at_read() && read.contains(&f.name))
    {
        Some(f) => {
            let on = [of_type(&t.name), of_field(&f.name)];
            Err(invalid_field(&f.name, at(&on, Case::ReadAsStored)))
        }
        None => Ok(()),
    }
}

/// Passes the refusal of a reshaping declaration, which a migration is.
pub(crate) fn unless_a_migration(e: Error) -> Result<()> {
    let said = e.case().map(|s| &s.case);
    match said {
        Some(
            Case::Reshapes { .. }
            | Case::ReshapesLinks { .. }
            | Case::ConstraintNeedsMigrate { .. },
        ) => Ok(()),
        _ => Err(e),
    }
}

/// A type's constraints, each named once and reading its record alone: its
/// own fields as stored or computed in the lake at the write, `previous`
/// and `changed`, and nothing that changes without a write of the record
/// (lakelands#1119).
fn constraints(o: &Ontology, t: &TypeDecl) -> Result<()> {
    let mut named = BTreeSet::new();
    for c in &t.constraints {
        let on = [
            of_type(&t.name),
            Place::TypeConstraint {
                name: c.name.clone(),
            },
        ];
        let wrong = |case| Err(invalid(at(&on, case)));
        let constraint = c.name.clone();
        name(Naming::Constraint, constraint.as_str()).map_err(|e| e.within(on[0].clone()))?;
        if !named.insert(&c.name) {
            return wrong(Case::ConstraintNamedTwice { constraint });
        }
        let e = typed(&c.holds, Eval::Guard).map_err(|s| invalid(at(&on, s)))?;
        let mut paths = Vec::new();
        lakelands_api::expr::paths(&e, &mut paths);
        let backed = |n: &str| {
            o.functions
                .get(n)
                .is_some_and(|f| crate::backed::is_backed(&f.decl))
        };
        let beyond = if paths.iter().any(|p| p.len() > 1) {
            Some(Beyond::Path)
        } else if e.calls(&[Func::InSet]) {
            Some(Beyond::InSet)
        } else if e.calls(&[Func::Setting]) {
            Some(Beyond::Setting)
        } else if e.reads_clock() {
            Some(Beyond::Clock)
        } else if e.calls(&[Func::Exists]) {
            Some(Beyond::Exists)
        } else {
            let mut fields = e.fields_read().into_iter();
            fields.find_map(|n| {
                let f = t.fields.iter().find(|f| &f.name == n)?;
                let field = n.clone();
                if f.at_read() {
                    Some(Beyond::ComputedAtRead { field })
                } else if f.embed.is_some() || f.function.as_deref().is_some_and(backed) {
                    Some(Beyond::ComputedOutside { field })
                } else {
                    None
                }
            })
        };
        if let Some(reads) = beyond {
            return wrong(Case::ConstraintReadsBeyond {
                constraint,
                reads: Box::new(reads),
            });
        }
        if let Some(n) = e
            .fields_read()
            .into_iter()
            .find(|n| !t.fields.iter().any(|f| &f.name == *n))
        {
            let (ty, field) = (t.name.clone(), n.clone());
            return wrong(Case::UnknownField { ty, field });
        }
        initial_holds(o, t, c, &e)?;
    }
    Ok(())
}

/// A constraint reading only fields no create can send, those verbs alone
/// write and those computed from them, is judged over what a create leaves
/// of them (lakelands#1130, lakelands#1169): their `initial`s, which no
/// create can replace, and a field with none is null. One an ingest admits
/// a breach of is not: an ingest makes records of the type whatever it
/// holds. Where it reads a field a create can send it is taken: the create
/// must send a value, and `describe` tells it.
fn initial_holds(
    o: &Ontology,
    t: &TypeDecl,
    c: &ConstraintDecl,
    e: &lakelands_api::expr::Expr,
) -> Result<()> {
    let resolved = crate::record::resolve_functions(t, o);
    let fixed = derive::fixed(&resolved);
    let read = e.fields_read();
    if read.is_empty() || !read.iter().all(|n| fixed.contains(*n)) || !c.at_ingest.refuses() {
        return Ok(());
    }
    let value_types: Vec<_> = o.value_types.values().map(|v| v.decl.clone()).collect();
    let broken = derive::breaks(&resolved, &value_types);
    let Some((_, part)) = broken.iter().find(|(b, _)| b.name == c.name) else {
        return Ok(());
    };
    // The fields verbs write that the part that breaks reads, through the
    // computed ones.
    let leaves = derive::leaves(&resolved, part);
    let leaves: Vec<&FieldDecl> = (t.fields.iter())
        .filter(|f| leaves.contains(&f.name))
        .collect();
    let constraint = c.name.clone();
    let (field, case) = match leaves.iter().find(|f| f.initial.is_none()) {
        Some(f) => {
            let field = f.name.clone();
            (field.clone(), Case::NoInitialBreaks { constraint, field })
        }
        None => {
            let field = leaves.first().map_or_else(
                || (**read.first().expect("a field read")).clone(),
                |f| f.name.clone(),
            );
            (field.clone(), Case::InitialBreaks { constraint, field })
        }
    };
    Err(crate::writes::constraint(Some(field), case))
}

/// A new field may not begin with `_`: under a pattern's bound name the
/// lake keeps such names for its own columns, `<name>._depth` (018,
/// extended). A field the type already holds keeps its name, as a lake
/// written before the rule holds it.
fn kept_names(o: &Ontology, t: &TypeDecl) -> Result<()> {
    let held = o.types.get(&t.name).map(|e| &e.decl.fields);
    let new = |f: &&FieldDecl| held.is_none_or(|h| !h.iter().any(|g| g.name == f.name));
    let kept = t
        .fields
        .iter()
        .filter(new)
        .find(|f| f.name.as_str().starts_with('_'));
    match kept {
        Some(f) => {
            let (ty, field) = (t.name.clone(), f.name.clone());
            Err(invalid_field(&f.name, Case::FieldNameKept { ty, field }))
        }
        None => Ok(()),
    }
}

/// Check one declaration against the ontology it would join. `who_is_system`
/// is true only for the store's own declarations.
pub fn check_declaration(o: &Ontology, d: &Declaration, who_is_system: bool) -> Result<()> {
    if let Some(fault) = d.annotation_fault() {
        return Err(fault.into());
    }
    if let Some(said) = d.integer_fault() {
        return Err(invalid(said));
    }
    match d {
        Declaration::Type(t) => {
            if is_system(&t.name) && !who_is_system {
                return Err(crate::writes::refused(Refusal::Reserved));
            }
            type_decl(o, t)?;
            kept_names(o, t)?;
            read_as_stored(o, t)?;
            constraints(o, t)?;
            // A change only a migration may make is told after every other
            // check, on the type and on each of its link types, so that a
            // migration meets them all; the first found, in a fixed order:
            // the type's, each link type's as declared, the constraints'
            // (027, lakelands#1124).
            let mut migrates = None;
            if let Some(current) = o.types.get(&t.name)
                && let Change::Migrate { reasons } = shape::classify(&current.decl, t)
            {
                let ty = t.name.clone();
                migrates = Some(invalid(Case::Reshapes { ty, reasons }));
            }
            // A relation's link type is a type like any: its fields are
            // checked as a type's, with `t` in force (its `from` points
            // there). And it is classified too: a relation dropped by a
            // migration and declared again with another target, or with a
            // field retyped, would reshape the links it holds as history.
            // A type that brings none copies nothing.
            let lts = links::link_types_of(t);
            let mut probe = std::borrow::Cow::Borrowed(o);
            if !lts.is_empty() {
                probe.to_mut().put(d.clone(), 0, None);
            }
            for lt in lts {
                let name = t.name.clone();
                type_decl(&probe, &lt).map_err(|e| e.within(Place::Links { name }))?;
                if let Some(current) = o.types.get(&lt.name)
                    && let Change::Migrate { reasons } = shape::classify(&current.decl, &lt)
                {
                    let (ty, links) = (t.name.clone(), lt.name.clone());
                    let e = || invalid(Case::ReshapesLinks { ty, links, reasons });
                    migrates.get_or_insert_with(e);
                }
            }
            // A constraint added or tightened may be broken by stored
            // records: a migration's preflight lists them. One restated in
            // other words, described otherwise or moved to admit at ingest
            // is not (lakelands#1119).
            let parsed = |c: &ConstraintDecl| typed(&c.holds, Eval::Guard).ok();
            let loosens = |c: &ConstraintDecl, h: &ConstraintDecl| {
                h.name == c.name
                    && parsed(h) == parsed(c)
                    && (h.at_ingest == c.at_ingest || !c.at_ingest.refuses())
            };
            let held = o.types.get(&t.name).map(|e| &e.decl.constraints);
            if let Some(held) = held
                && let Some(c) =
                    (t.constraints.iter()).find(|c| !held.iter().any(|h| loosens(c, h)))
            {
                let (ty, constraint) = (t.name.clone(), c.name.clone());
                migrates.get_or_insert_with(|| {
                    invalid(Case::ConstraintNeedsMigrate { ty, constraint })
                });
            }
            if let Some(e) = migrates {
                return Err(e);
            }
        }
        Declaration::ValueType(v) => {
            name(Naming::ValueType, v.name.as_str())?;
            let on = [Place::ValueType {
                name: v.name.clone(),
            }];
            if let Some(f) = &v.format
                && value::ValueFormat::parse(f).is_none()
            {
                let (value_type, format) = (v.name.clone(), f.clone());
                return Err(invalid(Case::ValueTypeFormat { value_type, format }));
            }
            if v.scale.is_some() && v.kind != StorageKind::Decimal {
                return Err(invalid(at(&on, Case::ScaleNotDecimal)));
            }
            if !v.one_of.is_empty() && v.kind != StorageKind::Text {
                return Err(invalid(at(&on, Case::OneOfNotText)));
            }
            if let Some(c) = &v.constraint {
                let on = [on[0].clone(), Place::Constraint];
                record_stands(false, condition(c, Eval::Record, &on)?, &on)?;
            }
            if let Some(current) = o.value_types.get(v.name.as_str()) {
                let value_type = v.name.clone();
                let why = match value_type_change(&current.decl, v) {
                    ValueTypeChange::Same | ValueTypeChange::Loosening => None,
                    ValueTypeChange::Tightening => Some(Case::ValueTypeTightens { value_type }),
                    ValueTypeChange::Reshaping => Some(Case::ValueTypeReshapes { value_type }),
                };
                if let Some(why) = why {
                    return Err(invalid(why));
                }
            }
        }
        Declaration::Verb(v) => {
            name(Naming::Verb, v.name.as_str())?;
            let on = [Place::Verb {
                name: v.name.clone(),
            }];
            let ty = v.on.clone();
            // A verb on its type has no record: nothing for a write effect
            // to set or a retirement to retire, nothing for `requires` to
            // read (013, extended).
            if v.scope == VerbScope::Type {
                if !matches!(v.effect, VerbEffect::External { .. }) {
                    return Err(invalid(at(&on, Case::TypeVerbWrites { ty })));
                }
                if v.requires.is_some() {
                    return Err(invalid(at(&on, Case::TypeVerbRequires { ty })));
                }
            }
            let t = declared(o, &v.on, &on)?;
            // A link type is one a relation carries. The name a rename or
            // a removal left behind stays declared for the links' history
            // and takes no link (035); to a verb declared now it is a name
            // no type has.
            let types = o.types.values().map(|e| &e.decl);
            if links::is_link_type(&v.on) && links::relation_of(types, &v.on).is_none() {
                return Err(invalid(at(&on, Case::Undeclared { ty })));
            }
            // A link is retired by `unlink`, which judges its relation; a
            // verb does not retire one (035, lakelands#1148).
            if links::is_link_type(&v.on) && matches!(v.effect, VerbEffect::Retire) {
                return Err(invalid(at(&on, Case::RetireVerbLink { ty })));
            }
            // Of the system types, a verb names a relation's link type, and
            // one whose effect is external a type of the host's vocabulary:
            // it sets nothing and hands its effect to the host's executor.
            // Every other is reserved (003) and written through its own door.
            let external = matches!(v.effect, VerbEffect::External { .. });
            let admitted = links::is_link_type(&v.on) || external && vocabulary_type(&v.on);
            if is_system(&v.on) && !admitted && !who_is_system {
                return Err(crate::writes::refused(Refusal::Reserved));
            }
            // A verb on a record of a view could never act: there is no
            // stored record to write or to hand an executor.
            if t.view.is_some() && v.scope == VerbScope::Record {
                return Err(invalid(at(&on, Case::VerbOnView { ty })));
            }
            let payload = [on[0].clone(), Place::Payload];
            for f in &v.payload {
                field_type(o, &payload, f, &v.on, None)?;
                computed_here(&payload, f)?;
            }
            if let Some(r) = &v.requires {
                condition(r, Eval::Verb, &[on[0].clone(), Place::Requires])?;
            }
            match &v.effect {
                VerbEffect::Write { set } => {
                    if set.is_empty() {
                        return Err(invalid(at(&on, Case::WritesNothing)));
                    }
                    for a in set {
                        if !t.fields.iter().any(|f| f.name == a.field) {
                            let (ty, field) = (v.on.clone(), a.field.clone());
                            return Err(invalid(at(&on, Case::UnknownField { ty, field })));
                        }
                        let on = [on[0].clone(), of_field(&a.field)];
                        // A link is keyed by its ends: a verb on a link type
                        // sets the link's own fields, as `link` takes no end
                        // among its fields (035).
                        if links::is_link_type(&v.on)
                            && [links::FROM, links::TO].contains(&a.field.as_str())
                        {
                            let end = a.field.clone();
                            return Err(invalid(at(&on, Case::LinkEndAsField { end })));
                        }
                        let value = condition(&a.value, Eval::Verb, &on)?;
                        // A member the verb's payload does not have is never
                        // given: the value would read null at every call.
                        let (_, members) = lakelands_core::derive::verb_reads(&value);
                        let has = |m: &&FieldName| v.payload.iter().any(|f| &f.name == *m);
                        if let Some(m) = members.iter().find(|m| !has(m)) {
                            let member = m.clone();
                            return Err(invalid(at(&on, Case::PayloadNoMember { member })));
                        }
                    }
                }
                VerbEffect::External { executor } => name(Naming::Executor, executor)?,
                _ => {}
            }
        }
        Declaration::Trigger(t) => {
            name(Naming::Trigger, t.name.as_str())?;
            let on = [Place::Trigger {
                name: t.name.clone(),
            }];
            let ty = t.on.clone();
            // A relation carried by a reference field has no link records,
            // so no link type to watch: its links change with the field.
            if let Some((owner, relation)) = lakelands_core::links::link_type_parts(&t.on)
                && let Ok(decl) = o.ty(&owner)
                && let Some(via) = decl
                    .relations
                    .iter()
                    .find(|r| r.name == relation)
                    .and_then(|r| r.via.as_ref())
            {
                let (via, owner) = (via.clone(), Box::new(owner));
                let case = Case::WatchesViaLinks {
                    ty,
                    relation,
                    via,
                    owner,
                };
                return Err(invalid_field(&"on".into(), at(&on, case)));
            }
            // A trigger on a view fires from the rows the lake keeps of it:
            // whether it keeps them is a setting's, judged at the write
            // (lakelands#1185).
            declared(o, &t.on, &on)?;
            if let Some(w) = &t.when {
                filter(&|n| o.set(n), &t.on, w, Eval::Guard, &on)?;
            }
        }
        Declaration::ObjectSet(s) => {
            name(Naming::ObjectSet, &s.name)?;
            let on = [Place::ObjectSet {
                name: s.name.clone(),
            }];
            declared(o, &s.of, &on)?;
            let ty = s.of.clone();
            // A set is of one type: declared again, it holds that type.
            if let Some(was) = o.object_sets.get(&s.name).filter(|e| e.decl.of != s.of) {
                let was = was.decl.of.clone();
                return Err(invalid_field(
                    &"of".into(),
                    at(&on, Case::SetRetyped { was, ty }),
                ));
            }
            // A set of a view holds the records the lake keeps of it: whether
            // it keeps them is a setting's, judged at the write
            // (lakelands#1185).
            let named = |n: &str| Some(s).filter(|s| s.name == n).or_else(|| o.set(n));
            set_filter(&named, s, &on)?;
        }
        Declaration::Function(f) => {
            name(Naming::Function, &f.name)?;
            name(Naming::Backing, &f.backing)?;
            let on = [Place::Function {
                name: f.name.clone(),
            }];
            // Declared again with other inputs, it must not close a cycle
            // in a type whose field it computes.
            let mut probe = o.clone();
            probe.put(d.clone(), 0, None);
            let callers = || {
                o.types.values().filter(|t| {
                    let links = t.decl.relations.iter().flat_map(|r| &r.fields);
                    (t.decl.fields.iter().chain(links))
                        .any(|x| x.function.as_deref() == Some(f.name.as_str()))
                })
            };
            for t in callers() {
                let within = [on[0].clone(), of_type(&t.decl.name)];
                crate::backed::computed_order(&probe, &t.decl)
                    .map_err(|e| invalid(at(&within, e)))?;
            }
            let input = [on[0].clone(), Place::Input];
            for i in &f.inputs {
                if !i.any_kind {
                    field_type(o, &input, &i.field, &TypeName::from(""), None)?;
                }
                computed_here(&input, &i.field)?;
            }
            // Declared again, a typed input must still be held in its kind by
            // each type that binds it; one that drops its type is always
            // taken.
            if let Some(case) = input_kind_broken(o, f) {
                return Err(invalid(at(&on, case)));
            }
            // A call made in process has nothing to gate (064).
            if f.backing == "expr" && !f.risk.is_low() {
                return Err(invalid(at(&on, Case::ExprRisk)));
            }
            match (f.backing.as_str(), &f.expr) {
                ("expr", Some(e)) => {
                    let e = record_stands(false, condition(e, Eval::Record, &on)?, &on)?;
                    // The expression reads only the inputs it declares, so
                    // the inputs a field is checked against are all it reads.
                    for field in derive::reads(&e) {
                        if !f.inputs.iter().any(|i| *i.name() == field) {
                            return Err(invalid(at(&on, Case::ReadsNoInput { field })));
                        }
                    }
                }
                ("expr", None) => return Err(invalid(at(&on, Case::ExprNoExpression))),
                (_, Some(_)) => {
                    let backing = f.backing.clone();
                    return Err(invalid(at(&on, Case::ExpressionNotExpr { backing })));
                }
                _ => {}
            }
            // Declared again, it must not compute what an expression
            // alone may: with a backing a field at the read that calls it
            // (052); with a backing or an input computed outside the lake
            // a reference that calls it or reads a field that does (032).
            // Each field of a type that calls it is asked, whatever else
            // the type says. Said of the function: the type may not be
            // its author's to know.
            let called = |n: &str| probe.functions.get(n).map(|e| &e.decl);
            for t in callers() {
                let links = t.decl.relations.iter().map(|r| &r.fields);
                for fields in std::iter::once(&t.decl.fields).chain(links) {
                    for x in fields {
                        let calls = x.function.as_deref() == Some(f.name.as_str());
                        let lands = || lands_in_the_fold(fields, x, &called, &mut Vec::new());
                        if x.at_read() && calls && crate::backed::is_backed(f) {
                            return Err(invalid(at(&on, Case::AtReadBacked)));
                        } else if x.ty.target.is_some() && lands() {
                            return Err(invalid(at(&on, Case::ReferenceUnjudged)));
                        }
                    }
                }
            }
        }
        Declaration::Setting(s) => setting_decl(o, s)?,
        Declaration::Withdraw(w) => withdrawal(o, w)?,
        _ => {}
    }
    reads_its_type(o, d)?;
    tool_names(o, d)
}

/// The kind `t` holds the field `input` names in, when `input` states
/// another: an input of no kind is bound by a field of any.
fn input_kind_held(t: &TypeDecl, input: &FunctionInput) -> Option<StorageKind> {
    let kind = input.kind()?;
    let held = t.fields.iter().find(|d| d.name == *input.name())?.ty.kind;
    (held != kind).then_some(held)
}

/// The first input of `f` declared in a kind that a type in `o` computing
/// a field by it holds in another, as that type's own declare would refuse
/// it (`function_input_kind`, naming the type).
pub(crate) fn input_kind_broken(o: &Ontology, f: &FunctionDecl) -> Option<Case> {
    let calls = |t: &TypeDecl| {
        t.fields
            .iter()
            .any(|x| x.function.as_deref() == Some(&*f.name))
    };
    let types = o.types.values().map(|e| &e.decl).filter(|t| calls(t));
    types
        .flat_map(|t| f.inputs.iter().map(move |i| (t, i)))
        .find_map(|(t, i)| {
            let held = input_kind_held(t, i)?;
            let (function, input, kind, ty) = (
                f.name.clone(),
                i.name().clone(),
                i.field.ty.kind,
                t.name.clone(),
            );
            Some(Case::FunctionInputKind {
                function,
                input,
                kind,
                ty,
                held,
            })
        })
}

/// A withdrawal: of a trigger, a verb, an object set or a function in
/// force, which nothing in force reads ([`reader`]).
fn withdrawal(o: &Ontology, w: &Withdrawal) -> Result<()> {
    use DeclarationKind::{Function, ObjectSet, Trigger, Verb};
    let (kind, name) = (w.kind.naming(), w.name.clone());
    if !matches!(w.kind, Trigger | Verb | ObjectSet | Function) {
        return Err(invalid(Case::WithdrawnKind { kind }));
    }
    if o.held_named(w.kind, &w.name).is_none() {
        return Err(invalid(Case::WithdrawnUndeclared { kind, name }));
    }
    match reader(o, w) {
        Some(by) => Err(invalid(Case::WithdrawnInUse { kind, name, by })),
        None => Ok(()),
    }
}

/// The first reader in force of what `w` withdraws, as a refusal names
/// it. Nothing reads a trigger. A function is read by each field it
/// computes; a verb by each field of its type only verbs write that no
/// other verb of the type sets (lakelands#1130), and the last that retires
/// a type only its verbs retire by that type (lakelands#1239); an object
/// set by each set and each trigger whose condition names it, itself or
/// through the sets it names.
pub(crate) fn reader(o: &Ontology, w: &Withdrawal) -> Option<String> {
    match w.kind {
        DeclarationKind::Function => function_reader(o, &w.name),
        DeclarationKind::Verb => verb_reader(o, &VerbName::from(w.name.as_str())),
        DeclarationKind::ObjectSet => set_reader(o, &w.name),
        _ => None,
    }
}

/// The first field only verbs write that `verb` alone of its type's verbs
/// sets: the field `status` of `order`; else its type, where it is the last
/// verb that retires a type only its verbs retire (lakelands#1239).
fn verb_reader(o: &Ontology, verb: &VerbName) -> Option<String> {
    let v = &o.verbs.get(verb)?.decl;
    let t = &o.types.get(&v.on)?.decl;
    let sets = |v: &lakelands_api::ontology::VerbDecl, f: &FieldName| matches!(&v.effect, VerbEffect::Write { set } if set.iter().any(|a| &a.field == f));
    let others = o.verbs.values().map(|e| &e.decl);
    let others = others.filter(|x| x.name != v.name && x.on == t.name);
    let only = |f: &&FieldDecl| {
        f.authority == Authority::Verb
            && sets(v, &f.name)
            && !others.clone().any(|x| sets(x, &f.name))
    };
    if let Some(f) = t.fields.iter().find(only) {
        return Some(format!("the field `{}` of `{}`", f.name, t.name));
    }
    let retires = |v: &lakelands_api::ontology::VerbDecl| matches!(v.effect, VerbEffect::Retire);
    let last = !t.retired_by.is_authors() && retires(v) && !others.clone().any(retires);
    last.then(|| format!("the type `{}`", t.name))
}

/// The first set, then trigger, in force whose condition names the set
/// `set`: the object set `big`, the trigger `watch`.
fn set_reader(o: &Ontology, set: &str) -> Option<String> {
    let names = |c: &Condition| {
        let mut named = Vec::new();
        lakelands_core::describe::in_sets(&c.typed().ok()?, &mut named);
        let through = |n: &String| o.set(n).is_some_and(|s| names_set(o, s, set));
        Some(named.iter().any(|n| n == set || through(n)))
    };
    let sets = o.object_sets.values().map(|e| &e.decl);
    let mut over = sets.filter(|s| s.name != set && names_set(o, s, set));
    if let Some(s) = over.next() {
        return Some(format!("the object set `{}`", s.name));
    }
    let mut triggers = o.triggers.values().map(|e| &e.decl);
    let watching = triggers.find(|t| t.when.as_ref().and_then(names) == Some(true));
    watching.map(|t| format!("the trigger `{}`", t.name))
}

/// The first field in force that a function computes, as a refusal names
/// it: the field `total` of `invoice`.
fn function_reader(o: &Ontology, function: &str) -> Option<String> {
    let calls = |f: &FieldDecl| f.function.as_deref() == Some(function);
    // A link type's fields are its relation's, named at the relation; one
    // no relation in force carries is named itself, as it stands alone.
    let carried = |t: &TypeDecl| {
        let (owner, relation) = links::link_type_parts(&t.name)?;
        let owner = &o.types.get(&owner)?.decl;
        let mut rs = owner.relations.iter();
        rs.find(|r| r.name == relation && r.carried_by_links())
    };
    let alone = || {
        let mut types = o.types.values().map(|e| &e.decl);
        types.find_map(|t| {
            let f = t.fields.iter().find(|f| calls(f))?;
            let alone = links::is_link_type(&t.name) && carried(t).is_none();
            alone.then(|| format!("the field `{}` of `{}`", f.name, t.name))
        })
    };
    let mut types = o.types.values().filter(|t| !is_system(&t.decl.name));
    let found = types.find_map(|t| {
        let t = &t.decl;
        let own = t.fields.iter().find(|f| calls(f));
        let own = own.map(|f| format!("the field `{}` of `{}`", f.name, t.name));
        own.or_else(|| {
            t.relations.iter().find_map(|r| {
                let f = r.fields.iter().find(|f| calls(f))?;
                Some(format!(
                    "the field `{}` of `{}`'s relation `{}`",
                    f.name, t.name, r.name
                ))
            })
        })
    });
    found.or_else(alone)
}

/// A host's setting: a key of its own, never one the store reads or one
/// under such a key's prefix; a group of its own, never the store's; and a
/// type a field could hold, but a reference, whose record nothing would
/// hold to it (063, extended).
fn setting_decl(o: &Ontology, s: &SettingDecl) -> Result<()> {
    use lakelands_api::settings::{STORE_GROUP, lake_setting, unlisted};
    let on = [Place::Setting { key: s.key.clone() }];
    let store = lake_setting(&s.key)
        .map(|(l, _)| l.key)
        .or_else(|| unlisted(&s.key));
    if let Some(prefix) = store {
        let prefix = prefix.to_owned();
        return Err(invalid(at(&on, Case::SettingStoreKey { prefix })));
    }
    if s.group.as_deref() == Some(STORE_GROUP) {
        return Err(invalid(at(&on, Case::SettingStoreGroup)));
    }
    if s.ty.kind == StorageKind::Reference {
        return Err(invalid(at(&on, Case::SettingReference)));
    }
    let value = FieldDecl {
        ty: s.ty.clone(),
        ..FieldDecl::of("value", s.ty.kind)
    };
    field_type(o, &on, &value, &TypeName::from(""), None)
}

/// Refuses `d` where the lake `o` with it declared has a problem among its
/// tools' names.
pub(crate) fn tool_names(o: &Ontology, d: &Declaration) -> Result<()> {
    // Where the writer's names say so, the lake with `d` has none.
    if o.tool_names().is_some_and(|names| names.clear(o, d)) {
        return Ok(());
    }
    let problems = tool_problems(o, d);
    match problems.is_empty() {
        true => Ok(()),
        false => Err(invalid(Case::ToolNames { problems })),
    }
}

/// What is wrong with the tool names of the lake `o` with `d` declared,
/// a type with the link types it brings, alone as in a set: no door
/// leaves a lake with a problem among them (027, extended).
pub(crate) fn tool_problems(o: &Ontology, d: &Declaration) -> Vec<NameProblem> {
    let mut types: Vec<TypeName> = o.types.keys().cloned().collect();
    let mut verbs: Vec<VerbName> = o.verbs.keys().cloned().collect();
    match d {
        Declaration::Type(t) => {
            let brought = links::link_types_of(t).into_iter().map(|l| l.name);
            let new = brought.chain([t.name.clone()]);
            types.extend(new.filter(|n| !o.types.contains_key(n)));
        }
        Declaration::Verb(v) if !verbs.contains(&v.name) => verbs.push(v.name.clone()),
        _ => {}
    }
    let mut relations: Vec<(TypeName, RelationName)> = o
        .types
        .values()
        .filter(|e| !matches!(d, Declaration::Type(t) if t.name == e.decl.name))
        .flat_map(|e| link_relations(&e.decl))
        .collect();
    if let Declaration::Type(t) = d {
        relations.extend(link_relations(t));
    }
    name_problems(&types, &verbs, &relations)
}

/// What is wrong with the tool names of `set` beside those of `o` that
/// its author may not be shown, `seen` being the types of `o` it is
/// shown, each with the relations it is: each verb's, and each type's
/// and each relation's that `seen` does not hold. The lake in force has
/// no such problem, so where this finds none, every problem of the lake
/// with `set` in it lies between what its author is shown, and none of
/// the lake's other names is made to say so.
pub(crate) fn unshown_tool_problems(
    o: &Ontology,
    seen: &Ontology,
    set: &[Declaration],
) -> Vec<NameProblem> {
    let (mut types, mut relations) = (Vec::new(), Vec::new());
    for (name, e) in &o.types {
        let view = seen.types.get(name).map(|s| &s.decl.relations);
        types.extend(view.is_none().then(|| name.clone()));
        let shown = |r: &RelationDecl| view.is_some_and(|v| v.iter().any(|s| s.name == r.name));
        let unshown = |r: &&RelationDecl| r.carried_by_links() && !shown(r);
        let named = e.decl.relations.iter().filter(unshown);
        relations.extend(named.map(|r| (name.clone(), r.name.clone())));
    }
    let mut verbs: Vec<VerbName> = o.verbs.keys().cloned().collect();
    for d in set {
        match d {
            Declaration::Type(t) => {
                types.extend(links::link_types_of(t).into_iter().map(|l| l.name));
                types.push(t.name.clone());
                relations.extend(link_relations(t));
            }
            Declaration::Verb(v) if !verbs.contains(&v.name) => verbs.push(v.name.clone()),
            _ => {}
        }
    }
    name_problems(&types, &verbs, &relations)
}

/// How a value type's new declaration relates to the one in force
/// (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueTypeChange {
    /// It accepts exactly what it did.
    Same,
    /// It accepts everything it did: no stored value can stop fitting.
    Loosening,
    /// It may refuse a value it accepted: a migration with a preflight.
    Tightening,
    /// Its `kind` or `scale` changes, which rewrites stored values.
    Reshaping,
}

/// Compare a value type with the one in force. `one_of` may gain values
/// or be removed, and a `constraint` or `format` removed, without a
/// stored value ever failing; anything else it would accept differently
/// may.
pub fn value_type_change(current: &ValueTypeDecl, new: &ValueTypeDecl) -> ValueTypeChange {
    if current.kind != new.kind || current.scale != new.scale {
        return ValueTypeChange::Reshaping;
    }
    if current.one_of == new.one_of
        && current.constraint == new.constraint
        && current.format == new.format
    {
        return ValueTypeChange::Same;
    }
    let one_of = new.one_of.is_empty()
        || (!current.one_of.is_empty() && current.one_of.iter().all(|v| new.one_of.contains(v)));
    let constraint = new.constraint.is_none() || new.constraint == current.constraint;
    let format = new.format.is_none() || new.format == current.format;
    if one_of && constraint && format {
        ValueTypeChange::Loosening
    } else {
        ValueTypeChange::Tightening
    }
}

#[cfg(test)]
mod tests;

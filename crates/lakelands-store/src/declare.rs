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
//!   decimal, number, boolean, date-time), and so are a view's `group_by`
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

use lakelands_api::expr::{Condition, Expr};
use lakelands_api::ontology::{
    Authority, Derived, FieldDecl, RelationDecl, StorageKind, TypeDecl, ValueTypeDecl, VerbEffect,
};
use lakelands_api::tools::name_problems;
use lakelands_api::write::Declaration;
use lakelands_api::{Error, FieldName, Refusal, RelationName, Result, TypeName, VerbName};
use lakelands_core::expr::{Place, check, parse};
use lakelands_core::rights::is_system;
use lakelands_core::shape::{self, Change};
use lakelands_core::{derive, links, names, value};

use crate::ontology::Ontology;

fn invalid(message: impl Into<String>) -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::Invalid {
            message: message.into(),
        },
    }
}

fn invalid_field(field: &FieldName, message: impl Into<String>) -> Error {
    Error::Refused {
        field: Some(field.clone()),
        reason: Refusal::Invalid {
            message: message.into(),
        },
    }
}

fn name(what: &str, n: &str) -> Result<()> {
    names::fault(what, n).map_or(Ok(()), |m| Err(invalid(m)))
}

/// A condition a declaration carries, which the log keeps: checked as
/// [`condition`] checks it. A declaration may call `date_bucket` from log
/// format version 3 on; the event that declares it is written at 3
/// (`lakelands_core::versions`, `docs/log-format.md` §6).
fn declared(c: &Condition, place: Place, at: &str) -> Result<Expr> {
    condition(c, place, at)
}

/// A condition in its typed form, checked for its place.
pub fn condition(c: &Condition, place: Place, at: &str) -> Result<Expr> {
    let e = match c {
        Condition::Expr(e) => e.clone(),
        Condition::Text(t) => {
            parse(t).map_err(|e| invalid(format!("{at}: `{t}` does not parse: {e}")))?
        }
    };
    check(&e, place).map_err(|e| invalid(format!("{at}: {e}")))?;
    Ok(e)
}

fn field_type(o: &Ontology, owner: &str, f: &FieldDecl, pending: &TypeName) -> Result<()> {
    name("field", f.name.as_str())?;
    let ty = &f.ty;
    let wrong = |m: &str| invalid_field(&f.name, format!("{owner}, field `{}`: {m}", f.name));
    match (ty.kind, &ty.target, ty.dimensions) {
        (StorageKind::Reference, None, _) => {
            return Err(wrong("a reference names its target type"));
        }
        (StorageKind::Reference, Some(t), _) if t != pending && !o.types.contains_key(t) => {
            return Err(wrong(&format!("its target `{t}` is not a declared type")));
        }
        (StorageKind::Vector, _, None | Some(0)) => {
            return Err(wrong("a vector names its dimensions, more than zero"));
        }
        (k, Some(_), _) if k != StorageKind::Reference => {
            return Err(wrong("only a reference names a target"));
        }
        (k, _, Some(_)) if k != StorageKind::Vector => {
            return Err(wrong("only a vector names dimensions"));
        }
        _ => {}
    }
    if ty.value_type.is_some() {
        value::conformance(f, &o.value_type_list()).map_err(|e| wrong(&e.to_string()))?;
    }
    if let Some(d) = &f.derived {
        declared(d, Place::Write, &format!("{owner}, field `{}`", f.name))?;
    }
    if let Some(func) = &f.function {
        if f.derived.is_some() {
            return Err(wrong(
                "a field is derived from an expression or computed by a function, not both",
            ));
        }
        if !o.functions.contains_key(func.as_str()) {
            return Err(wrong(&format!(
                "it names the function `{func}`, which is not declared"
            )));
        }
        if f.authority != lakelands_api::ontology::Authority::Derived {
            return Err(wrong(
                "a field computed by a function has the authority `derived`",
            ));
        }
    }
    Ok(())
}

fn type_decl(o: &Ontology, t: &TypeDecl) -> Result<()> {
    name("type", t.name.as_str())?;
    let owner = format!("type `{}`", t.name);
    let has = |f: &FieldName| t.fields.iter().any(|d| &d.name == f);
    let mut seen = std::collections::BTreeSet::new();
    if let Some((field, why)) = t.embedding_problem() {
        return Err(invalid_field(&field, why));
    }
    for f in &t.fields {
        if !seen.insert(&f.name) {
            return Err(invalid_field(
                &f.name,
                format!("{owner} declares `{}` twice", f.name),
            ));
        }
        field_type(o, &owner, f, &t.name)?;
        if let Some(func) = f
            .function
            .as_ref()
            .and_then(|n| o.functions.get(n.as_str()))
        {
            // A function computes a field from inputs the type holds, each
            // of the kind the function declares for it.
            for input in &func.decl.inputs {
                match t.fields.iter().find(|d| d.name == input.name) {
                    Some(d) if d.ty.kind == input.ty.kind => {}
                    Some(_) => {
                        return Err(invalid_field(
                            &f.name,
                            format!(
                                "{owner}, field `{}`: the function `{}` takes `{}` as a {}, \
                                 and `{}` holds it as another kind",
                                f.name,
                                func.decl.name,
                                input.name,
                                value::kind_name(input.ty.kind),
                                t.name
                            ),
                        ));
                    }
                    None => {
                        return Err(invalid_field(
                            &f.name,
                            format!(
                                "{owner}, field `{}`: the function `{}` takes `{}`, which is \
                                 not one of its fields",
                                f.name, func.decl.name, input.name
                            ),
                        ));
                    }
                }
            }
        }
        for d in &f.depends_on {
            if !has(d) {
                return Err(invalid_field(
                    &f.name,
                    format!(
                        "{owner}, field `{}` depends on `{d}`, which it does not have",
                        f.name
                    ),
                ));
            }
        }
        if f.ty.kind == StorageKind::Json
            && (o.types.contains_key(f.name.as_str()) || t.name.as_str() == f.name.as_str())
        {
            return Err(invalid_field(
                &f.name,
                format!("{owner}: a json field may not carry the name of a type"),
            ));
        }
    }
    for k in t.key.iter().chain(&t.valid_from).chain(&t.valid_to) {
        if !has(k) {
            return Err(invalid_field(
                k,
                format!("{owner} names `{k}`, which is not one of its fields"),
            ));
        }
    }
    for k in &t.key {
        let kind = t.fields.iter().find(|f| &f.name == k).map(|f| f.ty.kind);
        if kind.is_some_and(|kind| !lakelands_core::log::keyable(kind)) {
            return Err(invalid_field(
                k,
                format!(
                    "{owner} is keyed by `{k}`, whose kind has no string form for a key; a key \
                     field is text, a reference, a decimal, a number, a boolean or a date-time"
                ),
            ));
        }
    }
    for r in &t.relations {
        name("relation", r.name.as_str())?;
        // A relation is carried by its `via` field or by link records,
        // never both (`docs/decisions/035-a-link-is-a-record.md`).
        if r.via.is_some() && !r.fields.is_empty() {
            return Err(invalid(format!(
                "{owner}, relation `{}`: a relation carried by `via` has no fields of its own; \
                 fields belong to a relation carried by link records, declared without `via`",
                r.name
            )));
        }
        if !matches!(r.authority, Authority::Authored | Authority::Source) {
            return Err(invalid(format!(
                "{owner}, relation `{}`: its links are written by `authored` or `source`, not \
                 `{:?}`",
                r.name, r.authority
            )));
        }
        if r.authority == Authority::Source
            && let Some(f) = r
                .fields
                .iter()
                .find(|f| !matches!(f.authority, Authority::Source | Authority::Derived))
        {
            return Err(invalid_field(
                &f.name,
                format!(
                    "{owner}, relation `{}`: its links arrive from a source, so its field `{}` is \
                     `source` (or derived); an `{:?}` field of a link no door could write",
                    r.name, f.name, f.authority
                ),
            ));
        }
        if let Some(f) = r
            .fields
            .iter()
            .find(|f| f.name.as_str() == links::FROM || f.name.as_str() == links::TO)
        {
            return Err(invalid(format!(
                "{owner}, relation `{}`: a link's `{}` is its end; the relation cannot name a field \
                 of its own so",
                r.name, f.name
            )));
        }
        if let Some(before) = o
            .types
            .get(&t.name)
            .and_then(|e| e.decl.relations.iter().find(|b| b.name == r.name))
            && carrier(before) != carrier(r)
        {
            return Err(invalid(format!(
                "{owner}, relation `{}`: a relation keeps its carrier; it is carried by {} and \
                 cannot be carried by {}; declare the other kind under another name",
                r.name,
                carrier(before),
                carrier(r)
            )));
        }
        if r.target != t.name && !o.types.contains_key(&r.target) {
            return Err(invalid(format!(
                "{owner}, relation `{}`: `{}` is not a declared type",
                r.name, r.target
            )));
        }
        if let Some(via) = &r.via
            && !has(via)
        {
            return Err(invalid(format!(
                "{owner}, relation `{}`: `via` names `{via}`, which is not a field",
                r.name
            )));
        }
        if let Some(d) = &r.derived {
            derived(o, t, r, d, &owner)?;
        }
        for f in &r.fields {
            field_type(
                o,
                &format!("{owner}, relation `{}`", r.name),
                f,
                &links::link_type_name(&t.name, &r.name),
            )?;
        }
    }
    if let Some(v) = &t.view {
        let source = o.ty(&v.source).map_err(|_| {
            invalid(format!(
                "{owner}: its view reads `{}`, which is not declared",
                v.source
            ))
        })?;
        if let Some(w) = &v.r#where {
            declared(w, Place::Record, &format!("{owner}, view `where`"))?;
        }
        for g in &v.group_by {
            let Some(f) = source.fields.iter().find(|f| &f.name == g) else {
                return Err(invalid(format!(
                    "{owner}: its view groups by `{g}`, which `{}` does not have",
                    v.source
                )));
            };
            // The group_by values are the view record's key.
            if !lakelands_core::log::keyable(f.ty.kind) {
                return Err(invalid(format!(
                    "{owner}: its view groups by `{g}`, whose kind cannot be part of a key"
                )));
            }
        }
        for m in &v.measures {
            name("measure", m.name.as_str())?;
            declared(
                &m.expr,
                Place::Group,
                &format!("{owner}, measure `{}`", m.name),
            )?;
        }
    }
    // Order the fields as the fold will compute them: a field computed by
    // a function reads what that function's expression reads, so it is
    // ordered, and checked for cycles, with the function resolved.
    derive::order(&crate::record::resolve_functions(t, o))
        .map_err(|e| invalid(format!("{owner}: {e}")))?;
    computed_without_cycle(o, t).map_err(|e| invalid(format!("{owner}: {e}")))?;
    Ok(())
}

/// A type's computed fields, derived expressions and function-computed
/// fields alike, read one another without a cycle, so a chain of fills
/// within one record ends (`docs/decisions/022-functions-are-declarations.md`,
/// extended).
fn computed_without_cycle(o: &Ontology, t: &TypeDecl) -> std::result::Result<(), String> {
    crate::backed::computed_order(o, t).map(|_| ())
}

/// A type's relations carried by link records, as the tool generator
/// names their doors.
fn link_relations(t: &TypeDecl) -> Vec<(TypeName, RelationName)> {
    t.relations
        .iter()
        .filter(|r| links::carried_by_links(r))
        .map(|r| (t.name.clone(), r.name.clone()))
        .collect()
}

/// Whether a host vocabulary type may be declared under its name
/// (`docs/decisions/003-reservation-by-namespace.md`). A name under
/// `sys.link.` belongs to the link types the store derives from relations
/// (035), and the four names of `RESERVED_SYSTEM_TYPES` to the store's own
/// readers; a vocabulary that names one is refused, naming it.
pub fn check_vocabulary_type(t: &TypeDecl) -> Result<()> {
    let why = if lakelands_core::links::is_link_type(&t.name) {
        "the store derives the link types under `sys.link.` from relations itself"
    } else if lakelands_api::seam::RESERVED_SYSTEM_TYPES.contains(&t.name.as_str()) {
        "the store keeps it for its own reader"
    } else {
        return Ok(());
    };
    Err(invalid(format!(
        "the host vocabulary's type `{}` is reserved: {why}; nothing was appended, and the \
         lake is not opened for writing until the vocabulary names it otherwise",
        t.name
    )))
}

/// Check one declaration against the ontology it would join. `who_is_system`
/// What carries a relation's edges: a `via` field, link records, or
/// nothing stored (derived, 044).
fn carrier(r: &RelationDecl) -> &'static str {
    match (&r.via, &r.derived) {
        (_, Some(_)) => "matching values (`derived`)",
        (Some(_), None) => "a `via` field",
        (None, None) => "link records",
    }
}

/// A derived relation's match: one or more pairs, each a field of the
/// owner and a field of the target of the same storage kind, one a key
/// may be, so that identity of the stored values is what connects them;
/// and no carrier of its own (`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`).
fn derived(o: &Ontology, t: &TypeDecl, r: &RelationDecl, d: &Derived, owner: &str) -> Result<()> {
    let at = |why: String| invalid(format!("{owner}, relation `{}`: {why}", r.name));
    if r.via.is_some() || !r.fields.is_empty() || r.authority != Authority::Authored {
        return Err(at(
            "a derived relation stores no edge, so it has no `via`, no fields and no link \
             authority"
                .into(),
        ));
    }
    if d.pairs.is_empty() {
        return Err(at(
            "a derived relation matches at least one pair of fields".into()
        ));
    }
    let target = if r.target == t.name {
        Some(t)
    } else {
        o.ty(&r.target).ok()
    };
    for p in &d.pairs {
        if d.pairs.iter().filter(|q| *q == p).count() > 1 {
            return Err(at(format!(
                "the pair `{}` = `{}` is matched twice",
                p.from, p.to
            )));
        }
        let from = t.fields.iter().find(|f| f.name == p.from);
        let to = target.and_then(|u| u.fields.iter().find(|f| f.name == p.to));
        let (Some(from), Some(to)) = (from, to) else {
            return Err(at(format!(
                "the pair `{}` = `{}` names a field `{}` or `{}.{}` does not have",
                p.from, p.to, t.name, r.target, p.to
            )));
        };
        if from.ty.kind != to.ty.kind {
            return Err(at(format!(
                "the pair `{}` = `{}` matches a {:?} field with a {:?} field; values of two \
                 kinds are never identical, so the pair would match nothing",
                p.from, p.to, from.ty.kind, to.ty.kind
            )));
        }
        if !lakelands_core::log::keyable(from.ty.kind) {
            return Err(at(format!(
                "the pair `{}` = `{}` matches {:?} fields; a match is between fields of a kind a \
                 key may be: text, a reference, a decimal, a number, a boolean or a date-time",
                p.from, p.to, from.ty.kind
            )));
        }
    }
    Ok(())
}

/// is true only for the store's own declarations.
pub fn check_declaration(o: &Ontology, d: &Declaration, who_is_system: bool) -> Result<()> {
    if let Some(fault) = d.annotation_fault() {
        return Err(fault.into());
    }
    match d {
        Declaration::Type(t) => {
            if is_system(&t.name) && !who_is_system {
                return Err(Error::Refused {
                    field: None,
                    reason: Refusal::Reserved,
                });
            }
            type_decl(o, t)?;
            if let Some(current) = o.types.get(&t.name)
                && let Change::Migrate { reasons } = shape::classify(&current.decl, t)
            {
                let why: Vec<String> = reasons.iter().map(ToString::to_string).collect();
                return Err(invalid(format!(
                    "`{}` would reshape stored records ({}); that is a migration",
                    t.name,
                    why.join("; ")
                )));
            }
            // A relation's link type is a type like any: its fields are
            // checked as a type's, with `t` in force (its `from` points
            // there). And it is classified too: a relation dropped by a
            // migration and declared again with another target, or with a
            // field retyped, would reshape the links it kept.
            let mut probe = o.clone();
            probe.put(d.clone(), 0, None);
            for lt in links::link_types_of(t) {
                type_decl(&probe, &lt).map_err(|e| match e {
                    Error::Refused {
                        reason: Refusal::Invalid { message },
                        field,
                    } => Error::Refused {
                        field,
                        reason: Refusal::Invalid {
                            message: format!("the links of `{}`: {message}", t.name),
                        },
                    },
                    other => other,
                })?;
                if let Some(current) = o.types.get(&lt.name)
                    && let Change::Migrate { reasons } = shape::classify(&current.decl, &lt)
                {
                    let why: Vec<String> = reasons.iter().map(ToString::to_string).collect();
                    return Err(invalid(format!(
                        "`{}` would reshape the links `{}` holds ({}); that is a migration of \
                         `{}`",
                        t.name,
                        lt.name,
                        why.join("; "),
                        t.name
                    )));
                }
            }
        }
        Declaration::ValueType(v) => {
            name("value type", v.name.as_str())?;
            if let Some(f) = &v.format
                && value::ValueFormat::parse(f).is_none()
            {
                return Err(invalid(format!(
                    "value type `{}` names the unknown format `{f}`",
                    v.name
                )));
            }
            if v.scale.is_some() && v.kind != StorageKind::Decimal {
                return Err(invalid(format!(
                    "value type `{}`: only a decimal has a scale",
                    v.name
                )));
            }
            if !v.one_of.is_empty() && v.kind != StorageKind::Text {
                return Err(invalid(format!(
                    "value type `{}`: only text has `one_of`",
                    v.name
                )));
            }
            if let Some(c) = &v.constraint {
                declared(
                    c,
                    Place::Record,
                    &format!("value type `{}`, constraint", v.name),
                )?;
            }
            if let Some(current) = o.value_types.get(v.name.as_str()) {
                match value_type_change(&current.decl, v) {
                    ValueTypeChange::Same | ValueTypeChange::Loosening => {}
                    ValueTypeChange::Tightening => {
                        return Err(invalid(format!(
                            "value type `{}` would accept less than it does: stored values may \
                             stop fitting it; tighten it through `migrate`, whose preflight \
                             counts them",
                            v.name
                        )));
                    }
                    ValueTypeChange::Reshaping => {
                        return Err(invalid(format!(
                            "value type `{}` would change its `kind` or `scale`, which rewrites \
                             stored values; that is not a value-type migration: migrate the \
                             fields that use it",
                            v.name
                        )));
                    }
                }
            }
        }
        Declaration::Verb(v) => {
            name("verb", v.name.as_str())?;
            let t = o.ty(&v.on).map_err(|_| {
                invalid(format!(
                    "verb `{}` acts on `{}`, which is not declared",
                    v.name, v.on
                ))
            })?;
            for f in &v.payload {
                field_type(o, &format!("verb `{}`, payload", v.name), f, &v.on)?;
            }
            if let Some(r) = &v.requires {
                declared(r, Place::Record, &format!("verb `{}`, requires", v.name))?;
            }
            match &v.effect {
                VerbEffect::Write { set } => {
                    if set.is_empty() {
                        return Err(invalid(format!("verb `{}` writes nothing", v.name)));
                    }
                    for a in set {
                        if !t.fields.iter().any(|f| f.name == a.field) {
                            return Err(invalid(format!(
                                "verb `{}` sets `{}`, which `{}` does not have",
                                v.name, a.field, v.on
                            )));
                        }
                        declared(
                            &a.value,
                            Place::Record,
                            &format!("verb `{}`, `{}`", v.name, a.field),
                        )?;
                    }
                }
                VerbEffect::External { executor } => name("executor", executor)?,
            }
        }
        Declaration::Trigger(t) => {
            name("trigger", t.name.as_str())?;
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
                return Err(Error::Refused {
                    field: Some("on".into()),
                    reason: Refusal::Invalid {
                        message: format!(
                            "trigger `{}` watches `{}`, but `{relation}` is carried by the \
                             reference field `{via}`, not by link records; watch `{owner}` \
                             with `changed({via})`",
                            t.name, t.on
                        ),
                    },
                });
            }
            o.ty(&t.on).map_err(|_| {
                invalid(format!(
                    "trigger `{}` watches `{}`, which is not declared",
                    t.name, t.on
                ))
            })?;
            if let Some(w) = &t.when {
                declared(w, Place::Write, &format!("trigger `{}`", t.name))?;
            }
        }
        Declaration::ObjectSet(s) => {
            name("object set", &s.name)?;
            o.ty(&s.of).map_err(|_| {
                invalid(format!(
                    "object set `{}` holds `{}`, which is not declared",
                    s.name, s.of
                ))
            })?;
            declared(
                &s.filter,
                Place::Record,
                &format!("object set `{}`", s.name),
            )?;
        }
        Declaration::Function(f) => {
            name("function", &f.name)?;
            name("backing", &f.backing)?;
            // Declared again with other inputs, it must not close a cycle
            // in a type whose field it computes.
            let mut probe = o.clone();
            probe.put(d.clone(), 0, None);
            for t in o.types.values().filter(|t| {
                t.decl
                    .fields
                    .iter()
                    .any(|x| x.function.as_deref() == Some(f.name.as_str()))
            }) {
                computed_without_cycle(&probe, &t.decl).map_err(|e| {
                    invalid(format!(
                        "function `{}`, type `{}`: {e}",
                        f.name, t.decl.name
                    ))
                })?;
            }
            for input in &f.inputs {
                field_type(
                    o,
                    &format!("function `{}`, input", f.name),
                    input,
                    &TypeName::from(""),
                )?;
            }
            match (f.backing.as_str(), &f.expr) {
                ("expr", Some(e)) => {
                    let e = declared(e, Place::Record, &format!("function `{}`", f.name))?;
                    // The expression reads only the inputs it declares, so
                    // the inputs a field is checked against are all it reads.
                    for read in derive::reads(&e) {
                        if !f.inputs.iter().any(|i| i.name == read) {
                            return Err(invalid(format!(
                                "function `{}` reads `{read}`, which is not one of its inputs",
                                f.name
                            )));
                        }
                    }
                }
                ("expr", None) => {
                    return Err(invalid(format!(
                        "function `{}` is backed by `expr` and has no expression",
                        f.name
                    )));
                }
                (_, Some(_)) => {
                    return Err(invalid(format!(
                        "function `{}` has an expression but is backed by `{}`",
                        f.name, f.backing
                    )));
                }
                _ => {}
            }
        }
    }
    let mut types: Vec<TypeName> = o.types.keys().cloned().collect();
    let mut verbs: Vec<VerbName> = o.verbs.keys().cloned().collect();
    match d {
        Declaration::Type(t) if !types.contains(&t.name) => types.push(t.name.clone()),
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
    let problems = name_problems(&types, &verbs, &relations);
    if !problems.is_empty() {
        let all: Vec<String> = problems.iter().map(ToString::to_string).collect();
        return Err(invalid(all.join("; ")));
    }
    Ok(())
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

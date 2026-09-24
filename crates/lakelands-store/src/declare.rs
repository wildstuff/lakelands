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
use lakelands_api::ontology::{FieldDecl, StorageKind, TypeDecl, VerbEffect};
use lakelands_api::tools::name_problems;
use lakelands_api::write::Declaration;
use lakelands_api::{Error, FieldName, Refusal, Result, TypeName, VerbName};
use lakelands_core::expr::{Place, check, parse};
use lakelands_core::rights::is_system;
use lakelands_core::shape::{self, Change};
use lakelands_core::{derive, names, value};

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
        condition(d, Place::Write, &format!("{owner}, field `{}`", f.name))?;
    }
    if let Some(func) = &f.function {
        if f.derived.is_some() {
            return Err(wrong(
                "a field is derived from an expression or computed by a function, not both",
            ));
        }
        match o.functions.get(func.as_str()) {
            None => {
                return Err(wrong(&format!(
                    "it names the function `{func}`, which is not declared"
                )));
            }
            Some(e) if e.decl.backing != "expr" => {
                return Err(Error::Unbound {
                    seam: format!(
                        "the `{}` function backing (only `expr` computes fields in this release)",
                        e.decl.backing
                    ),
                });
            }
            Some(_) => {}
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
        if r.via.is_none() || !r.fields.is_empty() {
            return Err(invalid(format!(
                "{owner}, relation `{}`: in this release a relation is carried by a reference field \
                 named in `via`, and has no link fields of its own",
                r.name
            )));
        }
        if r.target != t.name && !o.types.contains_key(&r.target) {
            return Err(invalid(format!(
                "{owner}, relation `{}`: `{}` is not a declared type",
                r.name, r.target
            )));
        }
        if let Some(via) = &r.via {
            if !has(via) {
                return Err(invalid(format!(
                    "{owner}, relation `{}`: `via` names `{via}`, which is not a field",
                    r.name
                )));
            }
        }
        for f in &r.fields {
            field_type(o, &format!("{owner}, relation `{}`", r.name), f, &t.name)?;
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
            condition(w, Place::Record, &format!("{owner}, view `where`"))?;
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
            condition(
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
    Ok(())
}

/// Check one declaration against the ontology it would join. `who_is_system`
/// is true only for the store's own declarations.
pub fn check_declaration(o: &Ontology, d: &Declaration, who_is_system: bool) -> Result<()> {
    match d {
        Declaration::Type(t) => {
            if is_system(&t.name) && !who_is_system {
                return Err(Error::Refused {
                    field: None,
                    reason: Refusal::Reserved,
                });
            }
            type_decl(o, t)?;
            if let Some(current) = o.types.get(&t.name) {
                if let Change::Migrate { reasons } = shape::classify(&current.decl, t) {
                    let why: Vec<String> = reasons.iter().map(ToString::to_string).collect();
                    return Err(invalid(format!(
                        "`{}` would reshape stored records ({}); that is a migration",
                        t.name,
                        why.join("; ")
                    )));
                }
            }
        }
        Declaration::ValueType(v) => {
            name("value type", v.name.as_str())?;
            if let Some(f) = &v.format {
                if value::ValueFormat::parse(f).is_none() {
                    return Err(invalid(format!(
                        "value type `{}` names the unknown format `{f}`",
                        v.name
                    )));
                }
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
                condition(
                    c,
                    Place::Record,
                    &format!("value type `{}`, constraint", v.name),
                )?;
            }
            if let Some(current) = o.value_types.get(v.name.as_str()) {
                let c = &current.decl;
                if c.kind != v.kind
                    || c.one_of != v.one_of
                    || c.constraint != v.constraint
                    || c.scale != v.scale
                    || c.format != v.format
                {
                    return Err(invalid(format!(
                        "changing what `{}` accepts would reshape stored values; that is a migration",
                        v.name
                    )));
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
                condition(r, Place::Record, &format!("verb `{}`, requires", v.name))?;
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
                        condition(
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
            o.ty(&t.on).map_err(|_| {
                invalid(format!(
                    "trigger `{}` watches `{}`, which is not declared",
                    t.name, t.on
                ))
            })?;
            if let Some(w) = &t.when {
                condition(w, Place::Write, &format!("trigger `{}`", t.name))?;
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
            condition(
                &s.filter,
                Place::Record,
                &format!("object set `{}`", s.name),
            )?;
        }
        Declaration::Function(f) => {
            name("function", &f.name)?;
            name("backing", &f.backing)?;
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
                    let e = condition(e, Place::Record, &format!("function `{}`", f.name))?;
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
    let problems = name_problems(&types, &verbs);
    if !problems.is_empty() {
        let all: Vec<String> = problems.iter().map(ToString::to_string).collect();
        return Err(invalid(all.join("; ")));
    }
    Ok(())
}

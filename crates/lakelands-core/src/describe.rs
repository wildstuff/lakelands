//! `describe`, assembled: the lake's declarations as one principal may see
//! them, on the six axes of `lakelands_api::describe` — structure, access,
//! rights, actions, lineage, lifecycle.
//!
//! The answer is built from the same rules that gate reads and writes
//! (`crate::rights`), so `describe` never promises access a read or a
//! write would then refuse, and the tools generated from it
//! (`lakelands_api::tools::tools`) never offer a door that is shut. What a
//! principal holds no grant for is absent, not listed as forbidden.
//!
//! A declaration beside the types — a function, a trigger, an object set,
//! a type's view — is answered whole, and only to a principal who reads
//! every field it names or holds the `schema` grant. A condition says
//! what its fields are compared with and a function's inputs say what is
//! computed from them; a principal who may not read those fields learns
//! neither. A field's own `function` still names its function.

use lakelands_api::describe::{
    ExecutorDescription, FieldDescription, LakeDescription, LakeRights, TypeAccess,
    TypeDescription, TypeLifecycle, TypeLineage, TypeRights, VerbDescription,
};
use lakelands_api::engine::Declared;
use lakelands_api::expr::{Condition, Expr};
use lakelands_api::ontology::{Authority, RelationDecl, TypeDecl, VerbDecl, VerbEffect, ViewDecl};
use lakelands_api::write::{Declaration, FunctionDecl};
use lakelands_api::{Access, FieldName, LakeId, Principal, TypeName};

use crate::rights::{field_access, type_access};

/// The lake as `who` may see it.
pub fn lake(lake: &LakeId, declared: &[Declared], who: &Principal) -> LakeDescription {
    let types: Vec<TypeDescription> = each(declared, |d| match &d.declaration {
        Declaration::Type(t) => type_description(t, d, declared, who),
        _ => None,
    });
    let verbs = each(declared, |d| match &d.declaration {
        Declaration::Verb(v) => verb_description(v, declared, who),
        _ => None,
    });
    let value_types = each(declared, |d| match &d.declaration {
        Declaration::ValueType(v) => Some(v.clone()),
        _ => None,
    });
    let described = |ty: &TypeName| types.iter().any(|t| &t.name == ty);
    let functions = each(declared, |d| match &d.declaration {
        Declaration::Function(f) if function_visible(f, declared, who) => Some(f.clone()),
        _ => None,
    });
    let triggers = each(declared, |d| match &d.declaration {
        Declaration::Trigger(t)
            if described(&t.on) && condition_visible(t.when.as_ref(), &t.on, declared, who) =>
        {
            Some(t.clone())
        }
        _ => None,
    });
    let object_sets = each(declared, |d| match &d.declaration {
        Declaration::ObjectSet(s)
            if described(&s.of) && condition_visible(Some(&s.filter), &s.of, declared, who) =>
        {
            Some(s.clone())
        }
        _ => None,
    });
    LakeDescription {
        lake: lake.clone(),
        types,
        verbs,
        value_types,
        functions,
        triggers,
        object_sets,
        may: LakeRights {
            review: who.grants.review,
            schema: who.grants.schema,
            ingest: who.grants.ingest,
        },
    }
}

/// One type as `who` may see it; `None` when it may see none of it.
pub fn one_type(ty: &TypeName, declared: &[Declared], who: &Principal) -> Option<TypeDescription> {
    declared.iter().find_map(|d| match &d.declaration {
        Declaration::Type(t) if &t.name == ty => type_description(t, d, declared, who),
        _ => None,
    })
}

/// Whether `who` reads every one of `names` on type `ty`.
fn reads_all<'a>(
    names: impl IntoIterator<Item = &'a FieldName>,
    ty: &TypeName,
    declared: &[Declared],
    who: &Principal,
) -> bool {
    let Some(t) = find_type(declared, ty) else {
        return false;
    };
    names.into_iter().all(|n| {
        t.fields
            .iter()
            .find(|f| &f.name == n)
            .is_some_and(|f| field_access(who, t, f).reads())
    })
}

/// The fields a condition names; `None` when it does not parse.
fn named(c: &Condition) -> Option<std::collections::BTreeSet<FieldName>> {
    parsed(c).map(|e| crate::derive::reads(&e))
}

/// Whether `who` is shown the declaration `d`: whether [`lake`] describes
/// it to that principal. A refusal names a declaration only to one who is.
/// `declared` is what is in force: over a type as a migration would leave
/// it, a field it takes away is gone, and nothing that reads it would be
/// shown to anyone without the `schema` grant.
pub fn shown(d: &Declaration, declared: &[Declared], who: &Principal) -> bool {
    let seen = lake(&who.lake, declared, who);
    match d {
        // A view is shown when its definition is, not only its type's name.
        Declaration::Type(t) => {
            (seen.types.iter()).any(|s| s.name == t.name && (t.view.is_none() || s.view.is_some()))
        }
        Declaration::Verb(v) => seen.verbs.iter().any(|s| s.name == v.name),
        Declaration::Trigger(t) => seen.triggers.iter().any(|s| s.name == t.name),
        Declaration::ObjectSet(o) => seen.object_sets.iter().any(|s| s.name == o.name),
        _ => true,
    }
}

/// Whether a condition over type `ty` may be shown to `who`.
fn condition_visible(
    c: Option<&Condition>,
    ty: &TypeName,
    declared: &[Declared],
    who: &Principal,
) -> bool {
    who.grants.schema
        || c.is_none_or(|c| named(c).is_some_and(|n| reads_all(&n, ty, declared, who)))
}

/// Whether a view may be shown whole to `who`: every source field it names
/// is read.
fn view_visible(v: &ViewDecl, declared: &[Declared], who: &Principal) -> bool {
    who.grants.schema
        || view_reads(v).is_some_and(|reads| {
            reads
                .iter()
                .all(|(source, names)| reads_all(names, source, declared, who))
        })
}

/// The fields a view reads from each of its sources, its own and the
/// merged ones in order: a source is read as a read of it (024), so its
/// `where`, its grouping and what its measures aggregate. `None` when one
/// of its conditions does not parse.
pub fn view_reads(v: &ViewDecl) -> Option<Vec<(TypeName, std::collections::BTreeSet<FieldName>)>> {
    std::iter::once((&v.source, v.r#where.as_ref(), &v.group_by, &v.measures))
        .chain(
            v.merge
                .iter()
                .map(|m| (&m.source, m.r#where.as_ref(), &m.group_by, &m.measures)),
        )
        .map(|(source, filter, group_by, measures)| {
            let mut names: std::collections::BTreeSet<FieldName> =
                group_by.iter().cloned().collect();
            if let Some(f) = filter {
                names.extend(named(f)?);
            }
            for m in measures {
                aggregated(&parsed(&m.expr)?, false, &mut names);
            }
            Some((source.clone(), names))
        })
        .collect()
}

/// The fields of `ty` a declaration other than a view reads: a derived
/// field's expression on `ty` itself, the fields of `ty` a derived
/// relation matches as its target, a set's `where`, a trigger's `when`,
/// and a verb's `requires`, the fields it sets and what it sets them from
/// (where `payload.<name>` is the call's and reads no field). A condition
/// that does not parse reads nothing here.
pub fn declaration_reads(d: &Declaration, ty: &TypeName) -> std::collections::BTreeSet<FieldName> {
    let of = |c: &Condition| named(c).unwrap_or_default();
    match d {
        Declaration::Type(t) => {
            let own = t.fields.iter().filter(|_| &t.name == ty);
            let matched = t.relations.iter().filter(|r| &r.target == ty);
            own.filter_map(|f| f.derived.as_ref())
                .flat_map(of)
                .chain(
                    matched
                        .filter_map(|r| r.derived.as_ref())
                        .flat_map(|m| m.pairs.iter().map(|p| p.to.clone())),
                )
                .collect()
        }
        Declaration::ObjectSet(s) if &s.of == ty => of(&s.filter),
        Declaration::Trigger(t) if &t.on == ty => t.when.as_ref().map(of).unwrap_or_default(),
        Declaration::Verb(v) if &v.on == ty => {
            let mut names = v.requires.as_ref().map(of).unwrap_or_default();
            if let VerbEffect::Write { set } = &v.effect {
                for a in set {
                    let value = parsed(&a.value).map(|e| crate::derive::verb_reads(&e).0);
                    names.extend(value.unwrap_or_default());
                    names.insert(a.field.clone());
                }
            }
            names
        }
        _ => Default::default(),
    }
}

/// The fields a measure reads from its source's records: the paths inside
/// an aggregate's arguments. A name outside an aggregate is read from the
/// group, an earlier measure or a grouping field, and is no source field.
fn aggregated(e: &Expr, inside: bool, out: &mut std::collections::BTreeSet<FieldName>) {
    match e {
        Expr::Path { path } if inside => out.extend(path.first().cloned()),
        Expr::Call { func, .. } if func.is_aggregate() => {
            crate::expr::children(e).for_each(|a| aggregated(a, true, out));
        }
        _ => crate::expr::children(e).for_each(|a| aggregated(a, inside, out)),
    }
}

/// A condition in its typed form; `None` when it does not parse.
pub(crate) fn parsed(c: &Condition) -> Option<Expr> {
    match c {
        Condition::Expr(e) => Some(e.clone()),
        Condition::Text(t) => crate::expr::parse(t).ok(),
    }
}

/// Whether a function may be shown to `who`: it computes at least one
/// field, and on every type where it does, `who` reads that field and
/// every input, which the function reads from the fields of the same
/// names.
fn function_visible(f: &FunctionDecl, declared: &[Declared], who: &Principal) -> bool {
    if who.grants.schema {
        return true;
    }
    let mut uses = declared
        .iter()
        .filter_map(|d| match &d.declaration {
            Declaration::Type(t) => Some(t),
            _ => None,
        })
        .flat_map(|t| {
            t.fields
                .iter()
                .filter(|x| x.function.as_deref() == Some(f.name.as_str()))
                .map(move |x| (t, x))
        })
        .peekable();
    uses.peek().is_some()
        && uses.all(|(t, x)| {
            reads_all(
                std::iter::once(&x.name).chain(f.inputs.iter().map(|i| &i.name)),
                &t.name,
                declared,
                who,
            )
        })
}

/// What `pick` takes from each declaration, in declaration order.
fn each<'a, T>(declared: &'a [Declared], pick: impl FnMut(&'a Declared) -> Option<T>) -> Vec<T> {
    declared.iter().filter_map(pick).collect()
}

fn find_type<'a>(declared: &'a [Declared], ty: &TypeName) -> Option<&'a TypeDecl> {
    declared.iter().find_map(|d| match &d.declaration {
        Declaration::Type(t) if &t.name == ty => Some(t),
        _ => None,
    })
}

fn type_description(
    t: &TypeDecl,
    d: &Declared,
    declared: &[Declared],
    who: &Principal,
) -> Option<TypeDescription> {
    let fields: Vec<FieldDescription> = t
        .fields
        .iter()
        .map(|f| (f, field_access(who, t, f)))
        .filter(|(_, a)| *a != Access::None)
        .map(|(f, a)| FieldDescription::of(f, a))
        .collect();
    let whole = type_access(who, &t.name);
    if fields.is_empty() && whole == Access::None {
        return None;
    }
    let reads = fields.iter().any(|f| f.access.reads()) || whole.reads();
    let is_view = t.view.is_some();
    // A link type's records are written through its relation's link
    // doors, never through `upsert` or `retire`.
    let is_link = crate::links::is_link_type(&t.name);
    let may = TypeRights {
        find: reads,
        get: reads,
        upsert: !is_view && !is_link && fields.iter().any(FieldDescription::authored_writable),
        retire: !is_link && crate::rights::may_retire(who, t),
        link: t
            .relations
            .iter()
            .filter(|r| {
                crate::links::carried_by_links(r)
                    && r.authority == Authority::Authored
                    && find_type(declared, &r.target).is_some()
            })
            .filter(|r| {
                let lt = crate::links::link_type(&t.name, r);
                lt.fields
                    .iter()
                    .filter(|f| {
                        f.name.as_str() == crate::links::FROM || f.name.as_str() == crate::links::TO
                    })
                    .all(|f| field_access(who, &lt, f).writes())
            })
            .map(|r| r.name.clone())
            .collect(),
    };
    let verbs = each(declared, |d| match &d.declaration {
        Declaration::Verb(v) if v.on == t.name && verb_callable(v, t, who) => Some(v.name.clone()),
        _ => None,
    });
    Some(TypeDescription {
        name: t.name.clone(),
        description: t.description.clone(),
        label: t.label.clone(),
        language: t.language.clone(),
        view: t
            .view
            .clone()
            .filter(|v| view_visible(v, declared, who))
            .map(|v| *v),
        annotations: t.annotations.clone(),
        relations: t
            .relations
            .iter()
            .filter(|r| find_type(declared, &r.target).is_some())
            .map(|r| relation_description(&t.name, r, who))
            .collect(),
        access: TypeAccess {
            key: t.key.clone(),
            valid_from: t.valid_from.clone(),
            valid_to: t.valid_to.clone(),
            searchable: t.fields.iter().any(|f| f.searchable),
            field_meta: t.field_meta,
        },
        may,
        verbs,
        lineage: TypeLineage {
            sourced: t
                .fields
                .iter()
                .any(|f| matches!(f.authority, Authority::Source | Authority::Shared)),
            view_of: t.view.as_ref().map(|v| v.source.clone()),
        },
        lifecycle: TypeLifecycle {
            version: d.version,
            schema_hash: d.schema_hash.clone().unwrap_or_default(),
            sorted: vec![],
            indexed: vec![],
        },
        fields,
    })
}

/// A relation as the principal may see it: a link field it may not read
/// is described by its shape alone, as a type's own field is — never by
/// what it is computed from, what it means or how the host marked it
/// (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`).
fn relation_description(t: &TypeName, r: &RelationDecl, who: &Principal) -> RelationDecl {
    let link = crate::links::link_type(t, r);
    let mut r = r.clone();
    // A link field the principal may not know of is not described at all.
    r.fields
        .retain(|f| field_access(who, &link, f) != Access::None);
    for f in &mut r.fields {
        if !field_access(who, &link, f).reads() {
            f.derived = None;
            f.function = None;
            f.depends_on.clear();
            f.searchable = false;
            f.description = None;
            f.label = None;
            f.embed = None;
            f.annotations.clear();
        }
    }
    r
}

/// A verb is callable when every field its effect writes is granted to the
/// principal, or — for a verb that reaches outside the lake — when the
/// principal may write the type as a whole.
fn verb_callable(v: &VerbDecl, t: &TypeDecl, who: &Principal) -> bool {
    match &v.effect {
        VerbEffect::Write { set } => {
            !set.is_empty()
                && set
                    .iter()
                    .all(|a| who.grants.access(&t.name, &a.field).writes())
        }
        VerbEffect::External { .. } => type_access(who, &t.name).writes(),
    }
}

fn verb_description(
    v: &VerbDecl,
    declared: &[Declared],
    who: &Principal,
) -> Option<VerbDescription> {
    let t = find_type(declared, &v.on)?;
    if !verb_callable(v, t, who) {
        return None;
    }
    Some(VerbDescription {
        name: v.name.clone(),
        on: v.on.clone(),
        scope: v.scope,
        // The payload is the caller's own input: every field of it is its to
        // write.
        payload: v
            .payload
            .iter()
            .map(|f| FieldDescription::of(f, Access::Write))
            .collect(),
        risk: v.risk,
        external: matches!(v.effect, VerbEffect::External { .. }),
        // Whether the host binds it is the store's to say.
        executor: match &v.effect {
            VerbEffect::External { executor } => Some(ExecutorDescription {
                name: executor.clone(),
                bound: false,
            }),
            VerbEffect::Write { .. } => None,
        },
        description: v.description.clone(),
    })
}

#[cfg(test)]
mod tests;

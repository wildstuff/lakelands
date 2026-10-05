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

/// An object set's filter by the set's name, as a caller holds the sets.
pub type Sets<'a> = &'a dyn Fn(&str) -> Option<Condition>;

/// The sets among `declared`.
fn sets_of(declared: &[Declared]) -> impl Fn(&str) -> Option<Condition> + '_ {
    move |name| {
        declared.iter().find_map(|d| match &d.declaration {
            Declaration::ObjectSet(s) if s.name == name => Some(s.filter.clone()),
            _ => None,
        })
    }
}

/// The fields a condition names, and those the sets it names read, through
/// the sets those name: a set is its filter, so who reads a condition
/// reads what its sets read. `None` when it does not parse, or names a set
/// `sets` does not answer.
fn named(c: &Condition, sets: Sets<'_>) -> Option<std::collections::BTreeSet<FieldName>> {
    fn gather(
        c: &Condition,
        sets: Sets<'_>,
        seen: &mut Vec<String>,
        out: &mut std::collections::BTreeSet<FieldName>,
    ) -> Option<()> {
        let e = parsed(c)?;
        out.extend(crate::derive::reads(&e));
        let mut names = Vec::new();
        in_sets(&e, &mut names);
        for n in names
            .into_iter()
            .filter(|n| !seen.contains(n))
            .collect::<Vec<_>>()
        {
            seen.push(n.clone());
            gather(&sets(&n)?, sets, seen, out)?;
        }
        Some(())
    }
    let mut out = Default::default();
    gather(c, sets, &mut Vec::new(), &mut out).map(|()| out)
}

/// The sets an expression names with `in_set`.
fn in_sets(e: &Expr, out: &mut Vec<String>) {
    use lakelands_api::expr::{Func, Literal};
    if let Expr::Call {
        func: Func::InSet,
        args,
    } = e
        && let Some(Expr::Lit {
            value: Literal::Text(name),
        }) = args.first()
    {
        out.push(name.clone());
    }
    crate::expr::children(e).for_each(|c| in_sets(c, out));
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

/// Whether a proposed declaration is listed to `who`, over `declared` as
/// the proposal would leave it (013, extended): as [`lake`] would describe
/// it once in force, but a verb by the fields it names and not by who may
/// call it, since a reviewer need not be one who may.
pub fn listed(d: &Declaration, declared: &[Declared], who: &Principal) -> bool {
    let described = |ty: &TypeName| one_type(ty, declared, who).is_some();
    let holds = |c: Option<&Condition>, ty| condition_visible(c, ty, declared, who);
    match d {
        // A view without its definition would read as a stored type.
        Declaration::Type(t) => {
            described(&t.name) && (t.view.as_deref()).is_none_or(|v| view_visible(v, declared, who))
        }
        Declaration::Trigger(t) => described(&t.on) && holds(t.when.as_ref(), &t.on),
        Declaration::ObjectSet(s) => described(&s.of) && holds(Some(&s.filter), &s.of),
        // It names the fields it sets and says something of those it reads.
        Declaration::Verb(v) => {
            let VerbEffect::Write { set } = &v.effect else {
                return described(&v.on) && holds(v.requires.as_ref(), &v.on);
            };
            let known = |n: &FieldName| {
                let t = find_type(declared, &v.on);
                let f = t.and_then(|t| Some((t, t.fields.iter().find(|f| &f.name == n)?)));
                f.is_some_and(|(t, f)| field_access(who, t, f) != Access::None)
            };
            // In a value `payload.<name>` is the call's and no field.
            let value = |c: &Condition| {
                let fields = parsed(c).map(|e| crate::derive::verb_reads(&e).0);
                reads_all(&fields.unwrap_or_default(), &v.on, declared, who)
            };
            let all = |a: &lakelands_api::ontology::Assignment| known(&a.field) && value(&a.value);
            described(&v.on)
                && holds(v.requires.as_ref(), &v.on)
                && (who.grants.schema || set.iter().all(all))
        }
        Declaration::Function(f) => function_visible(f, declared, who),
        Declaration::ValueType(_) => true,
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
        || c.is_none_or(|c| {
            named(c, &sets_of(declared)).is_some_and(|n| reads_all(&n, ty, declared, who))
        })
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
                names.extend(named(f, &|_| None)?);
            }
            for m in measures {
                aggregated(&parsed(&m.expr)?, false, &mut names);
            }
            Some((source.clone(), names))
        })
        .collect()
}

/// The fields of `ty` a declaration reads: what it probes
/// ([`declaration_probes`]) and the fields a verb sets.
pub fn declaration_reads(
    d: &Declaration,
    ty: &TypeName,
    sets: Sets<'_>,
) -> std::collections::BTreeSet<FieldName> {
    let mut names = declaration_probes(d, ty, sets);
    if let Declaration::Verb(v) = d
        && let VerbEffect::Write { set } = &v.effect
        && &v.on == ty
    {
        names.extend(set.iter().map(|a| a.field.clone()));
    }
    names
}

/// The fields of `ty` whose values a declaration reads: a derived field's
/// expression on `ty` itself, the fields a derived relation matches, its
/// own and those of `ty` as its target, what a view reads of `ty` as a
/// source ([`view_reads`]), a set's `where`, a trigger's `when`, and a
/// verb's `requires` and what it sets its fields from (where
/// `payload.<name>` is the call's and reads no field), each with what the
/// sets it names read, as `sets` answers them. A condition that does not
/// parse reads nothing here.
pub fn declaration_probes(
    d: &Declaration,
    ty: &TypeName,
    sets: Sets<'_>,
) -> std::collections::BTreeSet<FieldName> {
    // A set `sets` does not answer takes none of the condition's own reads.
    let own = |c: &Condition| parsed(c).map(|e| crate::derive::reads(&e));
    let of = |c: &Condition| named(c, sets).or_else(|| own(c)).unwrap_or_default();
    match d {
        Declaration::Type(t) => {
            let own = t.fields.iter().filter(|_| &t.name == ty);
            let pairs = |r: &RelationDecl| r.derived.clone().map(|m| m.pairs).unwrap_or_default();
            let matching = (t.relations.iter().filter(|_| &t.name == ty)).flat_map(pairs);
            let matched = (t.relations.iter().filter(|r| &r.target == ty)).flat_map(pairs);
            let view = t
                .view
                .iter()
                .flat_map(|v| view_reads(v).unwrap_or_default());
            own.filter_map(|f| f.derived.as_ref())
                .flat_map(of)
                .chain(matching.map(|p| p.from))
                .chain(matched.map(|p| p.to))
                .chain(view.filter(|(source, _)| source == ty).flat_map(|v| v.1))
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
                }
            }
            names
        }
        _ => Default::default(),
    }
}

/// Leaves a write's answer the reactions it names to the reader `lake` is
/// described to (024): the triggers and object sets `describe` lists to
/// it, and of those only where it reads every field the condition names.
/// Being shown a definition is not reading what it reads, and naming a
/// reaction says that its condition held.
pub fn named_reactions(lake: &LakeDescription, set_off: &mut lakelands_api::write::SetOff) {
    let reads = |ty: &TypeName, c: Option<&Condition>| {
        let fields = lake.types.iter().find(|t| &t.name == ty).map(|t| &t.fields);
        let read = |n: &FieldName| {
            fields.is_some_and(|f| f.iter().any(|f| &f.name == n && f.access.reads()))
        };
        let sets = |n: &str| {
            let named = lake.object_sets.iter().find(|d| d.name == n);
            named.map(|d| d.filter.clone())
        };
        c.is_none_or(|c| named(c, &sets).is_some_and(|names| names.iter().all(read)))
    };
    let fired = |t: &lakelands_api::TriggerName| {
        let mut named = lake.triggers.iter().filter(|d| &d.name == t);
        named.any(|d| reads(&d.on, d.when.as_ref()))
    };
    set_off.fired.retain(fired);
    let listed = |s: &String| {
        let mut named = lake.object_sets.iter().filter(|d| &d.name == s);
        named.any(|d| reads(&d.of, Some(&d.filter)))
    };
    set_off.joined.retain(listed);
    set_off.left.retain(listed);
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

/// Whether `who` is shown `t`: it has some access to the type or to one
/// of its fields, and is shown every type that `t` is made of
/// (`docs/decisions/006-grants-not-roles.md`, extended): a link type is
/// its owner's edges and a view is its sources' records computed, so one
/// who is told the other does not exist is told the same of what is made
/// of it, whatever a grant on its own name says. Each type is asked once,
/// so views an imported log holds in a cycle are judged by their grants
/// and the read that refuses the cycle is reached. `find` answers a type
/// by its name; the description and every door that asks before it names
/// a type ask this one function.
pub fn type_shown<'a>(
    t: &'a TypeDecl,
    find: &dyn Fn(&TypeName) -> Option<&'a TypeDecl>,
    who: &Principal,
) -> bool {
    let own = |t: &TypeDecl| {
        let fields = &mut t.fields.iter();
        type_access(who, &t.name) != Access::None
            || fields.any(|f| field_access(who, t, f) != Access::None)
    };
    let (mut asked, mut next) = (vec![&t.name], vec![t]);
    while let Some(t) = next.pop() {
        let owner = crate::links::link_type_parts(&t.name).map(|p| p.0);
        let sources = (t.view.iter())
            .flat_map(|v| std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source)));
        for o in owner.iter().chain(sources).filter_map(find) {
            if asked.contains(&&o.name) {
                continue;
            }
            if !own(o) {
                return false;
            }
            asked.push(&o.name);
            next.push(o);
        }
    }
    own(t)
}

fn type_description(
    t: &TypeDecl,
    d: &Declared,
    declared: &[Declared],
    who: &Principal,
) -> Option<TypeDescription> {
    if !type_shown(t, &|n| find_type(declared, n), who) {
        return None;
    }
    let fields: Vec<FieldDescription> = t
        .fields
        .iter()
        .map(|f| (f, field_access(who, t, f)))
        .filter(|(_, a)| *a != Access::None)
        .map(|(f, a)| FieldDescription::of(f, a))
        .collect();
    let whole = type_access(who, &t.name);
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
            kept: None,
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

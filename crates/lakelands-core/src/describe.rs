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
//!
//! A value type is answered whole, and only to a principal shown a field
//! that uses it, where `describe` shows that field (a masked one too, whose
//! shape names it), or who holds `schema` (lakelands#1088). Its `one_of`
//! and `constraint` say what those fields' values may be.

use lakelands_api::describe::{
    ExecutorDescription, FieldDescription, LakeDescription, LakeRights, TypeAccess,
    TypeDescription, TypeLifecycle, TypeLineage, TypeRights, VerbDescription,
};
use lakelands_api::engine::Declared;
use lakelands_api::expr::{Condition, Expr};
use lakelands_api::ontology::{
    Authority, Derived, RelationDecl, Risk, TypeDecl, VerbDecl, VerbEffect, ViewDecl,
};
use lakelands_api::write::{Declaration, FunctionDecl, SettingDecl};
use lakelands_api::{Access, FieldName, LakeId, Principal, TypeName, ValueTypeName};

use crate::rights::{field_access, granted_access, only_holder, pinnable, type_access};
use crate::sight::Sight;

impl Sight<'_> {
    /// The lake as its principal may see it.
    pub fn describe(&self, lake: &LakeId) -> LakeDescription {
        let (declared, who) = (self.force.declared(), self.who());
        let types = each(&declared, |d| match &d.declaration {
            Declaration::Type(t) => self.type_description(t, d, &declared),
            _ => None,
        });
        let verbs = each(&declared, |d| match &d.declaration {
            Declaration::Verb(v) => self.verb_description(v),
            _ => None,
        });
        let value_types = each(&declared, |d| match &d.declaration {
            Declaration::ValueType(v) if self.uses_shown(&v.name, &declared) => Some(v.clone()),
            _ => None,
        });
        let value_type_versions = value_types
            .iter()
            .filter_map(|v| Some((v.name.clone(), version_of(&declared, &v.name)?)))
            .collect();
        let listed = |d: &Declared| self.listed(&d.declaration, &declared);
        let functions = each(&declared, |d| match &d.declaration {
            Declaration::Function(f) if listed(d) => Some(f.clone()),
            _ => None,
        });
        let triggers = each(&declared, |d| match &d.declaration {
            Declaration::Trigger(t) if listed(d) => Some(t.clone()),
            _ => None,
        });
        let object_sets = each(&declared, |d| match &d.declaration {
            Declaration::ObjectSet(s) if listed(d) => Some(s.clone()),
            _ => None,
        });
        LakeDescription {
            lake: lake.clone(),
            types,
            verbs,
            value_types,
            value_type_versions,
            functions,
            triggers,
            object_sets,
            settings: Default::default(),
            may: LakeRights {
                review: who.grants.review,
                schema: who.grants.schema,
                ingest: who.grants.ingest,
            },
        }
    }

    /// One type as its principal may see it; `None` when it may see none
    /// of it.
    pub fn describe_type(&self, ty: &TypeName) -> Option<TypeDescription> {
        let declared = self.force.declared();
        let mut t = declared.iter().find_map(|d| match &d.declaration {
            Declaration::Type(t) if &t.name == ty => self.type_description(t, d, &declared),
            _ => None,
        })?;
        // The declarations its key names: those of the fields as shown
        // (lakelands#1060).
        let named = t.lifecycle.value_types.keys();
        let declaration = |n| {
            declared.iter().find_map(|d| match &d.declaration {
                Declaration::ValueType(v) if &v.name == n => Some(v.clone()),
                _ => None,
            })
        };
        t.value_types = named.filter_map(declaration).collect();
        Some(t)
    }

    /// Whether its principal reads every one of `names` on type `ty`.
    fn reads_all<'a>(&self, names: impl IntoIterator<Item = &'a FieldName>, ty: &TypeName) -> bool {
        self.force.ty(ty).is_some() && names.into_iter().all(|n| self.reads_from(ty, n))
    }

    /// Whether its principal reads where a verb's path begins at `n` on
    /// `ty`: the field, or, where `ty` has none of that name, the relation,
    /// which it is shown (055, extended).
    pub fn reads_from(&self, ty: &TypeName, n: &FieldName) -> bool {
        let t = self
            .force
            .ty(ty)
            .filter(|t| !t.fields.iter().any(|f| &f.name == n));
        let mut named = t
            .into_iter()
            .flat_map(|t| t.relations.iter().map(move |r| (t, r)));
        self.reads(ty, n)
            || named.any(|(t, r)| r.name.as_str() == n.as_str() && self.relation(t, r))
    }
}

/// The version of the value type `name` among `declared`.
fn version_of(declared: &[Declared], name: &ValueTypeName) -> Option<u32> {
    declared.iter().find_map(|d| match &d.declaration {
        Declaration::ValueType(v) if &v.name == name => Some(d.version),
        _ => None,
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
    let mut out = std::collections::BTreeSet::new();
    gather(c, sets, &mut Vec::new(), &mut |e| {
        out.extend(crate::derive::reads(e))
    })?;
    Some(out)
}

/// The paths a condition reads, and those of the sets it names, as
/// [`named`] gathers them.
fn paths_named(c: &Condition, sets: Sets<'_>) -> Option<Vec<Vec<FieldName>>> {
    let mut out = Vec::new();
    gather(c, sets, &mut Vec::new(), &mut |e| {
        let mut paths = Vec::new();
        lakelands_api::expr::paths(e, &mut paths);
        out.extend(paths.into_iter().map(<[FieldName]>::to_vec));
    })?;
    Some(out)
}

/// `each` over a condition and the sets it names, through the sets those
/// name, each once.
fn gather(
    c: &Condition,
    sets: Sets<'_>,
    seen: &mut Vec<String>,
    each: &mut dyn FnMut(&Expr),
) -> Option<()> {
    let e = parsed(c)?;
    each(&e);
    let mut names = Vec::new();
    in_sets(&e, &mut names);
    for n in names
        .into_iter()
        .filter(|n| !seen.contains(n))
        .collect::<Vec<_>>()
    {
        seen.push(n.clone());
        gather(&sets(&n)?, sets, seen, each)?;
    }
    Some(())
}

/// The sets an expression names with `in_set`.
#[doc(hidden)]
pub fn in_sets(e: &Expr, out: &mut Vec<String>) {
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

impl Sight<'_> {
    /// Whether its principal is shown the declaration in force under `d`'s
    /// name: whether [`Self::describe`] names it. A refusal names a
    /// declaration only to one who is. Over a type as a migration would
    /// leave it, a field it takes away is gone, and nothing that reads it
    /// would be shown to anyone without the `schema` grant.
    pub fn shows(&self, d: &Declaration) -> bool {
        match d {
            // A view is shown when its definition is, not only its type's name.
            Declaration::Type(t) => self.ty(&t.name).is_ok_and(|s| {
                t.view.is_none() || s.view.as_deref().is_some_and(|v| self.view_visible(v))
            }),
            _ => {
                let declared = self.force.declared();
                let mut named = declared.iter().map(|x| &x.declaration);
                named.any(|x| match x {
                    _ if x.what() != d.what() => false,
                    Declaration::Verb(v) => self.verb_description(v).is_some(),
                    x => self.listed(x, &declared),
                })
            }
        }
    }

    /// Whether what its principal is described of `d` holds, in an
    /// expression, a node `test` accepts: a type member by member, as
    /// [`Self::describe`] gives it (024), any other declaration whole.
    pub fn shows_where(&self, d: &Declaration, test: &dyn Fn(&Expr) -> bool) -> bool {
        let Declaration::Type(t) = d else {
            return self.shows(d) && crate::expr::declares(d, test);
        };
        let Some(shown) = self.describe_type(&t.name) else {
            return false;
        };
        let mut t = t.clone();
        t.fields.retain_mut(|f| {
            let said = shown.fields.iter().find(|s| s.name == f.name);
            said.map(|s| f.derived.clone_from(&s.derived)).is_some()
        });
        (t.relations, t.view) = (shown.relations, shown.view.map(Box::new));
        crate::expr::declares(&Declaration::Type(t), test)
    }

    /// Whether a proposed declaration is listed to its principal, over the
    /// declarations as the proposal would leave them (013, extended): as
    /// [`Self::describe`] would describe it once in force, but a verb by
    /// the fields it names and not by who may call it, since a reviewer
    /// need not be one who may.
    pub fn lists(&self, d: &Declaration) -> bool {
        self.listed(d, &self.force.declared())
    }

    /// [`Self::lists`], `declared` being the sight's declarations, listed
    /// once for many.
    fn listed(&self, d: &Declaration, declared: &[Declared]) -> bool {
        let who = self.who();
        let described = |ty: &TypeName| self.ty(ty).is_ok();
        let holds = |c: Option<&Condition>, ty| self.condition_visible(c, ty, declared);
        match d {
            // A view without its definition would read as a stored type.
            Declaration::Type(t) => {
                described(&t.name) && (t.view.as_deref()).is_none_or(|v| self.view_visible(v))
            }
            Declaration::Trigger(t) => {
                described(&t.on)
                    && holds(t.when.as_ref(), &t.on)
                    && self.across_unheld(t.when.as_ref(), &t.on, declared)
            }
            Declaration::ObjectSet(s) => {
                described(&s.of)
                    && holds(Some(&s.filter), &s.of)
                    && self.across_unheld(Some(&s.filter), &s.of, declared)
            }
            // It names the fields it sets and says something of those it reads.
            Declaration::Verb(v) => {
                // And what its paths read of other records, from a relation
                // it is shown, of a holder of `schema` too, as `describe`
                // asks it of a verb in force (055).
                let on = self.force.ty(&v.on);
                let far = on.and_then(|on| v.reads_across(on, &|n| self.force.ty(n)));
                let far =
                    far.is_some_and(|f| f.iter().all(|(t, f)| field_access(who, t, f).reads()));
                let field =
                    |n: &FieldName| on.is_some_and(|t| t.fields.iter().any(|f| &f.name == n));
                let from = |p: &Vec<FieldName>| field(&p[0]) || self.reads_from(&v.on, &p[0]);
                if !far || !v.record_paths().is_some_and(|p| p.iter().all(from)) {
                    return false;
                }
                let VerbEffect::Write { set } = &v.effect else {
                    return described(&v.on) && holds(v.requires.as_ref(), &v.on);
                };
                let known = |n: &FieldName| {
                    let t = self.force.ty(&v.on);
                    let f = t.and_then(|t| Some((t, t.fields.iter().find(|f| &f.name == n)?)));
                    f.is_some_and(|(t, f)| field_access(who, t, f) != Access::None)
                };
                // In a value `payload.<name>` is the call's and no field.
                let value = |c: &Condition| {
                    let fields = parsed(c).map(|e| crate::derive::verb_reads(&e).0);
                    self.reads_all(&fields.unwrap_or_default(), &v.on)
                };
                let all =
                    |a: &lakelands_api::ontology::Assignment| known(&a.field) && value(&a.value);
                described(&v.on)
                    && holds(v.requires.as_ref(), &v.on)
                    && (who.grants.schema || set.iter().all(all))
            }
            Declaration::Function(f) => self.function_visible(f),
            Declaration::ValueType(v) => self.uses_shown(&v.name, declared),
            Declaration::Setting(s) => self.setting_visible(s),
            _ => false,
        }
    }

    /// Whether its principal holds `schema`, or is shown, in `declared`
    /// (the sight's declarations), a field whose value type is `vt`: a
    /// field of a type it is shown, a link type's among them, a verb's
    /// payload field or a function's input, each where `describe`
    /// shows it (lakelands#1088). The uses are found in one pass a sight.
    fn uses_shown(&self, vt: &ValueTypeName, declared: &[Declared]) -> bool {
        let who = self.who();
        who.grants.schema
            || self
                .used
                .get_or_init(|| self.used_in(declared))
                .contains(vt)
    }

    /// The value types of the uses [`Self::uses_shown`] counts.
    fn used_in(&self, declared: &[Declared]) -> std::collections::BTreeSet<ValueTypeName> {
        type Field = lakelands_api::ontology::FieldDecl;
        let who = self.who();
        let of = |fs: &mut dyn Iterator<Item = &Field>| -> Vec<ValueTypeName> {
            fs.filter_map(|f| f.ty.value_type.clone()).collect()
        };
        let known = |t: &TypeDecl| {
            let mut fs = t
                .fields
                .iter()
                .filter(|f| field_access(who, t, f) != Access::None);
            of(&mut fs)
        };
        let mut used = std::collections::BTreeSet::new();
        for d in declared {
            match &d.declaration {
                // A link type is declared as its own: its fields are judged
                // as a link row is, on it (lakelands#1092).
                Declaration::Type(t) if self.ty(&t.name).is_ok() => used.extend(known(t)),
                Declaration::Verb(v) if self.verb_description(v).is_some() => {
                    used.extend(of(&mut v.payload.iter()))
                }
                Declaration::Function(f) if self.function_visible(f) => {
                    used.extend(of(&mut f.inputs.iter()))
                }
                _ => {}
            }
        }
        used
    }

    /// Whether nothing a set's `where` or a trigger's `when` over `ty`
    /// reads across an edge, with the sets it names, is held from its
    /// principal: decided for no reader, what such a condition holds for a
    /// record tells what it reads there (055, extended).
    fn across_unheld(&self, c: Option<&Condition>, ty: &TypeName, all: &[Declared]) -> bool {
        let Some(on) = self.force.ty(ty) else {
            return true;
        };
        if on
            .view
            .as_deref()
            .is_some_and(|v| !self.unheld_under(v, &mut Vec::new()))
        {
            return false;
        }
        let Some(c) = c else {
            return true;
        };
        let paths = paths_named(c, &sets_of(all));
        let ty = |n: &TypeName| self.force.ty(n);
        paths.is_some_and(|p| crate::rights::holds_nothing_across(self.who(), on, &p, &ty))
    }

    /// Whether nothing of the records the view `v` reads is held from its
    /// principal: decided for no reader, a membership over the view's
    /// whole face tells what each of them adds (lakelands#1185). It holds
    /// `restricted`, no `only` on a source at any depth nor, of a link
    /// type, on its ends, and reads every field a source's `where`,
    /// grouping and measures read. `inside` are the views judged so far.
    fn unheld_under<'v>(&'v self, v: &'v ViewDecl, inside: &mut Vec<&'v ViewDecl>) -> bool {
        let who = self.who();
        let ty = |n: &TypeName| self.force.ty(n);
        inside.push(v);
        who.grants.restricted
            && view_reads(v).is_some_and(|reads| {
                reads.iter().all(|(from, names)| {
                    let Some(t) = ty(from) else { return false };
                    let inner = t.view.as_deref();
                    let inner = inner.filter(|i| !inside.iter().any(|v| std::ptr::eq(*v, *i)));
                    crate::rights::own_only(who, from).is_empty()
                        && crate::rights::held_ends(who, t, &ty).is_empty()
                        && self.reads_all(names, from)
                        && inner.is_none_or(|i| self.unheld_under(i, inside))
                })
            })
    }

    /// [`Self::across_unheld`] of a set's `where`, over the declarations
    /// in force.
    pub fn set_across_unheld(&self, s: &lakelands_api::write::ObjectSetDecl) -> bool {
        self.across_unheld(Some(&s.filter), &s.of, &self.force.declared())
    }

    /// Whether a condition over type `ty` may be shown to its principal.
    fn condition_visible(&self, c: Option<&Condition>, ty: &TypeName, all: &[Declared]) -> bool {
        let read = |c| named(c, &sets_of(all)).is_some_and(|n| self.reads_all(&n, ty));
        self.who().grants.schema || c.is_none_or(read)
    }

    /// Whether a view may be shown whole to its principal: it reads every
    /// field the view reads of its sources ([`view_reads`]), and of each
    /// source that is itself a view what that one reads, as a read of the
    /// view probes them (024, extended). So a definition is shown to one
    /// its read answers, and to a holder of `schema`.
    fn view_visible(&self, v: &ViewDecl) -> bool {
        self.who().grants.schema || self.reads_view(v, &mut Vec::new())
    }

    /// [`Self::view_visible`] without the grant; `inside` are the views
    /// judged so far, since an imported log may hold views in a cycle.
    fn reads_view<'v>(&'v self, v: &'v ViewDecl, inside: &mut Vec<&'v ViewDecl>) -> bool {
        let source = |n: &TypeName| self.force.ty(n);
        inside.push(v);
        view_reads(v).is_some_and(|reads| {
            reads.iter().all(|(from, names)| {
                let inner = source(from).and_then(|s| s.view.as_deref());
                let inner = inner.filter(|i| !inside.iter().any(|v| std::ptr::eq(*v, *i)));
                self.reads_all(names, from) && inner.is_none_or(|i| self.reads_view(i, inside))
            })
        })
    }
}

/// The fields a view reads from each of its sources, its own and the
/// merged ones in order ([`source_reads`]). `None` when one of its
/// conditions does not parse.
pub fn view_reads(v: &ViewDecl) -> Option<Vec<(TypeName, std::collections::BTreeSet<FieldName>)>> {
    std::iter::once((&v.source, v.r#where.as_ref(), &v.group_by, &v.measures))
        .chain(
            v.merge
                .iter()
                .map(|m| (&m.source, m.r#where.as_ref(), &m.group_by, &m.measures)),
        )
        .map(|(source, filter, group_by, measures)| {
            let names = source_reads(filter, group_by, measures)?;
            Some((source.clone(), names.into_iter().collect()))
        })
        .collect()
}

/// The fields a view reads of one source, in the order a read probes
/// them: a source is read as a read of it (024), so its grouping, its
/// `where` and what its measures aggregate ([`measure_reads`]).
/// `describe`, `declare`, the migration preflight and a read of the view
/// all go by it. `None` when one of its conditions does not parse.
pub fn source_reads(
    filter: Option<&Condition>,
    group_by: &[FieldName],
    measures: &[lakelands_api::ontology::Measure],
) -> Option<Vec<FieldName>> {
    let mut names = group_by.to_vec();
    if let Some(f) = filter {
        names.extend(named(f, &|_| None)?);
    }
    for m in measures {
        names.extend(measure_reads(&parsed(&m.expr)?));
    }
    Some(names)
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

/// The types `d` names, the only ones [`declaration_probes`] answers
/// anything for.
pub fn probed_types(d: &Declaration) -> std::collections::BTreeSet<TypeName> {
    match d {
        Declaration::Type(t) => {
            let views = t
                .view
                .iter()
                .flat_map(|v| view_reads(v).unwrap_or_default());
            let targets = t.relations.iter().map(|r| r.target.clone());
            let named = [t.name.clone()].into_iter().chain(targets);
            named.chain(views.map(|(source, _)| source)).collect()
        }
        Declaration::ObjectSet(s) => [s.of.clone()].into(),
        Declaration::Trigger(t) => [t.on.clone()].into(),
        Declaration::Verb(v) => [v.on.clone()].into(),
        _ => Default::default(),
    }
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
            let side = |r: &RelationDecl, i: usize| {
                r.derived
                    .as_ref()
                    .map(|d| d.reads()[i].clone())
                    .unwrap_or_default()
            };
            let matching = (t.relations.iter().filter(|_| &t.name == ty)).flat_map(|r| side(r, 0));
            let matched = (t.relations.iter().filter(|r| &r.target == ty)).flat_map(|r| side(r, 1));
            let view = t
                .view
                .iter()
                .flat_map(|v| view_reads(v).unwrap_or_default());
            own.filter_map(|f| f.derived.as_ref())
                .flat_map(of)
                .chain(matching)
                .chain(matched)
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
/// group, an earlier measure or a grouping field, and is no source field,
/// whatever the source holds under that name: a read that probed it there
/// would tell one kept from the field that it is declared (024, extended).
pub fn measure_reads(e: &Expr) -> std::collections::BTreeSet<FieldName> {
    let mut out = std::collections::BTreeSet::new();
    aggregated(e, false, &mut out);
    out
}

/// The paths inside an aggregate's arguments.
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

impl Sight<'_> {
    /// Whether its principal is told a host's setting declaration: a
    /// holder of `schema`, or one who may write the setting, writing
    /// `value` of `sys.setting` under a grant whose `only` there admits
    /// the setting's key and group (063, extended).
    pub fn setting_visible(&self, s: &SettingDecl) -> bool {
        let who = self.who();
        if who.grants.schema {
            return true;
        }
        let ty = TypeName::from(lakelands_api::seam::SETTING_TYPE);
        let Some(t) = self.force.ty(&ty) else {
            return false;
        };
        let value = t.fields.iter().find(|f| f.name.as_str() == "value");
        let writes = value.is_some_and(|f| field_access(who, t, f).writes());
        let admits = |(f, v): &(FieldName, lakelands_api::Value)| match f.as_str() {
            "key" => v.as_str() == Some(s.key.as_str()),
            "group" => v.as_str().is_some() && v.as_str() == s.group.as_deref(),
            _ => false,
        };
        writes && crate::rights::own_only(who, &ty).iter().all(admits)
    }

    /// Whether a function may be shown to its principal: it computes at
    /// least one field, and every type where it does is shown to it,
    /// which reads that field and every input, which the function reads
    /// from the fields of the same names.
    fn function_visible(&self, f: &FunctionDecl) -> bool {
        if self.who().grants.schema {
            return true;
        }
        let computed = |x: &&lakelands_api::ontology::FieldDecl| {
            x.function.as_deref() == Some(f.name.as_str())
        };
        let types = self.force.types();
        let mut uses = (types.iter())
            .flat_map(|t| t.fields.iter().filter(computed).map(move |x| (t, x)))
            .peekable();
        uses.peek().is_some()
            && uses.all(|(t, x)| {
                let read = std::iter::once(&x.name).chain(f.inputs.iter().map(|i| &i.name));
                self.ty(&t.name).is_ok() && self.reads_all(read, &t.name)
            })
    }
}

/// What `pick` takes from each declaration, in declaration order.
fn each<'a, T>(declared: &'a [Declared], pick: impl FnMut(&'a Declared) -> Option<T>) -> Vec<T> {
    declared.iter().filter_map(pick).collect()
}

/// Whether `who` is shown `t`: its grants give it some access to the type
/// or to one of its fields, a `sensitive` field's raise not among them,
/// and it is shown every type that `t` is made of
/// (`docs/decisions/006-grants-not-roles.md`, extended): a link type is
/// the edges from its owner to its relation's target, and a view is its
/// sources' records computed, so one who is told the other does not exist
/// is told the same of what is made of it, whatever a grant on its own
/// name says. Each type is asked once,
/// so views an imported log holds in a cycle are judged by their grants
/// and the read that refuses the cycle is reached. `find` answers a type
/// by its name; the description and every door that asks before it names
/// a type ask this one function, through [`Sight::ty`], and so do the
/// judgement of a relation and the making of a sight.
pub(crate) fn type_shown<'a>(
    t: &'a TypeDecl,
    find: &dyn Fn(&TypeName) -> Option<&'a TypeDecl>,
    who: &Principal,
) -> bool {
    // A type keyed by a field its principal may not know of is not shown:
    // every record key would hand out that field's value. A view's key is
    // judged where its values come from too (lakelands#782).
    let known = |t: &TypeDecl, key: &[FieldName]| {
        let mut key = t.fields.iter().filter(|f| key.contains(&f.name));
        key.all(|f| field_access(who, t, f) != Access::None)
    };
    let own = |t: &TypeDecl| {
        let fields = &mut t.fields.iter();
        let mut sources = t.key_sources().into_iter();
        known(t, t.key_fields())
            && sources.all(|(s, key)| find(s).is_none_or(|s| known(s, key)))
            && (type_access(who, &t.name) != Access::None
                || fields.any(|f| granted_access(who, t, f) != Access::None))
    };
    let (mut asked, mut next) = (vec![&t.name], vec![t]);
    while let Some(t) = next.pop() {
        let owner = crate::links::link_type_parts(&t.name).map(|p| p.0);
        // A link type's `to` names its relation's target.
        let target = (owner.iter())
            .flat_map(|_| &t.fields)
            .find(|f| f.name.as_str() == crate::links::TO)
            .and_then(|f| f.ty.target.as_ref());
        let sources = (t.view.iter())
            .flat_map(|v| std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source)));
        for o in owner.iter().chain(target).chain(sources).filter_map(find) {
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

/// The types `t`'s sight and its fields' rest on, `t` first: what
/// [`type_shown`] judges it by, what it is made of, and the target of
/// every reference it holds, its own and its relations' link fields, as
/// `rights::hide_unshown_references` hides one to a type its
/// reader is not shown. One of them re-keyed may show a value of a field
/// that reader may not know of as a key (lakelands#913).
pub fn shown_by<'a>(
    t: &'a TypeDecl,
    find: &dyn Fn(&TypeName) -> Option<&'a TypeDecl>,
) -> Vec<&'a TypeName> {
    let (mut asked, mut next) = (vec![&t.name], vec![t]);
    while let Some(t) = next.pop() {
        let owner = crate::links::link_type_parts(&t.name).map(|p| p.0);
        let links = t.relations.iter().flat_map(|r| &r.fields);
        let targets = t
            .fields
            .iter()
            .chain(links)
            .filter_map(|f| f.ty.target.as_ref());
        let sources = t.key_sources().into_iter().map(|(s, _)| s);
        for o in owner.iter().chain(targets).chain(sources).filter_map(find) {
            if !asked.contains(&&o.name) {
                asked.push(&o.name);
                next.push(o);
            }
        }
    }
    asked
}

/// Whether `who` is shown relation `r` of `owner` (006, extended): it is
/// shown both ends, it may know of the reference field that carries it,
/// and of a derived relation it may know of every field the match pairs,
/// on both sides. A relation it is not shown is, to it, one the type does
/// not declare, at every door. A field it knows of and may not read leaves
/// the relation shown; a walk over it is refused (024, extended).
pub(crate) fn relation_shown<'a>(
    owner: &TypeDecl,
    r: &RelationDecl,
    find: crate::rights::Find<'_, 'a>,
    who: &Principal,
) -> bool {
    let Some(target) = find(&r.target) else {
        return false;
    };
    let known = |t: &TypeDecl, n: &FieldName| {
        let f = t.fields.iter().find(|f| &f.name == n);
        f.is_some_and(|f| field_access(who, t, f) != Access::None)
    };
    let [from, to] = r.derived.as_ref().map(Derived::reads).unwrap_or_default();
    type_shown(owner, &|n| find(n), who)
        && type_shown(target, find, who)
        && r.via.iter().all(|v| known(owner, v))
        && from.iter().all(|n| known(owner, n))
        && to.iter().all(|n| known(target, n))
}

impl Sight<'_> {
    fn type_description(
        &self,
        t: &TypeDecl,
        d: &Declared,
        declared: &[Declared],
    ) -> Option<TypeDescription> {
        let (who, find) = (self.who(), &|n: &TypeName| self.force.ty(n));
        self.ty(&t.name).ok()?;
        let fields: Vec<FieldDescription> = t
            .fields
            .iter()
            .map(|f| (f, field_access(who, t, f)))
            .filter(|(_, a)| *a != Access::None)
            .map(|(f, a)| FieldDescription::of(f, a))
            .collect();
        let is_view = t.view.is_some();
        // A link type's records are written through its relation's link
        // doors, never through `upsert` or `retire`.
        let is_link = crate::links::is_link_type(&t.name);
        // A holder of `only` writes its own records, and makes one only where
        // it writes every field its `only` names (051).
        let held = only_holder(who, t, self.force);
        let may = TypeRights {
            find: true,
            get: true,
            upsert: crate::rights::creates(who, t)
                && !is_view
                && !is_link
                && fields.iter().any(FieldDescription::authored_writable),
            retire: !is_link && crate::rights::may_retire(who, t),
            link: t
                .relations
                .iter()
                .filter(|r| {
                    r.carried_by_links()
                        && r.authority == Authority::Authored
                        && self.relation(t, r)
                })
                .filter(|r| {
                    let lt = crate::links::link_type(&t.name, r);
                    lt.fields
                        .iter()
                        .filter(|f| {
                            f.name.as_str() == crate::links::FROM
                                || f.name.as_str() == crate::links::TO
                        })
                        .all(|f| field_access(who, &lt, f).writes())
                })
                .map(|r| r.name.clone())
                .collect(),
            // Told only where it applies, so it names no field its reader may
            // not read (024).
            only: match crate::rights::check_only(who, t) {
                Ok(()) => crate::rights::own_only(who, &t.name).into_iter().collect(),
                Err(_) => Default::default(),
            },
        };
        let verbs = each(declared, |d| match &d.declaration {
            Declaration::Verb(v)
                if v.on == t.name
                    && self.callable(t, v)
                    && (!held || v.risk == Risk::Low || pinnable(who, t, Some(v))) =>
            {
                Some(v.name.clone())
            }
            _ => None,
        });
        let view = (t.view.clone()).filter(|v| self.view_visible(v));
        // The hash of the shape as it is shown (024, extended).
        let whole = d.schema_hash.as_deref().unwrap_or_default();
        let schema_hash = crate::rights::shown_hash(who, t, view.is_some(), whole, find);
        Some(TypeDescription {
            // Told to one who reads every field it reads, as a set's
            // condition is (lakelands#1119).
            constraints: (t.constraints.iter())
                .filter(|c| {
                    let e = c.holds.typed().ok();
                    e.is_some_and(|e| self.reads_all(e.fields_read(), &t.name))
                })
                .cloned()
                .collect(),
            name: t.name.clone(),
            // `describe_type` fills them; `describe` names them once.
            value_types: Vec::new(),
            description: t.description.clone(),
            label: t.label.clone(),
            language: t.language.clone(),
            view: view.map(|mut v| {
                v.keep_measures(|n| fields.iter().any(|f| &f.name == n));
                *v
            }),
            annotations: t.annotations.clone(),
            relations: t
                .relations
                .iter()
                .filter(|r| self.relation(t, r))
                .map(|r| relation_description(&t.name, r, who))
                .collect(),
            access: TypeAccess {
                // The key its records carry (lakelands#782).
                key: t.key_fields().to_vec(),
                // A valid time over a field its reader may not know of is not
                // named (024, extended).
                valid_from: (t.valid_from.clone()).filter(|v| fields.iter().any(|f| &f.name == v)),
                valid_to: (t.valid_to.clone()).filter(|v| fields.iter().any(|f| &f.name == v)),
                // Over the fields as shown: a masked field is not searchable
                // to its reader, nor is one it may not know of sourced (024).
                searchable: fields.iter().any(|f| f.searchable),
                field_meta: t.field_meta,
            },
            may,
            verbs,
            lineage: TypeLineage {
                sourced: (fields.iter())
                    .any(|f| matches!(f.authority, Authority::Source | Authority::Shared)),
                insert_only: t.insert_only,
                view_of: t.view.as_ref().map(|v| v.source.clone()),
            },
            lifecycle: TypeLifecycle {
                version: d.version,
                schema_hash,
                sorted: vec![],
                sorted_whole: vec![],
                indexed: vec![],
                kept: None,
                // Of the fields as shown, as the hash is (024, extended).
                value_types: (fields.iter())
                    .filter_map(|f| f.ty.value_type.as_ref())
                    .filter_map(|n| Some((n.clone(), version_of(declared, n)?)))
                    .collect(),
            },
            fields,
        })
    }

    /// Whether its principal is shown `v` and may call it: `describe`
    /// lists it.
    pub fn verb_shown(&self, v: &VerbDecl) -> bool {
        self.verb_description(v).is_some()
    }

    fn verb_description(&self, v: &VerbDecl) -> Option<VerbDescription> {
        let (who, t) = (self.who(), self.force.ty(&v.on)?);
        // A verb that waits for review is proposed, and a holder of `only`
        // proposes none that could move its pairs (051). One on a type not
        // shown would name it.
        let held = only_holder(who, t, self.force) && !pinnable(who, t, Some(v));
        if self.ty(&v.on).is_err() || !self.callable(t, v) || (held && v.risk != Risk::Low) {
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
            f.shape_alone();
        }
    }
    r
}

#[cfg(test)]
mod tests;

//! A view at `declare`, checked as the store checks it, in its order and
//! with its cases at its places: its sources, each declared, read once,
//! not itself through other views and not a view with a parameter; its
//! conditions; its grouping, field by field; its merged sources; the
//! fields it declares; and its parameters. Its names and conditions are
//! checked by `rules`, at the store's places. The mock is a second
//! implementation, so these are copies of the store's (`declare.rs`), held
//! to it by `a_view_is_declared_only_as_the_store_declares_it`.

use std::collections::BTreeSet;

use lakelands_api::error::{Case, Names, Naming, Place, Result};
use lakelands_api::expr::{Expr, Func, Literal, children};
use lakelands_api::id::{FieldName, TypeName};
use lakelands_api::ontology::{FieldDecl, StorageKind, TypeDecl, ViewDecl};

use super::rules::{Eval, condition, name};
use super::{at, invalid, invalid_field, view_reads};
use crate::state::State;

/// Refuse `t`'s view as the store refuses it at `declare`.
pub(super) fn check(st: &State, t: &TypeDecl) -> Result<()> {
    let Some(v) = t.view.as_deref() else {
        return Ok(());
    };
    let of_type = Place::Type {
        name: t.name.clone(),
    };
    let owner = [of_type.clone()];
    if let Some(chain) = source_cycle(st, t, v) {
        return Err(invalid(at(&owner, Case::ViewCycle { chain })));
    }
    let source = declared(st, &v.source, &owner)?;
    unbound_source(&owner, source)?;
    let mut read = BTreeSet::new();
    if let Some(w) = &v.r#where {
        let on = [of_type.clone(), Place::Where];
        params_read(&condition(w, Eval::View, &on)?, &mut read);
    }
    for g in &v.group_by {
        grouped_field(source, g, &owner)?;
    }
    for m in &v.measures {
        name(Naming::Measure, m.name.as_str())?;
        let name = m.name.clone();
        let on = [of_type.clone(), Place::Measure { name }];
        params_read(&condition(&m.expr, Eval::Group, &on)?, &mut read);
    }
    merged_sources(st, &owner, v, source, &mut read)?;
    // Every field the view reads from a source is one that source has.
    for (from, reads) in view_reads(v) {
        let Some(s) = st.types.get(&from).map(|t| &t.decl) else {
            continue;
        };
        if let Some(n) = reads
            .iter()
            .find(|n| !s.fields.iter().any(|f| &f.name == *n))
        {
            let (ty, field) = (from.clone(), n.clone());
            return Err(invalid(at(&owner, Case::UnknownField { ty, field })));
        }
    }
    view_fields(&owner, t, v)?;
    view_params(&owner, v, &read)
}

/// The declared type a view reads: refused at `on`, naming the type, when
/// it is not declared.
fn declared<'s>(st: &'s State, ty: &TypeName, on: &[Place]) -> Result<&'s TypeDecl> {
    let missing = || invalid(at(on, Case::Undeclared { ty: ty.clone() }));
    st.types.get(ty).map(|t| &t.decl).ok_or_else(missing)
}

/// The names `param('name')` reads in `e`.
fn params_read(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Call {
            func: Func::Param,
            args,
        } => {
            if let Some(Expr::Lit {
                value: Literal::Text(name),
            }) = args.first()
            {
                out.insert(name.clone());
            }
        }
        _ => children(e).for_each(|c| params_read(c, out)),
    }
}

/// Whether `e` calls an aggregate anywhere.
fn aggregates(e: &Expr) -> bool {
    matches!(e, Expr::Call { func, .. } if func.is_aggregate()) || children(e).any(aggregates)
}

/// Whether `t`'s view comes back to `t` through the sources its views
/// read, `t` standing for its declared self: the chain, `t` first and
/// last, if it does.
fn source_cycle(st: &State, t: &TypeDecl, v: &ViewDecl) -> Option<Names> {
    fn walk<'a>(
        st: &'a State,
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
            let Some(view) = st.types.get(next).and_then(|u| u.decl.view.as_deref()) else {
                continue;
            };
            path.push(next);
            if walk(st, t, view, path, seen) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut chain: Vec<&TypeName> = vec![&t.name];
    walk(st, t, v, &mut chain, &mut BTreeSet::new()).then(|| chain.into_iter().collect())
}

/// The field `g` of `source` a view groups by, one a key may be; refused
/// naming it at `on`.
fn grouped_field<'a>(source: &'a TypeDecl, g: &FieldName, on: &[Place]) -> Result<&'a FieldDecl> {
    let f = source.fields.iter().find(|f| &f.name == g).ok_or_else(|| {
        let (ty, field) = (source.name.clone(), g.clone());
        invalid(at(on, Case::UnknownField { ty, field }))
    })?;
    if !lakelands_api::key::keyable(f.ty.kind) {
        let field = g.clone();
        return Err(invalid(at(on, Case::GroupUnkeyable { field })));
    }
    Ok(f)
}

/// A view's source may not be a view with parameters: only a read that
/// names a view binds them.
fn unbound_source(on: &[Place], source: &TypeDecl) -> Result<()> {
    match source.view.as_deref().and_then(|v| v.params.first()) {
        Some(p) => {
            let (source, param) = (source.name.clone(), p.name.clone());
            Err(invalid(at(on, Case::ReadsParamView { source, param })))
        }
        None => Ok(()),
    }
}

/// A view's merged sources: each declared, read once and not as the
/// view's own source, not a view with parameters, grouped by as many
/// fields as the view, each of the kind of the main source's field it
/// lines up with, with measures that aggregate and names unique in the
/// view. The parameters they read are added to `read`.
fn merged_sources(
    st: &State,
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
        let source = declared(st, &m.source, &on)?;
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
                let case = Case::MergedGroupKind { field, view_field };
                return Err(invalid(at(&on, case)));
            }
        }
        if let Some(w) = &m.r#where {
            let on = [&on[..], &[Place::Where]].concat();
            params_read(&condition(w, Eval::View, &on)?, read);
        }
        for x in &m.measures {
            name(Naming::Measure, x.name.as_str())?;
            let name = x.name.clone();
            let on = [&on[..], &[Place::Measure { name }]].concat();
            let e = condition(&x.expr, Eval::Group, &on)?;
            if !aggregates(&e) {
                let source = m.source.clone();
                return Err(invalid(at(&on, Case::MeasuresNothing { source })));
            }
            params_read(&e, read);
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

/// A view's declared fields are exactly what it computes: a field for each
/// `group_by` field and each measure, and no field that nothing fills.
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
        let case = at(owner, Case::ViewFieldUnfilled { field });
        return Err(invalid_field(&f.name, case));
    }
    Ok(())
}

/// A view's parameters: each declared once, of a scalar kind, and read by
/// its `where` or a measure; and every `param('name')` they hold, `read`,
/// names one of them.
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

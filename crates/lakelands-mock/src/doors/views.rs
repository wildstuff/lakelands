//! A view at `declare`, checked as the store checks it, in its order and
//! with its sentences: its sources, each declared, read once, not itself
//! through other views and not a view with a parameter; its conditions;
//! its grouping, field by field; its merged sources; the fields it
//! declares; and its parameters. The mock is a second implementation, so
//! these are copies of the store's (`declare.rs`), held to it by
//! `a_view_is_declared_only_as_the_store_declares_it`.

use std::collections::BTreeSet;

use lakelands_api::error::{Refusal, Result};
use lakelands_api::expr::{Condition, Expr, Func, Literal, children, parse};
use lakelands_api::id::{FieldName, TypeName};
use lakelands_api::ontology::{FieldDecl, StorageKind, TypeDecl, ViewDecl};

use super::{invalid, on, view_reads};
use crate::state::State;

/// Refuse `t`'s view as the store refuses it at `declare`.
pub(super) fn check(st: &State, t: &TypeDecl) -> Result<()> {
    let Some(v) = t.view.as_deref() else {
        return Ok(());
    };
    let owner = format!("type `{}`", t.name);
    if let Some(chain) = source_cycle(st, t, v) {
        return Err(invalid(format!(
            "{owner}: its view closes a cycle, {chain}; a view may not read itself, through \
             other views or directly"
        )));
    }
    let source = declared(st, &v.source).ok_or_else(|| {
        invalid(format!(
            "{owner}: its view reads `{}`, which is not declared",
            v.source
        ))
    })?;
    unbound_source(&owner, source)?;
    let mut read = BTreeSet::new();
    if let Some(w) = &v.r#where {
        params_read(&condition(w, &format!("{owner}, view `where`"))?, &mut read);
    }
    for g in &v.group_by {
        grouped_field(source, g, &format!("{owner}: its view"))?;
    }
    for m in &v.measures {
        let at = format!("{owner}, measure `{}`", m.name);
        params_read(&condition(&m.expr, &at)?, &mut read);
    }
    merged_sources(st, &owner, v, source, &mut read)?;
    // Every field the view reads from a source is one that source has.
    for (from, reads) in view_reads(v) {
        let Some(s) = declared(st, &from) else {
            continue;
        };
        if let Some(n) = reads
            .iter()
            .find(|n| !s.fields.iter().any(|f| &f.name == *n))
        {
            return Err(invalid(format!(
                "{owner}: its view reads `{n}` of `{from}`, which `{from}` does not have"
            )));
        }
    }
    view_fields(&owner, t, v)?;
    view_params(&owner, v, &read)
}

fn declared<'s>(st: &'s State, ty: &TypeName) -> Option<&'s TypeDecl> {
    st.types.get(ty).map(|t| &t.decl)
}

/// A condition parsed, refused at `at` when it does not.
fn condition(c: &Condition, at: &str) -> Result<Expr> {
    match c {
        Condition::Expr(e) => Ok(e.clone()),
        Condition::Text(t) => {
            parse(t).map_err(|e| invalid(format!("{at}: `{t}` does not parse: {e}")))
        }
    }
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
/// read, `t` standing for its declared self: the chain, as "`a` reads
/// `b` and `b` reads `a`", if it does.
fn source_cycle(st: &State, t: &TypeDecl, v: &ViewDecl) -> Option<String> {
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
            let Some(view) = declared(st, next).and_then(|u| u.view.as_deref()) else {
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
    if !walk(st, t, v, &mut chain, &mut BTreeSet::new()) {
        return None;
    }
    let links: Vec<String> = chain
        .windows(2)
        .map(|w| format!("`{}` reads `{}`", w[0], w[1]))
        .collect();
    links.split_last().map(|(last, rest)| match rest {
        [] => last.clone(),
        rest => format!("{} and {last}", rest.join(", ")),
    })
}

/// The field `g` of `source` a view groups by, one a key may be.
fn grouped_field<'a>(source: &'a TypeDecl, g: &FieldName, at: &str) -> Result<&'a FieldDecl> {
    let f = source.fields.iter().find(|f| &f.name == g).ok_or_else(|| {
        invalid(format!(
            "{at} groups by `{g}`, which `{}` does not have",
            source.name
        ))
    })?;
    if !lakelands_api::key::keyable(f.ty.kind) {
        return Err(invalid(format!(
            "{at} groups by `{g}`, whose kind cannot be part of a key"
        )));
    }
    Ok(f)
}

/// A view's source may not be a view with parameters: only a read that
/// names a view binds them.
fn unbound_source(owner: &str, source: &TypeDecl) -> Result<()> {
    match source.view.as_deref().and_then(|v| v.params.first()) {
        Some(p) => Err(invalid(format!(
            "{owner}: its view reads `{}`, a view with the parameter `{}`; only a read that \
             names a view binds its parameters, so no view reads one",
            source.name, p.name
        ))),
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
    owner: &str,
    v: &ViewDecl,
    main: &TypeDecl,
    read: &mut BTreeSet<String>,
) -> Result<()> {
    let mut names: Vec<&FieldName> = v.measures.iter().map(|m| &m.name).collect();
    for (i, m) in v.merge.iter().enumerate() {
        let at = format!("{owner}, merged source `{}`", m.source);
        if m.source == v.source || v.merge[..i].iter().any(|n| n.source == m.source) {
            return Err(invalid(format!(
                "{at}: a view reads each source once, and `{}` is read already",
                m.source
            )));
        }
        let source = declared(st, &m.source)
            .ok_or_else(|| invalid(format!("{at}: `{}` is not declared", m.source)))?;
        unbound_source(owner, source)?;
        if m.group_by.len() != v.group_by.len() {
            return Err(invalid(format!(
                "{at}: it groups by {} field(s), and the view by {}; each lines up with one \
                 of the view's, in order",
                m.group_by.len(),
                v.group_by.len()
            )));
        }
        for (g, to) in m.group_by.iter().zip(&v.group_by) {
            let kind = grouped_field(source, g, &at)?.ty.kind;
            if main
                .fields
                .iter()
                .find(|f| &f.name == to)
                .map(|f| f.ty.kind)
                != Some(kind)
            {
                return Err(invalid(format!(
                    "{at}: its `{g}` lines up with the view's `{to}`, of another kind, so no \
                     group would meet"
                )));
            }
        }
        if let Some(w) = &m.r#where {
            params_read(&condition(w, &format!("{at}, `where`"))?, read);
        }
        for x in &m.measures {
            let e = condition(&x.expr, &format!("{at}, measure `{}`", x.name))?;
            if !aggregates(&e) {
                return Err(invalid(format!(
                    "{at}, measure `{}`: it calls no aggregate, so it measures nothing of `{}`",
                    x.name, m.source
                )));
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
        return Err(invalid(format!(
            "{owner}: its view names the measure `{n}` twice; a view's measures and its merged \
             sources' share one set of names"
        )));
    }
    Ok(())
}

/// A view's declared fields are exactly what it computes: a field for each
/// `group_by` field and each measure, and no field that nothing fills.
fn view_fields(owner: &str, t: &TypeDecl, v: &ViewDecl) -> Result<()> {
    let computed: Vec<(&FieldName, &str)> = v
        .group_by
        .iter()
        .map(|g| (g, "group_by field"))
        .chain(v.measures.iter().map(|m| (&m.name, "measure")))
        .chain(
            v.merge
                .iter()
                .flat_map(|m| m.measures.iter().map(|x| (&x.name, "merged measure"))),
        )
        .collect();
    if let Some((n, what)) = computed
        .iter()
        .find(|(n, _)| !t.fields.iter().any(|f| &f.name == *n))
    {
        let message = format!(
            "{owner}: its view's {what} `{n}` has no declared field to land in; a view \
             declares a field for each value it computes, of the kind it holds"
        );
        return Err(on(n, Refusal::Invalid { message }));
    }
    if let Some(f) = t
        .fields
        .iter()
        .find(|f| !computed.iter().any(|(n, _)| **n == f.name))
    {
        let message = format!(
            "{owner}: its field `{}` is neither a `group_by` field nor a measure of its view, \
             so nothing would fill it",
            f.name
        );
        return Err(on(&f.name, Refusal::Invalid { message }));
    }
    Ok(())
}

/// A view's parameters: each declared once, of a scalar kind, and read by
/// its `where` or a measure; and every `param('name')` they hold, `read`,
/// names one of them.
fn view_params(owner: &str, v: &ViewDecl, read: &BTreeSet<String>) -> Result<()> {
    for (i, p) in v.params.iter().enumerate() {
        if v.params[..i].iter().any(|q| q.name == p.name) {
            return Err(invalid(format!(
                "{owner}: its view declares the parameter `{}` twice",
                p.name
            )));
        }
        if !matches!(
            p.kind,
            StorageKind::Text
                | StorageKind::Number
                | StorageKind::Decimal
                | StorageKind::Boolean
                | StorageKind::DateTime
        ) {
            let kind = serde_json::to_value(p.kind).unwrap_or_default();
            return Err(invalid(format!(
                "{owner}: its view's parameter `{}` is of kind {}; a parameter is text, \
                 number, decimal, boolean or date_time",
                p.name,
                kind.as_str().unwrap_or_default()
            )));
        }
        if !read.contains(p.name.as_str()) {
            return Err(invalid(format!(
                "{owner}: its view declares the parameter `{}`, which neither its `where` nor \
                 its measures read, so a reader would bind a value that changes nothing",
                p.name
            )));
        }
    }
    if let Some(unknown) = read
        .iter()
        .find(|n| !v.params.iter().any(|p| p.name.as_str() == n.as_str()))
    {
        let declared: Vec<String> = v.params.iter().map(|p| format!("`{}`", p.name)).collect();
        return Err(invalid(format!(
            "{owner}: its view reads param('{unknown}'), which it does not declare; {}",
            if declared.is_empty() {
                "it declares no parameters".to_owned()
            } else {
                format!("its parameters are {}", declared.join(", "))
            }
        )));
    }
    Ok(())
}

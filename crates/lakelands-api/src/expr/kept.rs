//! Whether a view can be kept: its records held as its sources are
//! written, and read as stored records are, where they would otherwise be
//! computed at every read.
//!
//! A kept record is the computation's, exactly. A kept group folds its
//! members as they arrive and leave, in whatever order that is, so a view
//! can be kept only where each aggregate's answer is a function of its
//! values, whatever order they are folded in, and where nothing but its
//! sources' records decides them. That is read off the declaration alone,
//! with the kinds its expressions certainly compute ([`super::kind`]).

use super::kind::{self, Kind};
use super::{Expr, Func, Literal, children, print, rewrite};
use crate::error::{Case, Place, Said};
use crate::ontology::{FieldDecl, StorageKind, TypeDecl, ViewDecl};
use crate::{FieldName, TypeName};

/// Whether `view` can be kept, by its declaration alone: it declares no
/// parameter and reads no clock, nor a field computed at the read, and every aggregate call of its measures
/// and of its merged sources' is a function of its values: any `count`, a
/// `sum` or `avg` of what is certainly a number, a `min` or `max` of what
/// is certainly a number or an instant, and that only under group fields
/// that are all text or references, of which there is one at least, so
/// that a group whose extreme left is found again by its fields' values.
/// Otherwise why not, said at the measure where it is one's: which call,
/// and what of it. Whether a source that is itself a view is kept is not
/// said here.
pub fn keepable<'a>(
    view: &ViewDecl,
    declared: impl Fn(&TypeName) -> Option<&'a TypeDecl>,
) -> Result<(), Said> {
    if let Some(p) = view.params.first() {
        let name = p.name.clone();
        return Err(Case::KeptParam { name }.into());
    }
    let own = (&view.source, &view.r#where, &view.group_by, &view.measures);
    let merged = (view.merge.iter()).map(|s| (&s.source, &s.r#where, &s.group_by, &s.measures));
    for (source, filter, group_by, measures) in std::iter::once(own).chain(merged) {
        let fields = declared(source).map_or(&[][..], |t| &t.fields);
        // A field computed at the read moves with the clock as well.
        let moving = |e: &Expr| {
            (e.fields_read().into_iter())
                .find(|n| at_read(fields, n))
                .cloned()
        };
        let source = source.clone();
        if let Some(field) = group_by.iter().find(|n| at_read(fields, n)).cloned() {
            return Err(Case::KeptGroupAtRead { field, source }.into());
        }
        for e in filter.iter().filter_map(|c| c.typed().ok()) {
            if e.reads_clock() {
                return Err(Case::KeptWhereClock { source }.into());
            }
            if let Some(field) = moving(&e) {
                return Err(Case::KeptWhereAtRead { field, source }.into());
            }
        }
        for m in measures {
            let at = || Place::Measure {
                name: m.name.clone(),
            };
            let said = |case: Case| Said::from(case).within(at());
            let e = m.expr.typed().map_err(|e| said(e.into()))?;
            if e.reads_clock() {
                return Err(said(Case::KeptClock));
            }
            if let Some(field) = moving(&e) {
                let source = source.clone();
                return Err(said(Case::KeptAtRead { field, source }));
            }
            let mut made = Vec::new();
            calls(&e, &mut made);
            for call in made {
                let Expr::Call { func, args } = call else {
                    continue;
                };
                folds(*func, print(call), args.first(), fields, group_by).map_err(said)?;
            }
        }
    }
    Ok(())
}

fn at_read(fields: &[FieldDecl], n: &FieldName) -> bool {
    fields.iter().any(|f| &f.name == n && f.at_read())
}

/// The aggregate calls in `e`.
fn calls<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
    match e {
        Expr::Call { func, .. } if func.is_aggregate() => out.push(e),
        e => children(e).for_each(|c| calls(c, out)),
    }
}

/// Whether one aggregate call, printed `call`, is a function of its
/// values, over a source with these fields grouped by `group_by`; or what
/// of it is not.
fn folds(
    func: Func,
    call: String,
    arg: Option<&Expr>,
    fields: &[FieldDecl],
    group_by: &[FieldName],
) -> Result<(), Case> {
    let declared = |f: &FieldName| fields.iter().find(|d| d.name == *f).map(|d| d.ty.kind);
    let certain = match (func, arg) {
        (Func::Count, _) => return Ok(()),
        (_, None) => return Err(Case::KeptNoArgument { call }),
        (_, Some(arg)) => folded_kind(arg, &declared),
    };
    let extreme = matches!(func, Func::Min | Func::Max);
    match (certain, extreme) {
        (Some(Kind::Number), _) | (Some(Kind::Instant), true) => {}
        (Some(kind), true) => return Err(Case::KeptOrdersKind { call, kind }),
        (None, true) => return Err(Case::KeptOrdersUncertain { call }),
        (Some(kind), false) => return Err(Case::KeptAddsKind { call, kind }),
        (None, false) => return Err(Case::KeptAddsUncertain { call }),
    }
    if !extreme {
        return Ok(());
    }
    let other = |f: &&FieldName| {
        !matches!(
            declared(f),
            Some(StorageKind::Text | StorageKind::Reference)
        )
    };
    match group_by.iter().find(other) {
        Some(field) => Err(Case::KeptExtremeGroupedBy {
            call,
            field: field.clone(),
        }),
        None if group_by.is_empty() => Err(Case::KeptExtremeUngrouped { call }),
        None => Ok(()),
    }
}

/// The kind a fold's argument certainly is among the values it folds. A
/// fold skips nulls, and arithmetic over a null is null, so an `if` with
/// `null` for one branch stands for its other branch here, wherever it
/// stands in the argument: `sum(if(open, amount, null) * qty)` adds
/// numbers.
fn folded_kind(e: &Expr, field: &dyn Fn(&FieldName) -> Option<StorageKind>) -> Option<Kind> {
    fn filtered(e: &Expr) -> Option<Expr> {
        let value = Literal::Null;
        let null = Expr::Lit { value };
        let Expr::Call {
            func: Func::If,
            args,
        } = e
        else {
            return None;
        };
        match args.as_slice() {
            [_, x, none] | [_, none, x] if *none == null => Some(rewrite(x, &filtered)),
            _ => None,
        }
    }
    let first = |p: &[FieldName]| field(p.first()?).map(|k| (k, 1));
    kind::of(&rewrite(e, &filtered), &first).ok().flatten()
}

#[cfg(test)]
mod tests;

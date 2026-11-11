//! The erase of one record's, one field's or one quarantined row's values
//! (`docs/decisions/115-a-record-s-or-a-field-s-values-can-be-erased.md`):
//! the host's act, under the writer's lock, one append. It is a removal
//! that names no source: [`Removal`] selects and takes for it, the fold
//! redacts by it, and the restate a removal owes restates it, so there is
//! no second walk and no second retirement path. It never retires.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::error::{Case, Missing};
use lakelands_api::event::EventBody;
use lakelands_api::id::KeyId;
use lakelands_api::quarantine::ClosedWhy;
use lakelands_api::write::{Breach, EraseReport, RecordField, RemovedField, Selection};
use lakelands_api::{Error, FieldName, Position, Result, TypeName};

use crate::forget::{count, counted, named_now};
use crate::lake::{Lake, Writer, owing_now};
use crate::log::Draft;
use crate::quarantine::{by_id, close_of};
use crate::remove::{Fixed, Removal, chain, keys_of, owner, takeable, told};

/// A field's erased versions, and the keys each value was sealed under.
type Taken = (u64, Vec<Vec<KeyId>>);

/// What the act takes, as the walk tallies it.
#[derive(Default)]
struct Tally {
    versions: Vec<u64>,
    fields: BTreeMap<(TypeName, FieldName), Taken>,
    sources: BTreeSet<lakelands_api::SourceId>,
    inside: BTreeSet<KeyId>,
    outside: BTreeSet<KeyId>,
}

/// The erase of `selection` on `w`, or, on a `preflight`, its report with
/// nothing appended; and the positions whose backed fields settle after.
pub(crate) fn erase(
    lake: &Lake,
    w: &mut Writer,
    selection: &Selection,
    (shredded, id, preflight): (bool, Option<&str>, bool),
) -> Result<(EraseReport, BTreeSet<Position>)> {
    let view = lake.engine.view(None)?;
    let missing = |name: &TypeName| Error::NotFound {
        missing: Missing::Type { name: name.clone() },
    };
    let entry = match selection {
        // The lake's own bookkeeping is no host's to erase: a `sys.` type
        // is refused as an unreadable one is.
        Selection::Record { ty, .. } if ty.as_str().starts_with("sys.") => return Err(missing(ty)),
        Selection::Record { ty, .. } => w.ontology.ty(ty).map(|_| None)?,
        Selection::Quarantined(id) => Some(view.quarantined(&by_id(id))?.pop().ok_or_else(
            || Error::NotFound {
                missing: Missing::Quarantined { id: id.clone() },
            },
        )?),
        _ => return Err(Error::invalid(Case::EraseSelection)),
    };
    let at = crate::writes::position_after(w, 0);
    let renames = lake.renames(Some(view.as_ref()))?;
    let mut act = Removal {
        at,
        source: owner(selection),
        selection: selection.clone(),
        before: None,
        keys: Vec::new(),
        apart: Vec::new(),
        fixed: Fixed::of(&w.ontology, &renames, at),
        chain: None,
    };
    act.chain = chain(lake, view.as_ref(), &act)?;
    let t = walk(lake, w, view.as_ref(), &act, &renames)?;
    let apart = apart_of(lake, &act, &t)?;
    let report = report_of(w, view.as_ref(), selection, &t, &apart)?;
    let mut report = EraseReport {
        quarantined: entry
            .as_ref()
            .filter(|q| q.closed.is_none())
            .map(|q| q.id.clone()),
        ..report
    };
    if shredded && report.unshredded > 0 {
        let count = report.unshredded;
        return Err(Error::invalid(Case::EraseNotShredded { count }));
    }
    let digest = serde_json::json!({"erase": selection, "versions": t.versions});
    let through = at.0.saturating_sub(1);
    let token = format!(
        "{through}-{}",
        lakelands_core::canonical::sha256_hex(&digest)
    );
    if id.is_some_and(|id| id != token) {
        return Err(Error::invalid(Case::EraseReportStale));
    }
    let erased = report.fields.iter().any(|f| f.redacted > 0);
    if preflight || (!erased && report.quarantined.is_none()) {
        report.report_id = token;
        if !erased {
            report.quarantined = None;
        }
        return Ok((report, BTreeSet::new()));
    }
    let now = lake.clock(w);
    let mut drafts = vec![Draft::system(
        now,
        EventBody::ValuesErased {
            selection: selection.clone(),
            keys: Vec::new(),
            apart: apart.clone(),
        },
    )];
    // An open entry the act empties is closed `removed` in the same append.
    drafts.extend(
        entry
            .filter(|q| q.closed.is_none())
            .map(|q| Draft::system(now, close_of(&q, ClosedWhy::Removed))),
    );
    drop(view);
    // The append destroys the apart keys once it is durable.
    let appended = lake.append(w, drafts)?;
    let view = lake.engine.view(None)?;
    let mut restate = lake.restate_for_act(view.as_ref(), &act)?;
    drop(view);
    while let crate::lake::scrub::Stepped::More = lake.step_act(w, &mut restate, at)? {}
    let last = appended.last().map_or(at, |e| e.position);
    report.position = last;
    report.log_events = last.0;
    report.report_id = token;
    let mut own: BTreeSet<Position> = appended.iter().map(|e| e.position).collect();
    own.extend(owing_now());
    Ok((report, own))
}

/// What `act` takes of the log before it, as the walk tallies it.
fn walk(
    lake: &Lake,
    w: &Writer,
    view: &dyn lakelands_api::engine::ProjectionView,
    act: &Removal,
    renames: &[lakelands_api::principal::Renamed],
) -> Result<Tally> {
    let at = act.at;
    let earlier = told(lake, view)?;
    let mut t = Tally::default();
    'walk: for page in lake.pages(Position::GENESIS) {
        for e in page? {
            if e.position >= at {
                break 'walk;
            }
            let sealed = match &e.body {
                EventBody::RecordQuarantined { sealed, .. } => sealed.as_ref(),
                _ => e.sealed.as_ref(),
            };
            let apart = sealed
                .into_iter()
                .flat_map(keys_of)
                .filter(|k| k.0)
                .map(|k| k.1);
            if !act.selects(&e) {
                if act.source.is_some() && e.source == act.source {
                    t.outside.extend(apart);
                }
                continue;
            }
            t.inside.extend(apart);
            let under = |f: &FieldName| {
                sealed
                    .filter(|s| s.fields.contains(f))
                    .map_or_else(Vec::new, |s| s.apart.clone())
            };
            let before = earlier.iter().filter(|r| r.selects(&e));
            let (ty, written): (&TypeName, Vec<&FieldName>) = match &e.body {
                // Every cell of a quarantined version, sealed or not (085).
                EventBody::RecordQuarantined { ty, cells, .. } if before.count() == 0 => {
                    let boxed = sealed.map_or(&[][..], |s| &s.fields);
                    let mut all: Vec<&FieldName> = cells.keys().chain(boxed).collect();
                    all.sort();
                    all.dedup();
                    (ty, all)
                }
                EventBody::RecordIngested { ty, patch, .. }
                | EventBody::RecordUpserted { ty, patch, .. } => {
                    let boxed = sealed
                        .filter(|s| s.boxed.is_some())
                        .map_or(&[][..], |s| &s.fields);
                    let plain = patch.iter().filter(|(_, v)| !v.is_null()).map(|(f, _)| f);
                    let decl = w.ontology.ty(ty).ok();
                    let earlier: Vec<&Removal> = earlier.iter().filter(|r| r.selects(&e)).collect();
                    let taken = |f: &&FieldName| {
                        decl.is_some_and(|d| takeable(d, f))
                            && act.takes(e.position, ty, f)
                            && !earlier.iter().any(|r| r.takes(e.position, ty, f))
                    };
                    (ty, plain.chain(boxed).filter(taken).collect())
                }
                _ => continue,
            };
            t.versions.push(e.position.0);
            t.sources.extend(e.source.clone());
            for f in written {
                let (ty, f) = named_now(renames, e.position, ty, f);
                let tally = t.fields.entry((ty, f.clone())).or_default();
                tally.0 += 1;
                tally.1.push(under(&f));
            }
        }
    }
    Ok(t)
}

/// The apart keys of `act`'s source the tally `t` lets it destroy.
fn apart_of(lake: &Lake, act: &Removal, t: &Tally) -> Result<Vec<KeyId>> {
    // A delivery's own apart key goes where it seals nothing outside the
    // selection; no other key is destroyed (115).
    let mut apart = Vec::new();
    if let Some(source) = &act.source {
        for k in t.inside.difference(&t.outside) {
            if lake.keys.holds(source, k)? {
                apart.push(k.clone());
            }
        }
    }
    Ok(apart)
}

/// The apart keys the erase of the closed entry `id` destroys, as the
/// act's own erase of it would (115).
pub(crate) fn apart_on_close(
    lake: &Lake,
    w: &Writer,
    id: &lakelands_api::quarantine::QuarantineId,
    at: Position,
) -> Result<Vec<KeyId>> {
    let view = lake.engine.view(None)?;
    let renames = lake.renames(Some(view.as_ref()))?;
    let selection = Selection::Quarantined(id.clone());
    let act = Removal {
        at,
        source: owner(&selection),
        selection,
        before: None,
        keys: Vec::new(),
        apart: Vec::new(),
        fixed: Fixed::of(&w.ontology, &renames, at),
        chain: None,
    };
    let t = walk(lake, w, view.as_ref(), &act, &renames)?;
    apart_of(lake, &act, &t)
}

/// The report of the tally `t`: the record it touches, kept alive, the
/// fields and what is computed from them.
fn report_of(
    w: &Writer,
    view: &dyn lakelands_api::engine::ProjectionView,
    selection: &Selection,
    t: &Tally,
    apart: &[KeyId],
) -> Result<EraseReport> {
    let mut unshredded = 0;
    let fields: Vec<RemovedField> = (t.fields.iter())
        .map(|((ty, field), (redacted, under))| {
            // A value sealed under a destroyed apart key is shredded: its
            // derived key can no longer be made.
            let shredded = (under.iter())
                .filter(|ks| ks.iter().any(|k| apart.contains(k)))
                .count() as u64;
            unshredded += redacted - shredded;
            let (ty, field, redacted) = (ty.clone(), field.clone(), *redacted);
            RemovedField {
                ty,
                field,
                redacted,
                shredded,
            }
        })
        .collect();
    let (mut types, mut kept, mut breaches) = (Vec::new(), Vec::new(), Vec::new());
    if let Selection::Record { ty, key, .. } = selection
        && let Some(r) = view.get(ty, key)?
    {
        // The record stays live, whatever the act leaves of it.
        let mut held = counted(ty);
        count(&mut held, &r);
        if !fields.is_empty() {
            types.push(held.clone());
        }
        kept.push(held);
        let decl = w.ontology.ty(ty)?;
        let emptied = |f: &&lakelands_api::ontology::FieldDecl| {
            f.required && r.fields.contains_key(&f.name) && fields.iter().any(|t| t.field == f.name)
        };
        breaches.extend(decl.fields.iter().filter(emptied).map(|f| Breach {
            ty: ty.clone(),
            constraint: "required".into(),
            records: 1,
            sample_keys: vec![key.clone()],
            field: Some(f.name.clone()),
        }));
    }
    let (mut recomputed, mut views) = (Vec::new(), Vec::new());
    for (name, entry) in &w.ontology.types {
        let decl = &entry.decl;
        if (decl.view.as_ref()).is_some_and(|v| types.iter().any(|t| t.ty == v.source)) {
            views.push(name.clone());
        }
        let taken = |n: &FieldName| fields.iter().any(|f| &f.ty == name && &f.field == n);
        for f in &decl.fields {
            let computed = f.derived.is_some() || f.function.is_some() || f.embed.is_some();
            if computed && f.depends_on.iter().any(taken) {
                let (ty, field) = (name.clone(), f.name.clone());
                recomputed.push(RecordField { ty, field });
            }
        }
    }
    let log_events = view.position().map_or(0, |p| p.0);
    Ok(EraseReport {
        selection: selection.clone(),
        types,
        fields,
        versions: t.versions.len() as u64,
        keys: Vec::new(),
        apart: apart.to_vec(),
        kept,
        // The act never takes a reference nor retires a record, so it
        // strands no reference.
        references: Vec::new(),
        unshredded,
        recomputed,
        views,
        breaches,
        log_events,
        position: view.position().unwrap_or(Position::GENESIS),
        report_id: String::new(),
        quarantined: None,
        sources: t.sources.iter().cloned().collect(),
    })
}

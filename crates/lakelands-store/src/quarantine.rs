//! Quarantined records: a source's records its ingest was refused, kept
//! as events of their own with their cells and refusal, until a
//! resubmission, a dismiss, a write at their key or an act on their
//! source closes them
//! (`docs/decisions/085-a-refused-record-may-be-quarantined-until-it-is-run-again-dismissed-or-its-key-lands.md`).
//!
//! The fold keeps each id's last version as a row, its cells under the
//! names of their fields at the head, and a closed one with no cell. The
//! listing reads a row under the sight of its type
//! ([`Sight::quarantined_cells`]); a caller's write door closes the open
//! rows at the type and key it writes in its own append
//! ([`Lake::closing`]).

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::ProjectionView;
use lakelands_api::error::Named;
use lakelands_api::event::{Event, EventBody};
use lakelands_api::quarantine::{
    ClosedWhy, QuarantineClose, QuarantineFilter, QuarantineId, QuarantineState, Quarantined,
    TypeQuarantined,
};
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::write::{Migration, ProposedWrite, WriteOpts};
use lakelands_api::{
    ConstraintName, Error, FieldName, Missing, RecordKey, Result, SourceId, TypeName,
};
use lakelands_core::sight::Sight;

use crate::lake::{Lake, Staged, Writer};
use crate::log::Draft;
use crate::ontology::Ontology;
use crate::reads::Reader;
use crate::writes::{Call, check_context, granted};

/// The rows `filter` asks for, as this append leaves them: the
/// projection's, each as `staged` holds it where the append wrote one.
pub(crate) fn rows(
    view: &dyn ProjectionView,
    staged: &BTreeMap<QuarantineId, Quarantined>,
    filter: &QuarantineFilter,
) -> Result<Vec<Quarantined>> {
    if staged.is_empty() {
        return view.quarantined(filter);
    }
    let unpaged = QuarantineFilter {
        limit: None,
        ..filter.clone()
    };
    let mut all: BTreeMap<_, _> = (view.quarantined(&unpaged)?.into_iter())
        .map(|q| (q.id.clone(), q))
        .collect();
    all.extend(staged.iter().map(|(id, q)| (id.clone(), q.clone())));
    Ok(filter.answer(all.into_values()))
}

/// The row of `id`, as this append leaves it.
pub(crate) fn row(
    view: &dyn ProjectionView,
    staged: &BTreeMap<QuarantineId, Quarantined>,
    id: &QuarantineId,
) -> Result<Option<Quarantined>> {
    if let Some(q) = staged.get(id) {
        return Ok(Some(q.clone()));
    }
    Ok(view.quarantined(&by_id(id))?.pop())
}

/// The filter that gets `id`.
pub(crate) fn by_id(id: &QuarantineId) -> QuarantineFilter {
    QuarantineFilter {
        source: Some(id.source.clone()),
        delivery: Some(id.delivery.clone()),
        index: Some(id.index),
        ..QuarantineFilter::default()
    }
}

/// The row a `record_quarantined` leaves, read [`crate::sealing::lifted`]
/// and opened, what is erased already gone: its cells as sent, and those
/// whose field was `sensitive` in `ontology`, the lake's as it came.
pub(crate) fn version_row(ontology: &Ontology, e: &Event) -> Option<Quarantined> {
    let EventBody::RecordQuarantined {
        ty,
        key,
        id,
        version,
        cells,
        seen_at,
        upstream,
        refusal,
        ..
    } = &e.body
    else {
        return None;
    };
    let decl = ontology.ty(ty).ok();
    let sensitive = (cells.keys())
        .filter(|f| decl.and_then(|d| d.field(f)).is_some_and(|f| f.sensitive))
        .cloned()
        .collect();
    Some(Quarantined {
        id: id.clone(),
        ty: ty.clone(),
        key: key.clone(),
        version: *version,
        position: e.position,
        quarantined_at: e.known_at,
        cells: cells.clone(),
        masked: Vec::new(),
        sensitive,
        seen_at: *seen_at,
        upstream: upstream.clone(),
        refusal: refusal.clone(),
        more: false,
        closed: None,
    })
}

/// `q` closed by `close`: it keeps no cell.
pub(crate) fn closed(mut q: Quarantined, close: QuarantineClose) -> Quarantined {
    q.closed = Some(close);
    (q.cells, q.masked, q.sensitive) = Default::default();
    q
}

/// The rows a migration reaches, as it leaves them: the cells of its type
/// under their fields' new names, and the rows of each link type a renamed
/// relation moves, `types`, under the new type.
pub(crate) fn renamed(
    view: &dyn ProjectionView,
    staged: &Staged,
    migration: &Migration,
    types: &BTreeMap<TypeName, TypeName>,
) -> Result<Vec<Quarantined>> {
    let mut out = Vec::new();
    let of = |ty: &TypeName| QuarantineFilter {
        ty: Some(ty.clone()),
        ..QuarantineFilter::default()
    };
    if !migration.renames.is_empty() {
        let new = |f: &FieldName| {
            let to = migration.renames.iter().find(|(old, _)| old == f);
            to.map_or_else(|| f.clone(), |(_, new)| new.clone())
        };
        for mut q in rows(view, &staged.quarantined, &of(&migration.to.name))? {
            q.cells = std::mem::take(&mut q.cells)
                .into_iter()
                .map(|(f, v)| (new(&f), v))
                .collect();
            q.sensitive = q.sensitive.iter().map(new).collect();
            out.push(q);
        }
    }
    for (from, to) in types {
        for mut q in rows(view, &staged.quarantined, &of(from))? {
            q.ty = to.clone();
            out.push(q);
        }
    }
    Ok(out)
}

/// Whether `q` is there to its reader: its type shown, the record at its
/// key not held from it, and, to a holder of `only`, the record its cells
/// would leave admitted, as a proposal's write is there (085).
pub(crate) fn there(sight: &Sight<'_>, live: Option<&StoredRecord>, q: &Quarantined) -> bool {
    if sight.ty(&q.ty).is_err() || live.is_some_and(|r| sight.withheld(r)) {
        return false;
    }
    let mut left = (live.filter(|r| !r.retired)).map_or_else(BTreeMap::new, |r| r.fields.clone());
    left.extend(q.cells.clone());
    sight.admit(&q.ty).holds(&left)
}

/// `q` as its reader is shown it: its cells by their fields' sight, those
/// masked named, and its refusal whole only where every name it holds is
/// shown, its kind alone otherwise, with `more` (085).
pub(crate) fn shown(
    sight: &Sight<'_>,
    live: Option<&StoredRecord>,
    mut q: Quarantined,
) -> Quarantined {
    let Ok(decl) = sight.ty(&q.ty) else {
        return q;
    };
    let fields = live.filter(|r| !r.retired).map(|r| &r.fields);
    (q.cells, q.masked) = sight.quarantined_cells(decl, &q.cells, &q.sensitive, fields);
    q.sensitive.clear();
    if sight.holds_all() {
        return q;
    }
    let mut named = Named::default();
    q.refusal.names(&mut named);
    // A constraint is told to one who reads every field it reads, as a
    // proposal's refusal tells it.
    let read = |f: &FieldName| sight.reads_from(&q.ty, f);
    let told = |n: &ConstraintName| {
        let c = decl.constraints.iter().find(|c| &c.name == n);
        let e = c.and_then(|c| c.holds.typed().ok());
        e.is_some_and(|e| e.fields_read().into_iter().all(read))
    };
    if named.constraints.iter().all(told) {
        named.constraints.clear();
    }
    let listed = ProposedWrite::Upsert {
        ty: q.ty.clone(),
        key: q.key.clone(),
        patch: q.cells.clone(),
        opts: WriteOpts::default(),
    };
    let sees = |t: &TypeName| sight.ty(t).is_ok();
    if !named.shown_by(&listed, &sees, &read, &|_| None) {
        q.refusal = q.refusal.kind_alone();
        q.more = true;
    }
    q
}

/// The `quarantine_closed` that closes `q` for `why`.
pub(crate) fn close_of(q: &Quarantined, why: ClosedWhy) -> EventBody {
    EventBody::QuarantineClosed {
        id: q.id.clone(),
        ty: q.ty.clone(),
        key: q.key.clone(),
        why,
    }
}

/// The closes of an act on `source`, or on its type `ty`: one for each of
/// its open rows that `reaches`, as `why`, with their count per type.
pub(crate) fn act_closes(
    view: &dyn ProjectionView,
    (source, ty): (&SourceId, Option<&TypeName>),
    why: ClosedWhy,
    reaches: &mut dyn FnMut(&Quarantined) -> Result<bool>,
) -> Result<(Vec<EventBody>, Vec<TypeQuarantined>)> {
    let open = QuarantineFilter {
        source: Some(source.clone()),
        ty: ty.cloned(),
        state: Some(QuarantineState::Open),
        ..QuarantineFilter::default()
    };
    let (mut closes, mut counts) = (Vec::new(), BTreeMap::<TypeName, u64>::new());
    for q in view.quarantined(&open)? {
        if reaches(&q)? {
            *counts.entry(q.ty.clone()).or_default() += 1;
            closes.push(close_of(&q, why));
        }
    }
    let counts = (counts.into_iter())
        .map(|(ty, quarantined)| TypeQuarantined { ty, quarantined })
        .collect();
    Ok((closes, counts))
}

/// The type and key a caller's write door lands `d` at, where it closes
/// the open rows there as landed: none for the store's own record events.
pub(crate) fn landing(d: &Draft) -> Option<(TypeName, RecordKey)> {
    match &d.body {
        EventBody::RecordUpserted { ty, key, .. } | EventBody::RecordIngested { ty, key, .. }
            if matches!(d.origin, Origin::Authored | Origin::Source)
                && d.writer != crate::record::STORE_WRITER =>
        {
            Some((ty.clone(), key.clone()))
        }
        _ => None,
    }
}

impl Lake {
    /// `drafts` with, after them, a `quarantine_closed` for each open
    /// quarantined record at the type and key of a `record_upserted` or
    /// `record_ingested` a caller's write door appends among them, after
    /// its last version (085). The store's own record events close
    /// nothing; nor is a record closed twice, by one append or by the
    /// appends of one batch.
    pub(crate) fn closing(&self, w: &mut Writer, mut drafts: Vec<Draft>) -> Result<Vec<Draft>> {
        if w.staged.is_none() {
            w.closing.clear();
        }
        let Some(now) = drafts
            .iter()
            .find(|d| landing(d).is_some())
            .map(|d| d.known_at)
        else {
            return Ok(drafts);
        };
        let view = self.engine.view(None)?;
        let any = QuarantineFilter {
            state: Some(QuarantineState::Open),
            limit: Some(1),
            ..QuarantineFilter::default()
        };
        let none_stored = view.quarantined(&any)?.is_empty();
        // What the append closes itself, a resubmission's close among it,
        // and what it quarantines, in its order.
        let mut shut: BTreeSet<QuarantineId> = w.closing.clone();
        shut.extend(drafts.iter().filter_map(|d| match &d.body {
            EventBody::QuarantineClosed { id, .. } => Some(id.clone()),
            _ => None,
        }));
        let mut opened: BTreeMap<(TypeName, RecordKey), BTreeSet<QuarantineId>> = BTreeMap::new();
        let mut asked: BTreeSet<(TypeName, RecordKey)> = BTreeSet::new();
        let mut closes = Vec::new();
        for d in &drafts {
            if let EventBody::RecordQuarantined { ty, key, id, .. } = &d.body {
                let at = (ty.clone(), key.clone());
                opened.entry(at).or_default().insert(id.clone());
            }
            let Some(at) = landing(d) else {
                continue;
            };
            let mut ids = opened.remove(&at).unwrap_or_default();
            if !none_stored && asked.insert(at.clone()) {
                let of = QuarantineFilter {
                    ty: Some(at.0.clone()),
                    key: Some(at.1.clone()),
                    state: Some(QuarantineState::Open),
                    ..QuarantineFilter::default()
                };
                ids.extend(view.quarantined(&of)?.into_iter().map(|q| q.id));
            }
            for id in ids {
                if !shut.insert(id.clone()) {
                    continue;
                }
                let (ty, key) = at.clone();
                let why = ClosedWhy::Landed;
                closes.push(Draft::system(
                    now,
                    EventBody::QuarantineClosed { id, ty, key, why },
                ));
            }
        }
        w.closing = shut;
        drafts.extend(closes);
        Ok(drafts)
    }
}

impl Reader<'_> {
    /// `quarantined`: the quarantined records there to a holder of
    /// `ingest`, each as it is shown them, in id order (085).
    pub fn quarantined(&self, filter: &QuarantineFilter) -> Result<Vec<Quarantined>> {
        granted(self.sight.who().grants.ingest)?;
        let limit = self.limit(filter.limit, false)?;
        let one = filter.source.is_some() && filter.delivery.is_some() && filter.index.is_some();
        let (mut out, mut after) = (Vec::new(), filter.after.clone());
        let mut page = u32::try_from(limit).unwrap_or(u32::MAX);
        // The engine is asked a page at a time, each twice the last, while
        // the listing is not full.
        loop {
            let asking = QuarantineFilter {
                limit: Some(page),
                after: after.clone(),
                key: None,
                ..filter.clone()
            };
            let rows = self.view.quarantined(&asking)?;
            let full = rows.len() as u32 == page;
            for q in rows {
                after = Some(q.id.clone());
                let live = self.view.get(&q.ty, &q.key)?;
                if there(self.sight, live.as_ref(), &q) {
                    out.push(shown(self.sight, live.as_ref(), q));
                }
                if out.len() >= limit {
                    return Ok(out);
                }
            }
            if !full {
                break;
            }
            page = page.saturating_mul(2);
        }
        if one && out.is_empty() {
            let id = QuarantineId {
                source: filter.source.clone().expect("asked"),
                delivery: filter.delivery.clone().expect("asked"),
                index: filter.index.expect("asked"),
            };
            return Err(Error::NotFound {
                missing: Missing::Quarantined { id },
            });
        }
        Ok(out)
    }
}

impl Call<'_> {
    /// `dismiss_quarantined`: the close of `id`, appended where it is
    /// open, and its first close where it was closed already (085).
    pub fn dismiss_quarantined(&self, id: &QuarantineId) -> Result<QuarantineClose> {
        check_context(self.lake, &self.context, self.who)?;
        granted(self.who.grants.ingest)?;
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        let not_found = || Error::NotFound {
            missing: Missing::Quarantined { id: id.clone() },
        };
        let q = view.quarantined(&by_id(id))?.pop().ok_or_else(not_found)?;
        let live = view.get(&q.ty, &q.key)?;
        if !there(&Sight::held(self.who, &w.ontology), live.as_ref(), &q) {
            return Err(not_found());
        }
        if let Some(close) = q.closed {
            return Ok(close);
        }
        drop(view);
        let now = self.lake.clock(&mut w);
        let draft = Draft {
            source: Some(q.id.source.clone()),
            ..self.draft(now, Origin::Authored, close_of(&q, ClosedWhy::Dismissed))
        };
        let events = self.lake.append(&mut w, vec![draft])?;
        let position = events.first().map(|e| e.position).ok_or_else(not_found)?;
        Ok(QuarantineClose {
            why: ClosedWhy::Dismissed,
            position,
        })
    }
}

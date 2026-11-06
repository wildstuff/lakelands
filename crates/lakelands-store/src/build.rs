//! A setting's index is built in parts beside the writer
//! (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`).
//!
//! A `sort_index.<type>` or `lookup_index.<type>` setting no longer walks
//! its type in the turn that appends it. The fold marks what the setting
//! asks for as building, and the door that wrote it sends this job and
//! answers when it is whole. Each turn fills a key range of `order_chunk`
//! records, and a read answers as before until the last field's `Keep`.
//! Nothing of the build is appended: it is the projection's, so a feed, a
//! mirror and an export carry the setting alone.

use std::sync::Arc;

use lakelands_api::engine::{Choose, Declared, Look, ProjectionView};
use lakelands_api::seam::{
    LOOKUP_INDEX_SETTING, SERIES_INDEX_SETTING, SORT_INDEX_SETTING, VIEW_KEPT_SETTING,
};
use lakelands_api::write::Declaration;
use lakelands_api::{FieldName, Position, RecordKey, Result, TypeName};
use lakelands_core::engines;

use crate::exec::{self, Turn};
use crate::lake::{Lake, Staged, Writer};
use crate::road::{self, Job, Reach, Turned};
use crate::writes::Write;

/// Which index a setting asks to have built.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Kind {
    /// `sort_index.<type>`: the order index.
    Sorted,
    /// `lookup_index.<type>`: the exact-text index.
    Looked,
    /// `view_kept.<view>`: the view's kept records.
    Kept,
    /// `series_index.<type>`: the series pairs' rows
    /// (`docs/decisions/097-a-read-in-a-window-walks-its-window.md` §How a
    /// lake with records gets it).
    Series,
}

/// The type and index a record write asks to have built, when it puts a
/// setting that asks for one. A setting retired is taken back: the same
/// road runs, with no field left to build and every field it named to
/// drop.
pub(crate) fn asked(write: &Write) -> Option<(TypeName, Kind)> {
    let (ty, key) = match write {
        Write::Upsert(ty, key, _) | Write::Retire(ty, key) => (ty, key),
        _ => return None,
    };
    if ty.as_str() != lakelands_api::seam::SETTING_TYPE {
        return None;
    }
    if let Some(ty) = key.as_str().strip_prefix(SORT_INDEX_SETTING) {
        return Some((TypeName::from(ty), Kind::Sorted));
    }
    if let Some(ty) = key.as_str().strip_prefix(LOOKUP_INDEX_SETTING) {
        return Some((TypeName::from(ty), Kind::Looked));
    }
    if let Some(ty) = key.as_str().strip_prefix(SERIES_INDEX_SETTING) {
        return Some((TypeName::from(ty), Kind::Series));
    }
    // Set, the setting builds the view; written false or retired, it takes
    // the view back and its records are cleared in parts. Both are the
    // road's, and the door waits for whichever it asked for.
    let view = key.as_str().strip_prefix(VIEW_KEPT_SETTING)?;
    Some((TypeName::from(view), Kind::Kept))
}

/// Send the builds the setting of `ty` asks for, one per name its fields
/// are kept under, and answer when they are whole: the door answers then,
/// as it does for an act. A field the setting took back is cleared in
/// parts after them, its `dropping` mark found at the walk.
pub(crate) async fn finish(lake: &Arc<Lake>, ty: &TypeName, kind: Kind) -> Result<()> {
    if let Kind::Kept = kind {
        // A take-back goes first: a view a setting asked for again between
        // two of its chunks starts its build where the drop ends.
        for view in dropping(lake).await? {
            let (tx, rx) = tokio::sync::oneshot::channel();
            road::send(lake, Box::new(Build::unmaking(view).answering(tx)));
            match rx.await {
                Ok(ended) => ended?,
                Err(_) => return Err(exec::halted(false)),
            }
        }
        // An outer view starts after its inner view's `Keep`, so the views
        // `ty` reads through are built first, and then the views that read
        // `ty`, whose settings may have come before its own.
        for view in under(lake, ty)
            .await?
            .into_iter()
            .chain(over(lake, ty).await?)
        {
            let (tx, rx) = tokio::sync::oneshot::channel();
            road::send(lake, Box::new(Build::new(view, Kind::Kept).answering(tx)));
            match rx.await {
                Ok(ended) => ended?,
                Err(_) => return Err(exec::halted(false)),
            }
        }
        return Ok(());
    }
    if let Kind::Series = kind {
        // A pair is the host's, and a type's own: no kept name holds one.
        let (tx, rx) = tokio::sync::oneshot::channel();
        road::send(
            lake,
            Box::new(Build::new(ty.clone(), Kind::Series).answering(tx)),
        );
        return match rx.await {
            Ok(ended) => ended,
            Err(_) => Err(exec::halted(false)),
        };
    }
    for name in engines::ordered_names(ty) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        road::send(lake, Box::new(Build::new(name, kind).answering(tx)));
        match rx.await {
            Ok(ended) => ended?,
            Err(_) => return Err(exec::halted(false)),
        }
    }
    for name in engines::ordered_names(ty) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        road::send(lake, Box::new(Dropping::new(name).answering(tx)));
        match rx.await {
            Ok(ended) => ended?,
            Err(_) => return Err(exec::halted(false)),
        }
    }
    Ok(())
}

/// What the log and the surfaces leave owed of a setting's build: a mark
/// the projection holds as building. A writer's open sends them and does
/// not wait. The types are read from the declarations in force, not from a
/// shape: a look at what is owed leaves no shape behind for a later read
/// as of an earlier position to reuse.
pub(crate) fn owed(_lake: &Lake, view: &dyn ProjectionView) -> Result<Vec<Box<dyn Job>>> {
    let declared = view.declarations()?;
    let mut owed: Vec<Box<dyn Job>> = Vec::new();
    // The views a build is making and the views a take-back is clearing,
    // each after the views it reads through: the road runs one job at a
    // time, in the order they are sent, and a drop before the build a
    // setting asked for again.
    let mut kept: Vec<(usize, TypeName)> = Vec::new();
    let mut drops: Vec<(usize, TypeName)> = Vec::new();
    for d in &declared {
        let Declaration::Type(t) = &d.declaration else {
            continue;
        };
        if t.view.is_some() {
            let state = crate::kept::state(view, &t.name)?;
            let depth = under_in(&declared, &t.name).len();
            if state.as_ref().is_some_and(|s| s.building.is_some()) {
                kept.push((depth, t.name.clone()));
            }
            if state.is_some_and(|s| s.dropping) {
                drops.push((depth, t.name.clone()));
            }
            continue;
        }
        if !view.series(&t.name)?.building.is_empty() {
            owed.push(Box::new(Build::new(t.name.clone(), Kind::Series)));
        }
        for name in engines::ordered_names(&t.name) {
            let chosen = view.chosen(&name)?;
            if !chosen.setting_building.is_empty() {
                owed.push(Box::new(Build::new(name.clone(), Kind::Sorted)));
            }
            if !chosen.looked_building.is_empty() || !chosen.looked_going.is_empty() {
                owed.push(Box::new(Build::new(name.clone(), Kind::Looked)));
            }
            if !chosen.dropping.is_empty() {
                owed.push(Box::new(Dropping::new(name)));
            }
        }
    }
    kept.sort_by_key(|(depth, _)| *depth);
    drops.sort_by_key(|(depth, _)| *depth);
    owed.extend(
        drops
            .into_iter()
            .map(|(_, v)| Box::new(Build::unmaking(v)) as Box<dyn Job>),
    );
    owed.extend(
        kept.into_iter()
            .map(|(_, v)| Box::new(Build::new(v, Kind::Kept)) as Box<dyn Job>),
    );
    Ok(owed)
}

/// The view `ty` and every view it reads through, in the order their
/// builds are sent: a view after the views it reads.
async fn under(lake: &Arc<Lake>, ty: &TypeName) -> Result<Vec<TypeName>> {
    let (lake, ty) = (lake.clone(), ty.clone());
    exec::run(move || {
        let view = lake.engine.view(None)?;
        Ok(under_in(&view.declarations()?, &ty))
    })
    .await
}

/// [`under`]'s order, from the declarations in force.
fn under_in(declared: &[Declared], ty: &TypeName) -> Vec<TypeName> {
    let mut order = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    push_under(declared, ty, &mut order, &mut seen);
    order
}

fn push_under(
    declared: &[Declared],
    ty: &TypeName,
    order: &mut Vec<TypeName>,
    seen: &mut std::collections::BTreeSet<TypeName>,
) {
    if !seen.insert(ty.clone()) {
        return;
    }
    let view = declared.iter().find_map(|d| match &d.declaration {
        Declaration::Type(t) if &t.name == ty => t.view.as_deref(),
        _ => None,
    });
    let Some(view) = view else {
        return;
    };
    let sources = std::iter::once(&view.source).chain(view.merge.iter().map(|m| &m.source));
    for source in sources {
        push_under(declared, source, order, seen);
    }
    order.push(ty.clone());
}

/// The views that read `ty` through, in the order their builds are sent:
/// each after the views it reads, so the ones nearest `ty` first.
async fn over(lake: &Arc<Lake>, ty: &TypeName) -> Result<Vec<TypeName>> {
    let (lake, ty) = (lake.clone(), ty.clone());
    exec::run(move || {
        let view = lake.engine.view(None)?;
        Ok(over_in(&view.declarations()?, &ty))
    })
    .await
}

/// [`over`]'s order, from the declarations in force.
///
/// A round takes the views whose every source that is a view of this lake
/// is already in an earlier round, so a view is never sent before one it
/// reads, however the declarations are ordered and however many hops away
/// that view is. A view whose sources a round cannot reach is not sent at
/// all: it cannot be kept while the others build.
fn over_in(declared: &[Declared], ty: &TypeName) -> Vec<TypeName> {
    let views: std::collections::BTreeSet<&TypeName> = declared
        .iter()
        .filter_map(|d| match &d.declaration {
            Declaration::Type(t) => t.view.is_some().then_some(&t.name),
            _ => None,
        })
        .collect();
    let mut order = Vec::new();
    let mut seen = std::collections::BTreeSet::from([ty.clone()]);
    loop {
        let next: Vec<TypeName> = (declared.iter())
            .filter_map(|d| match &d.declaration {
                Declaration::Type(t) if !seen.contains(&t.name) => {
                    let sources: Vec<&TypeName> = t
                        .view
                        .as_deref()
                        .into_iter()
                        .flat_map(|v| {
                            std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source))
                        })
                        .collect();
                    let reaches = sources.iter().any(|s| seen.contains(*s));
                    let reads_seen = sources
                        .iter()
                        .all(|s| !views.contains(*s) || seen.contains(*s));
                    (reaches && reads_seen).then(|| t.name.clone())
                }
                _ => None,
            })
            .collect();
        if next.is_empty() {
            return order;
        }
        seen.extend(next.iter().cloned());
        order.extend(next);
    }
}

/// The views whose setting was taken back and whose records a take-back is
/// clearing, the views each reads through first: the road sends a drop
/// before the build a setting asked for again.
async fn dropping(lake: &Arc<Lake>) -> Result<Vec<TypeName>> {
    let lake = lake.clone();
    exec::run(move || {
        let view = lake.engine.view(None)?;
        let declared = view.declarations()?;
        let mut views = Vec::new();
        for d in &declared {
            let Declaration::Type(t) = &d.declaration else {
                continue;
            };
            if t.view.is_some()
                && crate::kept::state(view.as_ref(), &t.name)?.is_some_and(|s| s.dropping)
            {
                views.push((under_in(&declared, &t.name).len(), t.name.clone()));
            }
        }
        views.sort_by_key(|(depth, _)| *depth);
        Ok(views.into_iter().map(|(_, v)| v).collect())
    })
    .await
}

/// One setting's build: the fields marked building, filled a key range at
/// a time, then kept.
pub(crate) struct Build {
    ty: TypeName,
    kind: Kind,
    /// Whether this is a take-back: the view's kept records are ended a
    /// key range a turn, not filled and kept.
    dropping: bool,
    /// The fields to fill, read at the walk.
    fields: Vec<FieldName>,
    /// The key the last chunk ended at.
    after: Option<RecordKey>,
    /// How many times the type's series pairs had been started at the
    /// walk: a turn that reads a greater number fills from the first key
    /// again, since a pair taken back and set again is a new pair.
    started: u64,
    /// Where the door that asked waits, if one did.
    answer: Option<tokio::sync::oneshot::Sender<Result<()>>>,
}

impl Build {
    fn new(ty: TypeName, kind: Kind) -> Build {
        Build {
            ty,
            kind,
            dropping: false,
            fields: Vec::new(),
            after: None,
            started: 0,
            answer: None,
        }
    }

    /// The take-back of `view`: its kept records are cleared in chunks.
    fn unmaking(view: TypeName) -> Build {
        Build {
            dropping: true,
            ..Build::new(view, Kind::Kept)
        }
    }

    fn answering(mut self, tx: tokio::sync::oneshot::Sender<Result<()>>) -> Build {
        self.answer = Some(tx);
        self
    }
}

impl Job for Build {
    fn reach(&self) -> Reach {
        Reach::default()
    }

    fn walk(&mut self, _lake: &Lake, view: &dyn ProjectionView) -> Result<()> {
        // A kept view's mark is read in each turn, which goes on from the
        // cursor it holds: the walk holds nothing.
        if let Kind::Kept = self.kind {
            return Ok(());
        }
        self.fields = match self.kind {
            Kind::Sorted => view.chosen(&self.ty)?.setting_building,
            Kind::Looked => view.chosen(&self.ty)?.looked_building,
            // Only the pairs a build is filling: a pair already kept is
            // never filled again, and a pair taken back between two turns
            // is not filled at all.
            Kind::Series => {
                let state = view.series(&self.ty)?;
                self.started = state.started;
                (state.building.iter())
                    .map(|p| {
                        FieldName::from(lakelands_api::seam::series_field(
                            p.order.as_str(),
                            p.series.as_str(),
                        ))
                    })
                    .collect()
            }
            Kind::Kept => Vec::new(),
        };
        Ok(())
    }

    fn fold(&mut self, _tail: &[lakelands_api::event::Event]) -> Result<()> {
        Ok(())
    }

    fn turn(&mut self, t: &Turn<'_>, w: &mut Writer) -> Result<Turned> {
        let limit = t.limits.order_chunk.max(1);
        match self.kind {
            // A pair's rows are an order index's, kept under a derived
            // name: the same `Fill` and `Keep` turns build them. A pair
            // taken back and set again between two turns is a new pair,
            // and its cursor is none again: the rows the take-back
            // cleared lie before the key in hand.
            Kind::Series => {
                let started = {
                    let view = t.engine.view(None)?;
                    view.series(&self.ty)?.started
                };
                if started != self.started {
                    self.started = started;
                    self.after = None;
                }
                self.turn_sorted(t, limit)
            }
            Kind::Sorted => self.turn_sorted(t, limit),
            Kind::Looked => self.turn_looked(t, limit),
            Kind::Kept => self.turn_kept(t, w, limit),
        }
    }

    fn end(&mut self, ended: Result<()>) {
        if let Some(tx) = self.answer.take() {
            let _ = tx.send(ended);
        }
    }
}

impl Build {
    fn turn_sorted(&mut self, t: &Turn<'_>, limit: u32) -> Result<Turned> {
        let Some(field) = self.fields.last().cloned() else {
            return Ok(Turned::Whole);
        };
        let fill = Choose::Fill {
            field: field.clone(),
            after: self.after.clone(),
            limit,
        };
        let chunk = t.engine.projection().choose_order(&self.ty, &fill);
        let Some(last) = chunk? else {
            let keep = t
                .engine
                .projection()
                .choose_order(&self.ty, &Choose::Keep(field));
            keep?;
            self.fields.pop();
            self.after = None;
            t.undescribe();
            return Ok(match self.fields.is_empty() {
                true => Turned::Whole,
                false => Turned::More,
            });
        };
        self.after = Some(last);
        Ok(Turned::More)
    }

    fn turn_looked(&mut self, t: &Turn<'_>, limit: u32) -> Result<Turned> {
        // Nothing coming: what is going still has to be kept out, and a
        // fill over no fields would page to the head without ending.
        if !self.fields.is_empty() {
            let fill = Look::Fill {
                fields: self.fields.clone(),
                after: self.after.clone(),
                limit,
            };
            if let Some(last) = t.engine.projection().look_index(&self.ty, &fill)? {
                self.after = Some(last);
                return Ok(Turned::More);
            }
        }
        let keep = Look::Keep {
            fields: std::mem::take(&mut self.fields),
        };
        t.engine.projection().look_index(&self.ty, &keep)?;
        t.undescribe();
        Ok(Turned::Whole)
    }

    /// One part of a kept view's build or of its take-back: a key range of
    /// the first source with records its cursor has not reached, folded
    /// into the view's groups and put, then, where every source is
    /// reached, the view kept; or a key range of its kept records, ended.
    /// The mark is read here, not held from the walk: a part that a runner
    /// stopped at is gone on from the cursor the state kept, and a mark a
    /// take-back took away ends the job.
    fn turn_kept(&mut self, t: &Turn<'_>, w: &mut Writer, limit: u32) -> Result<Turned> {
        let head = w.tip.as_ref().map_or(Position::GENESIS, |(p, _)| *p);
        // Kept whole already, taken back between two parts, or waiting for
        // a take-back that is not finished.
        let state = crate::kept::state(t.engine.view(None)?.as_ref(), &self.ty)?;
        let stands = state.is_some_and(|s| match self.dropping {
            true => s.dropping,
            false => !s.whole && s.building.is_some() && !s.dropping,
        });
        if !stands {
            return Ok(Turned::Whole);
        }
        let projection = t.engine.projection();
        projection.begin()?;
        // A view of the transaction's own, read after it began, as the
        // fold reads one.
        let view = match t.engine.view(None) {
            Ok(view) => view,
            Err(e) => {
                projection.rollback();
                return Err(e);
            }
        };
        let at = crate::kept::At {
            view: view.as_ref(),
            projection: Some(projection),
            ontology: &w.ontology,
            settings: &w.settings,
        };
        let mut staged = Staged::default();
        if self.dropping {
            return match crate::kept::drop_chunk(
                &at,
                &mut staged,
                &self.ty,
                limit,
                self.after.clone(),
                head,
            ) {
                Ok(Some(last)) => {
                    projection.commit(head)?;
                    self.after = Some(last);
                    Ok(Turned::More)
                }
                Ok(None) => {
                    projection.commit(head)?;
                    t.undescribe();
                    Ok(Turned::Whole)
                }
                Err(e) => {
                    projection.rollback();
                    Err(e)
                }
            };
        }
        let ready = match crate::kept::ready(&at, &staged, &self.ty) {
            Ok(ready) => ready,
            Err(e) => {
                projection.rollback();
                return Err(e);
            }
        };
        if !ready {
            // An inner view is not whole: nothing of this one is folded
            // until its `Keep`. This job ends here with the mark kept, and
            // a later open sends it again once the inner is whole; it does
            // not turn again, since the road runs one job at a time and the
            // same part would answer the same nothing.
            projection.rollback();
            return Ok(Turned::Whole);
        }
        let part =
            crate::kept::fill(&at, &mut staged, &self.ty, limit, head).and_then(
                |last| match last {
                    Some(_) => Ok(false),
                    None => crate::kept::keep(&at, &mut staged, &self.ty, head).map(|_| true),
                },
            );
        match part {
            Ok(kept) => {
                projection.commit(head)?;
                if kept {
                    t.undescribe();
                }
                Ok(match kept {
                    true => Turned::Whole,
                    false => Turned::More,
                })
            }
            Err(e) => {
                projection.rollback();
                Err(e)
            }
        }
    }
}

/// A setting's field taken back: its `dropping` mark is read at the walk,
/// and its rows are cleared a key range of `order_chunk` records a turn.
/// No read looks at a field the mark holds, so a part-cleared index is
/// never read.
pub(crate) struct Dropping {
    ty: TypeName,
    /// The fields still to clear, read at the walk.
    fields: Vec<FieldName>,
    /// The field in hand, and the key its last chunk ended at.
    field: Option<FieldName>,
    after: Option<RecordKey>,
    /// Where the door that asked waits, if one did.
    answer: Option<tokio::sync::oneshot::Sender<Result<()>>>,
}

impl Dropping {
    fn new(ty: TypeName) -> Dropping {
        Dropping {
            ty,
            fields: Vec::new(),
            field: None,
            after: None,
            answer: None,
        }
    }

    fn answering(mut self, tx: tokio::sync::oneshot::Sender<Result<()>>) -> Dropping {
        self.answer = Some(tx);
        self
    }
}

impl Job for Dropping {
    fn reach(&self) -> Reach {
        Reach::default()
    }

    fn walk(&mut self, _lake: &Lake, view: &dyn ProjectionView) -> Result<()> {
        self.fields = view.chosen(&self.ty)?.dropping;
        Ok(())
    }

    fn fold(&mut self, _tail: &[lakelands_api::event::Event]) -> Result<()> {
        Ok(())
    }

    fn turn(&mut self, t: &Turn<'_>, _w: &mut Writer) -> Result<Turned> {
        let Some(field) = self.field.clone().or_else(|| self.fields.last().cloned()) else {
            return Ok(Turned::Whole);
        };
        self.field = Some(field.clone());
        let drop = Choose::DropChunk {
            field,
            after: self.after.clone(),
            limit: t.limits.order_chunk.max(1),
        };
        let next = t.engine.projection().choose_order(&self.ty, &drop)?;
        let Some(last) = next else {
            self.fields.pop();
            self.field = None;
            self.after = None;
            t.undescribe();
            return Ok(match self.fields.is_empty() {
                true => Turned::Whole,
                false => Turned::More,
            });
        };
        self.after = Some(last);
        Ok(Turned::More)
    }

    fn end(&mut self, ended: Result<()>) {
        if let Some(tx) = self.answer.take() {
            let _ = tx.send(ended);
        }
    }
}

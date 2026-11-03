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
use lakelands_api::record::Value;
use lakelands_api::seam::{LOOKUP_INDEX_SETTING, SORT_INDEX_SETTING, VIEW_KEPT_SETTING};
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
}

/// The type and index a record write asks to have built, when it puts a
/// setting that asks for one. A setting retired is taken back: the same
/// road runs, with no field left to build and every field it named to
/// drop.
pub(crate) fn asked(write: &Write) -> Option<(TypeName, Kind)> {
    let (ty, key, patch) = match write {
        Write::Upsert(ty, key, patch) => (ty, key, Some(patch)),
        Write::Retire(ty, key) => (ty, key, None),
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
    let view = key.as_str().strip_prefix(VIEW_KEPT_SETTING)?;
    let asked = patch.is_some_and(|p| p.get(&FieldName::from("value")) == Some(&Value::Bool(true)));
    asked.then(|| (TypeName::from(view), Kind::Kept))
}

/// Send the builds the setting of `ty` asks for, one per name its fields
/// are kept under, and answer when they are whole: the door answers then,
/// as it does for an act. A field the setting took back is cleared in
/// parts after them, its `dropping` mark found at the walk.
pub(crate) async fn finish(lake: &Arc<Lake>, ty: &TypeName, kind: Kind) -> Result<()> {
    if let Kind::Kept = kind {
        // An outer view starts after its inner view's `Keep`, so the views
        // it reads through are built first and awaited.
        for view in under(lake, ty).await? {
            let (tx, rx) = tokio::sync::oneshot::channel();
            road::send(lake, Box::new(Build::new(view, Kind::Kept).answering(tx)));
            match rx.await {
                Ok(ended) => ended?,
                Err(_) => return Err(exec::halted(false)),
            }
        }
        return Ok(());
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
    // The views a build is making, each after the views it reads through:
    // the road runs one job at a time, in the order they are sent.
    let mut kept: Vec<(usize, TypeName)> = Vec::new();
    for d in &declared {
        let Declaration::Type(t) = &d.declaration else {
            continue;
        };
        if t.view.is_some() {
            let building = crate::kept::state(view, &t.name)?.is_some_and(|s| s.building.is_some());
            if building {
                kept.push((under_in(&declared, &t.name).len(), t.name.clone()));
            }
            continue;
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
    let kept = kept
        .into_iter()
        .map(|(_, v)| Box::new(Build::new(v, Kind::Kept)) as Box<dyn Job>);
    owed.extend(kept);
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

/// One setting's build: the fields marked building, filled a key range at
/// a time, then kept.
pub(crate) struct Build {
    ty: TypeName,
    kind: Kind,
    /// The fields to fill, read at the walk.
    fields: Vec<FieldName>,
    /// The key the last chunk ended at.
    after: Option<RecordKey>,
    /// Where the door that asked waits, if one did.
    answer: Option<tokio::sync::oneshot::Sender<Result<()>>>,
}

impl Build {
    fn new(ty: TypeName, kind: Kind) -> Build {
        Build {
            ty,
            kind,
            fields: Vec::new(),
            after: None,
            answer: None,
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
        let chosen = view.chosen(&self.ty)?;
        self.fields = match self.kind {
            Kind::Sorted => chosen.setting_building,
            Kind::Looked => chosen.looked_building,
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

    /// One part of a kept view's build: a key range of the first source
    /// with records its cursor has not reached, folded into the view's
    /// groups and put, then, where every source is reached, the view kept.
    /// The mark is read here, not held from the walk: a part that a runner
    /// stopped at is gone on from the cursor the state kept.
    fn turn_kept(&mut self, t: &Turn<'_>, w: &mut Writer, limit: u32) -> Result<Turned> {
        let head = w.tip.as_ref().map_or(Position::GENESIS, |(p, _)| *p);
        // Kept whole already, or taken back between two parts.
        let state = crate::kept::state(t.engine.view(None)?.as_ref(), &self.ty)?;
        if !state.is_some_and(|s| !s.whole && s.building.is_some()) {
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
        if !crate::kept::ready(&at, &staged, &self.ty)? {
            // An inner view is not whole yet: nothing of this one is folded
            // until its `Keep`, and the next part looks again.
            projection.rollback();
            return Ok(Turned::More);
        }
        let part = crate::kept::fill(&at, &mut staged, &self.ty, limit, head).and_then(|last| {
            match last {
                Some(_) => Ok(false),
                None => crate::kept::keep(&at, &mut staged, &self.ty, head).map(|_| true),
            }
        });
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

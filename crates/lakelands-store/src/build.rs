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

use lakelands_api::engine::{Choose, Look, ProjectionView};
use lakelands_api::seam::{LOOKUP_INDEX_SETTING, SORT_INDEX_SETTING};
use lakelands_api::{FieldName, RecordKey, Result, TypeName};
use lakelands_core::engines;

use crate::exec::{self, Turn};
use crate::lake::{Lake, Writer};
use crate::road::{self, Job, Reach, Turned};
use crate::writes::Write;

/// Which index a setting asks to have built.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Kind {
    /// `sort_index.<type>`: the order index.
    Sorted,
    /// `lookup_index.<type>`: the exact-text index.
    Looked,
}

/// The type and index a record write asks to have built, when it puts a
/// setting that asks for one. A retirement is taken back, not built.
pub(crate) fn asked(write: &Write) -> Option<(TypeName, Kind)> {
    let Write::Upsert(ty, key, _) = write else {
        return None;
    };
    if ty.as_str() != lakelands_api::seam::SETTING_TYPE {
        return None;
    }
    if let Some(ty) = key.as_str().strip_prefix(SORT_INDEX_SETTING) {
        return Some((TypeName::from(ty), Kind::Sorted));
    }
    key.as_str()
        .strip_prefix(LOOKUP_INDEX_SETTING)
        .map(|ty| (TypeName::from(ty), Kind::Looked))
}

/// Send the builds the setting of `ty` asks for, one per name its fields
/// are kept under, and answer when they are whole: the door answers then,
/// as it does for an act.
pub(crate) async fn finish(lake: &Arc<Lake>, ty: &TypeName, kind: Kind) -> Result<()> {
    for name in engines::ordered_names(ty) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        road::send(lake, Box::new(Build::new(name, kind).answering(tx)));
        match rx.await {
            Ok(ended) => ended?,
            Err(_) => return Err(exec::halted(false)),
        }
    }
    Ok(())
}

/// What the log and the surfaces leave owed of a setting's build: a mark
/// the projection holds as building. A writer's open sends them and does
/// not wait.
pub(crate) fn owed(lake: &Lake, view: &dyn ProjectionView) -> Result<Vec<Box<dyn Job>>> {
    let shape = lake.shape(view)?;
    let types = lakelands_core::sight::InForce::types(shape.as_ref());
    let stored = (types.into_iter())
        .filter(|t| t.view.is_none())
        .map(|t| t.name.clone());
    let mut owed: Vec<Box<dyn Job>> = Vec::new();
    for ty in stored {
        for name in engines::ordered_names(&ty) {
            let chosen = view.chosen(&name)?;
            if !chosen.setting_building.is_empty() {
                owed.push(Box::new(Build::new(name.clone(), Kind::Sorted)));
            }
            if !chosen.looked_building.is_empty() || !chosen.looked_going.is_empty() {
                owed.push(Box::new(Build::new(name, Kind::Looked)));
            }
        }
    }
    Ok(owed)
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
        let chosen = view.chosen(&self.ty)?;
        self.fields = match self.kind {
            Kind::Sorted => chosen.setting_building,
            Kind::Looked => chosen.looked_building,
        };
        Ok(())
    }

    fn fold(&mut self, _tail: &[lakelands_api::event::Event]) -> Result<()> {
        Ok(())
    }

    fn turn(&mut self, t: &Turn<'_>, _w: &mut Writer) -> Result<Turned> {
        let limit = t.limits.order_chunk.max(1);
        match self.kind {
            Kind::Sorted => self.turn_sorted(t, limit),
            Kind::Looked => self.turn_looked(t, limit),
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
}

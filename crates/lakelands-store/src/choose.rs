//! The lake chooses an order index from what is ordered by
//! (`docs/decisions/041-a-sorted-page-reads-its-page.md`).
//!
//! A `find` page ordered first by a stored field that `sort_index.<type>`
//! does not name sorts the type whole. When one field has that many pages
//! within a window, on a type that is large enough and holds few enough
//! chosen fields, the writer's process builds the field's order index
//! beside the log: nothing is appended, so a feed, a mirror and an export
//! carry nothing of it. The build is chunks of records, each a short write
//! between appends, and the fold keeps the field's rows from its first
//! step, so there is no catch-up: a reader takes the field only once the
//! last step has kept it. A host's `sort_index.<type>` setting wins, and a
//! field no page has been ordered by for the window of unuse is dropped.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lakelands_api::engine::{Choose, Chosen, Scan};
use lakelands_api::ontology::ComputedAt;
use lakelands_api::seam::sortable;
use lakelands_api::{FieldName, RecordKey, Result, TypeName};
use lakelands_core::{engines, lock};

use crate::exec;
use crate::lake::{Lake, read_only};

/// What a lake weighs to choose an order index and what it chose.
#[derive(Default)]
pub(crate) struct Choosing {
    /// When each field was last ordered by a whole sort, within the window.
    pages: BTreeMap<(TypeName, FieldName), VecDeque<Instant>>,
    /// The fields whose pages reached the bound, to weigh.
    pending: BTreeSet<(TypeName, FieldName)>,
    /// When each field the lake chose was last ordered by.
    used: BTreeMap<(TypeName, FieldName), Instant>,
    /// A task is choosing or dropping.
    running: bool,
    /// When the fields unused were last looked for.
    swept: Option<Instant>,
}

/// How often the chosen fields are looked at for unuse.
const SWEEP: Duration = Duration::from_secs(60);

impl Lake {
    /// A `find` page was ordered first by `field`, whole: count it, and
    /// weigh the field once enough pages came within the window. Only the
    /// writer's process counts: a reader-only process has no writer to act.
    pub(crate) fn note_whole(&self, ty: &TypeName, field: &FieldName) {
        let l = self.limits;
        if !l.choose_order_index || !self.is_writer() {
            return;
        }
        let now = Instant::now();
        let window = Duration::from_secs(u64::from(l.order_window_secs));
        let mut c = lock(&self.choosing);
        let key = (ty.clone(), field.clone());
        let pages = c.pages.entry(key.clone()).or_default();
        pages.push_back(now);
        while pages
            .front()
            .is_some_and(|t| now.duration_since(*t) > window)
        {
            pages.pop_front();
        }
        if pages.len() >= l.order_after_pages.max(1) as usize {
            c.pending.insert(key);
        }
    }

    /// A `find` page was ordered first by `field`, which the lake chose:
    /// it is used.
    pub(crate) fn note_used(&self, ty: &TypeName, field: &FieldName) {
        let mut c = lock(&self.choosing);
        if let Some(at) = c.used.get_mut(&(ty.clone(), field.clone())) {
            *at = Instant::now();
        }
    }

    /// Whether there is a field to weigh or a sweep to make, and nothing
    /// is choosing already: the caller then runs [`choose_orders`].
    pub(crate) fn choosing_due(&self) -> bool {
        let mut c = lock(&self.choosing);
        // Once at the writer's first read, to find what a writer before it
        // chose, and again while it holds a field the lake chose.
        let every = Duration::from_secs(u64::from(self.limits.order_unused_secs)).min(SWEEP);
        let again = c.swept.is_some_and(|at| at.elapsed() > every) && !c.used.is_empty();
        let due = !c.pending.is_empty() || c.swept.is_none() || again;
        let due = due && self.is_writer() && !c.running;
        c.running |= due;
        due
    }

    /// Whether the lake may choose the order index of `field` of `ty`: a
    /// stored, sortable field of a type that is large enough, holds fewer
    /// chosen fields than the bound, and whose order the host sets none of.
    fn eligible(&self, ty: &TypeName, field: &FieldName) -> Result<bool> {
        let l = self.limits;
        let view = self.engine.view(None)?;
        let shape = self.shape(view.as_ref())?;
        let Some(decl) = shape.ontology.ty(ty).ok().filter(|d| d.view.is_none()) else {
            return Ok(false);
        };
        let stored = |f: &&lakelands_api::ontology::FieldDecl| {
            f.name == *field && sortable(&f.ty.kind) && f.computed_at != Some(ComputedAt::Read)
        };
        let setting = shape.settings.get(&engines::sort_index_key(ty));
        if !engines::sorted_fields(ty, decl, setting).is_empty()
            || decl.fields.iter().find(stored).is_none()
        {
            return Ok(false);
        }
        let chosen = view.chosen(ty)?;
        let held = chosen.kept.len() + chosen.building.len();
        if chosen.kept.contains(field)
            || chosen.building.contains(field)
            || held >= l.order_max_fields as usize
        {
            return Ok(false);
        }
        // At least the bound, read as a page of that many: a count would
        // read the whole type, as long as the sort the choice saves.
        let min = l.order_min_records;
        let page = Scan {
            limit: min,
            ..Scan::default()
        };
        Ok(min == 0 || view.scan(ty, &page)?.len() >= min as usize)
    }

    /// What the engine holds chosen, of every stored type that holds any.
    fn chosen_all(&self) -> Result<BTreeMap<TypeName, Chosen>> {
        let view = self.engine.view(None)?;
        let shape = self.shape(view.as_ref())?;
        let types = lakelands_core::sight::InForce::types(shape.as_ref());
        let stored = (types.into_iter())
            .filter(|t| t.view.is_none())
            .map(|t| t.name.clone());
        let mut chosen: BTreeMap<TypeName, Chosen> = BTreeMap::new();
        for ty in stored {
            let held = view.chosen(&ty)?;
            if held != Chosen::default() {
                chosen.insert(ty, held);
            }
        }
        Ok(chosen)
    }
}

/// Weigh the fields whose pages reached the bound, build the order index
/// of each that is eligible, and drop what has gone unused. Best effort: a
/// lake that cannot do it now leaves its reads sorting.
pub(crate) async fn choose_orders(lake: &Arc<Lake>) {
    let _ = choose_all(lake).await;
    lock(&lake.choosing).running = false;
}

async fn choose_all(lake: &Arc<Lake>) -> Result<()> {
    loop {
        let next = {
            let mut c = lock(&lake.choosing);
            let first = c.pending.iter().next().cloned();
            first.inspect(|key| {
                c.pending.remove(key);
                c.pages.remove(key);
            })
        };
        let Some((ty, field)) = next else { break };
        if let Err(e) = choose_one(lake, &ty, &field).await {
            // A field left half built is dropped, not read.
            let _ = step(lake, &ty, Choose::Drop(field)).await;
            return Err(e);
        }
    }
    sweep(lake).await
}

/// Build the order index of `field` of `ty` if the lake may choose it.
async fn choose_one(lake: &Arc<Lake>, ty: &TypeName, field: &FieldName) -> Result<()> {
    let (l, t, f) = (lake.clone(), ty.clone(), field.clone());
    if !exec::run(move || l.eligible(&t, &f)).await? {
        return Ok(());
    }
    step(lake, ty, Choose::Start(field.clone())).await?;
    let mut after: Option<RecordKey> = None;
    loop {
        let fill = Choose::Fill {
            field: field.clone(),
            after: after.clone(),
            limit: lake.limits.order_chunk.max(1),
        };
        let Some(last) = step(lake, ty, fill).await? else {
            break;
        };
        after = Some(last);
    }
    step(lake, ty, Choose::Keep(field.clone())).await?;
    lock(&lake.choosing)
        .used
        .insert((ty.clone(), field.clone()), Instant::now());
    Ok(())
}

/// One step, a write of its own in the lake's queue: a write sent during
/// the build waits for the chunk in hand, never for the build, since the
/// next chunk queues behind it.
async fn step(lake: &Arc<Lake>, ty: &TypeName, step: Choose) -> Result<Option<RecordKey>> {
    let ty = ty.clone();
    exec::write(lake, move |t| {
        let mut w = t.writer();
        t.regain(&mut w)?;
        if w.lease.is_none() {
            return Err(read_only());
        }
        t.engine.projection().choose_order(&ty, &step)
    })
    .await
}

/// Forget what the engine no longer holds, start every chosen field a
/// writer finds as just used, drop one a task that died left half built,
/// and drop what has gone unused for the window. With the switch off, drop
/// every field the lake chose: only what the host sets is indexed then, a
/// choice an earlier writer made included.
async fn sweep(lake: &Arc<Lake>) -> Result<()> {
    let now = Instant::now();
    let l = lake.clone();
    let chosen = exec::run(move || l.chosen_all()).await?;
    let off = !lake.limits.choose_order_index;
    let mut drops = Vec::new();
    {
        let mut c = lock(&lake.choosing);
        c.swept = Some(now);
        let kept =
            |(t, f): &(TypeName, FieldName)| chosen.get(t).is_some_and(|c| c.kept.contains(f));
        c.used.retain(|key, _| kept(key));
        for (ty, fields) in &chosen {
            for f in &fields.kept {
                c.used.entry((ty.clone(), f.clone())).or_insert(now);
            }
            drops.extend(fields.building.iter().map(|f| (ty.clone(), f.clone())));
        }
        let unused = Duration::from_secs(u64::from(lake.limits.order_unused_secs));
        for (key, at) in &c.used {
            if off || now.duration_since(*at) > unused {
                drops.push(key.clone());
            }
        }
    }
    for (ty, field) in drops {
        step(lake, &ty, Choose::Drop(field.clone())).await?;
        lock(&lake.choosing).used.remove(&(ty, field));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

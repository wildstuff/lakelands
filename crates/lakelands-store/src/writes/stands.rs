//! Whether a reaction stands after an act erased what it read (065,
//! lakelands#1238).
//!
//! A `trigger_fired`, a `trigger_dropped` and an `object_set_changed` are
//! facts of the write that made them, and the log keeps them. One that an
//! act follows is decided again as its write decided it, over the
//! projection as of its cause, which the act's rebuild left without the
//! erased values: a trigger's `when` over the record as of the cause and
//! the one before it, a path across an edge read at the cause, with the
//! settings then and the event's time; a set's change where the membership
//! before the cause is not the one it moved to and the membership after
//! is. For a set across an edge or of a kept view that membership is the
//! kept one, which the fold decides itself; for any other, the set's
//! `where` over the record then, as a read inlines it, and before the
//! cause the membership the write read unless the record held an erased
//! value the `where` reads ([`member`]). One that does not
//! hold so is *overruled*. A trigger with no `when`, or withdrawn at its
//! cause, stands, and so does a reaction whose record is not there as of
//! its cause; so does every reaction no act follows, unread: one after
//! the last act that does not share its time, which every event of its
//! append shares.

use super::*;
use crate::lake::Shape;

/// The views and shapes of one decision, each read once a position, and
/// whether the event at a cause ends a range of one shape.
struct AsOf<'l> {
    lake: &'l Lake,
    views: BTreeMap<Position, Box<dyn ProjectionView + 'l>>,
    shapes: BTreeMap<Position, Arc<Shape>>,
    ends: BTreeMap<Position, bool>,
}

impl<'l> AsOf<'l> {
    fn view(&mut self, at: Position) -> Result<&dyn ProjectionView> {
        if !self.views.contains_key(&at) {
            let view = self.lake.engine.view(Some(at))?;
            self.views.insert(at, view);
        }
        Ok(self.views[&at].as_ref())
    }

    fn shape(&mut self, at: Position) -> Result<Arc<Shape>> {
        if let Some(s) = self.shapes.get(&at) {
            return Ok(s.clone());
        }
        let lake = self.lake;
        let shape = lake.shape(self.view(at)?)?;
        self.shapes.insert(at, shape.clone());
        Ok(shape)
    }

    fn ends(&mut self, at: Position) -> Result<bool> {
        if let Some(ends) = self.ends.get(&at) {
            return Ok(*ends);
        }
        let read = self.lake.read_these(&[at])?;
        let ends = (read.first()).is_some_and(|e| crate::lake::ends_a_range(&e.body));
        self.ends.insert(at, ends);
        Ok(ends)
    }

    fn get(
        &mut self,
        at: Position,
        ty: &TypeName,
        key: &RecordKey,
    ) -> Result<Option<StoredRecord>> {
        self.view(at)?.get(ty, key)
    }
}

/// The positions of the reactions among `events` that an act overruled,
/// in order; and the first one past what the projection has folded, as
/// while a rebuild runs, where deciding stops: such a reaction is told
/// neither standing nor overruled.
pub(crate) fn overruled<'e>(
    lake: &Lake,
    events: impl IntoIterator<Item = &'e Event>,
) -> Result<(Vec<Position>, Option<Position>)> {
    let Some((acted, known)) = lake.acted() else {
        return Ok((Vec::new(), None));
    };
    let mut asof = AsOf {
        lake,
        views: BTreeMap::new(),
        shapes: BTreeMap::new(),
        ends: BTreeMap::new(),
    };
    let (mut out, mut folded) = (Vec::new(), None);
    // The act's own append decided its retirements' reactions over the
    // values it erases: they share its time.
    let followed = |e: &&Event| e.position < acted || e.known_at == known;
    for e in events.into_iter().filter(followed) {
        let reaction = matches!(
            e.body,
            EventBody::TriggerFired { .. }
                | EventBody::TriggerDropped { .. }
                | EventBody::ObjectSetChanged { .. }
        );
        let Some(cause) = e.cause.filter(|_| reaction) else {
            continue;
        };
        let folded = match folded {
            Some(f) => f,
            None => *folded.insert(lake.engine.projection().folded_through()?),
        };
        if folded.is_none_or(|f| f < e.position) {
            return Ok((out, Some(e.position)));
        }
        if !stands(&mut asof, e, cause)? {
            out.push(e.position);
        }
    }
    Ok((out, None))
}

/// Whether `e`, caused at `cause`, stands: what is not a reaction does.
fn stands(asof: &mut AsOf<'_>, e: &Event, cause: Position) -> Result<bool> {
    // A cause is never the lake's first event.
    let before = Position(cause.0.saturating_sub(1));
    let max = asof.lake.limits.max_depth;
    match &e.body {
        EventBody::TriggerFired {
            trigger, ty, key, ..
        }
        | EventBody::TriggerDropped {
            trigger, ty, key, ..
        } => {
            let shape = asof.shape(cause)?;
            let o = &shape.ontology;
            let Some(t) = o.triggers.get(trigger.as_str()).map(|t| &t.decl) else {
                return Ok(true);
            };
            let (Some(c), Ok(on)) = (&t.when, o.ty(&t.on)) else {
                return Ok(true);
            };
            let at = [Place::Trigger {
                name: t.name.clone(),
            }];
            let when = declare::filter(&|n| o.set(n), &t.on, c, Eval::Guard, &at)?;
            let was = asof.get(before, ty, key)?.filter(|b| !b.retired);
            let Some(after) = asof.get(cause, ty, key)? else {
                return Ok(true);
            };
            let scope = WriteScope {
                after: &after.fields,
                before: was.as_ref().map(|b| &b.fields),
                retiring: was.is_some() && after.retired,
                meta: Some(&after.meta),
                settings: Some(&shape.settings),
                now: e.known_at,
            };
            let settings = (&shape.settings, t.when_strict);
            Ok(at_head(asof.view(cause)?, max, |h| {
                across::holds((h, o, on), &after, &when, scope, settings)
            }))
        }
        EventBody::ObjectSetChanged {
            set,
            ty,
            key,
            joined,
        } => {
            // Before the cause, by the shape then: a declaration, a
            // migration or a setting at the cause moves what the set reads.
            let then = match asof.ends(cause)? {
                true => asof.shape(before)?,
                false => asof.shape(cause)?,
            };
            let was = member(asof, &then, (before, e, true), (set, ty, key))?;
            let now = asof.shape(cause)?;
            let is = member(asof, &now, (e.position, e, false), (set, ty, key))?;
            Ok(was != *joined && is == *joined)
        }
        _ => Ok(true),
    }
}

/// Whether the record of `ty` at `key` is a member of `set` as of `at`, by
/// `shape`: the kept membership of a set across an edge or of a kept view,
/// the set's `where` over the record for any other; a set `shape` does
/// not declare holds none. The membership `was` before a write is the one
/// the write read, kept by an earlier decision, unless the record then
/// holds an erased value the `where` reads: a `where` that reads the clock
/// moves without a write, and the write that tells it decided `was` so.
fn member(
    asof: &mut AsOf<'_>,
    shape: &Shape,
    (at, e, was): (Position, &Event, bool),
    (set, ty, key): (&str, &TypeName, &RecordKey),
) -> Result<bool> {
    let o = &shape.ontology;
    let Some(s) = o.set(set) else {
        return Ok(false);
    };
    let view = o.ty(&s.of).is_ok_and(|t| t.view.is_some());
    if view || reads_across(o, s) {
        return asof.view(at)?.is_member(set, ty, key);
    }
    let Some(r) = asof.get(at, ty, key)? else {
        return Ok(false);
    };
    let filter = declare::set_filter(&|n| o.set(n), s, &[])?;
    let erased = |f: &FieldName| r.meta.get(f).is_some_and(|m| m.erased);
    if was && !lakelands_core::derive::reads(&filter).iter().any(erased) {
        return asof.view(at)?.is_member(set, ty, key);
    }
    let max = asof.lake.limits.max_depth;
    at_head(asof.view(at)?, max, |h| {
        member_of((s, o), &r, &shape.settings, e.known_at, h)
    })
}

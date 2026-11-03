//! The part rule the acts that retire records share
//! (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`): a purge
//! and a removal begin an act and append its retirements a part at a time,
//! each append at most `limits.part_events` long, one record's retirements
//! in one append, and `act_whole` where nothing is left and it fits.
//!
//! One copy, so that the two acts cannot drift: what a part carries — the
//! closes still owed and the records still to retire, each read again at
//! its live version as the part takes it — is written once. The acts
//! differ only in the change each retirement is caused as and in the map
//! their walk left them.

use std::collections::{BTreeMap, VecDeque};

use lakelands_api::engine::ProjectionView;
use lakelands_api::event::{ChangeKind, EventBody};
use lakelands_api::record::StoredRecord;
use lakelands_api::{Position, RecordKey, Result, Timestamp, TypeName};

use crate::lake::{Lake, Writer};
use crate::log::Draft;
use crate::writes;

/// What a part carries: the closes still owed, the records still to
/// retire, the change each retirement is caused as — a purge's `Purged`,
/// a removal's `Retired` — and the act they belong to.
pub(crate) struct Part<'a> {
    pub closes: &'a mut VecDeque<EventBody>,
    pub taken: &'a mut BTreeMap<(TypeName, RecordKey), StoredRecord>,
    pub kind: ChangeKind,
    pub act: Position,
}

/// The closes of the act's first turn that fit in `room` beside its
/// opening event; the rest stay owed to the parts.
pub(crate) fn take_closes(closes: &mut VecDeque<EventBody>, room: usize) -> Vec<EventBody> {
    let fit = room.saturating_sub(1).min(closes.len());
    closes.drain(..fit).collect()
}

/// One part: the closes still owed and the selection's next records, each
/// read again as it stands now, as one append of at most `room` events,
/// with `act_whole` in it where nothing is left and it fits. A record
/// another hand wrote since keeps its record; one gone is dropped (099,
/// "What stands between parts"). One record's retirements are one
/// append's, however many of them it takes.
pub(crate) fn part(
    lake: &Lake,
    (w, view): (&Writer, &dyn ProjectionView),
    now: Timestamp,
    room: usize,
    mut drafts: Vec<Draft>,
    part: Part<'_>,
) -> Result<(Vec<Draft>, bool)> {
    let Part {
        closes,
        taken,
        kind,
        act,
    } = part;
    let system = lake.system("lakelands");
    let asked = writes::Asked {
        lake,
        who: &system,
        context: &lakelands_api::WriteContext::default(),
    };
    while drafts.len() < room {
        let Some(body) = closes.pop_front() else {
            break;
        };
        drafts.push(Draft::system(now, body));
    }
    // The retirements are caused by their own position in the append,
    // after the closes the part carries in front of them.
    let mut d = asked.decide(w, view, now, drafts.len());
    d.bound = true;
    d.retiring_as = kind;
    while drafts.len() < room {
        let Some((_, before)) = taken.first_key_value() else {
            break;
        };
        let before = before.clone();
        let live = match view.get(&before.ty, &before.key)? {
            Some(live) if live.version == before.version => live,
            _ => {
                taken.remove(&(before.ty, before.key));
                continue;
            }
        };
        let body = writes::retirement(&live.ty, &live.key, live.version);
        let written = d.retiring(&live, Draft::system(now, body))?;
        if !drafts.is_empty() && drafts.len() + written.len() > room {
            break;
        }
        taken.remove(&(live.ty.clone(), live.key.clone()));
        drafts.extend(written);
    }
    let ends = taken.is_empty() && closes.is_empty();
    let whole = ends && drafts.len() < room;
    if whole {
        drafts.push(Draft::system(now, EventBody::ActWhole { act }));
    }
    Ok((drafts, whole))
}

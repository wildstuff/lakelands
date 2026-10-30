//! `ingest`: a stream of records, each from the call's source or its
//! own, in group-commit windows.
//!
//! Records are taken from the stream into a window until it holds the
//! call's `commit_window` records, else the lake's
//! `commit_window_events`, or 2 ms have passed since its first record;
//! the window is then decided whole and appended durably, and answers one
//! [`Committed`] with the position its last event took and the call's
//! delivery: the caller's, else `d<first position>`, the position of the
//! first event the call appended. Every `record_ingested` of the call
//! carries it in its envelope, or the delivery its record names: one call
//! may carry many deliveries, each taken back alone (087). A window is all or nothing: if one of its
//! records is refused, nothing of the window is appended, the refusal is
//! the stream's next item and the stream ends — the caller's own offset
//! says where to resume
//! (`docs/decisions/016-two-write-doors-by-origin.md`).
//!
//! **Idempotency by content.** A record's event id is derived from its
//! source, type, key, the record's version before it and its (checked)
//! values (`docs/log-format.md` §3). A record whose id is already in the
//! log, or earlier in the same window, appends nothing and is counted as a
//! duplicate; so does a record that would change no field — a full
//! re-import of unchanged data appends nothing. At-least-once delivery is
//! therefore safe. A window is judged first, then asks the projection for
//! all its ids in one call, and is judged again where one is applied. The
//! unchanged test comes after the door's: a record that carries a field
//! outside the source's authority is refused, not counted as a duplicate. A value that returns after a change is a new write, and
//! the same key with different values moves its version on, within a
//! window too, in stream order.

use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_core::Stream;
use lakelands_api::catalog::{CommitStream, SourceStream};
use lakelands_api::engine::ProjectionView;
use lakelands_api::error::{Case, Place, Said};
use lakelands_api::event::{EventBody, EventId, Sealed};
use lakelands_api::id::KeyId;
use lakelands_api::principal::WriteContext;
use lakelands_api::quarantine::{
    ClosedWhy, HostRefused, QuarantineClose, QuarantineFilter, QuarantineFull, QuarantineId,
    QuarantineState, Quarantined, Quarantining,
};
use lakelands_api::record::Origin;
use lakelands_api::write::{Committed, IngestOpts};
use lakelands_api::{
    DeliveryId, Error, Missing, Position, Principal, RecordKey, Result, SourceId, SourceRecord,
    Timestamp, TypeName,
};
use lakelands_core::sight::Door;

use crate::exec::{self, Turn};
use crate::lake::{Lake, Reads, Writer};
use crate::log::{self, Draft};
use crate::purge::made_by;
use crate::quarantine::{close_of, landing, there};
use crate::sealing::turns;
use crate::writes::{Call, Decide};
use lakelands_core::{canonical, links};

/// How long a window waits for more records after its first.
const WINDOW: Duration = Duration::from_millis(2);

/// The keys of a call sealed apart, one per source and delivery it writes
/// for, none naming the call's own, made before the first window that
/// seals a value of the pair and kept for its every window (065, 087).
pub type Apart = std::sync::Mutex<BTreeMap<Pair, KeyId>>;

/// A source and the delivery a record names, none for the call's.
type Pair = (SourceId, Option<DeliveryId>);

/// The record's own source, else the call's, and the delivery it names.
fn pair_of(r: &SourceRecord, source: &SourceId) -> Pair {
    (
        r.source.as_ref().unwrap_or(source).clone(),
        r.delivery.clone(),
    )
}

/// How a call quarantines (085): whether it does, the host's run that
/// keys its records in place of their delivery, and the host's refusal of
/// them where the call is the host's door's, which ingests none.
type Quarantine = (bool, Option<DeliveryId>, Option<Error>);

/// Decide and append one window, in the call's `delivery` where one is
/// named or decided already, its values sealed apart too where `apart`
/// keeps the call's keys; `places` holds each record's place among the
/// call's records of its delivery. It answers too the id of each record
/// it quarantined or found quarantined so already.
fn window(
    lake: &Turn<'_>,
    who: &Principal,
    (context, delivery, apart, (quarantine, run, host)): (
        WriteContext,
        Option<DeliveryId>,
        Option<&Apart>,
        Quarantine,
    ),
    source: &SourceId,
    (first, records, places): (u64, Vec<SourceRecord>, Vec<u64>),
) -> Result<(Committed, Vec<QuarantineId>)> {
    crate::writes::check_context(lake, &context, who)?;
    let mut w = lake.for_write()?;
    // Under the writer, so through every rename the lake has folded, and
    // with its references as the head's types show them (024, extended).
    let who = (lake
        .sight(who.clone(), (None, false), None, &w.ontology)?
        .0
        .who())
    .clone();
    let (who, followed) = (&who, None);
    let call = Call {
        lake,
        who,
        context,
        followed,
        shown: None,
    };
    let view = lake.engine.view(None)?;
    let now = lake.clock(&mut w);
    let next = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0.next());
    // A record quarantined is keyed by the call's delivery, which this
    // window's append decides where none is yet (085).
    let keyed: DeliveryId = delivery
        .clone()
        .unwrap_or_else(|| format!("d{next}").into());
    let quarantine = quarantine.then_some(Asked {
        call: &keyed,
        run: run.as_ref(),
        max: lake.limits.quarantined_max,
        host: host.as_ref(),
    });
    // The window is judged taking no id as applied, and the ids it asked
    // are asked in one call; where one is applied, the window is judged
    // again, each id asked as the record meets it, so every record is
    // judged as its own ask would have it (lakelands#1036).
    let mut asks = Asks::default();
    let judged = loop {
        let judged = judge(
            (&call, apart, quarantine),
            &w,
            (view.as_ref(), now),
            (source, first, &records, &places),
            &mut asks,
        );
        // An outage too: the judgement may have met it past a record that
        // an applied id refuses.
        if asks.found.is_some() || asks.asked.is_empty() {
            break judged;
        }
        let held = view.applied_many(&asks.asked)?.into_iter();
        let found: BTreeMap<_, _> = (asks.asked.drain(..).zip(held))
            .map(|(id, at)| (id, at.is_some()))
            .collect();
        if !found.values().any(|held| *held) {
            break judged;
        }
        asks.found = Some(found);
    };
    let Judged {
        mut drafts,
        reads,
        committed,
        duplicates,
        quarantined,
        ids,
    } = judged?;
    drop(view);
    let delivery = delivery.or_else(|| (!drafts.is_empty()).then_some(keyed));
    let ingested = |d: &&mut Draft| {
        matches!(
            d.body,
            EventBody::RecordIngested { .. } | EventBody::RecordQuarantined { .. }
        )
    };
    for d in drafts.iter_mut().filter(ingested) {
        d.delivery = d.delivery.take().or_else(|| delivery.clone());
    }
    // A window that appends nothing committed no record, and answers the head.
    let position = if drafts.is_empty() {
        w.tip.as_ref().map_or(Position::GENESIS, |t| t.0)
    } else {
        let events = lake.append_after(&mut w, drafts, reads)?;
        events.last().expect("appended").position
    };
    let committed = Committed {
        position,
        records: committed,
        duplicates,
        delivery,
        quarantined,
    };
    Ok((committed, ids))
}

/// What a judgement of a window takes as applied: at first no id, each it
/// meets kept in `asked`; judged again, the answers `found` for those, and
/// any other id asked as the record meets it.
#[derive(Default)]
struct Asks {
    asked: Vec<EventId>,
    found: Option<BTreeMap<EventId, bool>>,
}

impl Asks {
    fn applied(&mut self, view: &dyn ProjectionView, id: &EventId) -> Result<bool> {
        match self.found.as_ref().map(|found| found.get(id)) {
            None => {
                self.asked.push(id.clone());
                Ok(false)
            }
            Some(Some(held)) => Ok(*held),
            Some(None) => Ok(view.applied(id)?.is_some()),
        }
    }
}

/// The window's records judged: their drafts, the reads they rest on, how
/// many were written and counted duplicates, the stream index of each
/// it quarantined, and the id of each it quarantined or found so.
struct Judged {
    drafts: Vec<Draft>,
    reads: Reads,
    committed: u32,
    duplicates: u32,
    quarantined: Vec<u64>,
    ids: Vec<QuarantineId>,
}

/// What a call that quarantines asks: the delivery that keys its records,
/// the host's run in its place, the bound, and the host's refusal.
#[derive(Clone, Copy)]
struct Asked<'a> {
    call: &'a DeliveryId,
    run: Option<&'a DeliveryId>,
    max: u32,
    host: Option<&'a Error>,
}

/// The quarantined records a window has met, as it leaves them, over the
/// projection's: each it wrote a version of or closed, and per source how
/// many are open, the projection's count and the window's change.
struct Rows<'a> {
    /// What the call asks, where it quarantines.
    asked: Option<Asked<'a>>,
    rows: BTreeMap<QuarantineId, Quarantined>,
    open: BTreeMap<SourceId, (usize, i64)>,
}

impl Rows<'_> {
    fn row(&self, view: &dyn ProjectionView, id: &QuarantineId) -> Result<Option<Quarantined>> {
        crate::quarantine::row(view, &self.rows, id)
    }

    /// `source`'s open rows, as the projection counts them up to the
    /// bound and past it by one, and as the window opened and closed them.
    fn open(&mut self, view: &dyn ProjectionView, source: &SourceId) -> Result<&mut (usize, i64)> {
        if !self.open.contains_key(source) {
            let max = self.asked.map_or(0, |a| a.max);
            let open = QuarantineFilter {
                source: Some(source.clone()),
                state: Some(QuarantineState::Open),
                limit: Some(max.saturating_add(1)),
                ..QuarantineFilter::default()
            };
            let stored = view.quarantined(&open)?.len();
            self.open.insert(source.clone(), (stored, 0));
        }
        Ok(self.open.get_mut(source).expect("counted"))
    }

    /// The open rows at `at`, of any source, closed as a record lands
    /// there, as [`Lake::closing`] closes them after the window's append:
    /// their places under the bound are free to the window's next
    /// refusal, as to the next window's (085).
    fn landed(
        &mut self,
        view: &dyn ProjectionView,
        (ty, key): (TypeName, RecordKey),
    ) -> Result<()> {
        let at = QuarantineFilter {
            ty: Some(ty),
            key: Some(key),
            state: Some(QuarantineState::Open),
            ..QuarantineFilter::default()
        };
        for q in crate::quarantine::rows(view, &self.rows, &at)? {
            self.open(view, &q.id.source)?.1 -= 1;
            let close = QuarantineClose {
                why: ClosedWhy::Landed,
                position: q.position,
            };
            self.rows
                .insert(q.id.clone(), crate::quarantine::closed(q, close));
        }
        Ok(())
    }
}

/// The open quarantined record of `source` the record `r` resubmits,
/// there to its writer, where it names one (085); one it names that is not
/// is refused as one that never was.
fn resubmitted(
    d: &Decide<'_>,
    (view, rows): (&dyn ProjectionView, &Rows<'_>),
    r: &SourceRecord,
    source: &SourceId,
) -> Result<Option<Quarantined>> {
    let Some(rs) = &r.resubmits else {
        return Ok(None);
    };
    if let Some(own) = &r.source {
        crate::writes::check_source(own)?;
    }
    let id = QuarantineId {
        source: source.clone(),
        delivery: rs.delivery.clone(),
        index: rs.index,
    };
    if let Some(q) = rows.row(view, &id)?.filter(|q| q.closed.is_none()) {
        let live = view.get(&q.ty, &q.key)?;
        if there(&d.at.sight, live.as_ref(), &q) {
            return Ok(Some(q));
        }
    }
    Err(Error::NotFound {
        missing: Missing::Quarantined { id },
    })
}

fn judge(
    (call, apart, quarantine): (&Call<'_>, Option<&Apart>, Option<Asked<'_>>),
    w: &Writer,
    (view, now): (&dyn ProjectionView, Timestamp),
    (source, first, records, places): (&SourceId, u64, &[SourceRecord], &[u64]),
    asks: &mut Asks,
) -> Result<Judged> {
    let lake = call.lake;
    // A forgotten source is never given a key again, and writes nothing
    // (058).
    let forgotten = crate::lake::forgotten_list(view, &Default::default())?;
    let mut d = call.decide(w, view, now, 0);
    (d.door, d.envelope.origin) = (Door::Source, Origin::Source);
    let mut seen: BTreeSet<EventId> = BTreeSet::new();
    let mut drafts = Vec::new();
    let mut committed = 0u32;
    let mut duplicates = 0u32;
    // The window's records as the projection holds them, read a type at a
    // time rather than one by one; a key the window names twice is read
    // once, and its second record still reads the first as planned. Only
    // the records up to the first whose type or key the loop refuses are
    // read, so a key no engine can hold is refused, not sent.
    let mut keys: BTreeMap<&lakelands_api::TypeName, Vec<_>> = Default::default();
    let formed = |r: &&SourceRecord| {
        d.at.sight.ty(&r.ty).is_ok() && crate::writes::check_key(&r.key).is_ok()
    };
    // Where the call quarantines, the window goes on past a record so
    // refused, and every formed record is read.
    let primed: Vec<_> = match quarantine {
        None => records.iter().take_while(formed).collect(),
        Some(_) => records.iter().filter(formed).collect(),
    };
    for r in &primed {
        keys.entry(&r.ty).or_default().push(r.key.clone());
    }
    // The targets the written references of those records name, read the
    // same way: each reference is still judged from the same view.
    let mut targets: BTreeMap<&lakelands_api::TypeName, Vec<_>> = Default::default();
    for r in &primed {
        let decl = d.at.sight.ty(&r.ty).expect("a formed record names a type");
        for f in &decl.fields {
            let named = (f.ty.target.as_ref()).zip(r.fields.get(&f.name).and_then(|v| v.as_str()));
            // Only a key an engine can hold is read; any other value is
            // judged as before.
            let named = named.map(|(target, to)| (target, RecordKey::from(to)));
            if let Some((target, to)) = named.filter(|(_, to)| crate::writes::check_key(to).is_ok())
            {
                targets.entry(target).or_default().push(to);
            }
        }
    }
    for (ty, keys) in keys.into_iter().chain(targets) {
        d.pending.prime(view, ty, &keys)?;
    }
    if let Some(apart) = apart {
        ask_apart((lake, w), apart, (&d, &forgotten), source, records);
    }
    // The keys of an insert-only type this window writes, each once.
    let mut once = BTreeSet::new();
    let mut ciphers = crate::sealing::Ciphers::default();
    let mut rows = Rows {
        asked: quarantine,
        rows: BTreeMap::new(),
        open: BTreeMap::new(),
    };
    let (mut quarantined, mut ids) = (Vec::new(), Vec::new());
    for (i, (r, place)) in records.iter().zip(places).enumerate() {
        let index = first + i as u64;
        let pair = pair_of(r, source);
        // The record's own source, else the call's: its event and its id.
        let source = r.source.as_ref().unwrap_or(source);
        let target = resubmitted(&d, (view, &rows), r, source);
        let target = target.map_err(|e| ingested(e, index, first, r))?;
        let from = drafts.len();
        // The host's refusal is the record's, past what keeps one from
        // quarantine: it is not judged by the type's rules (085, extended).
        let host = quarantine.and_then(|a| a.host);
        let judged = host.map(|e| gate(&d, r, source, &forgotten).and(Err(e.clone())));
        let judged = judged.unwrap_or_else(|| {
            let decl = d.at.sight.ty(&r.ty)?;
            crate::writes::untold_setting(&d.at.sight, decl, &r.key)?;
            d.envelope.source = Some(source.clone());
            // The key's form before the door, as `upsert` checks it. Then the
            // door: a patch that carries a field the source is not the
            // authority on, nor admitted to, is refused whole, even when that
            // field's value is the one already stored.
            let patch = d.checked(decl, &r.key, &r.fields)?;
            if let Some(own) = &r.source {
                crate::writes::check_source(own)?;
            }
            if let Some(own) = &r.delivery {
                crate::writes::check_delivery(own)?;
            }
            stated(r)?;
            // A record a holder of `only` is not admitted is, to it, a key that
            // is free: it is not compared, and the write is refused as on a
            // free key with another's owner (051).
            let t = d.target(decl, &r.key)?;
            // A link type no relation carries, its relation renamed or
            // removed, stays declared for its history and takes no link.
            let types = w.ontology.types.values().map(|e| &e.decl);
            let relation = (links::is_link_type(&r.ty)).then(|| links::relation_of(types, &r.ty));
            if let Some(None) = relation {
                let ty = r.ty.clone();
                return Err(Error::invalid(Case::UnclaimedLinkType { ty }));
            }
            let other = t.own().is_none() && t.before.is_some();
            let version_before = t.before.as_ref().map_or(0, |b| b.version.0);
            if forgotten.contains_key(source) {
                let source = source.clone();
                return Err(Error::invalid(Case::SourceForgotten { source }));
            }
            // Its values of an erasable field enter the log sealed under its
            // key, in one box, and its id is the patch's and the box's as
            // stored: no digest of a plaintext value enters the log (058, 083).
            let sealed = sealing((lake, w, view, now), apart, &pair, (decl, &patch))?;
            let id = match &sealed {
                Some(s) => {
                    let mut body = EventBody::RecordIngested {
                        ty: r.ty.clone(),
                        key: r.key.clone(),
                        version: lakelands_api::Version(version_before + 1),
                        patch: patch.clone(),
                        made: false,
                    };
                    let mut boxed = s.clone();
                    let cipher = ciphers.sealed(&lake.keys, source, s)?;
                    crate::sealing::seal(&mut body, &mut boxed, source, cipher);
                    let EventBody::RecordIngested { patch: stored, .. } = &body else {
                        unreachable!("the body sealed is a record_ingested")
                    };
                    let boxed = boxed.boxed.unwrap_or_default();
                    log::boxed_ingest_id(source, &r.ty, &r.key, version_before, stored, &boxed)
                }
                None => log::ingest_id(source, &r.ty, &r.key, version_before, &patch),
            };
            let unchanged = t.live().is_some_and(|b| {
                patch.iter().all(|(f, v)| match b.fields.get(f) {
                    Some(stored) => stored == v,
                    None => v.is_null(),
                })
            });
            if unchanged || !other && (seen.contains(&id) || asks.applied(view, &id)?) {
                duplicates += 1;
                return Ok(());
            }
            if patch.is_empty() {
                return Err(Error::invalid(Case::PatchEmpty));
            }
            // An authored field it is admitted to, only where no other
            // hand's value stands (069).
            let made = || match t.live() {
                Some(r) => Ok(made_by(lake, &[r], std::slice::from_ref(source))?.contains(&r.made)),
                None => Ok(false),
            };
            lakelands_core::check_admitted(decl, (t.live(), source), &patch, made)?;
            // An insert-only type's key is written once: a record stands
            // at it, live or retired, or this window wrote it. Another's
            // record and a held one are judged as before, so the mark
            // tells nothing of them (lakelands#1044).
            let stands = t.before.is_some() && !other;
            let at = (r.ty.clone(), r.key.clone());
            if decl.insert_only && (stands || once.contains(&at)) {
                let (ty, key) = (r.ty.clone(), r.key.clone());
                return Err(Error::constraint(None, Case::WrittenOnce { ty, key }));
            }
            // The record is applied as the source states it, so a guard reads
            // the `seen_at` the fold keeps.
            let stated = (Some(id.clone()), r.seen_at, r.upstream.clone());
            (d.envelope.id, d.envelope.seen_at, d.envelope.upstream) = stated;
            d.envelope.sealed = sealed;
            let plan = d.write(decl, &r.key, t, patch, None)?;
            // A link holds its relation's cardinality, over the lake's live
            // links and those earlier in this window: judged once the plan
            // admitted the record its writer leaves,
            // so that it names nothing of another's record and passes over no
            // link a holder's `only` admits (051).
            if let Some(Some((t, relation))) = relation {
                // The ends as the record holds them: the key rule fills
                // those the patch leaves out.
                let end = |f: &str| {
                    let end = plan.after.fields.get(f).and_then(|v| v.as_str());
                    end.map(lakelands_api::RecordKey::from)
                };
                if let (Some(from), Some(to)) = (end(links::FROM), end(links::TO)) {
                    crate::writes::cardinality(
                        (view, &d.at.sight),
                        &d.pending,
                        (&t.name, relation),
                        &from,
                        &to,
                    )?;
                }
            }
            let planned = d.planned(plan)?.drafts.into_iter();
            drafts.extend(planned.map(|p| Draft {
                delivery: r.delivery.clone(),
                ..p
            }));
            if decl.insert_only {
                once.insert(at);
            }
            seen.insert(id);
            committed += 1;
            Ok(())
        });
        match judged {
            // A resubmission written, or counted a duplicate, closes what
            // it names, in the window's append (085).
            Ok(()) => {
                if let Some(q) = target {
                    let close = Draft {
                        id: None,
                        sealed: None,
                        seen_at: None,
                        upstream: None,
                        source: Some(q.id.source.clone()),
                        ..d.draft(close_of(&q, ClosedWhy::Resubmitted))
                    };
                    drafts.push(d.alone(close)?);
                    rows.open(view, &q.id.source)?.1 -= 1;
                    let why = ClosedWhy::Resubmitted;
                    let close = QuarantineClose {
                        why,
                        position: q.position,
                    };
                    rows.rows
                        .insert(q.id.clone(), crate::quarantine::closed(q, close));
                }
                if rows.asked.is_some() {
                    let landed: Vec<_> = drafts[from..].iter().filter_map(landing).collect();
                    landed
                        .into_iter()
                        .try_for_each(|at| rows.landed(view, at))?;
                }
            }
            // A record refused names itself and where its window began,
            // which nothing from was written (016, lakelands#1043); an
            // outage does not.
            Err(e) if rows.asked.is_none() => return Err(ingested(e, index, first, r)),
            Err(e) => {
                let (id, made) = quarantining(
                    (call.lake, w, view, now),
                    (&mut d, &mut rows, &mut ciphers, apart),
                    (r, &pair, target.as_ref(), &forgotten),
                    (e, index, first, *place),
                )?;
                ids.push(id);
                match made {
                    Some(draft) => {
                        drafts.push(draft);
                        quarantined.push(index);
                    }
                    None => duplicates += 1,
                }
            }
        }
    }
    let reads = d.pending.into_reads();
    Ok(Judged {
        drafts,
        reads,
        committed,
        duplicates,
        quarantined,
        ids,
    })
}

/// The declaration of `r`'s type, where nothing keeps a refusal of `r`
/// from quarantine, else what does as `r`'s own refusal: a type not
/// shown, a setting's key not told, a source or delivery not of its form,
/// what it states beside its fields out of range, a forgotten source, or
/// a key the log cannot hold (085).
fn gate<'d>(
    d: &'d Decide<'_>,
    r: &SourceRecord,
    source: &SourceId,
    forgotten: &BTreeMap<SourceId, String>,
) -> Result<&'d lakelands_api::ontology::TypeDecl> {
    let decl = d.at.sight.ty(&r.ty)?;
    crate::writes::untold_setting(&d.at.sight, decl, &r.key)?;
    r.source
        .as_ref()
        .map_or(Ok(()), crate::writes::check_source)?;
    r.delivery
        .as_ref()
        .map_or(Ok(()), crate::writes::check_delivery)?;
    stated(r)?;
    if forgotten.contains_key(source) {
        let source = source.clone();
        return Err(Error::invalid(Case::SourceForgotten { source }));
    }
    if r.key.as_str().contains('\0') {
        crate::writes::check_key(&r.key)?;
    }
    Ok(decl)
}

/// The values of `patch` a record of `decl` from the pair's source seals:
/// those of its erasable fields that hold one, in one box under the
/// source's newest key, turned as the settings say, and the pair's own
/// where the call is sealed apart (058, 065, 083, 087); none where it
/// holds none.
fn sealing(
    (lake, w, view, now): (&Lake, &Writer, &dyn ProjectionView, Timestamp),
    apart: Option<&Apart>,
    pair: &Pair,
    (decl, patch): (&lakelands_api::ontology::TypeDecl, &lakelands_api::Patch),
) -> Result<Option<Sealed>> {
    let source = &pair.0;
    let mut fields = erasable(decl, patch);
    if fields.is_empty() {
        return Ok(None);
    }
    fields.sort();
    let list = lake.keys.listed(view, source)?;
    let rotate = turns(&w.settings, list.as_deref(), source, now);
    let newest = list.as_ref().and_then(|l| l.firsts.last());
    let (key, _) = lake.keys.sealing(source, newest.map(|(k, _)| k), rotate)?;
    // A delivery sealed apart seals under its version and its own key,
    // composed (065).
    let apart = apart.map(|a| apart_key((lake, w), a, pair)).transpose()?;
    Ok(Some(Sealed {
        key,
        source: None,
        fields,
        apart: apart.into_iter().collect(),
        boxed: None,
    }))
}

/// The version that `r`'s refusal `e` makes of its quarantined record,
/// the one it resubmits or its own place among the call's records of its
/// delivery: none
/// where it is a duplicate of the record's last version (085), beside its
/// id: the host's run keys it in place of its delivery, its line in place
/// of its place. A refusal
/// of the call, of its source, of a type not shown or of a key or value
/// the log cannot hold is not quarantined, nor is one past the bound: the
/// stream ends at it as without the option, past the bound with the bound
/// beside it.
fn quarantining(
    (lake, w, view, now): (&Lake, &Writer, &dyn ProjectionView, Timestamp),
    (d, rows, ciphers, apart): (
        &mut Decide<'_>,
        &mut Rows<'_>,
        &mut crate::sealing::Ciphers,
        Option<&Apart>,
    ),
    (r, pair, target, forgotten): (
        &SourceRecord,
        &Pair,
        Option<&Quarantined>,
        &BTreeMap<SourceId, String>,
    ),
    (e, index, first, place): (Error, u64, u64, u64),
) -> Result<(QuarantineId, Option<Draft>)> {
    let Asked { call, run, max, .. } = rows.asked.expect("the call quarantines");
    let source = &pair.0;
    let keyed = run.or(pair.1.as_ref()).unwrap_or(call);
    let decl = match gate(d, r, source, forgotten) {
        Ok(decl) if !matches!(e, Error::Unavailable { .. } | Error::Unbound { .. }) => decl,
        _ => return Err(ingested(e, index, first, r)),
    };
    let id = target.map_or_else(
        || QuarantineId {
            source: source.clone(),
            delivery: keyed.clone(),
            index: r.line.unwrap_or(place),
        },
        |q| q.id.clone(),
    );
    let last = match target {
        Some(q) => Some(q.clone()),
        None => rows.row(view, &id)?,
    };
    let sealed = sealing((lake, w, view, now), apart, pair, (decl, &r.fields));
    let sealed = sealed.map_err(|e| ingested(e, index, first, r))?;
    let body = |version: u32| EventBody::RecordQuarantined {
        ty: r.ty.clone(),
        key: r.key.clone(),
        id: id.clone(),
        version,
        cells: r.fields.clone(),
        sealed: None,
        seen_at: r.seen_at,
        upstream: r.upstream.clone(),
        refusal: e.clone(),
    };
    let written = serde_json::to_value(body(1)).expect("an event body serialises to JSON");
    if canonical::integer_too_large(&written).is_some() {
        return Err(ingested(e, index, first, r));
    }
    let mut same = |q: &Quarantined| -> Result<bool> {
        Ok(match &q.closed {
            None => q.cells == r.fields && q.refusal == e,
            Some(c) if c.why == ClosedWhy::Removed => false,
            Some(_) => {
                let at = version_id((lake, ciphers), source, body(q.version), sealed.as_ref());
                view.applied(&at.map_err(|e| ingested(e, index, first, r))?)? == Some(q.position)
            }
        })
    };
    if let Some(q) = &last
        && same(q)?
    {
        return Ok((id, None));
    }
    // An id new or closed takes a place under the bound; a version of an
    // open one does not.
    if last.as_ref().is_none_or(|q| q.closed.is_some()) {
        let open = rows.open(view, source)?;
        if open.0 as i64 + open.1 >= i64::from(max) {
            let mut full = ingested(e, index, first, r);
            if let Error::Ingested {
                quarantine_full, ..
            } = &mut full
            {
                *quarantine_full = Some(QuarantineFull { max });
            }
            return Err(full);
        }
        open.1 += 1;
    }
    let version = last.as_ref().map_or(0, |q| q.version) + 1;
    let at = version_id((lake, ciphers), source, body(version), sealed.as_ref());
    let draft = Draft {
        id: Some(at.map_err(|e| ingested(e, index, first, r))?),
        source: Some(source.clone()),
        delivery: pair.1.clone(),
        sealed,
        seen_at: None,
        upstream: None,
        origin: Origin::Source,
        ..d.draft(body(version))
    };
    let row = Quarantined {
        id: id.clone(),
        ty: r.ty.clone(),
        key: r.key.clone(),
        version,
        position: Position::GENESIS,
        quarantined_at: now,
        cells: r.fields.clone(),
        masked: Vec::new(),
        sensitive: Vec::new(),
        seen_at: r.seen_at,
        upstream: r.upstream.clone(),
        refusal: e,
        more: false,
        closed: None,
    };
    rows.rows.insert(id.clone(), row);
    Ok((id, Some(d.alone(draft)?)))
}

/// The id of a quarantined record's version `body`, from its cells and box
/// as the log stores them (`docs/log-format.md` §3).
fn version_id(
    (lake, ciphers): (&Lake, &mut crate::sealing::Ciphers),
    source: &SourceId,
    mut body: EventBody,
    sealed: Option<&Sealed>,
) -> Result<EventId> {
    let mut boxed = None;
    if let Some(s) = sealed {
        let mut s = s.clone();
        let cipher = ciphers.sealed(&lake.keys, source, &s)?;
        crate::sealing::seal(&mut body, &mut s, source, cipher);
        boxed = s.boxed;
    }
    let EventBody::RecordQuarantined {
        id,
        version,
        cells,
        refusal,
        ..
    } = &body
    else {
        unreachable!("the body sealed is a record_quarantined")
    };
    let before = version - 1;
    Ok(log::quarantined_id(
        id,
        before,
        cells,
        boxed.as_deref(),
        refusal,
    ))
}

/// The refusal `e` of the record `r`, at `index` in its stream, its window
/// begun at `first`; an outage as it is, since it is no record's.
fn ingested(
    e: lakelands_api::Error,
    index: u64,
    first: u64,
    r: &SourceRecord,
) -> lakelands_api::Error {
    if matches!(e, lakelands_api::Error::Unavailable { .. }) {
        return e;
    }
    let record = lakelands_api::error::RecordName::Of {
        ty: r.ty.clone(),
        key: r.key.clone(),
    };
    lakelands_api::Error::Ingested {
        index,
        from: first,
        record,
        error: Box::new(e),
        quarantine_full: None,
    }
}

/// What a source states of a record beside its fields is written as it
/// was given, so it is judged as a written value is
/// (`docs/log-format.md` §1.2, §1.3): a `seen_at` out of the years 0000 to
/// 9999, which an instant built in process can be, and an upstream
/// position or a line beyond ±(2^53 − 1) are refused, each naming its
/// member.
fn stated(r: &SourceRecord) -> Result<()> {
    let at = |path: &str, case: Case| {
        let path = path.to_owned();
        Err(Error::invalid(
            Said::from(case).within(Place::Member { path }),
        ))
    };
    if let Some(t) = r.seen_at.filter(|t| !t.in_years()) {
        let value = t.to_string();
        return at("seen_at", Case::NotDateTime { value });
    }
    let position = r
        .upstream
        .as_ref()
        .map(|u| ("upstream.position", u.position.0));
    let large = position.into_iter().chain(r.line.map(|l| ("line", l)));
    let large = large.map(|(path, n)| (path, serde_json::Value::from(n)));
    match large
        .filter_map(|(p, n)| canonical::integer_too_large(&n).map(|n| (p, n.clone())))
        .next()
    {
        Some((path, n)) => at(path, Case::IntegerTooLarge { n }),
        None => Ok(()),
    }
}

/// The key the pair's values of the call are sealed apart under, by id:
/// the one `apart` keeps, else a new one the key store makes, kept there.
fn apart_key((lake, w): (&Lake, &Writer), apart: &Apart, pair: &Pair) -> Result<KeyId> {
    match kept((lake, w), apart, pair) {
        Some(id) => Ok(id),
        None => {
            let (id, _) = lake.keys.apart(&pair.0)?;
            lakelands_core::lock(apart).insert(pair.clone(), id.clone());
            Ok(id)
        }
    }
}

/// The key `apart` keeps for the pair. One an act of removal named
/// since, which a writer knows from the act it met or made, is not sealed
/// under again: what the call writes after the act is not the act's, so
/// it takes a key of its own (065).
fn kept((lake, w): (&Lake, &Writer), apart: &Apart, pair: &Pair) -> Option<KeyId> {
    let source = &pair.0;
    let taken = |id: &KeyId| {
        lake.keys.doomed(source, id) || w.removals.iter().any(|r| r.destroyed(source, id))
    };
    (lakelands_core::lock(apart).get(pair).cloned()).filter(|id| !taken(id))
}

/// Ask the key store, in one request, for a key of every pair of the
/// window's records that would seal a value and whose key the call does
/// not keep, so they are kept before the window's append (087). A record
/// would seal where its type is shown, its key, source and delivery are
/// well formed, its source is not forgotten and it carries a value of an
/// erasable field. The request is a saving: where it fails, or missed a
/// pair, a record asks for its key alone as it is judged.
fn ask_apart(
    (lake, w): (&Lake, &Writer),
    apart: &Apart,
    (d, forgotten): (&Decide<'_>, &BTreeMap<SourceId, String>),
    source: &SourceId,
    records: &[SourceRecord],
) {
    let mut pairs = BTreeSet::new();
    for r in records {
        let pair = pair_of(r, source);
        let seals =
            d.at.sight
                .ty(&r.ty)
                .is_ok_and(|decl| !erasable(decl, &r.fields).is_empty())
                && crate::writes::check_key(&r.key).is_ok()
                && crate::writes::check_source(&pair.0).is_ok()
                && (r.delivery.as_ref()).is_none_or(|d| crate::writes::check_delivery(d).is_ok())
                && !forgotten.contains_key(&pair.0);
        if seals && !pairs.contains(&pair) && kept((lake, w), apart, &pair).is_none() {
            pairs.insert(pair);
        }
    }
    if pairs.is_empty() {
        return;
    }
    let sources: Vec<SourceId> = pairs.iter().map(|(s, _)| s.clone()).collect();
    if let Ok(ids) = lake.keys.apart_many(&sources) {
        lakelands_core::lock(apart).extend(pairs.into_iter().zip(ids));
    }
}

/// The erasable fields of `decl` that `patch` gives a value.
fn erasable(
    decl: &lakelands_api::ontology::TypeDecl,
    patch: &lakelands_api::Patch,
) -> Vec<lakelands_api::FieldName> {
    (decl.fields.iter())
        .filter(|f| f.erasable && patch.get(&f.name).is_some_and(|v| !v.is_null()))
        .map(|f| f.name.clone())
        .collect()
}

/// The next item of a stream, if it arrives before `deadline`.
async fn next_before(
    stream: &mut SourceStream,
    deadline: Option<tokio::time::Instant>,
) -> Option<Option<SourceRecord>> {
    let next = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx));
    match deadline {
        None => Some(next.await),
        Some(d) => tokio::time::timeout_at(d, next).await.ok(),
    }
}

/// Run an ingest: windows are decided on the blocking pool as they fill,
/// and their answers sent back through the returned stream.
pub fn run(
    lake: Arc<Lake>,
    who: Principal,
    context: WriteContext,
    source: SourceId,
    opts: IngestOpts,
    mut records: SourceStream,
) -> CommitStream {
    let mut delivery = opts.delivery;
    let named = delivery.clone();
    let mut places: BTreeMap<Option<DeliveryId>, u64> = BTreeMap::new();
    let apart = opts.sealed_apart.then(|| Arc::new(Apart::default()));
    let quarantine = opts.quarantine_refused;
    let run = opts.run;
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Committed>>(4);
    let lakes = lake.limits.commit_window_events;
    let size = opts.commit_window.unwrap_or(lakes).max(1) as usize;
    tokio::spawn(async move {
        let (mut ended, mut next) = (false, 0u64);
        while !ended {
            let mut batch = Vec::new();
            let mut deadline = None;
            while batch.len() < size {
                match next_before(&mut records, deadline).await {
                    Some(Some(mut r)) => {
                        // A record that names the call's own delivery is
                        // in it: there is no second one of that name.
                        if r.delivery.is_some() && r.delivery == named {
                            r.delivery = None;
                        }
                        batch.push(r);
                        deadline.get_or_insert_with(|| tokio::time::Instant::now() + WINDOW);
                    }
                    Some(None) => {
                        ended = true;
                        break;
                    }
                    None => break,
                }
            }
            if batch.is_empty() {
                continue;
            }
            let (w, s, d) = (who.clone(), source.clone(), delivery.clone());
            let (a, context, r) = (apart.clone(), context.clone(), run.clone());
            let first = next;
            next += batch.len() as u64;
            let at = (batch.iter())
                .map(|r| {
                    let place = places.entry(r.delivery.clone()).or_default();
                    *place += 1;
                    *place - 1
                })
                .collect();
            let run = move |t: &Turn<'_>| {
                let how = (context, d, a.as_deref(), (quarantine, r, None));
                crate::lake::owing(|| window(t, &w, how, &s, (first, batch, at)))
            };
            let mut answer = exec::write(&lake, run).await;
            // The calls the window made due are made before it answers, so
            // its position is one the filled fields are visible from.
            if let Ok(((c, _), own)) = &mut answer
                && let Some(last) = crate::backed::settle_after(&lake, own).await
            {
                c.position = c.position.max(last);
            }
            let answer = answer.map(|((c, _), _)| c);
            if let Ok(c) = &answer {
                delivery.clone_from(&c.delivery);
            }
            let failed = answer.is_err();
            if tx.send(answer).await.is_err() || failed {
                return;
            }
        }
    });
    Box::pin(Receiver(rx))
}

/// Quarantine the record the host refused, as a call of it alone that
/// quarantines and ingests nothing: what keeps a refusal from quarantine
/// refuses it, as the record's (085, extended).
pub async fn host(
    lake: Arc<Lake>,
    (who, context): (Principal, WriteContext),
    source: SourceId,
    opts: IngestOpts,
    refused: HostRefused,
) -> Result<Quarantining> {
    let HostRefused {
        mut record,
        cause,
        reason,
    } = refused;
    let max = HOST_REASON_MAX;
    let cause_formed = (1..=64).contains(&cause.len())
        && (cause.bytes()).all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_'));
    let reason_formed =
        !reason.is_empty() && reason.len() <= max && !reason.chars().any(char::is_control);
    if !cause_formed || !reason_formed {
        let path = if cause_formed { "reason" } else { "cause" }.to_owned();
        let said = Said::from(Case::HostRefusalForm { max });
        return Err(Error::invalid(said.within(Place::Member { path })));
    }
    let said = Box::new(Said::from(Case::RefusedByHost { cause, reason }));
    let refusal = Error::refused(None, lakelands_api::error::Refusal::Declined(said));
    if record.delivery.is_some() && record.delivery == opts.delivery {
        record.delivery = None;
    }
    let apart = opts.sealed_apart.then(Apart::default);
    let work = move |t: &Turn<'_>| {
        let how = (
            context,
            opts.delivery,
            apart.as_ref(),
            (true, opts.run, Some(refusal)),
        );
        crate::lake::owing(|| window(t, &who, how, &source, (0, vec![record], vec![0])))
    };
    let ((c, ids), own) = exec::write(&lake, work).await?;
    let last = crate::backed::settle_after(&lake, &own).await;
    Ok(Quarantining {
        id: ids
            .into_iter()
            .next()
            .expect("a record not refused is quarantined"),
        position: last.map_or(c.position, |l| c.position.max(l)),
        duplicate: c.duplicates == 1,
    })
}

/// The most bytes of the host's reason for a refusal.
const HOST_REASON_MAX: usize = 1024;

/// The answers of an ingest, as a stream.
struct Receiver(tokio::sync::mpsc::Receiver<Result<Committed>>);

impl Stream for Receiver {
    type Item = Result<Committed>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}

#[cfg(test)]
mod tests;

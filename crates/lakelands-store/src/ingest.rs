//! `ingest`: a stream of records, each from the call's source or its
//! own, in group-commit windows.
//!
//! Records are taken from the stream into a window until it holds the
//! lake's `commit_window_events` records or 2 ms have passed since its first
//! record; the window is then decided whole and appended durably, and
//! answers one [`Committed`] with the position its last event took. A
//! window is all or nothing: if one of its records is refused, nothing of
//! the window is appended, the refusal is the stream's next item and the
//! stream ends — the caller's own offset says where to resume
//! (`docs/decisions/016-two-write-doors-by-origin.md`).
//!
//! **Idempotency by content.** A record's event id is derived from its
//! source, type, key, the record's version before it and its (checked)
//! values (`docs/log-format.md` §3). A record whose id is already in the
//! log, or earlier in the same window, appends nothing and is counted as a
//! duplicate; so does a record that would change no field — a full
//! re-import of unchanged data appends nothing. At-least-once delivery is
//! therefore safe. The unchanged test comes after the door's: a record that
//! carries a field outside the source's authority is refused, not counted
//! as a duplicate. A value that returns after a change is a new write, and
//! the same key with different values moves its version on, within a
//! window too, in stream order.

use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_core::Stream;
use lakelands_api::catalog::{CommitStream, SourceStream};
use lakelands_api::error::{Case, Place, Said};
use lakelands_api::event::EventId;
use lakelands_api::principal::WriteContext;
use lakelands_api::record::Origin;
use lakelands_api::write::Committed;
use lakelands_api::{Principal, Result, SourceId, SourceRecord};
use lakelands_core::rights::Door;

use crate::exec;
use crate::lake::Lake;
use crate::log;
use crate::writes::Call;
use lakelands_core::{canonical, links};

/// How long a window waits for more records after its first.
const WINDOW: Duration = Duration::from_millis(2);

/// Decide and append one window.
pub fn window(
    lake: &Lake,
    who: &Principal,
    context: WriteContext,
    source: &SourceId,
    (first, records): (u64, Vec<SourceRecord>),
) -> Result<Committed> {
    crate::writes::check_context(lake, &context)?;
    let mut w = lake.for_write()?;
    // Under the writer, so through every rename the lake has folded, and
    // with its references as the head's types show them (024, extended).
    let who = (lake.sight(who.clone(), None, None, &w.ontology)?.0.who()).clone();
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
    let mut d = call.decide(&w, view.as_ref(), now, 0);
    (d.door, d.envelope.origin) = (Door::Source, Origin::Source);
    let sees = lakelands_core::sight::Sight::held(who, &w.ontology);
    let mut seen: BTreeSet<EventId> = BTreeSet::new();
    let mut drafts = Vec::new();
    let mut committed = 0u32;
    let mut window_links = Vec::new();
    let mut duplicates = 0u32;
    // The window's records as the projection holds them, read a type at a
    // time rather than one by one; a key the window names twice is read
    // once, and its second record still reads the first as planned. Only
    // the records up to the first whose type or key the loop refuses are
    // read, so a key no engine can hold is refused, not sent.
    let mut keys: std::collections::BTreeMap<&lakelands_api::TypeName, Vec<_>> = Default::default();
    let formed =
        |r: &&SourceRecord| sees.ty(&r.ty).is_ok() && crate::writes::check_key(&r.key).is_ok();
    for r in records.iter().take_while(formed) {
        keys.entry(&r.ty).or_default().push(r.key.clone());
    }
    for (ty, keys) in keys {
        d.pending.prime(view.as_ref(), ty, &keys)?;
    }
    // The keys of an insert-only type this window writes, each once.
    let mut once = BTreeSet::new();
    for (i, r) in records.iter().enumerate() {
        let judged = (|| {
            let decl = sees.ty(&r.ty)?;
            // The key's form before the door, as `upsert` checks it. Then the
            // door: a patch that carries a field the source is not the
            // authority on is refused whole, even when that field's value is
            // the one already stored.
            let patch = d.checked(decl, &r.key, &r.fields)?;
            if let Some(own) = &r.source {
                crate::writes::check_source(own)?;
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
                return Err(lakelands_core::invalid(Case::UnclaimedLinkType { ty }));
            }
            let other = t.own().is_none() && t.before.is_some();
            let version_before = t.before.as_ref().map_or(0, |b| b.version.0);
            // The record's own source, else the call's: its event and its id.
            let source = r.source.as_ref().unwrap_or(source);
            d.envelope.source = Some(source.clone());
            let id = log::ingest_id(source, &r.ty, &r.key, version_before, &patch);
            let unchanged = t.live().is_some_and(|b| {
                patch.iter().all(|(f, v)| match b.fields.get(f) {
                    Some(stored) => stored == v,
                    None => v.is_null(),
                })
            });
            if unchanged || !other && (seen.contains(&id) || view.applied(&id)?.is_some()) {
                duplicates += 1;
                return Ok(());
            }
            if patch.is_empty() {
                return Err(lakelands_core::invalid(Case::PatchEmpty));
            }
            // An insert-only type's key is written once: a record stands
            // at it, live or retired, or this window wrote it. Another's
            // record and a held one are judged as before, so the mark
            // tells nothing of them (lakelands#1044).
            let stands = t.before.is_some() && !other;
            if decl.insert_only && (stands || !once.insert((r.ty.clone(), r.key.clone()))) {
                let (ty, key) = (r.ty.clone(), r.key.clone());
                return Err(crate::writes::constraint(
                    None,
                    Case::WrittenOnce { ty, key },
                ));
            }
            // The record is applied as the source states it, so a guard reads
            // the `seen_at` the fold keeps.
            let stated = (Some(id.clone()), r.seen_at, r.upstream.clone());
            (d.envelope.id, d.envelope.seen_at, d.envelope.upstream) = stated;
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
                        view.as_ref(),
                        &sees,
                        &t.name,
                        relation,
                        &from,
                        &to,
                    )?;
                    crate::writes::cardinality_among(&window_links, &r.ty, relation, &from, &to)?;
                    window_links.push((r.ty.clone(), from, to));
                }
            }
            drafts.extend(d.planned(plan)?.drafts);
            seen.insert(id);
            committed += 1;
            Ok(())
        })();
        // A record refused names itself and where its window began, which
        // nothing from was written (016, lakelands#1043); an outage does not.
        if let Err(e) = judged {
            return Err(ingested(e, first + i as u64, first, r));
        }
    }
    let reads = d.pending.into_reads();
    drop(view);
    // A window that appends nothing committed no record, and answers the head.
    let position = if drafts.is_empty() {
        w.tip
            .as_ref()
            .map_or(lakelands_api::Position::GENESIS, |t| t.0)
    } else {
        let events = lake.append_after(&mut w, drafts, reads)?;
        events.last().expect("appended").position
    };
    Ok(Committed {
        position,
        records: committed,
        duplicates,
    })
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
    }
}

/// What a source states of a record beside its fields is written as it
/// was given, so it is judged as a written value is
/// (`docs/log-format.md` §1.2, §1.3): a `seen_at` out of the years 0000 to
/// 9999, which an instant built in process can be, and an upstream
/// position beyond ±(2^53 − 1) are refused, each naming its member.
fn stated(r: &SourceRecord) -> Result<()> {
    let at = |path: &str, case: Case| {
        let path = path.to_owned();
        Err(lakelands_core::invalid(
            Said::from(case).within(Place::Member { path }),
        ))
    };
    if let Some(t) = r.seen_at.filter(|t| !t.in_years()) {
        let value = t.to_string();
        return at("seen_at", Case::NotDateTime { value });
    }
    let position = r.upstream.as_ref().map(|u| u.position.0.into());
    match position.as_ref().and_then(canonical::integer_too_large) {
        Some(n) => at("upstream.position", Case::IntegerTooLarge { n: n.clone() }),
        None => Ok(()),
    }
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
    mut records: SourceStream,
) -> CommitStream {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Committed>>(4);
    let size = lake.limits.commit_window_events.max(1) as usize;
    tokio::spawn(async move {
        let (mut ended, mut next) = (false, 0u64);
        while !ended {
            let mut batch = Vec::new();
            let mut deadline = None;
            while batch.len() < size {
                match next_before(&mut records, deadline).await {
                    Some(Some(r)) => {
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
            let (l, w, s) = (lake.clone(), who.clone(), source.clone());
            let first = next;
            next += batch.len() as u64;
            let run = move || crate::lake::owing(|| window(&l, &w, context, &s, (first, batch)));
            let mut answer = exec::run(run).await;
            // The calls the window made due are made before it answers, so
            // its position is one the filled fields are visible from.
            if let Ok((c, own)) = &mut answer
                && let Some(last) = crate::backed::settle_after(&lake, own).await
            {
                c.position = c.position.max(last);
            }
            let answer = answer.map(|(c, _)| c);
            let failed = answer.is_err();
            if tx.send(answer).await.is_err() || failed {
                return;
            }
        }
    });
    Box::pin(Receiver(rx))
}

/// The answers of an ingest, as a stream.
struct Receiver(tokio::sync::mpsc::Receiver<Result<Committed>>);

impl Stream for Receiver {
    type Item = Result<Committed>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}

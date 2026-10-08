//! `ingest`: a source's stream of records, in group-commit windows.
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
use lakelands_api::error::Case;
use lakelands_api::event::EventId;
use lakelands_api::principal::WriteContext;
use lakelands_api::write::Committed;
use lakelands_api::{Principal, Result, SourceId, SourceRecord};
use lakelands_core::rights::Door;

use crate::exec;
use crate::lake::Lake;
use crate::log;
use crate::writes::{Call, Pending};
use lakelands_core::links;

/// How long a window waits for more records after its first.
const WINDOW: Duration = Duration::from_millis(2);

/// Decide and append one window.
pub fn window(
    lake: &Lake,
    who: &Principal,
    context: WriteContext,
    source: &SourceId,
    records: Vec<SourceRecord>,
) -> Result<Committed> {
    crate::writes::check_context(lake, &context)?;
    let mut w = lake.for_write()?;
    // Under the writer, so through every rename the lake has folded, and
    // with its references as the head's types show them (024, extended).
    let (who, _) = lake.followed(who.clone(), None, None)?;
    let (who, _) = Lake::hidden(who, &w.ontology);
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
    let mut pending = Pending::default();
    let mut seen: BTreeSet<EventId> = BTreeSet::new();
    let mut drafts = Vec::new();
    let mut committed = 0u32;
    let mut window_links = Vec::new();
    let mut duplicates = 0u32;
    for r in records {
        let decl = crate::writes::shown_type(&w.ontology, &r.ty, who)?;
        // The key's form before the door, as `upsert` checks it.
        crate::writes::check_key(&r.key)?;
        // Then the door: a patch that carries a field the source is not
        // the authority on is refused whole, even when that field's value
        // is the one already stored.
        lakelands_core::rights::check_write(who, decl, Door::Source, &r.fields)?;
        let patch = crate::writes::check_values(&w.ontology, decl, &r.fields, &decl.fields)?;
        let before = pending.record(view.as_ref(), &r.ty, &r.key)?;
        // A record a holder of `only` is not admitted is, to it, a key that
        // is free: it is not compared, and `plan_record` refuses the write
        // as on a free key with another's owner (051).
        let own = crate::writes::owned(view.as_ref(), &pending, who, decl, before.clone())?;
        // A link type no relation carries, its relation renamed or
        // removed, stays declared for its history and takes no link.
        let types = w.ontology.types.values().map(|e| &e.decl);
        let relation = (links::is_link_type(&r.ty)).then(|| links::relation_of(types, &r.ty));
        if let Some(None) = relation {
            let ty = r.ty.clone();
            return Err(lakelands_core::invalid(Case::UnclaimedLinkType { ty }));
        }
        let other = own.is_none() && before.is_some();
        let version_before = before.as_ref().map_or(0, |b| b.version.0);
        let id = log::ingest_id(source, &r.ty, &r.key, version_before, &patch);
        let unchanged = own.as_ref().filter(|b| !b.retired).is_some_and(|b| {
            patch.iter().all(|(f, v)| match b.fields.get(f) {
                Some(stored) => stored == v,
                None => v.is_null(),
            })
        });
        if unchanged || !other && (seen.contains(&id) || view.applied(&id)?.is_some()) {
            duplicates += 1;
            continue;
        }
        let mut plan = call.plan_record(
            &w,
            view.as_ref(),
            &pending,
            &r.ty,
            &r.key,
            &patch,
            Door::Source,
            None,
            now,
            drafts.len(),
            who,
            &|_| {},
        )?;
        // A link holds its relation's cardinality, over the lake's live
        // links and those earlier in this window: judged once
        // `plan_record` admitted the record its writer leaves, as the
        // mock judges it, so that it names nothing of another's record
        // and passes over no link a holder's `only` admits (051).
        if let Some(Some((t, relation))) = relation {
            // The ends as the record holds them: the key rule fills
            // those the patch leaves out.
            let end = |f: &str| {
                let end = plan.after.fields.get(f).and_then(|v| v.as_str());
                end.map(lakelands_api::RecordKey::from)
            };
            if let (Some(from), Some(to)) = (end(links::FROM), end(links::TO)) {
                crate::writes::cardinality(view.as_ref(), who, &t.name, relation, &from, &to)?;
                crate::writes::cardinality_among(&window_links, &r.ty, relation, &from, &to)?;
                window_links.push((r.ty.clone(), from, to));
            }
        }
        plan.draft.id = Some(id.clone());
        plan.draft.source = Some(source.clone());
        plan.draft.seen_at = r.seen_at;
        plan.draft.upstream = r.upstream.clone();
        let at = crate::writes::position_after(&w, drafts.len());
        drafts.extend(call.planned(&w, view.as_ref(), &mut pending, plan, at, now)?);
        seen.insert(id);
        committed += 1;
    }
    drop(view);
    // A window that appends nothing committed no record, and answers the head.
    let position = if drafts.is_empty() {
        w.tip
            .as_ref()
            .map_or(lakelands_api::Position::GENESIS, |t| t.0)
    } else {
        let events = lake.append_after(&mut w, drafts, pending.into_reads())?;
        events.last().expect("appended").position
    };
    Ok(Committed {
        position,
        records: committed,
        duplicates,
    })
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
        let mut ended = false;
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
            let run = move || crate::lake::owing(|| window(&l, &w, context, &s, batch));
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

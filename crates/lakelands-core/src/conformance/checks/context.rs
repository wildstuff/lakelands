//! An event records the principal a write was made for, beside its writer
//! (`docs/decisions/088-an-event-records-the-principal-a-write-was-made-for.md`).
//!
//! A host binds a catalog to a context whose `on_behalf_of` names whom
//! the write is for. Each event written as the writer carries it in its
//! envelope, never in `context`, and each field it writes names it beside
//! the writer; an event the store writes as itself names none.
//!
//! The host may also name the write's `occasion`, its word for why it
//! wrote. Each event written as the writer carries it in `context`, as
//! given; an event the store writes as itself carries none, and an event
//! of a write that names none is written as before the member was.

use lakelands_api::principal::{OCCASION_MAX_BYTES, WriteContext};

use crate::conformance::fixture::{ingest, source_record, type_of, upsert};
use crate::conformance::prelude::*;

check!(
    ON_BEHALF_OF_RECORDED = a_write_made_for_a_person_names_them_beside_its_writer,
    "088 (lakelands#1489): a catalog of `service:mailer` bound for `human:alice` upserts, links, calls a verb that writes and ingests one record; each event written as `service:mailer` names `human:alice` in its envelope's `on_behalf_of`, none carries it in `context`, the `trigger_fired` and `object_set_changed` they cause name none, and a read with meta names `service:mailer` and `human:alice` on each field written; a rebuild and a reopen answer the same",
    given
);

check!(
    ON_BEHALF_OF_FORM = an_on_behalf_of_not_of_its_form_is_refused,
    "088 (lakelands#1489): a context naming `on_behalf_of` as `system:x`, `robot:x`, `human:`, an id holding a NUL, or the writer `service:mailer` itself, is refused invalid as `on_behalf_of_form` at the member, naming what it holds, appending nothing"
);

check!(
    OCCASION_RECORDED = an_ingest_under_an_occasion_records_it_in_each_events_context,
    "088, extended (lakelands#1584): `service:mailer` ingests two records of `reading`, on which a trigger fires, under the occasion `caught-up`; each `record_ingested` the feed hands a reader carries `caught-up` in its `context`, the `trigger_fired` they set off carries none, and an ingest naming none appends a `record_ingested` whose `context` holds no `occasion`, its hash the one its canonical bytes give"
);

check!(
    OCCASION_FORM = an_occasion_not_of_its_form_is_refused,
    "088, extended (lakelands#1584): an ingest under an occasion that is empty, over 128 bytes, or holds a line break, a NUL or DEL, is refused invalid as `occasion_form` at `occasion`, appending nothing"
);

/// The mailer, writing what it is asked to.
fn mailer(store: &dyn LakeStore) -> Principal {
    principal(store, PrincipalKind::Service, "mailer", Grants::owner())
}

/// A direct context made for `whom`.
pub(super) fn made_for(whom: &str) -> WriteContext {
    WriteContext {
        on_behalf_of: Some(whom.into()),
        ..WriteContext::default()
    }
}

/// `p9` and `r1`, with their provenance, as one JSON text.
async fn written(l: &Lake) -> Result<String, Failure> {
    let records = [
        l.read("partner", "p9").await?,
        l.read("reading", "r1").await?,
    ];
    Ok(serde_json::to_string(&records).expect("records serialise"))
}

async fn a_write_made_for_a_person_names_them_beside_its_writer(
    store: Store,
    given: Given,
) -> Held {
    let l = super::links::lake(store.as_ref()).await?;
    for d in [
        Declaration::Type(type_decl(
            "reading",
            vec![sourced("value", StorageKind::Number)],
        )),
        trigger_on("noted", "partner", None),
        object_set_on("renamed", "partner", "name = 'renamed'"),
        write_verb("rename", "partner", "name", "'renamed'"),
    ] {
        l.cat.declare(&l.own, d).await.must("declaring the lake")?;
    }
    let (mailer, alice) = (mailer(store.as_ref()), "human:alice");
    let cat = store.catalog(made_for(alice));
    let before = store.head().await.must("reading the head")?;
    let p9 = json!({"name": "p9"});
    upsert(cat.as_ref(), &mailer, "partner", "p9", p9).await?;
    let linked = super::links::link(cat.as_ref(), &mailer, "i1", "p9", json!({})).await;
    linked.must("linking `i1` to `p9` for alice")?;
    let (verb, key, opts) = ("rename".into(), "p9".into(), WriteOpts::default());
    let called = cat
        .call(&mailer, &verb, Some(&key), patch(json!({})), opts)
        .await;
    called.must("calling `rename` on `p9` for alice")?;
    let fed = vec![source_record("reading", "r1", json!({"value": 7}))];
    ingest(cat.as_ref(), &mailer, "meter", fed)
        .await
        .must("ingesting `r1` for alice")?;
    let events = events_after(store.as_ref(), before).await?;
    let mut derived = (false, false);
    for e in &events {
        let kind = e.body.kind();
        ensure!(
            e.context.on_behalf_of.is_none(),
            "no event's `context` carries `on_behalf_of`; the `{kind}` at {} does",
            e.position
        );
        let want = match e.writer.as_str() {
            "service:mailer" => Some(alice),
            _ => None,
        };
        ensure!(
            e.on_behalf_of.as_deref() == want,
            "the `{kind}` at {} written as `{}` names {want:?} as whom it was made for; it \
             names {:?}",
            e.position,
            e.writer,
            e.on_behalf_of
        );
        derived.0 |= matches!(e.body, EventBody::TriggerFired { .. });
        derived.1 |= matches!(e.body, EventBody::ObjectSetChanged { .. });
    }
    let kinds: Vec<_> = (events.iter())
        .filter(|e| e.writer == "service:mailer")
        .map(|e| e.body.kind())
        .collect();
    let wrote = ["record_upserted", "verb_called", "record_ingested"]
        .iter()
        .all(|k| kinds.contains(k));
    ensure!(
        wrote && derived == (true, true),
        "the mailer's writes append its upserts, the link among them, its `verb_called` and its ingest, \
         and set off a `trigger_fired` and an `object_set_changed`: {kinds:?}, {derived:?}"
    );
    for (ty, key, field) in [("partner", "p9", "name"), ("reading", "r1", "value")] {
        let r = l.read(ty, key).await?;
        let meta = r.meta.get(field);
        ensure!(
            meta.is_some_and(
                |m| m.writer == "service:mailer" && m.on_behalf_of.as_deref() == Some(alice)
            ),
            "`{ty}` `{key}`'s `{field}` names `service:mailer` and `{alice}` in its \
             provenance: {meta:?}"
        );
    }
    let seen = written(&l).await?;
    store.rebuild().await.must("rebuilding the lake")?;
    let rebuilt = written(&l).await?;
    let store = (given.reopen)(store, Default::default()).await;
    let store = store.map_err(|e| Failure::new(format!("reopening the lake: {e}")))?;
    let reopened = written(&Lake::of(store.as_ref())).await?;
    ensure!(
        rebuilt == seen && reopened == seen,
        "a rebuild and a reopen answer the records as written ({seen}): {rebuilt} / {reopened}"
    );
    Ok(())
}

async fn an_on_behalf_of_not_of_its_form_is_refused(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_of("note", &[("body", StorageKind::Text)]))
        .await?;
    let mailer = mailer(store.as_ref());
    let before = store.head().await.must("reading the head")?;
    for whom in [
        "system:x",
        "robot:x",
        "human:",
        "human:a\0b",
        "service:mailer",
    ] {
        let cat = store.catalog(made_for(whom));
        let said = try_upsert(cat.as_ref(), &mailer, "note", "n1", json!({"body": "x"})).await;
        let at = match &said {
            Err(Error::Refused { field, .. }) => field.as_ref().map(|f| f.as_str()),
            _ => None,
        };
        let case = invalid_case(&said);
        ensure!(
            at == Some("on_behalf_of")
                && matches!(case, Some(Case::OnBehalfOfForm { value }) if value == whom),
            "a write made for {whom:?} is refused invalid as `on_behalf_of_form` at \
             `on_behalf_of`, naming what it holds: {said:?}"
        );
    }
    head_unmoved(store.as_ref(), before, "made for none of its form").await
}

/// A direct context naming `occasion`.
fn on_occasion(occasion: &str) -> WriteContext {
    WriteContext {
        occasion: Some(occasion.into()),
        ..WriteContext::default()
    }
}

async fn an_ingest_under_an_occasion_records_it_in_each_events_context(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let reading = type_decl("reading", vec![sourced("value", StorageKind::Number)]);
    for d in [
        Declaration::Type(reading),
        trigger_on("noted", "reading", None),
    ] {
        l.cat.declare(&l.own, d).await.must("declaring the lake")?;
    }
    let mailer = mailer(store.as_ref());
    let before = store.head().await.must("reading the head")?;
    let cat = store.catalog(on_occasion("caught-up"));
    let fed = vec![
        source_record("reading", "r1", json!({"value": 1})),
        source_record("reading", "r2", json!({"value": 2})),
    ];
    let ingested = ingest(cat.as_ref(), &mailer, "meter", fed).await;
    ingested.must("ingesting `r1` and `r2` as caught up")?;
    let events = events_after(store.as_ref(), before).await?;
    let mut seen = (0, 0);
    for e in &events {
        let (kind, said) = (e.body.kind(), e.context.occasion.as_deref());
        let want = match e.body {
            EventBody::RecordIngested { .. } => Some("caught-up"),
            _ => None,
        };
        ensure!(
            said == want,
            "the `{kind}` at {}, written as `{}`, carries {want:?} as its occasion; it \
             carries {said:?}",
            e.position,
            e.writer
        );
        seen.0 += usize::from(matches!(e.body, EventBody::RecordIngested { .. }));
        seen.1 += usize::from(matches!(e.body, EventBody::TriggerFired { .. }));
    }
    ensure!(
        seen == (2, 2),
        "the ingest appends two `record_ingested` and sets off two `trigger_fired`: {seen:?}"
    );
    let before = store.head().await.must("reading the head")?;
    let fed = vec![source_record("reading", "r3", json!({"value": 3}))];
    let plain = store.catalog(WriteContext::default());
    ingest(plain.as_ref(), &mailer, "meter", fed)
        .await
        .must("ingesting `r3` naming no occasion")?;
    let events = events_after(store.as_ref(), before).await?;
    let e = (events.iter())
        .find(|e| matches!(e.body, EventBody::RecordIngested { .. }))
        .ok_or_else(|| Failure::new("the ingest of `r3` appends a `record_ingested`"))?;
    let line = serde_json::to_value(e).expect("an event serialises");
    ensure!(
        line["context"].get("occasion").is_none()
            && crate::engines::hash_of(line.clone()) == e.hash.0,
        "an ingest naming no occasion appends a `record_ingested` whose `context` holds no \
         `occasion` and whose hash is its canonical bytes': {line}"
    );
    Ok(())
}

async fn an_occasion_not_of_its_form_is_refused(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl(
        "reading",
        vec![sourced("value", StorageKind::Number)],
    ))
    .await?;
    let mailer = mailer(store.as_ref());
    let before = store.head().await.must("reading the head")?;
    let long = "a".repeat(OCCASION_MAX_BYTES + 1);
    for occasion in ["", &long, "live\nagain", "live\0", "live\u{7f}"] {
        let cat = store.catalog(on_occasion(occasion));
        let fed = vec![source_record("reading", "r1", json!({"value": 1}))];
        let said = ingest(cat.as_ref(), &mailer, "meter", fed).await;
        let at = match &said {
            Err(Error::Refused { field, .. }) => field.as_ref().map(|f| f.as_str()),
            _ => None,
        };
        let case = invalid_case(&said);
        ensure!(
            at == Some("occasion")
                && matches!(case, Some(Case::OccasionForm { max }) if *max == OCCASION_MAX_BYTES),
            "an ingest under the occasion {occasion:?} is refused invalid as `occasion_form` \
             at `occasion`: {said:?}"
        );
    }
    head_unmoved(store.as_ref(), before, "under no occasion of its form").await
}

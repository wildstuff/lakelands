//! An event records the principal a write was made for, beside its writer
//! (`docs/decisions/088-an-event-records-the-principal-a-write-was-made-for.md`).
//!
//! A host binds a catalog to a context whose `on_behalf_of` names whom
//! the write is for. Each event written as the writer carries it in its
//! envelope, never in `context`, and each field it writes names it beside
//! the writer; an event the store writes as itself names none.

use lakelands_api::principal::WriteContext;

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
        "the mailer's writes append its upsert, its link, its `verb_called` and its ingest, \
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

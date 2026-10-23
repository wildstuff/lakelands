//! A projection built ahead (071, lakelands#1322): the next release
//! builds the projection it lays out beside a lake an older release still
//! serves, from the log alone, and its writer's open of the layout before
//! goes on from that build and swaps it in.

use std::sync::Arc;

use lakelands_api::Location;
use lakelands_api::engine::{CanonicalEvent, EngineOpener, LakeEngine, OpenMode};

use super::erasable::{Keys, person, with};
use crate::conformance::fixture::{doors, owner, source_record};
use crate::conformance::prelude::*;

check!(
    BUILT_AHEAD = a_lake_built_ahead_then_written_answers_as_rebuilt,
    "071 (lakelands#1322): a lake's projection is built ahead beside the writer that holds it, through its head, while that writer, a release that keeps no scratch, goes on: it forgets a source and purges what another brought into a type after the build, and the writer's open of the layout before goes on from the build ahead, swaps it in and answers every record with its provenance as a full rebuild of the same log does, the forgotten source's values and the purged records gone; a build ahead resumed past a removal of a source's values folds the whole log again, told the act, and the open that swaps it in answers as a full rebuild, the removed values gone; on an engine that keeps no scratch the build ahead is refused as unbound and nothing is written",
    given
);

/// The harness's engine opener as a release that keeps no scratch has it:
/// its writer rebuilds in place, and never meets a build ahead.
struct Older(Arc<dyn EngineOpener>);

impl EngineOpener for Older {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn log_ends(
        &self,
        at: &Location,
    ) -> lakelands_api::Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        self.0.log_ends(at)
    }
    fn open(&self, at: &Location, mode: OpenMode) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        self.0.open(at, mode)
    }
}

/// Every `person` and `item` the lake holds, with its provenance.
async fn answers(store: &dyn LakeStore) -> Result<Value, Failure> {
    let (own, cat) = (owner(store), doors(store));
    let mut out = serde_json::Map::new();
    for ty in ["person", "item"] {
        let records = all(cat.as_ref(), &own, ty).await?;
        let records: Vec<Value> = (records.iter())
            .map(|r| json!({"key": r.key, "fields": r.fields, "meta": r.meta}))
            .collect();
        out.insert(ty.to_owned(), json!(records));
    }
    Ok(Value::Object(out))
}

/// The lake answers `want` of its people and items, and the same after a
/// full rebuild of its log.
async fn as_rebuilt(store: &dyn LakeStore, want: Value, when: &str) -> Held {
    let swapped = answers(store).await?;
    store.rebuild().await.must("rebuilding the lake")?;
    let rebuilt = answers(store).await?;
    let (people, items) = (&swapped["person"], &swapped["item"]);
    let kept = |list: &Value| -> Vec<Value> {
        (list.as_array().into_iter().flatten())
            .map(|r| json!([r["key"], r["fields"]]))
            .collect()
    };
    ensure!(
        swapped == rebuilt && json!([kept(people), kept(items)]) == want,
        "{when}, the swapped projection answers {want} and as a full rebuild does: it answered \
         {swapped}, and rebuilt {rebuilt}"
    );
    Ok(())
}

/// The writer's open of the lake marked one layout back: it goes on from
/// the build ahead, saying so.
async fn upgraded(
    store: Store,
    given: &Given,
    location: &Location,
    seams: Seams,
) -> Result<Store, Failure> {
    ensure!(
        (given.lower)(location),
        "the harness marks the lake's projection one layout back"
    );
    let store = (given.reopen)(store, seams).await;
    let store = store.map_err(|e| Failure::new(format!("the upgrade's open: {e}")))?;
    let why = store.rebuilt_at_open();
    ensure!(
        why.as_ref().is_some_and(|w| w.contains("build ahead")),
        "the writer's open of the layout before goes on from the build ahead: {why:?}"
    );
    Ok(store)
}

async fn a_lake_built_ahead_then_written_answers_as_rebuilt(store: Store, given: Given) -> Held {
    let keys = Arc::new(Keys::default());
    let seams = || with(&keys);
    let older: crate::conformance::Wrap =
        Arc::new(|inner| Arc::new(Older(inner)) as Arc<dyn EngineOpener>);
    let older = |store: Store| {
        let open = (given.fault)(store, seams(), older.clone());
        async move {
            open.await
                .map_err(|e| Failure::new(format!("the older release's open: {e}")))
        }
    };
    let Some(location) = given.location.clone() else {
        return Err(Failure::new("the harness names no location of the lake"));
    };
    let store = older(store).await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    l.declare(type_decl("item", vec![sourced("label", StorageKind::Text)]))
        .await?;
    let p = |k: &str, name: &str| {
        source_record("person", k, json!({"id": k, "name": name, "city": "Oslo"}))
    };
    let item = |k: &str| source_record("item", k, json!({"label": k}));
    l.ingest("crm", vec![p("p1", "Ada"), p("p2", "Bob")])
        .await
        .must("ingesting from crm")?;
    l.upsert("person", "p2", json!({"note": "kept"})).await?;
    l.ingest("erp", vec![p("p3", "Cy")])
        .await
        .must("ingesting from erp")?;
    l.ingest("web", vec![item("i1"), item("i2")])
        .await
        .must("ingesting from web")?;
    let head = store.head().await.must("reading the head")?;
    let built = (given.ahead)(location.clone(), seams()).await;
    if let Err(Error::Unbound { .. }) = &built {
        let after = store.head().await.must("reading the head")?;
        ensure!(
            after == head,
            "a build ahead refused as unbound writes nothing: the head moved from {head:?} to \
             {after:?}"
        );
        return Ok(());
    }
    let built = built.must("building the projection ahead beside the writer")?;
    ensure!(
        built.ahead.is_none() && built.through == Some(head) && built.events == head.0 + 1,
        "the first build ahead folds the whole log, through the head {head:?}: {built:?}"
    );
    // The older release acts after the build.
    store
        .forget_source(&"crm".into(), None)
        .await
        .must("forgetting crm")?;
    let items = TypeName::from("item");
    (store.purge_source(&"web".into(), Some(&items), None).await).must("purging web's items")?;
    l.ingest("web", vec![item("i3")])
        .await
        .must("ingesting from web again")?;
    drop(l);
    let store = upgraded(store, &given, &location, seams()).await?;
    let p2 = json!(["p2", {"id": "p2", "city": "Oslo", "note": "kept"}]);
    let p3 = json!(["p3", {"id": "p3", "name": "Cy", "city": "Oslo"}]);
    let i3 = json!(["i3", {"label": "i3"}]);
    let want = json!([[p2, p3], [i3]]);
    as_rebuilt(store.as_ref(), want, "with acts after the build ahead").await?;
    // A removal before the build ahead resumes: it folds the whole log.
    let store = older(store).await?;
    let first = (given.ahead)(location.clone(), seams()).await;
    first.must("building the projection ahead again")?;
    let all_of = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    let removed = store
        .remove_values(&"erp".into(), &all_of, false, None)
        .await;
    removed.must("removing erp's values")?;
    let head = store.head().await.must("reading the head")?;
    let built = (given.ahead)(location.clone(), seams()).await;
    let built = built.must("building ahead past the removal")?;
    ensure!(
        built.ahead.is_none() && built.through == Some(head) && built.events > head.0,
        "a build ahead resumed past a removal folds the whole log again, through the head \
         {head:?}: {built:?}"
    );
    Lake::of(store.as_ref())
        .upsert("person", "p2", json!({"note": "after"}))
        .await?;
    let store = upgraded(store, &given, &location, seams()).await?;
    let p2 = json!(["p2", {"id": "p2", "city": "Oslo", "note": "after"}]);
    let want = json!([[p2], [i3]]);
    as_rebuilt(
        store.as_ref(),
        want,
        "with a removal before the build ahead",
    )
    .await
}

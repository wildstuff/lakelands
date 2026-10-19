//! A source is forgotten in one act
//! (`docs/decisions/058-a-source-may-be-forgotten.md`): its keys are
//! destroyed, the projection is rebuilt, and every value it sealed reads
//! as erased at every position, what was computed from it included. A
//! crash between the act's steps leaves the rest to the next writer's
//! open. The checks bring the key store of [`super::erasable`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use lakelands_api::engine::{
    CanonicalEvent, EngineOpener, Lease, OpenMode, ProjectionStore, ProjectionView, Ranked,
    Rankings, VectorIndex,
};
use lakelands_api::event::{Digest, Event, EventBody, SegmentDigest};
use lakelands_api::id::KeyId;
use lakelands_api::ontology::{FieldDecl, StorageKind};
use lakelands_api::read::Found;
use lakelands_api::seam::{DataKey, KeyStore, Seams};
use lakelands_api::write::ForgetReport;
use lakelands_api::{Error, LakeId, LakeStore, Location, Position, SourceId, TypeName};
use serde_json::{Value, json};

use super::erasable::{Keys, again, person, read, with};
use crate::conformance::fixture::{
    Lake, Must, events_after, find_at, get_at, refused_like, source_record, sourced,
};
use crate::conformance::{Failure, Held, Reopen, Store};

check!(
    FORGOTTEN = a_forgotten_sources_values_read_as_erased_at_every_position,
    "058: `forget_preflight` names the records holding a source's erasable values, the values per field, the records it retires and those another hand co-owns, what is computed again, no plaintext, and the time it expects to hold the writer's lock; `forget_source` with that report appends `source_forgotten` naming the key's version and retires the records the source alone brought, destroys its keys and reclaims the storage; after it the source's values read as erased, with provenance saying `erased`, at the head and `as_of` before the act, and so do a derived field, a function's recorded output and a kept view's group computed from them, while a field that is not erasable keeps its history and another source's values stay; a rebuild answers the same; an ingest from the source is refused and appends nothing, as is a stale report, and forgetting it again answers it done and appends nothing",
    reopens
);

check!(
    FORGET_CRASH = a_forgetting_a_crash_stopped_is_finished_by_the_next_writers_open,
    "058: a forgetting stopped after its event, before its keys are destroyed, and one stopped after its keys are destroyed, before its rebuild, each leave the source's ingest refused; the first is finished by the act's repeat on the same handle, the second by the writer's next open, each destroying the keys, rebuilding and reclaiming, appending no second event, and the values read as erased at every position; an open after that changes nothing",
    reopens
);

check!(
    FORGET_PLAINTEXT = a_source_with_plaintext_values_in_the_log_is_not_forgotten,
    "058: a source's value written before its field was erasable is plaintext in the log; the preflight counts it, and `forget_source` is refused naming the count, appending nothing; so is a value written under a field's old name, before a migration renamed it to a field that is erasable now",
    reopens
);

check!(
    FORGET_IMPORT = an_export_imports_with_its_keys_or_with_its_forgetting,
    "058: an export holds a source's values sealed; imported with no key store it is refused as unbound, and with a key store that lacks the key as missing, each before anything is written; imported with the key store it reads them plain; once the source is forgotten, its export imports with no key store, and the values arrive erased, provenance saying so; imported with a backup of the key store from before the act, the import finishes the act: the backup's keys are destroyed, and the first read answers the values erased",
    given
);

/// A key store over [`Keys`] that fails on cue: `destroy`, or a key of
/// `erp` once a destroy has run, as a crash would stop the act there.
struct Faulty {
    keys: Arc<Keys>,
    destroy_fails: AtomicBool,
    after_destroy_fails: AtomicBool,
    destroyed: AtomicBool,
}

impl Faulty {
    fn new(keys: &Arc<Keys>) -> Faulty {
        Faulty {
            keys: keys.clone(),
            destroy_fails: AtomicBool::new(false),
            after_destroy_fails: AtomicBool::new(false),
            destroyed: AtomicBool::new(false),
        }
    }
}

fn crashed() -> Error {
    let lake = "the key store".to_owned();
    Error::unavailable(lakelands_api::error::Situation::Busy { lake })
}

impl KeyStore for Faulty {
    fn create(&self, lake: &LakeId, source: &SourceId) -> lakelands_api::Result<KeyId> {
        self.keys.create(lake, source)
    }
    fn key(
        &self,
        lake: &LakeId,
        source: &SourceId,
        id: &KeyId,
    ) -> lakelands_api::Result<Option<DataKey>> {
        let late = self.after_destroy_fails.load(Ordering::SeqCst);
        if late && self.destroyed.load(Ordering::SeqCst) && source.as_str() == "erp" {
            return Err(crashed());
        }
        self.keys.key(lake, source, id)
    }
    fn destroy(&self, lake: &LakeId, source: &SourceId) -> lakelands_api::Result<()> {
        if self.destroy_fails.load(Ordering::SeqCst) {
            return Err(crashed());
        }
        self.keys.destroy(lake, source)?;
        self.destroyed.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn ids(&self, lake: &LakeId, source: &SourceId) -> lakelands_api::Result<Vec<KeyId>> {
        self.keys.ids(lake, source)
    }
}

/// `person` with `initial`, derived from `name`, and `loud`, which the
/// suite's backing computes from it; the kept view `by_name` counts the
/// people of each name.
async fn lake(l: &Lake) -> Held {
    let kind = crate::conformance::backing::KIND;
    let text = json!({"kind": "text"});
    let config = json!({"token": "forgotten", "echo": "name"});
    l.declare_json(
        json!({"declare": "function", "name": "shout", "backing": kind,
        "inputs": [{"name": "name", "type": text}], "config": config}),
    )
    .await?;
    let mut t = person();
    let initial =
        json!({"name": "initial", "type": text, "authority": "derived", "derived": "name"});
    t.fields
        .push(serde_json::from_value(initial).expect("a field reads"));
    t.fields.push(crate::conformance::fixture::computed_by(
        "loud",
        StorageKind::Text,
        "shout",
        &["name"],
    ));
    l.declare(t).await?;
    let view = json!({"source": "person", "group_by": ["name"],
                      "measures": [{"name": "n", "expr": "count()"}]});
    l.declare_json(
        json!({"declare": "type", "name": "by_name", "key": ["name"], "view": view,
        "fields": [{"name": "name", "type": text}, {"name": "n", "type": {"kind": "number"}}]}),
    )
    .await?;
    l.set("view_kept.by_name", json!(true)).await?;
    Ok(())
}

/// `p1` from `crm` alone, `p2` from `crm` with an author's `note`, `p3`
/// from `erp`: the position after.
async fn people(store: &dyn LakeStore, l: &Lake) -> Result<Position, Failure> {
    let ada = source_record(
        "person",
        "p1",
        json!({"id": "p1", "name": "Ada", "city": "Rome"}),
    );
    let bob = source_record("person", "p2", json!({"id": "p2", "name": "Bob"}));
    l.ingest("crm", vec![ada, bob])
        .await
        .must("ingesting from crm")?;
    l.upsert("person", "p2", json!({"note": "kept"})).await?;
    let cy = source_record("person", "p3", json!({"id": "p3", "name": "Cy"}));
    l.ingest("erp", vec![cy]).await.must("ingesting from erp")?;
    store.head().await.must("reading the head")
}

/// The fields of `person` `key` as of `at` (the head when `None`), with
/// provenance.
async fn person_at(
    l: &Lake,
    key: &str,
    at: Option<Position>,
) -> Result<lakelands_api::Record, Failure> {
    let r = l.cat.get(&l.own, &"person".into(), get_at(key, at)).await;
    Ok(r.must(&format!("reading `{key}` as of {at:?}"))?.data)
}

/// The groups `by_name` holds as of `at`.
async fn names(l: &Lake, at: Option<Position>) -> Result<Vec<Value>, Failure> {
    let found = l.cat.find(&l.own, &"by_name".into(), find_at(at)).await;
    let Found::Records { rows, .. } = found.must("reading `by_name`")?.data else {
        return Err(Failure::new("`by_name` answers records".to_owned()));
    };
    Ok(rows
        .into_iter()
        .filter_map(|r| r.fields.get("name").cloned())
        .collect())
}

/// Whether `crm`'s values read as erased as of `at`: `p2`'s `name`,
/// `initial` and `loud` hold none, `name`'s provenance says `erased` and
/// names `crm`, `by_name` has no group of Ada or Bob, and `p3` reads Cy.
async fn erased(l: &Lake, at: Option<Position>) -> Held {
    let p2 = person_at(l, "p2", at).await?;
    let held: Vec<&str> = ["name", "initial", "loud"]
        .into_iter()
        .filter(|f| p2.fields.contains_key(*f))
        .collect();
    ensure!(
        held.is_empty(),
        "as of {at:?}, `p2` holds no value of crm's: it holds {held:?} in {p2:?}"
    );
    let meta = p2.meta.get("name");
    ensure!(
        meta.is_some_and(|m| m.erased && m.source.as_ref().is_some_and(|s| s == "crm")),
        "as of {at:?}, `p2`'s `name` says it was erased and came from crm; its provenance is {meta:?}"
    );
    ensure!(
        p2.fields.get("note") == Some(&json!("kept")),
        "as of {at:?}, `p2` keeps its author's note: {p2:?}"
    );
    let groups = names(l, at).await?;
    ensure!(
        !groups.contains(&json!("Ada"))
            && !groups.contains(&json!("Bob"))
            && groups.contains(&json!("Cy")),
        "as of {at:?}, the kept view `by_name` has a group for Cy and none for crm's names: {groups:?}"
    );
    let p3 = person_at(l, "p3", at).await?;
    ensure!(
        p3.fields.get("name") == Some(&json!("Cy")),
        "as of {at:?}, erp's `p3` reads Cy: {p3:?}"
    );
    Ok(())
}

/// What a report names, for comparison.
fn shape(r: &ForgetReport) -> Value {
    json!({
        "types": r.types.iter().map(|t| json!([t.ty, t.records])).collect::<Vec<_>>(),
        "fields": r.fields.iter().map(|f| json!([f.ty, f.field, f.values])).collect::<Vec<_>>(),
        "retired": r.retired.iter().map(|t| json!([t.ty, t.retired])).collect::<Vec<_>>(),
        "co_owned": r.co_owned.iter().map(|t| json!([t.ty, t.first])).collect::<Vec<_>>(),
        "recomputed": r.recomputed.iter().map(|f| json!(f.field)).collect::<Vec<_>>(),
        "views": r.views,
        "plaintext": r.plaintext,
    })
}

/// The `source_forgotten` events after `after`.
async fn forgettings(store: &dyn LakeStore, after: Position) -> Result<Vec<Event>, Failure> {
    let events = events_after(store, after).await?;
    let forgot = |e: &Event| matches!(e.body, EventBody::SourceForgotten { .. });
    Ok(events.into_iter().filter(forgot).collect())
}

/// An ingest from `crm` is refused, appending nothing.
async fn refused(store: &dyn LakeStore, l: &Lake) -> Held {
    let before = store.head().await.must("reading the head")?;
    let dee = source_record("person", "p4", json!({"id": "p4", "name": "Dee"}));
    refused_like(
        "an ingest from the forgotten crm",
        l.ingest("crm", vec![dee]).await,
        &json!({"code": "source_forgotten", "source": "crm"}),
    )?;
    let head = store.head().await.must("reading the head")?;
    ensure!(head == before, "the refused ingest appended up to {head}");
    Ok(())
}

async fn a_forgotten_sources_values_read_as_erased_at_every_position(
    store: Store,
    reopen: Reopen,
) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    lake(&l).await?;
    let before = people(store.as_ref(), &l).await?;
    let p2 = read(&l, "p2", "loud").await?;
    ensure!(
        p2 == json!("Bob"),
        "before the act `p2`'s `loud` reads Bob; it reads {p2}"
    );
    let crm: SourceId = "crm".into();
    let k = super::erasable::first(&keys, store.as_ref(), "crm")?;
    let report = store.forget_preflight(&crm).await.must("the preflight")?;
    let want = json!({
        "types": [["person", 2]], "fields": [["person", "name", 2]], "retired": [["person", 1]],
        "co_owned": [["person", ["p2"]]], "recomputed": ["initial", "loud"], "views": ["by_name"],
        "plaintext": 0,
    });
    ensure!(
        shape(&report) == want,
        "the preflight names {want}; it names {} in {report:?}",
        shape(&report)
    );
    ensure!(
        report.outputs == 2
            && report.events > 0
            && !report.report_id.is_empty()
            && report.reclaimed.is_none(),
        "the preflight counts crm's two recorded outputs and the log's events, carries an id and reclaims nothing: {report:?}"
    );
    let rate = report.events.saturating_mul(1000) / report.expected_ms.max(1);
    ensure!(
        rate > 0,
        "the preflight expects a time under the lock: {report:?}"
    );
    // A report of another head is refused.
    l.upsert("person", "p3", json!({"note": "moved"})).await?;
    refused_like(
        "forgetting crm with a report of another head",
        store.forget_source(&crm, Some(&report.report_id)).await,
        &json!({"code": "forget_report_stale"}),
    )?;
    let at = store.head().await.must("reading the head")?;
    let report = store
        .forget_preflight(&crm)
        .await
        .must("the preflight again")?;
    let done = store
        .forget_source(&crm, Some(&report.report_id))
        .await
        .must("forgetting crm")?;
    ensure!(
        done.reclaimed.is_some() && done.position > at,
        "the act appends and reclaims: {done:?}"
    );
    let forgot = forgettings(store.as_ref(), at).await?;
    let named = forgot.iter().map(|e| &e.body).collect::<Vec<_>>();
    ensure!(
        matches!(named.as_slice(), [EventBody::SourceForgotten { source, keys }] if source == "crm" && keys == &vec![k.clone()]),
        "one `source_forgotten` names crm and its key `{k}`; the log holds {named:?}"
    );
    let left = keys.ids(store.lake(), &crm).ok().unwrap_or_default();
    ensure!(
        left.is_empty(),
        "the key store holds no key of crm after the act: {left:?}"
    );
    let p1 = l
        .cat
        .get(&l.own, &"person".into(), get_at("p1", None))
        .await;
    ensure!(
        p1.is_err(),
        "`p1`, which crm alone brought, is retired: it reads {p1:?}"
    );
    let p1 = person_at(&l, "p1", Some(before)).await?;
    ensure!(
        !p1.fields.contains_key("name") && p1.fields.get("city") == Some(&json!("Rome")),
        "as of before the act `p1` holds no name, and its city, which is not erasable, stays: {p1:?}"
    );
    for at in [None, Some(before)] {
        erased(&l, at).await?;
    }
    store.rebuild().await.must("rebuilding after the act")?;
    for at in [None, Some(before)] {
        erased(&l, at).await?;
    }
    refused(store.as_ref(), &l).await?;
    // A repeat of the done act answers it done and appends nothing.
    let head = store.head().await.must("reading the head")?;
    let again = store
        .forget_source(&crm, None)
        .await
        .must("forgetting crm again")?;
    let after = store.head().await.must("reading the head")?;
    ensure!(
        after == head && again.types.is_empty() && again.retired.is_empty(),
        "forgetting crm again answers the act done, appending nothing; it answered {again:?}, the head moving {head} to {after}"
    );
    Ok(())
}

async fn a_forgetting_a_crash_stopped_is_finished_by_the_next_writers_open(
    store: Store,
    reopen: Reopen,
) -> Held {
    let keys = Arc::new(Keys::default());
    let faulty = Arc::new(Faulty::new(&keys));
    let seams = || Seams {
        keys: Some(faulty.clone() as Arc<dyn KeyStore>),
        backings: crate::conformance::backing::backings(),
        ..Seams::default()
    };
    let mut store = again(&reopen, store, seams(), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    lake(&l).await?;
    let before = people(store.as_ref(), &l).await?;
    let eve = source_record("person", "p5", json!({"id": "p5", "name": "Eve"}));
    l.ingest("crm2", vec![eve])
        .await
        .must("ingesting from crm2")?;
    let eve_at = store.head().await.must("reading the head")?;
    // `crm` stops after its event, `crm2` after its keys are destroyed,
    // where the rebuild asks for erp's key.
    for (source, stop, flag) in [
        ("crm", "after its event", &faulty.destroy_fails),
        (
            "crm2",
            "after its keys are destroyed",
            &faulty.after_destroy_fails,
        ),
    ] {
        let source: SourceId = source.into();
        faulty.destroyed.store(false, Ordering::SeqCst);
        flag.store(true, Ordering::SeqCst);
        let at = store.head().await.must("reading the head")?;
        let stopped = store.forget_source(&source, None).await;
        flag.store(false, Ordering::SeqCst);
        ensure!(
            stopped.is_err(),
            "the act stopped {stop} answers its failure; it answered {stopped:?}"
        );
        // The first is finished by the act's repeat; the second, whose
        // rebuild stopped and left the head behind the log, by the open.
        if source == "crm" {
            let done = store.forget_source(&source, None).await;
            ensure!(
                done.as_ref().is_ok_and(|r| r.reclaimed.is_some()),
                "the repeat of the act stopped {stop} finishes it and reclaims: {done:?}"
            );
        }
        store = again(&reopen, store, seams(), "after the stop").await?;
        let forgot = forgettings(store.as_ref(), at).await?;
        ensure!(
            forgot.len() == 1,
            "the act stopped {stop} has its event in the log: {forgot:?}; it answered {stopped:?}"
        );
        let l = Lake::of(store.as_ref());
        let left = keys.ids(store.lake(), &source).ok().unwrap_or_default();
        ensure!(
            left.is_empty(),
            "the open after a stop {stop} destroyed {source}'s keys: {left:?}"
        );
        let p5 = person_at(&l, "p5", Some(eve_at)).await?;
        ensure!(
            p5.fields.contains_key("name") == (source == "crm"),
            "after the open, crm2's `p5` holds its name only while crm2 is not forgotten: {p5:?}"
        );
        if source == "crm" {
            refused(store.as_ref(), &l).await?;
            for at in [None, Some(before)] {
                erased(&l, at).await?;
            }
        }
        let head = store.head().await.must("reading the head")?;
        store = again(&reopen, store, seams(), "once more").await?;
        let moved = events_after(store.as_ref(), head).await?;
        let other = moved
            .iter()
            .filter(|e| !matches!(e.body, EventBody::LeaseTaken { .. }));
        ensure!(
            other.count() == 0,
            "an open after the finished act appends nothing but its lease: {moved:?}"
        );
    }
    Ok(())
}

async fn a_source_with_plaintext_values_in_the_log_is_not_forgotten(
    store: Store,
    reopen: Reopen,
) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    let mut t = person();
    t.fields.retain(|f| f.name != "name");
    t.fields.push(sourced("name", StorageKind::Text));
    l.declare(t).await?;
    let ada = source_record("person", "p1", json!({"id": "p1", "name": "Ada"}));
    l.ingest("crm", vec![ada]).await.must("ingesting p1")?;
    l.declare(person()).await?;
    let report = store
        .forget_preflight(&"crm".into())
        .await
        .must("the preflight")?;
    ensure!(
        report.plaintext == 1,
        "the preflight counts one plaintext value of crm: {report:?}"
    );
    let before = store.head().await.must("reading the head")?;
    refused_like(
        "forgetting crm with a plaintext value in the log",
        store.forget_source(&"crm".into(), None).await,
        &json!({"code": "forget_plaintext", "source": "crm", "count": 1}),
    )?;
    let head = store.head().await.must("reading the head")?;
    ensure!(head == before, "the refused act appended up to {head}");
    // A value written under `email`, which a migration renamed to `mail`,
    // erasable since: the log holds it under the old name, in plaintext.
    let contact = |erasable: bool, name: &str| {
        let f = FieldDecl {
            erasable,
            ..sourced(name, StorageKind::Text)
        };
        let id = FieldDecl {
            required: true,
            ..sourced("id", StorageKind::Text)
        };
        lakelands_api::ontology::TypeDecl {
            key: vec!["id".into()],
            ..crate::conformance::fixture::type_decl("contact", vec![id, f])
        }
    };
    l.declare(contact(false, "email")).await?;
    let c1 = source_record("contact", "c1", json!({"id": "c1", "email": "a@b"}));
    l.ingest("web", vec![c1]).await.must("ingesting c1")?;
    l.migrate_renaming(contact(false, "mail"), &[("email", "mail")])
        .await?;
    l.declare(contact(true, "mail")).await?;
    let report = store
        .forget_preflight(&"web".into())
        .await
        .must("the preflight of web")?;
    ensure!(
        report.plaintext == 1,
        "the preflight counts web's plaintext value written as `email`, now `mail` and erasable: {report:?}"
    );
    refused_like(
        "forgetting web with a plaintext value under a renamed field",
        store.forget_source(&"web".into(), None).await,
        &json!({"code": "forget_plaintext", "source": "web", "count": 1}),
    )?;
    Ok(())
}

async fn an_export_imports_with_its_keys_or_with_its_forgetting(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let keys = Arc::new(Keys::default());
    let store = again(&given.reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(person()).await?;
    let ada = source_record("person", "p1", json!({"id": "p1", "name": "Ada"}));
    l.ingest("crm", vec![ada]).await.must("ingesting p1")?;
    let base = given.scratch.clone();
    let _ = std::fs::remove_dir_all(&base);
    let export = |name: &str| base.join(name);
    let lake = store.lake().clone();
    let import =
        |dir: std::path::PathBuf, seams: Seams| (given.import)(dir, lake.clone(), None, seams);
    crate::export::write(store.as_ref(), &export("sealed"))
        .await
        .must("exporting")?;
    let keyless = import(export("sealed"), Seams::default()).await;
    ensure!(
        matches!(&keyless, Err(Error::Unbound { .. })),
        "an export holding crm's sealed values, imported with no key store, is refused as unbound; it answered {:?}",
        keyless.as_ref().map(|_| ())
    );
    let other = Arc::new(Keys::default());
    let lacking = import(export("sealed"), with(&other)).await;
    ensure!(
        matches!(
            &lacking,
            Err(Error::Unavailable { situation })
                if matches!(**situation, lakelands_api::error::Situation::KeyMissing { .. })
        ),
        "an export holding crm's sealed values, imported with a key store that lacks its key, is refused as missing; it answered {:?}",
        lacking.as_ref().map(|_| ())
    );
    let keyed = import(export("sealed"), with(&keys)).await;
    let keyed = keyed.map_err(|e| Failure::new(format!("importing with the key store: {e}")))?;
    let name = (keyed.store.catalog(Default::default()))
        .get(&l.own, &"person".into(), get_at("p1", None))
        .await
        .must("reading the imported p1")?
        .data;
    ensure!(
        name.fields.get("name") == Some(&json!("Ada")),
        "imported with the key store, p1 reads its name: {name:?}"
    );
    let backup = Arc::new(keys.backup());
    let before = store.head().await.must("reading the head")?;
    store
        .forget_source(&"crm".into(), None)
        .await
        .must("forgetting crm")?;
    crate::export::write(store.as_ref(), &export("forgot"))
        .await
        .must("exporting again")?;
    let keyless = import(export("forgot"), Seams::default()).await;
    let keyless = keyless
        .map_err(|e| Failure::new(format!("importing the forgetting with no key store: {e}")))?;
    let p1 = (keyless.store.catalog(Default::default()))
        .get(&l.own, &"person".into(), get_at("p1", Some(before)))
        .await;
    let p1 = p1.must("reading the imported p1 before the act")?.data;
    let meta = p1.meta.get("name");
    ensure!(
        !p1.fields.contains_key("name") && meta.is_some_and(|m| m.erased),
        "imported with no key store, the forgotten crm's name arrives erased: {p1:?}"
    );
    // A backup of the key store from before the act still holds crm's key.
    let restored = import(export("forgot"), with(&backup)).await;
    let restored = restored.map_err(|e| Failure::new(format!("importing with the backup: {e}")))?;
    let p1 = (restored.store.catalog(Default::default()))
        .get(&l.own, &"person".into(), get_at("p1", Some(before)))
        .await;
    let p1 = p1.must("reading p1 imported with the backup")?.data;
    ensure!(
        !p1.fields.contains_key("name"),
        "imported with a backup that held crm's key, the first read answers the name erased: {p1:?}"
    );
    let held = backup
        .ids(store.lake(), &"crm".into())
        .ok()
        .unwrap_or_default();
    ensure!(
        held.is_empty(),
        "the import destroyed the backup's keys of crm: {held:?}"
    );
    Ok(())
}

check!(
    RECLAIM_OWED = a_reclaim_that_fails_keeps_no_writer_out,
    "058 and 065, extended (lakelands#1161, lakelands#1179): a forgetting whose reclaim fails answers its report, the values erased already, with the reclaim owed and its cause, as the lake does and its next preflight; a `rebuild()` whose reclaim fails answers, still owing it; the lake's next writer opens though the reclaim fails again, still owing it, writes and reads the values erased; the open after the reclaim can run finishes it and owes nothing, and the act's repeat answers it done; a removal whose reclaim fails answers as the forgetting does, and so does its preflight; a compact that runs clears what it owed, and the next open reclaims nothing, while one that fails with nothing owed owes nothing; a rebuild a forgetting still owes refuses the writer's open while it fails, as before",
    given
);

check!(
    FORGET_EXPECTED_TIME = the_expected_time_follows_the_lakes_last_rebuild,
    "058, extended (lakelands#1152): the time a preflight expects the act to hold the writer's lock is an estimate: before the handle has rebuilt, the log's events at the engine's stated rate and the preflight's own walk of the log, which the act walks again; after a rebuild, the events at that rebuild's measured rate, with the last reclaim's time and the walk, for a forgetting and a removal alike: after an open that rebuilds a projection an older build laid out, at most that open's time and the walk's; after `rebuild()` and a `compact()`, the act's rebuild and its reclaim, at most their measured time and the walk's and at least two thirds of the two less 10 ms, the engine's stated rate set aside",
    given
);

/// How many of the next reclaims, and of the next clears a rebuild
/// begins with, fail; the rebuild rate the engine states, where not 0;
/// and how many of the next writer's opens meet a projection an older
/// build laid out, so the store opens it for a rebuild (lakelands#1152).
pub type Fails = Arc<[AtomicU32; 4]>;

/// The harness's engine with a projection whose next reclaims and
/// clears fail as [`Fails`] says (lakelands#1161).
struct Reclaims {
    inner: Box<dyn lakelands_api::engine::LakeEngine>,
    fails: Fails,
}

/// The harness's engine opener, making [`Reclaims`].
struct ReclaimsOpener {
    inner: Arc<dyn EngineOpener>,
    fails: Fails,
}

/// What makes of the harness's engine one whose reclaims and clears fail
/// as `fails` says, for a [`crate::conformance::Given::fault`].
pub fn failing(fails: &Fails) -> crate::conformance::Wrap {
    let fails = fails.clone();
    Arc::new(move |inner| {
        let fails = fails.clone();
        Arc::new(ReclaimsOpener { inner, fails }) as Arc<dyn EngineOpener>
    })
}

/// Whether the next call `i` of [`Fails`] counts fails, counting it.
fn fails(fails: &Fails, i: usize) -> bool {
    let n = &fails[i];
    n.load(Ordering::SeqCst) > 0 && n.fetch_sub(1, Ordering::SeqCst) > 0
}

impl EngineOpener for ReclaimsOpener {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn log_ends(
        &self,
        location: &Location,
    ) -> lakelands_api::Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        self.inner.log_ends(location)
    }
    fn open(
        &self,
        location: &Location,
        mode: OpenMode,
    ) -> lakelands_api::Result<Box<dyn lakelands_api::engine::LakeEngine>> {
        if mode != OpenMode::Rebuild && fails(&self.fails, 3) {
            crate::engines::layout(&location.lake, Some(1), 2)?;
        }
        let (inner, fails) = (self.inner.open(location, mode)?, self.fails.clone());
        Ok(Box::new(Reclaims { inner, fails }))
    }
}

impl lakelands_api::engine::LakeEngine for Reclaims {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn rebuild_rate(&self) -> u64 {
        match self.fails[2].load(Ordering::SeqCst) {
            0 => self.inner.rebuild_rate(),
            stated => u64::from(stated),
        }
    }
    fn lake(&self) -> &LakeId {
        self.inner.lake()
    }
    fn take_lease(&self, holder: &str) -> lakelands_api::Result<Lease> {
        self.inner.take_lease(holder)
    }
    fn release_lease(&self, lease: &Lease) -> lakelands_api::Result<()> {
        self.inner.release_lease(lease)
    }
    fn tip(&self) -> lakelands_api::Result<Option<(Position, Digest)>> {
        self.inner.tip()
    }
    fn append(&self, lease: &Lease, events: &[CanonicalEvent]) -> lakelands_api::Result<()> {
        self.inner.append(lease, events)
    }
    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> lakelands_api::Result<()> {
        self.inner.import(lease, events)
    }
    fn read(&self, from: Position, limit: u32) -> lakelands_api::Result<Vec<CanonicalEvent>> {
        self.inner.read(from, limit)
    }
    fn seal_segment(
        &self,
        lease: &Lease,
        imported: bool,
        first: &mut dyn FnMut(&SegmentDigest) -> lakelands_api::Result<Vec<CanonicalEvent>>,
    ) -> lakelands_api::Result<Option<SegmentDigest>> {
        self.inner.seal_segment(lease, imported, first)
    }
    fn segments(&self) -> lakelands_api::Result<Vec<SegmentDigest>> {
        self.inner.segments()
    }
    fn projection(&self) -> &dyn ProjectionStore {
        self
    }
    fn view(&self, as_of: Option<Position>) -> lakelands_api::Result<Box<dyn ProjectionView + '_>> {
        self.inner.view(as_of)
    }
    fn ranked(&self, ty: &TypeName, asked: &Rankings, k: u32) -> lakelands_api::Result<Ranked> {
        self.inner.ranked(ty, asked, k)
    }
    fn vectors(&self) -> Option<&dyn VectorIndex> {
        self.inner.vectors()
    }
}

impl ProjectionStore for Reclaims {
    fn begin(&self) -> lakelands_api::Result<()> {
        self.inner.projection().begin()
    }
    fn commit(&self, through: Position) -> lakelands_api::Result<()> {
        self.inner.projection().commit(through)
    }
    fn rollback(&self) {
        self.inner.projection().rollback()
    }
    fn folded_through(&self) -> lakelands_api::Result<Option<Position>> {
        self.inner.projection().folded_through()
    }
    fn put_declaration(
        &self,
        declaration: &lakelands_api::write::Declaration,
        version: u32,
        schema_hash: Option<&str>,
        at: Position,
    ) -> lakelands_api::Result<()> {
        (self.inner.projection()).put_declaration(declaration, version, schema_hash, at)
    }
    fn put_record(
        &self,
        record: &lakelands_api::record::StoredRecord,
    ) -> lakelands_api::Result<()> {
        self.inner.projection().put_record(record)
    }
    fn end_head_only(&self, name: &TypeName, at: Position) -> lakelands_api::Result<()> {
        self.inner.projection().end_head_only(name, at)
    }
    fn put_proposal(
        &self,
        proposal: &lakelands_api::write::Proposal,
        at: Position,
    ) -> lakelands_api::Result<()> {
        self.inner.projection().put_proposal(proposal, at)
    }
    fn put_membership(
        &self,
        set: &str,
        ty: &TypeName,
        key: &lakelands_api::RecordKey,
        member: bool,
        at: Position,
    ) -> lakelands_api::Result<()> {
        (self.inner.projection()).put_membership(set, ty, key, member, at)
    }
    fn put_output(
        &self,
        seam: &str,
        input: &Digest,
        output: &Value,
        at: Position,
    ) -> lakelands_api::Result<()> {
        self.inner.projection().put_output(seam, input, output, at)
    }
    fn mark_applied(&self, applied: &lakelands_api::engine::Applied) -> lakelands_api::Result<()> {
        self.inner.projection().mark_applied(applied)
    }
    fn clear(&self) -> lakelands_api::Result<()> {
        match fails(&self.fails, 1) {
            true => Err(crashed()),
            false => self.inner.projection().clear(),
        }
    }
    fn compact(&self) -> lakelands_api::Result<lakelands_api::engine::Reclaimed> {
        match fails(&self.fails, 0) {
            true => Err(crashed()),
            false => self.inner.projection().compact(),
        }
    }
}

/// Whether a report answered, naming the cause of the reclaim the lake
/// owes.
fn owing(store: &dyn LakeStore, said: Result<&Option<String>, &Error>) -> bool {
    said.is_ok_and(|o| o.is_some() && *o == store.reclaim_owed())
}

async fn a_reclaim_that_fails_keeps_no_writer_out(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let keys = Arc::new(Keys::default());
    let seams = || with(&keys);
    let fails: Fails = Arc::default();
    let wrap = failing(&fails);
    let fault = |store: Store, what: &str| {
        let (open, what) = ((given.fault)(store, seams(), wrap.clone()), what.to_owned());
        async move {
            open.await
                .map_err(|e| Failure::new(format!("the writer's open {what} was refused: {e}")))
        }
    };
    let mut store = fault(store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    lake(&l).await?;
    people(store.as_ref(), &l).await?;
    let crm: SourceId = "crm".into();
    // A compact that fails with no act waiting on it owes nothing.
    fails[0].store(1, Ordering::SeqCst);
    let compacted = store.compact().await;
    ensure!(
        compacted.is_err() && store.reclaim_owed().is_none(),
        "a compact that fails with nothing owed answers its failure and owes nothing: \
         {compacted:?}, owing {:?}",
        store.reclaim_owed()
    );
    // The act's reclaim fails: the values are erased, the reclaim owed.
    fails[0].store(1, Ordering::SeqCst);
    let act = store.forget_source(&crm, None).await;
    let pre = store.forget_preflight(&crm).await;
    ensure!(
        owing(store.as_ref(), act.as_ref().map(|r| &r.reclaim_owed))
            && owing(store.as_ref(), pre.as_ref().map(|r| &r.reclaim_owed)),
        "a forgetting whose reclaim fails answers its report owing the reclaim, saying why, as \
         the lake and its next preflight do; it answered {act:?} and {pre:?}, owing {:?}",
        store.reclaim_owed()
    );
    erased(&l, None).await?;
    fails[0].store(1, Ordering::SeqCst);
    let rebuilt = store.rebuild().await;
    let tried = fails[0].load(Ordering::SeqCst) == 0;
    ensure!(
        rebuilt.is_ok() && tried && store.reclaim_owed().is_some(),
        "a rebuild whose reclaim fails answers, still owing it: {rebuilt:?}, tried {tried}"
    );
    // The next writer opens though the reclaim fails again.
    fails[0].store(1, Ordering::SeqCst);
    store = fault(store, "while its reclaim fails").await?;
    let (tried, owed) = (fails[0].load(Ordering::SeqCst) == 0, store.reclaim_owed());
    ensure!(
        tried && owed.is_some(),
        "the writer's open tries the owed reclaim and, failing, still owes it, saying why: tried \
         {tried}, owing {owed:?}"
    );
    let l = Lake::of(store.as_ref());
    erased(&l, None).await?;
    l.upsert("person", "p2", json!({"note": "written"})).await?;
    // The open after the reclaim can run finishes it.
    store = fault(store, "once its reclaim runs").await?;
    let done = store.forget_source(&crm, None).await;
    ensure!(
        store.reclaim_owed().is_none() && done.as_ref().is_ok_and(|r| r.reclaimed.is_none()),
        "the open that can reclaim finishes it and owes nothing, and the act's repeat answers it \
         done: owing {:?}, the repeat {done:?}",
        store.reclaim_owed()
    );
    // A removal's reclaim is owed as a forgetting's, and a compact that
    // runs clears it.
    let all = lakelands_api::write::Selection::WrittenBefore(
        lakelands_api::Timestamp::from_date(9999, 1, 1).expect("a date"),
    );
    let erp: SourceId = "erp".into();
    fails[0].store(1, Ordering::SeqCst);
    let act = store.remove_values(&erp, &all, false, None).await;
    let pre = store.remove_values_preflight(&erp, &all).await;
    ensure!(
        owing(store.as_ref(), act.as_ref().map(|r| &r.reclaim_owed))
            && owing(store.as_ref(), pre.as_ref().map(|r| &r.reclaim_owed)),
        "a removal whose reclaim fails answers its report owing the reclaim, as its next \
         preflight does; it answered {act:?} and {pre:?}"
    );
    let compacted = store.compact().await;
    ensure!(
        compacted.is_ok() && store.reclaim_owed().is_none(),
        "a compact that runs clears the owed reclaim: {compacted:?}, owing {:?}",
        store.reclaim_owed()
    );
    fails[0].store(1, Ordering::SeqCst);
    store = fault(store, "after the compact").await?;
    let left = fails[0].swap(0, Ordering::SeqCst);
    ensure!(
        left == 1 && store.reclaim_owed().is_none(),
        "after the compact the acts are done: the next open reclaims nothing ({left} reclaim \
         left to fail) and owes nothing"
    );
    // A rebuild still owed refuses the writer's open while it fails.
    let l = Lake::of(store.as_ref());
    let eve = source_record("person", "p5", json!({"id": "p5", "name": "Eve"}));
    l.ingest("crm2", vec![eve])
        .await
        .must("ingesting from crm2")?;
    fails[1].store(2, Ordering::SeqCst);
    let stopped = store.forget_source(&"crm2".into(), None).await;
    let refused = (given.fault)(store, seams(), wrap.clone()).await.map(drop);
    ensure!(
        stopped.is_err() && refused.is_err() && fails[1].load(Ordering::SeqCst) == 0,
        "a forgetting whose rebuild fails, and the writer's open whose rebuild of it fails \
         again, are refused; they answered {stopped:?} and {refused:?}"
    );
    Ok(())
}

async fn the_expected_time_follows_the_lakes_last_rebuild(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let keys = Arc::new(Keys::default());
    // An engine that states one event a second: the act's time read from
    // it is the log's events in seconds, far from any measured fold.
    let fails: Fails = Arc::default();
    fails[2].store(1, Ordering::SeqCst);
    let fault = |store: Store| (given.fault)(store, with(&keys), failing(&fails));
    let opened = |e: Error| Failure::new(format!("the writer's open was refused: {e}"));
    let store = fault(store).await.map_err(opened)?;
    let l = Lake::of(store.as_ref());
    lake(&l).await?;
    people(store.as_ref(), &l).await?;
    // Enough records that a fold takes time a clock reads: each folds a
    // derived field, a function's output and a kept view's group.
    for batch in 0..4 {
        let rows = (0..100).map(|i| {
            let key = format!("b{batch}-{i}");
            source_record("person", &key, json!({"id": key, "name": format!("n{i}")}))
        });
        l.ingest("bulk", rows.collect()).await.must("ingesting")?;
    }
    let (crm, erp): (SourceId, SourceId) = ("crm".into(), "erp".into());
    let all = lakelands_api::write::Selection::WrittenBefore(
        lakelands_api::Timestamp::from_date(9999, 1, 1).expect("a date"),
    );
    let ms =
        |began: std::time::Instant| u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
    let began = std::time::Instant::now();
    let pre = store.forget_preflight(&crm).await.must("the preflight")?;
    let (stated, walked) = (pre.events * 1000, ms(began));
    ensure!(
        pre.events > 400 && (stated..=stated + walked + 1).contains(&pre.expected_ms),
        "before a rebuild the preflight expects the log's events at the engine's rate of one a \
         second, {stated} ms, and its own walk of {walked} ms: {pre:?}"
    );
    // An open that rebuilds measures its fold.
    fails[3].store(1, Ordering::SeqCst);
    let began = std::time::Instant::now();
    let store = fault(store).await.map_err(opened)?;
    let took = ms(began);
    let began = std::time::Instant::now();
    let pre = store.forget_preflight(&crm).await.must("the preflight")?;
    let took = took + ms(began);
    ensure!(
        fails[3].load(Ordering::SeqCst) == 0 && pre.expected_ms <= took + 2,
        "after an open that rebuilt the projection, the open and the preflight's own walk of \
         {took} ms, the preflight expects at most that time; it expects {} ms of {} events",
        pre.expected_ms,
        pre.events
    );
    // A rebuild and a reclaim, the act's two steps, each measured.
    let began = std::time::Instant::now();
    store.rebuild().await.must("rebuilding")?;
    let rebuilt = ms(began);
    let began = std::time::Instant::now();
    store.compact().await.must("compacting")?;
    let took = rebuilt + ms(began);
    let began = std::time::Instant::now();
    let pre = store.forget_preflight(&crm).await.must("the preflight")?;
    let walked = ms(began);
    let began = std::time::Instant::now();
    let removal =
        (store.remove_values_preflight(&erp, &all).await).must("the removal's preflight")?;
    let walks = [walked, ms(began)];
    let both = [
        ("forgetting", pre.expected_ms),
        ("removal", removal.expected_ms),
    ];
    for ((what, ms), walked) in both.into_iter().zip(walks) {
        ensure!(
            ms <= took + walked + 3 && took * 2 <= ms * 3 + 30,
            "after a rebuild and a reclaim of {took} ms the {what}'s preflight, whose own walk \
             took {walked} ms, expects at most the three and at least two thirds of the two \
             less 10 ms; it expects {ms} ms of {} events",
            pre.events
        );
    }
    Ok(())
}

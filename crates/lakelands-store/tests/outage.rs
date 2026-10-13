//! An engine that cannot read a record by key answers its outage at the
//! doors that judge whether a key may be named to a principal, never the
//! answer a hold gives (lakelands#927): a migration's report, the proposal
//! listing with its accept and reject, and a cardinality refusal. On a
//! healthy engine each answers as it did. The engine is the memory engine
//! behind the wrapper of `probed`, which fails every read by key of one
//! type while it is set.

mod probed;

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::principal::PrincipalKind;
use lakelands_api::seam::Seams;
use lakelands_api::write::{Migration, Outcome};
use lakelands_api::{Error, LakeStore, Location, Principal, RecordKey, TypeName};
use lakelands_core::conformance::fixture::{Lake, principal};
use lakelands_engine_mem::MemoryOpener;
use probed::{Probe, ProbedOpener};
use serde_json::{Value, json};

/// A lake on the probed memory engine: `doc` d1 and d2 with a text `n`,
/// `partner` p1 and p2, and d1 picking p1, where a deal picks one.
async fn lake(name: &str) -> (Arc<dyn LakeStore>, Arc<Probe>) {
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory(name), seams, OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    l.declare_json(json!({"declare": "type", "name": "partner", "fields": [text("n")]}))
        .await
        .unwrap();
    l.declare_json(
        json!({"declare": "type", "name": "doc", "fields": [text("n")],
        "relations": [{"name": "pick", "target": "partner", "cardinality": "many_to_one"}]}),
    )
    .await
    .unwrap();
    for (ty, key) in [
        ("partner", "p1"),
        ("partner", "p2"),
        ("doc", "d1"),
        ("doc", "d2"),
    ] {
        l.upsert(ty, key, json!({"n": "x"})).await.unwrap();
    }
    let (doc, pick) = (&"doc".into(), &"pick".into());
    let (d1, p1) = (RecordKey::from("d1"), RecordKey::from("p1"));
    let linked = l.cat.link(
        &l.own,
        doc,
        &d1,
        pick,
        &p1,
        Default::default(),
        Default::default(),
    );
    linked.await.unwrap();
    (store, probe)
}

fn who(store: &dyn LakeStore, id: &str, grants: Value) -> Principal {
    principal(
        store,
        PrincipalKind::Agent,
        id,
        serde_json::from_value(grants).unwrap(),
    )
}

/// What `answer` says, short: its outage, its error, or what it answered.
fn said<T: std::fmt::Debug>(answer: lakelands_api::Result<T>) -> String {
    match answer {
        Err(Error::Unavailable { .. }) => "unavailable".to_owned(),
        Err(e) => format!("{e}"),
        Ok(v) => format!("{v:?}"),
    }
}

/// Each door asked on the healthy engine and with `failing`'s reads by
/// key failed: what it answers, before and while.
async fn both(probe: &Probe, failing: &str, ask: impl AsyncFn() -> String) -> (String, String) {
    let healthy = ask().await;
    *probe.fail_get.lock().unwrap() = Some(TypeName::from(failing));
    let failed = ask().await;
    *probe.fail_get.lock().unwrap() = None;
    (healthy, failed)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_engine_that_cannot_read_a_key_answers_its_outage_not_a_hold() {
    let (store, probe) = lake("outage").await;
    let l = Lake::of(store.as_ref());
    // A reader without `restricted`, who reviews and migrates.
    let ann = who(
        store.as_ref(),
        "ann",
        json!({"default": "write", "review": true, "schema": true}),
    );
    // A migration that makes `n` a number names a record its reader is
    // answered among its samples; the type is walked by scan, so only
    // that judgement reads `doc` by key.
    let to = json!({"name": "doc", "fields": [{"name": "n", "type": {"kind": "number"}}],
        "relations": [{"name": "pick", "target": "partner", "cardinality": "many_to_one"}]});
    let m: Migration = serde_json::from_value(json!({"to": to})).unwrap();
    let preflight = || async {
        let report = l.cat.migrate(&ann, m.clone()).await.map(|r| r.data);
        said(report.map(|_| "a report"))
    };
    let mut wrong = Vec::new();
    let (healthy, failed) = both(&probe, "doc", preflight).await;
    if healthy == "unavailable" || failed != "unavailable" {
        wrong.push(format!(
            "the preflight: {healthy} healthy, {failed} failing"
        ));
    }
    // A proposal on d2, by bob: listed to ann, and decided by her, on the
    // healthy engine; with `doc` unreadable, the listing, the accept and
    // the reject answer the outage, never "not there".
    let bob = who(store.as_ref(), "bob", json!({"default": "write"}));
    let write = serde_json::from_value(
        json!({"write": "upsert", "type": "doc", "key": "d2", "patch": {"n": "y"}}),
    )
    .unwrap();
    let Ok(Outcome::Proposed { proposal, .. }) = l.cat.propose(&bob, write, "b".into()).await
    else {
        panic!("bob's proposal must be proposed");
    };
    let listed = || async {
        let listed = l
            .cat
            .proposals(&ann, Default::default())
            .await
            .map(|r| r.data);
        match listed {
            Ok(all) => format!("{}", all.iter().any(|p| p.id == proposal)),
            other => said(other),
        }
    };
    let (healthy, failed) = both(&probe, "doc", listed).await;
    if healthy != "true" || failed != "unavailable" {
        wrong.push(format!("the listing: {healthy} healthy, {failed} failing"));
    }
    *probe.fail_get.lock().unwrap() = Some(TypeName::from("doc"));
    let rejected = said(l.cat.reject(&ann, &proposal, "no".into()).await);
    let accepted = said(l.cat.accept(&ann, &proposal).await);
    *probe.fail_get.lock().unwrap() = None;
    if rejected != "unavailable" || accepted != "unavailable" {
        wrong.push(format!(
            "the reject {rejected} and the accept {accepted}, failing"
        ));
    }
    // A second pick for d1, by ann: refused naming p1 on the healthy
    // engine, the outage with the link type unreadable.
    let (doc, pick) = (&"doc".into(), &"pick".into());
    let (d1, p2) = (RecordKey::from("d1"), RecordKey::from("p2"));
    let second = || async {
        let linked = l.cat.link(
            &ann,
            doc,
            &d1,
            pick,
            &p2,
            Default::default(),
            Default::default(),
        );
        said(linked.await)
    };
    let (healthy, failed) = both(&probe, "sys.link.doc.pick", second).await;
    if !healthy.contains("p1") || failed != "unavailable" {
        wrong.push(format!(
            "a second pick: {healthy} healthy, {failed} failing"
        ));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

//! A forgetting refolds only the records its source wrote where nothing
//! else reads them (`docs/decisions/058-a-source-may-be-forgotten.md`),
//! and answers every read as the rebuild does. Twin lakes are fed one
//! log; one forgets by the scoped refold and the other by the rebuild its
//! host asks for, and each read is compared at every position.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use lakelands_api::event::EventBody;
use lakelands_api::location::Limits;
use lakelands_api::ontology::{FieldDecl, StorageKind, TypeDecl};
use lakelands_api::query::{Search, SearchMode};
use lakelands_api::read::{Direction, Find, OrderBy, ReadAt};
use lakelands_api::write::ForgetWay;
use lakelands_api::{Error, LakeStore, Position, SourceId, TypeName};
use serde_json::{Value, json};

use super::erasable::{Keys, person, with};
use super::forgotten::{Fails, failing};
use crate::conformance::fixture::{
    Lake, Must, events_after, find_at, get_at, limits, order, refused_like, source_record, sourced,
    type_decl, vector,
};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    SCRUBBED = a_scoped_refold_answers_as_the_rebuild_at_every_position,
    "058, extended (lakelands#1120): where nothing but their own states reads the records a source wrote, `forget_preflight` and `forget_source` name the scoped refold, counting its records and events beside both expected times, and with `limits.forget_rebuild` they name the rebuild, saying the host asks for it; twin lakes fed one log, one forgetting each way, answer alike at every position: each record's fields with provenance, a value another source overwrote, a record the source alone brought retired and one an author co-owns erased, a derived field and a function's recorded output computed from the erased value, values written before and after a migration, `find` with its total, an order and a `where` on the erased field, a lexical search for the erased, retired and overwritten values, `describe_type`, and the records of a type no record restated reads, written before and after its own migration; a rebuild after the scoped refold answers the same",
    given
);

check!(
    SCRUB_FALLBACK = a_forgetting_whose_records_are_read_beyond_their_states_rebuilds,
    "058, extended (lakelands#1120): where a kept view, a set (one reading them two references away among them), a vector field, a relation carried by link records or another type's computed field reading through a reference reads the records a source wrote, `forget_preflight` and `forget_source` name the rebuild and say which, and the lake answers after the act as it does after a rebuild",
    given
);

check!(
    SCRUB_CRASH = a_scoped_refold_stopped_between_pages_is_finished_by_the_next_open,
    "058, extended (lakelands#1120): a scoped refold restates one page of records in each transaction; one stopped after its first page leaves the source's ingest refused, and the writer's next open refolds every page again, appending nothing, after which the act is done and the lake answers as it does after a rebuild",
    given
);

/// The harness's lake, or a failure naming `what` it opened.
fn opened(what: &str) -> impl Fn(Error) -> Failure + '_ {
    move |e| Failure::new(format!("the harness opened no {what}: {e}"))
}

/// `person` with `initial`, derived from `name`, and `loud`, which the
/// suite's backing computes from it, and `zip` after a migration.
fn people(zip: bool) -> TypeDecl {
    let mut t = person();
    let initial = json!({"name": "initial", "type": {"kind": "text"}, "authority": "derived",
                         "derived": "name"});
    t.fields
        .push(serde_json::from_value(initial).expect("a field reads"));
    t.fields.push(crate::conformance::fixture::computed_by(
        "loud",
        StorageKind::Text,
        "shout",
        &["name"],
    ));
    for f in (t.fields.iter_mut()).filter(|f| ["name", "city"].contains(&f.name.as_str())) {
        f.searchable = true;
    }
    if zip {
        t.fields.push(sourced("zip", StorageKind::Text));
    }
    t
}

/// `order`, which nothing of `person` reads, with `paid` after a
/// migration.
fn orders(paid: bool) -> TypeDecl {
    let id = FieldDecl {
        required: true,
        ..sourced("id", StorageKind::Text)
    };
    let mut fields = vec![id, sourced("total", StorageKind::Number)];
    if paid {
        fields.push(sourced("paid", StorageKind::Text));
    }
    TypeDecl {
        key: vec!["id".into()],
        ..type_decl("order", fields)
    }
}

/// The function `shout`, `person` and `order`.
async fn declared(l: &Lake) -> Held {
    let kind = crate::conformance::backing::KIND;
    let text = json!({"kind": "text"});
    let config = json!({"token": "scrubbed", "echo": "name"});
    l.declare_json(
        json!({"declare": "function", "name": "shout", "backing": kind,
        "inputs": [{"name": "name", "type": text}], "config": config}),
    )
    .await?;
    l.declare(people(false)).await?;
    l.declare(orders(false)).await?;
    Ok(())
}

/// The log both twins hold: `p1` from crm alone, `p2` from crm with an
/// author's note, `p3` from erp, `p4` from crm whose name erp overwrites
/// after a migration adds `zip`, and `p5` from crm after it; erp's
/// orders `o1` and `o2` before a migration of `order` adds `paid`, and
/// `o3` after it.
async fn written(l: &Lake) -> Held {
    let row = |key: &str, fields: Value| source_record("person", key, fields);
    let order = |key: &str, fields: Value| source_record("order", key, fields);
    let crm = vec![
        row("p1", json!({"id": "p1", "name": "Ada", "city": "Rome"})),
        row("p2", json!({"id": "p2", "name": "Bob"})),
        row("p4", json!({"id": "p4", "name": "Dee", "city": "Oslo"})),
    ];
    l.ingest("crm", crm).await.must("ingesting from crm")?;
    l.upsert("person", "p2", json!({"note": "kept"})).await?;
    let cy = row("p3", json!({"id": "p3", "name": "Cy"}));
    let o1 = order("o1", json!({"id": "o1", "total": 3}));
    let o2 = order("o2", json!({"id": "o2", "total": 5}));
    l.ingest("erp", vec![cy, o1, o2])
        .await
        .must("ingesting from erp")?;
    l.migrate(orders(true)).await?;
    l.migrate(people(true)).await?;
    let di = row("p4", json!({"id": "p4", "name": "Di", "zip": "0150"}));
    let o3 = order("o3", json!({"id": "o3", "total": 8, "paid": "yes"}));
    l.ingest("erp", vec![di, o3])
        .await
        .must("ingesting from erp")?;
    let eve = row("p5", json!({"id": "p5", "name": "Eve", "zip": "1000"}));
    l.ingest("crm", vec![eve])
        .await
        .must("ingesting from crm")?;
    let bob = row("p2", json!({"id": "p2", "city": "Bonn"}));
    l.ingest("erp", vec![bob])
        .await
        .must("ingesting from erp")?;
    Ok(())
}

/// The lake's positions from genesis, but a lease's: one writer's open
/// takes one where its twin's does not, so the twins' events are paired
/// by their place in this list.
async fn positions(store: &dyn LakeStore) -> Result<Vec<Position>, Failure> {
    let events = events_after(store, Position::GENESIS).await?;
    let leased = |b: &EventBody| matches!(b, EventBody::LeaseTaken { .. });
    Ok(events
        .into_iter()
        .filter(|e| !leased(&e.body))
        .map(|e| e.position)
        .collect())
}

/// `v` with each position in it named by its place in `at`, and what
/// differs between twins by their reads and clocks left out: the times
/// the pages sorted whole, and a cursor, which holds its read's time.
fn paired(v: Value, at: &[Position]) -> Value {
    match v {
        Value::Object(o) => Value::Object(
            o.into_iter()
                .filter(|(k, _)| {
                    !["known_at", "seen_at", "sorted_whole", "next_cursor"].contains(&k.as_str())
                })
                .map(|(k, v)| match (k.as_str(), v.as_u64()) {
                    ("position", Some(p)) => {
                        let i = at.iter().position(|a| a.0 == p);
                        (
                            k,
                            json!(i.map_or(format!("lease {p}"), |i| format!("#{i}"))),
                        )
                    }
                    _ => (k, paired(v, at)),
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(|v| paired(v, at)).collect()),
        v => v,
    }
}

/// An answer as JSON, the error's wire form where it failed.
fn answered<T: serde::Serialize>(r: lakelands_api::Result<lakelands_api::Reply<T>>) -> Value {
    match r {
        Ok(r) => serde_json::to_value(r.data).unwrap_or_default(),
        Err(e) => json!({"error": serde_json::to_value(&e).unwrap_or_default()}),
    }
}

/// Each read a twin is compared by, as of each of `at` (the head when
/// `None`), labelled: `person` `keys` with provenance, every record with
/// the total and the retired, an order by `name`, a `where` on it and
/// one value of it looked up, a lexical search for each of crm's names
/// and cities, `describe_type`, and every order.
async fn reads(
    store: &dyn LakeStore,
    keys: &[&str],
    at: &[Option<Position>],
) -> Result<Vec<(String, Value)>, Failure> {
    let l = Lake::of(store);
    let all = positions(store).await?;
    let ty = TypeName::from("person");
    let place = |a: &Option<Position>| match a {
        None => "the head".to_owned(),
        Some(p) => all
            .iter()
            .position(|q| q == p)
            .map_or(format!("lease {p}"), |i| format!("#{i}")),
    };
    let mut got = Vec::new();
    for a in at {
        let mut push = |what: String, v: Value| {
            got.push((format!("{what} as of {}", place(a)), paired(v, &all)))
        };
        for key in keys {
            let r = l.cat.get(&l.own, &ty, get_at(key, *a)).await;
            push(format!("`{key}`"), answered(r));
        }
        let finds = [
            (
                "every record",
                Find {
                    total: true,
                    retired: true,
                    ..find_at(*a)
                },
            ),
            (
                "an order by name",
                Find {
                    order_by: vec![order("name", OrderBy::Value, Direction::Desc)],
                    ..find_at(*a)
                },
            ),
            (
                "a where on name",
                Find {
                    filter: Some("exists(name) and name != \"Cy\"".into()),
                    total: true,
                    ..find_at(*a)
                },
            ),
            (
                "a name looked up",
                Find {
                    filter: Some("name = 'Bob'".into()),
                    ..find_at(*a)
                },
            ),
        ];
        for (what, find) in finds {
            let r = l.cat.find(&l.own, &ty, find).await;
            push(what.to_owned(), answered(r));
        }
        let at = ReadAt {
            as_of: *a,
            ..ReadAt::default()
        };
        for text in ["Ada", "Bob", "Dee", "Di", "Cy", "Rome", "Oslo", "Bonn"] {
            let search = Search {
                ty: ty.clone(),
                mode: SearchMode::lexical(text),
                k: 20,
                cursor: None,
                filter: None,
                rerank: None,
                with_meta: true,
                at,
            };
            let r = l.cat.search(&l.own, search).await;
            push(format!("a search for {text}"), answered(r));
        }
        let r = l.cat.describe_type(&l.own, &ty, at).await;
        push("`describe_type`".to_owned(), answered(r));
        let orders = Find {
            total: true,
            retired: true,
            ..find_at(*a)
        };
        let r = l.cat.find(&l.own, &"order".into(), orders).await;
        push("every order".to_owned(), answered(r));
    }
    Ok(got)
}

/// The reads `here` and `there` answer apart, each with where its
/// answers first part and what each holds there.
fn apart(here: &[(String, Value)], there: &[(String, Value)]) -> Vec<String> {
    let mut apart: Vec<String> = (here.iter().zip(there))
        .filter(|(a, b)| a != b)
        .map(|((what, a), (_, b))| format!("{what}: {}", parted(a, b, String::new())))
        .collect();
    if here.len() != there.len() {
        apart.push(format!("{} reads / {}", here.len(), there.len()));
    }
    apart
}

/// Where `a` and `b` first part, from `at`, and what each holds there.
fn parted(a: &Value, b: &Value, at: String) -> String {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys = x
                .keys()
                .chain(y.keys())
                .collect::<std::collections::BTreeSet<_>>();
            let null = Value::Null;
            (keys.into_iter())
                .find(|k| x.get(*k) != y.get(*k))
                .map(|k| {
                    let (u, v) = (x.get(k).unwrap_or(&null), y.get(k).unwrap_or(&null));
                    parted(u, v, format!("{at}.{k}"))
                })
                .unwrap_or_default()
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => (x.iter().zip(y))
            .enumerate()
            .find(|(_, (u, v))| u != v)
            .map(|(i, (u, v))| parted(u, v, format!("{at}[{i}]")))
            .unwrap_or_default(),
        _ => format!("{at} holds {a} / {b}"),
    }
}

/// Every position of `store`, and the head.
async fn every(store: &dyn LakeStore) -> Result<Vec<Option<Position>>, Failure> {
    let at = positions(store).await?;
    Ok(at.into_iter().map(Some).chain([None]).collect())
}

/// Forget crm by the preflight's report: the preflight's way and the
/// act's.
async fn forget(store: &dyn LakeStore) -> Result<(ForgetWay, ForgetWay), Failure> {
    let crm = SourceId::from("crm");
    let report = store.forget_preflight(&crm).await.must("the preflight")?;
    let done = store.forget_source(&crm, Some(&report.report_id)).await;
    Ok((report.way, done.must("forgetting crm")?.way))
}

/// Whether `way` names the rebuild, saying `why` among its reasons.
fn rebuilds(way: &ForgetWay, why: &str) -> bool {
    !way.scoped && way.why.iter().any(|w| w.contains(why))
}

const KEYS: [&str; 5] = ["p1", "p2", "p3", "p4", "p5"];

async fn a_scoped_refold_answers_as_the_rebuild_at_every_position(
    store: Store,
    given: Given,
) -> Held {
    let lake = store.lake().clone();
    let (ka, kb) = (Arc::new(Keys::default()), Arc::new(Keys::default()));
    let forced = Limits {
        forget_rebuild: true,
        ..limits()
    };
    let rebuilt = (given.relimit)(store, with(&ka), forced).await;
    let rebuilt = rebuilt.map_err(opened("lake"))?;
    let scoped = (given.twin)(lake, with(&kb))
        .await
        .map_err(opened("twin"))?;
    for s in [&rebuilt, &scoped] {
        let l = Lake::of(s.as_ref());
        declared(&l).await?;
        written(&l).await?;
    }
    let (asked, done) = forget(rebuilt.as_ref()).await?;
    let host = "limits.forget_rebuild";
    ensure!(
        rebuilds(&asked, host) && rebuilds(&done, host),
        "with `limits.forget_rebuild` the preflight and the act name the rebuild the host asks \
         for: {asked:?}, {done:?}"
    );
    let (asked, done) = forget(scoped.as_ref()).await?;
    ensure!(
        asked.scoped
            && done.scoped
            && asked.records == 4
            && asked.events > 0
            && asked.scoped_ms <= asked.rebuild_ms
            && asked.why.is_empty(),
        "the preflight and the act name the scoped refold of crm's four records, its events and \
         both expected times: {asked:?}, {done:?}"
    );
    let at = every(scoped.as_ref()).await?;
    let want = reads(rebuilt.as_ref(), &KEYS, &every(rebuilt.as_ref()).await?).await?;
    let got = reads(scoped.as_ref(), &KEYS, &at).await?;
    let differ = apart(&want, &got);
    ensure!(
        differ.is_empty() && got.len() == want.len(),
        "the twin that refolded crm's records answers as the one that rebuilt, at every \
         position; {} of {} reads differ, as (rebuilt / scoped): {:?}",
        differ.len(),
        want.len(),
        &differ[..differ.len().min(4)]
    );
    // Both lose crm's values: `p2` keeps its note and erp's city, `p4`
    // erp's name, and `p1` is retired.
    let l = Lake::of(scoped.as_ref());
    let p2 = l.read("person", "p2").await?;
    let p4 = l.read("person", "p4").await?;
    let p1 = l
        .cat
        .get(&l.own, &"person".into(), get_at("p1", None))
        .await;
    let erased = p2.meta.get("name").is_some_and(|m| m.erased);
    ensure!(
        erased
            && !p2.fields.contains_key("loud")
            && p2.fields.get("city") == Some(&json!("Bonn"))
            && p4.fields.get("name") == Some(&json!("Di"))
            && p1.is_err(),
        "after the act `p2` reads its name erased, `p4` erp's name and `p1` retired: {p2:?}, \
         {p4:?}, {p1:?}"
    );
    scoped.rebuild().await.must("rebuilding the twin")?;
    let again = reads(scoped.as_ref(), &KEYS, &at).await?;
    let differ = apart(&got, &again);
    ensure!(
        differ.is_empty(),
        "a rebuild after the scoped refold answers the same; {} reads differ: {:?}",
        differ.len(),
        &differ[..differ.len().min(4)]
    );
    Ok(())
}

/// What reads `person`'s records beyond their states, each declared after
/// `person`, and what the act says of it.
fn readers() -> [(&'static str, Vec<Value>, Option<&'static str>); 6] {
    let text = json!({"kind": "text"});
    let view = json!({"source": "person", "group_by": ["name"],
                      "measures": [{"name": "n", "expr": "count()"}]});
    let by_name = json!({"declare": "type", "name": "by_name", "key": ["name"], "view": view,
        "fields": [{"name": "name", "type": text}, {"name": "n", "type": {"kind": "number"}}]});
    let mut vectors = people(false);
    vectors.fields.push(vector("v", 2));
    let mut linked = serde_json::to_value(people(false)).unwrap_or_default();
    linked["relations"] = json!([{"name": "knows", "target": "order"}]);
    let visit = json!({"declare": "type", "name": "visit", "fields": [
        {"name": "who_id", "type": text},
        {"name": "who", "type": {"kind": "reference", "target": "person"},
         "authority": "derived", "derived": "who_id"}]});
    let set = json!({"declare": "object_set", "name": "placed", "of": "person",
                     "where": "city = 'Rome'"});
    let reference = |name: &str, target: &str| json!({"name": name, "type": {"kind": "reference", "target": target}});
    let stay = json!({"declare": "type", "name": "stay", "fields": [reference("who", "person")]});
    let trip = json!({"declare": "type", "name": "trip", "fields": [reference("on", "stay")]});
    let far = json!({"declare": "object_set", "name": "far", "of": "trip",
                     "where": "on.who.city = 'Rome'"});
    [
        ("the kept view `by_name`", vec![by_name], Some("by_name")),
        ("the set `placed`", vec![set], None),
        (
            "`person` keeps vectors",
            vec![serde_json::to_value(vectors).unwrap_or_default()],
            None,
        ),
        ("`visit` computes a field", vec![visit], None),
        (
            "`person` has a relation carried by link records",
            vec![linked],
            None,
        ),
        ("the set `far` over `trip`", vec![stay, trip, far], None),
    ]
}

async fn a_forgetting_whose_records_are_read_beyond_their_states_rebuilds(
    store: Store,
    given: Given,
) -> Held {
    let lake = store.lake().clone();
    let mut own = Some(store);
    for (why, decls, kept) in readers() {
        let keys = Arc::new(Keys::default());
        let store = match own.take() {
            Some(store) => (given.reopen)(store, with(&keys)).await,
            None => (given.twin)(lake.clone(), with(&keys)).await,
        };
        let store = store.map_err(opened("lake"))?;
        let l = Lake::of(store.as_ref());
        declared(&l).await?;
        for d in decls {
            match d.get("declare") {
                Some(_) => l.declare_json(d).await.map(drop)?,
                None => {
                    let t: TypeDecl = serde_json::from_value(d).expect("a type reads");
                    l.migrate(t).await.map(drop)?
                }
            }
        }
        if let Some(view) = kept {
            l.set(&format!("view_kept.{view}"), json!(true)).await?;
        }
        written(&l).await?;
        if why.starts_with("`visit`") {
            l.upsert("visit", "v1", json!({"who_id": "p2"})).await?;
        }
        if why.starts_with("the set `far`") {
            l.upsert("stay", "s1", json!({"who": "p1"})).await?;
            l.upsert("trip", "t1", json!({"on": "s1"})).await?;
        }
        let (asked, done) = forget(store.as_ref()).await?;
        ensure!(
            rebuilds(&asked, why) && rebuilds(&done, why),
            "where {why} reads crm's records, the preflight and the act name the rebuild, saying \
             so: {asked:?}, {done:?}"
        );
        let at = every(store.as_ref()).await?;
        let got = reads(store.as_ref(), &KEYS, &at).await?;
        store.rebuild().await.must("rebuilding")?;
        let again = reads(store.as_ref(), &KEYS, &at).await?;
        let differ = apart(&got, &again);
        ensure!(
            differ.is_empty(),
            "where {why} reads crm's records, the act answers as a rebuild; {} reads differ: {:?}",
            differ.len(),
            &differ[..differ.len().min(4)]
        );
    }
    Ok(())
}

/// How many records crm writes in the lake whose refold stops: more than
/// one page holds.
const MANY: usize = 300;

async fn a_scoped_refold_stopped_between_pages_is_finished_by_the_next_open(
    store: Store,
    given: Given,
) -> Held {
    let keys = Arc::new(Keys::default());
    let fails: Fails = Arc::default();
    let fault = |store: Store| (given.fault)(store, with(&keys), failing(&fails));
    let store = fault(store).await.map_err(opened("lake"))?;
    let l = Lake::of(store.as_ref());
    declared(&l).await?;
    let row = |key: String, fields: Value| source_record("person", &key, fields);
    let crm = (0..MANY).map(|i| {
        row(
            format!("c{i}"),
            json!({"id": format!("c{i}"), "name": format!("n{}", i % 10)}),
        )
    });
    l.ingest("crm", crm.collect())
        .await
        .must("ingesting from crm")?;
    let crm_at = store.head().await.must("reading the head")?;
    // Twice as many from erp, a tenth of crm's records co-owned.
    let erp = (0..2 * MANY).map(|i| {
        row(
            format!("e{i}"),
            json!({"id": format!("e{i}"), "name": format!("m{}", i % 10)}),
        )
    });
    l.ingest("erp", erp.collect())
        .await
        .must("ingesting from erp")?;
    let co = (0..MANY / 10).map(|i| {
        row(
            format!("c{i}"),
            json!({"id": format!("c{i}"), "city": "Rome"}),
        )
    });
    l.ingest("erp", co.collect())
        .await
        .must("ingesting from erp")?;
    let before = store.head().await.must("reading the head")?;
    let sample = ["c0", "c1", "c40", "c255", "c256", "c299", "e0"];
    let at = [Some(crm_at), Some(before), None];
    let crm = SourceId::from("crm");
    // The second page's commit fails, after the first page's.
    fails[5].store(2, Ordering::SeqCst);
    let stopped = store.forget_source(&crm, None).await;
    let first = fails[6].load(Ordering::SeqCst) as usize;
    ensure!(
        stopped.is_err() && fails[5].load(Ordering::SeqCst) == 0 && first == MANY,
        "the act stopped at its second page's commit answers its failure, both pages dropped: \
         it answered {stopped:?}, {first} records dropped"
    );
    let dee = source_record("person", "c999", json!({"id": "c999", "name": "Dee"}));
    refused_like(
        "an ingest from crm while its act is owed",
        l.ingest("crm", vec![dee]).await,
        &json!({"code": "source_forgotten", "source": "crm"}),
    )?;
    let head = store.head().await.must("reading the head")?;
    let store = fault(store).await.map_err(opened("lake"))?;
    let refolded = fails[6].load(Ordering::SeqCst) as usize - first;
    let opened_at = store.head().await.must("reading the head")?;
    let l = Lake::of(store.as_ref());
    let again = store
        .forget_source(&crm, None)
        .await
        .must("forgetting crm again")?;
    let after = store.head().await.must("reading the head")?;
    ensure!(
        refolded == MANY && again.types.is_empty() && after == opened_at && head <= opened_at,
        "the next open refolds all {MANY} of crm's records, after which the act is done and \
         appends nothing: {refolded} refolded, the act answered {again:?}, the head {opened_at} \
         to {after}"
    );
    let c1 = l.read("person", "c1").await?;
    ensure!(
        c1.meta.get("name").is_some_and(|m| m.erased),
        "after the open, co-owned `c1` reads its name erased: {c1:?}"
    );
    let got = reads(store.as_ref(), &sample, &at).await?;
    store.rebuild().await.must("rebuilding")?;
    let want = reads(store.as_ref(), &sample, &at).await?;
    let differ = apart(&want, &got);
    ensure!(
        differ.is_empty(),
        "the act the open finished answers as a rebuild; {} reads differ, as (rebuilt / \
         scoped): {:?}",
        differ.len(),
        &differ[..differ.len().min(4)]
    );
    let dropped = fails[6].load(Ordering::SeqCst) as usize;
    ensure!(
        dropped == first + MANY,
        "the rebuild drops no key's states: {dropped} dropped in all"
    );
    Ok(())
}

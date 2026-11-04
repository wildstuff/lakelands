//! A forgetting refolds only the records its source wrote where nothing
//! else reads them (`docs/decisions/058-a-source-may-be-forgotten.md`),
//! and answers every read as the rebuild does. Twin lakes are fed one
//! log; one forgets by the scoped refold and the other by the rebuild its
//! host asks for, and each read is compared at every position.

use lakelands_api::read::{Find, ReadAt};
use lakelands_api::write::ForgetWay;
use lakelands_api::{DeliveryId, IngestOpts};

use super::erasable::{Keys, person, with};
use super::forgotten::{Fails, failing};
use crate::conformance::fixture::{get_at, order, source_record};
use crate::conformance::prelude::*;

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
    "058, extended (lakelands#1120, 099): a scoped refold restates one page of records in each transaction; one stopped after its first page leaves the source's ingest refused, and the road beside the writer's next open refolds every page again, appending nothing, after which the act is done and the lake answers as it does after a rebuild",
    given
);

check!(
    RESTATE_SCOPED_FOR_A_REMOVAL =
        a_value_another_delivery_keeps_reads_erased_through_a_scoped_restate,
    "099: `crm` writes `p1` in its first delivery and `p2` in its second, both under its one version, and `erp` writes `p1`'s city; taking the first back takes `p1`'s name alone, the version staying for `p2`'s, and `p1` reads its name erased after the act and after a writer opens again, keeping `erp`'s city; forgetting `erp` then names the scoped refold in its preflight and its act, though the lake holds an act of removal, and the page refolds `p1` under the older act with its name erased still, leaving `p1` with neither value and `p2`'s name whole; a rebuild after the act answers the same",
    reopens
);

check!(
    RESTATE_TELLS_EVERY_ACT = a_scoped_pass_after_a_reopen_leaves_every_older_acts_values_erased,
    "099: `crm`'s two deliveries seal under its one version and `erp` co-writes `p1`, which the act of taking `crm`'s first delivery back redacts, and a writer opens again; a scoped refold of `erp` then names the scoped refold in its preflight and its act, and the page that restates `p1` folds it under the older act, whose value stays erased, as every act the lists hold is told before the first page; a rebuild after the act answers the same",
    reopens
);

check!(
    RESTATE_WRITES_GO_ON = a_write_between_two_pages_is_answered_before_the_act_is_whole,
    "099: a scoped restate restates a page of records in each turn of the writer; with a lake whose records take more pages than one, a write another source sends once the act's `act_whole` is in the log, its restate owed, is answered between two pages, before the act is whole, and the lake holds it after the act: the record it wrote reads whole, and the forgotten source's records read their names erased",
    reopens
);

check!(
    RESTATE_PAGE_OVER_ITS_BOUND = a_page_over_page_events_takes_the_rebuild,
    "099: with `limits.page_events` below what a page of a migrated lake folds, `forget_preflight` and `forget_source` name the rebuild, saying the page refolds more events than the limit, and the act answers after it as a rebuild does",
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

/// `person` with `city` erasable too, so two sources each hold an
/// erasable value on one record.
fn people_both() -> TypeDecl {
    let mut t = person();
    for f in t.fields.iter_mut().filter(|f| f.name == "city") {
        f.erasable = true;
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
                with_meta: true,
                at,
                ..searched(ty.clone(), SearchMode::lexical(text), 20)
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
    // The open leaves the act to the road beside it: poll, bounded, for
    // the refold it runs before any verb is called.
    settles(6_000, Duration::from_millis(10), || async {
        let refolded = fails[6].load(Ordering::SeqCst) as usize - first;
        ensure!(
            refolded == MANY,
            "the road beside the next open has refolded {refolded} of crm's {MANY} records"
        );
        Ok(())
    })
    .await?;
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
        "the road beside the next open refolds all {MANY} of crm's records, after which the act \
         is done and appends nothing: {refolded} refolded, the act answered {again:?}, the head \
         {opened_at} to {after}"
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

/// Feed one record from `source` as delivery `id` of `l`'s lake.
async fn delivered_as(l: &Lake, source: &str, id: &str, record: SourceRecord) -> Held {
    let opts = IngestOpts {
        delivery: Some(DeliveryId::from(id)),
        ..IngestOpts::default()
    };
    let source = SourceId::from(source);
    let fed = lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &source, opts, vec![record]);
    fed.await.must("ingesting a delivery")?;
    Ok(())
}

async fn a_value_another_delivery_keeps_reads_erased_through_a_scoped_restate(
    store: Store,
    reopen: Reopen,
) -> Held {
    use super::erasable::again;
    use super::removed::{at, erased};

    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(people_both()).await?;
    let row = |fields: Value| source_record("person", "p1", fields);
    delivered_as(&l, "crm", "crm-1", row(json!({"id": "p1", "name": "Ada"}))).await?;
    let bob = source_record("person", "p2", json!({"id": "p2", "name": "Bob"}));
    delivered_as(&l, "crm", "crm-2", bob).await?;
    delivered_as(&l, "erp", "e1", row(json!({"id": "p1", "city": "Oslo"}))).await?;
    let crm: SourceId = "crm".into();
    let done = store
        .remove_values(&crm, &Selection::Delivery("crm-1".into()), false, None)
        .await
        .must("removing crm's first delivery")?;
    ensure!(
        done.keys.is_empty() && done.unshredded == 1,
        "taking `crm-1` back keeps the version `p2`'s name is under and redacts `p1`'s name \
         alone: {done:?}"
    );
    let p1 = at(&l, "person", "p1", None).await?;
    let p2 = at(&l, "person", "p2", None).await?;
    ensure!(
        p1.as_ref().is_some_and(
            |r| erased(r, "name", "crm") && r.fields.get("city") == Some(&json!("Oslo"))
        ) && p2
            .as_ref()
            .is_some_and(|r| r.fields.get("name") == Some(&json!("Bob"))),
        "after the act `p1` reads its name erased and erp's city, and `p2` reads Bob: \
         {p1:?}, {p2:?}"
    );
    // A writer opens again, and erp's forgetting refolds `p1`, which the
    // older act took.
    let store = again(&reopen, store, with(&keys), "after the act").await?;
    let l = Lake::of(store.as_ref());
    let p1 = at(&l, "person", "p1", None).await?;
    ensure!(
        p1.as_ref().is_some_and(|r| erased(r, "name", "crm")),
        "a writer that opened again reads `p1`'s name erased: {p1:?}"
    );
    let erp: SourceId = "erp".into();
    let asked = store.forget_preflight(&erp).await.must("the preflight")?;
    let done = store
        .forget_source(&erp, Some(&asked.report_id))
        .await
        .must("forgetting erp")?;
    ensure!(
        asked.way.scoped && done.way.scoped,
        "the preflight and the act name the scoped refold, though the lake holds an act of \
         removal: {asked:?}, {done:?}"
    );
    let p1 = at(&l, "person", "p1", None).await?;
    let p2 = at(&l, "person", "p2", None).await?;
    ensure!(
        p1.as_ref()
            .is_some_and(|r| erased(r, "name", "crm") && erased(r, "city", "erp"))
            && p2
                .as_ref()
                .is_some_and(|r| r.fields.get("name") == Some(&json!("Bob"))),
        "the scoped refold leaves `p1` with neither value, its name erased by the older act \
         and its city by erp's, and `p2`'s name whole: {p1:?}, {p2:?}"
    );
    let read = ["p1", "p2"];
    let got = reads(store.as_ref(), &read, &[None]).await?;
    store.rebuild().await.must("rebuilding")?;
    let want = reads(store.as_ref(), &read, &[None]).await?;
    let differ = apart(&got, &want);
    ensure!(
        differ.is_empty(),
        "a rebuild after the act answers the same; {} reads differ: {:?}",
        differ.len(),
        &differ[..differ.len().min(4)]
    );
    Ok(())
}

async fn a_scoped_pass_after_a_reopen_leaves_every_older_acts_values_erased(
    store: Store,
    reopen: Reopen,
) -> Held {
    use super::erasable::again;
    use super::removed::{at, erased};

    let (keys, store) = reopen_with_keys(&reopen, store, "with a key store").await?;
    let l = Lake::of(store.as_ref());
    l.declare(people_both()).await?;
    let p = |key: &str, name: &str| source_record("person", key, json!({"id": key, "name": name}));
    // crm's two deliveries seal under its one version, which the first
    // act redacts; web's one delivery seals under a version the second
    // destroys.
    delivered_as(&l, "crm", "crm-1", p("p1", "Ada")).await?;
    delivered_as(&l, "crm", "crm-2", p("p2", "Bob")).await?;
    delivered_as(&l, "web", "w1", p("p3", "Cy")).await?;
    let coast =
        |key: &str, city: &str| source_record("person", key, json!({"id": key, "city": city}));
    delivered_as(&l, "erp", "e1", coast("p1", "Oslo")).await?;
    delivered_as(&l, "erp", "e2", coast("p3", "Rome")).await?;
    let crm: SourceId = "crm".into();
    let web: SourceId = "web".into();
    store
        .remove_values(&crm, &Selection::Delivery("crm-1".into()), false, None)
        .await
        .must("removing crm's first delivery")?;
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    let destroyed = store
        .remove_values(&web, &all, false, None)
        .await
        .must("removing all web wrote")?;
    ensure!(
        destroyed.keys.len() == 1,
        "taking back all web wrote destroys its version: {destroyed:?}"
    );
    // A writer opens again, and a scoped pass over erp restates `p1` and
    // `p3`, which the older acts took.
    let store = again(&reopen, store, with(&keys), "after the acts").await?;
    let l = Lake::of(store.as_ref());
    let erp: SourceId = "erp".into();
    let asked = store.forget_preflight(&erp).await.must("the preflight")?;
    let done = store
        .forget_source(&erp, Some(&asked.report_id))
        .await
        .must("forgetting erp")?;
    ensure!(
        asked.way.scoped && done.way.scoped,
        "the preflight and the act name the scoped refold, though the lake holds two acts \
         of removal: {asked:?}, {done:?}"
    );
    let p1 = at(&l, "person", "p1", None).await?;
    let p3 = at(&l, "person", "p3", None).await?;
    ensure!(
        p1.as_ref()
            .is_some_and(|r| erased(r, "name", "crm") && erased(r, "city", "erp"))
            && p3
                .as_ref()
                .is_some_and(|r| erased(r, "name", "web") && erased(r, "city", "erp")),
        "the page refolds `p1` under the act that redacted it and `p3` under the one that \
         destroyed its version, each name erased: {p1:?}, {p3:?}"
    );
    let read = ["p1", "p2", "p3"];
    let got = reads(store.as_ref(), &read, &[None]).await?;
    store.rebuild().await.must("rebuilding")?;
    let want = reads(store.as_ref(), &read, &[None]).await?;
    let differ = apart(&got, &want);
    ensure!(
        differ.is_empty(),
        "a rebuild after the act answers the same; {} reads differ: {:?}",
        differ.len(),
        &differ[..differ.len().min(4)]
    );
    Ok(())
}

async fn a_write_between_two_pages_is_answered_before_the_act_is_whole(
    store: Store,
    reopen: Reopen,
) -> Held {
    use super::removed::{Gate, Opens, at, erased};

    /// More records than one page holds, in enough pages that several
    /// turns follow the one the act's `source_forgotten` commits in.
    const WRITTEN: usize = 800;
    /// The records the lake holds beside them, so the scoped pass is the
    /// cheaper way: a rebuild would fold all of these again.
    const BESIDE: usize = 2_400;
    let (keys, gate) = (Arc::new(Keys::default()), Arc::new(Gate::default()));
    let seams = Seams {
        meter: Some(gate.clone()),
        ..with(&keys)
    };
    let what = "with a key store and a gate on its writer";
    let store = super::erasable::again(&reopen, store, seams, what).await?;
    let _opens = Opens(gate.clone());
    let l = Lake::of(store.as_ref());
    l.declare(people_both()).await?;
    let row =
        |key: &str, name: &str| source_record("person", key, json!({"id": key, "name": name}));
    let beside = (0..BESIDE).map(|i| row(&format!("e{i:04}"), &format!("m{}", i % 10)));
    l.ingest("erp", beside.collect())
        .await
        .must("ingesting from erp")?;
    let crm = (0..WRITTEN).map(|i| row(&format!("c{i:04}"), &format!("n{}", i % 10)));
    l.ingest("crm", crm.collect())
        .await
        .must("ingesting from crm")?;
    // One of crm's records erp co-writes, so it is restated and stays.
    let co = source_record("person", "c0000", json!({"id": "c0000", "city": "Oslo"}));
    l.ingest("erp", vec![co]).await.must("co-writing c0000")?;
    let start = store.head().await.must("reading the head")?;
    let crm: SourceId = "crm".into();
    let answered = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
    let done: Arc<std::sync::Mutex<Option<lakelands_api::write::ForgetReport>>> = Arc::default();
    let (order, slot) = (answered.clone(), done.clone());
    let act = {
        let store = store.clone();
        let crm = crm.clone();
        async move {
            let report = store
                .forget_source(&crm, None)
                .await
                .must("forgetting crm")?;
            *crate::lock(&slot) = Some(report);
            crate::lock(&order).push("act");
            Ok(())
        }
    };
    // The order is the lake's, not the scheduler's: the gate holds each of
    // the writer's turns and lets them through one at a time until the
    // act's appends, which end with `act_whole`, are in the log and the
    // turn held next is the act's, owed its pages; the write is sent only
    // then, and the gate opens once it is queued behind that turn, so it
    // is answered before the next page, which the act queues only when the
    // turn held has ended.
    gate.arm();
    let steps = {
        let (store, gate) = (store.clone(), gate.clone());
        async move {
            loop {
                gate.reaches("a turn of the writer held", |g| g.holds)
                    .await?;
                let events = events_after(store.as_ref(), start).await?;
                if (events.iter()).any(|e| e.body.kind() == "act_whole") {
                    break;
                }
                let passed = gate.now().passed;
                gate.let_one_through();
                gate.reaches("the turn held let through", |g| g.passed > passed)
                    .await?;
            }
            gate.watch();
            let behind = gate
                .reaches("the write queued behind the act's turn", |g| g.behind)
                .await;
            gate.open();
            behind
        }
    };
    let write = {
        let (store, gate) = (store.clone(), gate.clone());
        let order = answered.clone();
        async move {
            gate.reaches("the act owed its pages, its turn held", |g| g.watching)
                .await?;
            let l = Lake::of(store.as_ref());
            l.ingest(
                "erp",
                vec![source_record(
                    "person",
                    "z1",
                    json!({"id": "z1", "name": "Zed"}),
                )],
            )
            .await
            .must("ingesting from erp between two pages")?;
            crate::lock(&order).push("write");
            Ok(())
        }
    };
    let callers: Vec<Caller<'static, Held>> = vec![Box::pin(act), Box::pin(steps), Box::pin(write)];
    join_all(callers).await.into_iter().collect::<Held>()?;
    let order = crate::lock(&answered).clone();
    ensure!(
        order == ["write", "act"],
        "a write sent between two pages must be answered before the act is whole; they \
         answered {order:?}"
    );
    let report = crate::lock(&done).take().expect("the act answered");
    ensure!(
        report.way.scoped && report.way.why.is_empty(),
        "the act takes the scoped refold, a page a turn, so a write waits for one page: {:?}",
        report.way
    );
    let z1 = at(&l, "person", "z1", None).await?;
    ensure!(
        z1.as_ref()
            .is_some_and(|r| r.fields.get("name").is_some_and(|v| v == &json!("Zed"))),
        "the lake holds the record written between the pages, its name whole: {z1:?}"
    );
    let c0 = at(&l, "person", "c0000", None).await?;
    ensure!(
        c0.as_ref().is_some_and(
            |r| erased(r, "name", "crm") && r.fields.get("city") == Some(&json!("Oslo"))
        ),
        "the act still erases crm's name on a record erp co-writes, and keeps erp's city: \
         {c0:?}"
    );
    let c1 = at(&l, "person", "c0001", None).await?;
    ensure!(
        c1.is_none(),
        "the act retires a record crm alone brought: {c1:?}"
    );
    Ok(())
}

async fn a_page_over_page_events_takes_the_rebuild(store: Store, given: Given) -> Held {
    let keys = Arc::new(Keys::default());
    let low = Limits {
        page_events: 1,
        ..limits()
    };
    let store = (given.relimit)(store, with(&keys), low)
        .await
        .map_err(opened("lake"))?;
    let l = Lake::of(store.as_ref());
    declared(&l).await?;
    // A migration the page folds beside the records.
    l.migrate(people(true)).await?;
    let ada = source_record("person", "p1", json!({"id": "p1", "name": "Ada"}));
    let bob = source_record("person", "p2", json!({"id": "p2", "name": "Bob"}));
    l.ingest("crm", vec![ada, bob])
        .await
        .must("ingesting from crm")?;
    let crm: SourceId = "crm".into();
    let asked = store.forget_preflight(&crm).await.must("the preflight")?;
    let done = store
        .forget_source(&crm, Some(&asked.report_id))
        .await
        .must("forgetting crm")?;
    ensure!(
        rebuilds(&asked.way, "limits.page_events") && rebuilds(&done.way, "limits.page_events"),
        "with `limits.page_events` below the page's events, the preflight and the act name \
         the rebuild, saying so: {asked:?}, {done:?}"
    );
    let read = ["p1", "p2"];
    let got = reads(store.as_ref(), &read, &[None]).await?;
    store.rebuild().await.must("rebuilding")?;
    let want = reads(store.as_ref(), &read, &[None]).await?;
    let differ = apart(&got, &want);
    ensure!(
        differ.is_empty(),
        "the act answers after the rebuild as a rebuild does; {} reads differ: {:?}",
        differ.len(),
        &differ[..differ.len().min(4)]
    );
    Ok(())
}

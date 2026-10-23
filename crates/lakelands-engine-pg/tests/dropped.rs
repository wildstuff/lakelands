//! A served lake recovers when its connections are dropped under it
//! (lakelands#979): an operator's `pg_terminate_backend`, a server's
//! restart, a pooler's or a firewall's cut. A read opens a closed
//! connection again, once, and runs again; the writer, whose lease lived
//! in the session that ended, takes the lease again before its next write,
//! as an open does, with a greater fence, or is refused with the lease's
//! situation while another writer holds it.

mod common;

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::error::{Act, Situation};
use lakelands_api::event::EventBody;
use lakelands_api::principal::{Grants, PrincipalKind};
use lakelands_api::read::{Find, Found};
use lakelands_api::seam::Seams;
use lakelands_api::write::{Declaration, WriteOpts};
use lakelands_api::{Catalog, Error, LakeStore, Principal, WriteContext};
use lakelands_engine_pg::PgOpener;
use serde_json::json;

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    }
}

fn owner(lake: &str) -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: lake.into(),
        grants: Grants::owner(),
    }
}

/// Ends every session the lake's handles hold: the lease and the
/// projection, named by the lake's schema, and the readers this process's
/// lakes share, named by the process. Answers how many of each it ended.
async fn drop_connections(url: &str, schema: &str) -> (usize, usize) {
    let (url, schema) = (url.to_owned(), schema.to_owned());
    tokio::task::spawn_blocking(move || dropped(&url, &schema))
        .await
        .unwrap()
}

/// Drops the schema, off the runtime: the admin client blocks.
async fn gone(url: &str, schema: &str) {
    let (url, schema) = (url.to_owned(), schema.to_owned());
    tokio::task::spawn_blocking(move || common::drop_schema(&url, &schema))
        .await
        .unwrap();
}

fn dropped(url: &str, schema: &str) -> (usize, usize) {
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    let ended = client
        .query(
            "SELECT application_name = $1, pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE application_name IN ($1, $2)",
            &[
                &format!("lakelands {schema}"),
                &format!("lakelands readers {}", std::process::id()),
            ],
        )
        .unwrap();
    // The server ends a session a moment after it is asked.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let lakes = ended.iter().filter(|row| row.get::<_, bool>(0)).count();
    (lakes, ended.len() - lakes)
}

async fn put(cat: &dyn Catalog, who: &Principal, key: &str) -> Result<(), Error> {
    put_in(cat, who, "note", key).await
}

async fn put_in(cat: &dyn Catalog, who: &Principal, ty: &str, key: &str) -> Result<(), Error> {
    let fields = serde_json::from_value(json!({"n": key})).unwrap();
    cat.upsert(who, &ty.into(), &key.into(), fields, WriteOpts::default())
        .await
        .map(drop)
}

async fn keys(cat: &dyn Catalog, who: &Principal) -> Result<Vec<String>, Error> {
    let Found::Records { rows, .. } = cat.find(who, &"note".into(), Find::default()).await?.data
    else {
        panic!("records");
    };
    Ok(rows.into_iter().map(|r| r.key.to_string()).collect())
}

/// The lake's log, as the feed reads it from the engine.
async fn log(store: &dyn LakeStore) -> Vec<lakelands_api::event::Event> {
    let feed = store.feed();
    let batch = feed
        .poll(lakelands_api::Position(0), &Default::default(), 10_000)
        .await
        .unwrap();
    batch.events
}

/// The position of the log's last event.
async fn tip(store: &dyn LakeStore) -> Option<lakelands_api::Position> {
    log(store).await.last().map(|e| e.position)
}

/// The fences of the `lease_taken` events in the lake's log.
async fn fences(store: &dyn LakeStore) -> Vec<u64> {
    let events = log(store).await;
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::LeaseTaken { fence, .. } => Some(*fence),
            _ => None,
        })
        .collect()
}

/// Declares `ty` with one text field `n`.
async fn declare(cat: &dyn Catalog, who: &Principal, ty: &str) {
    let d =
        json!({"declare": "type", "name": ty, "fields": [{"name": "n", "type": {"kind": "text"}}]});
    let d: Declaration = serde_json::from_value(d).unwrap();
    cat.declare(who, d).await.unwrap();
}

/// Whether `answer` is the lease's situation: held by another writer or
/// its lock taken.
fn the_lease_elsewhere<T: std::fmt::Debug>(answer: &Result<T, Error>) -> bool {
    let lease = answer.as_ref().err().and_then(Error::situation);
    matches!(
        lease,
        Some(Situation::LeaseLocked { .. } | Situation::LeaseHeld { .. })
    )
}

async fn served(url: &str) -> (String, Arc<dyn LakeStore>, Arc<dyn Catalog>, Principal) {
    let schema = common::fresh_schema();
    let location = common::location(url, &schema, &schema);
    let store = lakelands_store::open(location, seams(), OpenMode::Create)
        .await
        .unwrap();
    let cat = store.catalog(WriteContext::default());
    let who = owner(&schema);
    declare(cat.as_ref(), &who, "note").await;
    put(cat.as_ref(), &who, "a").await.unwrap();
    (schema, store, cat, who)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_served_lake_reads_and_writes_again_after_its_connections_are_dropped() {
    let Some(url) = common::url_or_skip("dropped connections") else {
        return;
    };
    let (schema, store, cat, who) = served(&url).await;
    let before = fences(store.as_ref()).await;
    let (lakes, readers) = drop_connections(&url, &schema).await;
    assert_eq!(lakes, 2, "the lease and the projection");
    assert!(readers >= 1, "a reader");
    // A read opens the reader again and is answered.
    assert_eq!(
        keys(cat.as_ref(), &who).await.unwrap(),
        ["a"],
        "a read after the drop"
    );
    // The first write finds the writer's session gone before its `COMMIT`
    // is sent: the lease not held, nothing appended, the log's tip where
    // it was. The writer is lost, and the next write takes the lease again
    // first.
    let at = tip(store.as_ref()).await;
    let first = put(cat.as_ref(), &who, "b").await;
    let undone = first.as_ref().err().and_then(Error::situation);
    assert!(
        matches!(
            undone,
            Some(Situation::LeaseNotHeld {
                undone: Some(Act::Appended),
                ..
            })
        ),
        "the write that finds the session gone is answered the lease not held, nothing appended: {first:?}"
    );
    assert_eq!(tip(store.as_ref()).await, at, "nothing appended");
    put(cat.as_ref(), &who, "b").await.unwrap();
    put(cat.as_ref(), &who, "c").await.unwrap();
    assert_eq!(keys(cat.as_ref(), &who).await.unwrap(), ["a", "b", "c"]);
    let after = fences(store.as_ref()).await;
    assert_eq!(
        after.len(),
        before.len() + 1,
        "one lease_taken more: {before:?} then {after:?}"
    );
    assert!(
        after.last() > before.last(),
        "with a greater fence: {before:?} then {after:?}"
    );
    store.close().await.unwrap();
    gone(&url, &schema).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_writer_is_refused_while_another_holds_the_lease_and_writes_after_it() {
    let Some(url) = common::url_or_skip("dropped connections, a second writer") else {
        return;
    };
    let (schema, store, cat, who) = served(&url).await;
    drop_connections(&url, &schema).await;
    // Another writer takes the lease the dropped session let go of, and
    // declares a type the first writer has never seen.
    let location = common::location(&url, &schema, &schema);
    let second = lakelands_store::open(location, seams(), OpenMode::Write)
        .await
        .unwrap();
    let theirs = second.catalog(WriteContext::default());
    put(theirs.as_ref(), &who, "theirs").await.unwrap();
    declare(theirs.as_ref(), &who, "memo").await;
    let at = tip(store.as_ref()).await;
    // The first write finds the session gone; the second is refused by
    // the lease's situation, as is every one after it.
    let first = put(cat.as_ref(), &who, "mine").await;
    assert!(
        matches!(
            first.as_ref().err().and_then(Error::situation),
            Some(Situation::LeaseNotHeld { .. })
        ),
        "the write that finds the session gone: {first:?}"
    );
    let refused = put(cat.as_ref(), &who, "mine").await;
    assert!(
        the_lease_elsewhere(&refused),
        "while the second writer holds the lease, the first's write is refused with the lease's situation: {refused:?}"
    );
    assert!(
        !store.is_writer(),
        "a handle refused the lease is no writer"
    );
    // Neither a rebuild nor a compaction runs under the second's lease.
    let rebuilt = store.rebuild().await;
    assert!(the_lease_elsewhere(&rebuilt), "the rebuild: {rebuilt:?}");
    let compacted = store.compact().await;
    assert!(
        the_lease_elsewhere(&compacted),
        "the compaction: {compacted:?}"
    );
    // A purge's preflight reads, and takes no lease: it is answered while
    // the second holds it.
    let preflight = store.purge_preflight(&"crm".into(), None).await;
    assert!(preflight.is_ok(), "the purge's preflight: {preflight:?}");
    assert_eq!(tip(store.as_ref()).await, at, "nothing appended");
    assert_eq!(
        keys(theirs.as_ref(), &who).await.unwrap(),
        ["a", "theirs"],
        "the projection the second writer folded is untouched"
    );
    second.close().await.unwrap();
    // The lease is free: the next write takes it, folds what the second
    // writer appended, and is decided over it: a record of the type only
    // the second declared.
    put_in(cat.as_ref(), &who, "memo", "m1").await.unwrap();
    put(cat.as_ref(), &who, "mine").await.unwrap();
    assert!(store.is_writer(), "the re-opened handle writes");
    assert_eq!(
        keys(cat.as_ref(), &who).await.unwrap(),
        ["a", "mine", "theirs"]
    );
    let taken = fences(store.as_ref()).await;
    assert!(
        taken.windows(2).all(|w| w[0] < w[1]) && taken.len() == 3,
        "three leases, each fence greater: {taken:?}"
    );
    store.close().await.unwrap();
    gone(&url, &schema).await;
}

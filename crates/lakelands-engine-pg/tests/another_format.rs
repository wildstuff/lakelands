//! A lake written in another release's format is not opened
//! (lakelands#801,
//! `docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
//!
//! The store asks the opener for the ends of the log before it opens the
//! lake. Here that question reads the log's table and leaves the schema
//! as it was, and makes no schema where there is no lake; and a lake as
//! a later release would leave it, its tip in that release's format and its
//! projection without a layout mark, is refused for its format in every
//! mode and not for its layout, with no table made and no row changed.
//! A lake an earlier release left, its log in a format this release no
//! longer reads and a table it wrote that this build does not know, is
//! refused by its format too, not as a schema that is not a lake's
//! (lakelands#1764). An import onto either lake is answered from the same
//! question, and changes no row of it. A schema 0.23.0 left, its log in
//! format 23, is opened in place: a writer's open carries it up to this
//! release's format, under 0.23.0's layout mark or an older one
//! (lakelands#1824).

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::{CanonicalEvent, EngineOpener, OpenMode};
use lakelands_api::error::{Case, Situation};
use lakelands_api::event::{Digest, EventBody, LOG_FORMAT_VERSION, OLDEST_LOG_FORMAT};
use lakelands_api::seam::Seams;
use lakelands_api::{Location, Position, Timestamp};
use lakelands_core::canonical;
use lakelands_engine_pg::PgOpener;
use lakelands_store::log::{Draft, seal};
use serde_json::json;

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    }
}

/// The schema's tables, each with the number of its rows and the sum of
/// its rows' hashes: what a statement that made or changed anything in
/// it would move. Nothing where the schema is not there.
fn held(url: &str, schema: &str) -> Vec<(String, i64, String)> {
    let mut db = postgres::Client::connect(url, postgres::NoTls).unwrap();
    let tables = db.query(
        "SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = $1 ORDER BY 1",
        &[&schema],
    );
    let tables = tables.unwrap().into_iter().map(|r| r.get::<_, String>(0));
    tables
        .map(|table| {
            let rows = format!(
                "SELECT count(*), coalesce(sum(hashtext(t::text)::bigint), 0)::text \
                 FROM \"{schema}\".\"{table}\" t"
            );
            let row = db.query_one(&rows, &[]).unwrap();
            (table, row.get(0), row.get(1))
        })
        .collect()
}

/// A lake a store of this release made and closed, in a schema of its
/// own, and an export of it in a directory named as the schema is.
fn made(url: &str, on: &tokio::runtime::Runtime) -> (Location, String, PathBuf) {
    let schema = common::fresh_schema();
    let location = common::location(url, &schema, &schema);
    let store = on.block_on(lakelands_store::open(
        location.clone(),
        seams(),
        OpenMode::Create,
    ));
    let store = store.unwrap();
    let export = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{schema}.export"));
    let _ = std::fs::remove_dir_all(&export);
    on.block_on(lakelands_core::export::write(store.as_ref(), &export))
        .unwrap();
    on.block_on(store.close()).unwrap();
    (location, schema, export)
}

#[test]
fn the_ends_of_a_log_are_read_and_its_schema_is_left_as_it_was() {
    let Some(url) = common::url_or_skip("the ends of a log") else {
        return;
    };
    let on = tokio::runtime::Runtime::new().unwrap();
    let (location, schema, export) = made(&url, &on);
    let before = held(&url, &schema);
    let (first, last) = PgOpener.log_ends(&location).unwrap().expect("a lake");
    assert_eq!(
        held(&url, &schema),
        before,
        "reading the ends changed a row"
    );
    assert_eq!(first.position, Position(0));
    // An import onto it is told what it holds, and opens nothing of it.
    let e = on.block_on(lakelands_store::import(
        location.clone(),
        seams(),
        export.clone(),
    ));
    let e = e.expect_err("an import onto a lake that holds events was taken");
    assert!(
        matches!(e.case().map(|s| &s.case), Some(Case::ImportNotEmpty { head, .. }) if *head == last.position),
        "{e:?}"
    );
    assert_eq!(held(&url, &schema), before, "an import changed a row");
    let engine = PgOpener.open(&location, OpenMode::Read).unwrap();
    assert_eq!(engine.tip().unwrap(), Some((last.position, last.hash)));
    drop(engine);
    // Where there is no lake there is none after: no schema is made.
    let nowhere = common::fresh_schema();
    let asked = PgOpener.log_ends(&common::location(&url, &nowhere, &nowhere));
    assert!(asked.unwrap().is_none());
    let mut db = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    let there = "SELECT count(*) FROM pg_catalog.pg_namespace WHERE nspname = $1";
    let there: i64 = db.query_one(there, &[&nowhere]).unwrap().get(0);
    assert_eq!(there, 0, "asking for a log's ends made a schema");
    common::drop_schema(&url, &schema);
    let _ = std::fs::remove_dir_all(&export);
}

/// The same from both sides. A lake a later release left — its tip in
/// that release's format, its projection without a layout mark (that
/// release made none), and a table it made that this build does not know
/// — is refused for its format in every mode, and not for its layout. A
/// lake a release before this one left, its log in the format before
/// this release's and a table it wrote that this build does not know
/// (lakelands#1764), is refused by its format too, not as a schema that
/// is not a lake's. An import onto either is answered from the same
/// question, and changes no row of it.
#[test]
fn a_lake_of_another_release_is_refused_for_its_format_and_not_touched() {
    let Some(url) = common::url_or_skip("a lake of another release") else {
        return;
    };
    let reads = LOG_FORMAT_VERSION;
    let on = tokio::runtime::Runtime::new().unwrap();
    for (later, found) in [(true, reads + 1), (false, OLDEST_LOG_FORMAT - 1)] {
        let (location, schema, export) = made(&url, &on);
        let mut db = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        if later {
            // Its writer's last event, in the format after this release's.
            let engine = PgOpener.open(&location, OpenMode::Write).unwrap();
            let lease = engine.take_lease("a later release").unwrap();
            let (at, prev) = engine.tip().unwrap().unwrap();
            let (fence, holder) = (lease.fence, "a later release".into());
            let taken = Draft::system(
                Timestamp(i64::MAX / 2),
                EventBody::LeaseTaken { fence, holder },
            );
            let (_, stored) = seal(&location.lake, at.next(), &prev, fence, taken);
            let mut event: serde_json::Value = serde_json::from_slice(&stored.bytes).unwrap();
            event["v"] = json!(found);
            event.as_object_mut().unwrap().remove("hash");
            let hash = canonical::sha256_hex(&event);
            event["hash"] = json!(hash);
            let line = CanonicalEvent {
                hash: Digest(hash),
                bytes: canonical::to_vec(&event),
                ..stored
            };
            engine.append(&lease, &[line]).unwrap();
            engine.release_lease(&lease).unwrap();
            drop(engine);
            // Its projection carries no layout mark: that release made none.
            let unmark = format!("DELETE FROM \"{schema}\".meta WHERE k = 'layout'");
            assert_eq!(db.execute(&unmark, &[]).unwrap(), 1);
        } else {
            // Its writer's log and its format taken back below the oldest
            // this release reads: every line's `v` and the `lake_created`
            // body's `format`, as that release wrote them.
            let older = format!(
                "UPDATE \"{schema}\".log SET bytes = convert_to(replace(replace(convert_from(bytes, 'UTF8'), \
                 '\"v\":{reads}', '\"v\":{found}'), '\"format\":{reads}', '\"format\":{found}'), 'UTF8')"
            );
            db.batch_execute(&older).unwrap();
        }
        // And a table of its own, which this build does not know: the
        // format is what the refusal names, not the table.
        db.batch_execute(&format!("CREATE TABLE \"{schema}\".own (x int)"))
            .unwrap();
        drop(db);
        let before = held(&url, &schema);
        let another = |e: &lakelands_api::Error| {
            matches!(e.situation(), Some(Situation::LogFormat { lake, found: f, reads: r, .. })
                if *lake == schema && *f == u64::from(found) && *r == u64::from(reads))
        };
        for mode in [
            OpenMode::Read,
            OpenMode::Write,
            OpenMode::Create,
            OpenMode::Rebuild,
        ] {
            let opened = on.block_on(lakelands_store::open(location.clone(), seams(), mode));
            let Err(e) = opened else {
                panic!("a lake of another release opened for {mode:?}");
            };
            assert!(another(&e), "{mode:?}: {e:?}");
            assert_eq!(held(&url, &schema), before, "{mode:?} changed the schema");
        }
        let e = on.block_on(lakelands_store::import(
            location.clone(),
            seams(),
            export.clone(),
        ));
        let e = e.expect_err("an import onto a lake of another release was taken");
        assert!(another(&e), "an import: {e:?}");
        assert_eq!(held(&url, &schema), before, "an import changed the schema");
        common::drop_schema(&url, &schema);
        let _ = std::fs::remove_dir_all(&export);
    }
}

/// A schema 0.23.0 left, opened in place by a writer (lakelands#1824).
/// 0.23.0 laid a lake's schema out as this release does: the same tables
/// and columns, under layout mark 13, which it wrote. Its log is the lake
/// 0.23.0 made (`docs/log-format/lakes/23/`), every event in format 23,
/// held as 0.23.0 held it. The writer's open keeps every table, carries the
/// lake up by the `lease_taken` it appends, the log's first event of format
/// 24, and says so; the lake verifies, and the next writer's open carries
/// nothing up. Under an older layout mark, as a projection an older build
/// laid out, the same open rebuilds the projection, says why, and carries
/// the lake up too. (A schema with no mark is no schema 0.23.0 left: it
/// marked its layout, and such a schema is refused by its layout.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_schema_0_23_0_left_is_carried_up_in_place_by_a_writers_open() {
    use lakelands_core::conformance::fixture::log_events;
    let Some(url) = common::url_or_skip("a schema 0.23.0 left") else {
        return;
    };
    let kept = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/lakes/23");
    for marked in [13_i64, 12] {
        let schema = common::fresh_schema();
        let location = common::location(&url, "kept23", &schema);
        // The file keyring the lake shipped with, copied: a keyring writes.
        let ring = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{schema}.keys"));
        let _ = std::fs::remove_dir_all(&ring);
        std::fs::create_dir_all(&ring).unwrap();
        let file = "kept23.keys.json";
        std::fs::copy(kept.join("keys").join(file), ring.join(file)).unwrap();
        let seams = Seams {
            keys: Some(Arc::new(lakelands_backings::keyring::FileKeyring::new(
                &ring,
            ))),
            ..seams()
        };
        // The schema as 0.23.0 left it: its log of format 23, row for row,
        // and its projection folded.
        lakelands_store::import(location.clone(), seams.clone(), kept.clone())
            .await
            .unwrap();
        // The blocking client runs where the runtime lets a worker block.
        use tokio::task::block_in_place as sync;
        let tables = |url: &str| -> Vec<String> {
            sync(|| held(url, &schema).into_iter().map(|t| t.0).collect())
        };
        let before = tables(&url);
        let mark = format!("SELECT n FROM \"{schema}\".meta WHERE k = 'layout'");
        let layout = |url: &str| -> Option<i64> {
            sync(|| {
                let mut db = postgres::Client::connect(url, postgres::NoTls).unwrap();
                db.query_one(&mark, &[]).unwrap().get(0)
            })
        };
        assert_eq!(layout(&url), Some(13), "0.23.0 marked its layout 13");
        if marked != 13 {
            let older = format!("UPDATE \"{schema}\".meta SET n = {marked} WHERE k = 'layout'");
            sync(|| {
                let mut db = postgres::Client::connect(&url, postgres::NoTls).unwrap();
                assert_eq!(db.execute(&older, &[]).unwrap(), 1);
            });
        }
        let store = lakelands_store::open(location.clone(), seams.clone(), OpenMode::Write);
        let store = store.await.unwrap();
        assert_eq!(store.upgraded_at_open(), Some(23), "marked: {marked}");
        assert_eq!(
            store.rebuilt_at_open().is_some(),
            marked != 13,
            "marked: {marked}"
        );
        // While the open's rebuild folds, the feed leaves a reaction it
        // cannot decide yet (the `trigger_fired` at 15) to a later poll:
        // the log is followed until it reaches the head.
        let head = store.head().await.unwrap();
        let asked = std::time::Instant::now();
        let log = loop {
            let log = log_events(store.as_ref()).await.unwrap();
            if log.last().is_some_and(|e| e.position == head) {
                break log;
            }
            assert!(
                asked.elapsed() < std::time::Duration::from_secs(60),
                "marked: {marked}: the feed reaches {head} within a minute"
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        };
        let formats: Vec<_> = log.iter().map(|e| (e.v, e.body.kind())).collect();
        let (tip, below) = formats.split_last().unwrap();
        assert_eq!(*tip, (24, "lease_taken"), "marked: {marked}");
        assert!(below.iter().all(|(v, _)| *v == 23), "{formats:?}");
        store.verify(None, None).await.unwrap();
        store.close().await.unwrap();
        drop(store);
        assert_eq!(tables(&url), before, "the open changed the schema's tables");
        assert_eq!(layout(&url), Some(13), "marked: {marked}");
        let again = lakelands_store::open(location.clone(), seams, OpenMode::Write);
        let again = again.await.unwrap();
        assert_eq!(again.upgraded_at_open(), None, "carried up twice");
        assert_eq!(again.rebuilt_at_open(), None, "rebuilt twice");
        again.close().await.unwrap();
        drop(again);
        sync(|| common::drop_schema(&url, &schema));
        let _ = std::fs::remove_dir_all(&ring);
    }
}

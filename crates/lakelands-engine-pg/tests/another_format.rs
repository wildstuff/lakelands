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
//! An import onto either lake is answered from the same question, and
//! changes no row of it.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::{CanonicalEvent, EngineOpener, OpenMode};
use lakelands_api::error::{Case, Situation};
use lakelands_api::event::{Digest, EventBody, LOG_FORMAT_VERSION};
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

#[test]
fn a_lake_a_later_release_left_is_refused_for_its_format_and_not_touched() {
    let Some(url) = common::url_or_skip("a lake of a later release") else {
        return;
    };
    let reads = LOG_FORMAT_VERSION;
    let on = tokio::runtime::Runtime::new().unwrap();
    let (location, schema, export) = made(&url, &on);
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
    event["v"] = json!(reads + 1);
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
    let mut db = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    let unmark = format!("DELETE FROM \"{schema}\".meta WHERE k = 'layout'");
    assert_eq!(db.execute(&unmark, &[]).unwrap(), 1);
    drop(db);
    let before = held(&url, &schema);
    let another = |e: &lakelands_api::Error| {
        matches!(e.situation(), Some(Situation::LogFormat { lake, found, reads: r, .. })
            if *lake == schema && *found == u64::from(reads + 1) && *r == u64::from(reads))
    };
    for mode in [
        OpenMode::Read,
        OpenMode::Write,
        OpenMode::Create,
        OpenMode::Rebuild,
    ] {
        let opened = on.block_on(lakelands_store::open(location.clone(), seams(), mode));
        let Err(e) = opened else {
            panic!("a lake of a later release opened for {mode:?}");
        };
        assert!(another(&e), "{mode:?}: {e:?}");
        assert_eq!(held(&url, &schema), before, "{mode:?} changed the schema");
    }
    let e = on.block_on(lakelands_store::import(
        location.clone(),
        seams(),
        export.clone(),
    ));
    let e = e.expect_err("an import onto a lake of a later release was taken");
    assert!(another(&e), "an import: {e:?}");
    assert_eq!(held(&url, &schema), before, "an import changed the schema");
    common::drop_schema(&url, &schema);
    let _ = std::fs::remove_dir_all(&export);
}

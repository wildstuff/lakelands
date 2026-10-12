//! Part of a lake without its events is damage (lakelands#865,
//! `docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
//!
//! This engine keeps the log as one table, with the members an index
//! would hold as its columns, so there is no index to lose apart. What
//! can be lost is the table, or its rows. A schema whose `log` table is
//! gone is refused by the question for the log's ends, before an open
//! would make the table again. A `log` table that holds no event under a
//! projection folded through a position is the store's to find: a
//! writer's open, a reader's and an import are refused, and nothing is
//! appended.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::error::{Flaw, Part, Situation};
use lakelands_api::seam::Seams;
use lakelands_api::{Error, LakeStore, Location};
use lakelands_engine_pg::PgOpener;

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    }
}

/// The schema's tables but its lease, each with the number of its rows
/// and a sum over them: what a statement that made a table or changed a
/// row would move.
fn held(url: &str, schema: &str) -> Vec<(String, i64, String)> {
    let mut db = postgres::Client::connect(url, postgres::NoTls).unwrap();
    let tables = db.query(
        "SELECT tablename FROM pg_catalog.pg_tables \
         WHERE schemaname = $1 AND tablename <> 'lease' ORDER BY 1",
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
/// own, and an export of it.
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

/// The doors into the schema: the question for the log's ends, an open
/// to read, to write and to create, and an import.
const DOORS: usize = 5;

/// What one door answers, with its name.
fn knock(
    door: usize,
    on: &tokio::runtime::Runtime,
    location: &Location,
    export: &std::path::Path,
) -> (String, Option<Error>) {
    match door {
        0 => {
            let asked = PgOpener.log_ends(location).map(|_| ());
            ("log_ends".to_owned(), asked.err())
        }
        4 => {
            let imported = lakelands_store::import(location.clone(), seams(), export.to_owned());
            ("an import".to_owned(), on.block_on(imported).err())
        }
        n => {
            let mode = [OpenMode::Read, OpenMode::Write, OpenMode::Create][n - 1];
            let opened = on.block_on(lakelands_store::open(location.clone(), seams(), mode));
            let opened = opened.map(|store: Arc<dyn LakeStore>| drop(store));
            (format!("an open for {mode:?}"), opened.err())
        }
    }
}

#[test]
fn a_schema_whose_log_table_is_gone_is_refused_and_the_table_is_not_made_again() {
    let Some(url) = common::url_or_skip("a lost log table") else {
        return;
    };
    let on = tokio::runtime::Runtime::new().unwrap();
    let (location, schema, export) = made(&url, &on);
    let mut db = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    db.batch_execute(&format!("DROP TABLE \"{schema}\".log"))
        .unwrap();
    let before = held(&url, &schema);
    for door in 0..DOORS {
        let (door, e) = knock(door, &on, &location, &export);
        let e = e.unwrap_or_else(|| panic!("{door} took a lake whose log is gone"));
        assert!(
            matches!(e.situation(), Some(Situation::Storage { .. })),
            "{door}: {e:?}"
        );
        assert_eq!(
            held(&url, &schema),
            before,
            "{door} made or changed a table"
        );
    }
    common::drop_schema(&url, &schema);
    let _ = std::fs::remove_dir_all(&export);
}

#[test]
fn a_projection_ahead_of_its_log_is_refused_and_nothing_is_appended() {
    let Some(url) = common::url_or_skip("an emptied log") else {
        return;
    };
    let on = tokio::runtime::Runtime::new().unwrap();
    let (location, schema, export) = made(&url, &on);
    let mut db = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    db.batch_execute(&format!("DELETE FROM \"{schema}\".log"))
        .unwrap();
    assert!(PgOpener.log_ends(&location).unwrap().is_none());
    let before = held(&url, &schema);
    let ahead = |s: &Situation| {
        matches!(
            s,
            Situation::Damaged {
                part: Part::Log,
                position: None,
                flaw: Flaw::EndsBefore { .. }
            }
        )
    };
    for door in 1..DOORS {
        let (door, e) = knock(door, &on, &location, &export);
        let e = e.unwrap_or_else(|| panic!("{door} took a projection ahead of its log"));
        assert!(e.situation().is_some_and(ahead), "{door}: {e:?}");
        assert_eq!(
            held(&url, &schema),
            before,
            "{door} appended or changed a row"
        );
    }
    common::drop_schema(&url, &schema);
    let _ = std::fs::remove_dir_all(&export);
}

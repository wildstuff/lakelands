//! A rebuild beside the projection, on the PostgreSQL engine
//! (lakelands#1322): a builder killed halfway leaves the projection
//! answering and its scratch schema for the next writer's open to remove;
//! a scratch is built beside projection tables this build does not know.

mod common;

use std::process::Command;

use lakelands_api::engine::{EngineOpener, LakeEngine, Lease, OpenMode};
use lakelands_api::error::Situation;
use lakelands_api::event::Digest;
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_api::{Error, Location};
use lakelands_core::conformance::engine::chain;
use lakelands_engine_pg::PgOpener;

/// Set to a lake's schema, the child this test binary runs as.
const CHILD: &str = "LAKELANDS_TEST_SCRATCH_CHILD";

fn at(url: &str, schema: &str) -> Location {
    common::location(url, "beside", schema)
}

/// Fold `key` at `through`.
fn fold(engine: &dyn LakeEngine, key: &str, through: u64) {
    let p = engine.projection();
    p.begin().unwrap();
    p.put_record(&StoredRecord {
        ty: TypeName::from("t"),
        key: RecordKey::from(key),
        version: Version(through),
        fields: Default::default(),
        meta: Default::default(),
        position: Position(through),
        made: Position(through),
        retired: false,
        restricted: false,
    })
    .unwrap();
    p.commit(Position(through)).unwrap();
}

/// Where `engine` folded through, and whether its head reads `key`.
fn reads(engine: &dyn LakeEngine, key: &str) -> (Option<Position>, bool) {
    let view = engine.view(None).unwrap();
    let got = view.get(&TypeName::from("t"), &RecordKey::from(key));
    (
        engine.projection().folded_through().unwrap(),
        got.unwrap().is_some(),
    )
}

/// A lake of five events with `a` folded at 1, and the writer's lease.
fn lake(url: &str, schema: &str) -> (Box<dyn LakeEngine>, Lease) {
    let writer = PgOpener.open(&at(url, schema), OpenMode::Create).unwrap();
    let lease = writer.take_lease("writer").unwrap();
    writer
        .append(&lease, &chain(0, 5, &Digest::zero(), &lease))
        .unwrap();
    fold(writer.as_ref(), "a", 1);
    (writer, lease)
}

/// Run `sql` on the test server.
fn run(url: &str, sql: String) {
    let url = url.to_owned();
    std::thread::spawn(move || {
        let mut db = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        db.batch_execute(&sql).unwrap();
    })
    .join()
    .unwrap();
}

/// Whether the schema `name` is there.
fn there(url: &str, name: &str) -> bool {
    let (url, name) = (url.to_owned(), name.to_owned());
    std::thread::spawn(move || {
        let mut db = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        let found = db.query_opt("SELECT 1 FROM pg_namespace WHERE nspname = $1", &[&name]);
        found.unwrap().is_some()
    })
    .join()
    .unwrap()
}

/// The child: folds into the scratch and dies before it marks it.
#[test]
fn a_builder_that_dies_halfway() {
    let (Some(schema), Some(url)) = (std::env::var(CHILD).ok(), common::url()) else {
        return;
    };
    let built = PgOpener.open_scratch(&at(&url, &schema)).unwrap();
    fold(built.as_ref(), "b", 3);
    std::process::abort();
}

#[test]
fn a_builder_killed_halfway_leaves_the_projection_answering() {
    let Some(url) = common::url_or_skip("the pg rebuild-beside test") else {
        return;
    };
    let schema = common::fresh_schema();
    let (writer, lease) = lake(&url, &schema);
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "a_builder_that_dies_halfway", "--test-threads=1"])
        .env(CHILD, &schema)
        .output()
        .unwrap();
    assert!(!child.status.success(), "the builder was to die: {child:?}");
    let scratch = common::scratch_of(&schema);
    assert!(there(&url, &scratch), "the builder left its scratch");
    let reader = PgOpener.open(&at(&url, &schema), OpenMode::Read).unwrap();
    for engine in [writer.as_ref(), reader.as_ref()] {
        assert_eq!(reads(engine, "a"), (Some(Position(1)), true));
        assert_eq!(reads(engine, "b"), (Some(Position(1)), false));
    }
    let swapped = writer.swap(&lease);
    assert!(
        matches!(&swapped, Err(Error::Unavailable { situation }) if matches!(**situation, Situation::NoScratch { .. })),
        "{swapped:?}"
    );
    drop(writer);
    let again = PgOpener.open(&at(&url, &schema), OpenMode::Write).unwrap();
    assert!(!there(&url, &scratch), "the next writer's open removes it");
    assert_eq!(reads(again.as_ref(), "a"), (Some(Position(1)), true));
    assert_eq!(reads(reader.as_ref(), "a"), (Some(Position(1)), true));
    drop((again, reader));
    common::drop_schemas(&url);
}

#[test]
fn a_scratch_is_built_beside_projection_tables_this_build_does_not_know() {
    let Some(url) = common::url_or_skip("the pg rebuild-beside test") else {
        return;
    };
    let schema = common::fresh_schema();
    drop(lake(&url, &schema));
    run(
        &url,
        format!(
            "DROP TABLE \"{schema}\".records CASCADE; \
             CREATE TABLE \"{schema}\".of_another_layout (x INTEGER); \
             UPDATE \"{schema}\".meta SET n = n - 1 WHERE k = 'layout'"
        ),
    );
    assert!(
        PgOpener.open(&at(&url, &schema), OpenMode::Write).is_err(),
        "a writer opens a schema with a table it does not know"
    );
    let built = PgOpener.open_scratch(&at(&url, &schema)).unwrap();
    fold(built.as_ref(), "b", 4);
    assert_eq!(built.mark_built().unwrap(), Some(Position(4)));
    assert_eq!(reads(built.as_ref(), "b"), (Some(Position(4)), true));
    drop(built);
    let scratch = common::scratch_of(&schema);
    assert!(
        there(&url, &scratch),
        "a marked scratch outlives its handle"
    );
    common::drop_schemas(&url);
}

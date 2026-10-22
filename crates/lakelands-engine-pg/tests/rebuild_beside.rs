//! A rebuild beside the projection, on the PostgreSQL engine
//! (lakelands#1322): a builder killed halfway leaves the projection
//! answering and its scratch schema for the next writer's open to remove.

mod common;

use std::process::Command;

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::error::Situation;
use lakelands_api::event::Digest;
use lakelands_api::{Error, Location, Position};
use lakelands_core::conformance::engine::chain;
use lakelands_engine_pg::PgOpener;

/// Set to a lake's schema, the child this test binary runs as.
const CHILD: &str = "LAKELANDS_TEST_SCRATCH_CHILD";

fn at(url: &str, schema: &str) -> Location {
    common::location(url, "beside", schema)
}

/// An empty fold through `through`.
fn fold(engine: &dyn LakeEngine, through: u64) {
    engine.projection().begin().unwrap();
    engine.projection().commit(Position(through)).unwrap();
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
    fold(built.as_ref(), 3);
    std::process::abort();
}

#[test]
fn a_builder_killed_halfway_leaves_the_projection_answering() {
    let Some(url) = common::url_or_skip("the pg rebuild-beside test") else {
        return;
    };
    let schema = common::fresh_schema();
    let writer = PgOpener.open(&at(&url, &schema), OpenMode::Create).unwrap();
    let lease = writer.take_lease("writer").unwrap();
    writer
        .append(&lease, &chain(0, 5, &Digest::zero(), &lease))
        .unwrap();
    fold(writer.as_ref(), 1);
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "a_builder_that_dies_halfway", "--test-threads=1"])
        .env(CHILD, &schema)
        .output()
        .unwrap();
    assert!(!child.status.success(), "the builder was to die: {child:?}");
    let scratch = common::scratch_of(&schema);
    assert!(there(&url, &scratch), "the builder left its scratch");
    assert_eq!(
        writer.projection().folded_through().unwrap(),
        Some(Position(1))
    );
    let swapped = writer.swap(&lease);
    assert!(
        matches!(&swapped, Err(Error::Unavailable { situation }) if matches!(**situation, Situation::NoScratch { .. })),
        "{swapped:?}"
    );
    drop(writer);
    let again = PgOpener.open(&at(&url, &schema), OpenMode::Write).unwrap();
    assert!(!there(&url, &scratch), "the next writer's open removes it");
    assert_eq!(
        again.projection().folded_through().unwrap(),
        Some(Position(1))
    );
    drop(again);
    common::drop_schemas(&url);
}

//! The writer carries the log's tip and its active segment from the take
//! of the lease into every append (lakelands#1374): while the lease is
//! held, an append asks the server for neither. The log's and the
//! segments' tables are hidden behind views that show no row, which an
//! insert still reaches, so a read of either would find the log empty.
//! By hand, with `LAKELANDS_TEST_PG_URL` set.

mod common;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::event::Digest;
use lakelands_core::conformance::engine::{chain, seal};
use lakelands_engine_pg::PgOpener;

/// Statements on the test server, on a connection of their own.
fn admin(url: &str, statements: &str) {
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    client.batch_execute(statements).unwrap();
}

/// Hide the log's and the segments' rows from every read.
fn hide(url: &str, schema: &str) {
    let mut sql = String::new();
    for t in ["log", "segments"] {
        sql += &format!(
            "ALTER TABLE \"{schema}\".{t} RENAME TO {t}_hidden; \
             CREATE VIEW \"{schema}\".{t} AS SELECT * FROM \"{schema}\".{t}_hidden WHERE false; "
        );
    }
    admin(url, &sql);
}

/// Show them again.
fn show(url: &str, schema: &str) {
    let mut sql = String::new();
    for t in ["log", "segments"] {
        sql += &format!(
            "DROP VIEW \"{schema}\".{t}; ALTER TABLE \"{schema}\".{t}_hidden RENAME TO {t}; "
        );
    }
    admin(url, &sql);
}

/// Each event's position and the segment it was appended into.
fn placed(url: &str, schema: &str) -> Vec<(i64, i64)> {
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    let rows = client
        .query(
            &format!("SELECT position, segment FROM \"{schema}\".log ORDER BY position"),
            &[],
        )
        .unwrap();
    rows.iter().map(|r| (r.get(0), r.get(1))).collect()
}

/// The schema, dropped when the test ends, also when it fails.
struct Schema<'u>(&'u str, String);

impl Drop for Schema<'_> {
    fn drop(&mut self) {
        common::drop_schema(self.0, &self.1);
    }
}

#[test]
fn a_held_lease_appends_without_asking_for_the_tip_or_the_active_segment() {
    let Some(url) = common::url_or_skip("the carried tip") else {
        return;
    };
    let kept = Schema(&url, common::fresh_schema());
    let schema = kept.1.clone();
    let engine = PgOpener
        .open(
            &common::location(&url, "carried", &schema),
            OpenMode::Create,
        )
        .unwrap();
    let lease = engine.take_lease("writer").unwrap();
    let events = chain(0, 3, &Digest::zero(), &lease);
    hide(&url, &schema);
    let first = engine.append(&lease, &events[..2]);
    let second = engine.append(&lease, &events[2..]);
    show(&url, &schema);
    first.unwrap();
    second.unwrap();
    assert_eq!(placed(&url, &schema), [(0, 0), (1, 0), (2, 0)]);

    // A seal reads the segment it closes; the appends after it go on
    // into the segment it began, without asking.
    let next = chain(3, 3, &events[2].hash, &lease);
    seal(engine.as_ref(), &lease, &next[..1]).unwrap();
    hide(&url, &schema);
    let after = engine.append(&lease, &next[1..]);
    show(&url, &schema);
    after.unwrap();
    let segments = [(0, 0), (1, 0), (2, 0), (3, 3), (4, 3), (5, 3)];
    assert_eq!(placed(&url, &schema), segments);
    assert_eq!(
        engine.tip().unwrap(),
        Some((next[2].position, next[2].hash.clone()))
    );
}

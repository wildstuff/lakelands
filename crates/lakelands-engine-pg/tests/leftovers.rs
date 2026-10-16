//! A run of the PostgreSQL suites leaves no test schema behind: every test
//! drops the schemas it made when it is done, also when it fails
//! (lakelands#1011). `scripts/conformance.sh` runs this check last, once
//! the engine's tests and the CLI's mirror checks have ended; it is
//! ignored so that no other run of the crate's tests runs it beside them.
//!
//! It holds only where that one run owns the database: CI's job, or a
//! worktree's own container running one suite at a time. Another run in the
//! same database leaves schemas while it runs.

mod common;

#[test]
#[ignore = "run last by scripts/conformance.sh, once every PostgreSQL test has ended"]
fn no_test_schema_is_left() {
    let Some(url) = common::url_or_skip("the check for test schemas left") else {
        return;
    };
    let mut client = postgres::Client::connect(&url, postgres::NoTls).expect("the test server");
    let rows = client
        .query(
            "SELECT nspname FROM pg_catalog.pg_namespace \
             WHERE starts_with(nspname, 'lakelands_test_') ORDER BY nspname",
            &[],
        )
        .expect("listing the schemas");
    let left: Vec<String> = rows.iter().map(|r| r.get(0)).collect();
    assert!(
        left.is_empty(),
        "a test left its schema behind: {left:?}; this holds only where one run owns the \
         database, and another run in this database also leaves schemas while it runs"
    );
}

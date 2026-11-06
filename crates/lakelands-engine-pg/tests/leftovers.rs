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
    // Whatever a failed drop said, where the test that dropped it could not
    // show it (lakelands#1740): the schema's name and the server's message
    // stand beside the leftovers so the run says why, not only what.
    let failed = common::failed_drops();
    assert!(
        left.is_empty(),
        "a test left its schema behind: {left:?}{failed}; this holds only where one run owns the \
         database, and another run in this database also leaves schemas while it runs"
    );
}

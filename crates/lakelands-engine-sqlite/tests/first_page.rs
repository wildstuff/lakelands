//! A first page is read from an index kept in the page's order and stops
//! at its limit; it never reads the type and sorts it (lakelands#580): a
//! page in key order, and one in the order of a field the lake indexes.
//! The engine keeps no statistics, so a statement's plan is the same on
//! every lake, and it is read here from a new one, of each such statement
//! as the engine itself writes it. Where a statement joins `records` to
//! an index's rows, it seeks each record by its key (lakelands#574).

use std::path::PathBuf;

use lakelands_api::Location;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_engine_sqlite::{SqliteOpener, first_page_statements};

#[test]
fn a_first_page_is_planned_without_a_sort() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("first-page-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    drop(
        SqliteOpener
            .open(&Location::sqlite("pages", &dir), OpenMode::Create)
            .unwrap(),
    );
    let db = rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    let statements = first_page_statements();
    assert!(statements.len() >= 4, "the engine names {statements:?}");
    for statement in statements {
        let mut explain = db
            .prepare(&format!("EXPLAIN QUERY PLAN {statement}"))
            .unwrap();
        let unbound = vec![rusqlite::types::Null; explain.parameter_count()];
        let plan: Vec<String> = explain
            .query_map(rusqlite::params_from_iter(unbound), |row| row.get(3))
            .and_then(Iterator::collect)
            .unwrap();
        assert!(
            !plan.iter().any(|step| step.contains("TEMP B-TREE")),
            "a first page is planned with a sort, so it reads every row it could answer before \
             it answers its limit:\n  {statement}\n  {plan:#?}"
        );
        // An index's rows are cut at the limit before their records are
        // joined: each record is then sought by its key. Joined before the
        // cut, the planner seeks `records` by a range of the key for every
        // index row, with no sort to show it.
        assert!(
            !statement.contains("JOIN records r")
                || (plan.iter())
                    .any(|step| step.starts_with("SEARCH r ") && step.contains("key=?")),
            "a first page joins `records` to an index's rows without seeking each record by \
             its key:\n  {statement}\n  {plan:#?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

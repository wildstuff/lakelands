//! A transaction's puts are written a page of 1 024 at a time inside the
//! open transaction (`src/handle.rs`, `stage`), so a put can answer what
//! until then only a commit answered: a page the server refuses. What such
//! a page leaves is shown here and nowhere else, since only PostgreSQL
//! refuses a state the other engines take: its `text` holds no NUL byte.
//!
//! A written page is in the database before its commit, and the vectors
//! put beside its records with it. The engine check holds what a view
//! reads meanwhile; what a search of the vectors reads is held here, since
//! the memory engine puts a vector outside the transaction.

mod common;

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_engine_pg::PgOpener;
use serde_json::json;

/// The key no page can write.
const REFUSED: &str = "bad\0key";

fn record(key: &str, at: u64) -> StoredRecord {
    StoredRecord {
        ty: TypeName::from("t"),
        key: RecordKey::from(key),
        version: Version(1),
        fields: serde_json::from_value(json!({"v": key.replace('\0', "")})).unwrap(),
        meta: Default::default(),
        position: Position(at),
        retired: false,
        restricted: false,
    }
}

/// Whether the head holds the record `key`.
fn holds(engine: &dyn LakeEngine, key: &str) -> bool {
    let view = engine.view(None).unwrap();
    view.get(&TypeName::from("t"), &RecordKey::from(key))
        .unwrap()
        .is_some()
}

#[test]
fn a_page_the_server_refuses_leaves_nothing_of_its_transaction() {
    let Some(url) = common::url_or_skip("the pg page-write test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "pages", &schema), OpenMode::Create)
        .unwrap();
    let p = engine.projection();
    // A first page that is written, then the refused key in the second:
    // the put that fills the second page answers the server's refusal.
    p.begin().unwrap();
    let mut refused = None;
    for i in 0..3_000 {
        let key = match i {
            1_100 => REFUSED.to_owned(),
            _ => format!("a{i:05}"),
        };
        if let Err(e) = p.put_record(&record(&key, 1)) {
            refused = Some((i, e));
            break;
        }
    }
    let (at, _) = refused.expect("a put answers the refusal of the page it filled");
    assert!(
        (1_100..2_048).contains(&at),
        "the put numbered {at} answered the refusal; the first page of 1 024 holds no such key \
         and the second ends at 2 047"
    );
    p.rollback();
    assert!(
        !holds(engine.as_ref(), "a00000"),
        "the page written before the refused one is rolled back with it"
    );
    // The next transaction starts clean and commits.
    p.begin().unwrap();
    p.put_record(&record("b", 1)).unwrap();
    p.commit(Position(1)).unwrap();
    assert!(holds(engine.as_ref(), "b") && !holds(engine.as_ref(), "a00000"));
    // The refused key in the last page: the commit answers, and nothing of
    // the page written before it stays.
    p.begin().unwrap();
    for i in 0..1_100 {
        p.put_record(&record(&format!("c{i:05}"), 2)).unwrap();
    }
    p.put_record(&record(REFUSED, 2)).unwrap();
    assert!(
        p.commit(Position(2)).is_err(),
        "a commit whose last page the server refuses answers the refusal"
    );
    p.rollback();
    assert!(
        !holds(engine.as_ref(), "c00000"),
        "a refused commit leaves nothing of the pages written before it"
    );
    p.begin().unwrap();
    p.put_record(&record("d", 2)).unwrap();
    p.commit(Position(2)).unwrap();
    assert!(holds(engine.as_ref(), "d") && holds(engine.as_ref(), "b"));
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// A caller that commits after a put answered a page's refusal, and never
/// rolls back, is answered an error, and the session is left clean: the
/// next transaction begins, puts and commits, and nothing of the refused
/// one stays.
#[test]
fn a_commit_after_a_refused_page_leaves_the_next_transaction_clean() {
    let Some(url) = common::url_or_skip("the pg page-write test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "pages", &schema), OpenMode::Create)
        .unwrap();
    let p = engine.projection();
    p.begin().unwrap();
    for i in 0..1_100 {
        p.put_record(&record(&format!("a{i:05}"), 1)).unwrap();
    }
    p.put_record(&record(REFUSED, 1)).unwrap();
    let refused = (0..1_024).find_map(|i| p.put_record(&record(&format!("b{i:05}"), 1)).err());
    assert!(
        refused.is_some(),
        "the put that fills the second page answers the server's refusal of it"
    );
    assert!(
        p.commit(Position(1)).is_err(),
        "a commit after a refused page answers an error"
    );
    // No `rollback`: the commit that failed ended the transaction.
    p.begin()
        .expect("the next transaction begins on a clean session");
    p.put_record(&record("c", 1)).unwrap();
    p.commit(Position(1)).unwrap();
    assert!(
        holds(engine.as_ref(), "c") && !holds(engine.as_ref(), "a00000"),
        "the next transaction's record is there, and nothing of the refused one"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

#[test]
fn a_search_reads_the_committed_vectors_while_a_page_is_written() {
    let Some(url) = common::url_or_skip("the pg page-write test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "pages", &schema), OpenMode::Create)
        .unwrap();
    let (p, index) = (engine.projection(), engine.vectors().unwrap());
    let t = TypeName::from("t");
    let put = |key: &str, v: [f32; 2], at: u64| {
        p.put_record(&record(key, at)).unwrap();
        index.put(&t, &RecordKey::from(key), "e", &v).unwrap();
    };
    p.begin().unwrap();
    put("a", [1.0, 0.0], 1);
    p.commit(Position(1)).unwrap();
    // More than a page of records, each with a vector nearer the query
    // than the committed one.
    p.begin().unwrap();
    for i in 0..1_100 {
        put(&format!("b{i:05}"), [0.0, 1.0], 2);
    }
    let near = lakelands_core::conformance::engine::reads::nearest(
        engine.as_ref(),
        &t,
        "e",
        &[0.0, 1.0],
        3,
    )
    .unwrap();
    let near: Vec<&str> = near.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        (near, index.count(&t, "e").unwrap()),
        (vec!["a"], 1),
        "a search and a count while the transaction is open, a page of it written, read the \
         one committed vector"
    );
    p.commit(Position(2)).unwrap();
    assert_eq!(index.count(&t, "e").unwrap(), 1_101);
    drop(engine);
    common::drop_schema(&url, &schema);
}

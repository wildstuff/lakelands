//! A statement the server refuses answers its severity, SQLSTATE and
//! primary message, never its `DETAIL`, which quotes the failing row: a
//! record's key and values, which its caller may not be shown
//! (lakelands#735). A check constraint the test adds makes the server
//! refuse a record whose state holds a value.

mod common;

use lakelands_api::Error;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::id::{Position, Version};
use lakelands_api::record::StoredRecord;
use lakelands_engine_pg::PgOpener;
use serde_json::json;

#[test]
fn a_refused_statement_answers_no_detail_of_the_failing_row() {
    let Some(url) = common::url_or_skip("the pg server-error test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(
            &common::location(&url, "refused", &schema),
            OpenMode::Create,
        )
        .unwrap();
    let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    client
        .batch_execute(&format!(
            "ALTER TABLE {schema}.records ADD CONSTRAINT no_marker \
             CHECK (state NOT LIKE '%marker-value%')"
        ))
        .unwrap();
    let p = engine.projection();
    p.begin().unwrap();
    let answer = p
        .put_record(&StoredRecord {
            ty: "t".into(),
            key: "marker-key".into(),
            version: Version(1),
            fields: serde_json::from_value(json!({"v": "marker-value"})).unwrap(),
            meta: Default::default(),
            position: Position(1),
            retired: false,
            restricted: false,
        })
        .and_then(|()| p.commit(Position(1)));
    let Err(Error::Unavailable { situation }) = answer else {
        panic!("a write the server refuses is unavailable; it answered {answer:?}");
    };
    let message = serde_json::to_string(&Error::Unavailable { situation }).unwrap();
    assert!(
        !message.contains("marker-value") && !message.contains("marker-key"),
        "the answer quotes nothing of the failing row: {message}"
    );
    assert!(
        message.contains("23514") && message.contains("no_marker"),
        "the answer names the SQLSTATE and the server's primary message: {message}"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

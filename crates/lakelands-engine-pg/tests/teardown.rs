//! The drop beside a live reader (lakelands#1816): `meta` is made before
//! `routes`, so the cascade takes `meta` first and the reader's `routes`
//! lock closes the road's cycle; the drop is retried until the reader ends.

mod common;

#[test]
#[rustfmt::skip] // The shape of the cycle reads as the road's, in the lines a budget allows.
fn a_drop_beside_a_live_reader_ends_with_the_schema_gone() {
    let Some(url) = common::url_or_skip("a drop beside a live reader") else { return; };
    let schema = common::fresh_schema();
    let mut reader = postgres::Client::connect(&url, postgres::NoTls).expect("the test server");
    reader.batch_execute(&format!(
        "CREATE SCHEMA \"{schema}\"; CREATE TABLE \"{schema}\".meta (k text); \
         CREATE TABLE \"{schema}\".routes (n int)"
    )).expect("the schema is made");
    reader.batch_execute(&format!(
        "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; \
         LOCK TABLE \"{schema}\".routes IN ACCESS SHARE MODE"
    )).expect("the reader holds the schema");
    let dropped = { let (url, schema) = (url.clone(), schema.clone()); std::thread::spawn(move || lakelands_test_pg::drop_schema(&url, &schema)) };
    std::thread::sleep(std::time::Duration::from_millis(300));
    reader.batch_execute(&format!("SELECT count(*) FROM \"{schema}\".meta")).expect("the reader's read ends, whichever of the two was the victim");
    reader.batch_execute("COMMIT").expect("the reader ends");
    if let Err(e) = dropped.join().expect("the teardown's thread") { panic!("the drop was refused: {}", lakelands_test_pg::said(&e)) }
    let left = reader.query_opt("SELECT 1 FROM pg_catalog.pg_namespace WHERE nspname = $1", &[&schema]).expect("looking for the schema").is_some();
    assert!(!left, "the schema `{schema}` is gone");
}

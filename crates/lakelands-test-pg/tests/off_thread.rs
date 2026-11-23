//! The off-thread teardown's failure (lakelands#1816): a schema the server
//! never lets go fails the test that ended with it, naming the schema and
//! what the server said — where the helper it replaces dropped the error
//! on the floor and left the end-of-run check to find the schema.

#[test]
#[should_panic(expected = "a test schema was left behind: lakelands_test_offthread: ")]
fn a_schema_the_server_never_took_fails_the_test() {
    // Port 1 answers nothing: the drop cannot even connect, which is the
    // refusal the test must show, with no server of any kind involved.
    lakelands_test_pg::drop_all_off_thread(vec![(
        String::from("postgres://nobody:nobody@127.0.0.1:1/lakelands"),
        String::from("lakelands_test_offthread"),
    )]);
}

#[test]
fn nothing_to_drop_is_not_a_failure() {
    lakelands_test_pg::drop_all_off_thread(Vec::new());
}

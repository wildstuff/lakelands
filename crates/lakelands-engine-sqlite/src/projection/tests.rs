use lakelands_api::id::TypeName;
use lakelands_api::seam::kept_name;
use lakelands_core::conformance::fixture::stored_record;
use rusqlite::{Connection, StatementStatus};

use super::{Kept, SCHEMA, drop_bases, put_record, unused_bases};

/// A source's record and its view's kept record are put in turn at every
/// event: each type's declaration, a kept name's its view's, is read once
/// in the transaction, not again at every turn (lakelands#575). The read is counted where SQLite
/// counts it: the times its cached statement ran.
#[test]
fn two_types_put_in_turn_are_each_read_once() {
    let conn = Connection::open_in_memory().unwrap();
    conn.set_prepared_statement_cache_capacity(64);
    conn.execute_batch(SCHEMA).unwrap();
    let types = [TypeName::from("t"), kept_name(&TypeName::from("v"))];
    let (mut kept, mut sharing) = (Kept::new(), lakelands_core::engines::Sharing::default());
    for at in 1..=100 {
        let record = stored_record(
            types[at as usize % 2].clone(),
            format!("k{}", at % 7),
            Default::default(),
            at,
        );
        put_record(&conn, (&mut kept, &mut sharing), &record, true).unwrap();
    }
    // The statement of `type_at` at the head, as it prepares it.
    let read = conn
        .prepare_cached("SELECT state FROM declarations WHERE ident = ?1 AND until IS NULL")
        .unwrap();
    assert_eq!(
        read.get_status(StatementStatus::Run),
        2,
        "the times a type's declaration was read at the head while a hundred records of two \
         types were put in turn"
    );
}

/// A sweep's chunk deletes the bases it was listed with but one a state put
/// after the listing names: the write that names it keeps it, and a base
/// no state names, listed or not, is all the list holds (099, R6).
#[test]
fn a_base_named_after_its_listing_stays_through_the_chunk() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    let state = |id: i64, ty: &str, since: i64, base: i64| {
        conn.execute(
            "INSERT INTO records (ty, id, since, flags, state) VALUES (?1, ?2, ?3, 0, ?4)",
            rusqlite::params![ty, id, since, format!("{{\"base\":{base}}}")],
        )
        .unwrap();
    };
    for (id, block) in [(1, "a"), (2, "b"), (3, "c")] {
        conn.execute(
            "INSERT INTO bases (id, block) VALUES (?1, ?2)",
            rusqlite::params![id, block],
        )
        .unwrap();
    }
    state(1, "t", 5, 1);
    let listed = unused_bases(&conn).unwrap();
    assert_eq!(
        listed,
        [2, 3],
        "the bases no state names, listed at position 10"
    );
    // A write past the listing names base 2, in a type the listing never met.
    state(2, "u", 20, 2);
    drop_bases(&conn, &listed, lakelands_api::id::Position(10)).unwrap();
    let left: Vec<i64> = conn
        .prepare("SELECT id FROM bases ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        left,
        [1, 2],
        "the base named before the list and the one named after stay"
    );
    assert_eq!(unused_bases(&conn).unwrap(), Vec::<u64>::new());
}

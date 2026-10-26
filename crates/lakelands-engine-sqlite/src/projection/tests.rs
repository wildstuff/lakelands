use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::kept_name;
use rusqlite::{Connection, StatementStatus};

use super::{Kept, SCHEMA, put_record};

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
        let record = StoredRecord {
            ty: types[at as usize % 2].clone(),
            key: RecordKey::from(format!("k{}", at % 7)),
            version: Version(1),
            fields: Default::default(),
            meta: Default::default(),
            position: Position(at),
            made: Position(at),
            retired: false,
            restricted: false,
        };
        put_record(&conn, (&mut kept, &mut sharing), &record).unwrap();
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

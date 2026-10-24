//! A lake's parts are each made in the tablespace its location names
//! (`PgTablespaces`): the log's tables and their indexes in `log`, the
//! projection's tables in `projection`, their indexes in `indexes`, the
//! vectors and theirs in `vectors`. The test makes its tablespaces in
//! place, which takes a superuser; on a server where it cannot, it says
//! so and stands down.

mod common;

use lakelands_api::EngineConfig;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_engine_pg::PgOpener;

#[test]
fn each_part_of_a_lake_is_made_in_its_tablespace() {
    let Some(url) = common::url_or_skip("each_part_of_a_lake_is_made_in_its_tablespace") else {
        return;
    };
    let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    let spaces = ["log", "projection", "indexes", "vectors"].map(|p| common::prefix() + p);
    let made = (client.batch_execute("SET allow_in_place_tablespaces = on")).and_then(|()| {
        let make = |t: &String| format!("CREATE TABLESPACE \"{t}\" LOCATION ''");
        spaces
            .iter()
            .try_for_each(|t| client.batch_execute(&make(t)))
    });
    let schema = common::fresh_schema();
    let mut location = common::location(&url, "placed", &schema);
    if let EngineConfig::Postgres { tablespaces: t, .. } = &mut location.engine {
        let t = t.get_or_insert_default();
        [t.log, t.projection, t.indexes, t.vectors] = spaces.clone().map(Some);
    }
    match made {
        Err(e) => {
            eprintln!("this server makes no tablespace for the test's user ({e}); standing down")
        }
        Ok(()) => drop(PgOpener.open(&location, OpenMode::Create).unwrap()),
    }
    let rows = client.query(
        "SELECT c.relname, coalesce(o.relname, c.relname), x.indrelid IS NOT NULL, \
         coalesce(t.spcname, '') FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace AND n.nspname = $1 \
         LEFT JOIN pg_index x ON x.indexrelid = c.oid LEFT JOIN pg_class o ON o.oid = x.indrelid \
         LEFT JOIN pg_tablespace t ON t.oid = c.reltablespace WHERE c.relkind IN ('r', 'i')",
        &[&schema],
    );
    for row in rows.unwrap() {
        let (name, table, index): (String, String, bool) = (row.get(0), row.get(1), row.get(2));
        let part = match table.as_str() {
            "lease" | "segments" | "log" => 0,
            "vectors" => 3,
            _ => 1 + usize::from(index),
        };
        assert_eq!(row.get::<_, String>(3), spaces[part], "{name}");
    }
    common::drop_schema(&url, &schema);
    for t in spaces {
        let _ = client.batch_execute(&format!("DROP TABLESPACE IF EXISTS \"{t}\""));
    }
}

//! A lake's parts are each made in the tablespace its location names
//! (`PgTablespaces`): the log's tables and their indexes in `log`, the
//! projection's tables in `projection`, their indexes in `indexes`, the
//! vectors and theirs in `vectors`. A part named by none is made where
//! `DEFAULT` puts it, and a rebuild beside remakes every part where it is
//! now named but the log and `meta`, which stay. The test makes
//! its tablespaces in place, which takes a superuser; on a server that
//! refuses its user that privilege, it says so and stands down.

mod common;

use lakelands_api::EngineConfig;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::event::Digest;
use lakelands_api::id::Position;
use lakelands_api::location::PgTablespaces;
use lakelands_core::conformance::engine::chain;
use lakelands_engine_pg::PgOpener;
use postgres::error::SqlState;

/// The test's tablespaces and the schemas made in them, dropped when it
/// ends, by a panic as well.
struct Made {
    client: postgres::Client,
    spaces: [String; 4],
    schemas: [String; 2],
}

impl Drop for Made {
    fn drop(&mut self) {
        for s in &self.schemas {
            let scratch = common::scratch_of(s);
            let drop = format!("DROP SCHEMA IF EXISTS \"{s}\", \"{scratch}\" CASCADE");
            let _ = self.client.batch_execute(&drop);
        }
        for t in &self.spaces {
            let drop = format!("DROP TABLESPACE IF EXISTS \"{t}\"");
            let _ = self.client.batch_execute(&drop);
        }
    }
}

/// Every table and index in `schema` is in the tablespace `want` names for
/// it, by its table's name and its part (0 the log, 1 the projection, 2 its
/// indexes, 3 the vectors), `""` for the database's default; and each part
/// has one.
fn placed(client: &mut postgres::Client, schema: &str, want: impl Fn(&str, usize) -> String) {
    let rows = client.query(
        "SELECT c.relname, coalesce(o.relname, c.relname), x.indrelid IS NOT NULL, \
         coalesce(t.spcname, '') FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace AND n.nspname = $1 \
         LEFT JOIN pg_index x ON x.indexrelid = c.oid LEFT JOIN pg_class o ON o.oid = x.indrelid \
         LEFT JOIN pg_tablespace t ON t.oid = c.reltablespace WHERE c.relkind IN ('r', 'i')",
        &[&schema],
    );
    let mut parts = [false; 4];
    for row in rows.unwrap() {
        let (name, table, index): (String, String, bool) = (row.get(0), row.get(1), row.get(2));
        let part = match table.as_str() {
            "lease" | "segments" | "log" | "frames" => 0,
            "vectors" => 3,
            _ => 1 + usize::from(index),
        };
        parts[part] = true;
        assert_eq!(row.get::<_, String>(3), want(&table, part), "{name}");
    }
    assert_eq!(parts, [true; 4], "a table or an index of each part");
}

#[test]
fn each_part_of_a_lake_is_made_in_its_tablespace() {
    let Some(url) = common::url_or_skip("each_part_of_a_lake_is_made_in_its_tablespace") else {
        return;
    };
    let client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    let at = ["log", "projection", "indexes", "vectors"].map(|p| common::prefix() + p);
    let schemas = [common::fresh_schema(), common::fresh_schema()];
    let (spaces, [first, second]) = (at.clone(), schemas.clone());
    let mut made = Made {
        client,
        spaces,
        schemas,
    };
    let c = &mut made.client;
    let make = (c.batch_execute("SET allow_in_place_tablespaces = on")).and_then(|()| {
        let make = |t: &String| format!("CREATE TABLESPACE \"{t}\" LOCATION ''");
        at.iter().try_for_each(|t| c.batch_execute(&make(t)))
    });
    if let Err(e) = make {
        assert_eq!(e.code(), Some(&SqlState::INSUFFICIENT_PRIVILEGE), "{e}");
        return eprintln!("this server lets the test's user make no tablespace; standing down");
    }
    let location = |schema: &str, [log, projection, indexes, vectors]: [Option<String>; 4]| {
        let mut location = common::location(&url, "placed", schema);
        if let EngineConfig::Postgres { tablespaces, .. } = &mut location.engine {
            *tablespaces = Some(PgTablespaces {
                log,
                projection,
                indexes,
                vectors,
            });
        }
        location
    };
    let all = at.clone().map(Some);
    let created = PgOpener.open(&location(&first, all.clone()), OpenMode::Create);
    drop(created.unwrap());
    placed(&mut made.client, &first, |_, part| at[part].clone());

    // The log and the projection named: the indexes and the vectors go
    // where `DEFAULT` puts them, not where the log's tables went before them.
    let named = [all[0].clone(), all[1].clone(), None, None];
    let writer = PgOpener.open(&location(&second, named.clone()), OpenMode::Create);
    let writer = writer.unwrap();
    let lease = writer.take_lease("writer").unwrap();
    let events = chain(0, 5, &Digest::zero(), &lease);
    writer.append(&lease, &events).unwrap();
    placed(&mut made.client, &second, |_, part| {
        named[part].clone().unwrap_or_default()
    });

    // Rebuilt beside with the log unnamed and the rest named: every part
    // moves but the log and `meta`, which stay where they were made.
    let rest = [None, all[1].clone(), all[2].clone(), all[3].clone()];
    let built = PgOpener.open_scratch(&location(&second, rest)).unwrap();
    let p = built.projection();
    p.begin().unwrap();
    p.commit(Position(4)).unwrap();
    assert_eq!(built.mark_built().unwrap(), Some(Position(4)));
    drop(built);
    assert_eq!(writer.swap(&lease).unwrap(), Some(Position(4)));
    placed(&mut made.client, &second, |table, part| {
        match (table, part) {
            ("meta", 2) => String::new(),
            (_, 0) => at[0].clone(),
            (_, part) => at[part].clone(),
        }
    });
}

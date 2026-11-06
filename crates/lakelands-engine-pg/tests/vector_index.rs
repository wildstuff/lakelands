//! The graphs a lake's `vector_index.<type>` setting asks for, kept by
//! pgvector (`docs/decisions/078-under-an-index-a-search-pages-one-ranking-of-what-its-reader-may-see.md`):
//! the setting's commit builds B, the first hold builds A beside it and
//! a new generation of B that never held the record, a build a hold
//! overtakes starts again, a writer's open drops what a build left half
//! done, and the setting retired drops every graph. On a server without
//! pgvector no graph is kept, and every search answers the exact ranking.

mod common;

use std::time::{Duration, Instant};

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode, Rankings, Scan};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::Declaration;
use lakelands_core::conformance::engine::reads::nearest;
use lakelands_core::conformance::fixture::stored_record;
use lakelands_core::engines;
use lakelands_engine_pg::PgOpener;
use serde_json::json;

/// Records in the lake, and the dimensions of their vectors.
const RECORDS: u64 = 600;
const DIMENSIONS: usize = 8;

/// Run statements from a connection of the test's own.
fn exec(url: &str, statements: String) -> Vec<postgres::Row> {
    let url = url.to_owned();
    std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        client.query(&statements, &[]).unwrap()
    })
    .join()
    .unwrap()
}

/// Run several statements from a connection of the test's own.
fn batch(url: &str, statements: String) {
    let url = url.to_owned();
    std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        client.batch_execute(&statements).unwrap();
    })
    .join()
    .unwrap()
}

/// Whether the server has pgvector, creating it if the test may.
fn pgvector(url: &str) -> bool {
    let url = url.to_owned();
    std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        let _ = client.batch_execute("CREATE EXTENSION IF NOT EXISTS vector");
        client
            .query_opt("SELECT 1 FROM pg_extension WHERE extname = 'vector'", &[])
            .unwrap()
            .is_some()
    })
    .join()
    .unwrap()
}

/// The `i`th vector: small whole numbers from a fixed generator, never all
/// zeros.
fn vector(i: u64) -> Vec<f32> {
    let mut z = i.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..DIMENSIONS)
        .map(|_| {
            z = (z ^ (z >> 29)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            ((z >> 40) % 17) as f32 - 8.0
        })
        .map(|x| if x == 0.0 { 1.0 } else { x })
        .collect()
}

fn declaration() -> Declaration {
    serde_json::from_value(json!({
        "declare": "type", "name": "t",
        "fields": [
            {"name": "e", "type": {"kind": "vector", "dimensions": DIMENSIONS}},
            {"name": "f", "type": {"kind": "vector", "dimensions": DIMENSIONS}}
        ]
    }))
    .unwrap()
}

fn record(i: u64) -> StoredRecord {
    stored_record(
        "t",
        format!("k{i:04}"),
        serde_json::from_value(json!({"e": vector(i), "f": vector(i)})).unwrap(),
        1,
    )
}

/// The state of the setting `vector_index.t` asking for `fields`, at `at`.
fn setting(fields: Value, at: u64) -> StoredRecord {
    StoredRecord {
        version: Version(at),
        ..stored_record(
            lakelands_api::seam::SETTING_TYPE,
            "vector_index.t",
            serde_json::from_value(json!({"key": "vector_index.t", "value": fields})).unwrap(),
            at,
        )
    }
}

/// Fold the records and their vectors into a fresh lake, then the setting
/// that asks for the index on `e`.
fn fill(engine: &dyn LakeEngine) {
    let p = engine.projection();
    let index = engine.vectors().expect("the engine keeps vectors");
    p.begin().unwrap();
    p.put_declaration(&declaration(), 1, None, Position(0))
        .unwrap();
    for i in 0..RECORDS {
        let r = record(i);
        p.put_record(&r).unwrap();
        for f in ["e", "f"] {
            index.put(&r.ty, &r.key, f, &vector(i)).unwrap();
        }
    }
    p.put_record(&setting(json!(["e"]), 1)).unwrap();
    p.commit(Position(1)).unwrap();
}

/// The exact ranking of the lake's vectors for `query`, at most `k`.
fn exact(query: &[f32], k: u32) -> Vec<(RecordKey, f64)> {
    let mut all: Vec<(RecordKey, f64)> = (0..RECORDS)
        .map(|i| {
            (
                record(i).key,
                lakelands_api::query::cosine(query, &vector(i)),
            )
        })
        .collect();
    engines::rank(&mut all, k);
    all
}

/// The exact ranking of the lake's vectors for `query` but those of
/// `held`, at most `k`.
fn exact_but(query: &[f32], k: u32, held: &[u64]) -> Vec<(RecordKey, f64)> {
    let mut all: Vec<(RecordKey, f64)> = (0..RECORDS)
        .filter(|i| !held.contains(i))
        .map(|i| {
            (
                record(i).key,
                lakelands_api::query::cosine(query, &vector(i)),
            )
        })
        .collect();
    engines::rank(&mut all, k);
    all
}

/// The nearest `k` of `e` for a reader that holds `restricted`.
fn read(engine: &dyn LakeEngine, query: &[f32], k: u32, restricted: bool) -> Vec<(RecordKey, f64)> {
    let asked = Rankings {
        vectors: vec![("e".into(), query.to_vec())],
        seen: Some(Scan {
            restricted,
            ..Scan::default()
        }),
        ..Rankings::default()
    };
    engine
        .ranked(&TypeName::from("t"), &asked, k)
        .unwrap()
        .lists
        .swap_remove(0)
}

/// Hold the `i`th record at `at`.
fn hold(engine: &dyn LakeEngine, i: u64, at: u64) {
    let p = engine.projection();
    p.begin().unwrap();
    p.put_record(&StoredRecord {
        restricted: true,
        version: Version(2),
        position: Position(at),
        ..record(i)
    })
    .unwrap();
    p.commit(Position(at)).unwrap();
}

/// The table of generation `g` of `t`'s graph of `field` and `class`.
fn table(field: &str, class: &str, g: i64) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(format!("t\0{field}\0{class}\0{g}").as_bytes());
    format!("vgraph_{}", lakelands_core::canonical::hex(&digest[..8]))
}

/// The graph rows, `(field, class, generation, stale, next)` in order.
type Row = (String, String, Option<i64>, Option<i64>, Option<i64>);

fn rows(url: &str, schema: &str) -> Vec<Row> {
    exec(
        url,
        format!(
            "SELECT field, class, gen, stale, next FROM \"{schema}\".graphs ORDER BY field, class"
        ),
    )
    .iter()
    .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4)))
    .collect()
}

/// The graph tables of the schema, in order.
fn tables(url: &str, schema: &str) -> Vec<String> {
    exec(
        url,
        format!(
            "SELECT relname::text FROM pg_class WHERE relnamespace = '\"{schema}\"'::regnamespace \
             AND relkind = 'r' AND relname LIKE 'vgraph%' ORDER BY 1"
        ),
    )
    .iter()
    .map(|r| r.get(0))
    .collect()
}

/// The rows once the builder is done: every graph read, none stale or
/// building, and no table but those the rows read; `(field, class,
/// generation)`.
fn built(url: &str, schema: &str) -> Vec<(String, String, i64)> {
    let asked = Instant::now();
    loop {
        let rows = rows(url, schema);
        let done: Vec<(String, String, i64)> = rows
            .iter()
            .filter(|r| r.3.is_none() && r.4.is_none())
            .filter_map(|r| Some((r.0.clone(), r.1.clone(), r.2?)))
            .collect();
        let mut read: Vec<String> = done.iter().map(|(f, c, g)| table(f, c, *g)).collect();
        read.sort();
        if done.len() == rows.len() && tables(url, schema) == read {
            return done;
        }
        assert!(
            asked.elapsed() < Duration::from_secs(60),
            "the builder is done within a minute: {rows:?}, {:?}",
            tables(url, schema)
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The keys `t`'s graph of `field` and `class` holds an entry of.
fn graph_keys(url: &str, schema: &str, field: &str, class: &str) -> Vec<String> {
    let g = built(url, schema)
        .into_iter()
        .find(|(f, c, _)| f == field && c == class)
        .map(|(f, c, g)| table(&f, &c, g))
        .expect("the graph is built");
    exec(
        url,
        format!("SELECT key FROM \"{schema}\".{g} WHERE unit IS NOT NULL ORDER BY key"),
    )
    .iter()
    .map(|r| r.get(0))
    .collect()
}

#[test]
fn an_indexed_field_reads_its_graph_and_every_other_field_ranks_exactly() {
    let Some(url) = common::url_or_skip("the pg vector-index test") else {
        return;
    };
    let with_pgvector = pgvector(&url);
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "vindex", &schema), OpenMode::Create)
        .unwrap();
    fill(engine.as_ref());
    let index = engine.vectors().unwrap();
    let (query, t) = (vector(7), TypeName::from("t"));
    if !with_pgvector {
        assert_eq!(
            (rows(&url, &schema), tables(&url, &schema)),
            (Vec::new(), Vec::new()),
            "without pgvector no graph is kept"
        );
        for f in ["e", "f"] {
            assert_eq!(
                nearest(engine.as_ref(), &t, f, &query, 10).unwrap(),
                exact(&query, 10),
                "without pgvector `{f}` answers the exact ranking"
            );
        }
        drop(engine);
        common::drop_schema(&url, &schema);
        return;
    }
    let graphs = built(&url, &schema);
    assert_eq!(
        graphs
            .iter()
            .map(|(f, c, _)| (f.as_str(), c.as_str()))
            .collect::<Vec<_>>(),
        [("e", "b")],
        "the setting's commit builds `e`'s B, and no A while nothing is held"
    );
    for k in [1, 10, 100] {
        assert_eq!(
            nearest(engine.as_ref(), &t, "e", &query, k).unwrap(),
            exact(&query, k),
            "the stream from `e`'s graph answers the exact ranking, {k} deep"
        );
    }
    let read = table("e", "b", graphs[0].2);
    exec(
        &url,
        format!("DELETE FROM \"{schema}\".{read} WHERE key = 'k0007'"),
    );
    let got = nearest(engine.as_ref(), &t, "e", &query, 10).unwrap();
    assert!(
        !got.iter().any(|(k, _)| k.as_str() == "k0007") && got.len() == 10,
        "a search of `e` reads its graph: an entry taken from it is not found"
    );
    assert_eq!(
        nearest(engine.as_ref(), &t, "f", &query, 10).unwrap(),
        exact(&query, 10),
        "`f`, which the setting does not name, answers the exact ranking"
    );
    // A write after the build reaches the graph in the fold that puts it.
    let p = engine.projection();
    p.begin().unwrap();
    let near = RecordKey::from("near");
    index.put(&t, &near, "e", &query).unwrap();
    p.put_record(&StoredRecord {
        key: near.clone(),
        position: Position(2),
        ..record(7)
    })
    .unwrap();
    p.commit(Position(2)).unwrap();
    let got = nearest(engine.as_ref(), &t, "e", &query, 1).unwrap();
    assert_eq!(
        got.first().map(|(k, _)| k.as_str()),
        Some("near"),
        "a vector put after the build is streamed from the graph"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// A hold makes B stale: its readers are ranked exactly until a new
/// generation, copied from the head, is swapped in. That generation is
/// a new table that never held the record; A, built at the type's first
/// hold, holds it, and the old table is gone.
#[test]
fn a_hold_swaps_in_a_b_that_never_held_the_record() {
    let Some(url) = common::url_or_skip("the pg vector-index test") else {
        return;
    };
    if !pgvector(&url) {
        return;
    }
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "vindex", &schema), OpenMode::Create)
        .unwrap();
    fill(engine.as_ref());
    let before = built(&url, &schema);
    let query = vector(7);
    hold(engine.as_ref(), 7, 2);
    let after = built(&url, &schema);
    let b = after.iter().find(|(_, c, _)| c == "b").unwrap();
    assert!(
        after.iter().any(|(_, c, _)| c == "a") && b.2 > before[0].2,
        "the first hold builds A, and B again: {after:?}"
    );
    assert!(
        !tables(&url, &schema).contains(&table("e", "b", before[0].2)),
        "the generation of B that held the record is dropped"
    );
    let missing = |class: &str| {
        !graph_keys(&url, &schema, "e", class)
            .iter()
            .any(|k| k == "k0007")
    };
    assert!(
        missing("b") && !missing("a"),
        "B's new generation never held the held record; A holds it"
    );
    assert_eq!(
        read(engine.as_ref(), &query, 10, false),
        exact_but(&query, 10, &[7]),
        "a reader without `restricted` reads B, and finds the held record nowhere"
    );
    assert_eq!(
        read(engine.as_ref(), &query, 10, true),
        exact(&query, 10),
        "a reader holding `restricted` reads A, and finds it"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// A vector removed since B's copy leaves no row in force, yet B's graph
/// took its shape from it: the record's hold builds B again all the same.
/// The states are as the store writes them, the vector kept apart under
/// `vectors`, and the hold's state names no vector.
#[test]
fn a_hold_of_a_record_whose_vector_went_builds_b_again() {
    let Some(url) = common::url_or_skip("the pg vector-index test") else {
        return;
    };
    if !pgvector(&url) {
        return;
    }
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "vindex", &schema), OpenMode::Create)
        .unwrap();
    let p = engine.projection();
    p.begin().unwrap();
    p.put_declaration(&declaration(), 1, None, Position(0))
        .unwrap();
    p.commit(Position(1)).unwrap();
    p.begin().unwrap();
    for i in 0..RECORDS {
        // Whole numbers, as the store's canonical states write them.
        let whole: Vec<i64> = vector(i).iter().map(|x| *x as i64).collect();
        p.put_record(&StoredRecord {
            fields: serde_json::from_value(json!({"e": whole, "f": whole})).unwrap(),
            position: Position(2),
            made: Position(2),
            ..record(i)
        })
        .unwrap();
    }
    p.put_record(&setting(json!(["e"]), 2)).unwrap();
    p.commit(Position(2)).unwrap();
    let state = exec(
        &url,
        format!("SELECT state::text FROM \"{schema}\".records WHERE key = 'k0007'"),
    );
    let state: serde_json::Value = serde_json::from_str(state[0].get(0)).unwrap();
    assert!(
        state["fields"].get("e").is_none() && state["vectors"].get("e").is_some(),
        "the state keeps its vector apart: {state}"
    );
    let before = built(&url, &schema);
    let r = record(7);
    p.begin().unwrap();
    engine
        .vectors()
        .unwrap()
        .remove(&r.ty, &r.key, "e")
        .unwrap();
    p.commit(Position(3)).unwrap();
    p.begin().unwrap();
    let f: Vec<i64> = vector(7).iter().map(|x| *x as i64).collect();
    let fields = serde_json::from_value(json!({ "f": f })).unwrap();
    p.put_record(&StoredRecord {
        fields,
        restricted: true,
        version: Version(2),
        position: Position(4),
        made: Position(2),
        ..r
    })
    .unwrap();
    p.commit(Position(4)).unwrap();
    let after = built(&url, &schema);
    let b = |rows: &[(String, String, i64)]| rows.iter().find(|(_, c, _)| c == "b").unwrap().2;
    assert!(
        b(&after) > b(&before),
        "B is built again: {before:?}, {after:?}"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// What a writer that went away mid-build leaves, a claimed generation
/// and its half-built table, and any table no row names, the next
/// writer's open drops; the generation read stays.
#[test]
fn a_half_built_graph_is_dropped_at_a_writers_open() {
    let Some(url) = common::url_or_skip("the pg vector-index test") else {
        return;
    };
    if !pgvector(&url) {
        return;
    }
    let schema = common::fresh_schema();
    let location = common::location(&url, "vindex", &schema);
    fill(PgOpener.open(&location, OpenMode::Create).unwrap().as_ref());
    let graphs = built(&url, &schema);
    let (half, stray) = (table("e", "b", 99), "vgraph_0000000000000000");
    batch(
        &url,
        format!(
            "UPDATE \"{schema}\".graphs SET next = 99; \
             CREATE TABLE \"{schema}\".{half} (key text); \
             CREATE TABLE \"{schema}\".{stray} (key text)"
        ),
    );
    let writer = PgOpener.open(&location, OpenMode::Write).unwrap();
    assert_eq!(
        (rows(&url, &schema), tables(&url, &schema)),
        (
            vec![("e".into(), "b".into(), Some(graphs[0].2), None, None)],
            vec![table("e", "b", graphs[0].2)]
        ),
        "the writer's open gives up the claim and drops both tables; B's generation stays"
    );
    let query = vector(7);
    assert_eq!(
        nearest(writer.as_ref(), &TypeName::from("t"), "e", &query, 10).unwrap(),
        exact(&query, 10),
        "and is read"
    );
    drop(writer);
    common::drop_schema(&url, &schema);
}

/// B's copies stalled at their table's creation, after their snapshot,
/// by an event trigger on the test's own lock, which takes a superuser:
/// `None` where the server has no pgvector or the test is no superuser.
/// The trigger fires at every table created in the database, so it reads
/// its own schema's `graphs` only past the schema's check: a statement
/// naming it would lock it in every other test's transaction, and hold
/// off the builder's sweep there.
struct Stall {
    url: String,
    schema: String,
    release: Option<(std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>)>,
}

impl Stall {
    fn new(url: &str, schema: &str) -> Stall {
        let s = format!("\"{schema}\"");
        let key = format!("hashtext('{schema}')");
        batch(
            url,
            format!(
                "CREATE SEQUENCE {s}.stalled; \
                 CREATE FUNCTION {s}.stall() RETURNS event_trigger LANGUAGE plpgsql AS $f$ BEGIN \
                 IF EXISTS (SELECT 1 FROM pg_event_trigger_ddl_commands() c \
                     WHERE c.schema_name = '{schema}' AND c.object_identity LIKE '%vgraph%') THEN \
                   IF EXISTS (SELECT 1 FROM {s}.graphs WHERE class = 'b' AND next IS NOT NULL) THEN \
                     PERFORM nextval('{s}.stalled'); PERFORM pg_advisory_xact_lock_shared({key}); \
                   END IF; \
                 END IF; END $f$; \
                 CREATE EVENT TRIGGER \"stall_{schema}\" ON ddl_command_end \
                 WHEN TAG IN ('CREATE TABLE') EXECUTE FUNCTION {s}.stall()"
            ),
        );
        let (locked, release) = (std::sync::mpsc::channel(), std::sync::mpsc::channel::<()>());
        let holder = {
            let (url, locked, wait) = (url.to_owned(), locked.0, release.1);
            std::thread::spawn(move || {
                let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
                client
                    .batch_execute(&format!("SELECT pg_advisory_lock({key})"))
                    .unwrap();
                locked.send(()).unwrap();
                wait.recv().unwrap();
            })
        };
        locked.1.recv().unwrap();
        Stall {
            url: url.to_owned(),
            schema: schema.to_owned(),
            release: Some((release.0, holder)),
        }
    }

    /// Whether the server lets the test stall a copy.
    fn possible(url: &str) -> bool {
        let superuser = exec(
            url,
            "SELECT rolsuper FROM pg_roles WHERE rolname = current_user".into(),
        );
        pgvector(url) && superuser[0].get::<_, bool>(0)
    }

    /// How many of B's copies were stalled.
    fn stalled(&self) -> i64 {
        exec(
            &self.url,
            format!(
                "SELECT CASE WHEN is_called THEN last_value ELSE 0 END FROM \"{}\".stalled",
                self.schema
            ),
        )[0]
        .get(0)
    }

    /// Wait for the first copy to stall.
    fn reached(&self) {
        let asked = Instant::now();
        while self.stalled() == 0 {
            assert!(asked.elapsed() < Duration::from_secs(60), "B's copy starts");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Let every copy go on, and wait for the builder; the rows built.
    fn release(&mut self) -> Vec<(String, String, i64)> {
        if let Some((release, holder)) = self.release.take() {
            release.send(()).unwrap();
            holder.join().unwrap();
        }
        let graphs = built(&self.url, &self.schema);
        exec(
            &self.url,
            format!("DROP EVENT TRIGGER \"stall_{}\"", self.schema),
        );
        graphs
    }
}

/// A hold folded while B's next generation is copied may have reached
/// the copy: the swap refuses that generation and the build starts
/// again, from a copy after the hold.
#[test]
fn a_hold_during_the_copy_starts_the_build_again() {
    let Some(url) = common::url_or_skip("the pg vector-index test") else {
        return;
    };
    if !Stall::possible(&url) {
        return;
    }
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "vindex", &schema), OpenMode::Create)
        .unwrap();
    fill(engine.as_ref());
    built(&url, &schema);
    let mut stall = Stall::new(&url, &schema);
    hold(engine.as_ref(), 7, 2);
    stall.reached();
    let first = rows(&url, &schema)
        .into_iter()
        .find(|r| r.1 == "b")
        .and_then(|r| r.4)
        .unwrap();
    hold(engine.as_ref(), 8, 3);
    let graphs = stall.release();
    let b = graphs.iter().find(|(_, c, _)| c == "b").unwrap().2;
    assert!(
        stall.stalled() == 2 && b > first,
        "the swap refuses the copy the hold overtook and the build starts again: \
         {} copies, generation {b} after {first}",
        stall.stalled()
    );
    assert!(
        !graph_keys(&url, &schema, "e", "b")
            .iter()
            .any(|k| k == "k0007" || k == "k0008"),
        "the generation swapped in never held either record"
    );
    let query = vector(8);
    assert_eq!(
        read(engine.as_ref(), &query, 10, false),
        exact_but(&query, 10, &[7, 8]),
        "B's reader finds neither"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// A vector removed while B's next generation is copied, after the copy's
/// snapshot, is taken out of that generation before it is swapped in.
#[test]
fn a_vector_removed_during_the_copy_is_no_entry_of_the_generation() {
    let Some(url) = common::url_or_skip("the pg vector-index test") else {
        return;
    };
    if !Stall::possible(&url) {
        return;
    }
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "vindex", &schema), OpenMode::Create)
        .unwrap();
    fill(engine.as_ref());
    built(&url, &schema);
    let mut stall = Stall::new(&url, &schema);
    hold(engine.as_ref(), 7, 2);
    stall.reached();
    let (t, gone) = (TypeName::from("t"), RecordKey::from("k0005"));
    engine.vectors().unwrap().remove(&t, &gone, "e").unwrap();
    stall.release();
    assert!(
        !graph_keys(&url, &schema, "e", "b")
            .iter()
            .any(|k| k == "k0005"),
        "the generation swapped in holds no entry of the vector removed during its copy"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// Retiring the setting drops every row, and the builder every table.
#[test]
fn retiring_the_setting_drops_every_graph() {
    let Some(url) = common::url_or_skip("the pg vector-index test") else {
        return;
    };
    if !pgvector(&url) {
        return;
    }
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "vindex", &schema), OpenMode::Create)
        .unwrap();
    fill(engine.as_ref());
    let p = engine.projection();
    p.begin().unwrap();
    p.put_record(&setting(json!(["e", "f"]), 2)).unwrap();
    p.commit(Position(2)).unwrap();
    assert_eq!(
        built(&url, &schema).len(),
        2,
        "the setting naming `e` and `f` builds B of both"
    );
    p.begin().unwrap();
    p.put_record(&StoredRecord {
        retired: true,
        ..setting(json!(["e", "f"]), 3)
    })
    .unwrap();
    p.commit(Position(3)).unwrap();
    assert_eq!(
        built(&url, &schema),
        Vec::new(),
        "the setting retired drops every row and table"
    );
    let query = vector(7);
    assert_eq!(
        nearest(engine.as_ref(), &TypeName::from("t"), "e", &query, 10).unwrap(),
        exact(&query, 10),
        "with the setting retired, `e` answers the exact ranking"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// Searches that overlap without a pause, each locking `graphs` before
/// its snapshot as a ranking does and reading the generation its row
/// names, hold off neither the builder's drop of a generation swapped
/// away nor a hold's build. Each search sleeps 0.3 s between reading
/// the row and reading the generation it names, so a drop that does not
/// wait for it leaves it no table; the second reader starts 0.15 s after
/// the first, so `graphs` is never free. The second hold comes 8 s after
/// the first, by when a builder that took a refused drop for a failure
/// would be waiting out its back-off.
#[test]
fn searches_that_never_pause_hold_off_no_drop_and_no_build() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    let Some(url) = common::url_or_skip("the pg vector-index test") else {
        return;
    };
    if !pgvector(&url) {
        return;
    }
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "vindex", &schema), OpenMode::Create)
        .unwrap();
    fill(engine.as_ref());
    built(&url, &schema);
    let (stop, failed) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(Mutex::new(Vec::new())),
    );
    let readers: Vec<_> = (0..2)
        .map(|n| {
            let (url, schema) = (url.clone(), schema.clone());
            let (stop, failed) = (Arc::clone(&stop), Arc::clone(&failed));
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150 * n));
                let mut c = postgres::Client::connect(&url, postgres::NoTls).unwrap();
                let s = format!("\"{schema}\"");
                while !stop.load(Ordering::Relaxed) {
                    let read = (|| -> Result<(), postgres::Error> {
                        c.batch_execute(&format!(
                            "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; \
                             LOCK TABLE {s}.graphs IN ACCESS SHARE MODE"
                        ))?;
                        let g: i64 = c
                            .query_one(
                                &format!(
                                    "SELECT gen FROM {s}.graphs WHERE field = 'e' AND class = 'b'"
                                ),
                                &[],
                            )?
                            .get(0);
                        c.batch_execute(&format!(
                            "SELECT pg_sleep(0.3); SELECT count(*) FROM {s}.{}; COMMIT",
                            table("e", "b", g)
                        ))
                    })();
                    if let Err(e) = read {
                        let e = e
                            .as_db_error()
                            .map_or(e.to_string(), |d| d.message().into());
                        failed.lock().unwrap().push(e);
                        let _ = c.batch_execute("ROLLBACK");
                    }
                }
            })
        })
        .collect();
    std::thread::sleep(Duration::from_secs(1));
    let b = |url: &str| {
        rows(url, &schema)
            .into_iter()
            .find(|r| r.1 == "b" && r.3.is_none() && r.4.is_none())
            .and_then(|r| r.2)
    };
    // The seconds until B is swapped past `from`, and those until no
    // table is left but the ones the rows read.
    let swap_and_drop = |from: i64| {
        let asked = Instant::now();
        while b(&url).is_none_or(|g| g <= from) {
            assert!(
                asked.elapsed() < Duration::from_secs(60),
                "B is built again"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let swapped = asked.elapsed().as_secs_f64();
        loop {
            let rows = rows(&url, &schema);
            let mut read: Vec<String> = rows
                .iter()
                .filter_map(|r| Some(table(&r.0, &r.1, r.2?)))
                .collect();
            read.sort();
            if tables(&url, &schema) == read || asked.elapsed() > Duration::from_secs(60) {
                return (
                    swapped,
                    asked.elapsed().as_secs_f64(),
                    tables(&url, &schema),
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let first = b(&url).unwrap();
    hold(engine.as_ref(), 7, 2);
    let once = swap_and_drop(first);
    std::thread::sleep(Duration::from_secs(8));
    let second = b(&url).unwrap();
    hold(engine.as_ref(), 8, 3);
    let twice = swap_and_drop(second);
    stop.store(true, Ordering::Relaxed);
    for r in readers {
        r.join().unwrap();
    }
    assert!(
        once.1 < 3.0 && twice.0 < 3.0 && twice.1 < 3.0,
        "under searches that never pause, each old generation is dropped within 3 s of the \
         hold and the second hold's B is swapped in within 3 s: after the first hold, swapped \
         at {:.2} s, the tables as the rows read at {:.2} s ({:?}); after the second, at \
         {:.2} s and {:.2} s ({:?})",
        once.0,
        once.1,
        once.2,
        twice.0,
        twice.1,
        twice.2
    );
    let failed = failed.lock().unwrap();
    assert!(
        failed.is_empty(),
        "every search read its generation whole: {failed:?}"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// A pull deeper than pgvector's default `hnsw.max_scan_tuples` of
/// 20 000 is streamed whole: a search lifts the bound. That takes a
/// field larger than any row of the suite holds on every engine, so it
/// is held here, on vectors alone.
#[test]
fn a_pull_past_the_default_scan_bound_is_streamed_whole() {
    const FIELD: u32 = 25_000;
    let Some(url) = common::url_or_skip("the pg vector-index test") else {
        return;
    };
    if !pgvector(&url) {
        return;
    }
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "vindex", &schema), OpenMode::Create)
        .unwrap();
    let (p, index, t) = (
        engine.projection(),
        engine.vectors().unwrap(),
        TypeName::from("t"),
    );
    // Every vector its own direction, turning away from the first.
    let key = |i: u32| RecordKey::from(format!("k{i:05}"));
    let at = |i: u32| {
        let turn = f64::from(i) * 1.5 / f64::from(FIELD);
        let mut v = vec![0.0; DIMENSIONS];
        (v[0], v[1]) = (turn.cos() as f32, turn.sin() as f32);
        v
    };
    p.begin().unwrap();
    p.put_declaration(&declaration(), 1, None, Position(0))
        .unwrap();
    for i in 0..FIELD {
        index.put(&t, &key(i), "e", &at(i)).unwrap();
    }
    p.put_record(&setting(json!(["e"]), 1)).unwrap();
    p.commit(Position(1)).unwrap();
    assert_eq!(built(&url, &schema).len(), 1, "`e` is indexed");
    let (query, k) = (at(0), FIELD - 1_000);
    let got = nearest(engine.as_ref(), &t, "e", &query, k).unwrap();
    let mut want: Vec<(RecordKey, f64)> = (0..FIELD)
        .map(|i| (key(i), lakelands_api::query::cosine(&query, &at(i))))
        .collect();
    engines::rank(&mut want, k);
    assert!(
        got == want,
        "the nearest {k} of {FIELD} vectors are the exact ranking's; {} came",
        got.len()
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// The scan that answers for an indexed field leaves out a vector of
/// zeros, as the graph does: else a hit would show that the field
/// is smaller than the pull, to a reader shown only part of it. Twin
/// fields hold the same forty vectors and one of zeros; the second holds
/// three hundred more, farther away, so its index fills the pull and the
/// first's does not. After the build, two of the forty indexed are
/// written over, one with zeros and one with a vector of another
/// dimension. Neither field answers a vector without a unit vector, and
/// the first answers the exact ranking of the other thirty-eight. A
/// field without the setting scores the vector of zeros 0, as every
/// engine does.
///
/// It is this engine's own test and no row of the suite: what leaks is
/// which of this engine's two ways answered, and an engine without an
/// index scans both twins and answers the vector of zeros in both. The
/// `k` of 64 is the store's first pull for a page of ten, and the store
/// cuts what a reader is shown after the pull, so the three hundred
/// stand for records the reader is not shown.
#[test]
fn the_scan_that_answers_for_an_index_holds_no_vector_of_zeros() {
    let Some(url) = common::url_or_skip("the pg vector-index test") else {
        return;
    };
    if !pgvector(&url) {
        return;
    }
    let key = |i: u32| RecordKey::from(format!("k{i:05}"));
    // Twenty near the query, one of zeros, twenty beyond a right angle,
    // and in the second field three hundred farther still.
    let at = |i: u32| {
        let turn = match i {
            0..=20 => f64::from(i) * 0.01,
            21..41 => 1.7 + f64::from(i) * 0.01,
            _ => 2.2 + f64::from(i) * 0.002,
        };
        let mut v = vec![0.0; DIMENSIONS];
        if i != 20 {
            (v[0], v[1]) = (turn.cos() as f32, turn.sin() as f32);
        }
        v
    };
    let t = TypeName::from("t");
    let (query, k) = (at(0), 64);
    let mut answers = Vec::new();
    for held in [41, 341] {
        let schema = common::fresh_schema();
        let engine = PgOpener
            .open(&common::location(&url, "vindex", &schema), OpenMode::Create)
            .unwrap();
        let (p, index) = (engine.projection(), engine.vectors().unwrap());
        p.begin().unwrap();
        p.put_declaration(&declaration(), 1, None, Position(0))
            .unwrap();
        for i in 0..held {
            index.put(&t, &key(i), "e", &at(i)).unwrap();
            index.put(&t, &key(i), "f", &at(i)).unwrap();
        }
        p.put_record(&setting(json!(["e"]), 1)).unwrap();
        p.commit(Position(1)).unwrap();
        assert_eq!(built(&url, &schema).len(), 1, "`e` is indexed");
        // Over two built entries, one of zeros and one of another
        // dimension: neither has a unit vector, so the graph keeps the
        // key and no unit, and a pull past the stream's end reads the
        // graph's other entries.
        p.begin().unwrap();
        index.put(&t, &key(5), "e", &[0.0; DIMENSIONS]).unwrap();
        index.put(&t, &key(7), "e", &[1.0; DIMENSIONS + 1]).unwrap();
        p.commit(Position(2)).unwrap();
        let shared = |field: &str| -> Vec<(RecordKey, f64)> {
            let got = nearest(engine.as_ref(), &t, field, &query, k).unwrap();
            got.into_iter()
                .filter(|(key, _)| key.as_str() < "k00041")
                .collect()
        };
        let scanned = shared("f");
        assert!(
            scanned
                .iter()
                .any(|(got, score)| *got == key(20) && *score == 0.0),
            "a field without the setting scores a vector of zeros 0"
        );
        answers.push(shared("e"));
        drop(engine);
        common::drop_schema(&url, &schema);
    }
    let mut want: Vec<(RecordKey, f64)> = (0..41)
        .filter(|i| ![5, 7, 20].contains(i))
        .map(|i| (key(i), lakelands_api::query::cosine(&query, &at(i))))
        .collect();
    engines::rank(&mut want, k);
    assert!(
        answers[0] == want,
        "a field smaller than the pull answers every vector but those without a unit vector, \
         exactly: {:?}",
        answers[0]
    );
    assert!(
        answers[1] == want,
        "the twin that holds more answers the same of what both hold: {:?}",
        answers[1]
    );
}

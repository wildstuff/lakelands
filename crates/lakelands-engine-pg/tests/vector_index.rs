//! The vector index a lake's `vector_index.<type>` setting asks for, kept
//! by pgvector (`docs/decisions/043-an-index-asked-for-answers-its-set.md`):
//! the setting's write builds it and marks it, a search reads it only while
//! the mark describes the head, a writer's open rebuilds a stale one, and
//! the setting retired drops it. On a server without pgvector nothing is
//! marked, and every search answers the exact ranking.

mod common;

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::Declaration;
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
    StoredRecord {
        ty: TypeName::from("t"),
        key: RecordKey::from(format!("k{i:04}")),
        version: Version(1),
        fields: serde_json::from_value(json!({"e": vector(i), "f": vector(i)})).unwrap(),
        meta: Default::default(),
        position: Position(1),
        retired: false,
        restricted: false,
    }
}

/// The state of the setting `vector_index.t` asking for `fields`, at `at`.
fn setting(fields: Value, at: u64) -> StoredRecord {
    StoredRecord {
        ty: TypeName::from(lakelands_api::seam::SETTING_TYPE),
        key: RecordKey::from("vector_index.t"),
        version: Version(at),
        fields: serde_json::from_value(json!({"key": "vector_index.t", "value": fields})).unwrap(),
        meta: Default::default(),
        position: Position(at),
        retired: false,
        restricted: false,
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

/// The record whose unit vector [`decoy`] points at the query: only a
/// search through the index can answer it among the nearest.
const DECOY: &str = "k0500";

/// Point `DECOY`'s unit vector at `query`, leaving its vector as it was:
/// the index then offers it as the nearest, while the one cosine scores
/// its real vector, which the exact ranking does not put near the top.
fn decoy(url: &str, schema: &str, query: &[f32]) {
    let n = query
        .iter()
        .map(|x| f64::from(*x).powi(2))
        .sum::<f64>()
        .sqrt();
    let unit: Vec<String> = query
        .iter()
        .map(|x| ((f64::from(*x) / n) as f32).to_string())
        .collect();
    exec(
        url,
        format!(
            "UPDATE \"{schema}\".vectors SET unit = '[{}]' WHERE key = '{DECOY}' AND field = 'e'",
            unit.join(",")
        ),
    );
}

/// Whether an answer came through the index: it holds the decoy.
fn through_index(answer: &[(RecordKey, f64)]) -> bool {
    answer.iter().any(|(k, _)| k.as_str() == DECOY)
}

/// The mark of `e`'s index and how many indexes the lake keeps.
fn marks(url: &str, schema: &str) -> (Option<i64>, i64) {
    let row = exec(
        url,
        format!(
            "SELECT (SELECT n FROM \"{schema}\".meta WHERE k = 'vindex.t.e'), \
             (SELECT count(*) FROM pg_indexes WHERE schemaname = '{schema}' \
              AND indexname LIKE 'vindex_%')"
        ),
    );
    (row[0].get(0), row[0].get(1))
}

#[test]
fn an_indexed_field_reads_its_index_and_every_other_field_scans() {
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
    let query = vector(7);
    let t = TypeName::from("t");
    if !with_pgvector {
        assert_eq!(
            marks(&url, &schema),
            (None, 0),
            "without pgvector no field is marked and no index is built"
        );
        for f in ["e", "f"] {
            assert_eq!(
                index.nearest(&t, f, &query, 10).unwrap(),
                exact(&query, 10),
                "without pgvector `{f}` answers the exact ranking"
            );
        }
        drop(engine);
        common::drop_schema(&url, &schema);
        return;
    }
    assert_eq!(
        marks(&url, &schema),
        (Some(1), 1),
        "the setting's write builds `e`'s index and marks it kept through its position"
    );
    assert!(
        !exact(&query, 10).iter().any(|(k, _)| k.as_str() == DECOY),
        "the decoy is no exact neighbour"
    );
    let got = index.nearest(&t, "e", &query, 10).unwrap();
    assert_eq!(got.len(), 10, "the index answers k candidates");
    let mut ranked = got.clone();
    engines::rank(&mut ranked, 10);
    assert_eq!(
        got, ranked,
        "the candidates are ranked by the one cosine, ties by key"
    );
    for (key, score) in &got {
        let i: u64 = key.as_str()[1..].parse().unwrap();
        assert_eq!(
            score.to_bits(),
            lakelands_api::query::cosine(&query, &vector(i)).to_bits(),
            "`{key}` scores the one cosine"
        );
    }
    let exact10 = exact(&query, 10);
    let found = got.iter().filter(|h| exact10.contains(h)).count();
    assert!(
        found >= 9,
        "the index finds at least 9 of the exact 10 on a small field: {found}"
    );
    decoy(&url, &schema, &query);
    assert!(
        through_index(&index.nearest(&t, "e", &query, 10).unwrap()),
        "a search of `e` reads its index"
    );
    assert_eq!(
        index.nearest(&t, "f", &query, 10).unwrap(),
        exact10,
        "`f`, which the setting does not name, answers the exact ranking"
    );
    // A write after the build reaches the index in the fold that puts it.
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
    assert_eq!(
        marks(&url, &schema).0,
        Some(2),
        "the fold keeps the mark at its position"
    );
    // Three unit vectors now point at the query: `k0007`'s, `near`'s and
    // the decoy's; the index offers all three, and the one cosine ranks the
    // two true ones first, their tie by key.
    let got = index.nearest(&t, "e", &query, 3).unwrap();
    let keys: Vec<&str> = got.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        ["k0007", "near", DECOY],
        "a vector put after the build is found through the index, and the candidates rank by \
         the one cosine, ties by key"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

#[test]
fn a_stale_index_is_not_read_and_a_writers_open_rebuilds_it() {
    let Some(url) = common::url_or_skip("the pg vector-index test") else {
        return;
    };
    if !pgvector(&url) {
        return;
    }
    let schema = common::fresh_schema();
    let location = common::location(&url, "vindex", &schema);
    fill(PgOpener.open(&location, OpenMode::Create).unwrap().as_ref());
    // What a writer without the index leaves: vectors without their unit
    // vector, and the mark behind the fold.
    exec(
        &url,
        format!("UPDATE \"{schema}\".vectors SET unit = NULL WHERE key < 'k0300'"),
    );
    exec(
        &url,
        format!("UPDATE \"{schema}\".meta SET n = 0 WHERE k = 'vindex.t.e'"),
    );
    let query = vector(7);
    let t = TypeName::from("t");
    decoy(&url, &schema, &query);
    let reader = PgOpener.open(&location, OpenMode::Read).unwrap();
    assert_eq!(
        reader
            .vectors()
            .unwrap()
            .nearest(&t, "e", &query, 10)
            .unwrap(),
        exact(&query, 10),
        "a stale index is not read: the search scans, exactly"
    );
    let writer = PgOpener.open(&location, OpenMode::Write).unwrap();
    let row = exec(
        &url,
        format!("SELECT count(*) FROM \"{schema}\".vectors WHERE field = 'e' AND unit IS NULL"),
    );
    assert_eq!(
        (marks(&url, &schema).0, row[0].get::<_, i64>(0)),
        (Some(1), 0),
        "a writer's open fills every unit vector again and marks the index kept through the fold"
    );
    decoy(&url, &schema, &query);
    assert!(
        through_index(
            &reader
                .vectors()
                .unwrap()
                .nearest(&t, "e", &query, 10)
                .unwrap()
        ),
        "once rebuilt, the index is read again"
    );
    drop((reader, writer));
    common::drop_schema(&url, &schema);
}

#[test]
fn retiring_the_setting_drops_the_index_and_its_mark() {
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
    let two = exec(
        &url,
        format!("SELECT count(*) FROM \"{schema}\".meta WHERE k LIKE 'vindex.%' AND n = 2"),
    );
    assert_eq!(
        (two[0].get::<_, i64>(0), marks(&url, &schema).1),
        (2, 2),
        "the setting naming `e` and `f` builds and marks both"
    );
    p.begin().unwrap();
    p.put_record(&StoredRecord {
        retired: true,
        ..setting(json!(["e", "f"]), 3)
    })
    .unwrap();
    p.commit(Position(3)).unwrap();
    let units = exec(
        &url,
        format!("SELECT count(*) FROM \"{schema}\".vectors WHERE unit IS NOT NULL"),
    );
    assert_eq!(
        (marks(&url, &schema), units[0].get::<_, i64>(0)),
        ((None, 0), 0),
        "the setting retired drops every index, mark and unit vector"
    );
    let query = vector(7);
    assert_eq!(
        engine
            .vectors()
            .unwrap()
            .nearest(&TypeName::from("t"), "e", &query, 10)
            .unwrap(),
        exact(&query, 10),
        "with the setting retired, `e` answers the exact ranking"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// How many locks any session holds on `vectors` that keep another
/// session's read or write out: an index built or dropped inside an open
/// transaction holds one until its commit.
fn held(url: &str, schema: &str) -> i64 {
    let row = exec(
        url,
        format!(
            "SELECT count(*) FROM pg_locks l JOIN pg_class c ON c.oid = l.relation \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = '{schema}' AND c.relname = 'vectors' \
             AND l.mode NOT IN ('AccessShareLock', 'RowShareLock', 'RowExclusiveLock')"
        ),
    );
    row[0].get(0)
}

/// A transaction's puts are written a page at a time, so a setting may be
/// written long before its commit. Its page builds nothing: the commit
/// builds the index, once, after the last page. Until then no build's or
/// drop's lock is held on `vectors`, and a search from the reader goes on
/// reading the index the last commit left; after it every vector, put
/// before the setting or after, holds its unit vector.
#[test]
fn a_setting_written_a_page_before_its_commit_builds_the_index_at_the_commit() {
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
    let (p, index) = (engine.projection(), engine.vectors().unwrap());
    let put = |i: u64| {
        let r = record(i);
        p.put_record(&r).unwrap();
        index.put(&r.ty, &r.key, "e", &vector(i)).unwrap();
    };
    // More than a page of other puts: a setting put before them is written
    // with its page here.
    let others = |at: u64| {
        for i in 0..1_100 {
            let other = StoredRecord {
                ty: TypeName::from("u"),
                position: Position(at),
                ..record(i)
            };
            p.put_record(&other).unwrap();
        }
    };
    p.begin().unwrap();
    p.put_declaration(&declaration(), 1, None, Position(0))
        .unwrap();
    (0..RECORDS / 2).for_each(put);
    p.put_record(&setting(json!(["e"]), 1)).unwrap();
    others(1);
    (RECORDS / 2..RECORDS).for_each(put);
    assert_eq!(
        held(&url, &schema),
        0,
        "the setting's page is written and the transaction open: no index is built yet, so \
         nothing holds `vectors` against another session"
    );
    p.commit(Position(1)).unwrap();
    let unfilled = exec(
        &url,
        format!("SELECT count(*) FROM \"{schema}\".vectors WHERE field = 'e' AND unit IS NULL"),
    );
    assert_eq!(
        (marks(&url, &schema), unfilled[0].get::<_, i64>(0)),
        ((Some(1), 1), 0),
        "the commit builds and marks `e`'s index, and every vector, put before the setting \
         or after, holds its unit vector"
    );
    let (query, t) = (vector(7), TypeName::from("t"));
    decoy(&url, &schema, &query);
    assert!(
        through_index(&index.nearest(&t, "e", &query, 10).unwrap()),
        "a search of `e` reads the index the commit built"
    );
    // The setting written again, over an index that is there: its page drops
    // nothing, and a search goes on reading the index until the commit.
    p.begin().unwrap();
    p.put_record(&setting(json!(["e", "f"]), 2)).unwrap();
    others(2);
    assert_eq!(
        (held(&url, &schema), marks(&url, &schema)),
        (0, (Some(1), 1)),
        "the setting's page is written and the transaction open: `e`'s index is not dropped \
         and `f`'s not built before the commit"
    );
    assert!(
        through_index(&index.nearest(&t, "e", &query, 10).unwrap()),
        "while the transaction is open a search of `e` reads the index the last commit left"
    );
    p.commit(Position(2)).unwrap();
    assert_eq!(
        marks(&url, &schema),
        (Some(2), 2),
        "the commit builds the indexes the setting asks for, `e`'s again and `f`'s"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

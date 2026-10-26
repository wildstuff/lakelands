//! A projection an older build laid out (lakelands#1151): its `routes`
//! naming kinds, types and sources in text, its states without the `procedure` a
//! computed field's metadata names (lakelands#1191), its states holding
//! their rows' type, key and position and their false flags
//! (lakelands#1318), no table `bases` (lakelands#1432), and its mark one
//! layout back. A reader's open is refused naming the layouts; the
//! writer's open rebuilds it from the log under its lease, saying why,
//! and the next writer's open rebuilds nothing. A projection layout 6
//! kept under a vector index (unit vectors in `vectors`, a partial HNSW
//! index per field there, the field's mark in `meta`, no graph) is
//! rebuilt with its graphs, and its pages are what they were.

mod common;

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::error::Situation;
use lakelands_api::id::TypeName;
use lakelands_api::ontology::{
    Authority, FieldDecl,
    StorageKind::{Decimal, Text},
};
use lakelands_api::principal::Principal;
use lakelands_api::query::{Search, SearchMode};
use lakelands_api::read::ReadAt;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::fixture::{
    Lake, computed_by, reads_all, restrict, source_record, sourced, type_decl, vector,
};
use lakelands_core::engines::Elements;
use lakelands_engine_pg::PgOpener;
use serde_json::json;

async fn open(location: &Location, mode: OpenMode) -> lakelands_api::Result<Arc<dyn LakeStore>> {
    let seams = Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    };
    lakelands_store::open(location.clone(), seams, mode).await
}

/// The first column of the first row `statement` answers.
fn one(url: &str, statement: &str) -> i64 {
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    client.query_one(statement, &[]).unwrap().get(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writers_open_rebuilds_a_projection_an_older_build_laid_out() {
    let Some(url) = common::url_or_skip("older layout") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "older", &schema);
    let store = open(&location, OpenMode::Create).await.unwrap();
    let l = Lake::of(store.as_ref());
    let twice = json!({"declare": "function", "name": "twice", "backing": "expr",
        "inputs": [{"name": "n", "type": {"kind": "decimal"}}], "expr": "n * 2"});
    let twice = serde_json::from_value(twice).unwrap();
    l.declare_as(twice, "declaring `twice`").await.unwrap();
    let fields = vec![
        sourced("text", Text),
        sourced("n", Decimal),
        computed_by("double", Decimal, "twice", &["n"]),
    ];
    l.declare(type_decl("note", fields)).await.unwrap();
    let a = source_record("note", "a", json!({"text": "kept", "n": "2"}));
    l.ingest("crm", vec![a]).await.unwrap();
    store.close().await.unwrap();
    let layout = format!("SELECT n FROM \"{schema}\".meta WHERE k = 'layout'");
    let wanted = tokio::task::block_in_place(|| one(&url, &layout));
    let older = format!(
        "DROP TABLE \"{schema}\".routes; DROP TABLE \"{schema}\".names; \
         DROP TABLE \"{schema}\".bases; \
         CREATE TABLE \"{schema}\".routes (at BIGINT NOT NULL, kind TEXT NOT NULL, ty TEXT, \
         key TEXT, source TEXT); \
         CREATE INDEX routes_source ON \"{schema}\".routes (source, at); \
         UPDATE \"{schema}\".meta SET n = {} WHERE k = 'layout'",
        wanted - 1
    );
    let unnamed = format!(
        "UPDATE \"{schema}\".records SET state = (jsonb_build_object('type', ty, 'key', key, \
         'position', since, 'retired', false, 'restricted', false) \
         || replace(state, '\"procedure\":\"twice\",', '')::jsonb)::text"
    );
    tokio::task::block_in_place(|| {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        let n = client.execute(&unnamed, &[]).unwrap();
        assert!(n > 0, "a state names `twice` as its procedure");
        client.batch_execute(&older).unwrap();
    });
    let refused = open(&location, OpenMode::Read).await.err().unwrap();
    assert!(
        matches!(refused.situation(), Some(Situation::ProjectionLayout { found, .. }) if *found == Some(wanted as u64 - 1)),
        "a reader's open of the older layout is refused: {refused:?}"
    );
    let store = open(&location, OpenMode::Write).await.unwrap();
    let why = store.rebuilt_at_open();
    assert!(
        why.as_ref()
            .is_some_and(|w| w.contains(&format!("layout {}", wanted - 1))),
        "the writer's open says why it rebuilt: {why:?}"
    );
    let note = Lake::of(store.as_ref()).read("note", "a").await.unwrap();
    assert_eq!(note.fields["text"], "kept");
    let procedure = note.meta["double"].procedure.as_deref();
    assert_eq!(procedure, Some("twice"), "the rebuild names the function");
    let sourced = format!(
        "SELECT count(*) FROM \"{schema}\".routes r JOIN \"{schema}\".names n ON n.id = r.source \
         WHERE n.name = 'crm' AND pg_typeof(r.kind) = 'integer'::regtype"
    );
    let (sourced, now) = tokio::task::block_in_place(|| (one(&url, &sourced), one(&url, &layout)));
    assert!(
        sourced > 0 && now == wanted,
        "the rebuild indexed crm's events at layout {wanted}"
    );
    let held = format!(
        "SELECT count(*) FROM \"{schema}\".records WHERE state::jsonb ?| \
         array['type', 'key', 'position'] OR state::jsonb @> '{{\"retired\": false}}' \
         OR state::jsonb @> '{{\"restricted\": false}}'"
    );
    let held = tokio::task::block_in_place(|| one(&url, &held));
    assert_eq!(
        held, 0,
        "no state holds what its row holds, or a false flag"
    );
    let tables = format!(
        "SELECT count(*) FROM pg_tables WHERE schemaname = '{schema}' AND tablename = 'bases'"
    );
    let bases = tokio::task::block_in_place(|| one(&url, &tables));
    assert_eq!(bases, 1, "the rebuild lays out `bases`");
    store.close().await.unwrap();
    let store = open(&location, OpenMode::Write).await.unwrap();
    assert_eq!(
        store.rebuilt_at_open(),
        None,
        "the next writer's open rebuilds nothing"
    );
    store.close().await.unwrap();
    common::drop_schema(&url, &schema);
}

/// The points of the vector lake, and their dimensions.
const POINTS: u64 = 400;
const DIMENSIONS: usize = 8;

/// The `i`th point's vector: small whole numbers, never all zeros.
fn point(i: u64) -> Vec<i64> {
    let mut z = i.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..DIMENSIONS)
        .map(|_| {
            z = (z ^ (z >> 29)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            ((z >> 40) % 17) as i64 - 8
        })
        .map(|x| if x == 0 { 1 } else { x })
        .collect()
}

/// Twelve pages of ten of `point.v` nearest `q` for `who`: each hit's
/// key and score bits, and whether every page said `index`.
async fn pages(l: &Lake, who: &Principal, q: &[f32]) -> (Vec<(String, u64)>, bool) {
    let (mut got, mut cursor, mut indexed) = (Vec::new(), None, true);
    for _ in 0..12 {
        let asked = Search {
            ty: TypeName::from("point"),
            mode: SearchMode::vector("v", q.to_vec()),
            k: 10,
            cursor,
            filter: None,
            rerank: None,
            with_meta: false,
            matches: false,
            at: ReadAt::default(),
        };
        let page = l.cat.search(who, asked).await.unwrap().data;
        indexed &= page.ranked_by == "index";
        got.extend(
            page.hits
                .iter()
                .map(|h| (h.record.key.0.clone(), h.score.to_bits())),
        );
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    (got, indexed)
}

/// Wait until the builder has built A and B of `point.v`, none stale.
fn settled(url: &str, schema: &str) {
    let done = format!(
        "SELECT count(*) FROM \"{schema}\".graphs \
         WHERE gen IS NOT NULL AND next IS NULL AND stale IS NULL"
    );
    let asked = std::time::Instant::now();
    while one(url, &done) != 2 {
        assert!(
            asked.elapsed() < std::time::Duration::from_secs(60),
            "A and B are built within a minute"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writers_open_rebuilds_a_layout_6_vector_index_with_its_pages_kept() {
    let Some(url) = common::url_or_skip("older vector layout") else {
        return;
    };
    let ext = tokio::task::block_in_place(|| {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        let _ = client.batch_execute("CREATE EXTENSION IF NOT EXISTS vector");
        let found =
            "SELECT extnamespace::regnamespace::text FROM pg_extension WHERE extname = 'vector'";
        client
            .query_opt(found, &[])
            .unwrap()
            .map(|r| r.get::<_, String>(0))
    });
    let Some(ext) = ext else {
        eprintln!("older vector layout: the server has no pgvector");
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "older", &schema);
    let store = open(&location, OpenMode::Create).await.unwrap();
    let l = Lake::of(store.as_ref());
    let v = FieldDecl {
        authority: Authority::Source,
        ..vector("v", DIMENSIONS as u32)
    };
    l.declare(type_decl("point", vec![v])).await.unwrap();
    let key = |i: u64| format!("p{i:04}");
    let points = (0..POINTS).map(|i| source_record("point", &key(i), json!({"v": point(i)})));
    l.ingest("points", points.collect()).await.unwrap();
    l.set("vector_index.point", json!(["v"])).await.unwrap();
    let held: Vec<u64> = (0..POINTS).filter(|i| i % 40 == 7).collect();
    for i in &held {
        restrict(store.as_ref(), "point", key(*i), "held")
            .await
            .unwrap();
    }
    tokio::task::block_in_place(|| settled(&url, &schema));
    let reader = reads_all(store.as_ref(), "reader");
    let queries: Vec<Vec<f32>> = (0..3)
        .map(|q| point(POINTS + q).iter().map(|x| *x as f32).collect())
        .collect();
    let mut before = Vec::new();
    for q in &queries {
        for who in [&l.own, &reader] {
            before.push(pages(&l, who, q).await);
        }
    }
    store.close().await.unwrap();

    // Layout 6's shape, as its build left it: the unit vectors of the
    // field's rows in force, its partial index and mark, and no graph.
    let s = format!("\"{schema}\"");
    tokio::task::block_in_place(|| {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        let graphs = format!(
            "SELECT string_agg(format('{s}.%I', relname), ', ') FROM pg_class \
             WHERE relnamespace = '{s}'::regnamespace AND relkind = 'r' AND relname LIKE 'vgraph%'"
        );
        let graphs: String = client.query_one(&graphs, &[]).unwrap().get(0);
        client
            .batch_execute(&format!(
                "DROP TABLE {s}.graphs, {s}.graph_keys, {graphs}; \
                 ALTER TABLE {s}.vectors ADD COLUMN unit {ext}.vector"
            ))
            .unwrap();
        let rows = format!(
            "SELECT key, single, data FROM {s}.vectors \
             WHERE ty = 'point' AND field = 'v' AND until IS NULL"
        );
        let (keys, units): (Vec<String>, Vec<String>) = (client.query(&rows, &[]).unwrap())
            .iter()
            .map(|r| {
                let e = Elements::new(r.get(1), r.get(2)).ranked();
                let norm = e.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
                let unit: Vec<String> = (e.iter())
                    .map(|x| ((f64::from(*x) / norm) as f32).to_string())
                    .collect();
                (r.get(0), format!("[{}]", unit.join(",")))
            })
            .unzip();
        assert_eq!(
            keys.len() as u64,
            POINTS,
            "every point keeps its vector in force"
        );
        let fill = format!(
            "UPDATE {s}.vectors AS t SET unit = CAST(u.unit AS {ext}.vector) \
             FROM unnest($1::text[], $2::text[]) AS u (key, unit) \
             WHERE t.ty = 'point' AND t.field = 'v' AND t.key = u.key AND t.until IS NULL"
        );
        client.execute(&fill, &[&keys, &units]).unwrap();
        use sha2::{Digest as _, Sha256};
        let index = lakelands_core::canonical::hex(&Sha256::digest(b"point\0v")[..8]);
        client
            .batch_execute(&format!(
                "CREATE INDEX vindex_{index} ON {s}.vectors \
                 USING hnsw ((unit::{ext}.vector({DIMENSIONS})) {ext}.vector_l2_ops) \
                 WITH (m = 16, ef_construction = 64) \
                 WHERE ty = 'point' AND field = 'v' AND until IS NULL; \
                 INSERT INTO {s}.meta (k, n, t) \
                 SELECT 'vindex.point.v', n, '{DIMENSIONS}' FROM {s}.meta WHERE k = 'folded'; \
                 UPDATE {s}.meta SET n = 6 WHERE k = 'layout'"
            ))
            .unwrap();
    });

    let store = open(&location, OpenMode::Write).await.unwrap();
    let why = store.rebuilt_at_open();
    assert!(
        why.as_ref().is_some_and(|w| w.contains("layout 6")),
        "the writer's open rebuilds layout 6, saying so: {why:?}"
    );
    tokio::task::block_in_place(|| settled(&url, &schema));
    let left = format!(
        "SELECT (SELECT count(*) FROM pg_indexes WHERE schemaname = '{schema}' \
         AND indexname LIKE 'vindex%') + (SELECT count(*) FROM information_schema.columns \
         WHERE table_schema = '{schema}' AND table_name = 'vectors' AND column_name = 'unit') \
         + (SELECT count(*) FROM {s}.meta WHERE k LIKE 'vindex.%')"
    );
    assert_eq!(
        tokio::task::block_in_place(|| one(&url, &left)),
        0,
        "no index, unit vector or mark of layout 6 is left"
    );
    let l = Lake::of(store.as_ref());
    let reader = reads_all(store.as_ref(), "reader");
    let mut after = Vec::new();
    for q in &queries {
        for who in [&l.own, &reader] {
            after.push(pages(&l, who, q).await);
        }
    }
    assert!(
        before
            .iter()
            .all(|(hits, indexed)| *indexed && hits.len() == 120),
        "every page says `index` and holds its ten hits"
    );
    let seen =
        |hits: &[(String, u64)]| hits.iter().any(|(k, _)| held.iter().any(|i| *k == key(*i)));
    assert!(
        seen(&before[0].0) && !before.iter().skip(1).step_by(2).any(|(h, _)| seen(h)),
        "the owner's pages hold a held point and the reader's none"
    );
    assert_eq!(
        after, before,
        "the owner's and the reader's pages are what they were"
    );
    store.close().await.unwrap();
    common::drop_schema(&url, &schema);
}

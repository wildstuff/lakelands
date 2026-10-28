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
//! rebuilt with its graphs, and its pages are what they were. A lake of
//! format 19, its log without `frames` and its projection one layout
//! back, as 0.19.0 left it, is upgraded by the first writer's open
//! (lakelands#1467). A lake 0.20.0 left, of format 20 with its `id`,
//! `prev` and `hash` as text beside `applied`, is read as it is and
//! converted in place by the first writer's open, once, as it upgrades,
//! every event, digest and export as before (lakelands#1518).

mod common;

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::error::Situation;
use lakelands_api::event::{EventBody, LOG_FORMAT_VERSION};
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
    Lake, computed_by, log_events, reads_all, restrict, source_record, sourced, type_decl, vector,
};
use lakelands_core::engines::Elements;
use lakelands_engine_pg::PgOpener;
use serde_json::json;

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    }
}

async fn open(location: &Location, mode: OpenMode) -> lakelands_api::Result<Arc<dyn LakeStore>> {
    lakelands_store::open(location.clone(), seams(), mode).await
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

/// The tables of a lake 0.19.0 made, which opens no schema holding others.
const V0_19_TABLES: &str = "applied declarations declarations_head edges full_text graph_keys \
    graphs lease log lookup_fields members meta outputs proposals records records_head routes \
    segments sort_index text_index vectors";

/// The first writer's open of a lake as 0.19.0 left it: a log of format
/// 19 with no `frames` table, under a projection one layout back. It
/// rebuilds the projection, makes the log's tables this build adds and
/// appends the upgrade, in one try (lakelands#1467).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_format_19_lake_an_older_build_laid_out_upgrades_on_the_first_writers_open() {
    let Some(url) = common::url_or_skip("format 19 upgrade") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "vectors", &schema);
    let vectors =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/vectors/19");
    lakelands_store::import(location.clone(), seams(), vectors)
        .await
        .unwrap();
    // The import upgraded the log: its events after the vectors' eight,
    // and every table and the layout 0.19.0 did not have, are taken back.
    let s = format!("\"{schema}\"");
    let older = format!(
        "DELETE FROM {s}.log WHERE position > 7; \
         DO $$ DECLARE t TEXT; BEGIN FOR t IN SELECT tablename FROM pg_tables \
         WHERE schemaname = '{schema}' AND tablename <> ALL (string_to_array('{V0_19_TABLES}', ' ')) \
         LOOP EXECUTE format('DROP TABLE {s}.%I', t); END LOOP; END $$; \
         DROP TABLE {s}.routes; \
         CREATE TABLE {s}.routes (at BIGINT NOT NULL, kind TEXT NOT NULL, ty TEXT, \
         key TEXT, source TEXT); \
         UPDATE {s}.meta SET n = 7 WHERE k IN ('folded', 'layout'); {}",
        as_text(&s)
    );
    let tables = format!(
        "SELECT string_agg(tablename, ' ' ORDER BY tablename COLLATE \"C\") FROM pg_tables \
         WHERE schemaname = '{schema}'"
    );
    let as_left = tokio::task::block_in_place(|| {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        client.batch_execute(&older).unwrap();
        client.query_one(&tables, &[]).unwrap().get::<_, String>(0)
    });
    assert_eq!(as_left, V0_19_TABLES, "the lake holds 0.19.0's tables");
    // A rebuild refused before its swap leaves the lake as 0.19.0 opens
    // it: here, while another session holds the builder's lock.
    let (locked, release) = (std::sync::mpsc::channel(), std::sync::mpsc::channel::<()>());
    let holder = {
        let (url, schema, locked) = (url.clone(), schema.clone(), locked.0);
        std::thread::spawn(move || {
            let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
            let builder =
                "SELECT pg_advisory_lock(hashtextextended('lakelands rebuild ' || $1, 0))";
            client.query_one(builder, &[&schema]).unwrap();
            locked.send(()).unwrap();
            let _ = release.1.recv();
        })
    };
    tokio::task::block_in_place(|| locked.1.recv()).unwrap();
    let refused = open(&location, OpenMode::Write).await.err();
    assert!(
        matches!(
            refused.as_ref().and_then(|e| e.situation()),
            Some(Situation::Rebuilding { .. })
        ),
        "the writer's open is refused while the builder's lock is held: {refused:?}"
    );
    let unchanged = tokio::task::block_in_place(|| {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        client.query_one(&tables, &[]).unwrap().get::<_, String>(0)
    });
    assert_eq!(
        unchanged, V0_19_TABLES,
        "a refused upgrade leaves 0.19.0's tables and no other"
    );
    let left = tokio::task::block_in_place(|| kept(&url, &schema));
    assert_eq!(left, AS_TEXT, "a refused upgrade leaves the log's columns");
    drop(release.0);
    tokio::task::block_in_place(|| holder.join()).unwrap();
    let store = open(&location, OpenMode::Write).await.unwrap();
    let why = store.rebuilt_at_open();
    assert!(
        why.as_ref().is_some_and(|w| w.contains("layout 7")),
        "the writer's open says why it rebuilt: {why:?}"
    );
    let log = log_events(store.as_ref()).await.unwrap();
    let marked = log.iter().filter(
        |e| matches!(e.body, EventBody::LakeUpgraded { from: 19, to } if to == LOG_FORMAT_VERSION),
    );
    assert_eq!(marked.count(), 1, "one `lake_upgraded` from 19: {log:?}");
    let frames = format!(
        "SELECT count(*) FROM pg_tables WHERE schemaname = '{schema}' AND tablename = 'frames'"
    );
    assert_eq!(
        tokio::task::block_in_place(|| one(&url, &frames)),
        1,
        "the upgrade makes `frames`"
    );
    store.verify(None, None).await.unwrap();
    let kept = tokio::task::block_in_place(|| kept(&url, &schema));
    assert_eq!(kept, CONVERTED, "the rebuild's swap converts the log");
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

/// The log taken back to 0.20.0's: text columns, `applied`, no `log_id`.
fn as_text(s: &str) -> String {
    format!(
        "DROP INDEX {s}.log_id; \
         ALTER TABLE {s}.log ALTER COLUMN id TYPE text USING encode(id, 'hex'), \
         ALTER COLUMN prev TYPE text USING encode(prev, 'hex'), \
         ALTER COLUMN hash TYPE text USING encode(hash, 'hex'); \
         CREATE TABLE {s}.applied (id TEXT PRIMARY KEY, at BIGINT NOT NULL); \
         INSERT INTO {s}.applied SELECT id, min(position) FROM {s}.log GROUP BY id"
    )
}

const AS_TEXT: &str = "text text text t f";
const CONVERTED: &str = "bytea bytea bytea f t";

/// The types of `hash`, `id` and `prev`, `applied` there, `log_id` there.
fn kept(url: &str, schema: &str) -> String {
    let s = format!("\"{schema}\"");
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    let kept = format!(
        "SELECT concat_ws(' ', (SELECT string_agg(format_type(atttypid, NULL), ' ' \
         ORDER BY attname) FROM pg_attribute WHERE attrelid = '{s}.log'::regclass \
         AND attname IN ('hash', 'id', 'prev')), to_regclass('{s}.applied') IS NOT NULL, \
         to_regclass('{s}.log_id') IS NOT NULL)"
    );
    client.query_one(&kept, &[]).unwrap().get(0)
}

/// `log`'s rows, hex as hex in either type, the digests, the table's file.
fn rows(url: &str, schema: &str) -> (String, String, i64) {
    let s = format!("\"{schema}\"");
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    let hex = |c: &str| format!("ltrim({c}::text, '\\x')");
    let rows = format!(
        "SELECT (SELECT string_agg(concat_ws(' ', position, segment, fence, {}, {}, {}, \
         encode(bytes, 'hex')), E'\\n' ORDER BY position) FROM {s}.log), \
         (SELECT string_agg(first || ' ' || coalesce(digest, '-'), ',' ORDER BY first) \
         FROM {s}.segments), (SELECT relfilenode::int8 FROM pg_class WHERE oid = '{s}.log'::regclass)",
        hex("id"),
        hex("prev"),
        hex("hash")
    );
    let row = client.query_one(&rows, &[]).unwrap();
    (row.get(0), row.get(1), row.get(2))
}

/// The files of an export's `log/`, each with its bytes, in order.
fn exported(location: &Location, dir: &std::path::Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
    let _ = std::fs::remove_dir_all(dir);
    lakelands_engine_pg::export(location, dir).unwrap();
    let files = std::fs::read_dir(dir.join("log"))
        .unwrap()
        .map(|f| f.unwrap());
    let read = |f: std::fs::DirEntry| (f.file_name(), std::fs::read(f.path()).unwrap());
    let mut files: Vec<_> = files.map(read).collect();
    files.sort();
    files
}

/// A lake as 0.20.0 left it, of format 20 on text columns beside
/// `applied`: its ends are read as they are and a reader is refused
/// until it is upgraded; a conversion that fails, on an id not of its
/// form or a `prev` that spells no bytes, leaves it; the first writer's
/// open converts it as it takes the lease and appends one `lake_upgraded`
/// from 20, every event, digest and export as before, each id the log
/// held answering where it stands; the open after rewrites nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lake_0_20_0_left_is_converted_in_place_by_the_first_writers_open() {
    use lakelands_api::engine::EngineOpener;
    use lakelands_api::error::{Flaw, Part};
    use lakelands_api::event::EventId;
    use lakelands_api::id::Position;
    let Some(url) = common::url_or_skip("log of 0.20.0") else {
        return;
    };
    let schema = common::fresh_schema();
    let s = format!("\"{schema}\"");
    let location = common::location(&url, "vectors", &schema);
    let run = |sql: &str| {
        let connect = || postgres::Client::connect(&url, postgres::NoTls).unwrap();
        tokio::task::block_in_place(|| connect().batch_execute(sql).unwrap());
    };
    let kept = || tokio::task::block_in_place(|| kept(&url, &schema));
    let engine = |mode| tokio::task::block_in_place(|| PgOpener.open(&location, mode).unwrap());
    // Two segments, the first sealed: the vectors' eight events, and none
    // of the import's upgrade.
    let vectors =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/vectors/20");
    lakelands_store::import(location.clone(), seams(), vectors)
        .await
        .unwrap();
    run(&format!(
        "DELETE FROM {s}.log WHERE position > 7; DROP TABLE {s}.quarantined; \
         UPDATE {s}.meta SET n = 7 WHERE k = 'folded'"
    ));
    let events = tokio::task::block_in_place(|| engine(OpenMode::Read).read(Position(0), 100));
    let events = events.unwrap();
    let scratch = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(&schema);
    let taken = |at: &str| {
        let export = || exported(&location, &scratch.join(at));
        tokio::task::block_in_place(|| (rows(&url, &schema), export()))
    };
    let (before, export) = taken("before");
    run(&as_text(&s));
    assert_eq!(kept(), AS_TEXT);
    // Read as it is, both ends by 20; a reader of the store is refused.
    let ends = PgOpener.log_ends(&location).unwrap().unwrap();
    let v = |e: &[u8]| serde_json::from_slice::<serde_json::Value>(e).unwrap()["v"].clone();
    assert_eq!([&ends.0, &ends.1], [&events[0], &events[7]], "the ends");
    assert_eq!([v(&ends.0.bytes), v(&ends.1.bytes)], [json!(20), json!(20)]);
    let read = tokio::task::block_in_place(|| engine(OpenMode::Read).read(Position(0), 100));
    assert_eq!(read.unwrap(), events, "the engine reads it as it is");
    let refused = open(&location, OpenMode::Read).await.err();
    assert!(
        matches!(
            refused.as_ref().and_then(|e| e.situation()),
            Some(Situation::LogUpgradeOwed { found: 20, .. })
        ),
        "a reader waits for the writer: {refused:?}"
    );
    // Two conversions that fail, each leaving the log as it was.
    let planted = [
        "SET id = upper(id) WHERE position = 3",
        "SET id = lower(id)",
        "SET prev = 'zz' || prev WHERE position = 5",
        "SET prev = ltrim(prev, 'z')",
    ];
    for (n, [plant, undo]) in planted.as_chunks().0.iter().enumerate() {
        run(&format!("UPDATE {s}.log {plant}"));
        let refused = open(&location, OpenMode::Write).await.err().unwrap();
        let situation = refused.situation();
        assert!(
            n > 0
                || matches!(situation, Some(Situation::Damaged { part: Part::Log, position: Some(p), flaw: Flaw::IdNotOfForm }) if p.0 == 3),
            "an id not of its form is refused where it is: {refused:?}"
        );
        assert_eq!(kept(), AS_TEXT, "left as it was: {refused:?}");
        run(&format!("UPDATE {s}.log {undo}"));
    }
    let store = open(&location, OpenMode::Write).await.unwrap();
    assert_eq!(kept(), CONVERTED);
    assert_eq!(
        store.rebuilt_at_open(),
        None,
        "converted as it takes the lease"
    );
    let read = log_events(store.as_ref()).await.unwrap();
    let marked = read.iter().filter(
        |e| matches!(e.body, EventBody::LakeUpgraded { from: 20, to } if to == LOG_FORMAT_VERSION),
    );
    assert_eq!(marked.count(), 1, "one `lake_upgraded` from 20: {read:?}");
    store.verify(None, None).await.unwrap();
    store.close().await.unwrap();
    let (after, again) = taken("after");
    // The open appended its `lease_taken` and `lake_upgraded` after them.
    assert!(after.0.starts_with(&before.0), "every event as before");
    assert_eq!(after.1, before.1, "every segment's digest as before");
    let (sealed, active) = export.split_at(export.len() - 1);
    assert_eq!(again.len(), export.len(), "the export's files as before");
    assert_eq!(&again[..sealed.len()], sealed, "the sealed ones'");
    assert_eq!(again[sealed.len()].0, active[0].0);
    assert!(again[sealed.len()].1.starts_with(&active[0].1));
    let reader = engine(OpenMode::Read);
    let read = tokio::task::block_in_place(|| reader.read(Position(0), 100)).unwrap();
    assert_eq!(read[..events.len()], events[..]);
    // Each id the log held before answers where it stands.
    let mut ids: Vec<EventId> = events.iter().map(|e| e.id.clone()).collect();
    ids.push(EventId(format!("{:032x}", 1)));
    let applied = tokio::task::block_in_place(|| reader.view(None)?.applied_many(&ids));
    let mut at: Vec<_> = events.iter().map(|e| Some(e.position)).collect();
    at.push(None);
    assert_eq!(applied.unwrap(), at, "each id where it stands");
    drop(reader);
    let first = tokio::task::block_in_place(|| rows(&url, &schema));
    let store = open(&location, OpenMode::Write).await.unwrap();
    store.close().await.unwrap();
    let second = tokio::task::block_in_place(|| rows(&url, &schema));
    assert_eq!(second.2, first.2, "the next open rewrites nothing");
    let _ = std::fs::remove_dir_all(&scratch);
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
            text_as_of: false,
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

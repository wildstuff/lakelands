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
//! (lakelands#1467). A log 0.20.0 kept, its `id`, `prev` and `hash` as
//! text beside `applied`, is read as it is and converted in place by the
//! first writer's open, once, every event, digest and export as before
//! (lakelands#1518).

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
    let seams = Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    };
    let vectors =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/vectors/19");
    lakelands_store::import(location.clone(), seams, vectors)
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
    assert_eq!(
        tokio::task::block_in_place(|| kept(&url, &schema)),
        AS_TEXT,
        "a refused upgrade leaves the log's columns as they were"
    );
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
    assert_eq!(
        tokio::task::block_in_place(|| kept(&url, &schema)),
        CONVERTED,
        "the rebuild's swap converts the log"
    );
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

/// The statements that take a lake's log back to the columns a release
/// before 0.21.0 kept: `id`, `prev` and `hash` as text, no index on `id`,
/// and `applied` holding each id at its first position.
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

/// What `kept` answers of a log this build converted.
const CONVERTED: (&str, bool, bool) = ("bytea bytea bytea", false, true);

/// The types `log` keeps `hash`, `id` and `prev` in, whether `applied` is
/// there, and whether the index on `log (id)` is.
fn kept(url: &str, schema: &str) -> (&'static str, bool, bool) {
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    let row = client
        .query_one(
            "SELECT (SELECT string_agg(format_type(atttypid, NULL), ' ' ORDER BY attname) \
             FROM pg_attribute WHERE attrelid = to_regclass($1) \
             AND attname IN ('hash', 'id', 'prev')), \
             to_regclass($2) IS NOT NULL, to_regclass($3) IS NOT NULL",
            &[
                &format!("\"{schema}\".log"),
                &format!("\"{schema}\".applied"),
                &format!("\"{schema}\".log_id"),
            ],
        )
        .unwrap();
    let types = match row.get::<_, String>(0).as_str() {
        "bytea bytea bytea" => "bytea bytea bytea",
        "text text text" => "text text text",
        other => panic!("the log's columns are of no one release: {other}"),
    };
    (types, row.get(1), row.get(2))
}

/// What a log 0.20.0 kept answers of `kept`.
const AS_TEXT: (&str, bool, bool) = ("text text text", true, false);

/// Every row of `log` and of `segments` as text, the log's hex columns as
/// the hex they spell whatever they are kept in, and the log's table's
/// file, which a conversion writes anew.
fn rows(url: &str, schema: &str) -> (String, String, u32) {
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    let s = format!("\"{schema}\"");
    let (id, prev, hash) = (
        typed(&mut client, &s, "id"),
        typed(&mut client, &s, "prev"),
        typed(&mut client, &s, "hash"),
    );
    let log = client
        .query_one(
            &format!(
                "SELECT string_agg(concat_ws(' ', position, segment, fence, \
                 {id}, {prev}, {hash}, encode(bytes, 'hex')), E'\\n' ORDER BY position) \
                 FROM {s}.log"
            ),
            &[],
        )
        .unwrap()
        .get(0);
    let segments = client
        .query_one(
            &format!(
                "SELECT string_agg(first || ' ' || coalesce(digest, '-'), ',' ORDER BY first) \
                 FROM {s}.segments"
            ),
            &[],
        )
        .unwrap()
        .get(0);
    let file = client
        .query_one(
            &format!("SELECT relfilenode::int8 FROM pg_class WHERE oid = '{s}.log'::regclass"),
            &[],
        )
        .unwrap()
        .get::<_, i64>(0);
    (log, segments, file as u32)
}

/// `column` of `{s}.log` as the hex it spells, read in its type.
fn typed(client: &mut postgres::Client, s: &str, column: &str) -> String {
    let bytes: bool = client
        .query_one(
            "SELECT atttypid = 'bytea'::regtype FROM pg_attribute \
             WHERE attrelid = to_regclass($1) AND attname = $2",
            &[&format!("{s}.log"), &column],
        )
        .unwrap()
        .get(0);
    match bytes {
        true => format!("encode({column}, 'hex')"),
        false => column.to_owned(),
    }
}

/// The files of an export's `log/`, each with its bytes, in order.
fn exported(location: &Location, dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let _ = std::fs::remove_dir_all(dir);
    lakelands_engine_pg::export(location, dir).unwrap();
    let mut files: Vec<_> = std::fs::read_dir(dir.join("log"))
        .unwrap()
        .map(|f| f.unwrap().path())
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

/// A log as 0.20.0 kept it, `id`, `prev` and `hash` as text beside
/// `applied`, under the tables of its schema: its ends and its events are
/// read as they are, by a reader and by a build ahead. The first writer's
/// open converts it in place, and every event, every segment's digest and
/// the export are as they were. A conversion that fails, on an id not of
/// its form or midway on a `prev` that spells no bytes, leaves the log as
/// it was, and the next open converts it; the open after that changes
/// nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_log_0_20_0_kept_is_converted_in_place_by_the_first_writers_open() {
    use lakelands_api::engine::EngineOpener;
    use lakelands_api::error::{Flaw, Part};
    use lakelands_api::id::Position;
    let Some(url) = common::url_or_skip("log of 0.20.0") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "vectors", &schema);
    let seams = || Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    };
    // Two segments, the first sealed, and the events a writer appends after.
    let vectors =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/vectors/20");
    lakelands_store::import(location.clone(), seams(), vectors)
        .await
        .unwrap();
    let store = open(&location, OpenMode::Write).await.unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("note", vec![sourced("text", Text)]))
        .await
        .unwrap();
    for n in 0..3 {
        let note = source_record("note", &format!("n{n}"), json!({"text": "kept"}));
        l.ingest("crm", vec![note]).await.unwrap();
    }
    let events = log_events(store.as_ref()).await.unwrap();
    store.close().await.unwrap();
    let scratch = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(&schema);
    let (before, export) = tokio::task::block_in_place(|| {
        let before = rows(&url, &schema);
        let export = exported(&location, &scratch.join("before"));
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        let s = format!("\"{schema}\"");
        client
            .batch_execute(&format!("DROP TABLE {s}.quarantined; {}", as_text(&s)))
            .unwrap();
        (before, export)
    });
    assert_eq!(tokio::task::block_in_place(|| kept(&url, &schema)), AS_TEXT);
    // Read as it is: its ends, by a reader, and by a build ahead.
    let ends = PgOpener.log_ends(&location).unwrap().unwrap();
    let (first, last) = (&events[0], events.last().unwrap());
    assert_eq!(
        (ends.0.position, &ends.0.id, ends.1.position, &ends.1.hash),
        (first.position, &first.id, last.position, &last.hash),
        "the ends"
    );
    let reader = open(&location, OpenMode::Read).await.unwrap();
    assert_eq!(log_events(reader.as_ref()).await.unwrap(), events);
    reader.close().await.unwrap();
    lakelands_store::build_ahead(location.clone(), seams())
        .await
        .unwrap();
    assert_eq!(tokio::task::block_in_place(|| kept(&url, &schema)), AS_TEXT);
    // Two conversions that fail, each leaving the log as it was.
    let s = format!("\"{schema}\"");
    let planted = [
        (
            format!("UPDATE {s}.log SET id = upper(id) WHERE position = 3"),
            format!("UPDATE {s}.log SET id = lower(id) WHERE position = 3"),
        ),
        (
            format!("UPDATE {s}.log SET prev = 'zz' || prev WHERE position = 5"),
            format!("UPDATE {s}.log SET prev = substr(prev, 3) WHERE position = 5"),
        ),
    ];
    for (n, (plant, undo)) in planted.iter().enumerate() {
        tokio::task::block_in_place(|| {
            let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
            client.batch_execute(plant).unwrap();
        });
        let refused = open(&location, OpenMode::Write).await.err().unwrap();
        if n == 0 {
            assert!(
                matches!(
                    refused.situation(),
                    Some(Situation::Damaged {
                        part: Part::Log,
                        position: Some(Position(3)),
                        flaw: Flaw::IdNotOfForm
                    })
                ),
                "an id not of its form is refused where it is: {refused:?}"
            );
        }
        tokio::task::block_in_place(|| {
            assert_eq!(
                kept(&url, &schema),
                AS_TEXT,
                "a failed conversion leaves the log: {refused:?}"
            );
            let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
            client.batch_execute(undo).unwrap();
        });
    }
    let store = open(&location, OpenMode::Write).await.unwrap();
    let (after, again) = tokio::task::block_in_place(|| {
        (
            rows(&url, &schema),
            exported(&location, &scratch.join("after")),
        )
    });
    assert_eq!(
        tokio::task::block_in_place(|| kept(&url, &schema)),
        CONVERTED
    );
    // The open appended its `lease_taken`, after every event it found.
    assert!(after.0.starts_with(&before.0), "every event as before");
    assert_eq!(after.1, before.1, "every segment's digest as before");
    let names = |files: &[(String, Vec<u8>)]| files.iter().map(|f| f.0.clone()).collect::<Vec<_>>();
    assert_eq!(
        names(&again),
        names(&export),
        "the export's files as before"
    );
    let (sealed, active) = export.split_at(export.len() - 1);
    assert_eq!(
        &again[..sealed.len()],
        sealed,
        "the sealed segments' exports as before"
    );
    assert!(
        again[sealed.len()].1.starts_with(&active[0].1),
        "the active segment's as before"
    );
    assert_eq!(
        log_events(store.as_ref()).await.unwrap()[..events.len()],
        events[..]
    );
    store.verify(None, None).await.unwrap();
    // The fold marks what it folds, and the log answers it.
    let l = Lake::of(store.as_ref());
    let note = source_record("note", "n3", json!({"text": "kept"}));
    l.ingest("crm", vec![note]).await.unwrap();
    assert_eq!(l.read("note", "n3").await.unwrap().fields["text"], "kept");
    store.close().await.unwrap();
    let first = tokio::task::block_in_place(|| rows(&url, &schema));
    let store = open(&location, OpenMode::Write).await.unwrap();
    store.close().await.unwrap();
    let second = tokio::task::block_in_place(|| rows(&url, &schema));
    assert_eq!(
        second.2, first.2,
        "the open after the conversion rewrites nothing"
    );
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

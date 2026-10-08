//! What only the PostgreSQL engine decides: the export of its log as the
//! format's files (the same bytes the store's export writes), the lease as a row and a lock, and what it refuses at
//! open (`docs/decisions/038-a-lake-in-postgresql.md`).

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::error::Situation;
use lakelands_api::location::EngineConfig;
use lakelands_api::seam::Seams;
use lakelands_api::write::Declaration;
use lakelands_api::{Error, Grants, Location, Principal, PrincipalKind, WriteContext};
use lakelands_core::conformance::engine::chain;
use lakelands_engine_pg::PgOpener;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A statement on the test server, as an operator would run it.
fn admin(url: &str, statement: &str) -> Vec<postgres::Row> {
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    client.query(statement, &[]).unwrap()
}

/// The exported directory verifies from its files alone, as a lake
/// directory does (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`).
#[tokio::test(flavor = "multi_thread")]
async fn an_exported_lake_verifies_from_its_segment_files_alone() {
    let Some(url) = common::url_or_skip("export") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "ledger", &schema);
    let seams = Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    };
    let store = lakelands_store::open(location.clone(), seams, OpenMode::Create)
        .await
        .unwrap();
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "ledger".into(),
        grants: Grants::owner(),
    };
    let cat = store.catalog(WriteContext::default());
    let entry: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "entry",
        "fields": [{"name": "note", "type": {"kind": "text"}, "searchable": true}]
    }))
    .unwrap();
    cat.declare(&owner, entry).await.unwrap();
    for (key, text) in [("a", "café \"quoted\" 🌊"), ("b", "line\u{2028}separator")] {
        let patch = serde_json::from_value(json!({ "note": text })).unwrap();
        cat.upsert(
            &owner,
            &"entry".into(),
            &key.into(),
            patch,
            Default::default(),
        )
        .await
        .unwrap();
    }
    store.checkpoint().await.unwrap();
    let patch = serde_json::from_value(json!({ "note": "after" })).unwrap();
    cat.upsert(
        &owner,
        &"entry".into(),
        &"c".into(),
        patch,
        Default::default(),
    )
    .await
    .unwrap();
    store.close().await.unwrap();
    drop((cat, store));

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("pg-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (l, d) = (location.clone(), dir.clone());
    tokio::task::spawn_blocking(move || lakelands_engine_pg::export(&l, &d))
        .await
        .unwrap()
        .unwrap();

    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("log"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 2, "one sealed segment and the active one");
    let mut prev = "0".repeat(64);
    let mut next = 0u64;
    let mut digests = Vec::new();
    for file in &files {
        let raw = std::fs::read(file).unwrap();
        let first = next;
        assert!(
            raw.ends_with(b"\n"),
            "{} ends with a line feed",
            file.display()
        );
        for line in raw[..raw.len() - 1].split(|b| *b == b'\n') {
            let event: Value = serde_json::from_slice(line).unwrap();
            assert_eq!(
                lakelands_core::canonical::to_vec(&event),
                line,
                "the stored line is canonical"
            );
            assert_eq!(event["position"], json!(next));
            assert_eq!(event["prev"], json!(prev));
            let mut body = event.clone();
            body.as_object_mut().unwrap().remove("hash");
            let hash = hex(&Sha256::digest(lakelands_core::canonical::to_vec(&body)));
            assert_eq!(event["hash"], json!(hash), "position {next}: the hash");
            if event["body"]["kind"] == "checkpoint" {
                assert_eq!(event["body"]["hash"], json!(prev));
                assert_eq!(event["body"]["through"], json!(next - 1));
                assert_eq!(event["body"]["segments"], json!(digests));
            }
            prev = hash;
            next += 1;
        }
        digests.push(json!({
            "first": first,
            "last": next - 1,
            "digest": hex(&Sha256::digest(&raw)),
        }));
    }

    // The store's export, over the feed of the same lake opened to read,
    // writes the same files byte for byte.
    let seams = Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    };
    let reader = lakelands_store::open(location.clone(), seams, OpenMode::Read)
        .await
        .unwrap();
    let via = dir.with_file_name(format!("pg-export-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&via);
    let manifest = lakelands_core::export::write(reader.as_ref(), &via)
        .await
        .unwrap();
    let names: Vec<String> = manifest.segments.iter().map(|s| s.file.clone()).collect();
    let engine_names: Vec<String> = files
        .iter()
        .map(|f| format!("log/{}", f.file_name().unwrap().to_string_lossy()))
        .collect();
    assert_eq!(names, engine_names, "the same files");
    for name in &names {
        assert_eq!(
            std::fs::read(via.join(name)).unwrap(),
            std::fs::read(dir.join(name)).unwrap(),
            "{name}: the store's export and the engine's agree byte for byte"
        );
    }
    lakelands_core::export::verify(&dir).unwrap();
    let _ = std::fs::remove_dir_all(&via);
    let _ = std::fs::remove_dir_all(&dir);
    common::drop_schema(&url, &schema);
}

#[test]
fn a_fence_raised_in_the_lease_row_refuses_the_holders_next_append_and_seal() {
    let Some(url) = common::url_or_skip("fence") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "fenced", &schema), OpenMode::Create)
        .unwrap();
    let lease = engine.take_lease("writer").unwrap();
    let events = chain(0, 2, &lakelands_api::event::Digest::zero(), &lease);
    engine.append(&lease, &events).unwrap();
    // The lock was lost without this process knowing, and another writer
    // took a greater token.
    admin(
        &url,
        &format!(
            "UPDATE \"{schema}\".lease SET fence = fence + 1, holder = 'another' WHERE one = 1"
        ),
    );
    let more = chain(2, 1, &events[1].hash, &lease);
    let refused = engine.append(&lease, &more);
    assert!(
        matches!(&refused, Err(Error::Unavailable { situation }) if situation.to_string().contains("stale")),
        "{refused:?}"
    );
    let sealed = lakelands_core::conformance::engine::seal(engine.as_ref(), &lease, &more);
    assert!(
        matches!(&sealed, Err(Error::Unavailable { situation }) if situation.to_string().contains("another")),
        "{sealed:?}"
    );
    assert_eq!(engine.tip().unwrap().map(|t| t.0.0), Some(1));
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// A writer whose session died keeps no one out: the server released its
/// lock, the next writer advances the token, and the dead holder is fenced.
#[test]
fn the_next_writer_takes_over_from_a_session_that_died() {
    let Some(url) = common::url_or_skip("takeover") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "taken", &schema);
    let first = PgOpener.open(&location, OpenMode::Create).unwrap();
    let lease = first.take_lease("first").unwrap();
    let events = chain(0, 2, &lakelands_api::event::Digest::zero(), &lease);
    first.append(&lease, &events).unwrap();

    let second = PgOpener.open(&location, OpenMode::Write).unwrap();
    let held = second.take_lease("second");
    assert!(
        matches!(held.as_ref().err().and_then(Error::situation), Some(Situation::LeaseLocked { holder: Some(h), .. }) if h == "first"),
        "while the first session lives, its lease holds: {held:?}"
    );

    // The first writer's session ends without a word: its lock goes.
    let ended = admin(
        &url,
        &format!(
            "SELECT pg_terminate_backend(pid) FROM pg_locks WHERE locktype = 'advisory' \
             AND ((classid::bigint << 32) | objid::bigint) = \
             hashtextextended('lakelands lease ' || '{schema}', 0)"
        ),
    );
    assert_eq!(ended.len(), 1, "one session held the lake's lock");
    std::thread::sleep(std::time::Duration::from_millis(200));

    let taken = second.take_lease("second").unwrap();
    assert_eq!(taken.fence, lease.fence + 1, "the token moved on");
    let more = chain(2, 1, &events[1].hash, &lease);
    assert!(
        first.append(&lease, &more).is_err(),
        "the dead holder's token is refused"
    );
    let theirs = chain(2, 1, &events[1].hash, &taken);
    second.append(&taken, &theirs).unwrap();
    assert_eq!(second.tip().unwrap().map(|t| t.0.0), Some(2));
    drop((first, second));
    common::drop_schema(&url, &schema);
}

#[test]
fn an_open_refuses_what_the_location_cannot_mean() {
    let Some(url) = common::url_or_skip("refusals") else {
        return;
    };
    let schema = common::fresh_schema();
    // TLS in the URL: configured by the location alone.
    let mut with_tls = common::location(&url, "tls", &schema);
    if let EngineConfig::Postgres { url: u, .. } = &mut with_tls.engine {
        u.push_str(if u.contains('?') {
            "&sslmode=disable"
        } else {
            "?sslmode=disable"
        });
    }
    let refused = PgOpener.open(&with_tls, OpenMode::Create).map(|_| ());
    assert!(
        matches!(&refused, Err(Error::Refused { .. })),
        "a URL carrying sslmode is refused: {refused:?}"
    );
    // No user in the URL: the driver would take the operating system's.
    let mut userless = common::location(&url, "userless", &schema);
    if let EngineConfig::Postgres { url: u, .. } = &mut userless.engine {
        *u = "postgres://localhost/lakelands".into();
    }
    let refused = PgOpener.open(&userless, OpenMode::Create).map(|_| ());
    assert!(
        matches!(&refused, Err(Error::Refused { .. })),
        "a URL naming no user is refused: {refused:?}"
    );
    // A schema an identifier cannot hold, never shortened.
    let long = Location {
        engine: match common::location(&url, "x", &"s".repeat(64)).engine {
            e @ EngineConfig::Postgres { .. } => e,
            _ => unreachable!(),
        },
        ..Location::memory("x")
    };
    assert!(matches!(
        PgOpener.open(&long, OpenMode::Create).map(|_| ()),
        Err(Error::Refused { .. })
    ));
    let with_nul = common::location(&url, "x", "lake\0name");
    assert!(
        matches!(
            PgOpener.open(&with_nul, OpenMode::Create).map(|_| ()),
            Err(Error::Refused { .. })
        ),
        "a schema holding NUL is refused"
    );
    // A schema holding something else: one schema holds one lake and
    // nothing beside it.
    let foreign = common::fresh_schema();
    admin(&url, &format!("CREATE SCHEMA \"{foreign}\""));
    admin(&url, &format!("CREATE TABLE \"{foreign}\".notes (x int)"));
    let refused = PgOpener
        .open(
            &common::location(&url, "foreign", &foreign),
            OpenMode::Create,
        )
        .map(|_| ());
    assert!(
        matches!(&refused, Err(Error::Refused { reason: lakelands_api::Refusal::Invalid { message }, .. }) if message.contains("notes")),
        "a schema holding another table is refused: {refused:?}"
    );
    common::drop_schema(&url, &foreign);
    // Nothing there: reading and writing without `create` are refused, and
    // create nothing.
    let missing = common::location(&url, "missing", &schema);
    for mode in [OpenMode::Read, OpenMode::Write] {
        assert!(matches!(
            PgOpener.open(&missing, mode).map(|_| ()),
            Err(Error::Unavailable { .. })
        ));
    }
    let schemas = admin(
        &url,
        &format!("SELECT 1 FROM pg_namespace WHERE nspname = '{schema}'"),
    );
    assert!(schemas.is_empty(), "a refused open created the schema");
    // A schema holding another lake.
    drop(PgOpener.open(&missing, OpenMode::Create).unwrap());
    let other = common::location(&url, "other", &schema);
    assert!(matches!(
        PgOpener.open(&other, OpenMode::Read).map(|_| ()),
        Err(Error::Refused { .. })
    ));
    common::drop_schema(&url, &schema);
}

#[test]
fn the_engine_answers_to_its_name() {
    let Some(url) = common::url_or_skip("name") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine: Box<dyn LakeEngine> = PgOpener
        .open(&common::location(&url, "named", &schema), OpenMode::Create)
        .unwrap();
    assert_eq!(engine.name(), "pg");
    assert_eq!(PgOpener.name(), "pg");
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// A location that names no schema puts the lake in `lakelands_<lake>`,
/// as the server itself answers.
#[test]
fn a_location_without_a_schema_puts_the_lake_in_lakelands_and_its_name() {
    let Some(url) = common::url_or_skip("the default schema") else {
        return;
    };
    // A lake name under this process's prefix, so a schema left behind by
    // a failed run is dropped with the others.
    let lake = format!("test_{}_defaults", std::process::id());
    let mut location = common::location(&url, &lake, "unused");
    if let EngineConfig::Postgres { schema, .. } = &mut location.engine {
        *schema = None;
    }
    let engine = PgOpener
        .open(&location, OpenMode::Create)
        .expect("a lake opens in its default schema");
    drop(engine);
    let expected = format!("lakelands_{lake}");
    let rows = admin(
        &url,
        &format!("SELECT nspname FROM pg_catalog.pg_namespace WHERE nspname = '{expected}'"),
    );
    assert_eq!(rows.len(), 1, "the server holds the schema `{expected}`");
    // Only this test's schema: the other tests' lakes are still open.
    admin(&url, &format!("DROP SCHEMA \"{expected}\" CASCADE"));
}

/// A compact is `VACUUM (ANALYZE)` of the projection's tables, run outside
/// a transaction and never `FULL`: a reader that holds a snapshot open on
/// the projection keeps reading and does not hold it up, the log's tables
/// are not vacuumed, and the report carries the tables' measured size
/// (`docs/decisions/040-maintenance-never-changes-an-answer.md`).
#[tokio::test(flavor = "multi_thread")]
async fn a_compact_vacuums_the_projection_beside_an_open_reader_and_reports_its_size() {
    let Some(url) = common::url_or_skip("compact") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "tidy", &schema);
    let seams = || Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    };
    let store = lakelands_store::open(location.clone(), seams(), OpenMode::Create)
        .await
        .unwrap();
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "tidy".into(),
        grants: Grants::owner(),
    };
    let cat = store.catalog(WriteContext::default());
    let note: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "note",
        "fields": [{"name": "text", "type": {"kind": "text"}, "searchable": true}]
    }))
    .unwrap();
    cat.declare(&owner, note).await.unwrap();
    for round in 0..3 {
        for key in ["a", "b", "c"] {
            let patch =
                serde_json::from_value(json!({ "text": format!("{key} {round}") })).unwrap();
            cat.upsert(
                &owner,
                &"note".into(),
                &key.into(),
                patch,
                Default::default(),
            )
            .await
            .unwrap();
        }
    }
    store.rebuild().await.unwrap();
    let head = store.head().await.unwrap();

    // A reader handle, and a session holding a snapshot of the projection.
    let reader = lakelands_store::open(location, seams(), OpenMode::Read)
        .await
        .unwrap();
    let (snapshot_url, records) = (url.clone(), format!("\"{schema}\".records"));
    let (hold, release) = std::sync::mpsc::channel::<()>();
    let (held, holding) = std::sync::mpsc::channel::<i64>();
    let holder = std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&snapshot_url, postgres::NoTls).unwrap();
        let mut t = client
            .build_transaction()
            .isolation_level(postgres::IsolationLevel::RepeatableRead)
            .start()
            .unwrap();
        let n: i64 = t
            .query_one(&format!("SELECT count(*) FROM {records}"), &[])
            .unwrap()
            .get(0);
        held.send(n).unwrap();
        // A compact that waited for this snapshot would never be done: the
        // holder gives up after a while and says so.
        let waited = release
            .recv_timeout(std::time::Duration::from_secs(20))
            .is_err();
        let again: i64 = t
            .query_one(&format!("SELECT count(*) FROM {records}"), &[])
            .unwrap()
            .get(0);
        t.commit().unwrap();
        (n, again, waited)
    });
    holding.recv().unwrap();

    let report = store.compact().await.unwrap();
    let _ = hold.send(());
    let (n, again, waited) = holder.join().unwrap();
    assert!(!waited, "the compact waited for a reader's snapshot");
    assert_eq!(
        n, again,
        "the snapshot read the same rows across the compact"
    );
    assert_eq!(report.head, head);
    let (before, after) = (report.before.unwrap(), report.after.unwrap());
    assert!(before > 0 && after > 0, "{report:?}");
    assert_eq!(reader.head().await.unwrap(), head, "the reader reads on");

    // The synchronous client runs off the test's runtime.
    let vacuumed = |table: &str| -> bool {
        let statement = format!(
            "SELECT last_vacuum IS NOT NULL AND last_analyze IS NOT NULL \
             FROM pg_catalog.pg_stat_user_tables \
             WHERE schemaname = '{schema}' AND relname = '{table}'"
        );
        tokio::task::block_in_place(|| admin(&url, &statement)[0].get(0))
    };
    for table in ["records", "applied", "outputs", "meta"] {
        assert!(vacuumed(table), "the projection's `{table}` was vacuumed");
    }
    assert!(!vacuumed("log"), "the log's table was not vacuumed");
    drop((store, reader));
    common::drop_schema(&url, &schema);
}

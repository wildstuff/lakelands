//! A lake directory verifies from its segment files alone
//! (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`).
//!
//! A store writes a lake through this engine, seals a segment with a
//! checkpoint, and closes. The check then reads nothing but `log/*.jsonl`,
//! as the log format's `verify.py` reads its vectors: every line is the
//! canonical bytes of the event it parses to, the hashes chain from 64
//! zeros, positions follow each other, and every checkpoint lists the
//! sealed segments with the digests of their files. The verifiers of an
//! export, `lakelands_core::export::verify` and `verify.py <dir>`, read
//! the directory the same way and agree.

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::ontology::{Authority, FieldDecl, FieldType, StorageKind, TypeDecl};
use lakelands_api::seam::Seams;
use lakelands_api::write::Declaration;
use lakelands_api::{Grants, Location, Principal, PrincipalKind, WriteContext};
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lake_directory_verifies_from_its_segment_files_alone() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("lake-directory-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let seams = Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::sqlite("ledger", &dir), seams, OpenMode::Create)
        .await
        .unwrap();
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "ledger".into(),
        grants: Grants::owner(),
    };
    let cat = store.catalog(WriteContext::default());
    let note = FieldDecl {
        annotations: Default::default(),
        name: "note".into(),
        ty: FieldType::kind(StorageKind::Text),
        required: false,
        authority: Authority::Authored,
        derived: None,
        function: None,
        depends_on: vec![],
        on_input_change: Default::default(),
        initial: None,
        sensitive: false,
        searchable: true,
        description: None,
        label: None,
        embed: None,
    };
    cat.declare(
        &owner,
        Declaration::Type(TypeDecl {
            annotations: Default::default(),
            name: "entry".into(),
            key: vec![],
            fields: vec![note],
            relations: vec![],
            view: None,
            valid_from: None,
            valid_to: None,
            field_meta: true,
            language: None,
            description: None,
            label: None,
        }),
    )
    .await
    .unwrap();
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

    // Both verifiers of an export read the lake directory as one: the
    // library's, and `verify.py`, which shares no code with Lakelands and
    // needs `python3` (a run without it fails saying so).
    let files_say = lakelands_core::export::verify(&dir).unwrap();
    assert_eq!(
        (files_say.through.0, files_say.hash.0.as_str()),
        (next - 1, prev.as_str())
    );
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/vectors/verify.py");
    let run = match std::process::Command::new("python3")
        .arg(&script)
        .arg(&dir)
        .output()
    {
        Ok(run) => run,
        Err(e) => panic!(
            "this test runs `python3 {}` over the lake directory and needs python3 on the \
             PATH: {e}",
            script.display()
        ),
    };
    let said = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "verify.py refused the lake directory:\n{said}"
    );
    let last = format!("verified through position {}, hash {prev}", next - 1);
    assert!(said.trim_end().ends_with(&last), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_append_cut_short_by_a_crash_is_not_there_and_the_next_writer_cuts_it_off() {
    use lakelands_api::engine::EngineOpener;
    use lakelands_core::conformance::engine::chain;
    use std::io::Write;

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("torn-append-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let location = Location::sqlite("torn", &dir);
    let engine = SqliteOpener.open(&location, OpenMode::Create).unwrap();
    let lease = engine.take_lease("first").unwrap();
    let events = chain(0, 2, &lakelands_api::event::Digest::zero(), &lease);
    engine.append(&lease, &events).unwrap();
    engine.release_lease(&lease).unwrap();
    drop(engine);

    // A crash mid-append: its segment lines and its index lines written,
    // its commit line not.
    let segment = dir.join("log").join(format!("{:020}.jsonl", 0));
    let index = dir.join("index").join(format!("{:020}.idx", 0));
    let whole = std::fs::metadata(&segment).unwrap().len();
    let indexed = std::fs::metadata(&index).unwrap().len();
    let append = |path: &PathBuf, bytes: &[u8]| {
        let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(bytes).unwrap();
    };
    append(&segment, b"{\"torn\":true}\n{\"half");
    append(&index, b"[\"e\",2,1,0,0,\"x\",\"y\",\"z\"]\n");

    let reader = SqliteOpener.open(&location, OpenMode::Read).unwrap();
    assert_eq!(
        reader.tip().unwrap().map(|t| t.0.0),
        Some(1),
        "the torn append is not there"
    );
    assert_eq!(
        reader.read(lakelands_api::id::Position(0), 10).unwrap(),
        events
    );

    let writer = SqliteOpener.open(&location, OpenMode::Write).unwrap();
    let lease = writer.take_lease("second").unwrap();
    assert_eq!(
        std::fs::metadata(&segment).unwrap().len(),
        whole,
        "the segment is cut back"
    );
    assert_eq!(
        std::fs::metadata(&index).unwrap().len(),
        indexed,
        "the index is cut back"
    );
    let more = chain(2, 1, &events[1].hash, &lease);
    writer.append(&lease, &more).unwrap();
    assert_eq!(
        writer
            .read(lakelands_api::id::Position(0), 10)
            .unwrap()
            .len(),
        3
    );
    drop((reader, writer));
    let _ = std::fs::remove_dir_all(&dir);
}

fn scratch(name: &str) -> PathBuf {
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_fence_raised_in_the_lease_file_refuses_the_holders_next_append_and_seal() {
    use lakelands_api::engine::EngineOpener;
    use lakelands_core::conformance::engine::chain;

    let dir = scratch("fence-moved");
    let engine = SqliteOpener
        .open(&Location::sqlite("fenced", &dir), OpenMode::Create)
        .unwrap();
    let lease = engine.take_lease("writer").unwrap();
    let events = chain(0, 2, &lakelands_api::event::Digest::zero(), &lease);
    engine.append(&lease, &events).unwrap();
    // The lock was lost without this process knowing, and another writer
    // took a greater token.
    std::fs::write(
        dir.join("lease"),
        format!("{{\"fence\":{},\"holder\":\"another\"}}\n", lease.fence + 1),
    )
    .unwrap();
    let more = chain(2, 1, &events[1].hash, &lease);
    let refused = engine.append(&lease, &more);
    assert!(
        matches!(&refused, Err(lakelands_api::Error::Unavailable { message }) if message.contains("another")),
        "{refused:?}"
    );
    assert!(matches!(
        lakelands_core::conformance::engine::seal(engine.as_ref(), &lease, &more),
        Err(lakelands_api::Error::Unavailable { .. })
    ));
    assert_eq!(engine.tip().unwrap().map(|t| t.0.0), Some(1));
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_segment_whose_index_was_never_created_is_recovered_by_the_next_writer() {
    use lakelands_api::engine::EngineOpener;
    use lakelands_core::conformance::engine::chain;

    let dir = scratch("missing-index");
    let location = Location::sqlite("indexless", &dir);
    drop(SqliteOpener.open(&location, OpenMode::Create).unwrap());
    // A crash between creating a new lake's first segment and its index:
    // the one segment that is created empty.
    let index = dir.join("index").join(format!("{:020}.idx", 0));
    std::fs::remove_file(&index).unwrap();

    let writer = SqliteOpener.open(&location, OpenMode::Write).unwrap();
    let lease = writer.take_lease("first").unwrap();
    assert!(index.is_file());
    let events = chain(0, 2, &lakelands_api::event::Digest::zero(), &lease);
    writer.append(&lease, &events).unwrap();
    assert_eq!(writer.tip().unwrap().map(|t| t.0.0), Some(1));
    drop(writer);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_read_only_open_leaves_the_directory_as_it_was() {
    use lakelands_api::engine::EngineOpener;

    let dir = scratch("read-only");
    let location = Location::sqlite("untouched", &dir);
    std::fs::create_dir_all(dir.join("log")).unwrap();
    std::fs::write(dir.join("lake"), "untouched\n").unwrap();
    let without = SqliteOpener.open(&location, OpenMode::Read).map(|_| ());
    assert!(
        matches!(without, Err(lakelands_api::Error::Unavailable { .. })),
        "{without:?}"
    );
    assert!(
        !dir.join("projection.db").exists(),
        "a reader created the projection"
    );
    std::fs::remove_dir_all(&dir).unwrap();

    let writer = SqliteOpener.open(&location, OpenMode::Create).unwrap();
    drop(writer);
    let listing = |dir: &PathBuf| -> Vec<(String, u64)> {
        let mut all: Vec<(String, u64)> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    e.metadata().unwrap().len(),
                )
            })
            .collect();
        all.sort();
        all
    };
    let before = listing(&dir);
    let reader = SqliteOpener.open(&location, OpenMode::Read).unwrap();
    reader.view(None).unwrap();
    drop(reader);
    assert_eq!(
        listing(&dir),
        before,
        "a read-only open changed the directory"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A handle keeps a mark per 256 events of the index, not the index, and
/// reads an event's index line from the file when asked for it
/// (lakelands#133). A committed index line changed to name another
/// position is a damaged index, refused as unavailable naming it, both to
/// a handle already open and to one that opens afterwards: never read as
/// another event.
#[test]
fn an_index_line_naming_another_position_is_a_damaged_index() {
    use lakelands_api::engine::EngineOpener;
    use lakelands_api::id::Position;
    use lakelands_core::conformance::engine::chain;

    let dir = scratch("damaged-index");
    let location = Location::sqlite("damaged", &dir);
    let engine = SqliteOpener.open(&location, OpenMode::Create).unwrap();
    let lease = engine.take_lease("writer").unwrap();
    let events = chain(0, 600, &lakelands_api::event::Digest::zero(), &lease);
    for batch in events.chunks(100) {
        engine.append(&lease, batch).unwrap();
    }
    let reader = SqliteOpener.open(&location, OpenMode::Read).unwrap();
    assert_eq!(reader.read(Position(0), 600).unwrap(), events);
    assert_eq!(reader.read(Position(300), 5).unwrap(), events[300..305]);

    // Position 300's line names 301 instead, the file's length unchanged.
    let index = dir.join("index").join(format!("{:020}.idx", 0));
    let text = std::fs::read_to_string(&index).unwrap();
    let damaged = text.replacen("[\"e\",300,", "[\"e\",301,", 1);
    assert_ne!(text, damaged, "the index holds position 300's line");
    std::fs::write(&index, damaged).unwrap();

    let refused = |what: &str, answer: lakelands_api::Result<Vec<_>>| match answer {
        Err(lakelands_api::Error::Unavailable { message }) => assert!(
            message.contains("damaged") && message.contains("300"),
            "{what}: {message}"
        ),
        other => panic!("{what}: a damaged index is refused, and it answered {other:?}"),
    };
    refused(
        "a handle already open",
        reader
            .read(Position(290), 20)
            .map(|e| e.len())
            .map(|_| vec![()]),
    );
    // A handle opened now reads the index's end at its open, not the
    // line: it opens and answers the tip, and its read of the log from the
    // first position, as `verify` reads it, is refused at the line.
    let opened = SqliteOpener.open(&location, OpenMode::Read).unwrap();
    assert_eq!(opened.tip().unwrap().map(|t| t.0.0), Some(599));
    assert_eq!(opened.read(Position(0), 300).unwrap(), events[..300]);
    refused(
        "a handle opened afterwards",
        opened.read(Position(0), 600).map(|_| vec![()]),
    );
    drop((reader, opened, engine));
    let _ = std::fs::remove_dir_all(&dir);
}

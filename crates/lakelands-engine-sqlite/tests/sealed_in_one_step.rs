//! A segment after the first comes to exist with its first events in it
//! (lakelands#623).
//!
//! A seal writes the next segment's index whole, then its lines under a
//! name nobody lists, and the rename to the segment's name is the step.
//! So a `checkpoint` whose write the disk cuts short has sealed nothing:
//! the lake directory verifies right then, by both verifiers of segment
//! files, and after the writes and checkpoints that follow the directory
//! and an export verify and the export imports. A seal that fails before
//! its rename takes its files away; what a crash there left changes no
//! read, is written over by a seal at that position and is removed by the
//! next writer's lease.
//!
//! The write that is cut short is a real one: the process's file size
//! limit, as in `failed_append.rs`. The limit is the process's, so the
//! tests here run one at a time, in a binary of their own, and they are
//! Linux's.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::error::{Part, Situation};
use lakelands_api::event::Digest;
use lakelands_api::id::Position;
use lakelands_api::seam::Seams;
use lakelands_api::{Grants, LakeStore, Location, Principal, PrincipalKind, WriteContext};
use lakelands_core::conformance::engine::{chain, seal};
use lakelands_engine_sqlite::SqliteOpener;
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
use serde_json::{Value, json};
use tokio::signal::unix::{SignalKind, signal};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// `SIGXFSZ`, which a write past the file size limit raises: taken by a
/// handler, the write answers its error; left alone, it ends the process.
const XFSZ: i32 = 25;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("one-step-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    }
}

fn owner() -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "ledger".into(),
        grants: Grants::owner(),
    }
}

async fn put(store: &dyn LakeStore, key: &str) {
    let patch = serde_json::from_value(json!({ "note": key })).unwrap();
    let cat = store.catalog(WriteContext::default());
    cat.upsert(
        &owner(),
        &"entry".into(),
        &key.into(),
        patch,
        Default::default(),
    )
    .await
    .unwrap();
}

/// The names in one of the lake directory's two directories of the log.
fn names(dir: &Path, sub: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir.join(sub))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn file(first: u64, extension: &str) -> String {
    format!("{first:020}.{extension}")
}

/// The kind of the first event of each segment file, in order, as it is
/// restored from the lines that hold it.
fn first_kinds(dir: &Path) -> Vec<String> {
    lakelands_core::export::segment_files(dir)
        .unwrap()
        .iter()
        .map(|path| {
            let raw = std::fs::read(path).unwrap();
            let mut lines = lakelands_core::log::Lines::default();
            let first = raw
                .split(|b| *b == b'\n')
                .find_map(|line| lines.read(line).unwrap())
                .unwrap();
            let event: Value = serde_json::from_slice(&first).unwrap();
            event["body"]["kind"].as_str().unwrap().to_owned()
        })
        .collect()
}

/// Both verifiers of segment files take `dir` and read it through
/// `through`: the library's, and `verify.py`, which shares no code with
/// Lakelands and needs `python3`.
fn verifies(dir: &Path, through: u64, when: &str) {
    let said = lakelands_core::export::verify(dir);
    assert_eq!(
        said.as_ref()
            .map(|v| v.through.0)
            .map_err(|e| e.to_string()),
        Ok(through),
        "{when}"
    );
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/log-format/vectors/20/verify.py");
    let run = std::process::Command::new("python3")
        .arg(&script)
        .arg(dir)
        .output()
        .expect("this test runs verify.py over the lake directory and needs python3");
    let said = String::from_utf8_lossy(&run.stdout);
    assert!(run.status.success(), "{when}: verify.py refused:\n{said}");
    let read = format!("verified through position {through},");
    assert!(said.contains(&read), "{when}: {said}");
}

/// The store verifies its lake, the directory verifies from its files, and
/// an export of it verifies and imports, all through `through`.
async fn whole(store: &Arc<dyn LakeStore>, base: &Path, through: u64, when: &str) {
    let said = store.verify(None, None).await.unwrap();
    assert_eq!(said.through.0, through, "{when}");
    verifies(&base.join("lake"), through, when);
    let out = base.join(format!("export {when}"));
    let manifest = lakelands_core::export::write(store.as_ref(), &out).await;
    assert_eq!(manifest.unwrap().through.0, through, "{when}");
    verifies(&out, through, &format!("the export {when}"));
    let to = Location::sqlite("ledger", base.join(format!("imported {when}")));
    let imported = lakelands_store::import(to, seams(), out).await;
    let imported = imported.unwrap_or_else(|e| panic!("{when}: the import: {e}"));
    assert_eq!(imported.through.0, through, "{when}");
}

/// A `checkpoint` whose write is cut short, in the next segment's index or
/// in its lines, answers its error and has sealed nothing: `log/` and
/// `index/` hold what they held and the directory verifies. The next write
/// lands in the first segment, the checkpoint sent again seals it and
/// begins the next one, and what follows verifies, exports and imports,
/// on this handle and on the next writer's.
#[test]
fn a_checkpoint_whose_write_is_cut_short_has_sealed_nothing() {
    let _alone = lakelands_core::lock(&ONE_AT_A_TIME);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(cut_short());
}

async fn cut_short() {
    let _taken = signal(SignalKind::from_raw(XFSZ)).unwrap();
    // The index of one event is shorter than 300 bytes and a checkpoint's
    // line longer: the first limit ends the index's write, the second the
    // lines'.
    for (cut, limit) in [("the index", 64), ("the lines", 300)] {
        let base = scratch("checkpoint");
        let dir = base.join("lake");
        let location = || Location::sqlite("ledger", &dir);
        let store = lakelands_store::open(location(), seams(), OpenMode::Create)
            .await
            .unwrap();
        let declared = json!({"declare": "type", "name": "entry",
            "fields": [{"name": "note", "type": {"kind": "text"}}]});
        let cat = store.catalog(WriteContext::default());
        cat.declare(&owner(), serde_json::from_value(declared).unwrap())
            .await
            .unwrap();
        put(store.as_ref(), "a").await;
        let head = store.head().await.unwrap();
        let held = (names(&dir, "log"), names(&dir, "index"));

        let was = getrlimit(Resource::Fsize);
        let (current, maximum) = (Some(limit), was.maximum);
        setrlimit(Resource::Fsize, Rlimit { current, maximum }).unwrap();
        let failed = store.checkpoint().await;
        setrlimit(Resource::Fsize, was).unwrap();
        let failed = failed.unwrap_err();
        let part = if cut == "the index" {
            Part::LogIndex
        } else {
            Part::Segment
        };
        assert!(
            matches!(failed.situation(), Some(Situation::Storage { part: p, detail, .. })
                if *p == part && detail.as_str().contains("File too large")),
            "{cut}: {failed}"
        );
        assert_eq!(
            (names(&dir, "log"), names(&dir, "index")),
            held,
            "{cut}: {failed}"
        );
        assert_eq!(store.head().await.unwrap(), head, "{cut}");
        verifies(&dir, head.0, &format!("{cut} cut short"));

        put(store.as_ref(), "b").await;
        assert_eq!(names(&dir, "log"), held.0, "{cut}: the next write");
        let sealed = store.checkpoint().await.unwrap();
        assert_eq!(sealed.0, head.0 + 2, "{cut}: the checkpoint again");
        assert_eq!(
            names(&dir, "log"),
            [file(0, "jsonl"), file(sealed.0, "jsonl")]
        );
        put(store.as_ref(), "c").await;
        whole(&store, &base, sealed.0 + 1, "after the checkpoint again").await;
        let later = store.checkpoint().await.unwrap();
        put(store.as_ref(), "d").await;
        whole(&store, &base, later.0 + 1, "after a later checkpoint").await;
        store.close().await.unwrap();
        drop((cat, store));

        // The next writer's `lease_taken` lands behind `d`.
        let next = lakelands_store::open(location(), seams(), OpenMode::Write)
            .await
            .unwrap();
        whole(&next, &base, later.0 + 2, "on the next writer").await;
        assert_eq!(
            first_kinds(&dir),
            ["lake_created", "checkpoint", "checkpoint"],
            "{cut}"
        );
        next.close().await.unwrap();
        drop(next);
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// A lake of six events whose writer holds its lease.
fn six(name: &str) -> (PathBuf, Box<dyn LakeEngine>, lakelands_api::engine::Lease) {
    let dir = scratch(name);
    let location = Location::sqlite("lake", &dir);
    let writer = SqliteOpener.open(&location, OpenMode::Create).unwrap();
    let lease = writer.take_lease("writer").unwrap();
    let events = chain(0, 6, &Digest::zero(), &lease);
    writer.append(&lease, &events).unwrap();
    (dir, writer, lease)
}

/// A seal that fails before its rename leaves the log as it was and none
/// of its files: here the next segment's index cannot be created, because
/// a directory stands where it belongs. The next append lands in the
/// segment that was active, and once the index can be written a seal
/// closes all of it as one.
#[test]
fn a_seal_that_fails_before_its_rename_leaves_nothing() {
    let _alone = lakelands_core::lock(&ONE_AT_A_TIME);
    let (dir, writer, lease) = six("refused");
    let events = writer.read(Position(0), 10).unwrap();
    let in_the_way = dir.join("index").join(file(6, "idx"));
    std::fs::create_dir(&in_the_way).unwrap();
    let next = chain(6, 1, &events[5].hash, &lease);
    let failed = seal(writer.as_ref(), &lease, &next).unwrap_err();
    assert!(failed.to_string().contains("Is a directory"), "{failed}");
    assert_eq!(names(&dir, "log"), [file(0, "jsonl")], "{failed}");
    assert_eq!(writer.segments().unwrap(), vec![]);
    std::fs::remove_dir(&in_the_way).unwrap();

    writer.append(&lease, &next).unwrap();
    let last = chain(7, 1, &next[0].hash, &lease);
    let sealed = seal(writer.as_ref(), &lease, &last).unwrap().unwrap();
    assert_eq!((sealed.first.0, sealed.last.0), (0, 6));
    assert_eq!(names(&dir, "log"), [file(0, "jsonl"), file(7, "jsonl")]);
    let reader = SqliteOpener
        .open(&Location::sqlite("lake", &dir), OpenMode::Read)
        .unwrap();
    let all = [events, next, last].concat();
    assert_eq!(reader.read(Position(0), 10).unwrap(), all);
    let _ = std::fs::remove_dir_all(&dir);
}

/// What a crash between a seal's writes and its rename leaves, the next
/// segment's index and its lines under their other name, is read by
/// nobody: every handle reads the log as it was, and a read-only open
/// changes nothing. A seal at that position writes over both, and the
/// segment holds the seal's events alone. Where no seal comes at that
/// position, the next writer removes them as it takes the lease.
#[test]
fn what_a_crash_before_the_rename_left_is_read_by_nobody() {
    let _alone = lakelands_core::lock(&ONE_AT_A_TIME);
    for then in ["a seal at that position", "the next writer's lease"] {
        let (dir, writer, lease) = six("crashed");
        let events = writer.read(Position(0), 10).unwrap();
        let (index, partial) = (
            dir.join("index").join(file(6, "idx")),
            dir.join("log").join(file(6, "jsonl.partial")),
        );
        let left = "[\"e\",6,1,0,9,\"x\",\"y\",\"z\"]\n[\"c\"]\n";
        std::fs::write(&index, left).unwrap();
        std::fs::write(&partial, "{\"left\":6}\n").unwrap();
        let location = Location::sqlite("lake", &dir);
        let reader = SqliteOpener.open(&location, OpenMode::Read).unwrap();
        for handle in [&writer, &reader] {
            assert_eq!(handle.read(Position(0), 10).unwrap(), events, "{then}");
            assert_eq!(handle.tip().unwrap().unwrap().0, Position(5), "{then}");
        }
        assert!(
            index.is_file() && partial.is_file(),
            "{then}: a read removed"
        );
        let next = chain(6, 2, &events[5].hash, &lease);
        if then == "a seal at that position" {
            seal(writer.as_ref(), &lease, &next).unwrap().unwrap();
            let lines: Vec<u8> = (next.iter())
                .flat_map(|e| e.bytes.iter().copied().chain(*b"\n"))
                .collect();
            let segment = dir.join("log").join(file(6, "jsonl"));
            assert_eq!(std::fs::read(segment).unwrap(), lines);
            assert_eq!(names(&dir, "log"), [file(0, "jsonl"), file(6, "jsonl")]);
        } else {
            writer.append(&lease, &next).unwrap();
            writer.release_lease(&lease).unwrap();
            let later = SqliteOpener.open(&location, OpenMode::Write).unwrap();
            later.take_lease("later").unwrap();
            assert_eq!(names(&dir, "log"), [file(0, "jsonl")]);
            assert_eq!(names(&dir, "index"), [file(0, "idx")]);
        }
        let all = [events, next].concat();
        assert_eq!(reader.read(Position(0), 10).unwrap(), all, "{then}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// An error after the rename says that the seal stands. Here the handle
/// cannot read on: while the seal runs, the index of the segment it
/// closes gains a line that is no index line, and a commit line after it,
/// which make it damage, not a crash's tear. The seal answers that the
/// segment is sealed and the next one in the log's files, and it is: the
/// directory lists it, with its event's line. Once the index reads again
/// the handle reads the sealed segment and the event that begins the next.
#[test]
fn an_error_after_the_rename_says_that_the_seal_stands() {
    let _alone = lakelands_core::lock(&ONE_AT_A_TIME);
    let (dir, writer, lease) = six("stands");
    let events = writer.read(Position(0), 10).unwrap();
    let index = dir.join("index").join(file(0, "idx"));
    let whole = std::fs::read(&index).unwrap();
    let next = chain(6, 1, &events[5].hash, &lease);
    let failed = writer.seal_segment(&lease, false, &mut |_| {
        let damage: &[u8] = b"no index line\n[\"c\"]\n";
        std::fs::write(&index, [&whole[..], damage].concat()).unwrap();
        Ok(next.clone())
    });
    let failed = failed.unwrap_err().to_string();
    assert!(
        failed.contains("the segment through position 5 is sealed") && failed.contains("damaged"),
        "{failed}"
    );
    assert_eq!(names(&dir, "log"), [file(0, "jsonl"), file(6, "jsonl")]);
    let segment = std::fs::read(dir.join("log").join(file(6, "jsonl"))).unwrap();
    assert_eq!(segment, [&next[0].bytes[..], b"\n"].concat());

    std::fs::write(&index, &whole).unwrap();
    assert_eq!(writer.tip().unwrap().unwrap().0, Position(6), "{failed}");
    let sealed = writer.segments().unwrap();
    assert_eq!((sealed.len(), sealed[0].last.0), (1, 5), "{failed}");
    let all = [events, next].concat();
    assert_eq!(writer.read(Position(0), 10).unwrap(), all, "{failed}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A leftover the next writer cannot remove does not refuse its lease: it
/// is read by nobody and is passed over. Here a directory stands under
/// each of the two names a crashed seal leaves, and a directory is not
/// removed as a file is, for any user. The lease is taken, both stay, and
/// the writer appends.
#[test]
fn a_leftover_that_cannot_be_removed_refuses_no_lease() {
    let _alone = lakelands_core::lock(&ONE_AT_A_TIME);
    let (dir, writer, lease) = six("kept");
    let events = writer.read(Position(0), 10).unwrap();
    writer.release_lease(&lease).unwrap();
    drop(writer);
    let left = [
        dir.join("index").join(file(6, "idx")),
        dir.join("log").join(file(6, "jsonl.partial")),
    ];
    left.iter().for_each(|l| std::fs::create_dir(l).unwrap());
    let location = Location::sqlite("lake", &dir);
    let later = SqliteOpener.open(&location, OpenMode::Write).unwrap();
    let lease = later.take_lease("later").unwrap();
    assert!(left.iter().all(|l| l.is_dir()), "a directory was removed");
    let next = chain(6, 1, &events[5].hash, &lease);
    later.append(&lease, &next).unwrap();
    assert_eq!(
        later.read(Position(0), 10).unwrap(),
        [events, next].concat()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

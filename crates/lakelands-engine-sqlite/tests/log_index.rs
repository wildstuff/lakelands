//! What a handle reads of the log's index, and what a writer leaves of
//! the projection's journal (lakelands#564).
//!
//! An open reads the end of each segment's index: the event line before
//! the last commit line says what the segment has committed, whatever a
//! crash left after it. A read finds the line it starts at by halving the
//! index and goes on where the last one ended. A damaged line is found by
//! the read that meets it, and so by `verify`, not by the open but in the
//! active segment's last group, which an open checks. A writer
//! that gives its lease up leaves the journal empty. The engine seam's
//! checks hold what every engine answers; these hold what this engine's
//! files make of it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lakelands_api::engine::{CanonicalEvent, EngineOpener, LakeEngine, OpenMode};
use lakelands_api::event::Digest;
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::ontology::StorageKind::Text;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::Seams;
use lakelands_api::{Error, Location};
use lakelands_core::conformance::engine::{chain, seal};
use lakelands_core::conformance::fixture::{Lake, source_record, sourced, type_decl};
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

/// What an open reads of an index's end at once.
const BLOCK: usize = 64 * 1024;

fn scratch(name: &str) -> PathBuf {
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn open(dir: &Path, mode: OpenMode) -> Box<dyn LakeEngine> {
    SqliteOpener
        .open(&Location::sqlite("lake", dir), mode)
        .unwrap()
}

fn index(dir: &Path, first: u64) -> PathBuf {
    dir.join("index").join(format!("{first:020}.idx"))
}

fn add(path: &Path, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
}

/// Index lines of events that were never committed, from position `from`.
fn uncommitted(from: u64, n: u64) -> String {
    (from..from + n)
        .map(|p| format!("[\"e\",{p},1,0,0,\"x\",\"y\",\"z\"]\n"))
        .collect()
}

/// A lake of `n` chained events, appended `group` at a time under one
/// lease, which is given up.
fn lake_of(dir: &Path, n: u64, group: usize) -> Vec<CanonicalEvent> {
    let engine = open(dir, OpenMode::Create);
    let lease = engine.take_lease("writer").unwrap();
    let events = chain(0, n, &Digest::zero(), &lease);
    for batch in events.chunks(group) {
        engine.append(&lease, batch).unwrap();
    }
    engine.release_lease(&lease).unwrap();
    events
}

fn tip(engine: &dyn LakeEngine) -> Option<u64> {
    engine.tip().unwrap().map(|t| t.0.0)
}

#[test]
fn an_open_answers_the_last_committed_event_whatever_a_crash_left_after_it() {
    for (what, committed, left) in [
        ("event lines without a commit line", 6, uncommitted(6, 3)),
        ("half a line", 6, "[\"e\",6,1,0".to_owned()),
        (
            "a line and half the next",
            6,
            uncommitted(6, 1) + "[\"e\",7,",
        ),
        (
            "lines in a segment that committed none",
            0,
            uncommitted(0, 2),
        ),
    ] {
        let dir = scratch("crash-left");
        let events = lake_of(&dir, committed, 2);
        let (segment, index) = (
            dir.join("log").join(format!("{:020}.jsonl", 0)),
            index(&dir, 0),
        );
        let whole = std::fs::metadata(&index).unwrap().len();
        add(&segment, b"{\"torn\":true}\n{\"half");
        add(&index, left.as_bytes());

        let reader = open(&dir, OpenMode::Read);
        assert_eq!(tip(reader.as_ref()), committed.checked_sub(1), "{what}");
        assert_eq!(reader.read(Position(0), 100).unwrap(), events, "{what}");

        let writer = open(&dir, OpenMode::Write);
        let lease = writer.take_lease("next").unwrap();
        assert_eq!(
            std::fs::metadata(&index).unwrap().len(),
            whole,
            "{what}: the next writer cuts the index back to its last commit line"
        );
        let prev = events.last().map_or_else(Digest::zero, |e| e.hash.clone());
        let more = chain(committed, 2, &prev, &lease);
        writer.append(&lease, &more).unwrap();
        let all: Vec<_> = events.iter().chain(&more).cloned().collect();
        assert_eq!(writer.read(Position(0), 100).unwrap(), all, "{what}");
        assert_eq!(reader.read(Position(0), 100).unwrap(), all, "{what}");
        drop((reader, writer));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn an_open_reads_back_past_every_block_that_holds_no_commit_line() {
    let dir = scratch("long-tail");
    let events = lake_of(&dir, 5, 2);
    let index = index(&dir, 0);
    let committed = std::fs::read(&index).unwrap();
    // An unfinished line that puts the last commit line across the border
    // of the last block at each of its bytes, where the open passes it by
    // and reads on from the one before, and before that block; then whole
    // lines of more than two blocks.
    let mut tails: Vec<String> = (0..=8)
        .map(|k| format!("[\"e\",{}", "1".repeat(BLOCK - 5 - k)))
        .collect();
    tails.push(uncommitted(5, 6000));
    assert!(tails[9].len() > 2 * BLOCK);
    for (k, tail) in tails.iter().enumerate() {
        std::fs::write(&index, &committed).unwrap();
        add(&index, tail.as_bytes());
        let reader = open(&dir, OpenMode::Read);
        assert_eq!(tip(reader.as_ref()), Some(4), "tail {k}");
        assert_eq!(reader.read(Position(0), 100).unwrap(), events, "tail {k}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_read_from_any_position_answers_the_events_from_it() {
    const N: u64 = 3000;
    let dir = scratch("any-position");
    let engine = open(&dir, OpenMode::Create);
    let lease = engine.take_lease("writer").unwrap();
    let events = chain(0, N, &Digest::zero(), &lease);
    let mut rest = &events[..];
    for group in [1, 7, 32, 100].into_iter().cycle() {
        let (batch, after) = rest.split_at(group.min(rest.len()));
        engine.append(&lease, batch).unwrap();
        rest = after;
        if rest.is_empty() {
            break;
        }
    }
    assert!(std::fs::metadata(index(&dir, 0)).unwrap().len() > 8 * BLOCK as u64);
    let from = |p: u64, n: u64| &events[p as usize..(p + n).min(N) as usize];

    // Every position, and the one past the last, in an order in which no
    // read goes on where the one before ended.
    let reader = open(&dir, OpenMode::Read);
    for i in 0..=N {
        let p = i * 1237 % (N + 1);
        assert_eq!(reader.read(Position(p), 3).unwrap(), from(p, 3), "from {p}");
    }
    // Pages that follow each other, from the middle and from the first.
    let reader = open(&dir, OpenMode::Read);
    for p in [1000, 1064, 1128, 0, 256, 512] {
        assert_eq!(reader.read(Position(p), 64).unwrap(), from(p, 64));
    }
    let mut all = Vec::new();
    while let page = reader.read(Position(all.len() as u64), 256).unwrap()
        && !page.is_empty()
    {
        all.extend(page);
    }
    assert_eq!(all, events);
    // The writer's own handle, which appended every line it now reads.
    assert_eq!(engine.read(Position(1999), 3).unwrap(), from(1999, 3));
    drop((reader, engine));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_read_goes_on_past_an_append_and_across_a_seal() {
    let dir = scratch("across-a-seal");
    let writer = open(&dir, OpenMode::Create);
    let lease = writer.take_lease("writer").unwrap();
    let events = chain(0, 1400, &Digest::zero(), &lease);
    let append = |span: std::ops::Range<usize>| {
        for batch in events[span].chunks(50) {
            writer.append(&lease, batch).unwrap();
        }
    };
    append(0..700);
    let reader = open(&dir, OpenMode::Read);
    let mut read = 0;
    while read < 700 {
        let page = reader.read(Position(read as u64), 256).unwrap();
        assert_eq!(page, events[read..(read + 256).min(700)]);
        read += page.len();
    }
    // The reader is at the tip; the segment grows, is sealed, and the
    // next one grows.
    append(700..800);
    assert_eq!(reader.read(Position(700), 256).unwrap(), events[700..800]);
    let sealed = seal(writer.as_ref(), &lease, &events[800..801]);
    let sealed = sealed.unwrap().unwrap();
    assert_eq!((sealed.first.0, sealed.last.0), (0, 799));
    append(801..1100);
    assert_eq!(reader.read(Position(800), 256).unwrap(), events[800..1056]);
    assert_eq!(reader.read(Position(790), 20).unwrap(), events[790..810]);
    assert_eq!(
        reader.read(Position(1056), 256).unwrap(),
        events[1056..1100]
    );
    // The reader has read the second segment as the last one. It grows
    // again and is sealed before the reader looks: the lines it gained are
    // read all the same, and only then is its index passed by.
    append(1100..1200);
    let second = seal(writer.as_ref(), &lease, &events[1200..1201]);
    let second = second.unwrap().unwrap();
    append(1201..1400);
    for _ in 0..2 {
        assert_eq!(
            reader.read(Position(1090), 200).unwrap(),
            events[1090..1290]
        );
        assert_eq!(reader.read(Position(1290), 200).unwrap(), events[1290..]);
        assert_eq!(tip(reader.as_ref()), Some(1399));
    }
    assert_eq!(
        reader.segments().unwrap(),
        vec![sealed.clone(), second.clone()]
    );

    let fresh = open(&dir, OpenMode::Read);
    assert_eq!(tip(fresh.as_ref()), Some(1399));
    for (p, n) in [(799, 2), (800, 1), (0, 5), (1399, 5), (950, 3), (400, 900)] {
        let end = (p + n).min(1400);
        assert_eq!(
            fresh.read(Position(p as u64), n as u32).unwrap(),
            events[p..end],
            "from {p}"
        );
    }
    assert_eq!(fresh.segments().unwrap(), vec![sealed, second]);
    drop((reader, fresh, writer));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_damaged_line_is_found_by_the_read_that_meets_it_and_by_verify_not_by_the_open() {
    let dir = scratch("damaged-line");
    let seams = || Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    };
    let location = Location::sqlite("lake", &dir);
    let store = lakelands_store::open(location.clone(), seams(), OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("note", vec![sourced("text", Text)]))
        .await
        .unwrap();
    for i in 0..40 {
        let record = source_record("note", &format!("n{i}"), json!({"text": "a note"}));
        l.ingest("load", vec![record]).await.unwrap();
    }
    let whole = store.verify(None, None).await.unwrap();
    store.close().await.unwrap();
    drop((l, store));

    // Position 20's line names 21 instead, the file's length unchanged.
    let index = index(&dir, 0);
    let text = std::fs::read_to_string(&index).unwrap();
    let damaged = text.replacen("[\"e\",20,", "[\"e\",21,", 1);
    assert_ne!(text, damaged, "the index holds position 20's line");
    std::fs::write(&index, damaged).unwrap();

    let refused = |what: &str, answer: Result<(), Error>| match answer {
        Err(Error::Unavailable { situation }) => {
            let message = situation.to_string();
            assert!(message.contains("damaged"), "{what}: {message}")
        }
        other => panic!("{what}: a damaged index is refused, and it answered {other:?}"),
    };
    let engine = open(&dir, OpenMode::Read);
    assert_eq!(
        tip(engine.as_ref()),
        Some(whole.through.0),
        "the open reads the index's end and answers the tip"
    );
    assert_eq!(engine.read(Position(0), 20).unwrap().len(), 20);
    refused(
        "a read across the line",
        engine.read(Position(15), 10).map(|_| ()),
    );
    let store = lakelands_store::open(location, seams(), OpenMode::Read)
        .await
        .unwrap();
    refused("verify", store.verify(None, None).await.map(|_| ()));
    drop((engine, store));
    let _ = std::fs::remove_dir_all(&dir);
}

/// What this thread has read from files so far, as the kernel counts it:
/// `rchar` the bytes, `syscr` the read calls.
#[cfg(target_os = "linux")]
fn so_far(counter: &str) -> u64 {
    let io = std::fs::read_to_string("/proc/thread-self/io").unwrap();
    let count = io
        .lines()
        .find_map(|line| line.strip_prefix(counter)?.strip_prefix(": "));
    count.unwrap().parse().unwrap()
}

/// What `f` answers, and what this thread read from files for it.
#[cfg(target_os = "linux")]
fn counted<R>(counter: &str, f: impl FnOnce() -> R) -> (R, u64) {
    let before = so_far(counter);
    let answer = f();
    (answer, so_far(counter) - before)
}

/// As `counted`, in bytes.
#[cfg(target_os = "linux")]
fn cost<R>(f: impl FnOnce() -> R) -> (R, u64) {
    counted("rchar", f)
}

/// A handle looks for new lines in the index of the last segment alone: a
/// segment it has read while a later one existed is sealed and its index
/// does not change. Counted as read calls, of which looking into one
/// index is one: the answers would hold for a handle that opened every
/// segment's index at every read.
#[cfg(target_os = "linux")]
#[test]
fn a_handle_passes_by_the_index_of_a_segment_it_has_read_sealed() {
    const SEGMENTS: usize = 120;
    let dir = scratch("many-segments");
    let writer = open(&dir, OpenMode::Create);
    let lease = writer.take_lease("writer").unwrap();
    let events = chain(0, 2 * SEGMENTS as u64, &Digest::zero(), &lease);
    writer.append(&lease, &events[..2]).unwrap();
    for batch in events[2..].chunks(2) {
        seal(writer.as_ref(), &lease, batch).unwrap().unwrap();
    }
    let reader = open(&dir, OpenMode::Read);
    let (last, asked) = counted("syscr", || tip(reader.as_ref()));
    assert_eq!(last, Some(2 * SEGMENTS as u64 - 1));
    assert!(
        asked < 10,
        "the tip made {asked} read calls beside {SEGMENTS} sealed segments"
    );
    // A page out of two sealed segments in the middle.
    let (page, read) = counted("syscr", || reader.read(Position(101), 3).unwrap());
    assert_eq!(page, events[101..104]);
    assert!(
        read < 30,
        "a page of three made {read} read calls beside {SEGMENTS} sealed segments"
    );
    let (page, wrote) = counted("syscr", || writer.read(Position(101), 3).unwrap());
    assert_eq!(page, events[101..104]);
    assert!(
        wrote < 30,
        "the writer's page made {wrote} read calls beside the segments it sealed"
    );
    println!(
        "beside {SEGMENTS} sealed segments: the tip made {asked} read calls, a page of three {read}, the writer's {wrote}"
    );
    drop((reader, writer));
    let _ = std::fs::remove_dir_all(&dir);
}

/// What an open and a read cost, counted as the bytes read from files:
/// the answers above would hold for a handle that read the whole index at
/// its open, or every line from the first up to the one a read starts at.
#[cfg(target_os = "linux")]
#[test]
fn an_open_and_a_read_read_blocks_of_the_index_not_the_index() {
    let dir = scratch("cost");
    let events = lake_of(&dir, 12_000, 32);
    let block = BLOCK as u64;
    let whole = std::fs::metadata(index(&dir, 0)).unwrap().len();
    assert!(whole > 32 * block, "the index is {whole} bytes");

    let (reader, opened) = cost(|| open(&dir, OpenMode::Read));
    assert!(
        opened < 4 * block,
        "the open read {opened} bytes beside an index of {whole}"
    );
    let (last, asked) = cost(|| tip(reader.as_ref()));
    assert_eq!(last, Some(11_999));
    assert!(asked < 1024, "the tip, asked again, read {asked} bytes");
    let (page, middle) = cost(|| reader.read(Position(6000), 3).unwrap());
    assert_eq!(page, events[6000..6003]);
    assert!(
        middle < 2 * block,
        "a read from the middle read {middle} bytes beside an index of {whole}"
    );
    let (page, follows) = cost(|| reader.read(Position(6003), 3).unwrap());
    assert_eq!(page, events[6003..6006]);
    assert!(
        follows < block / 4,
        "the read that follows read {follows} bytes"
    );
    // Three blocks a crash left without a commit line: the open reads
    // back through them to the last commit line and forward through them
    // again, not the committed index before them.
    add(&index(&dir, 0), "1".repeat(3 * BLOCK + 100).as_bytes());
    let (after, crashed) = cost(|| open(&dir, OpenMode::Read));
    assert_eq!(tip(after.as_ref()), Some(11_999));
    assert!(
        crashed < 12 * block,
        "the open after a crash read {crashed} bytes beside an index of {whole}"
    );
    println!(
        "of an index of {whole} bytes: the open read {opened}, the tip asked again {asked}, \
         a read from the middle {middle}, the read that follows {follows}, the open after a \
         crash {crashed}"
    );
    drop((reader, after));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A handle passes a segment by only after a refresh that read it to its
/// end, with a later one listed, and did not fail: one that fails after
/// the later segment was listed leaves the earlier one to be read again.
#[test]
fn a_refresh_that_fails_once_leaves_no_segment_passed_by_unread() {
    let dir = scratch("failed-refresh");
    let writer = open(&dir, OpenMode::Create);
    let lease = writer.take_lease("writer").unwrap();
    let events = chain(0, 13, &Digest::zero(), &lease);
    writer.append(&lease, &events[..10]).unwrap();
    let reader = open(&dir, OpenMode::Read);
    assert_eq!(reader.read(Position(0), 100).unwrap(), events[..10]);
    // The first segment gains two events and is sealed, and the next one
    // takes an event, before the reader looks again.
    writer.append(&lease, &events[10..12]).unwrap();
    let sealed = seal(writer.as_ref(), &lease, &events[12..]);
    let sealed = sealed.unwrap().unwrap();
    // The reader's next look lists the new segment and cannot read the
    // first one's index: a directory stands in its place.
    let (index, aside) = (index(&dir, 0), dir.join("aside.idx"));
    std::fs::rename(&index, &aside).unwrap();
    std::fs::create_dir(&index).unwrap();
    assert!(reader.read(Position(10), 100).is_err());
    std::fs::remove_dir(&index).unwrap();
    std::fs::rename(&aside, &index).unwrap();
    for _ in 0..2 {
        assert_eq!(reader.read(Position(10), 100).unwrap(), events[10..]);
        assert_eq!(tip(reader.as_ref()), Some(12));
        assert_eq!(reader.segments().unwrap(), vec![sealed.clone()]);
    }
    drop((reader, writer));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A line that names another position is refused when it alone is read:
/// the line a read starts at is reached from a line before it, checked to
/// be the position that follows, wherever the halving lands. Every line
/// in turn names the position after its own, and that position is read by
/// a fresh handle. A line of the last group, or the one before it, is
/// found by the open, which reads that group and checks it.
#[test]
fn a_line_that_names_another_position_is_refused_when_it_alone_is_read() {
    let dir = scratch("misnamed-line");
    let events = lake_of(&dir, 600, 50);
    let index = index(&dir, 0);
    let text = std::fs::read_to_string(&index).unwrap();
    // Reads that answered the misnamed line's event as the one asked for,
    // and reads that answered the one asked for without meeting the line.
    let (mut wrong, mut unmet) = (Vec::new(), Vec::new());
    for q in 0..599u64 {
        let damaged = text.replacen(&format!("[\"e\",{q},"), &format!("[\"e\",{},", q + 1), 1);
        assert_ne!(text, damaged, "the index holds position {q}'s line");
        std::fs::write(&index, damaged).unwrap();
        let reader = match SqliteOpener.open(&Location::sqlite("lake", &dir), OpenMode::Read) {
            Ok(reader) => reader,
            Err(e) if q >= 549 && e.to_string().contains("damaged") => continue,
            Err(e) => panic!("{q}: the open refused {e}"),
        };
        match reader.read(Position(q + 1), 1) {
            Err(Error::Unavailable { situation }) => {
                let message = situation.to_string();
                assert!(message.contains("damaged"), "{q}: {message}")
            }
            Ok(page) if page[0].bytes == events[q as usize].bytes => wrong.push(q + 1),
            Ok(_) => unmet.push(q + 1),
            Err(other) => panic!("{q}: {other:?}"),
        }
    }
    assert!(
        wrong.is_empty() && unmet.is_empty(),
        "a read of a position whose line follows one that names it is refused; answered with \
         the event before: {wrong:?}; answered without meeting the line: {unmet:?}"
    );
    std::fs::write(&index, text).unwrap();
    let reader = open(&dir, OpenMode::Read);
    assert_eq!(reader.read(Position(0), 600).unwrap(), events);
    let _ = std::fs::remove_dir_all(&dir);
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().map(|e| e.unwrap()) {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// `tests/fixtures/a-lake-as-written` is a lake directory as the build
/// before lakelands#564 wrote it, which read every index line at its
/// open: twelve events in a sealed segment, six in the active one, in
/// groups of four and of three; its projection left out, a writer makes
/// it. Its files are read as that build read them.
#[test]
fn an_index_written_before_the_open_read_its_end_is_read_the_same() {
    let dir = scratch("as-written");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/a-lake-as-written");
    copy(&fixture, &dir);
    let location = Location::sqlite("kept", &dir);
    let engine = SqliteOpener.open(&location, OpenMode::Write).unwrap();
    let lines: Vec<Vec<u8>> = [0, 12]
        .iter()
        .flat_map(|first| {
            let path = fixture.join("log").join(format!("{first:020}.jsonl"));
            let raw = std::fs::read(path).unwrap();
            raw.split(|b| *b == b'\n')
                .filter(|line| !line.is_empty())
                .map(<[u8]>::to_vec)
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(lines.len(), 18);
    let read = engine.read(Position(0), 100).unwrap();
    assert_eq!(
        read.iter().map(|e| e.bytes.clone()).collect::<Vec<_>>(),
        lines
    );
    for (p, event) in read.iter().enumerate() {
        let stored: serde_json::Value = serde_json::from_slice(&event.bytes).unwrap();
        assert_eq!(event.position.0, p as u64);
        assert_eq!(json!(event.hash.0), stored["hash"]);
        assert_eq!(json!(event.prev.0), stored["prev"]);
        assert_eq!(json!(event.fence), stored["fence"]);
    }
    assert_eq!(engine.read(Position(11), 2).unwrap(), read[11..13]);
    assert_eq!(
        engine.tip().unwrap(),
        Some((Position(17), read[17].hash.clone()))
    );
    let sealed = engine.segments().unwrap();
    assert_eq!(sealed.len(), 1);
    assert_eq!((sealed[0].first.0, sealed[0].last.0), (0, 11));

    let lease = engine.take_lease("next").unwrap();
    let more = chain(18, 3, &read[17].hash, &lease);
    engine.append(&lease, &more).unwrap();
    assert_eq!(engine.read(Position(16), 100).unwrap()[2..], more);
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_writer_that_gives_its_lease_up_leaves_the_journal_empty() {
    let dir = scratch("journal");
    let engine = open(&dir, OpenMode::Create);
    let (db, journal) = (dir.join("projection.db"), dir.join("projection.db-wal"));
    let len = || std::fs::metadata(&journal).map_or(0, |m| m.len());
    let fold = |at: u64| {
        let p = engine.projection();
        p.begin().unwrap();
        p.put_record(&StoredRecord {
            ty: TypeName::from("note"),
            key: RecordKey::from(format!("n{at}")),
            version: Version(1),
            fields: serde_json::from_value(json!({"text": "x".repeat(4000)})).unwrap(),
            meta: Default::default(),
            position: Position(at),
            made: Position(at),
            retired: false,
            restricted: false,
        })
        .unwrap();
        p.commit(Position(at)).unwrap();
    };
    let lease = engine.take_lease("writer").unwrap();
    fold(1);
    assert!(len() > 0, "the fold wrote to the journal");
    engine.release_lease(&lease).unwrap();
    assert_eq!(len(), 0, "the release truncated it");
    assert_eq!(
        engine.projection().folded_through().unwrap(),
        Some(Position(1))
    );

    // A reader in another connection holds a snapshot: the release does
    // not wait for it, succeeds, and leaves the journal for the next one.
    let lease = engine.take_lease("writer").unwrap();
    fold(2);
    let reader =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let _: i64 = reader
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
        .unwrap();
    let asked = std::time::Instant::now();
    engine.release_lease(&lease).unwrap();
    assert!(asked.elapsed().as_secs() < 5, "the release did not wait");
    assert!(len() > 0, "the journal a reader holds is left");
    reader.execute_batch("COMMIT").unwrap();
    let lease = engine.take_lease("writer").unwrap();
    engine.release_lease(&lease).unwrap();
    assert_eq!(len(), 0, "the next release truncates it");
    assert_eq!(
        engine.projection().folded_through().unwrap(),
        Some(Position(2))
    );
    drop((reader, engine));
    let _ = std::fs::remove_dir_all(&dir);
}

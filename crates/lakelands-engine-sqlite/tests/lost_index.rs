//! Part of a lake without its events is damage, said before anything is
//! cut, made or replaced (lakelands#865,
//! `docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
//!
//! The order of this engine's writes tells a crash from a loss. A crash
//! leaves an index shorter than its segment, or a first segment of no
//! bytes with no index yet; both are read as before. No crash leaves a
//! segment with lines and no index file, a later segment with none, or
//! a projection with no segment file: each is a lake that lost its log's
//! index or its log. Such a directory was read as holding no log, so an
//! import replaced its segment and an open to create cut it. Here every
//! door answers `damaged` and leaves each file and each directory as it
//! was. Each door is knocked on by itself, the engine's own open as well
//! as the store's, and what a door changed is put back before the next.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::error::{Case, Flaw, Part, Situation};
use lakelands_api::seam::Seams;
use lakelands_api::{Error, LakeStore, Location, Position};
use lakelands_engine_sqlite::SqliteOpener;

const MODES: [OpenMode; 4] = [
    OpenMode::Read,
    OpenMode::Write,
    OpenMode::Create,
    OpenMode::Rebuild,
];

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    }
}

/// A directory of the test's own, with nothing in it.
fn fresh(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("lost-index-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// What a directory holds: every file under it with its bytes, and
/// every directory.
type Held = BTreeMap<PathBuf, Option<Vec<u8>>>;

/// What `dir` holds.
fn held(dir: &Path) -> Held {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(held(&path));
            out.insert(path, None);
        } else {
            out.insert(path.clone(), Some(std::fs::read(&path).unwrap()));
        }
    }
    out
}

/// What `dir` holds but the projection's shared memory, which a reader's
/// open of a healthy lake writes too: the log, the lease, the name, and
/// the projection with its write-ahead log.
fn log_held(dir: &Path) -> Held {
    let mut all = held(dir);
    all.retain(|path, _| !path.ends_with("projection.db-shm"));
    all
}

/// What `dir` holds but the projection's write-ahead log and shared
/// memory, which a reader's connect makes where its maker wrote none yet.
fn beside_wal(dir: &Path) -> Held {
    let mut all = log_held(dir);
    all.retain(|path, _| !path.ends_with("projection.db-wal"));
    all
}

/// Make `dir` hold `what` and nothing else.
fn put_back(dir: &Path, what: &Held) {
    std::fs::remove_dir_all(dir).unwrap();
    std::fs::create_dir_all(dir).unwrap();
    for (path, bytes) in what {
        match bytes {
            None => std::fs::create_dir_all(path).unwrap(),
            Some(bytes) => {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, bytes).unwrap();
            }
        }
    }
}

fn segment(dir: &Path, first: u64) -> PathBuf {
    dir.join("log").join(format!("{first:020}.jsonl"))
}

fn index(dir: &Path, first: u64) -> PathBuf {
    dir.join("index").join(format!("{first:020}.idx"))
}

/// A lake a store of this release made and closed, sealed `seals`
/// times, with an export of it beside its directory: where each segment
/// after the first begins, and the position the projection is folded
/// through.
async fn made(dir: &Path, seals: usize) -> (Location, PathBuf, Vec<u64>, Position) {
    let location = Location::sqlite("kept", dir);
    let store = lakelands_store::open(location.clone(), seams(), OpenMode::Create);
    let store = store.await.unwrap();
    let mut firsts = Vec::new();
    for _ in 0..seals {
        firsts.push(store.checkpoint().await.unwrap().0);
    }
    let export = dir.with_extension("export");
    let _ = std::fs::remove_dir_all(&export);
    let written = lakelands_core::export::write(store.as_ref(), &export);
    written.await.unwrap();
    store.close().await.unwrap();
    // Two more writers, so the active segment's index ends in groups of
    // its own.
    for _ in 0..2 {
        let again = lakelands_store::open(location.clone(), seams(), OpenMode::Write);
        again.await.unwrap().close().await.unwrap();
    }
    let reader = lakelands_store::open(location.clone(), seams(), OpenMode::Read);
    let folded = reader.await.unwrap().head().await.unwrap();
    (location, export, firsts, folded)
}

/// The doors into the directory: the question for the log's ends, the
/// engine's own open in each mode, the store's open in each mode, and
/// an import.
const DOORS: std::ops::Range<usize> = 0..10;

/// What one door answers, with its name.
async fn knock(door: usize, location: &Location, export: &Path) -> (String, Option<Error>) {
    match door {
        0 => {
            let asked = SqliteOpener.log_ends(location).map(|_| ());
            ("log_ends".to_owned(), asked.err())
        }
        1..=4 => {
            let mode = MODES[door - 1];
            let opened = SqliteOpener.open(location, mode).map(drop);
            (format!("the engine's open for {mode:?}"), opened.err())
        }
        5..=8 => {
            let mode = MODES[door - 5];
            let opened = lakelands_store::open(location.clone(), seams(), mode).await;
            let opened = opened.map(|store: Arc<dyn LakeStore>| drop(store));
            (format!("an open for {mode:?}"), opened.err())
        }
        _ => {
            let imported = lakelands_store::import(location.clone(), seams(), export.to_owned());
            ("an import".to_owned(), imported.await.err())
        }
    }
}

/// Each of `doors` refuses the directory as `refused` judges, and after
/// each, what `kept` reads of the directory is as it was. Every door is
/// asked, whatever the one before it did: what a door changed is put
/// back, and the doors that failed are named together.
async fn refused_and_left(
    dir: &Path,
    location: &Location,
    export: &Path,
    doors: impl IntoIterator<Item = usize>,
    kept: fn(&Path) -> Held,
    refused: impl Fn(&Error) -> bool,
) {
    let (all, before) = (held(dir), kept(dir));
    let mut failed = Vec::new();
    for door in doors {
        let (door, e) = knock(door, location, export).await;
        match e {
            None => failed.push(format!("{door} took a lake that lost part of itself")),
            Some(e) if !refused(&e) => {
                failed.push(format!("{door} answered {e:?}"));
            }
            Some(_) => {}
        }
        let after = kept(dir);
        if after != before {
            let changed: Vec<_> = (after.iter().filter(|(p, b)| before.get(*p) != Some(b)))
                .chain(before.iter().filter(|(p, _)| !after.contains_key(*p)))
                .map(|(p, _)| p.strip_prefix(dir).unwrap().display().to_string())
                .collect();
            failed.push(format!("{door} made, cut or changed {changed:?}"));
        }
        put_back(dir, &all);
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// A refusal whose situation `judge` holds.
fn says(judge: impl Fn(&Situation) -> bool) -> impl Fn(&Error) -> bool {
    move |e| e.situation().is_some_and(&judge)
}

fn index_missing(of: u64) -> impl Fn(&Error) -> bool {
    let named = format!("{of:020}.jsonl");
    says(move |s| {
        matches!(s, Situation::Damaged { part: Part::LogIndex, position: None, flaw: Flaw::IndexMissing { name } }
            if *name == named)
    })
}

fn clear(dir: &Path, export: &Path) {
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(export);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_segment_without_its_index_file_is_damage_and_nothing_is_cut() {
    // The active segment's index is gone.
    let dir = fresh("active");
    let (location, export, firsts, _) = made(&dir, 1).await;
    std::fs::remove_file(index(&dir, firsts[0])).unwrap();
    refused_and_left(
        &dir,
        &location,
        &export,
        DOORS,
        held,
        index_missing(firsts[0]),
    )
    .await;
    clear(&dir, &export);

    // A sealed segment's index is gone.
    let dir = fresh("sealed");
    let (location, export, _, _) = made(&dir, 1).await;
    std::fs::remove_file(index(&dir, 0)).unwrap();
    refused_and_left(&dir, &location, &export, DOORS, held, index_missing(0)).await;
    clear(&dir, &export);

    // A later segment has none whatever it holds: a seal writes the next
    // segment's index before the segment comes to exist.
    let dir = fresh("later");
    let (location, export, firsts, _) = made(&dir, 1).await;
    std::fs::remove_file(index(&dir, firsts[0])).unwrap();
    std::fs::write(segment(&dir, firsts[0]), b"").unwrap();
    refused_and_left(
        &dir,
        &location,
        &export,
        DOORS,
        held,
        index_missing(firsts[0]),
    )
    .await;
    clear(&dir, &export);

    // The index directory is gone: the first segment is named, and no
    // door makes the directory again.
    let dir = fresh("directory");
    let (location, export, _, _) = made(&dir, 1).await;
    std::fs::remove_dir_all(dir.join("index")).unwrap();
    refused_and_left(&dir, &location, &export, DOORS, held, index_missing(0)).await;
    clear(&dir, &export);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_projection_with_no_segment_file_is_damage_and_no_lake_is_made_over_it() {
    let no_segment = |e: &Error| {
        matches!(
            e.situation(),
            Some(Situation::Damaged {
                part: Part::Log,
                position: None,
                flaw: Flaw::NoSegmentFile
            })
        )
    };
    // The log and its index are gone, the projection is there.
    let dir = fresh("no-log");
    let (location, export, _, _) = made(&dir, 1).await;
    std::fs::remove_dir_all(dir.join("log")).unwrap();
    std::fs::remove_dir_all(dir.join("index")).unwrap();
    refused_and_left(&dir, &location, &export, DOORS, held, no_segment).await;
    clear(&dir, &export);

    // The log's directory is there and holds no segment.
    let dir = fresh("empty-log");
    let (location, export, firsts, _) = made(&dir, 1).await;
    std::fs::remove_file(segment(&dir, 0)).unwrap();
    std::fs::remove_file(segment(&dir, firsts[0])).unwrap();
    refused_and_left(&dir, &location, &export, DOORS, held, no_segment).await;
    clear(&dir, &export);
}

/// The store's own check, on this engine, comes before the engine cuts.
/// An index that is there and was emptied or shortened reads as a
/// crash's tail, and a writer taking the lease cuts the segment back to
/// it; an active segment that is gone leaves its index to be swept. The
/// projection beside each is folded past what is left, so the store
/// refuses a reader's open, a writer's and an import, and the writers
/// are refused before they take the lease: no file of the log, the
/// lease or the lake's name is cut, removed or written again.
#[tokio::test(flavor = "multi_thread")]
async fn a_writer_refused_for_a_projection_ahead_of_its_log_cuts_nothing() {
    let ahead = |folded: Position| {
        says(move |s| {
            matches!(s, Situation::Damaged { part: Part::Log, position: None, flaw: Flaw::EndsBefore { position } }
                if *position == folded)
        })
    };
    // Where the question for the log's ends sees a lake, an import is
    // refused for that.
    let held_already =
        |e: &Error| matches!(e.case().map(|s| &s.case), Some(Case::ImportNotEmpty { .. }));

    // The active segment's index holds nothing.
    let dir = fresh("emptied");
    let (location, export, firsts, folded) = made(&dir, 1).await;
    std::fs::write(index(&dir, firsts[0]), b"").unwrap();
    refused_and_left(&dir, &location, &export, 5..8, log_held, ahead(folded)).await;
    refused_and_left(&dir, &location, &export, [9], log_held, held_already).await;
    clear(&dir, &export);

    // It lost its last group: the lines after the commit line before
    // the last.
    let dir = fresh("shortened");
    let (location, export, firsts, folded) = made(&dir, 1).await;
    let lines = std::fs::read(index(&dir, firsts[0])).unwrap();
    let commit = b"[\"c\"]\n";
    let ends: Vec<usize> = (lines.windows(commit.len()).enumerate())
        .filter_map(|(at, w)| (w == commit).then_some(at + commit.len()))
        .collect();
    assert!(
        ends.len() >= 2,
        "the active segment's index holds two groups"
    );
    std::fs::write(index(&dir, firsts[0]), &lines[..ends[ends.len() - 2]]).unwrap();
    refused_and_left(&dir, &location, &export, 5..8, log_held, ahead(folded)).await;
    refused_and_left(&dir, &location, &export, [9], log_held, held_already).await;
    clear(&dir, &export);

    // The active segment is gone and its index is there.
    let dir = fresh("no-active");
    let (location, export, firsts, folded) = made(&dir, 1).await;
    std::fs::remove_file(segment(&dir, firsts[0])).unwrap();
    refused_and_left(&dir, &location, &export, 5..8, log_held, ahead(folded)).await;
    refused_and_left(&dir, &location, &export, [9], log_held, held_already).await;
    clear(&dir, &export);

    // A lake of one segment whose index holds nothing: the question for
    // the log's ends sees no lake there, and the import is refused by
    // the store's check.
    let dir = fresh("one-segment");
    let (location, export, _, folded) = made(&dir, 0).await;
    std::fs::write(index(&dir, 0), b"").unwrap();
    assert!(SqliteOpener.log_ends(&location).unwrap().is_none());
    refused_and_left(&dir, &location, &export, 5..8, log_held, ahead(folded)).await;
    refused_and_left(&dir, &location, &export, [9], log_held, ahead(folded)).await;
    // A rebuild makes the projection from the log as it stands: here,
    // from a log read as holding nothing, whose lines it cuts.
    let rebuilt = lakelands_store::open(location.clone(), seams(), OpenMode::Rebuild);
    rebuilt.await.unwrap().close().await.unwrap();
    clear(&dir, &export);
}

/// A log whose segments do not follow each other lost a sealed segment
/// or holds another log's: each begins at the position after the last of
/// the one before it, and its first event names that one's hash.
#[tokio::test(flavor = "multi_thread")]
async fn a_log_whose_segments_do_not_follow_each_other_is_damage() {
    let stands = |expected: u64, found: u64| {
        says(move |s| {
            matches!(s, Situation::Damaged { part: Part::Log, position: None, flaw: Flaw::Position { expected: e, found: Some(f) } }
                if *e == Position(expected) && *f == Position(found))
        })
    };
    // The first segment is gone: its index is left, and stays.
    let dir = fresh("first-gone");
    let (location, export, firsts, _) = made(&dir, 2).await;
    std::fs::remove_file(segment(&dir, 0)).unwrap();
    refused_and_left(&dir, &location, &export, DOORS, held, stands(0, firsts[0])).await;
    clear(&dir, &export);

    // A sealed segment between two is gone.
    let dir = fresh("middle-gone");
    let (location, export, firsts, _) = made(&dir, 2).await;
    std::fs::remove_file(segment(&dir, firsts[0])).unwrap();
    let gap = stands(firsts[0], firsts[1]);
    refused_and_left(&dir, &location, &export, DOORS, held, gap).await;
    clear(&dir, &export);

    // Another lake's first segment and its index stand where this
    // lake's were: the same positions, and other events.
    let (dir, other) = (fresh("foreign"), fresh("foreign-other"));
    let (location, export, firsts, _) = made(&dir, 2).await;
    let (_, other_export, other_firsts, _) = made(&other, 2).await;
    assert_eq!(firsts, other_firsts, "two lakes made alike seal alike");
    std::fs::copy(segment(&other, 0), segment(&dir, 0)).unwrap();
    std::fs::copy(index(&other, 0), index(&dir, 0)).unwrap();
    let at = firsts[0];
    let prev = says(move |s| {
        matches!(s, Situation::Damaged { part: Part::Log, position: Some(p), flaw: Flaw::PrevHash }
            if *p == Position(at))
    });
    refused_and_left(&dir, &location, &export, DOORS, held, prev).await;
    clear(&dir, &export);
    clear(&other, &other_export);
}

/// A directory whose making stopped before its first segment holds no
/// lake: its name is written and the log's directories are made, one
/// or both. No open but one to create makes anything in it, and that
/// one finishes the lake.
#[tokio::test(flavor = "multi_thread")]
async fn a_making_that_stopped_before_its_first_segment_is_no_lake() {
    let no_lake = says(|s| matches!(s, Situation::NoLake { .. }));
    for with_index in [false, true] {
        let dir = fresh("stopped");
        std::fs::create_dir_all(dir.join("log")).unwrap();
        std::fs::write(dir.join("lake"), "kept\n").unwrap();
        if with_index {
            std::fs::create_dir_all(dir.join("index")).unwrap();
        }
        let location = Location::sqlite("kept", &dir);
        assert!(SqliteOpener.log_ends(&location).unwrap().is_none());
        // Every open but the two to create: Read, Write and Rebuild, the
        // engine's and the store's.
        let doors = [1, 2, 4, 5, 6, 8];
        refused_and_left(&dir, &location, &dir, doors, held, &no_lake).await;
        let store = lakelands_store::open(location.clone(), seams(), OpenMode::Create);
        store.await.unwrap().close().await.unwrap();
        let (first, _) = SqliteOpener.log_ends(&location).unwrap().expect("a lake");
        assert_eq!(first.position, Position(0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// What a crash leaves is read as before: a first segment of no bytes
/// with no index yet is a lake being made, and the writer makes the
/// index.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_first_segment_with_no_index_yet_is_a_lake_being_made() {
    let dir = fresh("being-made");
    std::fs::create_dir_all(dir.join("log")).unwrap();
    std::fs::create_dir_all(dir.join("index")).unwrap();
    std::fs::write(segment(&dir, 0), b"").unwrap();
    let location = Location::sqlite("kept", &dir);
    assert!(SqliteOpener.log_ends(&location).unwrap().is_none());
    assert!(!index(&dir, 0).exists(), "asking for the ends made a file");
    let store = lakelands_store::open(location.clone(), seams(), OpenMode::Create);
    store.await.unwrap().close().await.unwrap();
    assert!(index(&dir, 0).is_file());
    let (first, _) = SqliteOpener.log_ends(&location).unwrap().expect("a lake");
    assert_eq!(first.position, Position(0));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `projection.db` that holds no table is a projection being made: its
/// maker has connected and not made the tables yet. A reader is told
/// that there is no projection yet, as the moment before, not that
/// another build laid it out and a rebuild is wanted; and a writer makes
/// it.
#[tokio::test(flavor = "multi_thread")]
async fn a_projection_with_no_table_yet_is_a_projection_being_made() {
    let dir = fresh("no-table");
    let (location, export, _, _) = made(&dir, 0).await;
    for name in ["projection.db", "projection.db-wal", "projection.db-shm"] {
        let _ = std::fs::remove_file(dir.join(name));
    }
    let maker = rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    maker.pragma_update(None, "journal_mode", "WAL").unwrap();
    let no_projection = says(|s| matches!(s, Situation::NoProjection { .. }));
    // A reader, the engine's and the store's, while the maker is there.
    refused_and_left(&dir, &location, &export, [1, 5], beside_wal, no_projection).await;
    drop(maker);
    let writer = lakelands_store::open(location.clone(), seams(), OpenMode::Write);
    writer.await.unwrap().close().await.unwrap();
    // The projection the writer made is folded through the log's tip.
    let reader = lakelands_store::open(location.clone(), seams(), OpenMode::Read);
    let head = reader.await.unwrap().head().await.unwrap();
    let (_, last) = SqliteOpener.log_ends(&location).unwrap().expect("a lake");
    assert_eq!(head, last.position);
    clear(&dir, &export);
}

//! A rebuild beside the projection, on the SQLite engine (lakelands#1322):
//! a builder killed halfway leaves the projection answering and its
//! scratch for the next writer's open to remove; a scratch is built beside
//! a projection it never opens; a swap is refused while another handle
//! holds the projection it would replace, as the question asked before
//! it answers, and a handle that opens during one reads the projection put
//! in place; a mark the log does not reach
//! keeps no scratch; writers' opens the lease refuses fail no build and
//! no swap (lakelands#1485).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use fs4::FileExt;
use lakelands_api::engine::{EngineOpener, LakeEngine, Lease, OpenMode};
use lakelands_api::error::Situation;
use lakelands_api::event::Digest;
use lakelands_api::{Error, Location, Position};
use lakelands_core::conformance::engine::chain;
use lakelands_engine_sqlite::SqliteOpener;

/// Set to a lake's directory, the child this test binary runs as.
const CHILD: &str = "LAKELANDS_TEST_SCRATCH_CHILD";

fn dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("rebuild-beside-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn at(dir: &Path) -> Location {
    Location::sqlite("beside", dir)
}

fn open(dir: &Path, mode: OpenMode) -> lakelands_api::Result<Box<dyn LakeEngine>> {
    SqliteOpener.open(&at(dir), mode)
}

/// A lake of five events with its projection folded through 1, and the
/// writer's lease.
fn lake(dir: &Path) -> (Box<dyn LakeEngine>, Lease) {
    let writer = open(dir, OpenMode::Create).unwrap();
    let lease = writer.take_lease("writer").unwrap();
    writer
        .append(&lease, &chain(0, 5, &Digest::zero(), &lease))
        .unwrap();
    fold(writer.as_ref(), 1);
    (writer, lease)
}

/// An empty fold through `through`.
fn fold(engine: &dyn LakeEngine, through: u64) {
    engine.projection().begin().unwrap();
    engine.projection().commit(Position(through)).unwrap();
}

fn scratch(dir: &Path) -> lakelands_api::Result<Box<dyn LakeEngine>> {
    SqliteOpener.open_scratch(&at(dir))
}

/// A scratch folded through 4 and marked built, its handle dropped.
fn built(dir: &Path) {
    let built = scratch(dir).unwrap();
    fold(built.as_ref(), 4);
    assert_eq!(built.mark_built().unwrap(), Some(Position(4)));
}

fn folded(engine: &dyn LakeEngine) -> Option<Position> {
    engine.projection().folded_through().unwrap()
}

/// The scratch's files left in `dir`, but an empty mark file, which is kept.
fn left(dir: &Path) -> Vec<String> {
    let names = std::fs::read_dir(dir).unwrap().flatten();
    let mut names: Vec<_> = names
        .filter(|e| e.metadata().is_ok_and(|m| m.len() > 0))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.contains("rebuild"))
        .collect();
    names.sort();
    names
}

fn refused<T: std::fmt::Debug>(
    answer: lakelands_api::Result<T>,
    is: impl Fn(&Situation) -> bool,
) -> bool {
    matches!(&answer, Err(Error::Unavailable { situation }) if is(situation))
}

/// The child: folds into the scratch and dies before it marks it.
#[test]
fn a_builder_that_dies_halfway() {
    let Some(dir) = std::env::var_os(CHILD) else {
        return;
    };
    let built = scratch(Path::new(&dir)).unwrap();
    fold(built.as_ref(), 3);
    std::process::abort();
}

#[test]
fn a_builder_killed_halfway_leaves_the_projection_answering() {
    let dir = dir("killed");
    let (writer, lease) = lake(&dir);
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "a_builder_that_dies_halfway", "--test-threads=1"])
        .env(CHILD, &dir)
        .output()
        .unwrap();
    assert!(!child.status.success(), "the builder was to die: {child:?}");
    assert!(
        left(&dir).contains(&"projection.db.rebuild".to_owned()),
        "the builder left its scratch: {:?}",
        left(&dir)
    );
    assert_eq!(folded(writer.as_ref()), Some(Position(1)));
    let reader = open(&dir, OpenMode::Read).unwrap();
    assert_eq!(folded(reader.as_ref()), Some(Position(1)));
    let swapped = writer.swap(&lease);
    assert!(
        refused(swapped, |s| matches!(s, Situation::NoScratch { .. })),
        "a scratch never marked is not swapped in"
    );
    drop(reader);
    drop(writer);
    let again = open(&dir, OpenMode::Write).unwrap();
    assert!(
        left(&dir).contains(&"projection.db.rebuild".to_owned()),
        "an open the lease may yet refuse leaves it (lakelands#1485)"
    );
    again.take_lease("next").unwrap();
    assert_eq!(
        left(&dir),
        Vec::<String>::new(),
        "the next writer removes it once it holds the lease"
    );
    assert_eq!(folded(again.as_ref()), Some(Position(1)));
}

#[test]
fn a_scratch_is_built_beside_a_projection_that_is_no_database() {
    let dir = dir("garbage");
    drop(lake(&dir));
    let projection = dir.join("projection.db");
    for end in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(dir.join(format!("projection.db{end}")));
    }
    std::fs::write(&projection, b"not a database, so no open of it succeeds").unwrap();
    let built = scratch(&dir).unwrap();
    fold(built.as_ref(), 4);
    assert_eq!(built.mark_built().unwrap(), Some(Position(4)));
    drop(built);
    assert_eq!(
        std::fs::read(&projection).unwrap(),
        b"not a database, so no open of it succeeds"
    );
    assert!(
        open(&dir, OpenMode::Read).is_err(),
        "a reader opens what is no database"
    );
}

#[test]
fn a_swap_from_a_rebuild_handle_over_an_older_layout_waits_for_every_other_handle() {
    let dir = dir("older");
    let (writer, _) = lake(&dir);
    drop(writer);
    rusqlite::Connection::open(dir.join("projection.db"))
        .and_then(|db| db.execute("UPDATE meta SET v = v - 1 WHERE k = 'layout'", []))
        .unwrap();
    assert!(
        open(&dir, OpenMode::Read).is_err(),
        "a reader's open refuses the older layout"
    );
    let rebuilder = open(&dir, OpenMode::Rebuild).unwrap();
    let lease = rebuilder.take_lease("rebuilder").unwrap();
    built(&dir);
    assert_eq!(
        left(&dir),
        ["projection.db.rebuild", "projection.db.rebuild.mark"],
        "a marked scratch outlives its handle, and its journal is in its file"
    );
    assert!(rebuilder.may_swap().unwrap(), "no other handle holds it");
    let other = open(&dir, OpenMode::Rebuild).unwrap();
    // Asked before the swap, it answers as the swap does, and leaves the
    // handle holding the projection as before.
    assert!(!rebuilder.may_swap().unwrap(), "another handle holds it");
    assert!(refused(rebuilder.swap(&lease), |s| matches!(
        s,
        Situation::SwapBusy { .. }
    )));
    drop(other);
    assert!(rebuilder.may_swap().unwrap(), "the other handle is gone");
    assert_eq!(rebuilder.swap(&lease).unwrap(), Some(Position(4)));
    assert_eq!(left(&dir), Vec::<String>::new());
    assert_eq!(folded(rebuilder.as_ref()), Some(Position(4)));
    // An idle reader keeps the next swap out as well.
    let reader = open(&dir, OpenMode::Read).unwrap();
    assert_eq!(folded(reader.as_ref()), Some(Position(4)));
    built(&dir);
    assert!(refused(rebuilder.swap(&lease), |s| matches!(
        s,
        Situation::SwapBusy { .. }
    )));
    assert_eq!(folded(reader.as_ref()), Some(Position(4)));
    drop(reader);
    assert_eq!(rebuilder.swap(&lease).unwrap(), Some(Position(4)));
    fold(rebuilder.as_ref(), 4);
}

#[test]
fn a_reader_that_opens_during_a_swap_reads_the_projection_put_in_place() {
    let dir = dir("during");
    let (writer, _) = lake(&dir);
    built(&dir);
    drop(writer);
    let scratch = dir.join("projection.db.rebuild");
    rusqlite::Connection::open(&scratch)
        .and_then(|db| db.query_row("PRAGMA journal_mode = DELETE", [], |_| Ok(())))
        .unwrap();
    // The test stands where a swap does: it holds the projection, replaces
    // the file and lets go.
    let swap = std::fs::File::open(dir.join("lake")).unwrap();
    FileExt::lock(&swap).unwrap();
    let (tx, rx) = mpsc::channel();
    let opening = dir.clone();
    let reader = std::thread::spawn(move || {
        let reader = open(&opening, OpenMode::Read).unwrap();
        tx.send(folded(reader.as_ref())).unwrap();
        reader
    });
    let early = rx.recv_timeout(Duration::from_millis(500));
    assert!(early.is_err(), "a reader opened during the swap: {early:?}");
    for end in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(dir.join(format!("projection.db{end}")));
    }
    std::fs::rename(&scratch, dir.join("projection.db")).unwrap();
    FileExt::unlock(&swap).unwrap();
    assert_eq!(rx.recv().unwrap(), Some(Position(4)));
    drop(reader.join().unwrap());
}

#[test]
fn a_mark_past_the_logs_end_keeps_no_scratch() {
    let dir = dir("past");
    let (writer, lease) = lake(&dir);
    built(&dir);
    let mark = dir.join("projection.db.rebuild.mark");
    let text = std::fs::read_to_string(&mark).unwrap();
    std::fs::write(&mark, text.replace("\"position\":4", "\"position\":99")).unwrap();
    assert!(refused(writer.swap(&lease), |s| matches!(
        s,
        Situation::NoScratch { .. }
    )));
    assert_eq!(folded(writer.as_ref()), Some(Position(1)));
    assert_eq!(folded(scratch(&dir).unwrap().as_ref()), None);
}

/// Two threads opening the lake for writing, each refused the lease,
/// fail none of its holder's builds beside and swaps.
#[test]
fn writers_the_lease_refuses_fail_no_build_and_no_swap() {
    let dir = dir("refused");
    let (writer, lease) = lake(&dir);
    let done = Arc::new(AtomicBool::new(false));
    let probes: Vec<_> = (0..2)
        .map(|_| (dir.clone(), done.clone()))
        .map(|(dir, done)| {
            std::thread::spawn(move || {
                let mut refused = 0;
                while !done.load(Ordering::SeqCst) {
                    let probe = open(&dir, OpenMode::Write);
                    refused += u32::from(probe.is_ok_and(|p| p.take_lease("probe").is_err()));
                    std::thread::sleep(Duration::from_millis(1));
                }
                refused
            })
        })
        .collect();
    let busy = |s: &Situation| matches!(s, Situation::SwapBusy { .. });
    for _ in 0..150 {
        built(&dir);
        let swapped = std::iter::repeat_with(|| writer.swap(&lease));
        let mut swapped = swapped
            .skip_while(|a| matches!(a, Err(Error::Unavailable { situation }) if busy(situation)));
        swapped.next().unwrap().unwrap();
    }
    done.store(true, Ordering::SeqCst);
    let refused: u32 = probes.into_iter().map(|p| p.join().unwrap()).sum();
    assert!(refused > 0, "no writer was refused the lease");
}

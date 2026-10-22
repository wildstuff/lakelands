//! A rebuild beside the projection, on the SQLite engine (lakelands#1322):
//! a builder killed halfway leaves the projection answering and its
//! scratch for the next writer's open to remove; a swap is refused while
//! another handle reads the projection it would replace; and a scratch is
//! built beside a projection of an older layout, under a writer's lease,
//! without opening that projection.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// A lake of five events with its projection folded through 1, and the
/// writer's lease.
fn lake(dir: &Path) -> (Box<dyn LakeEngine>, Lease) {
    let writer = SqliteOpener
        .open(&Location::sqlite("beside", dir), OpenMode::Create)
        .unwrap();
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
    SqliteOpener.open_scratch(&Location::sqlite("beside", dir))
}

fn left(dir: &Path) -> Vec<String> {
    let names = std::fs::read_dir(dir).unwrap().flatten();
    let mut names: Vec<_> = names
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.contains("rebuild"))
        .collect();
    names.sort();
    names
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
    assert_eq!(
        writer.projection().folded_through().unwrap(),
        Some(Position(1))
    );
    let reader = SqliteOpener
        .open(&Location::sqlite("beside", &dir), OpenMode::Read)
        .unwrap();
    assert_eq!(
        reader.projection().folded_through().unwrap(),
        Some(Position(1))
    );
    let swapped = writer.swap(&lease);
    assert!(
        matches!(&swapped, Err(Error::Unavailable { situation }) if matches!(**situation, Situation::NoScratch { .. })),
        "{swapped:?}"
    );
    drop(reader);
    drop(writer);
    let again = SqliteOpener
        .open(&Location::sqlite("beside", &dir), OpenMode::Write)
        .unwrap();
    assert_eq!(
        left(&dir),
        Vec::<String>::new(),
        "the next writer's open removes it"
    );
    assert_eq!(
        again.projection().folded_through().unwrap(),
        Some(Position(1))
    );
}

#[test]
fn a_swap_waits_for_no_reader_and_reads_nothing_of_an_older_layout() {
    let dir = dir("older");
    let (writer, lease) = lake(&dir);
    let projection = dir.join("projection.db");
    rusqlite::Connection::open(&projection)
        .and_then(|db| db.execute("UPDATE meta SET v = v - 1 WHERE k = 'layout'", []))
        .unwrap();
    let reader = SqliteOpener.open(&Location::sqlite("beside", &dir), OpenMode::Read);
    assert!(reader.is_err(), "a reader's open refuses the older layout");
    let before = std::fs::read(&projection).unwrap();
    let built = scratch(&dir).unwrap();
    assert!(built.take_lease("scratch").is_err());
    fold(built.as_ref(), 4);
    assert_eq!(built.mark_built().unwrap(), Some(Position(4)));
    drop(built);
    assert_eq!(
        std::fs::read(&projection).unwrap(),
        before,
        "the scratch never wrote it"
    );
    assert_eq!(
        left(&dir),
        ["projection.db.rebuild", "projection.db.rebuild.mark"],
        "a marked scratch outlives its handle, and its journal is in its file"
    );
    // A handle onto the projection keeps it from being replaced.
    let other = SqliteOpener
        .open(&Location::sqlite("beside", &dir), OpenMode::Rebuild)
        .unwrap();
    let swapped = writer.swap(&lease);
    assert!(
        matches!(&swapped, Err(Error::Unavailable { situation }) if matches!(**situation, Situation::SwapBusy { .. })),
        "{swapped:?}"
    );
    assert_eq!(
        writer.projection().folded_through().unwrap(),
        Some(Position(1))
    );
    drop(other);
    assert_eq!(writer.swap(&lease).unwrap(), Some(Position(4)));
    assert_eq!(left(&dir), Vec::<String>::new());
    let reader = SqliteOpener
        .open(&Location::sqlite("beside", &dir), OpenMode::Read)
        .unwrap();
    assert_eq!(
        reader.projection().folded_through().unwrap(),
        Some(Position(4))
    );
    fold(writer.as_ref(), 4);
}

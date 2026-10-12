//! A lake written in another release's format is not opened
//! (lakelands#801,
//! `docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
//!
//! The store asks the opener for the ends of the log before it opens the
//! lake. Here that question reads this engine's files and leaves the
//! directory as it was, byte for byte, and makes nothing where there is
//! no lake; and a lake as a later release would leave it, its tip in
//! that release's format and its projection without a layout mark, is refused
//! for its format in every mode and not for its layout, with nothing in
//! its directory made or changed. An import onto either lake is answered
//! from the same question, and changes no file of it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lakelands_api::engine::{CanonicalEvent, EngineOpener, OpenMode};
use lakelands_api::error::{Case, Situation};
use lakelands_api::event::{Digest, EventBody, LOG_FORMAT_VERSION};
use lakelands_api::seam::Seams;
use lakelands_api::{Location, Position, Timestamp};
use lakelands_core::canonical;
use lakelands_engine_sqlite::SqliteOpener;
use lakelands_store::log::{Draft, seal};
use serde_json::json;

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    }
}

/// A directory of the test's own, with nothing in it.
fn fresh(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("another-format-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Every file under `dir`, with its bytes.
fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(files(&path));
        } else {
            out.insert(path.clone(), std::fs::read(&path).unwrap());
        }
    }
    out
}

/// A lake a store of this release made and closed, and an export of it
/// beside its directory.
async fn made(dir: &Path) -> (Location, PathBuf) {
    let location = Location::sqlite("kept", dir);
    let store = lakelands_store::open(location.clone(), seams(), OpenMode::Create);
    let store = store.await.unwrap();
    let export = dir.with_extension("export");
    let _ = std::fs::remove_dir_all(&export);
    let written = lakelands_core::export::write(store.as_ref(), &export);
    written.await.unwrap();
    store.close().await.unwrap();
    (location, export)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_ends_of_a_log_are_read_and_its_directory_is_left_as_it_was() {
    let dir = fresh("ends");
    let (location, export) = made(&dir).await;
    let before = files(&dir);
    let (first, last) = SqliteOpener.log_ends(&location).unwrap().expect("a lake");
    assert_eq!(files(&dir), before, "reading the ends changed a file");
    assert_eq!(first.position, Position(0));
    // An import onto it is told what it holds, and opens nothing of it.
    let e = lakelands_store::import(location.clone(), seams(), export.clone()).await;
    let e = e.expect_err("an import onto a lake that holds events was taken");
    assert!(
        matches!(e.case().map(|s| &s.case), Some(Case::ImportNotEmpty { head, .. }) if *head == last.position),
        "{e:?}"
    );
    assert_eq!(files(&dir), before, "an import changed a file of the lake");
    let engine = SqliteOpener.open(&location, OpenMode::Read).unwrap();
    assert_eq!(engine.tip().unwrap(), Some((last.position, last.hash)));
    drop(engine);
    // Where there is no lake there is none after: no directory is made.
    let nowhere = fresh("nowhere");
    let asked = SqliteOpener.log_ends(&Location::sqlite("kept", &nowhere));
    assert!(asked.unwrap().is_none());
    assert!(
        !nowhere.exists(),
        "asking for a log's ends made a directory"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&export);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lake_a_later_release_left_is_refused_for_its_format_and_not_touched() {
    let reads = LOG_FORMAT_VERSION;
    let dir = fresh("later");
    let (location, export) = made(&dir).await;
    // Its writer's last event, in the format after this release's.
    let engine = SqliteOpener.open(&location, OpenMode::Write).unwrap();
    let lease = engine.take_lease("a later release").unwrap();
    let (at, prev) = engine.tip().unwrap().unwrap();
    let (fence, holder) = (lease.fence, "a later release".into());
    let taken = Draft::system(
        Timestamp(i64::MAX / 2),
        EventBody::LeaseTaken { fence, holder },
    );
    let (_, stored) = seal(&"kept".into(), at.next(), &prev, fence, taken);
    let mut event: serde_json::Value = serde_json::from_slice(&stored.bytes).unwrap();
    event["v"] = json!(reads + 1);
    event.as_object_mut().unwrap().remove("hash");
    let hash = canonical::sha256_hex(&event);
    event["hash"] = json!(hash);
    let line = CanonicalEvent {
        hash: Digest(hash),
        bytes: canonical::to_vec(&event),
        ..stored
    };
    engine.append(&lease, &[line]).unwrap();
    engine.release_lease(&lease).unwrap();
    drop(engine);
    // Its projection carries no layout mark: that release made none.
    let projection = rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    let unmarked = projection.execute("DELETE FROM meta WHERE k = 'layout'", []);
    assert_eq!(unmarked.unwrap(), 1);
    drop(projection);
    let before = files(&dir);
    let another = |e: &lakelands_api::Error| {
        matches!(e.situation(), Some(Situation::LogFormat { lake, found, reads: r, .. })
            if lake == "kept" && *found == u64::from(reads + 1) && *r == u64::from(reads))
    };
    for mode in [
        OpenMode::Read,
        OpenMode::Write,
        OpenMode::Create,
        OpenMode::Rebuild,
    ] {
        let Err(e) = lakelands_store::open(location.clone(), seams(), mode).await else {
            panic!("a lake of a later release opened for {mode:?}");
        };
        assert!(another(&e), "{mode:?}: {e:?}");
        assert_eq!(files(&dir), before, "{mode:?} changed a file of the lake");
    }
    let e = lakelands_store::import(location.clone(), seams(), export.clone()).await;
    let e = e.expect_err("an import onto a lake of a later release was taken");
    assert!(another(&e), "an import: {e:?}");
    assert_eq!(files(&dir), before, "an import changed a file of the lake");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&export);
}

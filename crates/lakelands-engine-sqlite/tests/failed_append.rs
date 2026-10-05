//! What lies past the committed end of the log's files, and the append
//! that left it there (lakelands#618).
//!
//! An append writes the segment's lines, then the index's. One that fails
//! after it wrote cuts back what it wrote before it answers its error, and
//! every append and every seal starts from the committed lengths, whoever
//! left bytes past them: a writer never writes behind such bytes and never
//! seals a file that holds them. A handle whose lease is no longer the
//! lake's cuts nothing.
//!
//! The append that fails is a real one: the process's file size limit
//! ends a write in the middle of a line. The limit is the process's, so
//! the tests here run one at a time, in a binary of their own, and they
//! are Linux's.

#![cfg(target_os = "linux")]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use lakelands_api::Location;
use lakelands_api::engine::{CanonicalEvent, EngineOpener, LakeEngine, Lease, OpenMode};
use lakelands_api::event::Digest;
use lakelands_api::id::Position;
use lakelands_core::conformance::engine::chain;
use lakelands_engine_sqlite::SqliteOpener;
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
use sha2::{Digest as _, Sha256};
use tokio::signal::unix::{SignalKind, signal};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// `SIGXFSZ`, which a write past the file size limit raises: taken by a
/// handler, the write answers its error; left alone, it ends the process.
const XFSZ: i32 = 25;

fn open(dir: &Path, mode: OpenMode) -> Box<dyn LakeEngine> {
    SqliteOpener
        .open(&Location::sqlite("lake", dir), mode)
        .unwrap()
}

fn add(path: &Path, bytes: &str) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes.as_bytes()).unwrap();
}

/// `n` chained events from `start` whose bytes say which append wrote
/// them.
fn said(start: u64, n: u64, prev: &Digest, lease: &Lease, by: &str) -> Vec<CanonicalEvent> {
    let mut events = chain(start, n, prev, lease);
    for e in &mut events {
        e.bytes = format!("{{\"n\":{},\"by\":\"{by}\"}}", e.position.0).into_bytes();
    }
    events
}

/// The segment file that holds `events` and nothing else.
fn lines(events: &[CanonicalEvent]) -> Vec<u8> {
    events
        .iter()
        .flat_map(|e| e.bytes.iter().copied().chain(*b"\n"))
        .collect()
}

/// Index lines of events that were never committed, from position `from`.
fn uncommitted(from: u64, n: u64) -> String {
    (from..from + n)
        .map(|p| format!("[\"e\",{p},1,0,0,\"x\",\"y\",\"z\"]\n"))
        .collect()
}

/// `f` under a file size limit of `limit` bytes: the kernel ends the write
/// that reaches it there, and refuses the rest.
fn limited<R>(limit: u64, f: impl FnOnce() -> R) -> R {
    let was = getrlimit(Resource::Fsize);
    let (current, maximum) = (Some(limit), was.maximum);
    setrlimit(Resource::Fsize, Rlimit { current, maximum }).unwrap();
    let answer = f();
    setrlimit(Resource::Fsize, was).unwrap();
    answer
}

/// A lake of six committed events whose writer holds its lease.
struct Scene {
    dir: PathBuf,
    writer: Box<dyn LakeEngine>,
    lease: Lease,
    committed: Vec<CanonicalEvent>,
    _alone: MutexGuard<'static, ()>,
}

impl Scene {
    fn new(name: &str) -> Scene {
        let _alone = lakelands_core::lock(&ONE_AT_A_TIME);
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let writer = open(&dir, OpenMode::Create);
        let lease = writer.take_lease("writer").unwrap();
        let committed = said(0, 6, &Digest::zero(), &lease, "committed");
        for group in committed.chunks(2) {
            writer.append(&lease, group).unwrap();
        }
        Scene {
            dir,
            writer,
            lease,
            committed,
            _alone,
        }
    }

    fn segment(&self) -> PathBuf {
        self.dir.join("log").join(format!("{:020}.jsonl", 0))
    }

    fn index(&self) -> PathBuf {
        self.dir.join("index").join(format!("{:020}.idx", 0))
    }

    /// How long the segment and its index are.
    fn lengths(&self) -> (u64, u64) {
        [self.segment(), self.index()]
            .map(|p| std::fs::metadata(p).unwrap().len())
            .into()
    }

    /// `n` events that follow the committed ones.
    fn next(&self, n: u64, by: &str) -> Vec<CanonicalEvent> {
        said(6, n, &self.committed[5].hash, &self.lease, by)
    }

    /// The writer and a reader opened now both read `all`, and the segment
    /// file holds its lines and nothing else.
    fn holds(&self, all: &[CanonicalEvent], what: &str) {
        let reader = open(&self.dir, OpenMode::Read);
        for (who, engine) in [("the writer", &self.writer), ("a reader", &reader)] {
            assert_eq!(engine.read(Position(0), 100).unwrap(), all, "{what}, {who}");
            let seven = engine.read(Position(7), 1).unwrap();
            assert_eq!(seven, all[7..8], "{what}, {who}, position 7 alone");
        }
        let file = std::fs::read(self.segment()).unwrap();
        assert_eq!(file, lines(all), "{what}: the segment file");
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An append the kernel ends in the middle of a line, of the index (the
/// segment's lines are all written by then) or of the segment, answers its
/// error with both files at their committed lengths and the handle at its
/// committed state. The same handle's next appends are then read back byte
/// for byte, by it, by a reader and by the next writer, whose lease finds
/// nothing to cut.
#[tokio::test(flavor = "multi_thread")]
async fn an_append_that_fails_after_it_wrote_cuts_back_before_it_answers() {
    let _taken = signal(SignalKind::from_raw(XFSZ)).unwrap();
    for torn in ["the index", "the segment"] {
        let s = Scene::new("torn");
        let committed = s.lengths();
        let first = s.next(3, "the append that fails");
        let limit = match torn {
            "the index" => committed.1 + 300,
            _ => committed.0 + first[0].bytes.len() as u64 + 10,
        };
        let failed = limited(limit, || s.writer.append(&s.lease, &first)).unwrap_err();
        assert!(failed.to_string().contains("File too large"), "{failed}");
        assert_eq!(s.lengths(), committed, "{torn} torn: {failed}");
        assert_eq!(s.writer.tip().unwrap().unwrap().0, Position(5), "{torn}");
        assert_eq!(s.writer.read(Position(0), 100).unwrap(), s.committed);

        let again = s.next(2, "the next append");
        let third = said(8, 1, &again[1].hash, &s.lease, "a third");
        s.writer.append(&s.lease, &again).unwrap();
        s.writer.append(&s.lease, &third).unwrap();
        let all = [&s.committed[..], &again, &third].concat();
        s.holds(&all, torn);

        s.writer.release_lease(&s.lease).unwrap();
        let (next, appended) = (open(&s.dir, OpenMode::Write), s.lengths());
        next.take_lease("next").unwrap();
        assert_eq!(
            s.lengths(),
            appended,
            "{torn}: the next lease cut something"
        );
        assert_eq!(next.read(Position(0), 100).unwrap(), all, "{torn}");
    }
}

/// An append starts from the committed lengths: bytes past them, in the
/// segment, in the index or in both, are cut before it writes, so it is
/// never written behind them.
#[test]
fn an_append_cuts_what_lies_past_the_committed_end_before_it_writes() {
    for (what, in_segment, in_index) in [
        ("half a line in the segment", "{\"n\":6,\"by", String::new()),
        (
            "lines in both",
            "{\"left\":6}\n{\"left\":7}\n",
            uncommitted(6, 2),
        ),
        ("half a line in the index", "", "[\"e\",6,1,15".to_owned()),
    ] {
        let s = Scene::new("left");
        add(&s.segment(), in_segment);
        add(&s.index(), &in_index);
        let more = s.next(2, "appended");
        s.writer.append(&s.lease, &more).unwrap();
        s.holds(&[&s.committed[..], &more].concat(), what);
    }
}

/// A seal starts from the committed lengths too: the file it seals holds
/// its events' lines and nothing else, and its digest is the file's.
#[test]
fn a_seal_cuts_what_lies_past_the_committed_end_before_it_seals() {
    let s = Scene::new("sealed");
    add(&s.segment(), "{\"left\":6}\n{\"le");
    add(&s.index(), &uncommitted(6, 1));
    let sealed = s.writer.seal_segment(&s.lease).unwrap().unwrap();
    let file = std::fs::read(s.segment()).unwrap();
    assert_eq!(file, lines(&s.committed));
    let files = lakelands_core::canonical::hex(&Sha256::digest(&file));
    assert_eq!(sealed.digest.0, files);
}

/// A handle whose lease is no longer the lake's cuts nothing: what lies
/// past the committed end then is the append another writer is writing.
/// The handle gave its lease up and another took it; or it holds its lock
/// still and the lease file records a later token.
#[test]
fn a_handle_whose_lease_is_no_longer_the_lakes_cuts_nothing() {
    for lost in ["released and taken", "a later token in the file"] {
        let s = Scene::new("stale");
        let other = open(&s.dir, OpenMode::Write);
        if lost == "released and taken" {
            s.writer.release_lease(&s.lease).unwrap();
            other.take_lease("other").unwrap();
        } else {
            let later = format!("{{\"fence\":{},\"holder\":\"other\"}}\n", s.lease.fence + 1);
            std::fs::write(s.dir.join("lease"), later).unwrap();
        }
        add(
            &s.segment(),
            "{\"n\":6,\"by\":\"the other writer\"}\n{\"n\":7",
        );
        add(&s.index(), &uncommitted(6, 1));
        let in_flight = s.lengths();
        assert!(
            s.writer.append(&s.lease, &s.next(1, "stale")).is_err(),
            "{lost}"
        );
        assert!(s.writer.seal_segment(&s.lease).is_err(), "{lost}");
        assert_eq!(s.lengths(), in_flight, "{lost}: the stale handle cut");
    }
}

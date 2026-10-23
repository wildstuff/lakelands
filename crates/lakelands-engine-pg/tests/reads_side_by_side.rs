//! The reads of one lake run side by side (lakelands#1324). Another
//! session holds the lake's `proposals` locked, so a view's read of the
//! proposals waits inside its read transaction; a get of a record, which
//! reads no proposal, answers meanwhile on another reader. With one reader
//! a handle's get waited behind that read. And the reads of one lake that
//! wait hold no more than their share of the readers the process's lakes
//! share: another lake's get answers meanwhile.

mod common;

use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::ProposalFilter;
use lakelands_engine_pg::{LAKE_READERS, PgOpener};

/// A lake in a fresh schema holding one record, `t` `a`.
fn lake_of_one(url: &str) -> (Arc<dyn LakeEngine>, String) {
    let schema = common::fresh_schema();
    let location = common::location(url, "side", &schema);
    let engine: Arc<dyn LakeEngine> = PgOpener.open(&location, OpenMode::Create).unwrap().into();
    let p = engine.projection();
    p.begin().unwrap();
    p.put_record(&StoredRecord {
        ty: TypeName::from("t"),
        key: RecordKey::from("a"),
        version: Version(1),
        fields: Default::default(),
        meta: Default::default(),
        position: Position(1),
        made: Position(1),
        retired: false,
        restricted: false,
    })
    .unwrap();
    p.commit(Position(1)).unwrap();
    (engine, schema)
}

/// Another session holding `schema`'s proposals locked, until something
/// is sent on the sender answered, or it is dropped.
fn hold_proposals(url: &str, schema: &str) -> (mpsc::Sender<()>, thread::JoinHandle<()>) {
    let (locked, release) = (mpsc::channel(), mpsc::channel::<()>());
    let holder = {
        let (url, schema, locked) = (url.to_owned(), schema.to_owned(), locked.0);
        thread::spawn(move || {
            let mut other = postgres::Client::connect(&url, postgres::NoTls).unwrap();
            let lock = format!("LOCK TABLE \"{schema}\".proposals IN ACCESS EXCLUSIVE MODE");
            let mut t = other.transaction().unwrap();
            t.batch_execute(&lock).unwrap();
            locked.send(()).unwrap();
            let _ = release.1.recv();
        })
    };
    locked.1.recv().unwrap();
    (release.0, holder)
}

/// A view's read of the proposals, on a thread of its own.
fn read_proposals(
    engine: &Arc<dyn LakeEngine>,
) -> thread::JoinHandle<lakelands_api::Result<usize>> {
    let engine = engine.clone();
    thread::spawn(move || {
        let view = engine.view(None)?;
        view.proposals(&ProposalFilter::default()).map(|p| p.len())
    })
}

/// Wait until `at_least` of the process's readers wait on a lock, reading
/// in `schema`.
fn readers_wait(url: &str, schema: &str, at_least: i64) {
    let readers = format!("lakelands readers {}", std::process::id());
    let mut watch = postgres::Client::connect(url, postgres::NoTls).unwrap();
    let asked = Instant::now();
    let waiting = "SELECT count(*) FROM pg_stat_activity \
                   WHERE application_name = $1 AND wait_event_type = 'Lock' \
                   AND query LIKE '%' || $2 || '%'";
    while watch
        .query_one(waiting, &[&readers, &schema])
        .unwrap()
        .get::<_, i64>(0)
        < at_least
    {
        assert!(
            asked.elapsed() < Duration::from_secs(10),
            "{at_least} reads wait on the lock"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// A get of `t` `a`, on a thread of its own, answered within 10 s.
fn get_within(
    engine: &Arc<dyn LakeEngine>,
) -> Result<lakelands_api::Result<bool>, mpsc::RecvTimeoutError> {
    let (answer, answered) = mpsc::channel();
    let engine = engine.clone();
    thread::spawn(move || {
        let view = engine.view(None);
        let got = view.and_then(|v| v.get(&TypeName::from("t"), &RecordKey::from("a")));
        let _ = answer.send(got.map(|r| r.is_some()));
    });
    answered.recv_timeout(Duration::from_secs(10))
}

#[test]
fn a_read_answers_while_another_holds_its_transaction_open() {
    let Some(url) = common::url_or_skip("the pg side-by-side reads test") else {
        return;
    };
    let (engine, schema) = lake_of_one(&url);
    let (release, holder) = hold_proposals(&url, &schema);
    let waiting = read_proposals(&engine);
    readers_wait(&url, &schema, 1);
    let got = get_within(&engine);
    release.send(()).unwrap();
    holder.join().unwrap();
    assert_eq!(
        waiting.join().unwrap().unwrap(),
        0,
        "the held read answers once let go"
    );
    assert!(
        matches!(got, Ok(Ok(true))),
        "a get answers while another read of the lake waits in its transaction: {got:?}"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// Twice as many reads of one lake as it has readers wait on a lock: they
/// hold [`LAKE_READERS`] of the shared readers, and a get of another lake
/// on the same server answers meanwhile.
#[test]
fn reads_of_one_lake_that_wait_leave_readers_to_another() {
    let Some(url) = common::url_or_skip("the pg reads-of-two-lakes test") else {
        return;
    };
    let ((stuck, schema), (other, beside)) = (lake_of_one(&url), lake_of_one(&url));
    let (release, holder) = hold_proposals(&url, &schema);
    let waiting: Vec<_> = (0..2 * LAKE_READERS)
        .map(|_| read_proposals(&stuck))
        .collect();
    readers_wait(&url, &schema, LAKE_READERS as i64);
    // Time for any read past its share to take a reader, were it let.
    thread::sleep(Duration::from_millis(500));
    let got = get_within(&other);
    release.send(()).unwrap();
    holder.join().unwrap();
    for read in waiting {
        assert_eq!(
            read.join().unwrap().unwrap(),
            0,
            "a held read answers once let go"
        );
    }
    assert!(
        matches!(got, Ok(Ok(true))),
        "a get of one lake answers while reads of another wait: {got:?}"
    );
    drop((stuck, other));
    common::drop_schema(&url, &schema);
    common::drop_schema(&url, &beside);
}

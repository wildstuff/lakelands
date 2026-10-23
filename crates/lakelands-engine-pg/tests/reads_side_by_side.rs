//! The reads of one lake run side by side (lakelands#1324). Another
//! session holds the lake's `proposals` locked, so a view's read of the
//! proposals waits inside its read transaction; a get of a record, which
//! reads no proposal, answers meanwhile on another reader. With one reader
//! a handle's get waited behind that read.

mod common;

use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::ProposalFilter;
use lakelands_engine_pg::PgOpener;

#[test]
fn a_read_answers_while_another_holds_its_transaction_open() {
    let Some(url) = common::url_or_skip("the pg side-by-side reads test") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "side", &schema);
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

    let (locked, release) = (mpsc::channel(), mpsc::channel::<()>());
    let holder = {
        let (url, schema, locked) = (url.clone(), schema.clone(), locked.0);
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
    let waiting = {
        let engine = engine.clone();
        thread::spawn(move || {
            let view = engine.view(None)?;
            view.proposals(&ProposalFilter::default()).map(|p| p.len())
        })
    };
    let readers = format!("lakelands readers {}", std::process::id());
    let mut watch = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    let asked = Instant::now();
    while watch
        .query_one(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE application_name = $1 AND wait_event_type = 'Lock'",
            &[&readers],
        )
        .unwrap()
        .get::<_, i64>(0)
        == 0
    {
        assert!(
            asked.elapsed() < Duration::from_secs(10),
            "the read waits on the lock"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let (answer, answered) = mpsc::channel();
    {
        let engine = engine.clone();
        thread::spawn(move || {
            let view = engine.view(None);
            let got = view.and_then(|v| v.get(&TypeName::from("t"), &RecordKey::from("a")));
            let _ = answer.send(got.map(|r| r.is_some()));
        });
    }
    let got = answered.recv_timeout(Duration::from_secs(10));
    release.0.send(()).unwrap();
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
    common::drop_schemas(&url);
}

//! The reads of one lake run side by side (lakelands#1324). One thread
//! scans a type again and again, each scan one read transaction, and
//! counts the scans it has begun and ended. A get made on another thread
//! while a scan is under way answers before the scan ends: while the scan
//! holds its transaction open. With one reader the get waited for the
//! scan, and ended after it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Instant;

use lakelands_api::Location;
use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode, Scan};
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_core::conformance::fixture::stored_record;
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

const GETS: usize = 20;

#[test]
fn a_read_answers_while_another_holds_its_transaction_open() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("side-by-side-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let location = Location::sqlite("side", &dir);
    let engine: Arc<dyn LakeEngine> = SqliteOpener
        .open(&location, OpenMode::Create)
        .unwrap()
        .into();
    let p = engine.projection();
    p.begin().unwrap();
    for i in 0..5_000 {
        p.put_record(&stored_record(
            "t",
            format!("k{i:05}"),
            serde_json::from_value(json!({"text": "words ".repeat(20)})).unwrap(),
            1,
        ))
        .unwrap();
    }
    p.commit(Position(1)).unwrap();

    // A get begins a quarter of a scan's time into one, when the scan
    // holds its transaction open: of the shorter of two scans.
    let all = Scan {
        limit: u32::MAX,
        ..Scan::default()
    };
    let into = (0..2)
        .map(|_| {
            let scanned = Instant::now();
            let view = engine.view(None).unwrap();
            assert_eq!(view.scan(&TypeName::from("t"), &all).unwrap().len(), 5_000);
            scanned.elapsed() / 4
        })
        .min()
        .unwrap();
    // Odd while a scan is under way.
    let (scans, done) = (
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicBool::new(false)),
    );
    let scanning = {
        let (engine, scans, done) = (engine.clone(), scans.clone(), done.clone());
        thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let view = engine.view(None).unwrap();
                scans.fetch_add(1, Ordering::SeqCst);
                let scanned = view.scan(&TypeName::from("t"), &all).unwrap();
                scans.fetch_add(1, Ordering::SeqCst);
                assert_eq!(scanned.len(), 5_000);
            }
        })
    };
    let (mut inside, mut last) = (0, 0);
    for i in 0..GETS {
        // A scan begun after the last get's.
        let began = loop {
            match scans.load(Ordering::SeqCst) {
                under_way if under_way % 2 == 1 && under_way > last => break under_way,
                _ => {
                    assert!(!scanning.is_finished(), "the scans stopped");
                    thread::yield_now();
                }
            }
        };
        last = began;
        thread::sleep(into);
        let view = engine.view(None).unwrap();
        let key = RecordKey::from(format!("k{:05}", i * 100));
        assert!(view.get(&TypeName::from("t"), &key).unwrap().is_some());
        inside += usize::from(scans.load(Ordering::SeqCst) == began);
    }
    done.store(true, Ordering::SeqCst);
    scanning.join().unwrap();
    assert!(
        inside >= GETS * 3 / 4,
        "{inside} of {GETS} gets began and ended inside one scan"
    );
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}

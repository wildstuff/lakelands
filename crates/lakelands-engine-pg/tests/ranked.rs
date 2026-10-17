//! `ranked` on PostgreSQL reads its position and every list from one
//! state (`docs/seams.md` §`ranked`, lakelands#826).
//!
//! Its statements run one after another, and a fold or a rebuild may
//! commit between any two. Two tests hold one call between two of its
//! statements: another session rebuilds an index of `full_text` inside a
//! transaction it keeps open. That takes the index alone exclusively, so
//! the call's locks of its tables and its read of the folded position
//! pass, and its read of `full_text`, which must open the index, waits
//! until the session lets go. A fold that writes no text commits
//! meanwhile. The third holds the call before its first statement, behind
//! a `clear`.

mod common;

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode, Ranked, Rankings};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::Declaration;
use lakelands_engine_pg::PgOpener;
use serde_json::json;

fn declaration() -> Declaration {
    serde_json::from_value(json!({
        "declare": "type", "name": "t",
        "fields": [
            {"name": "body", "type": {"kind": "text"}, "searchable": true},
            {"name": "e", "type": {"kind": "vector", "dimensions": 2}}
        ]
    }))
    .unwrap()
}

/// The text and the vector the tests ask for.
fn asked() -> Rankings {
    Rankings {
        text: Some(("quay".into(), None)),
        vectors: vec![("e".into(), vec![1.0, 0.0])],
        ..Rankings::default()
    }
}

/// A lake folded through 1: the record `a` holds `quay` and (1, 0).
fn lake(url: &str, schema: &str) -> Box<dyn LakeEngine> {
    let engine = PgOpener
        .open(&common::location(url, "ranked", schema), OpenMode::Create)
        .unwrap();
    let (p, ty) = (engine.projection(), TypeName::from("t"));
    p.begin().unwrap();
    p.put_declaration(&declaration(), 1, None, Position(0))
        .unwrap();
    p.put_record(&StoredRecord {
        ty: ty.clone(),
        key: RecordKey::from("a"),
        version: Version(1),
        fields: serde_json::from_value(json!({"body": "quay", "e": [1.0, 0.0]})).unwrap(),
        meta: Default::default(),
        position: Position(1),
        made: Position(1),
        retired: false,
        restricted: false,
    })
    .unwrap();
    let index = engine.vectors().expect("the engine keeps vectors");
    index.put(&ty, &"a".into(), "e", &[1.0, 0.0]).unwrap();
    p.commit(Position(1)).unwrap();
    engine
}

/// Each list's keys.
fn keys(ranked: &Ranked) -> Vec<Vec<String>> {
    let keys = |l: &Vec<(RecordKey, f64)>| l.iter().map(|(k, _)| k.to_string()).collect();
    ranked.lists.iter().map(keys).collect()
}

/// One `ranked` call for a text and a vector, held between its read of
/// the folded position and its read of `full_text`; `inside` runs while
/// it is held, with the test's session and the schema. Answers what the
/// call answered.
fn held_inside_ranked(
    url: &str,
    schema: &str,
    engine: &dyn LakeEngine,
    inside: impl FnOnce(&mut postgres::Client),
) -> Ranked {
    let mut side = postgres::Client::connect(url, postgres::NoTls).unwrap();
    side.batch_execute(&format!("BEGIN; REINDEX INDEX \"{schema}\".full_text_key"))
        .unwrap();
    std::thread::scope(|s| {
        let call = s.spawn(|| engine.ranked(&TypeName::from("t"), &asked(), 10));
        // Until the call waits for the index.
        let waiting = format!(
            "SELECT count(*) FROM pg_locks WHERE NOT granted \
             AND relation = '\"{schema}\".full_text_key'::regclass"
        );
        let asked_at = std::time::Instant::now();
        while side.query_one(&waiting, &[]).unwrap().get::<_, i64>(0) == 0 {
            assert!(asked_at.elapsed().as_secs() < 20, "the call never waited");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        inside(&mut side);
        side.batch_execute("ROLLBACK").unwrap();
        call.join().unwrap().expect("the call answers")
    })
}

/// A fold that commits between two statements of one call is in none of
/// its lists: the position is the one before the fold, and the vector's
/// list, read after the commit, does not hold the fold's vector. The call
/// after it holds both.
#[test]
fn a_fold_committed_inside_one_call_is_in_none_of_its_lists() {
    let Some(url) = common::url_or_skip("the pg ranked test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = lake(&url, &schema);
    let ranked = held_inside_ranked(&url, &schema, engine.as_ref(), |_| {
        let p = engine.projection();
        p.begin().unwrap();
        let index = engine.vectors().unwrap();
        index
            .put(&"t".into(), &"b".into(), "e", &[1.0, 0.0])
            .unwrap();
        p.commit(Position(2)).unwrap();
    });
    assert_eq!(
        (ranked.position, keys(&ranked)),
        (Some(Position(1)), vec![vec!["a".to_owned()]; 2]),
        "the call the fold committed inside"
    );
    let after = engine.ranked(&"t".into(), &asked(), 10).unwrap();
    assert_eq!(after.position, Some(Position(2)));
    assert_eq!(keys(&after), [vec!["a"], vec!["a", "b"]]);
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// The call holds every table it reads in `ACCESS SHARE` mode: at its
/// second statement it holds the tables of the statements still to come,
/// so a rebuild's `clear`, which drops them all, waits for the call and
/// cannot empty a table under the position read.
#[test]
fn a_call_holds_every_table_it_reads_from_its_first_statement() {
    let Some(url) = common::url_or_skip("the pg ranked test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = lake(&url, &schema);
    let mut held = Vec::new();
    held_inside_ranked(&url, &schema, engine.as_ref(), |side| {
        let rows = side
            .query(
                "SELECT c.relname FROM pg_locks l JOIN pg_class c ON c.oid = l.relation \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = $1 AND c.relkind = 'r' AND l.granted \
                 AND l.mode = 'AccessShareLock' AND l.pid <> pg_backend_pid() ORDER BY 1",
                &[&schema],
            )
            .unwrap();
        held = rows.iter().map(|r| r.get::<_, String>(0)).collect();
    });
    let read = [
        "declarations",
        "declarations_head",
        "full_text",
        "meta",
        "records",
        "records_head",
        "vectors",
    ];
    assert_eq!(held, read, "the tables the waiting call holds");
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// The locks come before the snapshot. Another session holds
/// `declarations` exclusively; a rebuild's `clear` queues for it, and
/// then the call does, behind the `clear`. Let go, the `clear` lands
/// first, and the call, whose snapshot is taken only once it holds its
/// tables, answers the cleared projection: no position, and empty lists.
/// A call that read the folded position first would answer that position
/// over tables made again and empty.
#[test]
fn a_clear_queued_ahead_of_a_call_is_in_its_position() {
    let Some(url) = common::url_or_skip("the pg ranked test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = lake(&url, &schema);
    let mut side = postgres::Client::connect(url.as_str(), postgres::NoTls).unwrap();
    side.batch_execute(&format!(
        "BEGIN; LOCK TABLE \"{schema}\".declarations IN ACCESS EXCLUSIVE MODE"
    ))
    .unwrap();
    let waiting = format!(
        "SELECT count(*) FROM pg_locks WHERE NOT granted \
         AND relation = '\"{schema}\".declarations'::regclass"
    );
    let ranked = std::thread::scope(|s| {
        let mut until = |n: i64| {
            let asked_at = std::time::Instant::now();
            while side.query_one(&waiting, &[]).unwrap().get::<_, i64>(0) < n {
                assert!(asked_at.elapsed().as_secs() < 20, "never {n} waiting");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        let clear = s.spawn(|| engine.projection().clear());
        until(1);
        let call = s.spawn(|| engine.ranked(&TypeName::from("t"), &asked(), 10));
        until(2);
        side.batch_execute("ROLLBACK").unwrap();
        clear.join().unwrap().expect("the clear");
        call.join().unwrap().expect("the call answers")
    });
    assert_eq!(
        (ranked.position, keys(&ranked)),
        (None, vec![Vec::<String>::new(); 2]),
        "the call the clear landed ahead of"
    );
    drop(engine);
    common::drop_schema(&url, &schema);
}

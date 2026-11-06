//! A held lake whose tip is an ingest of `crm` whose `sealed` names
//! `web`, as no writer writes one and an import now refuses: its line
//! and its index entry rewritten in place, its hash made good, and the
//! projection one layout back, as a release before this one left it.
//! The writer's open rebuilds from the log and refuses the lake as
//! damaged, naming the event (`docs/log-format.md` §1.1).

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::error::{Flaw, Situation};
use lakelands_api::seam::Seams;
use lakelands_api::{Location, SourceId};
use lakelands_core::conformance::fixture::{Lake, log_events, source_record, sourced, type_decl};
use lakelands_engine_sqlite::SqliteOpener;

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_writers_open_refuses_a_held_ingest_sealed_under_another_source() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("seals-another-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let at = Location::sqlite("held".to_owned(), dir.clone());
    let store = lakelands_store::open(at.clone(), seams(), OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    let text = sourced("text", lakelands_api::ontology::StorageKind::Text);
    l.declare(type_decl("note", vec![text])).await.unwrap();
    let a = source_record("note", "a", serde_json::json!({"text": "t"}));
    l.ingest("crm", vec![a]).await.unwrap();
    let log = log_events(store.as_ref()).await.unwrap();
    store.close().await.unwrap();
    drop(store);
    let tip = log.last().unwrap();
    let mut v = serde_json::to_value(tip).unwrap();
    v["sealed"] = serde_json::json!({"key": "1", "source": "web"});
    v.as_object_mut().unwrap().remove("hash");
    let hash = lakelands_api::canonical::sha256_hex(&v);
    let sealed = r#""sealed":{"key":"1","source":"web"},"source":"crm""#;
    let (old, p) = (tip.hash.0.clone(), tip.position.0);
    let last = |d: &str| {
        let mut files: Vec<_> = std::fs::read_dir(dir.join(d))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        files.pop().unwrap()
    };
    // The line grows by the seal; its index entry's length and hash follow.
    let path = last("log");
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text.matches(r#""source":"crm""#).count(),
        1,
        "one ingest of crm"
    );
    let text = text
        .replace(r#""source":"crm""#, sealed)
        .replace(&old, &hash);
    std::fs::write(&path, text).unwrap();
    let path = last("index");
    let text = std::fs::read_to_string(&path).unwrap();
    let entry = |line: &str| {
        let mut e: serde_json::Value = serde_json::from_str(line).unwrap();
        // An event line ends with its hash, whether it names `prev` or not.
        let last = e.as_array().unwrap().len() - 1;
        if e[last] == old.as_str() {
            e[4] = (e[4].as_u64().unwrap() + sealed.len() as u64 - 14).into();
            e[last] = hash.as_str().into();
        }
        e.to_string() + "\n"
    };
    std::fs::write(&path, text.lines().map(entry).collect::<String>()).unwrap();
    let db = rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    db.execute("UPDATE meta SET v = v - 1 WHERE k = 'layout'", [])
        .unwrap();
    drop(db);
    let refused = lakelands_store::open(at, seams(), OpenMode::Write).await;
    let _ = std::fs::remove_dir_all(&dir);
    let web = SourceId::from("web");
    let refused = refused.err().unwrap();
    assert!(
        matches!(refused.situation(), Some(Situation::Damaged {
            position: Some(at), flaw: Flaw::SealsAnother { sealed }, ..
        }) if at.0 == p && *sealed == web),
        "{refused:?}"
    );
}

//! A falling fence written below the store, as no engine's append or
//! import lets one in: the lake's tip carried under the fence of the
//! writer before, its line and its index entry rewritten in place, its
//! hash made good. `verify` refuses it at its position
//! (`docs/log-format.md` §4).

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::error::{Flaw, Situation};
use lakelands_api::seam::Seams;
use lakelands_api::{Error, Location};
use lakelands_core::conformance::fixture::{Lake, field, log_events, type_decl};
use lakelands_engine_sqlite::SqliteOpener;

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_refuses_a_fence_that_falls_below_the_engine() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("fence-falls-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let at = Location::sqlite("falls".to_owned(), dir.clone());
    let made = lakelands_store::open(at.clone(), seams(), OpenMode::Create)
        .await
        .unwrap();
    made.close().await.unwrap();
    drop(made);
    // A second writer, at fence 2, writes one record: the tip.
    let store = lakelands_store::open(at.clone(), seams(), OpenMode::Write)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl(
        "note",
        vec![field("text", lakelands_api::ontology::StorageKind::Text)],
    ))
    .await
    .unwrap();
    l.upsert("note", "n1", serde_json::json!({"text": "t"}))
        .await
        .unwrap();
    let log = log_events(store.as_ref()).await.unwrap();
    store.close().await.unwrap();
    drop(store);
    let tip = log.last().unwrap();
    assert!(
        tip.fence >= 2,
        "the tip is under the second writer's fence: {}",
        tip.fence
    );
    let (high, low) = (tip.fence, tip.fence - 1);
    // Its line, under the fence before, its hash made good.
    let mut v: serde_json::Value = serde_json::to_value(tip).unwrap();
    v["fence"] = serde_json::json!(low);
    v.as_object_mut().unwrap().remove("hash");
    let hash = lakelands_api::canonical::sha256_hex(&v);
    let replace = |path: PathBuf, from: &[(String, String)]| {
        let mut text = std::fs::read_to_string(&path).unwrap();
        for (a, b) in from {
            assert_eq!(
                text.matches(a.as_str()).count(),
                1,
                "{a} once in {}",
                path.display()
            );
            text = text.replace(a.as_str(), b.as_str());
        }
        std::fs::write(&path, text).unwrap();
    };
    let old = tip.hash.0.clone();
    let only = |d: &str| {
        let mut files: Vec<_> = std::fs::read_dir(dir.join(d))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        files.pop().unwrap()
    };
    let p = tip.position.0;
    replace(
        only("log"),
        &[(
            format!("\"fence\":{high},\"hash\":\"{old}\""),
            format!("\"fence\":{low},\"hash\":\"{hash}\""),
        )],
    );
    replace(
        only("index"),
        &[
            (format!("[\"d\",{p},{high},"), format!("[\"d\",{p},{low},")),
            (format!("\"{old}\"]"), format!("\"{hash}\"]")),
        ],
    );
    let store = lakelands_store::open(at, seams(), OpenMode::Read)
        .await
        .unwrap();
    let verified = store.verify(None, None).await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        matches!(&verified, Err(Error::Unavailable { situation })
            if matches!(&**situation, Situation::Damaged {
                position: Some(at), flaw: Flaw::FenceBelow { fence, floor }, ..
            } if at.0 == p && *fence == low && *floor == high)),
        "{verified:?}"
    );
}

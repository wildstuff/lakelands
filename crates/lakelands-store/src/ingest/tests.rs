use std::sync::Arc;

use lakelands_api::engine::{About, Applied, OpenMode};
use lakelands_api::seam::Seams;
use lakelands_api::write::Declaration;
use lakelands_api::{
    Grants, LakeStore, Location, Patch, Position, PrincipalKind, RecordKey, TypeName,
};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

use super::*;
use crate::doors::Store;

/// A window judged first as if no id were applied is judged again where
/// one is, so each record is judged as its own ask would have it. Here the
/// projection holds the id of the window's first record on another event,
/// as a log another build wrote can (log-format §3), and no door makes:
/// the first record is a duplicate, and the second, on the same key of an
/// `insert_only` type, is written at the version the first left, which the
/// first judgement refused as `written_once`.
#[tokio::test]
async fn a_window_with_an_applied_id_is_judged_as_each_record_asks() {
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        ..Seams::default()
    };
    let lake = Lake::open(&Location::memory("asks"), seams, OpenMode::Create);
    let lake = Arc::new(lake.expect("the lake opens"));
    let cat = Store { lake: lake.clone() }.catalog(WriteContext::default());
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "asks".into(),
        grants: Grants::owner(),
    };
    let feed: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "feed", "insert_only": true,
        "fields": [{"name": "body", "type": {"kind": "text"}, "authority": "source"}]
    }))
    .expect("a declaration");
    cat.declare(&owner, feed).await.expect("declaring");
    let (source, ty, key) = (
        SourceId::from("dev"),
        TypeName::from("feed"),
        RecordKey::from("k"),
    );
    let body = |b: &str| -> Patch { serde_json::from_value(json!({"body": b})).expect("a patch") };
    let id = log::ingest_id(&source, &ty, &key, 0, &body("a"));
    let (p, head) = (lake.engine.projection(), lake.head());
    let about = vec![About {
        ty: Some(ty.clone()),
        key: Some(key.clone()),
    }];
    let applied = Applied {
        id,
        at: head,
        kind: "record_ingested",
        about,
        source: None,
    };
    (p.begin().and_then(|()| p.mark_applied(&applied)))
        .and_then(|()| p.commit(head))
        .expect("marking the id applied");
    let record = |b: &str| SourceRecord {
        source: None,
        upstream: None,
        ty: ty.clone(),
        key: key.clone(),
        fields: body(b),
        seen_at: None,
        resubmits: None,
    };
    let fed = lakelands_api::ingest_all(
        cat.as_ref(),
        &owner,
        &source,
        vec![record("a"), record("b")],
    );
    let fed = fed.await.expect("the window is written");
    let counted: Vec<_> = fed.iter().map(|c| (c.records, c.duplicates)).collect();
    assert_eq!(
        counted,
        [(1, 1)],
        "one window: `a` a duplicate, `b` written"
    );
    let held = lake
        .engine
        .view(None)
        .and_then(|v| v.get(&ty, &key))
        .expect("reading `k`");
    let held = held.expect("`k` is written");
    assert_eq!(
        (held.fields.get("body"), held.version.0, lake.head()),
        (Some(&json!("b")), 1, Position(head.0 + 1)),
        "`b` is written at version 1 by one event"
    );
}

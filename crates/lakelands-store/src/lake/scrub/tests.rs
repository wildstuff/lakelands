//! What a scoped refold writes: a page's records and outputs alone, and
//! nothing of a kept view's order, whose own folds it keeps from the
//! engine (`docs/decisions/098-a-lake-stores-what-it-cannot-derive.md`,
//! lakelands#1690).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use lakelands_api::Location;
use lakelands_api::Version;
use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode, ProjectionStore};
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::order_name;
use serde_json::json;

use super::{Scope, Scoped};

/// A record of `ty` at `key` at `at`, with `fields`.
fn record(ty: &str, key: &str, fields: serde_json::Value, at: u64) -> StoredRecord {
    let fields = serde_json::from_value(fields).unwrap();
    StoredRecord {
        fields,
        version: Version(3),
        position: Position(at),
        retired: false,
        ..StoredRecord::ended(ty.into(), key.into(), Position(at))
    }
}

/// A scoped refold writes the page's records and outputs alone: an order
/// entry put or a name ended under a scope leaves the engine's order as
/// it was, so a kept view the page does not restate keeps its entries.
#[test]
fn a_scoped_refold_writes_no_order_entries() {
    let engine: Arc<dyn LakeEngine> = lakelands_engine_mem::MemoryOpener::new()
        .open(&Location::memory("scoped-order"), OpenMode::Create)
        .unwrap()
        .into();
    let (ty, key) = (TypeName::from("line"), RecordKey::from);
    let order = order_name(&TypeName::from("ends"));
    // The engine holds one entry before the scope.
    let p = engine.projection();
    p.begin().unwrap();
    p.put_order(&order, &key("a"), Some(&b"a1"[..])).unwrap();
    p.commit(Position(1)).unwrap();
    let scope = Scope {
        keys: BTreeSet::from([(ty.clone(), key("a"))]),
        types: BTreeSet::from([ty.clone()]),
        near: BTreeSet::new(),
        outputs: BTreeSet::new(),
        head: Position(2),
        at: std::sync::atomic::AtomicU64::new(0),
    };
    let scoped = Scoped {
        engine: engine.clone(),
        scope: Mutex::new(Some(Arc::new(scope))),
    };
    scoped.begin().unwrap();
    scoped
        .put_order(&order, &key("b"), Some(&b"b2"[..]))
        .unwrap();
    scoped.end_order(&order).unwrap();
    scoped
        .put_record(&record("line", "a", json!({"n": 1}), 2))
        .unwrap();
    scoped
        .put_record(&record("line", "z", json!({"n": 2}), 2))
        .unwrap();
    scoped.commit(Position(2)).unwrap();
    let v = engine.view(None).unwrap();
    let a = v.order(&order, &key("a")).unwrap();
    let b = v.order(&order, &key("b")).unwrap();
    let written = v.get(&ty, &key("a")).unwrap();
    let kept = v.get(&ty, &key("z")).unwrap();
    assert!(
        (a.as_deref(), b.as_deref()) == (Some(&b"a1"[..]), None),
        "a scope put b and ended the name: the order reads a {a:?} and b {b:?}, where the \
         scope leaves the entry a alone and writes none of b"
    );
    assert!(
        written.as_ref().is_some_and(|r| r.position == Position(2)),
        "the scope did not write its page's record `a`: {written:?}"
    );
    assert!(
        kept.is_none(),
        "the scope wrote `z`, a record its page does not restate: {kept:?}"
    );
}

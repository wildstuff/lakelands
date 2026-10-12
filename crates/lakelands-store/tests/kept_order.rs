//! A view's kept records are read in key order, a page going on from the
//! last key of the one before (lakelands#575). At an earlier position an
//! engine answers them in that order by its statement's plan alone, so
//! the store checks the order it is handed: a page out of key order is
//! refused, not read on from, and a count under a `where` goes by the
//! same pages. The memory engine behind the wrapper of
//! `probed` hands the kept records reversed.

mod probed;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use lakelands_api::engine::OpenMode;
use lakelands_api::read::{Find, Found};
use lakelands_api::seam::Seams;
use lakelands_api::{Error, Location};
use lakelands_core::conformance::fixture::{Lake, agg, patch};
use lakelands_engine_mem::MemoryOpener;
use probed::{Probe, ProbedOpener};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_name_handed_out_of_key_order_is_not_answered() {
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory("kept-order"), seams, OpenMode::Create)
        .await
        .expect("a new lake");
    let l = Lake::of(store.as_ref());
    let field = |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}});
    let line = [field("kind", "text"), field("net", "decimal")];
    let book = json!({"declare": "type", "name": "book", "key": ["kind"],
        "fields": [field("kind", "text"), field("total", "decimal")],
        "view": {"source": "line", "group_by": ["kind"],
                 "measures": [{"name": "total", "expr": "sum(net)"}]}});
    for d in [
        json!({"declare": "type", "name": "line", "fields": line}),
        book,
    ] {
        l.declare_json(d).await.expect("declaring");
    }
    let put = |ty: &str, key: &str, fields: serde_json::Value| {
        let (ty, key) = (ty.into(), key.into());
        let l = Lake::of(store.as_ref());
        async move {
            (l.cat)
                .upsert(&l.own, &ty, &key, patch(fields), Default::default())
                .await
                .expect("a write");
        }
    };
    put("line", "a", json!({"kind": "k", "net": "3"})).await;
    put("line", "b", json!({"kind": "j", "net": "4"})).await;
    let kept = json!({"key": "view_kept.book", "value": true});
    put("sys.setting", "view_kept.book", kept).await;
    let book = "book".into();
    let find = || l.cat.find(&l.own, &book, Find::default());
    let Found::Records { rows, .. } = find().await.expect("in key order").data else {
        panic!("records");
    };
    let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(keys, ["j", "k"], "the view's two groups, kept");
    probe.reversed.store(true, Ordering::SeqCst);
    let said = match find().await {
        Err(Error::Unavailable { message }) => message,
        other => panic!("a page out of key order must be refused: {other:?}"),
    };
    assert!(
        said.contains("`sys:kept.book` out of key order, `j` after `k`"),
        "{said}"
    );
    // A count under a `where` goes by the same pages, and is refused too.
    let counted = Find {
        filter: Some("total > 0".into()),
        aggregate: Some(lakelands_api::read::Aggregate {
            group_by: Vec::new(),
            measures: vec![agg("n", lakelands_api::read::AggFunc::Count, None)],
        }),
        ..Find::default()
    };
    let counted = l.cat.find(&l.own, &book, counted).await;
    assert!(
        matches!(&counted, Err(Error::Unavailable { message }) if message.contains("out of key order")),
        "a filtered count of a kept name out of key order must be refused: {counted:?}"
    );
}

//! A stack of views over views is read and described in time that grows
//! with its views, not with its ways down: each view is asked once a read
//! (lakelands#942).

use lakelands_api::describe::Kept;
use lakelands_api::read::{Find, Found};
use lakelands_api::seam::{SETTING_TYPE, VIEW_KEPT_SETTING};
use lakelands_api::{ReadAt, TypeName};
use serde_json::json;

use super::only::holder;
use crate::conformance::fixture::{Lake, Must};
use crate::conformance::{Held, Store};

check!(
    VIEW_STACK = a_stack_of_views_over_views_is_read_in_time_its_views_take,
    "026 and 051, extended (lakelands#942): twenty-two levels of two views, each reading the two below (the one as its source, the other merged, each under a `where` that drops a category), are read by `find` on the top view by the owner, by a reader without `restricted`, as of a position and by a holder of `only`, and the lake described to the owner and to a reader without `restricted`, and the top read again by both and the lake described again once the fold keeps every view of the stack, as `describe_type` says, each within five seconds and answering the top view's groups by value, the first level's `where` dropping one category and every higher level's another, where a walk that asked a view once for every way down to it would ask 2^23 times"
);

/// The levels of the stack.
const LEVELS: usize = 22;

/// The top's groups, `cat n m`: `k2` and `k1`, which the `where`s leave
/// out, in none; one group of the level below and of its merged view in
/// each left. A holder of `only` on the claims of `k0` reads `k0` alone.
const WANT: [&str; 2] = ["k0 1 1", "k3 1 1"];

/// The longest any one read may take.
const BOUND: std::time::Duration = std::time::Duration::from_secs(5);

async fn a_stack_of_views_over_views_is_read_in_time_its_views_take(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    let num = |n: &str| json!({"name": n, "type": {"kind": "number"}});
    for ty in ["claim", "other"] {
        let fields = json!([text("cat")]);
        l.declare_json(json!({"declare": "type", "name": ty, "fields": fields}))
            .await?;
    }
    for i in 0..20 {
        let cat = format!("k{}", i % 4);
        l.upsert("claim", &format!("c{i}"), json!({ "cat": cat }))
            .await?;
    }
    l.upsert("other", "o1", json!({"cat": "k0"})).await?;
    let (mut a, mut b) = ("claim".to_owned(), "other".to_owned());
    let mut views = Vec::new();
    for level in 1..=LEVELS {
        // The first level drops `k2`; every level over views drops `k1`.
        let drop = if level == 1 {
            "cat != 'k2'"
        } else {
            "cat != 'k1'"
        };
        let (na, nb) = (format!("a{level}"), format!("b{level}"));
        for (name, source, merged) in [(&na, &a, &b), (&nb, &b, &a)] {
            let count = |n: &str| json!([{"name": n, "expr": "count()"}]);
            let merge = json!([{"source": merged, "where": drop, "group_by": ["cat"], "measures": count("m")}]);
            let view = json!({"source": source, "where": drop, "group_by": ["cat"], "measures": count("n"),
                              "merge": merge});
            let fields = json!([text("cat"), num("n"), num("m")]);
            l.declare_json(json!({"declare": "type", "name": name, "key": ["cat"],
                                  "fields": fields, "view": view}))
                .await?;
        }
        views.extend([na.clone(), nb.clone()]);
        (a, b) = (na, nb);
    }
    let top = TypeName::from(a.as_str());
    let head = store.head().await.must("reading the head")?;
    let plain = holder(store.as_ref(), "plain", json!({"default": "read"}));
    let only = json!({"default": "read", "restricted": true,
                      "types": {"claim": {"only": {"cat": "k0"}}}});
    let only = holder(store.as_ref(), "only", only);
    let then = Find {
        at: ReadAt {
            as_of: Some(head),
            ..ReadAt::default()
        },
        ..Find::default()
    };
    for (what, who, find) in [
        ("the owner", &l.own, Find::default()),
        ("a reader without `restricted`", &plain, Find::default()),
        ("the owner as of a position", &l.own, then),
        ("a holder of `only`", &only, Find::default()),
    ] {
        let started = std::time::Instant::now();
        let found = l.cat.find(who, &top, find).await;
        let took = started.elapsed();
        let groups = groups(&found);
        let want = match what {
            "a holder of `only`" => &WANT[..1],
            _ => &WANT[..],
        };
        ensure!(
            took <= BOUND && groups == want,
            "the top of {LEVELS} levels of views over views is read by {what} within {BOUND:?}: \
             groups {groups:?} in {took:?}; {:?}",
            found.err()
        );
    }
    for (what, who) in [
        ("the owner", &l.own),
        ("a reader without `restricted`", &plain),
    ] {
        let started = std::time::Instant::now();
        let described = l.cat.describe(who, ReadAt::default()).await;
        let took = started.elapsed();
        ensure!(
            took <= BOUND && described.is_ok(),
            "a lake of {LEVELS} levels of views over views is described to {what} within \
             {BOUND:?}: {took:?}, {:?}",
            described.err()
        );
    }
    // Kept by the fold, every view of the stack, it is read alike.
    for view in &views {
        let key = format!("{VIEW_KEPT_SETTING}{view}");
        let setting = json!({"key": key, "value": true});
        l.upsert(SETTING_TYPE, &key, setting).await?;
    }
    let started = std::time::Instant::now();
    let described = l.cat.describe(&plain, ReadAt::default()).await;
    let took = started.elapsed();
    ensure!(
        took <= BOUND && described.is_ok(),
        "a lake of {LEVELS} levels of kept views is described to a reader without \
         `restricted` within {BOUND:?}: {took:?}, {:?}",
        described.err()
    );
    let kept = l.describe_type(&a).await?.lifecycle.kept;
    ensure!(
        kept == Some(Kept::Kept),
        "the fold keeps the top of the stack once every view is named; it says {kept:?}"
    );
    for (what, who) in [
        ("the owner", &l.own),
        ("a reader without `restricted`", &plain),
    ] {
        let started = std::time::Instant::now();
        let found = l.cat.find(who, &top, Find::default()).await;
        let took = started.elapsed();
        let groups = groups(&found);
        ensure!(
            took <= BOUND && groups == WANT,
            "the top of {LEVELS} levels of kept views is read by {what} within {BOUND:?}: \
             groups {groups:?} in {took:?}; {:?}",
            found.err()
        );
    }
    Ok(())
}

/// The groups a `find` answered, `cat n m`, sorted; none where it is refused.
fn groups(found: &lakelands_api::Result<lakelands_api::Reply<Found>>) -> Vec<String> {
    let Ok(Found::Records { rows, .. }) = found.as_ref().map(|r| &r.data) else {
        return Vec::new();
    };
    let cell = |r: &lakelands_api::Record, f: &str| {
        r.fields.get(f).map_or("-".to_owned(), |v| match v {
            serde_json::Value::String(s) => s.clone(),
            v => v.to_string(),
        })
    };
    let mut out: Vec<String> = (rows.iter())
        .map(|r| format!("{} {} {}", cell(r, "cat"), cell(r, "n"), cell(r, "m")))
        .collect();
    out.sort();
    out
}

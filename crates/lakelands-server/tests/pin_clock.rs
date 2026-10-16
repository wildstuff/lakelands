//! A pin of the head keeps the instant it was set at, and its reads read
//! that clock (052, lakelands#571): in a lake idle since its last write,
//! `now()` under a head pin is the pin's instant, not the `known_at` of
//! the last event, and every pinned read repeats it. A pin of a named
//! position, and a read naming its own `as_of`, read the `known_at` of the
//! event there. No caller names the clock: a `clock` argument is refused.

mod common;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::wire::Mcp;
use common::{BACKENDS, Backend};
use lakelands_api::write::Declaration;
use lakelands_api::{Find, Timestamp, WriteContext};
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread")]
async fn a_pin_of_the_head_reads_the_instant_it_was_set_at() {
    for backend in BACKENDS {
        pinned_on(backend).await;
    }
}

fn wall() -> Timestamp {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    Timestamp(now.as_micros() as i64)
}

/// The `read_at` of each bill a find through MCP answers, and its keys.
async fn found(mcp: &mut Mcp, args: Value) -> (Vec<Timestamp>, Vec<String>) {
    let result = mcp.call("bill_find", args).await;
    assert_ne!(result["isError"], true, "{result}");
    let rows = result["structuredContent"]["data"]["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let read = |r: &Value| r["fields"]["read_at"].as_str()?.parse().ok();
    let keys = |r: &Value| r["key"].as_str().map(String::from);
    (
        rows.iter().filter_map(read).collect(),
        rows.iter().filter_map(keys).collect(),
    )
}

async fn pinned_on(backend: Backend) {
    let store = common::empty(backend).await;
    let cat = store.catalog(WriteContext::default());
    let own = common::owner();
    let bill = json!({"declare": "type", "name": "bill", "fields": [
        {"name": "due_on", "type": {"kind": "date_time"}},
        {"name": "read_at", "type": {"kind": "date_time"}, "authority": "derived",
         "derived": "now()", "computed_at": "read"}]});
    let bill: Declaration = serde_json::from_value(bill).unwrap();
    cat.declare(&own, bill).await.unwrap();
    // `b1` falls due two seconds after its write; the lake then stays idle.
    let due = Timestamp(wall().0 + 2_000_000);
    let fields = [("due_on".into(), json!(due.to_string()))].into();
    let at = cat
        .upsert(
            &own,
            &"bill".into(),
            &"b1".into(),
            fields,
            Default::default(),
        )
        .await
        .unwrap()
        .position();
    let feed = store.feed().poll(at, &Default::default(), 1).await.unwrap();
    assert_eq!(feed.events[0].position, at);
    let known = feed.events[0].known_at;
    assert!(known < due, "{backend:?}: `b1` lands before it falls due");
    tokio::time::sleep(Duration::from_millis(2200)).await;

    let mut mcp = Mcp::open(store.clone(), own.clone()).await;
    let before = wall();
    let pinned = mcp.call("pin", json!({})).await;
    let set = wall();
    assert_eq!(
        pinned["structuredContent"]["pinned"],
        json!(at.0),
        "{pinned}"
    );
    let (clocks, _) = found(&mut mcp, json!({})).await;
    let [clock] = clocks[..] else {
        panic!("{backend:?}: one bill, one clock: {clocks:?}")
    };
    assert!(
        before <= clock && clock <= set,
        "{backend:?}: under a pin of the head, `now()` is the pin's instant, between \
         {before:?} and {set:?}, not the last write's {known:?}: it is {clock:?}"
    );
    let (_, due_now) = found(&mut mcp, json!({"where": "due_on < now()"})).await;
    assert_eq!(
        due_now,
        ["b1"],
        "{backend:?}: `b1` is due under the pin's clock"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (again, _) = found(&mut mcp, json!({})).await;
    assert_eq!(again, [clock], "{backend:?}: a pinned read repeats");

    // A read naming its own `as_of` reads the `known_at` of its event.
    let (own_at, due_then) = found(&mut mcp, json!({"as_of": at.0})).await;
    assert_eq!(own_at, [known], "{backend:?}: a read's own `as_of`");
    assert!(due_then.len() == 1, "{backend:?}: the bill is read");
    let (_, due_then) = found(&mut mcp, json!({"as_of": at.0, "where": "due_on < now()"})).await;
    assert!(due_then.is_empty(), "{backend:?}: not due as of its write");

    // A pin of a named position, the head too, reads the `known_at` there.
    mcp.call("pin", json!({"as_of": at.0})).await;
    let (named, _) = found(&mut mcp, json!({})).await;
    assert_eq!(named, [known], "{backend:?}: a pin of a named position");

    // No caller names the clock.
    let refused = mcp
        .call("bill_find", json!({"clock": "2500-01-01T00:00:00Z"}))
        .await;
    assert_eq!(refused["isError"], true, "{backend:?}: {refused}");
    let read = cat
        .find(&own, &"bill".into(), Find::default())
        .await
        .unwrap();
    assert_eq!(read.position, at, "{backend:?}: nothing was appended");
}

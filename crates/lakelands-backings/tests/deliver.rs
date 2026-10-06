//! The webhook deliverer against a memory lake and a fake endpoint on
//! loopback: what it sends, in which order, what it keeps, and how it
//! treats each kind of answer.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use lakelands_api::engine::OpenMode;
use lakelands_api::expr::Condition;
use lakelands_api::ontology::{StorageKind, TriggerDecl};
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::Declaration;
use lakelands_api::{LakeId, LakeStore, Location, Position};
use lakelands_backings::deliver::{DeliverConfig, deliverers, sign};
use lakelands_core::conformance::fixture::{declare, doors, field, owner, type_decl, upsert};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A fake endpoint: it answers each request with the next status of its
/// script (200 once the script is spent) and keeps every request.
struct Fake {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn bodies(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|r| r[r.find("\r\n\r\n").unwrap() + 4..].to_owned())
            .collect()
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/hooks/lakelands", self.port)
    }
}

async fn fake(script: &[u16]) -> Fake {
    fake_with_body(script, 0).await
}

/// As [`fake`], answering a body of `body` bytes.
async fn fake_with_body(script: &[u16], body: usize) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let script = Arc::new(Mutex::new(
        script.iter().copied().collect::<VecDeque<u16>>(),
    ));
    let kept = requests.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = s.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if got.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            kept.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&got).to_string());
            let status = script.lock().unwrap().pop_front().unwrap_or(200);
            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-length: {body}\r\nconnection: close\r\n\r\n{}",
                "x".repeat(body)
            );
            let _ = s.write_all(reply.as_bytes()).await;
        }
    });
    Fake { port, requests }
}

/// A directory of the test's own, standing for the root.
fn root() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "deliver-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("secrets")).unwrap();
    dir
}

/// A memory lake with `invoice { status }` and the triggers `overdue`
/// (status = 'overdue') and `paid` (status = 'paid').
async fn lake() -> Arc<dyn LakeStore> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!("deliver-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let store = lakelands_store::open(
        Location::memory(name),
        Seams {
            engines: vec![Arc::new(lakelands_engine_mem::MemoryOpener::new())],
            ..Seams::default()
        },
        OpenMode::Create,
    )
    .await
    .unwrap();
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl("invoice", vec![field("status", StorageKind::Text)]),
    )
    .await
    .unwrap();
    for (name, value) in [("overdue", "overdue"), ("paid", "paid")] {
        cat.declare(
            &own,
            Declaration::Trigger(TriggerDecl {
                max_depth: None,
                name: name.into(),
                on: "invoice".into(),
                when: Some(Condition::from(format!("status = '{value}'").as_str())),
                description: None,
            }),
        )
        .await
        .unwrap();
    }
    store
}

async fn set(store: &dyn LakeStore, key: &str, status: &str) {
    upsert(
        doors(store).as_ref(),
        &owner(store),
        "invoice",
        key,
        json!({ "status": status }),
    )
    .await
    .unwrap();
}

fn config(endpoint: Value) -> DeliverConfig {
    let mut e = json!({"name": "billing", "lakes": [], "start": "genesis"});
    e.as_object_mut()
        .unwrap()
        .extend(endpoint.as_object().unwrap().clone());
    serde_json::from_value(json!({ "endpoints": [e], "retry_first_secs": 1, "retry_max_secs": 2 }))
        .unwrap()
}

fn lake_id(store: &dyn LakeStore) -> LakeId {
    owner(store).lake
}

/// The HMAC-SHA256 of `body` under `key`, written out (RFC 2104), as a
/// receiver without the deliverer's library would compute it.
fn hmac_sha256(key: &[u8], body: &[u8]) -> String {
    let mut k = [0u8; 64];
    k[..key.len()].copy_from_slice(key);
    let pad = |b: u8| k.iter().map(|x| x ^ b).collect::<Vec<u8>>();
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(body)
        .finalize();
    let outer = Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize();
    outer.iter().map(|b| format!("{b:02x}")).collect()
}

fn header<'r>(request: &'r str, name: &str) -> Option<&'r str> {
    request
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{name}: ")))
}

fn journal(dir: &std::path::Path, lake: &LakeId) -> Vec<Value> {
    std::fs::read_to_string(
        dir.join("deliveries")
            .join(lake.as_str())
            .join("billing.ndjson"),
    )
    .unwrap_or_default()
    .lines()
    .map(|l| serde_json::from_str(l).unwrap())
    .collect()
}

#[tokio::test]
async fn fired_triggers_go_in_log_order_as_the_feed_holds_them_signed_over_the_raw_body() {
    let store = lake().await;
    for (k, s) in [("i1", "overdue"), ("i2", "open"), ("i3", "paid")] {
        set(store.as_ref(), k, s).await;
    }
    let f = fake(&[]).await;
    let dir = root();
    std::fs::write(dir.join("secrets/billing.key"), "s3cret\n").unwrap();
    let lake = lake_id(store.as_ref());
    let c = config(json!({
        "url": f.url(), "lakes": [lake.as_str()], "secret_file": "secrets/billing.key"
    }));
    let mut d = deliverers(&c, &dir, &lake).unwrap().pop().unwrap();
    assert!(!format!("{d:?}").contains("s3cret"));
    d.round(store.as_ref()).await.unwrap();

    let fired = store
        .feed()
        .poll(
            Position::GENESIS,
            &FeedFilter {
                kinds: vec!["trigger_fired".into(), "object_set_changed".into()],
                types: vec![],
                keys: Vec::new(),
            },
            100,
        )
        .await
        .unwrap()
        .events;
    assert_eq!(fired.len(), 2, "i1 overdue, i3 paid");
    let want: Vec<String> = fired
        .iter()
        .map(|e| String::from_utf8(lakelands_core::canonical::of(e)).unwrap())
        .collect();
    assert_eq!(f.bodies(), want, "the feed's bytes, in log order");
    for (request, event) in f.requests().iter().zip(&fired) {
        assert!(request.starts_with("POST /hooks/lakelands HTTP/1.1\r\n"));
        assert_eq!(header(request, "lakelands-lake"), Some(lake.as_str()));
        assert_eq!(
            header(request, "lakelands-position"),
            Some(event.position.0.to_string().as_str())
        );
        assert_eq!(
            header(request, "lakelands-event"),
            Some(event.id.0.as_str())
        );
        assert_eq!(header(request, "lakelands-attempt"), Some("1"));
        let body = &request[request.find("\r\n\r\n").unwrap() + 4..];
        let want = format!("sha256={}", hmac_sha256(b"s3cret", body.as_bytes()));
        assert_eq!(header(request, "lakelands-signature"), Some(want.as_str()));
        assert_eq!(sign(b"s3cret", body.as_bytes()), want);
    }
    let lines = journal(&dir, &lake);
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|l| l["outcome"] == "delivered"));
    assert!(
        !std::fs::read_to_string(
            dir.join("deliveries")
                .join(lake.as_str())
                .join("billing.ndjson")
        )
        .unwrap()
        .contains("s3cret")
    );
}

#[tokio::test]
async fn the_position_is_kept_and_a_restart_sends_only_what_came_after() {
    let store = lake().await;
    set(store.as_ref(), "i1", "overdue").await;
    let f = fake(&[]).await;
    let dir = root();
    let lake = lake_id(store.as_ref());
    let c = config(json!({"url": f.url(), "lakes": [lake.as_str()]}));
    let mut d = deliverers(&c, &dir, &lake).unwrap().pop().unwrap();
    d.round(store.as_ref()).await.unwrap();
    d.round(store.as_ref()).await.unwrap();
    assert_eq!(f.requests().len(), 1, "nothing twice within a run");
    drop(d);

    set(store.as_ref(), "i2", "paid").await;
    let mut d = deliverers(&c, &dir, &lake).unwrap().pop().unwrap();
    d.round(store.as_ref()).await.unwrap();
    let bodies = f.bodies();
    assert_eq!(bodies.len(), 2, "{bodies:?}");
    assert!(bodies[1].contains("\"paid\""), "{}", bodies[1]);
}

#[tokio::test]
async fn a_new_endpoint_starting_at_the_head_skips_the_past() {
    let store = lake().await;
    set(store.as_ref(), "i1", "overdue").await;
    let f = fake(&[]).await;
    let dir = root();
    let lake = lake_id(store.as_ref());
    let c = config(json!({"url": f.url(), "lakes": [lake.as_str()], "start": "head"}));
    let mut d = deliverers(&c, &dir, &lake).unwrap().pop().unwrap();
    d.round(store.as_ref()).await.unwrap();
    assert!(f.requests().is_empty());
    set(store.as_ref(), "i2", "paid").await;
    d.round(store.as_ref()).await.unwrap();
    assert_eq!(f.requests().len(), 1);
}

/// A poll examines at most `FEED_EXAMINED` events, so an endpoint whose
/// events are far apart reads on past a poll that answered none: the round
/// says more is waiting, and the next one delivers.
#[tokio::test]
async fn a_sparse_endpoint_reads_on_past_a_bounded_poll() {
    let store = lake().await;
    for i in 0..4_200 {
        set(store.as_ref(), &format!("o{i}"), "open").await;
    }
    set(store.as_ref(), "i1", "overdue").await;
    let f = fake(&[]).await;
    let dir = root();
    let lake = lake_id(store.as_ref());
    let c = config(json!({"url": f.url(), "lakes": [lake.as_str()]}));
    let mut d = deliverers(&c, &dir, &lake).unwrap().pop().unwrap();
    assert!(
        d.round(store.as_ref()).await.unwrap(),
        "a poll that stopped short of the head with no events wants another"
    );
    assert!(f.requests().is_empty());
    assert!(!d.round(store.as_ref()).await.unwrap());
    let bodies = f.bodies();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(bodies[0].contains("\"overdue\""), "{}", bodies[0]);
}

/// A sleep that returns at once and keeps what it was asked to wait.
fn recording() -> (lakelands_backings::deliver::Sleep, Arc<Mutex<Vec<u64>>>) {
    let waits = Arc::new(Mutex::new(Vec::new()));
    let kept = waits.clone();
    let sleep: lakelands_backings::deliver::Sleep = Arc::new(move |d: std::time::Duration| {
        kept.lock().unwrap().push(d.as_secs());
        Box::pin(async {})
    });
    (sleep, waits)
}

#[tokio::test]
async fn a_transient_failure_is_tried_again_with_a_wait_that_doubles_to_the_cap() {
    let store = lake().await;
    set(store.as_ref(), "i1", "overdue").await;
    let f = fake(&[503, 503, 503, 503, 503, 503]).await;
    let dir = root();
    let lake = lake_id(store.as_ref());
    let mut c = config(json!({"url": f.url(), "lakes": [lake.as_str()]}));
    c.retry_max_secs = 10;
    let (sleep, waits) = recording();
    let mut d = deliverers(&c, &dir, &lake)
        .unwrap()
        .pop()
        .unwrap()
        .with_sleep(sleep);
    d.round(store.as_ref()).await.unwrap();
    assert_eq!(*waits.lock().unwrap(), [1, 2, 4, 8, 10, 10]);
    let lines = journal(&dir, &lake);
    let attempts: Vec<Value> = lines.iter().map(|l| l["attempt"].clone()).collect();
    assert_eq!(attempts, [1, 2, 3, 4, 5, 6, 7].map(|n| json!(n)));
    let waited: Vec<Value> = lines.iter().map(|l| l["retry_in_secs"].clone()).collect();
    assert_eq!(
        waited,
        [
            json!(1),
            json!(2),
            json!(4),
            json!(8),
            json!(10),
            json!(10),
            Value::Null
        ]
    );
    assert_eq!(lines[6]["outcome"], "delivered");
    let attempt_headers: Vec<Option<String>> = f
        .requests()
        .iter()
        .map(|r| header(r, "lakelands-attempt").map(str::to_owned))
        .collect();
    assert_eq!(attempt_headers.last(), Some(&Some("7".into())));
}

#[tokio::test]
async fn a_misconfigured_endpoint_or_a_busy_one_is_tried_again_and_a_redirect_is_refused() {
    for (status, retried) in [
        (401, true),
        (403, true),
        (404, true),
        (408, true),
        (429, true),
        (500, true),
        (301, false),
        (307, false),
        (400, false),
        (413, false),
    ] {
        let store = lake().await;
        set(store.as_ref(), "i1", "overdue").await;
        let f = fake(&[status]).await;
        let dir = root();
        let lake = lake_id(store.as_ref());
        let c = config(json!({"url": f.url(), "lakes": [lake.as_str()]}));
        let (sleep, _) = recording();
        let mut d = deliverers(&c, &dir, &lake)
            .unwrap()
            .pop()
            .unwrap()
            .with_sleep(sleep);
        d.round(store.as_ref()).await.unwrap();
        let outcomes: Vec<Value> = journal(&dir, &lake)
            .iter()
            .map(|l| l["outcome"].clone())
            .collect();
        let want = if retried {
            vec![json!("retrying"), json!("delivered")]
        } else {
            vec![json!("refused")]
        };
        assert_eq!(outcomes, want, "{status}");
    }
}

#[tokio::test]
async fn a_2xx_is_delivered_whatever_its_body() {
    let store = lake().await;
    set(store.as_ref(), "i1", "overdue").await;
    let dir = root();
    let lake = lake_id(store.as_ref());
    let mut c = config(json!({"url": "http://127.0.0.1:1/", "lakes": [lake.as_str()]}));
    c.max_reply_bytes = 16;
    let f = fake_with_body(&[200], 17).await;
    c.endpoints[0].url = f.url();
    let mut d = deliverers(&c, &dir, &lake).unwrap().pop().unwrap();
    // Bounded: a deliverer that took the body for a failure would retry
    // this event forever. The bound only turns that into a failure, so it
    // is generous for a loaded machine (lakelands#158).
    tokio::time::timeout(std::time::Duration::from_secs(60), d.round(store.as_ref()))
        .await
        .expect("a 2xx with an oversized body is delivered, not retried")
        .unwrap();
    assert_eq!(f.requests().len(), 1);
    let outcomes: Vec<Value> = journal(&dir, &lake)
        .iter()
        .map(|l| l["outcome"].clone())
        .collect();
    assert_eq!(outcomes, [json!("delivered")]);
}

#[tokio::test]
async fn an_event_the_receiver_refuses_is_journaled_and_the_next_one_goes() {
    let store = lake().await;
    set(store.as_ref(), "i1", "overdue").await;
    set(store.as_ref(), "i2", "paid").await;
    let f = fake(&[422]).await;
    let dir = root();
    let lake = lake_id(store.as_ref());
    let c = config(json!({"url": f.url(), "lakes": [lake.as_str()]}));
    let mut d = deliverers(&c, &dir, &lake).unwrap().pop().unwrap();
    d.round(store.as_ref()).await.unwrap();
    assert_eq!(f.requests().len(), 2, "refused once, then the next event");
    let lines = journal(&dir, &lake);
    assert_eq!(
        (lines[0]["outcome"].clone(), lines[0]["status"].clone()),
        (json!("refused"), json!(422))
    );
    assert_eq!(lines[1]["outcome"], "delivered");
    // The position moved past the refused event: a restart sends nothing.
    drop(d);
    let mut d = deliverers(&c, &dir, &lake).unwrap().pop().unwrap();
    d.round(store.as_ref()).await.unwrap();
    assert_eq!(f.requests().len(), 2);
}

#[tokio::test]
async fn a_refused_event_mid_page_moves_the_position_past_it() {
    let store = lake().await;
    for k in ["i1", "i2", "i3"] {
        set(store.as_ref(), k, "overdue").await;
    }
    // The second of three is refused.
    let f = fake(&[200, 400, 200]).await;
    let dir = root();
    let lake = lake_id(store.as_ref());
    let c = config(json!({"url": f.url(), "lakes": [lake.as_str()]}));
    let mut d = deliverers(&c, &dir, &lake).unwrap().pop().unwrap();
    d.round(store.as_ref()).await.unwrap();
    let lines = journal(&dir, &lake);
    let refused = lines
        .iter()
        .find(|l| l["outcome"] == "refused")
        .expect("one refused");
    let kept: Value = serde_json::from_str(
        &std::fs::read_to_string(
            dir.join("deliveries")
                .join(lake.as_str())
                .join("billing.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        kept["next"].as_u64().unwrap() > refused["position"].as_u64().unwrap(),
        "{kept} after {refused}"
    );
    assert_eq!(f.requests().len(), 3);
}

#[tokio::test]
async fn an_endpoint_receives_only_the_triggers_it_names() {
    let store = lake().await;
    set(store.as_ref(), "i1", "overdue").await;
    set(store.as_ref(), "i2", "paid").await;
    let f = fake(&[]).await;
    let dir = root();
    let lake = lake_id(store.as_ref());
    let c = config(json!({"url": f.url(), "lakes": [lake.as_str()], "triggers": ["paid"]}));
    let mut d = deliverers(&c, &dir, &lake).unwrap().pop().unwrap();
    d.round(store.as_ref()).await.unwrap();
    let bodies = f.bodies();
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].contains("\"paid\""));
    // Another lake's configuration makes no deliverer for this one.
    let other = config(json!({"url": f.url(), "lakes": ["elsewhere"]}));
    assert!(deliverers(&other, &root(), &lake).unwrap().is_empty());
}

#[tokio::test]
async fn an_endpoint_receives_the_object_set_changes_it_names() {
    let store = lake().await;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    for (name, value) in [("late", "overdue"), ("settled", "paid")] {
        let set: lakelands_api::write::ObjectSetDecl = serde_json::from_value(json!({
            "name": name, "of": "invoice", "where": format!("status = '{value}'")
        }))
        .unwrap();
        cat.declare(&own, Declaration::ObjectSet(set))
            .await
            .unwrap();
    }
    set(store.as_ref(), "i1", "overdue").await;
    set(store.as_ref(), "i2", "paid").await;
    let f = fake(&[]).await;
    let dir = root();
    let lake = lake_id(store.as_ref());
    let c = config(json!({
        "url": f.url(), "lakes": [lake.as_str()], "kinds": ["object_set_changed"], "sets": ["late"]
    }));
    let mut d = deliverers(&c, &dir, &lake).unwrap().pop().unwrap();
    d.round(store.as_ref()).await.unwrap();
    let bodies = f.bodies();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(
        bodies[0].contains("\"object_set_changed\"") && bodies[0].contains("\"late\""),
        "{}",
        bodies[0]
    );
}

#[tokio::test]
async fn a_second_deliverer_of_the_same_endpoint_and_lake_is_refused() {
    let store = lake().await;
    let f = fake(&[]).await;
    let dir = root();
    let lake = lake_id(store.as_ref());
    let c = config(json!({"url": f.url(), "lakes": [lake.as_str()]}));
    let first = deliverers(&c, &dir, &lake).unwrap();
    let why = deliverers(&c, &dir, &lake).unwrap_err();
    assert!(why.contains("another process"), "{why}");
    drop(first);
    assert_eq!(deliverers(&c, &dir, &lake).unwrap().len(), 1);
}

#[test]
fn a_configuration_that_could_send_what_was_not_meant_is_refused() {
    let refuse = |endpoint: Value, why: &str| {
        let got = config(endpoint).check().unwrap_err();
        assert!(got.contains(why), "{why}: {got}");
        assert!(!got.contains("hunter2"), "{got}");
    };
    refuse(
        json!({"url": "https://user:hunter2@hooks.example/x", "lakes": ["l"]}),
        "user information",
    );
    refuse(
        json!({"url": "https://hooks.example/x", "lakes": ["l"], "kinds": ["record_upserted"]}),
        "not one a deliverer sends",
    );
    refuse(
        json!({"url": "ftp://hooks.example/x", "lakes": ["l"]}),
        "neither",
    );
    refuse(json!({"url": "https://hooks.example/x"}), "names no lake");
    refuse(
        json!({"url": "https://hooks.example/x", "lakes": ["l"], "name": "../up"}),
        "not letters",
    );
    let twice: DeliverConfig = serde_json::from_value(json!({"endpoints": [
        {"name": "a", "url": "https://h.example/", "lakes": ["l"]},
        {"name": "a", "url": "https://h.example/", "lakes": ["l"]}
    ]}))
    .unwrap();
    assert!(twice.check().unwrap_err().contains("twice"));
    let zero: DeliverConfig = serde_json::from_value(json!({"endpoints": [], "batch": 0})).unwrap();
    assert!(zero.check().unwrap_err().contains("`batch`"));
    let mut backwards = config(json!({"url": "https://h.example/", "lakes": ["l"]}));
    backwards.retry_first_secs = 20;
    backwards.retry_max_secs = 10;
    assert!(
        backwards
            .check()
            .unwrap_err()
            .contains("`retry_first_secs`")
    );
    let unknown: Result<DeliverConfig, _> =
        serde_json::from_value(json!({"endpoints": [], "retries": 3}));
    assert!(unknown.is_err());
}

#[tokio::test]
async fn a_secret_file_that_is_missing_or_empty_is_refused_at_start() {
    let store = lake().await;
    let dir = root();
    let lake = lake_id(store.as_ref());
    let c = config(json!({
        "url": "http://127.0.0.1:1/x", "lakes": [lake.as_str()], "secret_file": "secrets/none"
    }));
    assert!(
        deliverers(&c, &dir, &lake)
            .unwrap_err()
            .contains("cannot be read")
    );
    std::fs::write(dir.join("secrets/none"), "\n").unwrap();
    assert!(deliverers(&c, &dir, &lake).unwrap_err().contains("empty"));
}

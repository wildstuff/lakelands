//! The embedding client against a fake `/v1/embeddings` endpoint on
//! loopback: what it sends, how it reads the answer, and how it fails.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use lakelands_api::Error;
use lakelands_api::error::Case;
use lakelands_api::seam::EmbedBackend;
use lakelands_backings::{Embed, EmbedConfig};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// What the fake answers a request body with: a status line and a body,
/// or `None` to never answer.
type Answer = dyn Fn(&Value) -> Option<(&'static str, String)> + Send + Sync;

/// A fake endpoint; it keeps every request it read, headers and body.
struct Fake {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn bodies(&self) -> Vec<Value> {
        self.requests()
            .iter()
            .map(|r| serde_json::from_str(&r[r.find("\r\n\r\n").unwrap() + 4..]).unwrap())
            .collect()
    }

    fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/v1/embeddings", self.port)
    }
}

async fn fake(answer: Box<Answer>) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let kept = requests.clone();
    let answer: Arc<Answer> = answer.into();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let kept = kept.clone();
            let answer = answer.clone();
            tokio::spawn(async move {
                let mut got = Vec::new();
                let mut buf = [0u8; 4096];
                let body_at = loop {
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
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
                            break end + 4;
                        }
                    }
                };
                let text = String::from_utf8_lossy(&got).to_string();
                kept.lock().unwrap().push(text.clone());
                let body: Value = serde_json::from_str(&text[body_at..]).unwrap_or(Value::Null);
                match answer(&body) {
                    Some((status, reply)) => {
                        let reply = format!(
                            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                             content-length: {}\r\nconnection: close\r\n\r\n{reply}",
                            reply.len()
                        );
                        let _ = s.write_all(reply.as_bytes()).await;
                    }
                    None => tokio::time::sleep(std::time::Duration::from_secs(10)).await,
                }
            });
        }
    });
    Fake { port, requests }
}

/// The vector the fake makes of a text `t<n>`: `[n, 1]`.
fn vector_of(text: &str) -> Value {
    let n: f64 = text.trim_start_matches('t').parse().unwrap_or(0.0);
    json!([n, 1.0])
}

/// A well-behaved endpoint of two dimensions that answers its items in
/// reverse order.
fn reversed(body: &Value) -> Option<(&'static str, String)> {
    let data: Vec<Value> = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .rev()
        .map(|(i, t)| json!({"object": "embedding", "index": i, "embedding": vector_of(t.as_str().unwrap())}))
        .collect();
    Some((
        "200 OK",
        json!({"object": "list", "data": data, "model": body["model"]}).to_string(),
    ))
}

fn config(endpoint: String) -> EmbedConfig {
    serde_json::from_value(json!({
        "endpoint": endpoint,
        "model": "tiny",
        "dimensions": 2,
        "timeout_secs": 1,
        "max_reply_bytes": 4096,
    }))
    .unwrap()
}

/// A directory of the test's own, standing for the root.
fn root() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "embed-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("secrets")).unwrap();
    dir
}

fn client(config: EmbedConfig) -> Embed {
    Embed::new(config, &root()).unwrap()
}

fn texts(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("t{i}")).collect()
}

fn failure(r: lakelands_api::Result<Vec<Vec<f32>>>) -> String {
    match r {
        Err(Error::Unavailable { situation }) => situation.to_string(),
        other => panic!("expected the embedding to fail, got {other:?}"),
    }
}

#[tokio::test]
async fn texts_go_in_batches_and_come_back_placed_by_index() {
    let f = fake(Box::new(reversed)).await;
    let mut c = config(f.endpoint());
    c.max_batch = 2;
    let e = client(c);
    assert_eq!((e.model(), e.dimensions()), ("tiny", 2));
    let got = e.embed(&texts(5)).await.unwrap();
    let want: Vec<Vec<f32>> = (0..5).map(|n| vec![n as f32, 1.0]).collect();
    assert_eq!(got, want);
    let bodies = f.bodies();
    assert_eq!(
        bodies,
        [
            json!({"model": "tiny", "input": ["t0", "t1"]}),
            json!({"model": "tiny", "input": ["t2", "t3"]}),
            json!({"model": "tiny", "input": ["t4"]}),
        ]
    );
    let request = &f.requests()[0];
    assert!(
        request.starts_with("POST /v1/embeddings HTTP/1.1\r\n"),
        "{request}"
    );
    assert!(
        !request.to_ascii_lowercase().contains("authorization"),
        "no key, no header: {request}"
    );
}

#[tokio::test]
async fn no_text_makes_no_request() {
    let f = fake(Box::new(reversed)).await;
    let got = client(config(f.endpoint())).embed(&[]).await.unwrap();
    assert!(got.is_empty());
    assert!(f.requests().is_empty(), "{:?}", f.requests());
}

#[tokio::test]
async fn the_key_file_is_read_from_the_root_and_sent_as_a_bearer_token() {
    let f = fake(Box::new(reversed)).await;
    let dir = root();
    std::fs::write(dir.join("secrets/embed.key"), "sk-test-123\n").unwrap();
    let mut c = config(f.endpoint());
    c.api_key_file = Some("secrets/embed.key".into());
    c.send_dimensions = true;
    let e = Embed::new(c, &dir).unwrap();
    assert!(!format!("{e:?}").contains("sk-test"), "{e:?}");
    e.embed(&texts(1)).await.unwrap();
    let request = &f.requests()[0];
    assert!(
        request.contains("authorization: Bearer sk-test-123\r\n"),
        "{request}"
    );
    assert_eq!(
        f.bodies()[0],
        json!({"model": "tiny", "input": ["t0"], "dimensions": 2})
    );
}

#[test]
fn a_key_file_that_is_missing_or_empty_or_an_endpoint_that_is_no_url_is_refused() {
    let dir = root();
    let mut c = config("http://127.0.0.1:1/v1/embeddings".into());
    c.api_key_file = Some("secrets/missing.key".into());
    let why = Embed::new(c.clone(), &dir).unwrap_err();
    assert!(why.contains("cannot be read"), "{why}");
    assert!(
        why.contains(&dir.join("secrets/missing.key").display().to_string()),
        "{why}"
    );
    std::fs::write(dir.join("secrets/empty.key"), " \n").unwrap();
    c.api_key_file = Some("secrets/empty.key".into());
    let why = Embed::new(c, &dir).unwrap_err();
    assert!(why.contains("is empty"), "{why}");
    let why = Embed::new(config("ftp://example.org/".into()), &dir).unwrap_err();
    assert!(
        why.contains("neither an `http` nor an `https` URL"),
        "{why}"
    );
    for zero in ["dimensions", "max_batch", "timeout_secs", "max_reply_bytes"] {
        let mut c = serde_json::to_value(config("http://127.0.0.1:1/".into())).unwrap();
        c[zero] = json!(0);
        let why = Embed::new(serde_json::from_value(c).unwrap(), &dir).unwrap_err();
        assert!(why.contains(&format!("`{zero}` is zero")), "{why}");
    }
    let why = Embed::new(config("http://user:sk-secret@127.0.0.1:1/".into()), &dir).unwrap_err();
    assert!(why.contains("user information"), "{why}");
    assert!(!why.contains("sk-secret"), "{why}");
    let unknown: Result<EmbedConfig, _> = serde_json::from_value(json!({
        "endpoint": "http://127.0.0.1:1/", "model": "m", "dimensions": 2, "api_key": "x"
    }));
    assert!(unknown.is_err(), "an unknown key is refused");
}

#[test]
fn an_https_endpoint_is_refused_at_open_when_tls_cannot_be_set_up_for_it() {
    let dir = root();
    let mut c = config("https://127.0.0.1:1/v1/embeddings".into());
    c.ca_file = Some("secrets/missing.pem".into());
    let why = Embed::new(c.clone(), &dir).unwrap_err();
    assert!(why.contains("certificate bundle"), "{why}");
    assert!(
        why.contains(&dir.join("secrets/missing.pem").display().to_string()),
        "the bundle is the root's: {why}"
    );
    std::fs::write(dir.join("secrets/empty.pem"), "no certificate here\n").unwrap();
    c.ca_file = Some("secrets/empty.pem".into());
    let why = Embed::new(c, &dir).unwrap_err();
    assert!(why.contains("holds no certificate"), "{why}");
    let why = Embed::new(config("https://-bad-.example/v1/embeddings".into()), &dir).unwrap_err();
    assert!(why.contains("not a name TLS can verify"), "{why}");
    // Plain `http` needs no bundle.
    let mut c = config("http://127.0.0.1:1/v1/embeddings".into());
    c.ca_file = Some("secrets/missing.pem".into());
    assert!(Embed::new(c, &dir).is_ok());
}

/// Each way an endpoint can answer wrongly, and what the failure says.
#[tokio::test]
async fn every_wrong_answer_is_unavailable_naming_the_model_and_the_kind_only() {
    type Case = (&'static str, Box<Answer>);
    let secret = "secret-query";
    let cases: Vec<Case> = vec![
        (
            "answered 500",
            Box::new(|_| {
                Some((
                    "500 Internal Server Error",
                    "{\"error\": \"secret-query\"}".into(),
                ))
            }),
        ),
        ("redirect", Box::new(|_| Some(("302 Found", String::new())))),
        (
            "401",
            Box::new(|_| Some(("401 Unauthorized", String::new()))),
        ),
        ("404", Box::new(|_| Some(("404 Not Found", String::new())))),
        (
            "429",
            Box::new(|_| Some(("429 Too Many Requests", String::new()))),
        ),
        ("within 1 seconds", Box::new(|_| None)),
        (
            "not the contract's",
            Box::new(|_| Some(("200 OK", "{\"echo\": \"secret-query\"}".into()))),
        ),
        (
            "2 vectors for 1 texts",
            Box::new(|_| {
                Some((
                    "200 OK",
                    json!({"data": [
                        {"index": 0, "embedding": [1.0, 0.0]},
                        {"index": 1, "embedding": [1.0, 0.0]},
                    ]})
                    .to_string(),
                ))
            }),
        ),
        (
            "index 3",
            Box::new(|_| {
                Some((
                    "200 OK",
                    json!({"data": [{"index": 3, "embedding": [1.0, 0.0]}]}).to_string(),
                ))
            }),
        ),
        (
            "3 dimensions, not 2",
            Box::new(|_| {
                Some((
                    "200 OK",
                    json!({"data": [{"index": 0, "embedding": [1.0, 0.0, 0.0]}]}).to_string(),
                ))
            }),
        ),
        (
            "not finite",
            Box::new(|_| {
                Some((
                    "200 OK",
                    "{\"data\": [{\"index\": 0, \"embedding\": [1e40, 0.0]}]}".into(),
                ))
            }),
        ),
        (
            "at most 4096 bytes",
            Box::new(|_| Some(("200 OK", format!("{{\"pad\": \"{}\"}}", "x".repeat(5000))))),
        ),
    ];
    for (kind, answer) in cases {
        let f = fake(answer).await;
        let e = client(config(f.endpoint()));
        let started = std::time::Instant::now();
        let why = failure(e.embed(&[secret.to_owned()]).await);
        assert!(why.contains(kind), "{kind}: {why}");
        assert!(
            why.contains("the embedding backend `tiny`"),
            "{kind}: {why}"
        );
        assert!(!why.contains(&f.port.to_string()), "{kind}: {why}");
        assert!(!why.contains(secret), "{kind}: {why}");
        assert!(started.elapsed().as_secs() < 4, "{kind}: the timeout holds");
    }
}

/// A request refused for what it carries is `Invalid`, so the store asks
/// again in halves; it says no more than any other failure.
#[tokio::test]
async fn a_request_refused_for_what_it_carries_is_invalid() {
    let secret = "secret-query";
    for status in [
        "400 Bad Request",
        "413 Payload Too Large",
        "422 Unprocessable Entity",
    ] {
        let f = fake(Box::new(move |_| {
            Some((status, "{\"error\": \"secret-query\"}".into()))
        }))
        .await;
        match client(config(f.endpoint()))
            .embed(&[secret.to_owned()])
            .await
        {
            Err(e @ Error::Refused { .. }) => {
                let model = "tiny".to_owned();
                let status = status[..3].parse().ok();
                let case = Case::EmbeddingRefused { model, status };
                assert_eq!(e.case().map(|said| &said.case), Some(&case), "{e}");
                assert!(!e.to_string().contains(secret), "{e}");
            }
            other => panic!("{status}: expected `Invalid`, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn an_index_answered_twice_is_refused() {
    let f = fake(Box::new(|_| {
        Some((
            "200 OK",
            json!({"data": [
                {"index": 0, "embedding": [1.0, 0.0]},
                {"index": 0, "embedding": [0.0, 1.0]},
            ]})
            .to_string(),
        ))
    }))
    .await;
    let why = failure(client(config(f.endpoint())).embed(&texts(2)).await);
    assert!(why.contains("index 0 twice"), "{why}");
}

#[tokio::test]
async fn an_endpoint_nobody_listens_on_is_unavailable() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let e = client(config(format!("http://127.0.0.1:{port}/v1/embeddings")));
    let why = failure(e.embed(&texts(1)).await);
    assert!(why.contains("cannot be connected to"), "{why}");
}

/// A memory lake with the client for `endpoint` bound, holding two
/// documents: `a` matches the text `quayside` and points north, `b`
/// matches nothing and points east.
async fn lake_with(endpoint: String) -> Arc<dyn lakelands_api::LakeStore> {
    use lakelands_api::engine::OpenMode;
    use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind};
    use lakelands_api::seam::Seams;
    use lakelands_api::write::Declaration;
    use lakelands_api::{Location, WriteContext};
    use lakelands_core::conformance::fixture::{field, owner, patch, type_decl};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let lake = format!("embed-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let store = lakelands_store::open(
        Location::memory(lake),
        Seams {
            engines: vec![Arc::new(lakelands_engine_mem::MemoryOpener::new())],
            embed: Some(Arc::new(client(config(endpoint)))),
            ..Seams::default()
        },
        OpenMode::Create,
    )
    .await
    .unwrap();
    let cat = store.catalog(WriteContext::default());
    let own = owner(store.as_ref());
    let body = FieldDecl {
        searchable: true,
        ..field("body", StorageKind::Text)
    };
    let v = FieldDecl {
        ty: FieldType {
            dimensions: Some(2),
            ..FieldType::kind(StorageKind::Vector)
        },
        ..field("v", StorageKind::Text)
    };
    // A hybrid search without a vector reads a field a model fills
    // (lakelands#743): `e`, the embedding of `body` by `tiny`.
    let e = lakelands_core::conformance::fixture::embedding("e", 2, &["body"], "tiny");
    cat.declare(&own, Declaration::Type(type_decl("doc", vec![body, v, e])))
        .await
        .unwrap();
    for (key, body, v) in [("a", "quayside", [0.0, 1.0]), ("b", "harbour", [1.0, 0.0])] {
        cat.upsert(
            &own,
            &"doc".into(),
            &key.into(),
            patch(json!({"body": body, "v": v})),
            Default::default(),
        )
        .await
        .unwrap();
    }
    store
}

fn search(mode: lakelands_api::query::SearchMode) -> lakelands_api::query::Search {
    lakelands_api::query::Search {
        ty: "doc".into(),
        mode,
        k: 5,
        cursor: None,
        filter: None,
        with_meta: false,
        matches: false,
        at: lakelands_api::ReadAt::default(),
        rerank: None,
    }
}

fn hybrid(text: &str) -> lakelands_api::query::SearchMode {
    lakelands_api::query::SearchMode::Hybrid {
        text: text.into(),
        field: "e".into(),
        vector: None,
        fields: None,
        text_fields: None,
        weight: None,
        text_weight: None,
        model: None,
    }
}

/// The client bound into a store: a hybrid search without a vector ranks
/// through the endpoint, a vector search never reaches it, and an
/// endpoint nobody answers on makes the search unavailable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_ranks_a_hybrid_search_through_the_client() {
    use lakelands_api::WriteContext;
    use lakelands_api::query::SearchMode;
    use lakelands_core::conformance::fixture::owner;

    // Every text is (1, 0), so both records' `e` point along it: `b`,
    // whose text does not match, is a hit only by the vector.
    let f = fake(Box::new(|body| {
        let data: Vec<Value> = (0..body["input"].as_array().unwrap().len())
            .map(|i| json!({"index": i, "embedding": [1.0, 0.0]}))
            .collect();
        Some(("200 OK", json!({"data": data}).to_string()))
    }))
    .await;
    let store = lake_with(f.endpoint()).await;
    let cat = store.catalog(WriteContext::default());
    let own = owner(store.as_ref());
    // The records' own `e` was embedded at their writes; the search's
    // requests are the ones after.
    let before = f.requests().len();
    let got = cat
        .search(&own, search(hybrid("quayside")))
        .await
        .unwrap()
        .data;
    let mut keys: Vec<&str> = got.hits.iter().map(|h| h.record.key.as_str()).collect();
    keys.sort();
    assert_eq!((keys, got.ranked_by.as_str()), (vec!["a", "b"], "hybrid"));
    assert_eq!(
        f.bodies()[before..],
        [json!({"model": "tiny", "input": ["quayside"]})]
    );
    cat.search(
        &own,
        search(SearchMode::Vector {
            field: "v".into(),
            vector: vec![1.0, 0.0],
            weight: None,
            also: Vec::new(),
            model: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        f.requests().len(),
        before + 1,
        "a vector search sends nothing"
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let store = lake_with(format!("http://127.0.0.1:{port}/v1/embeddings")).await;
    let cat = store.catalog(WriteContext::default());
    let own = owner(store.as_ref());
    match cat.search(&own, search(hybrid("quayside"))).await {
        Err(Error::Unavailable { situation }) => {
            let message = situation.to_string();
            assert!(message.contains("cannot be connected to"), "{message}");
            assert!(!message.contains("quayside"), "{message}");
        }
        other => panic!("expected the search to be unavailable, got {other:?}"),
    }
}

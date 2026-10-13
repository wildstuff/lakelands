//! What the door checks do not reach: every error decoded as the one the
//! server answered, an ingest's windows and a refused record over the
//! wire, `poll` read as the store's own feed reads, an export over the
//! wire as the store's own, the host operations, and the keys: refused
//! before sending, never shown.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lakelands_api::error::{Case, Error, Missing, Place, Refusal, Said};
use lakelands_api::id::{Position, RecordKey, SourceId, TypeName, Version};
use lakelands_api::location::Limits;
use lakelands_api::read::Get;
use lakelands_api::record::SourceRecord;
use lakelands_api::seam::{FEED_EXAMINED, FeedFilter};
use lakelands_api::write::{MigrationReport, WriteOpts};
use lakelands_api::{LakeId, LakeStore, Principal, Timestamp, WriteContext};
use lakelands_client::{ApiKey, RemoteConfig, RemoteLake};
use lakelands_door_checks::checks::{Cast, patch, setup};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A server that answers each request with the next canned answer, and
/// counts the connections it took.
async fn fake(
    answers: Vec<(u16, &'static str, String)>,
) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let taken = Arc::new(AtomicUsize::new(0));
    let count = taken.clone();
    tokio::spawn(async move {
        for (status, media, body) in answers {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            count.fetch_add(1, Ordering::SeqCst);
            let mut head = Vec::new();
            let mut b = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if s.read(&mut b).await.unwrap_or(0) == 0 {
                    break;
                }
                head.push(b[0]);
            }
            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: {media}\r\ncontent-length: {}\r\nlakelands-position: 7\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(reply.as_bytes()).await;
            let _ = s.shutdown().await;
        }
        // Take and count whatever else comes, answering nothing.
        while let Ok((_, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
        }
    });
    (addr, taken)
}

fn owner(lake: &LakeId) -> Principal {
    Cast::of(lake).owner
}

fn keys(ids: &[&str]) -> BTreeMap<String, ApiKey> {
    ids.iter()
        .map(|id| (id.to_string(), ApiKey::from(format!("k-{id}"))))
        .collect()
}

fn get(k: &str) -> Get {
    Get {
        key: RecordKey::from(k),
        with_meta: false,
        at: Default::default(),
        params: Default::default(),
    }
}

/// Every error of the contract, answered at the status the REST door
/// gives it, comes back as that error: its kind, its field and what it
/// carries.
#[tokio::test(flavor = "multi_thread")]
async fn every_error_comes_back_as_the_error_the_server_answered() {
    let refused = |field: Option<&str>, reason| Error::Refused {
        field: field.map(Into::into),
        reason,
    };
    let (ty, field) = (TypeName::from("invoice"), "status".into());
    let case = Case::UnknownField { ty, field };
    let place = Place::Type {
        name: "invoice".into(),
    };
    let said = || Box::new(Said::from(case.clone()).within(place.clone()));
    let report = MigrationReport {
        ty: "invoice".into(),
        from_schema_hash: "a".into(),
        to_schema_hash: "b".into(),
        records_scanned: 3,
        fields: Vec::new(),
        dropped: Vec::new(),
        rekeyed: Vec::new(),
        references: Vec::new(),
        sample_collisions: Vec::new(),
        restated: Vec::new(),
        lossless: false,
        report_id: "r".into(),
        more: true,
        relations: Vec::new(),
        carried: Vec::new(),
    };
    let errors = [
        (401, refused(None, Refusal::NotGranted)),
        (403, refused(Some("amount"), Refusal::NotGranted)),
        (403, refused(Some("amount"), Refusal::WrongDoor)),
        (403, refused(None, Refusal::Reserved)),
        (422, refused(Some("status"), Refusal::Invalid(said()))),
        (422, refused(Some("partner"), Refusal::Constraint(said()))),
        (422, refused(None, Refusal::Precondition(said()))),
        (422, refused(None, Refusal::TooDeep { max: 4 })),
        (422, refused(None, Refusal::TooLarge { max: 5 })),
        (
            422,
            refused(
                None,
                Refusal::Migration {
                    report: Box::new(report),
                },
            ),
        ),
        (409, refused(None, Refusal::Decided)),
        (409, refused(None, Refusal::Unanswered(said()))),
        (
            422,
            refused(None, Refusal::OutOfRange { head: Position(7) }),
        ),
        (
            404,
            Error::NotFound {
                missing: Missing::Record {
                    ty: "invoice".into(),
                    key: "INV-9".into(),
                },
            },
        ),
        (
            404,
            Error::NotFound {
                missing: Missing::Type {
                    name: "nowhere".into(),
                },
            },
        ),
        (
            412,
            Error::Conflict {
                expected: Version(1),
                actual: Version(2),
            },
        ),
        (
            501,
            Error::unbound(lakelands_api::error::Seam::Executor {
                name: "mailer".into(),
            }),
        ),
        (
            503,
            Error::unavailable(lakelands_api::error::Situation::Moved),
        ),
    ];
    let answers = errors
        .iter()
        .map(|(status, e)| {
            let mut v = serde_json::to_value(e).unwrap();
            v["what"] = json!("a sentence");
            v["todo"] = json!("another");
            (*status, "application/json", v.to_string())
        })
        .collect();
    let (addr, _) = fake(answers).await;
    let lake = LakeId::from("finance");
    let remote = common::remote(addr, &lake, keys(&["owner"]));
    let c = remote.catalog(WriteContext::default());
    for (_, e) in errors {
        let got = c.get(&owner(&lake), &"invoice".into(), get("INV-1")).await;
        assert_eq!(got.unwrap_err(), e);
    }
}

/// An answer that is not the contract's is `Unavailable`, naming the
/// lake, and never its address, the path or the key.
#[tokio::test(flavor = "multi_thread")]
async fn an_answer_that_is_not_the_contracts_is_unavailable_naming_where() {
    let (addr, _) = fake(vec![
        (502, "text/html", "<h1>bad gateway</h1>".into()),
        (200, "application/json", "not json".into()),
    ])
    .await;
    let lake = LakeId::from("finance");
    let remote = common::remote(addr, &lake, keys(&["owner"]));
    let c = remote.catalog(WriteContext::default());
    for status in ["502", "200"] {
        let e = c
            .get(&owner(&lake), &"invoice".into(), get("INV 1"))
            .await
            .unwrap_err();
        let Error::Unavailable { situation } = &e else {
            panic!("expected unavailable, got {e:?}");
        };
        let message = situation.to_string();
        assert!(
            message.contains("the REST door of lake `finance`"),
            "{message}"
        );
        assert!(
            !message.contains(&addr.to_string()) && !message.contains("INV"),
            "{message}"
        );
        assert!(message.contains(status), "{message}");
        assert!(!message.contains("k-owner"), "{message}");
    }
}

/// Ingest over the wire: one answer per window as it commits, and a
/// record the lake refuses ends the stream with the contract's error,
/// what came before it committed.
///
/// The server commits in windows of one record. The wire shows one
/// `Committed` line per window whatever the window holds, and how a window
/// fills is the store's, held in process by the suite. A window of one
/// closes at its record, so no answer here depends on how fast the next
/// record arrives: a larger window closes after 2 ms too, and a window is
/// all or nothing, so on a loaded machine a record could share the refused
/// record's window and not be committed (lakelands#807).
#[tokio::test(flavor = "multi_thread")]
async fn ingest_answers_each_window_and_ends_on_a_refused_record() {
    let limits = Limits {
        commit_window_events: 1,
        ..Limits::default()
    };
    let store = common::memory_lake("finance", limits).await;
    let (addr, keys) = common::serve(store.clone()).await;
    let remote: Arc<dyn LakeStore> = Arc::new(common::remote(addr, store.lake(), keys));
    // The client's limits are the ones it was configured with, not the
    // server's: here the server commits in windows of one.
    assert_eq!(remote.limits(), Limits::default());
    assert_ne!(remote.limits(), store.limits());
    let f = setup(remote.clone()).await;
    let record = |ty: &str, k: &str, amount: &str| SourceRecord {
        upstream: None,
        ty: ty.into(),
        key: k.into(),
        fields: patch(&[("amount", json!(amount))]),
        seen_at: None,
    };
    let collect = |records: Vec<SourceRecord>| {
        let (c, who) = (f.catalog.clone(), f.cast.importer.clone());
        async move {
            let stream = lakelands_door_checks::stream::of(records);
            let mut commits = c
                .ingest(&who, &SourceId::from("bank"), stream)
                .await
                .expect("the ingest starts");
            lakelands_door_checks::stream::collect(&mut commits).await
        }
    };
    let out = collect(vec![
        record("invoice", "INV-1", "10"),
        record("invoice", "INV-2", "20"),
        record("invoice", "INV-3", "30"),
    ])
    .await;
    let windows: Vec<(u32, u32)> = out
        .iter()
        .map(|c| {
            let c = c.as_ref().expect("each window commits");
            (c.records, c.duplicates)
        })
        .collect();
    assert_eq!(windows, [(1, 0), (1, 0), (1, 0)]);

    let out = collect(vec![
        record("invoice", "INV-4", "40"),
        record("invoice", "INV-5", "50"),
        record("nowhere", "N-1", "60"),
    ])
    .await;
    assert_eq!(out.len(), 3, "{out:?}");
    for window in &out[..2] {
        assert_eq!(window.as_ref().map(|c| c.records), Ok(1));
    }
    // The lake's own answer, not a failure of the wire: the refusal,
    // naming the record and where its window began (lakelands#1043).
    assert_eq!(
        out[2],
        Err(Error::Ingested {
            index: 2,
            from: 2,
            record: lakelands_api::error::RecordName::Of {
                ty: "nowhere".into(),
                key: "N-1".into(),
            },
            error: Box::new(Error::NotFound {
                missing: Missing::Type {
                    name: "nowhere".into()
                }
            }),
        }),
        "the lake's own answer, not a failure of the wire"
    );
    let local = store.catalog(WriteContext::default());
    let read = local
        .get(&f.cast.owner, &"invoice".into(), get("INV-5"))
        .await
        .expect("what came before the refused record is committed");
    assert_eq!(
        read.data.fields[&lakelands_api::id::FieldName::from("amount")],
        json!("50")
    );
}

/// `poll` over the wire reads what the store's own feed reads, for any
/// start, filter and limit, and follows the head as it moves.
#[tokio::test(flavor = "multi_thread")]
async fn poll_reads_what_the_stores_own_feed_reads() {
    let store = common::memory_lake("finance", Limits::default()).await;
    let (addr, keys) = common::serve(store.clone()).await;
    let remote: Arc<dyn LakeStore> = Arc::new(common::remote(addr, store.lake(), keys));
    let f = setup(remote.clone()).await;
    let write = |k: &'static str, status: &'static str| {
        let (c, who) = (f.catalog.clone(), f.cast.owner.clone());
        async move {
            c.upsert(
                &who,
                &"invoice".into(),
                &k.into(),
                patch(&[("status", json!(status))]),
                WriteOpts::default(),
            )
            .await
            .expect("the write applies")
        }
    };
    for (k, s) in [("INV-1", "open"), ("INV-2", "open"), ("INV-1", "paid")] {
        write(k, s).await;
    }
    let filters = [
        FeedFilter::default(),
        FeedFilter {
            kinds: vec!["record_upserted".into()],
            types: Vec::new(),
            keys: Vec::new(),
        },
        FeedFilter {
            kinds: Vec::new(),
            types: vec![TypeName::from("invoice")],
            keys: Vec::new(),
        },
        FeedFilter {
            kinds: Vec::new(),
            types: vec![TypeName::from("nowhere")],
            keys: Vec::new(),
        },
    ];
    let head = store.head().await.unwrap();
    let (theirs, ours) = (store.feed(), remote.feed());
    for filter in &filters {
        for from in [0, 1, head.0, head.0 + 1] {
            for limit in [1, 2, 256] {
                let want = theirs.poll(Position(from), filter, limit).await.unwrap();
                let got = ours.poll(Position(from), filter, limit).await.unwrap();
                assert_eq!(got, want, "from {from}, limit {limit}, filter {filter:?}");
            }
        }
    }
    // Caught up, a poll answers nothing and names where to go on; what
    // is written next is there.
    let caught_up = ours
        .poll(Position(head.0 + 1), &filters[0], 256)
        .await
        .unwrap();
    assert!(caught_up.events.is_empty());
    assert_eq!(caught_up.next, Position(head.0 + 1));
    let written = write("INV-3", "open").await;
    let next = ours.poll(caught_up.next, &filters[0], 256).await.unwrap();
    let positions: Vec<Position> = next.events.iter().map(|e| e.position).collect();
    assert_eq!(positions.last(), Some(&written.position()));
    assert_eq!(next.next, Position(written.position().0 + 1));
    assert_eq!(remote.head().await.unwrap(), written.position());
}

/// A poll over the wire whose filter matches nothing reaches the head and
/// ends: the server reads the log a bounded poll at a time
/// (lakelands#558). No `limit` answers more than the bound, as on the
/// store's own feed.
#[tokio::test(flavor = "multi_thread")]
async fn a_poll_over_the_wire_that_matches_nothing_reaches_the_head() {
    let store = common::memory_lake("finance", Limits::default()).await;
    let (addr, keys) = common::serve(store.clone()).await;
    let remote: Arc<dyn LakeStore> = Arc::new(common::remote(addr, store.lake(), keys));
    let f = setup(store.clone()).await;
    let records = (0..4200).map(|i| SourceRecord {
        upstream: None,
        ty: "invoice".into(),
        key: RecordKey::from(format!("B-{i}").as_str()),
        fields: patch(&[("amount", json!(format!("{i}.00")))]),
        seen_at: None,
    });
    let records = lakelands_door_checks::stream::of(records.collect());
    let bank = SourceId::from("bank");
    let mut acks = f
        .catalog
        .ingest(&f.cast.importer, &bank, records)
        .await
        .unwrap();
    let acks = lakelands_door_checks::stream::collect(&mut acks).await;
    assert!(acks.iter().all(Result::is_ok), "{acks:?}");
    let head = store.head().await.unwrap();
    assert!(head.0 > u64::from(FEED_EXAMINED));
    let none = FeedFilter {
        kinds: vec!["trigger_fired".into()],
        ..FeedFilter::default()
    };
    let polled = remote.feed().poll(Position(0), &none, 10).await.unwrap();
    assert!(polled.events.is_empty());
    assert_eq!((polled.next, polled.head), (head.next(), head));
    let all = FeedFilter::default();
    let ours = remote
        .feed()
        .poll(Position(0), &all, u32::MAX)
        .await
        .unwrap();
    let theirs = store
        .feed()
        .poll(Position(0), &all, u32::MAX)
        .await
        .unwrap();
    assert_eq!(ours.events.len() as u32, FEED_EXAMINED);
    assert_eq!(ours, theirs);
}

/// An export through a client is the served store's own, byte for byte:
/// every event crosses the wire and re-serialises to its stored bytes,
/// the host's checkpoint and a restricted record's hold included.
#[tokio::test(flavor = "multi_thread")]
async fn an_export_over_the_wire_is_the_served_stores_own() {
    let store = common::memory_lake("finance", Limits::default()).await;
    let (addr, keys) = common::serve(store.clone()).await;
    let remote: Arc<dyn LakeStore> = Arc::new(common::remote(addr, store.lake(), keys));
    let f = setup(remote.clone()).await;
    let write = |k: &'static str, status: &'static str| {
        let (c, who) = (f.catalog.clone(), f.cast.owner.clone());
        async move {
            c.upsert(
                &who,
                &"invoice".into(),
                &k.into(),
                patch(&[("status", json!(status))]),
                WriteOpts::default(),
            )
            .await
            .expect("the write applies")
        }
    };
    write("INV-1", "open").await;
    write("INV-2", "open").await;
    store.checkpoint().await.unwrap();
    store
        .restrict(&"invoice".into(), &"INV-2".into(), "a hold".into())
        .await
        .unwrap();
    write("INV-1", "paid").await;

    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("client-export-{}", std::process::id()));
    let (ours, theirs) = (base.join("remote"), base.join("store"));
    let _ = std::fs::remove_dir_all(&base);
    let got = lakelands_core::export::write(remote.as_ref(), &ours)
        .await
        .unwrap();
    let want = lakelands_core::export::write(store.as_ref(), &theirs)
        .await
        .unwrap();
    assert_eq!(got, want, "the same manifest");
    assert_eq!(got.segments.len(), 2, "{got:?}");
    for s in &want.segments {
        assert_eq!(
            std::fs::read(ours.join(&s.file)).unwrap(),
            std::fs::read(theirs.join(&s.file)).unwrap(),
            "{}: byte for byte",
            s.file
        );
    }
    let files = lakelands_core::export::verify(&ours).unwrap();
    let lake = store.verify(None, None).await.unwrap();
    assert_eq!((files.through, files.hash), (lake.through, lake.hash));
    let _ = std::fs::remove_dir_all(&base);
}

/// The host's operations are no network door's: each is `Unbound`,
/// naming it, and nothing is sent.
#[tokio::test(flavor = "multi_thread")]
async fn every_host_operation_is_unbound_naming_it() {
    let (addr, taken) = fake(Vec::new()).await;
    let lake = LakeId::from("finance");
    let remote = common::remote(addr, &lake, keys(&["owner"]));
    let (ty, key) = (TypeName::from("invoice"), RecordKey::from("INV-1"));
    let midnight: Timestamp = serde_json::from_value(json!("2026-09-25T00:00:00Z")).unwrap();
    let answers = [
        ("verify", remote.verify(None, None).await.map(|_| ())),
        ("rebuild", remote.rebuild().await.map(|_| ())),
        (
            "restrict",
            remote.restrict(&ty, &key, "hold".into()).await.map(|_| ()),
        ),
        ("unrestrict", remote.unrestrict(&ty, &key).await.map(|_| ())),
        ("checkpoint", remote.checkpoint().await.map(|_| ())),
        ("age", remote.age(midnight).await.map(|_| ())),
        (
            "purge_source",
            remote
                .purge_source(&SourceId::from("bank"), None, None)
                .await
                .map(|_| ()),
        ),
        ("compact", remote.compact().await.map(|_| ())),
        (
            "bind_embed",
            remote.bind_embed(lakelands_core::conformance::backing::embedder()),
        ),
        (
            "position_at",
            remote.position_at(midnight).await.map(|_| ()),
        ),
        (
            "vector_coverage",
            remote.vector_coverage(&ty).await.map(|_| ()),
        ),
    ];
    for (op, answer) in answers {
        match answer {
            Err(e) if e.seam().is_some() => {
                let Some(lakelands_api::error::Seam::Operation { op: named }) = e.seam() else {
                    panic!("{op}: {e:?}");
                };
                assert_eq!(named, op, "{op}");
            }
            other => panic!("{op}: expected unbound, got {other:?}"),
        }
    }
    assert_eq!(taken.load(Ordering::SeqCst), 0);
}

/// A feed filter with an empty key is refused before anything is sent,
/// naming `keys`: the door takes each key as a `key` parameter, and an
/// empty one is refused there. A filter with keys is sent (the door
/// checks answer it); only an empty key stops here (lakelands#146).
#[tokio::test(flavor = "multi_thread")]
async fn a_feed_filter_with_an_empty_key_is_refused_before_anything_is_sent() {
    let (addr, taken) = fake(Vec::new()).await;
    let lake = LakeId::from("finance");
    let remote = common::remote(addr, &lake, keys(&["owner"]));
    let filter = FeedFilter {
        keys: vec![RecordKey::from("INV-1"), RecordKey::from("")],
        ..FeedFilter::default()
    };
    match remote.feed().poll(Position(0), &filter, 10).await {
        Err(Error::Refused {
            field: Some(field),
            reason: lakelands_api::Refusal::Invalid { .. },
        }) => assert_eq!(field.as_str(), "keys"),
        other => panic!("an empty key is refused as invalid, naming `keys`: {other:?}"),
    }
    assert_eq!(taken.load(Ordering::SeqCst), 0);
}

/// A principal the client holds no key for, or of another lake, is
/// refused before anything is sent; so is the feed without a `feed_key`.
#[tokio::test(flavor = "multi_thread")]
async fn a_who_without_a_key_is_refused_before_anything_is_sent() {
    let (addr, taken) = fake(Vec::new()).await;
    let lake = LakeId::from("finance");
    let cast = Cast::of(&lake);
    let mut config = common::config(addr, &lake, keys(&["owner"]));
    config.feed_key = None;
    let remote = RemoteLake::connect(config).unwrap();
    let c = remote.catalog(WriteContext::default());
    let mut elsewhere = cast.owner.clone();
    elsewhere.lake = LakeId::from("hr");
    for who in [&cast.clerk, &elsewhere] {
        let e = c
            .get(who, &"invoice".into(), get("INV-1"))
            .await
            .unwrap_err();
        assert!(
            matches!(
                e,
                Error::Refused {
                    field: None,
                    reason: Refusal::NotGranted
                }
            ),
            "{e:?}"
        );
    }
    let e = remote
        .feed()
        .poll(Position(0), &FeedFilter::default(), 10)
        .await
        .unwrap_err();
    assert!(
        matches!(
            e,
            Error::Refused {
                reason: Refusal::NotGranted,
                ..
            }
        ),
        "{e:?}"
    );
    assert!(matches!(remote.head().await, Err(Error::Refused { .. })));
    assert_eq!(taken.load(Ordering::SeqCst), 0, "nothing was sent");
}

/// A key is in no `Debug` and in no failure: the connection refused
/// names the lake, and not its address, the path or the key.
#[tokio::test(flavor = "multi_thread")]
async fn no_key_shows_in_debug_or_in_a_failure() {
    let closed = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap()
    };
    let lake = LakeId::from("finance");
    let config = common::config(closed, &lake, keys(&["owner"]));
    let shown = format!("{config:?}");
    let remote = RemoteLake::connect(config).unwrap();
    let shown = format!("{shown} {remote:?}");
    let e = remote
        .catalog(WriteContext::default())
        .get(&owner(&lake), &"invoice".into(), get("INV-1"))
        .await
        .unwrap_err();
    let Error::Unavailable { situation } = &e else {
        panic!("expected unavailable, got {e:?}");
    };
    let message = situation.to_string();
    assert!(
        message.contains("the REST door of lake `finance`"),
        "{message}"
    );
    assert!(
        !message.contains(&closed.to_string()) && !message.contains("INV-1"),
        "{message}"
    );
    for text in [&shown, &message] {
        assert!(!text.contains("k-owner"), "{text}");
    }
}

/// The URL is `http` or `https`, without user-info, a query or a
/// fragment; `feed_key` names a principal with a key; `https` needs a
/// bundle it can read. A refusal never repeats a secret the URL held.
#[test]
fn only_a_plain_http_or_https_url_is_taken() {
    let lake = LakeId::from("finance");
    let base = |url: &str| RemoteConfig {
        url: url.into(),
        ..common::config("127.0.0.1:1".parse().unwrap(), &lake, keys(&["owner"]))
    };
    for url in [
        "ftp://lakes.example/",
        "lakes.example",
        "http://someone:s3cret@lakes.example/",
        "http://lakes.example/?key=s3cret",
        "http://lakes.example/#s3cret",
    ] {
        let e = RemoteLake::connect(base(url)).unwrap_err();
        assert!(!e.0.contains("s3cret"), "{url}: {e}");
    }
    let mut c = base("http://lakes.example/");
    c.feed_key = Some("clerk".into());
    assert!(RemoteLake::connect(c).is_err(), "`feed_key` names no key");
    let mut c = base("https://lakes.example/");
    c.ca_file = Some("/nonexistent/ca.pem".into());
    assert!(RemoteLake::connect(c).is_err(), "no bundle to trust");
    assert!(RemoteLake::connect(base("http://lakes.example:8080/under/")).is_ok());
}

/// A server that answers every request with `head` and then `chunk`
/// over and over, as a chunked body that never ends.
async fn endless(head: &'static str, chunk: &'static [u8]) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut req = Vec::new();
                let mut b = [0u8; 1];
                while !req.ends_with(b"\r\n\r\n") && s.read(&mut b).await.unwrap_or(0) > 0 {
                    req.push(b[0]);
                }
                let reply = format!(
                    "HTTP/1.1 {head}\r\nlakelands-position: 7\r\ntransfer-encoding: chunked\r\n\r\n"
                );
                if s.write_all(reply.as_bytes()).await.is_err() {
                    return;
                }
                let framed = [format!("{:x}\r\n", chunk.len()).as_bytes(), chunk, b"\r\n"].concat();
                // Until the client hangs up.
                while s.write_all(&framed).await.is_ok() {}
            });
        }
    });
    addr
}

/// A client whose answers may hold 64 KiB, and the failure that says so.
fn capped(addr: std::net::SocketAddr) -> RemoteLake {
    let lake = LakeId::from("finance");
    let mut config = common::config(addr, &lake, keys(&["owner", "importer"]));
    config.max_answer_bytes = 64 << 10;
    RemoteLake::connect(config).unwrap()
}

fn too_long(e: Error, what: &str) {
    match e {
        Error::Unavailable { situation } => {
            let message = situation.to_string();
            assert!(
                message.contains("more than 65536 bytes"),
                "{what}: {message}"
            )
        }
        other => panic!("{what}: expected unavailable, got {other:?}"),
    }
}

/// No whole answer is held past `max_answer_bytes`, a success's or a
/// failure's: an endless body is `Unavailable`, saying so, and the
/// client stops reading.
#[tokio::test(flavor = "multi_thread")]
async fn no_whole_answer_grows_past_its_cap() {
    let lake = LakeId::from("finance");
    for head in [
        "200 OK\r\ncontent-type: application/json",
        "500 X\r\ncontent-type: application/json",
    ] {
        let remote = capped(endless(head, &[b' '; 1024]).await);
        let e = remote
            .catalog(WriteContext::default())
            .get(&owner(&lake), &"invoice".into(), get("INV-1"))
            .await
            .unwrap_err();
        too_long(e, head);
    }
}

/// A feed block that never ends is not held past the cap, though its
/// bytes keep the stream from ever being silent.
#[tokio::test(flavor = "multi_thread")]
async fn no_feed_block_grows_past_its_cap() {
    let remote = capped(endless("200 OK\r\ncontent-type: text/event-stream", &[b'x'; 1024]).await);
    let e = remote
        .feed()
        .poll(Position(0), &FeedFilter::default(), 10)
        .await
        .unwrap_err();
    too_long(e, "a feed block without its end");
}

/// An ingest answer's line that never ends is not held past the cap.
#[tokio::test(flavor = "multi_thread")]
async fn no_ingest_line_grows_past_its_cap() {
    let remote = capped(
        endless(
            "200 OK\r\ncontent-type: application/x-ndjson",
            &[b'x'; 1024],
        )
        .await,
    );
    let importer = Cast::of(&LakeId::from("finance")).importer;
    let mut commits = remote
        .catalog(WriteContext::default())
        .ingest(
            &importer,
            &SourceId::from("bank"),
            lakelands_door_checks::stream::of(Vec::new()),
        )
        .await
        .expect("the ingest starts");
    let out = lakelands_door_checks::stream::collect(&mut commits).await;
    assert_eq!(out.len(), 1, "{out:?}");
    too_long(
        out.into_iter().next().unwrap().unwrap_err(),
        "an ingest line without its end",
    );
}

/// An empty id would name another route, since the server drops empty
/// path segments (`get` would become `find`, `retire` a `DELETE` on the
/// collection): it is refused as invalid before anything is sent. So is
/// an idempotency key a header cannot carry.
#[tokio::test(flavor = "multi_thread")]
async fn what_a_path_or_a_header_cannot_carry_is_refused_before_sending() {
    let (addr, taken) = fake(Vec::new()).await;
    let lake = LakeId::from("finance");
    let remote = common::remote(addr, &lake, keys(&["owner"]));
    let c = remote.catalog(WriteContext::default());
    let who = owner(&lake);
    let (ty, empty) = (TypeName::from("invoice"), RecordKey::from(""));
    let crlf = WriteOpts {
        idempotency_key: Some("retry-1\r\nx-evil: 1".into()),
        ..WriteOpts::default()
    };
    let answers = [
        c.get(&who, &ty, get("")).await.map(|_| ()),
        c.get(&who, &TypeName::from(""), get("INV-1"))
            .await
            .map(|_| ()),
        c.retire(&who, &ty, &empty, WriteOpts::default())
            .await
            .map(|_| ()),
        c.upsert(&who, &ty, &empty, patch(&[]), WriteOpts::default())
            .await
            .map(|_| ()),
        c.unlink(
            &who,
            &ty,
            &"INV-1".into(),
            &"partner".into(),
            &empty,
            WriteOpts::default(),
        )
        .await
        .map(|_| ()),
        c.accept(&who, &"".into()).await.map(|_| ()),
        c.upsert(&who, &ty, &"INV-1".into(), patch(&[]), crlf.clone())
            .await
            .map(|_| ()),
        c.retire(&who, &ty, &"INV-1".into(), crlf).await.map(|_| ()),
    ];
    for (i, a) in answers.into_iter().enumerate() {
        assert!(
            matches!(
                a,
                Err(Error::Refused {
                    field: None,
                    reason: Refusal::Invalid { .. }
                })
            ),
            "case {i}: {a:?}"
        );
    }
    assert_eq!(taken.load(Ordering::SeqCst), 0, "nothing was sent");
}

/// An executor that answers `null`, holding its first call until it is
/// let go.
#[derive(Default)]
struct Null {
    ran: AtomicUsize,
    go: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl lakelands_api::seam::VerbExecutor for Null {
    fn name(&self) -> &str {
        "null"
    }

    async fn execute(
        &self,
        _: &lakelands_api::seam::VerbInvocation,
    ) -> lakelands_api::Result<serde_json::Value> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        while !self.go.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        Ok(serde_json::Value::Null)
    }
}

/// An executor's answer of `null` comes back over the wire as an answer:
/// the call and its retry under its key are `applied` with `"output":
/// null`. Sent again while the executor still runs, the call is 409
/// `unanswered`, with the door's advice.
#[tokio::test(flavor = "multi_thread")]
async fn an_answer_of_null_comes_back_and_a_call_still_running_is_409() {
    let null = Arc::new(Null::default());
    let seams = lakelands_api::seam::Seams {
        engines: vec![Arc::new(lakelands_engine_mem::MemoryOpener::new())],
        executors: vec![null.clone()],
        ..Default::default()
    };
    let location = lakelands_api::Location::memory("nulls");
    let create = lakelands_api::engine::OpenMode::Create;
    let store = lakelands_store::open(location, seams, create)
        .await
        .unwrap();
    let lake = store.lake().clone();
    let cat = store.catalog(WriteContext::default());
    for d in [
        json!({"declare": "type", "name": "job", "fields": [{"name": "n", "type": {"kind": "text"}}]}),
        json!({"declare": "verb", "name": "dispatch", "on": "job", "risk": "low",
               "effect": "external", "executor": "null"}),
    ] {
        cat.declare(&owner(&lake), serde_json::from_value(d).unwrap())
            .await
            .unwrap();
    }
    let (job, a) = (TypeName::from("job"), RecordKey::from("a"));
    let opts = WriteOpts::default();
    cat.upsert(&owner(&lake), &job, &a, patch(&[("n", json!("x"))]), opts)
        .await
        .unwrap();
    let (addr, keys) = common::serve(store).await;
    let remote = Arc::new(common::remote(addr, &lake, keys));
    let call = || {
        let remote = remote.clone();
        let lake = lake.clone();
        async move {
            let opts = WriteOpts {
                idempotency_key: Some("once".into()),
                ..WriteOpts::default()
            };
            let c = remote.catalog(WriteContext::default());
            let (verb, a) = ("dispatch".into(), RecordKey::from("a"));
            c.call(&owner(&lake), &verb, Some(&a), patch(&[]), opts)
                .await
        }
    };
    let first = tokio::spawn(call());
    while null.ran.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "POST /lakes/{lake}/verbs/dispatch/a HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\
         authorization: Bearer k-owner\r\nidempotency-key: once\r\n\
         content-type: application/json\r\ncontent-length: 2\r\n\r\n{{}}"
    );
    s.write_all(request.as_bytes()).await.unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).await.unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert!(head.starts_with("HTTP/1.1 409"), "{head}");
    assert_eq!(body["reason"]["refusal"], "unanswered", "{body}");
    let todo = "a retry under this key runs nothing: if the call's executor is still running";
    assert!(body["todo"].as_str().unwrap().starts_with(todo), "{body}");
    null.go.store(true, Ordering::SeqCst);
    let first = first.await.unwrap().unwrap();
    assert!(
        matches!(&first, lakelands_api::Outcome::Applied { output: Some(o), .. } if o.is_null()),
        "{first:?}"
    );
    assert_eq!(call().await.unwrap(), first, "the retry answers the first");
    assert_eq!(null.ran.load(Ordering::SeqCst), 1, "run once");
}

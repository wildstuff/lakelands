//! The store over the in-memory engine: what the conformance suite does
//! not reach through the doors — the lease across opens, verification,
//! checkpoints, the change feed, verbs by risk, ingest's windows,
//! migrations over history, search paging and embedding, and the
//! declarations the door refuses.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::event::{Digest, EventBody, SegmentDigest};
use lakelands_api::expr::Condition;
use lakelands_api::ontology::{
    Assignment, Authority, FieldDecl, FieldType, RelationDecl, Risk, StorageKind, TypeDecl,
    VerbDecl, VerbEffect,
};
use lakelands_api::query::{Search, SearchMode};
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::{
    Declaration, FunctionDecl, Migration, Outcome, ProposalFilter, ProposedWrite,
};
use lakelands_api::{
    Error, Get, Grants, LakeStore, Location, Patch, Position, Principal, PrincipalKind, ReadAt,
    Refusal, SourceRecord, WriteContext, WriteOpts,
};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

fn name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("store-test-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// The tests' one opener: a test that reopens a lake finds it again, and
/// every test's lakes carry names of their own.
fn opener() -> MemoryOpener {
    static OPENER: std::sync::OnceLock<MemoryOpener> = std::sync::OnceLock::new();
    OPENER.get_or_init(MemoryOpener::new).clone()
}

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(opener())],
        ..Seams::default()
    }
}

async fn open(lake: &str, mode: OpenMode) -> lakelands_api::Result<Arc<dyn LakeStore>> {
    lakelands_store::open(Location::memory(lake), seams(), mode).await
}

fn owner(lake: &str) -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: lake.into(),
        grants: Grants::owner(),
    }
}

fn field(name: &str, kind: StorageKind) -> FieldDecl {
    FieldDecl {
        annotations: Default::default(),
        name: name.into(),
        ty: FieldType::kind(kind),
        required: false,
        authority: Authority::Authored,
        derived: None,
        function: None,
        depends_on: vec![],
        on_input_change: Default::default(),
        sensitive: false,
        searchable: false,
        description: None,
        label: None,
        embed: None,
    }
}

fn ty(name: &str, fields: Vec<FieldDecl>) -> TypeDecl {
    TypeDecl {
        annotations: Default::default(),
        name: name.into(),
        key: vec![],
        fields,
        relations: vec![],
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
    }
}

fn patch(v: serde_json::Value) -> Patch {
    serde_json::from_value(v).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_writer_is_refused_at_once_and_a_reader_opens_beside_it() {
    let lake = name();
    let first = open(&lake, OpenMode::Create).await.unwrap();
    assert!(first.is_writer());
    let second = open(&lake, OpenMode::Write).await;
    assert!(
        matches!(second, Err(Error::Unavailable { .. })),
        "{:?}",
        second.err()
    );
    let reader = open(&lake, OpenMode::Read).await.unwrap();
    assert!(!reader.is_writer());
    let o = owner(&lake);
    let refused = reader
        .catalog(WriteContext::default())
        .declare(&o, Declaration::Type(ty("note", vec![])))
        .await;
    assert!(matches!(refused, Err(Error::Unavailable { .. })));
    first.close().await.unwrap();
    let again = open(&lake, OpenMode::Write).await.unwrap();
    assert!(again.is_writer());
}

/// Only the writer's handle compacts: a handle opened read-only, and a
/// writer that gave its lease up, refuse it as unavailable, as they refuse
/// a checkpoint; the writer's compact appends nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_handle_refuses_compact_and_the_writer_compacts_in_place() {
    let lake = name();
    let writer = open(&lake, OpenMode::Create).await.unwrap();
    let reader = open(&lake, OpenMode::Read).await.unwrap();
    let head = writer.head().await.unwrap();
    for (who, store) in [("a reader", &reader), ("a writer", &writer)] {
        if who == "a writer" {
            let report = writer.compact().await.unwrap();
            assert_eq!(
                (report.before, report.after, report.head),
                (None, None, head)
            );
            writer.close().await.unwrap();
        }
        for (op, answer) in [
            ("compact", store.compact().await.map(|_| ())),
            ("checkpoint", store.checkpoint().await.map(|_| ())),
        ] {
            match answer {
                Err(Error::Unavailable { message }) => {
                    assert_eq!(message, "this lake is open read-only here", "{who}, {op}");
                }
                other => panic!("{who}, {op}: expected unavailable, got {other:?}"),
            }
        }
    }
    assert_eq!(writer.head().await.unwrap(), head, "nothing was appended");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_lake_starts_with_its_creation_its_lease_and_its_system_types() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let feed = store.feed();
    let batch = feed
        .poll(Position(0), &FeedFilter::default(), 10)
        .await
        .unwrap();
    let kinds: Vec<&str> = batch.events.iter().map(|e| e.body.kind()).collect();
    assert_eq!(
        kinds,
        ["lake_created", "lease_taken", "declared", "declared"]
    );
    assert_eq!(
        batch.events[0].fence, 0,
        "position 0 is written before any lease"
    );
    assert_eq!(batch.events[1].fence, 1);
    let v = store.verify(None, None).await.unwrap();
    assert_eq!(v.through, Position(3));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_checkpoint_seals_a_segment_and_verification_follows_it() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    cat.declare(
        &o,
        Declaration::Type(ty("note", vec![field("body", StorageKind::Text)])),
    )
    .await
    .unwrap();
    let at = store.checkpoint().await.unwrap();
    cat.upsert(
        &o,
        &"note".into(),
        &"n1".into(),
        patch(json!({"body": "x"})),
        Default::default(),
    )
    .await
    .unwrap();
    let v = store.verify(None, None).await.unwrap();
    assert!(v.through > at);
    let checkpoints = store
        .feed()
        .poll(
            Position(0),
            &FeedFilter {
                kinds: vec!["checkpoint".into()],
                types: vec![],
                keys: Vec::new(),
            },
            10,
        )
        .await
        .unwrap();
    assert_eq!(checkpoints.events.len(), 1);
    assert_eq!(store.rebuild().await.unwrap(), v.through);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_feed_filters_by_kind_and_type_and_says_where_to_go_on() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    for t in ["a", "b"] {
        cat.declare(
            &o,
            Declaration::Type(ty(t, vec![field("x", StorageKind::Text)])),
        )
        .await
        .unwrap();
        cat.upsert(
            &o,
            &t.into(),
            &"k".into(),
            patch(json!({"x": "1"})),
            Default::default(),
        )
        .await
        .unwrap();
    }
    let filter = FeedFilter {
        kinds: vec!["record_upserted".into()],
        types: vec!["b".into()],
        keys: Vec::new(),
    };
    let batch = store.feed().poll(Position(0), &filter, 10).await.unwrap();
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.next, batch.head.next());
    let later = store.feed().poll(batch.next, &filter, 10).await.unwrap();
    assert!(later.events.is_empty());
}

fn invoice_with_verbs() -> Vec<Declaration> {
    let write = |name: &str, risk, value: &str| VerbDecl {
        name: name.into(),
        on: "invoice".into(),
        scope: Default::default(),
        payload: vec![field("note", StorageKind::Text)],
        risk,
        requires: None,
        effect: VerbEffect::Write {
            set: vec![Assignment {
                field: "status".into(),
                value: Condition::from(value),
            }],
        },
        description: None,
    };
    let pay = VerbDecl {
        name: "pay".into(),
        on: "invoice".into(),
        scope: Default::default(),
        payload: vec![],
        risk: Risk::Low,
        requires: None,
        effect: VerbEffect::External {
            executor: "bank".into(),
        },
        description: None,
    };
    vec![
        Declaration::Type(ty("invoice", vec![field("status", StorageKind::Text)])),
        Declaration::Verb(write(
            "settle",
            Risk::Low,
            "concat('settled: ', payload.note)",
        )),
        Declaration::Verb(write("write_off", Risk::High, "'written off'")),
        Declaration::Verb(pay),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_low_verb_runs_a_high_one_waits_for_acceptance_and_an_unbound_one_is_refused() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    for d in invoice_with_verbs() {
        cat.declare(&o, d).await.unwrap();
    }
    cat.upsert(
        &o,
        &"invoice".into(),
        &"i1".into(),
        patch(json!({"status": "open"})),
        Default::default(),
    )
    .await
    .unwrap();

    let ran = cat
        .call(
            &o,
            &"settle".into(),
            Some(&"i1".into()),
            patch(json!({"note": "cash"})),
            Default::default(),
        )
        .await
        .unwrap();
    assert!(
        matches!(
            ran,
            Outcome::Applied {
                version: Some(_),
                ..
            }
        ),
        "{ran:?}"
    );
    let get = |k: &str| Get {
        key: k.into(),
        with_meta: true,
        at: ReadAt::default(),
        params: Default::default(),
    };
    let r = cat
        .get(&o, &"invoice".into(), get("i1"))
        .await
        .unwrap()
        .data;
    assert_eq!(r.fields["status"], json!("settled: cash"));
    assert_eq!(r.meta["status"].procedure_version.as_deref(), Some("1"));

    let waiting = cat
        .call(
            &o,
            &"write_off".into(),
            Some(&"i1".into()),
            patch(json!({})),
            Default::default(),
        )
        .await
        .unwrap();
    let Outcome::Proposed { proposal, .. } = waiting else {
        panic!("{waiting:?}")
    };
    let pending = cat
        .proposals(&o, ProposalFilter::default())
        .await
        .unwrap()
        .data;
    assert_eq!(pending.len(), 1);
    let accepted = cat.accept(&o, &proposal).await.unwrap();
    let r = cat.get(&o, &"invoice".into(), get("i1")).await.unwrap();
    assert_eq!(r.data.fields["status"], json!("written off"));
    assert_eq!(
        accepted,
        Outcome::Applied {
            position: r.data.meta["status"].position,
            version: Some(r.data.version),
            output: None,
            set_off: Default::default(),
            renamed: Vec::new(),
        },
        "accepting a verb answers the write it made, not the verb's call"
    );
    let again = cat.accept(&o, &proposal).await;
    assert!(matches!(
        again,
        Err(Error::Refused {
            reason: Refusal::Decided,
            ..
        })
    ));

    let unbound = cat
        .call(
            &o,
            &"pay".into(),
            Some(&"i1".into()),
            patch(json!({})),
            Default::default(),
        )
        .await;
    assert!(matches!(unbound, Err(Error::Unbound { .. })), "{unbound:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn ingest_skips_redeliveries_and_unchanged_records_but_writes_a_revert() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let mut amount = field("amount", StorageKind::Decimal);
    amount.authority = Authority::Source;
    cat.declare(&o, Declaration::Type(ty("tx", vec![amount])))
        .await
        .unwrap();
    let rec = |v: &str| SourceRecord {
        upstream: None,
        ty: "tx".into(),
        key: "t1".into(),
        fields: patch(json!({"amount": v})),
        seen_at: None,
    };
    let deliver = |records: Vec<SourceRecord>| {
        let cat = cat.clone();
        let o = o.clone();
        async move {
            let s: lakelands_api::SourceStream = Box::pin(Stream(records.into_iter()));
            let mut out = cat.ingest(&o, &"bank".into(), s).await.unwrap();
            let mut windows = Vec::new();
            while let Some(w) = std::future::poll_fn(|cx| out.as_mut().poll_next(cx)).await {
                windows.push(w.unwrap());
            }
            windows
        }
    };
    let totals = |w: &[lakelands_api::Committed]| {
        w.iter()
            .fold((0, 0), |(r, d), c| (r + c.records, d + c.duplicates))
    };
    assert_eq!(totals(&deliver(vec![rec("1.00")]).await), (1, 0));
    assert_eq!(
        totals(&deliver(vec![rec("1.00")]).await),
        (0, 1),
        "a redelivery appends nothing"
    );
    assert_eq!(
        totals(&deliver(vec![rec("2.00"), rec("1.00")]).await),
        (2, 0),
        "a revert is a write"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_write_to_a_record_in_one_window_is_folded_over_the_first() {
    // The window reads `a1` from the projection once and hands that read
    // to the fold; the fold's own first put of `a1` must answer the second
    // write, never the read from before the window.
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let mut label = field("label", StorageKind::Text);
    label.authority = Authority::Source;
    let mut limit = field("limit", StorageKind::Decimal);
    limit.authority = Authority::Source;
    cat.declare(&o, Declaration::Type(ty("acct", vec![label, limit])))
        .await
        .unwrap();
    let rec = |fields: serde_json::Value| SourceRecord {
        upstream: None,
        ty: "acct".into(),
        key: "a1".into(),
        fields: patch(fields),
        seen_at: None,
    };
    let deliver = |records: Vec<SourceRecord>| {
        let cat = cat.clone();
        let o = o.clone();
        async move {
            let s: lakelands_api::SourceStream = Box::pin(Stream(records.into_iter()));
            let mut out = cat.ingest(&o, &"bank".into(), s).await.unwrap();
            let mut windows = Vec::new();
            while let Some(w) = std::future::poll_fn(|cx| out.as_mut().poll_next(cx)).await {
                windows.push(w.unwrap());
            }
            windows
        }
    };
    deliver(vec![rec(json!({"label": "old"}))]).await;
    let windows = deliver(vec![
        rec(json!({"label": "new"})),
        rec(json!({"limit": "5.00"})),
    ])
    .await;
    assert_eq!(windows.len(), 1, "both writes go in one window");
    assert_eq!(windows[0].records, 2);
    let r = cat
        .get(
            &o,
            &"acct".into(),
            Get {
                key: "a1".into(),
                with_meta: false,
                at: ReadAt::default(),
                params: Default::default(),
            },
        )
        .await
        .unwrap()
        .data;
    assert_eq!(
        (
            r.fields["label"].clone(),
            r.fields["limit"].clone(),
            r.version
        ),
        (json!("new"), json!("5.00"), lakelands_api::Version(3)),
        "the second write is folded over the first, not over the state before the window"
    );
}

struct Stream<I>(I);

impl<I: Iterator<Item = SourceRecord> + Unpin> futures_core::Stream for Stream<I> {
    type Item = SourceRecord;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<SourceRecord>> {
        std::task::Poll::Ready(self.0.next())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_door_refuses_what_this_release_does_not_hold() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let sys = cat
        .declare(&o, Declaration::Type(ty("sys.mine", vec![])))
        .await;
    assert!(matches!(
        sys,
        Err(Error::Refused {
            reason: Refusal::Reserved,
            ..
        })
    ));
    // A relation carried by `via` holds no fields of its own: those belong
    // to a relation carried by link records.
    let mut linked = ty("a", vec![field("b", StorageKind::Reference)]);
    linked.relations.push(RelationDecl {
        derived: None,
        annotations: Default::default(),
        name: "to_a".into(),
        target: "a".into(),
        cardinality: Default::default(),
        inverse: None,
        via: Some("b".into()),
        fields: vec![field("since", StorageKind::DateTime)],
        authority: Default::default(),
        description: None,
    });
    assert!(matches!(
        cat.declare(&o, Declaration::Type(linked)).await,
        Err(Error::Refused {
            reason: Refusal::Invalid { .. },
            ..
        })
    ));
    // No `http` backing is bound here, so a function of that kind is
    // refused, naming the kind.
    let remote = cat
        .declare(
            &o,
            Declaration::Function(FunctionDecl {
                name: "remote".into(),
                backing: "http".into(),
                inputs: vec![],
                expr: None,
                config: json!({"url": "x"}),
                description: None,
            }),
        )
        .await;
    assert!(
        matches!(&remote, Err(Error::Unbound { seam }) if seam.contains("`http`")),
        "{remote:?}"
    );
    let empty = cat
        .upsert(
            &o,
            &"sys.setting".into(),
            &"k".into(),
            patch(json!({})),
            Default::default(),
        )
        .await;
    assert!(
        matches!(
            empty,
            Err(Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            })
        ),
        "an empty patch is refused"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_migration_covers_retired_records_and_keeps_the_key() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let mut m = ty(
        "m",
        vec![field("k", StorageKind::Text), field("n", StorageKind::Text)],
    );
    m.key = vec!["k".into()];
    cat.declare(&o, Declaration::Type(m.clone())).await.unwrap();
    for (k, n) in [("r1", "1"), ("r2", "not a number")] {
        cat.upsert(
            &o,
            &"m".into(),
            &k.into(),
            patch(json!({"n": n})),
            Default::default(),
        )
        .await
        .unwrap();
    }
    cat.retire(&o, &"m".into(), &"r2".into(), Default::default())
        .await
        .unwrap();

    let mut to = m.clone();
    to.fields[1] = field("n", StorageKind::Number);
    let retyped = Migration {
        to,
        renames: vec![],
        relation_renames: Vec::new(),
    };
    let report = cat.migrate(&o, retyped.clone()).await.unwrap().data;
    assert!(
        !report.lossless,
        "the retired `r2` cannot convert, so the migration is not lossless: {report:?}"
    );
    let refused = cat.migrate_apply(&o, retyped, &report.report_id).await;
    assert!(
        matches!(
            refused,
            Err(Error::Refused {
                reason: Refusal::Migration { .. },
                ..
            })
        ),
        "{refused:?}"
    );
    let head = store.head().await.unwrap();
    store.rebuild().await.unwrap();
    assert_eq!(store.head().await.unwrap(), head, "the lake folds on");

    let mut rekeyed = m.clone();
    rekeyed.key = vec!["n".into()];
    let r = cat
        .migrate(
            &o,
            Migration {
                to: rekeyed,
                renames: vec![],
                relation_renames: Vec::new(),
            },
        )
        .await;
    assert!(
        matches!(
            &r,
            Err(Error::Refused {
                reason: Refusal::Invalid { message },
                ..
            }) if message.contains("key")
        ),
        "a migration that changes the key is refused: {r:?}"
    );
}

/// `verify` over a lake whose first segment a writer that is not the
/// store sealed, beginning the next with the event `first` makes of the
/// tip's position and hash and of every segment sealed, that one included.
async fn verified_after_a_seal_by_another_writer(
    first: impl Fn(Position, Digest, Vec<SegmentDigest>) -> EventBody,
) -> lakelands_api::Result<lakelands_api::Verification> {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    cat.declare(
        &o,
        Declaration::Type(ty("t", vec![field("a", StorageKind::Text)])),
    )
    .await
    .unwrap();
    store.close().await.unwrap();
    drop((cat, store));

    let engine = opener()
        .open(&Location::memory(lake.as_str()), OpenMode::Write)
        .unwrap();
    let lease = engine.take_lease("another writer").unwrap();
    let (tip, hash) = engine.tip().unwrap().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let sealed = engine.seal_segment(&lease, false, &mut |sealed| {
        let draft = lakelands_store::log::Draft::system(
            lakelands_api::Timestamp(now.as_micros() as i64),
            first(tip, hash.clone(), vec![sealed.clone()]),
        );
        let (lake, at) = (lake.as_str().into(), tip.next());
        let (_, canon) = lakelands_store::log::seal(&lake, at, &hash, lease.fence, draft);
        Ok(vec![canon])
    });
    sealed.unwrap().unwrap();
    engine.release_lease(&lease).unwrap();
    drop(engine);
    let reader = open(&lake, OpenMode::Read).await.unwrap();
    reader.verify(None, None).await
}

/// A checkpoint that does not follow the segment it seals is reported.
#[tokio::test(flavor = "multi_thread")]
async fn a_tampered_checkpoint_is_reported_by_verify() {
    let r = verified_after_a_seal_by_another_writer(|tip, hash, segments| EventBody::Checkpoint {
        through: Position(tip.0 - 1),
        hash,
        segments,
    })
    .await;
    assert!(
        matches!(&r, Err(Error::Unavailable { message }) if message.contains("checkpoint")),
        "{r:?}"
    );
}

/// A segment after the first begins with a `checkpoint`
/// (`docs/log-format.md` §4.2): `verify` refuses a log whose second
/// segment begins with any other event, naming the position, and takes
/// one that begins with its checkpoint (lakelands#623).
#[tokio::test(flavor = "multi_thread")]
async fn a_segment_that_begins_with_no_checkpoint_is_reported_by_verify() {
    let whole =
        verified_after_a_seal_by_another_writer(|through, hash, segments| EventBody::Checkpoint {
            through,
            hash,
            segments,
        })
        .await;
    assert!(whole.is_ok(), "{whole:?}");
    let r = verified_after_a_seal_by_another_writer(|_, _, _| EventBody::LeaseTaken {
        fence: 2,
        holder: "another writer".into(),
    })
    .await;
    assert!(
        matches!(&r, Err(Error::Unavailable { message })
            if message.contains("a segment begins there") && message.contains("no checkpoint")),
        "{r:?}"
    );
}

/// A log's format versions only go up (`docs/log-format.md` §6):
/// `verify` refuses an event written in a lower version than the one
/// before it, naming both.
#[tokio::test(flavor = "multi_thread")]
async fn verify_refuses_a_format_version_that_goes_down() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    store.close().await.unwrap();
    drop(store);
    let engine = opener()
        .open(&Location::memory(lake.as_str()), OpenMode::Write)
        .unwrap();
    let lease = engine.take_lease("tamperer").unwrap();
    let (tip, hash) = engine.tip().unwrap().unwrap();
    let now = lakelands_api::Timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros() as i64,
    );
    let taken = |fence| {
        lakelands_store::log::Draft::system(
            now,
            EventBody::LeaseTaken {
                fence,
                holder: "tamperer".into(),
            },
        )
    };
    let (two, canon_two) = lakelands_store::log::seal_at(
        2,
        &lake.as_str().into(),
        tip.next(),
        &hash,
        lease.fence,
        taken(lease.fence),
    );
    let (_, canon_one) = lakelands_store::log::seal_at(
        1,
        &lake.as_str().into(),
        two.position.next(),
        &two.hash,
        lease.fence,
        taken(lease.fence),
    );
    engine.append(&lease, &[canon_two, canon_one]).unwrap();
    engine.release_lease(&lease).unwrap();
    drop(engine);

    let reader = open(&lake, OpenMode::Read).await.unwrap();
    let r = reader.verify(None, None).await;
    assert!(
        matches!(&r, Err(Error::Unavailable { message })
            if message.contains("format 1") && message.contains("format 2")),
        "{r:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_store_dropped_without_closing_gives_its_lease_up() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    assert!(store.is_writer());
    drop(store);
    let again = open(&lake, OpenMode::Write).await.unwrap();
    assert!(again.is_writer());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proposal_holds_the_write_as_checked() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    cat.declare(
        &o,
        Declaration::Type(ty("e", vec![field("at", StorageKind::DateTime)])),
    )
    .await
    .unwrap();
    let lenient = json!({"at": "2026-09-24T10:00:00Z"});
    cat.upsert(
        &o,
        &"e".into(),
        &"applied".into(),
        patch(lenient.clone()),
        Default::default(),
    )
    .await
    .unwrap();
    let stored = cat
        .get(
            &o,
            &"e".into(),
            Get {
                key: "applied".into(),
                with_meta: false,
                at: ReadAt::default(),
                params: Default::default(),
            },
        )
        .await
        .unwrap()
        .data
        .fields["at"]
        .clone();
    assert_ne!(stored, lenient["at"], "the date-time has one written form");
    cat.propose(
        &o,
        ProposedWrite::Upsert {
            ty: "e".into(),
            key: "proposed".into(),
            patch: patch(lenient),
            opts: Default::default(),
        },
        "review".into(),
    )
    .await
    .unwrap();
    let p = cat
        .proposals(&o, ProposalFilter::default())
        .await
        .unwrap()
        .data;
    let Some(ProposedWrite::Upsert { patch, .. }) = &p[0].write else {
        panic!("{:?}", p[0].write);
    };
    assert_eq!(patch["at"], stored, "the proposal holds the checked value");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ingest_that_carries_an_authored_field_is_refused_even_unchanged() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let mut amount = field("amount", StorageKind::Decimal);
    amount.authority = Authority::Source;
    cat.declare(
        &o,
        Declaration::Type(ty("tx", vec![amount, field("note", StorageKind::Text)])),
    )
    .await
    .unwrap();
    cat.upsert(
        &o,
        &"tx".into(),
        &"t1".into(),
        patch(json!({"note": "x"})),
        Default::default(),
    )
    .await
    .unwrap();
    let s: lakelands_api::SourceStream = Box::pin(Stream(
        vec![SourceRecord {
            upstream: None,
            ty: "tx".into(),
            key: "t1".into(),
            fields: patch(json!({"note": "x"})),
            seen_at: None,
        }]
        .into_iter(),
    ));
    let mut out = cat.ingest(&o, &"bank".into(), s).await.unwrap();
    let first = std::future::poll_fn(|cx| out.as_mut().poll_next(cx))
        .await
        .expect("an answer");
    assert!(
        matches!(first, Err(Error::Refused { .. })),
        "the source is not the authority on `note`: {first:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_filtered_search_pulls_until_its_page_is_full() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let mut body = field("body", StorageKind::Text);
    body.searchable = true;
    cat.declare(
        &o,
        Declaration::Type(ty("doc", vec![body, field("n", StorageKind::Number)])),
    )
    .await
    .unwrap();
    for n in 0..300 {
        cat.upsert(
            &o,
            &"doc".into(),
            &format!("d{n:03}").into(),
            patch(json!({"body": "apple", "n": n})),
            Default::default(),
        )
        .await
        .unwrap();
    }
    let search = |cursor| Search {
        ty: "doc".into(),
        mode: SearchMode::Lexical {
            text: "apple".into(),
            fields: None,
        },
        k: 3,
        cursor,
        filter: Some(Condition::from("n >= 295")),
        with_meta: false,
        at: ReadAt::default(),
    };
    let first = cat.search(&o, search(None)).await.unwrap().data;
    assert_eq!(
        first.hits.len(),
        3,
        "the matches lie past the first candidates"
    );
    let second = cat
        .search(&o, search(first.next_cursor.clone()))
        .await
        .unwrap()
        .data;
    assert_eq!(second.hits.len(), 2);
    assert!(second.next_cursor.is_none());
}

struct Axis;

#[async_trait::async_trait]
impl lakelands_api::seam::EmbedBackend for Axis {
    fn model(&self) -> &str {
        "axis"
    }
    fn dimensions(&self) -> u32 {
        2
    }
    async fn embed(&self, texts: &[String]) -> lakelands_api::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hybrid_search_embeds_its_text_through_the_bound_backend() {
    let lake = name();
    let store = lakelands_store::open(
        Location::memory(lake.as_str()),
        Seams {
            embed: Some(Arc::new(Axis)),
            ..seams()
        },
        OpenMode::Create,
    )
    .await
    .unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let mut body = field("body", StorageKind::Text);
    body.searchable = true;
    let mut v = field("v", StorageKind::Vector);
    v.ty.dimensions = Some(2);
    cat.declare(&o, Declaration::Type(ty("doc", vec![body, v])))
        .await
        .unwrap();
    cat.upsert(
        &o,
        &"doc".into(),
        &"d1".into(),
        patch(json!({"body": "apple", "v": [1.0, 0.0]})),
        Default::default(),
    )
    .await
    .unwrap();
    let hits = cat
        .search(
            &o,
            Search {
                ty: "doc".into(),
                mode: SearchMode::Hybrid {
                    text: "apple".into(),
                    field: "v".into(),
                    vector: None,
                    fields: None,
                    weight: None,
                    text_weight: None,
                },
                k: 5,
                cursor: None,
                filter: None,
                with_meta: false,
                at: ReadAt::default(),
            },
        )
        .await
        .unwrap()
        .data;
    assert_eq!(hits.ranked_by, "hybrid");
    assert_eq!(hits.hits.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn declare_refuses_unkeyable_keys_and_unchecked_functions() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let invalid = |r: &lakelands_api::Result<Outcome>| {
        matches!(
            r,
            Err(Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            })
        )
    };
    let mut j = ty("j", vec![field("doc", StorageKind::Json)]);
    j.key = vec!["doc".into()];
    let r = cat.declare(&o, Declaration::Type(j)).await;
    assert!(invalid(&r), "a json key field: {r:?}");

    let function = |name: &str, input: FieldDecl, expr: &str| {
        Declaration::Function(FunctionDecl {
            name: name.into(),
            backing: "expr".into(),
            inputs: vec![input],
            expr: Some(Condition::from(expr)),
            config: serde_json::Value::Null,
            description: None,
        })
    };
    let r = cat
        .declare(
            &o,
            function("stray", field("b", StorageKind::Number), "c + 1"),
        )
        .await;
    assert!(invalid(&r), "an expression reading past its inputs: {r:?}");
    cat.declare(
        &o,
        function("next", field("b", StorageKind::Number), "b + 1"),
    )
    .await
    .unwrap();
    let computed = || {
        let mut a = field("a", StorageKind::Number);
        a.function = Some("next".into());
        a.authority = Authority::Derived;
        a
    };
    let mut b = field("b", StorageKind::Number);
    b.derived = Some(Condition::from("a + 1"));
    b.authority = Authority::Derived;
    let r = cat
        .declare(&o, Declaration::Type(ty("cycle", vec![computed(), b])))
        .await;
    assert!(invalid(&r), "a function-computed field in a cycle: {r:?}");
    let r = cat
        .declare(
            &o,
            Declaration::Type(ty("kinds", vec![computed(), field("b", StorageKind::Text)])),
        )
        .await;
    assert!(invalid(&r), "an input of another kind: {r:?}");
    cat.declare(
        &o,
        Declaration::Type(ty(
            "fine",
            vec![computed(), field("b", StorageKind::Number)],
        )),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn retiring_needs_every_field_writable_field_by_field() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    cat.declare(
        &o,
        Declaration::Type(ty(
            "note",
            vec![field("a", StorageKind::Text), field("b", StorageKind::Text)],
        )),
    )
    .await
    .unwrap();
    for k in ["n1", "n2"] {
        cat.upsert(
            &o,
            &"note".into(),
            &k.into(),
            patch(json!({"a": "x", "b": "y"})),
            Default::default(),
        )
        .await
        .unwrap();
    }
    let with = |grants: Grants| Principal {
        kind: PrincipalKind::Agent,
        id: "agent".into(),
        lake: lake.as_str().into(),
        grants,
    };
    let grants: Grants = serde_json::from_value(json!({
        "default": "read",
        "types": {"note": {"fields": {"a": "write", "b": "write"}}}
    }))
    .unwrap();
    cat.retire(
        &with(grants),
        &"note".into(),
        &"n1".into(),
        Default::default(),
    )
    .await
    .expect("every field granted one by one may retire");
    let grants: Grants = serde_json::from_value(json!({
        "default": "read",
        "types": {"note": {"default": "write", "fields": {"b": "read"}}}
    }))
    .unwrap();
    let r = cat
        .retire(
            &with(grants),
            &"note".into(),
            &"n2".into(),
            Default::default(),
        )
        .await;
    assert!(
        matches!(
            r,
            Err(Error::Refused {
                reason: Refusal::NotGranted,
                ..
            })
        ),
        "one field it may only read keeps it from retiring: {r:?}"
    );
}

struct Bank;

#[async_trait::async_trait]
impl lakelands_api::seam::VerbExecutor for Bank {
    fn name(&self) -> &str {
        "bank"
    }
    async fn execute(
        &self,
        invocation: &lakelands_api::seam::VerbInvocation,
    ) -> lakelands_api::Result<serde_json::Value> {
        match invocation.record.as_ref().map(|r| r.key.as_str()) {
            Some("i1") => Ok(json!({"paid": true})),
            _ => Err(Error::Unavailable {
                message: "the bank is closed".into(),
            }),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_external_verb_answers_its_executors_output_or_its_failure() {
    let lake = name();
    let store = lakelands_store::open(
        Location::memory(lake.as_str()),
        Seams {
            executors: vec![Arc::new(Bank)],
            ..seams()
        },
        OpenMode::Create,
    )
    .await
    .unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    for d in invoice_with_verbs() {
        cat.declare(&o, d).await.unwrap();
    }
    for k in ["i1", "i2"] {
        cat.upsert(
            &o,
            &"invoice".into(),
            &k.into(),
            patch(json!({"status": "open"})),
            Default::default(),
        )
        .await
        .unwrap();
    }
    let pay = |k: &'static str| {
        let (cat, o) = (cat.clone(), o.clone());
        async move {
            cat.call(
                &o,
                &"pay".into(),
                Some(&k.into()),
                patch(json!({})),
                Default::default(),
            )
            .await
            .unwrap()
        }
    };
    let Outcome::Applied { output, .. } = pay("i1").await else {
        panic!("a low verb runs")
    };
    assert_eq!(output, Some(json!({"paid": true})));
    let Outcome::Applied {
        position, output, ..
    } = pay("i2").await
    else {
        panic!("a low verb runs")
    };
    let error = output.expect("a failure is the recorded output");
    assert!(
        error["error"]
            .as_str()
            .is_some_and(|e| e.contains("closed")),
        "{error}"
    );
    assert!(position.0 > 0, "the verb's position is real");
    let upserted = cat
        .upsert(
            &o,
            &"invoice".into(),
            &"i3".into(),
            patch(json!({"status": "open"})),
            Default::default(),
        )
        .await
        .unwrap();
    assert!(
        matches!(upserted, Outcome::Applied { output: None, .. }),
        "only an external verb carries an output"
    );
}

/// Counts its calls, and answers the count.
struct Counter(std::sync::atomic::AtomicU32);

#[async_trait::async_trait]
impl lakelands_api::seam::VerbExecutor for Counter {
    fn name(&self) -> &str {
        "bank"
    }
    async fn execute(
        &self,
        _: &lakelands_api::seam::VerbInvocation,
    ) -> lakelands_api::Result<serde_json::Value> {
        let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        Ok(json!({ "call": n }))
    }
}

/// A retry with the same idempotency key answers the first outcome
/// (`WriteOpts::idempotency_key`): for an external verb, the verb's own
/// position and the output recorded for it, even after other writes, and
/// the executor is not called again (lakelands#98).
#[tokio::test(flavor = "multi_thread")]
async fn a_retried_external_verb_answers_its_first_position_and_output() {
    let lake = name();
    let counter = Arc::new(Counter(Default::default()));
    let store = lakelands_store::open(
        Location::memory(lake.as_str()),
        Seams {
            executors: vec![counter.clone()],
            ..seams()
        },
        OpenMode::Create,
    )
    .await
    .unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    for d in invoice_with_verbs() {
        cat.declare(&o, d).await.unwrap();
    }
    let open = |k: &'static str| {
        let (cat, o) = (cat.clone(), o.clone());
        async move {
            cat.upsert(
                &o,
                &"invoice".into(),
                &k.into(),
                patch(json!({"status": "open"})),
                Default::default(),
            )
            .await
            .unwrap()
        }
    };
    open("i1").await;
    let pay = || {
        let (cat, o) = (cat.clone(), o.clone());
        async move {
            cat.call(
                &o,
                &"pay".into(),
                Some(&"i1".into()),
                patch(json!({})),
                WriteOpts {
                    idempotency_key: Some("pay-i1".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
        }
    };
    let first = pay().await;
    let Outcome::Applied {
        position, output, ..
    } = &first
    else {
        panic!("a low verb runs: {first:?}")
    };
    assert_eq!(output, &Some(json!({"call": 1})));
    let feed = store
        .feed()
        .poll(*position, &FeedFilter::default(), 1)
        .await
        .unwrap();
    assert_eq!(
        feed.events[0].body.kind(),
        "verb_called",
        "the first call answers the verb's own position"
    );
    let head = store.head().await.unwrap();
    assert_eq!(pay().await, first, "a retry answers the first outcome");
    assert_eq!(store.head().await.unwrap(), head, "a retry appends nothing");
    // Other writes after the output was recorded: still the first outcome.
    open("i2").await;
    open("i3").await;
    assert_eq!(pay().await, first);
    assert_eq!(
        counter.0.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the executor runs once for the call"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn apply_takes_a_set_whole_and_refuses_duplicates_and_readers() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let mut to = field("to", StorageKind::Reference);
    to.ty.target = Some("b".into());
    let forward = vec![
        Declaration::Type(ty("a", vec![to])),
        Declaration::Type(ty("b", vec![field("n", StorageKind::Number)])),
    ];
    let reader = Principal {
        grants: Grants::default(),
        ..owner(&lake)
    };
    let refused = cat.apply(&reader, forward.clone()).await;
    assert!(
        matches!(
            refused,
            Err(Error::Refused {
                reason: Refusal::NotGranted,
                ..
            })
        ),
        "{refused:?}"
    );
    let twice = cat
        .apply(&o, vec![forward[1].clone(), forward[1].clone()])
        .await;
    assert!(
        matches!(&twice, Err(Error::Refused { reason: Refusal::Invalid { message }, .. }) if message.contains("more than once")),
        "{twice:?}"
    );
    let head = store.head().await.unwrap();
    cat.apply(&o, forward).await.unwrap();
    assert_eq!(store.head().await.unwrap().0, head.0 + 2);
    assert_eq!(store.limits(), lakelands_api::location::Limits::default());
}

struct Later;

impl lakelands_api::seam::SystemVocabulary for Later {
    fn types(&self) -> Vec<TypeDecl> {
        vec![ty("sys.host_note", vec![field("text", StorageKind::Text)])]
    }
    fn may_hold(&self, _: &lakelands_api::LakeId, _: &lakelands_api::TypeName) -> bool {
        true
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_type_added_later_reaches_the_lake_on_its_next_open_for_writing() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    store.close().await.unwrap();
    drop(store);
    let with = Seams {
        vocabulary: Some(Arc::new(Later)),
        ..seams()
    };
    let store = lakelands_store::open(
        Location::memory(lake.as_str()),
        with.clone(),
        OpenMode::Write,
    )
    .await
    .unwrap();
    let o = owner(&lake);
    let cat = store.catalog(WriteContext::default());
    let d = cat.describe(&o, ReadAt::default()).await.unwrap().data;
    assert!(
        d.types.iter().any(|t| t.name.as_str() == "sys.host_note"),
        "the host type was declared on open"
    );
    let head = store.head().await.unwrap();
    store.close().await.unwrap();
    drop((cat, store));
    let again = lakelands_store::open(Location::memory(lake.as_str()), with, OpenMode::Write)
        .await
        .unwrap();
    assert_eq!(
        again.head().await.unwrap().0,
        head.0 + 1,
        "a type already held is not declared again; only the lease is taken"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn proposals_come_in_the_callers_order_and_an_unknown_member_is_refused() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    cat.declare(
        &o,
        Declaration::Type(ty("t", vec![field("n", StorageKind::Number)])),
    )
    .await
    .unwrap();
    for why in ["b", "c", "a"] {
        cat.propose(
            &o,
            ProposedWrite::Upsert {
                ty: "t".into(),
                key: why.into(),
                patch: patch(json!({"n": 1})),
                opts: Default::default(),
            },
            why.into(),
        )
        .await
        .unwrap();
    }
    let list = |order: serde_json::Value| {
        let cat = cat.clone();
        let o = o.clone();
        async move {
            let filter: ProposalFilter =
                serde_json::from_value(json!({ "order_by": order })).unwrap();
            cat.proposals(&o, filter)
                .await
                .map(|r| r.data.into_iter().map(|p| p.why).collect::<Vec<_>>())
        }
    };
    assert_eq!(
        list(json!([])).await.unwrap(),
        ["a", "c", "b"],
        "newest first"
    );
    assert_eq!(
        list(json!([{"field": "why"}])).await.unwrap(),
        ["a", "b", "c"]
    );
    assert_eq!(
        list(json!([{"field": "why", "dir": "desc"}]))
            .await
            .unwrap(),
        ["c", "b", "a"]
    );
    assert!(matches!(
        list(json!([{"field": "colour"}])).await,
        Err(Error::Refused {
            reason: Refusal::Invalid { .. },
            ..
        })
    ));
}

struct Echo;

#[async_trait::async_trait]
impl lakelands_api::seam::VerbExecutor for Echo {
    fn name(&self) -> &str {
        "bank"
    }
    async fn execute(
        &self,
        invocation: &lakelands_api::seam::VerbInvocation,
    ) -> lakelands_api::Result<serde_json::Value> {
        let record = invocation.record.as_ref().expect("a verb on a record");
        Ok(json!({
            "fields": record.fields,
            "masked": record.masked,
        }))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_executor_receives_the_record_unmasked() {
    let lake = name();
    let store = lakelands_store::open(
        Location::memory(lake.as_str()),
        Seams {
            executors: vec![Arc::new(Echo)],
            ..seams()
        },
        OpenMode::Create,
    )
    .await
    .unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    for d in invoice_with_verbs() {
        cat.declare(&o, d).await.unwrap();
    }
    cat.upsert(
        &o,
        &"invoice".into(),
        &"i1".into(),
        patch(json!({"status": "open"})),
        Default::default(),
    )
    .await
    .unwrap();
    let caller = Principal {
        kind: PrincipalKind::Agent,
        id: "agent".into(),
        lake: lake.as_str().into(),
        grants: serde_json::from_value(json!({
            "default": "none",
            "types": {"invoice": {"default": "write", "fields": {"status": "none"}}}
        }))
        .unwrap(),
    };
    let Outcome::Applied { output, .. } = cat
        .call(
            &caller,
            &"pay".into(),
            Some(&"i1".into()),
            patch(json!({})),
            Default::default(),
        )
        .await
        .unwrap()
    else {
        panic!("a low verb runs")
    };
    let output = output.expect("the executor's output");
    assert_eq!(output["fields"]["status"], json!("open"), "{output}");
    assert_eq!(output["masked"], json!([]), "{output}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declaration_names_every_tool_name_problem_at_once() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let long = "a".repeat(60);
    let r = cat
        .declare(&owner(&lake), Declaration::Type(ty(&long, vec![])))
        .await;
    let Err(Error::Refused {
        reason: Refusal::Invalid { message },
        ..
    }) = r
    else {
        panic!("{r:?}")
    };
    assert!(
        message.matches("is longer than").count() > 1,
        "every too-long tool name is named: {message}"
    );
}

fn reference(name: &str, target: &str) -> FieldDecl {
    let mut f = field(name, StorageKind::Reference);
    f.ty.target = Some(target.into());
    f
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reference_must_name_a_live_record_or_one_earlier_in_the_window() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let mut partner = field("name", StorageKind::Text);
    partner.authority = Authority::Shared;
    let mut to = reference("partner", "partner");
    to.authority = Authority::Shared;
    cat.apply(
        &o,
        vec![
            Declaration::Type(ty("invoice", vec![to])),
            Declaration::Type(ty("partner", vec![partner])),
        ],
    )
    .await
    .unwrap();
    let head = store.head().await.unwrap();
    let r = cat
        .upsert(
            &o,
            &"invoice".into(),
            &"i".into(),
            patch(json!({"partner": "nobody"})),
            Default::default(),
        )
        .await;
    assert!(
        matches!(&r, Err(Error::Refused { field: Some(f), .. }) if f.as_str() == "partner"),
        "{r:?}"
    );
    assert_eq!(store.head().await.unwrap(), head);
    let records = vec![
        SourceRecord {
            upstream: None,
            ty: "partner".into(),
            key: "acme".into(),
            fields: patch(json!({"name": "Acme"})),
            seen_at: None,
        },
        SourceRecord {
            upstream: None,
            ty: "invoice".into(),
            key: "i".into(),
            fields: patch(json!({"partner": "acme"})),
            seen_at: None,
        },
    ];
    let s: lakelands_api::SourceStream = Box::pin(Stream(records.into_iter()));
    let mut out = cat.ingest(&o, &"crm".into(), s).await.unwrap();
    let first = std::future::poll_fn(|cx| out.as_mut().poll_next(cx))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first.records, 2,
        "a reference to a record earlier in the window resolves"
    );
    cat.retire(&o, &"partner".into(), &"acme".into(), Default::default())
        .await
        .expect("retiring a referenced record stays allowed");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redeclared_function_recomputes_its_fields_naming_its_version() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let function = |expr: &str| {
        Declaration::Function(FunctionDecl {
            name: "scaled".into(),
            backing: "expr".into(),
            inputs: vec![field("net", StorageKind::Decimal)],
            expr: Some(Condition::from(expr)),
            config: serde_json::Value::Null,
            description: None,
        })
    };
    cat.declare(&o, function("net * 2")).await.unwrap();
    let mut scaled = field("scaled", StorageKind::Decimal);
    scaled.function = Some("scaled".into());
    scaled.authority = Authority::Derived;
    cat.declare(
        &o,
        Declaration::Type(ty("bill", vec![field("net", StorageKind::Decimal), scaled])),
    )
    .await
    .unwrap();
    cat.upsert(
        &o,
        &"bill".into(),
        &"b".into(),
        patch(json!({"net": "3"})),
        Default::default(),
    )
    .await
    .unwrap();
    let read = || async {
        let r = cat
            .get(
                &o,
                &"bill".into(),
                Get {
                    key: "b".into(),
                    with_meta: true,
                    at: ReadAt::default(),
                    params: Default::default(),
                },
            )
            .await
            .unwrap()
            .data;
        (
            r.fields["scaled"].clone(),
            r.meta["scaled"].procedure_version.clone(),
            r.version,
        )
    };
    assert_eq!(
        read().await,
        (json!("6"), Some("1".into()), lakelands_api::Version(1))
    );
    let redeclared = cat.declare(&o, function("net * 3")).await.unwrap();
    let (value, procedure, version) = read().await;
    assert_eq!((value, procedure), (json!("9"), Some("2".into())));
    assert_eq!(
        version,
        lakelands_api::Version(1),
        "no write touched the record"
    );
    store.rebuild().await.unwrap();
    assert_eq!(
        read().await.0,
        json!("9"),
        "a replay recomputes it the same way"
    );
    let _ = redeclared;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_from_before_a_retirement_never_applies_after_it() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    cat.declare(
        &o,
        Declaration::Type(ty("c", vec![field("n", StorageKind::Number)])),
    )
    .await
    .unwrap();
    let expecting = |v: u64| lakelands_api::write::WriteOpts {
        expected_version: Some(lakelands_api::Version(v)),
        idempotency_key: None,
    };
    cat.upsert(
        &o,
        &"c".into(),
        &"k".into(),
        patch(json!({"n": 1})),
        Default::default(),
    )
    .await
    .unwrap();
    cat.retire(&o, &"c".into(), &"k".into(), Default::default())
        .await
        .unwrap();
    let r = cat
        .upsert(
            &o,
            &"c".into(),
            &"k".into(),
            patch(json!({"n": 5})),
            expecting(1),
        )
        .await;
    assert_eq!(
        r,
        Err(Error::Conflict {
            expected: lakelands_api::Version(1),
            actual: lakelands_api::Version(2)
        })
    );
    let again = cat
        .upsert(
            &o,
            &"c".into(),
            &"k".into(),
            patch(json!({"n": 3})),
            expecting(2),
        )
        .await
        .unwrap();
    assert!(matches!(
        again,
        Outcome::Applied {
            version: Some(lakelands_api::Version(3)),
            ..
        }
    ));
    let fresh = cat
        .upsert(
            &o,
            &"c".into(),
            &"new".into(),
            patch(json!({"n": 1})),
            expecting(0),
        )
        .await;
    assert!(
        fresh.is_ok(),
        "a key never written is at version 0: {fresh:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sum_over_nothing_carries_the_fields_scale() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let money: Declaration = serde_json::from_value(json!({
        "declare": "value_type", "name": "money", "kind": "decimal", "scale": 2
    }))
    .unwrap();
    cat.declare(&o, money).await.unwrap();
    let mut amount = field("amount", StorageKind::Decimal);
    amount.ty.value_type = Some("money".into());
    cat.declare(&o, Declaration::Type(ty("payment", vec![amount])))
        .await
        .unwrap();
    let find: lakelands_api::Find = serde_json::from_value(json!({
        "aggregate": {"measures": [{"name": "total", "func": "sum", "field": "amount"}]}
    }))
    .unwrap();
    let found = cat.find(&o, &"payment".into(), find).await.unwrap().data;
    let lakelands_api::Found::Groups { rows } = found else {
        panic!("groups")
    };
    assert_eq!(rows[0].values["total"], json!("0.00"));
}

#[tokio::test(flavor = "multi_thread")]
async fn search_finds_a_restricted_record_only_with_the_grant() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let mut body = field("body", StorageKind::Text);
    body.searchable = true;
    cat.declare(&o, Declaration::Type(ty("doc", vec![body])))
        .await
        .unwrap();
    cat.upsert(
        &o,
        &"doc".into(),
        &"d".into(),
        patch(json!({"body": "apple"})),
        Default::default(),
    )
    .await
    .unwrap();
    store
        .restrict(&"doc".into(), &"d".into(), "hold".into())
        .await
        .unwrap();
    let search = || Search {
        ty: "doc".into(),
        mode: SearchMode::Lexical {
            text: "apple".into(),
            fields: None,
        },
        k: 5,
        cursor: None,
        filter: None,
        with_meta: false,
        at: ReadAt::default(),
    };
    let with = cat.search(&o, search()).await.unwrap().data;
    assert_eq!(with.hits.len(), 1, "the restricted grant finds it");
    let reader = Principal {
        grants: serde_json::from_value(json!({"default": "read"})).unwrap(),
        ..owner(&lake)
    };
    let without = cat.search(&reader, search()).await.unwrap().data;
    assert!(without.hits.is_empty(), "without the grant it is withheld");
}

#[tokio::test(flavor = "multi_thread")]
async fn numbers_and_vectors_read_in_their_one_json_form_live_and_after_a_rebuild() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let mut v = field("v", StorageKind::Vector);
    v.ty.dimensions = Some(2);
    cat.declare(
        &o,
        Declaration::Type(ty("p", vec![field("n", StorageKind::Number), v])),
    )
    .await
    .unwrap();
    cat.upsert(
        &o,
        &"p".into(),
        &"a".into(),
        patch(json!({"n": 2.0, "v": [0.0, 1.0]})),
        Default::default(),
    )
    .await
    .unwrap();
    let read = || async {
        cat.get(
            &o,
            &"p".into(),
            Get {
                key: "a".into(),
                with_meta: false,
                at: ReadAt::default(),
                params: Default::default(),
            },
        )
        .await
        .unwrap()
        .data
        .fields
    };
    let live = read().await;
    assert_eq!((&live["n"], &live["v"]), (&json!(2), &json!([0, 1])));
    store.rebuild().await.unwrap();
    assert_eq!(read().await, live);
}

/// An embedding backend that keeps how many texts each call carried, and
/// fails every call when told to.
struct Counting {
    model: &'static str,
    dimensions: u32,
    fail: bool,
    calls: std::sync::Mutex<Vec<usize>>,
}

impl Counting {
    fn new(model: &'static str, fail: bool) -> Arc<Counting> {
        Counting::sized(model, 2, fail)
    }

    fn sized(model: &'static str, dimensions: u32, fail: bool) -> Arc<Counting> {
        Arc::new(Counting {
            model,
            dimensions,
            fail,
            calls: Default::default(),
        })
    }
}

#[async_trait::async_trait]
impl lakelands_api::seam::EmbedBackend for Counting {
    fn model(&self) -> &str {
        self.model
    }
    fn dimensions(&self) -> u32 {
        self.dimensions
    }
    async fn embed(&self, texts: &[String]) -> lakelands_api::Result<Vec<Vec<f32>>> {
        self.calls.lock().unwrap().push(texts.len());
        if self.fail {
            return Err(Error::Unavailable {
                message: "the endpoint timed out".into(),
            });
        }
        Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
    }
}

async fn open_with(lake: &str, embed: Arc<Counting>, mode: OpenMode) -> Arc<dyn LakeStore> {
    lakelands_store::open(
        Location::memory(lake),
        Seams {
            embed: Some(embed),
            ..seams()
        },
        mode,
    )
    .await
    .unwrap()
}

fn embedding(model: &str) -> FieldDecl {
    let mut v = field("vec", StorageKind::Vector);
    v.ty.dimensions = Some(2);
    v.authority = Authority::Derived;
    v.embed = Some(lakelands_api::ontology::EmbedDecl {
        from: vec!["body".into()],
        model: model.into(),
    });
    v
}

/// Seventy records with sixty-nine distinct texts, then an embedding
/// declared on their type: the backfill sends each distinct text once, at
/// most `EMBED_BATCH` to a request, and fills every record.
#[tokio::test(flavor = "multi_thread")]
async fn a_backfill_sends_each_distinct_text_once_in_batches_of_at_most_sixty_four() {
    let lake = name();
    let backend = Counting::new("m", false);
    let store = open_with(&lake, backend.clone(), OpenMode::Create).await;
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    cat.declare(
        &o,
        Declaration::Type(ty("doc", vec![field("body", StorageKind::Text)])),
    )
    .await
    .unwrap();
    for i in 0..70 {
        let text = format!("text {}", i.min(68));
        cat.upsert(
            &o,
            &"doc".into(),
            &format!("d{i}").as_str().into(),
            patch(json!({ "body": text })),
            Default::default(),
        )
        .await
        .unwrap();
    }
    cat.add_field(&o, &"doc".into(), embedding("m"))
        .await
        .unwrap();
    assert_eq!(*backend.calls.lock().unwrap(), [64, 5]);
    for i in [0, 63, 68, 69] {
        let r = cat
            .get(&o, &"doc".into(), get(&format!("d{i}")))
            .await
            .unwrap()
            .data;
        assert!(r.fields.contains_key("vec"), "d{i}: {r:?}");
    }
}

/// A request that fails records a failure for each text it carried.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_request_records_a_failure_for_each_text() {
    let lake = name();
    let backend = Counting::new("m", true);
    let store = open_with(&lake, backend.clone(), OpenMode::Create).await;
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    cat.declare(
        &o,
        Declaration::Type(ty("doc", vec![field("body", StorageKind::Text)])),
    )
    .await
    .unwrap();
    for (k, text) in [("a", "one"), ("b", "two")] {
        cat.upsert(
            &o,
            &"doc".into(),
            &k.into(),
            patch(json!({ "body": text })),
            Default::default(),
        )
        .await
        .unwrap();
    }
    cat.add_field(&o, &"doc".into(), embedding("m"))
        .await
        .unwrap();
    assert_eq!(*backend.calls.lock().unwrap(), [2]);
    for k in ["a", "b"] {
        let r = cat.get(&o, &"doc".into(), get(k)).await.unwrap().data;
        let why = r.meta.get("vec").and_then(|m| m.error.clone());
        assert!(
            !r.fields.contains_key("vec") && why.as_deref() == Some("the endpoint timed out"),
            "{k}: {r:?}"
        );
    }
}

/// A lake whose embedding was declared with one model, opened again with
/// a backend of another: a hybrid search over the field is `unbound`,
/// naming both models, and a due embedding waits instead of being made.
#[tokio::test(flavor = "multi_thread")]
async fn another_model_bound_later_is_unbound_for_search_and_makes_no_embedding() {
    let lake = name();
    let first = open_with(&lake, Counting::new("m", false), OpenMode::Create).await;
    let cat = first.catalog(WriteContext::default());
    let o = owner(&lake);
    let mut body = field("body", StorageKind::Text);
    body.searchable = true;
    cat.declare(&o, Declaration::Type(ty("doc", vec![body, embedding("m")])))
        .await
        .unwrap();
    first.close().await.unwrap();

    let other = Counting::new("n", false);
    let store = open_with(&lake, other.clone(), OpenMode::Write).await;
    let cat = store.catalog(WriteContext::default());
    cat.upsert(
        &o,
        &"doc".into(),
        &"d".into(),
        patch(json!({ "body": "apple" })),
        Default::default(),
    )
    .await
    .unwrap();
    let r = cat.get(&o, &"doc".into(), get("d")).await.unwrap().data;
    assert!(!r.fields.contains_key("vec"), "{r:?}");
    assert!(other.calls.lock().unwrap().is_empty());
    let got = cat
        .search(
            &o,
            Search {
                ty: "doc".into(),
                mode: SearchMode::Hybrid {
                    text: "apple".into(),
                    field: "vec".into(),
                    vector: None,
                    fields: None,
                    weight: None,
                    text_weight: None,
                },
                k: 5,
                cursor: None,
                filter: None,
                with_meta: false,
                at: ReadAt::default(),
            },
        )
        .await;
    match got {
        Err(Error::Unbound { seam }) => {
            assert!(seam.contains("`m`") && seam.contains("`n`"), "{seam}");
        }
        other => panic!("expected unbound, got {other:?}"),
    }
    store.close().await.unwrap();

    // The same model making vectors of other dimensions: unbound too.
    let wider = Counting::sized("m", 3, false);
    let store = open_with(&lake, wider.clone(), OpenMode::Write).await;
    let cat = store.catalog(WriteContext::default());
    let got = cat
        .search(
            &o,
            Search {
                ty: "doc".into(),
                mode: SearchMode::Hybrid {
                    text: "apple".into(),
                    field: "vec".into(),
                    vector: None,
                    fields: None,
                    weight: None,
                    text_weight: None,
                },
                k: 5,
                cursor: None,
                filter: None,
                with_meta: false,
                at: ReadAt::default(),
            },
        )
        .await;
    assert!(
        matches!(&got, Err(Error::Unbound { seam }) if seam.contains("2 dimensions") && seam.contains("with 3")),
        "{got:?}"
    );
    assert!(wider.calls.lock().unwrap().is_empty());
}

/// A redeclaration that touches nothing an embedding reads rewrites no
/// record: the vector, its position and the version in its metadata stay
/// those that produced it.
#[tokio::test(flavor = "multi_thread")]
async fn a_redeclaration_touching_nothing_rewrites_no_embedded_record() {
    let lake = name();
    let backend = Counting::new("m", false);
    let store = open_with(&lake, backend.clone(), OpenMode::Create).await;
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let decl = ty(
        "doc",
        vec![field("body", StorageKind::Text), embedding("m")],
    );
    cat.declare(&o, Declaration::Type(decl.clone()))
        .await
        .unwrap();
    cat.upsert(
        &o,
        &"doc".into(),
        &"d".into(),
        patch(json!({ "body": "apple" })),
        Default::default(),
    )
    .await
    .unwrap();
    let before = cat.get(&o, &"doc".into(), get("d")).await.unwrap().data;
    let mut described = decl;
    described.description = Some("Documents.".into());
    cat.declare(&o, Declaration::Type(described)).await.unwrap();
    let after = cat.get(&o, &"doc".into(), get("d")).await.unwrap().data;
    assert_eq!(after.fields, before.fields);
    assert_eq!(after.meta.get("vec"), before.meta.get("vec"));
    assert_eq!(
        before.meta["vec"].procedure_version.as_deref(),
        Some("1"),
        "the version that declared the embedding"
    );
    assert_eq!(*backend.calls.lock().unwrap(), [1]);
    // Describe shows what the vector is made from, and with which model.
    let d = cat
        .describe_type(&o, &"doc".into(), ReadAt::default())
        .await
        .unwrap()
        .data;
    let vec = d.fields.iter().find(|f| f.name.as_str() == "vec").unwrap();
    let embed = vec
        .embed
        .as_ref()
        .expect("an embedding describes its source");
    assert_eq!((embed.from.len(), embed.model.as_str()), (1, "m"));
}

fn get(key: &str) -> Get {
    Get {
        key: key.into(),
        with_meta: true,
        at: ReadAt::default(),
        params: Default::default(),
    }
}

/// An embedding is a derived vector made from other text fields of its
/// type, sensitive where any of them is; anything else is refused as
/// invalid, naming the field.
#[tokio::test(flavor = "multi_thread")]
async fn an_embedding_declared_wrongly_is_refused_naming_the_field() {
    let lake = name();
    let store = open_with(&lake, Counting::new("m", false), OpenMode::Create).await;
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    let with = |from: &[&str], edit: fn(&mut FieldDecl)| {
        let mut v = embedding("m");
        v.embed.as_mut().unwrap().from = from.iter().map(|f| (*f).into()).collect();
        edit(&mut v);
        let mut secret = field("secret", StorageKind::Text);
        secret.sensitive = true;
        ty(
            "doc",
            vec![
                field("body", StorageKind::Text),
                field("n", StorageKind::Number),
                secret,
                v,
            ],
        )
    };
    let cases: Vec<(&str, TypeDecl)> = vec![
        ("not text", with(&["n"], |_| {})),
        ("not one of its fields", with(&["nowhere"], |_| {})),
        ("from itself", with(&["vec"], |_| {})),
        ("twice", with(&["body", "body"], |_| {})),
        ("names the fields", with(&[], |_| {})),
        ("sensitive", with(&["secret"], |_| {})),
        (
            "authority",
            with(&["body"], |v| v.authority = Authority::Authored),
        ),
        (
            "`depends_on`",
            with(&["body"], |v| v.depends_on = vec!["body".into()]),
        ),
        (
            "only a vector",
            with(&["body"], |v| {
                v.ty = FieldType::kind(StorageKind::Text);
            }),
        ),
        (
            "only one",
            with(&["body"], |v| v.derived = Some(Condition::from("body"))),
        ),
    ];
    for (why, decl) in cases {
        let got = cat.declare(&o, Declaration::Type(decl)).await;
        match got {
            Err(Error::Refused {
                field: Some(f),
                reason: Refusal::Invalid { message },
            }) => {
                assert_eq!(f.as_str(), "vec", "{why}: {message}");
                assert!(message.contains(why), "{why}: {message}");
            }
            other => panic!("{why}: expected invalid, got {other:?}"),
        }
    }
    // Sensitive where its source is: accepted.
    let ok = with(&["secret"], |v| v.sensitive = true);
    cat.declare(&o, Declaration::Type(ok)).await.unwrap();
}

/// An executor that waits at a gate: it says when it has been called, and
/// answers only once released.
struct Gate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    calls: std::sync::atomic::AtomicU32,
}

#[async_trait::async_trait]
impl lakelands_api::seam::VerbExecutor for Gate {
    fn name(&self) -> &str {
        "bank"
    }
    async fn execute(
        &self,
        _: &lakelands_api::seam::VerbInvocation,
    ) -> lakelands_api::Result<serde_json::Value> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        Ok(json!({ "paid": true }))
    }
}

/// A retry looks for the call's output among the 4,096 events after the
/// call; an output recorded further on is answered as none, the position
/// stands, and nothing runs again (`docs/seams.md`, `VerbExecutor`). The
/// reach is the store's constant (`RETRY_REACH` in `writes.rs`), the same
/// on every engine, which is why this is a store test on the memory
/// engine. A store that blocks fails the test at the time limit instead of
/// hanging it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retry_past_the_reach_answers_the_position_without_the_output() {
    tokio::time::timeout(std::time::Duration::from_secs(120), retry_past_the_reach())
        .await
        .expect("the retry past the reach finishes within two minutes");
}

async fn retry_past_the_reach() {
    let lake = name();
    let gate = Arc::new(Gate {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        calls: Default::default(),
    });
    let store = lakelands_store::open(
        Location::memory(lake.as_str()),
        Seams {
            executors: vec![gate.clone()],
            ..seams()
        },
        OpenMode::Create,
    )
    .await
    .unwrap();
    let cat = store.catalog(WriteContext::default());
    let o = owner(&lake);
    for d in invoice_with_verbs() {
        cat.declare(&o, d).await.unwrap();
    }
    for k in ["i1", "busy"] {
        cat.upsert(
            &o,
            &"invoice".into(),
            &k.into(),
            patch(json!({"status": "open"})),
            Default::default(),
        )
        .await
        .unwrap();
    }
    let key = WriteOpts {
        idempotency_key: Some("pay-far".into()),
        ..Default::default()
    };
    let first = {
        let (cat, o, key) = (cat.clone(), o.clone(), key.clone());
        tokio::spawn(async move {
            cat.call(&o, &"pay".into(), Some(&"i1".into()), patch(json!({})), key)
                .await
                .unwrap()
        })
    };
    gate.entered.notified().await;
    // The lake takes more than the reach of other events while the
    // executor runs, so its output lands past it.
    for n in 0..4_100 {
        cat.upsert(
            &o,
            &"invoice".into(),
            &"busy".into(),
            patch(json!({"status": format!("s{n}")})),
            Default::default(),
        )
        .await
        .unwrap();
    }
    gate.release.notify_one();
    let first = first.await.unwrap();
    let Outcome::Applied {
        position, output, ..
    } = &first
    else {
        panic!("a low verb runs: {first:?}")
    };
    assert_eq!(
        output,
        &Some(json!({"paid": true})),
        "the call answers its output"
    );
    let again = cat
        .call(&o, &"pay".into(), Some(&"i1".into()), patch(json!({})), key)
        .await
        .unwrap();
    assert!(
        matches!(&again, Outcome::Applied { position: p, output: None, .. } if p == position),
        "past the reach, a retry answers the call's position {position} with no output: \
         {again:?}"
    );
    assert_eq!(
        gate.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the retry runs nothing again"
    );
}

/// The log format's negative vector, a `known_at` going back at position
/// 6, handed to an engine as it stands and verified by the store:
/// `verify` refuses it at that position (`docs/log-format.md` §1.3), as
/// `export::verify` and `verify.py` do over its files.
#[tokio::test(flavor = "multi_thread")]
async fn verify_refuses_a_known_at_that_goes_back_naming_its_position() {
    use lakelands_api::engine::CanonicalEvent;
    use lakelands_api::event::{Digest, EventId};

    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/log-format/vectors/negative/known-at-goes-back/log")
        .join(format!("{:020}.jsonl", 0));
    let text = std::fs::read_to_string(&path).unwrap();
    let events: Vec<CanonicalEvent> = text
        .lines()
        .map(|line| {
            let e: serde_json::Value = serde_json::from_str(line).unwrap();
            let s = |m: &str| e[m].as_str().unwrap().to_owned();
            CanonicalEvent {
                position: Position(e["position"].as_u64().unwrap()),
                id: EventId(s("id")),
                prev: Digest(s("prev")),
                hash: Digest(s("hash")),
                fence: e["fence"].as_u64().unwrap(),
                bytes: line.as_bytes().to_vec(),
            }
        })
        .collect();
    // The vectors' lake is named `vectors`; no other test uses the name.
    let engine = opener()
        .open(&Location::memory("vectors"), OpenMode::Create)
        .unwrap();
    let lease = engine.take_lease("vectors").unwrap();
    assert_eq!(lease.fence, 1, "the vector's events carry fence 1");
    engine.append(&lease, &events).unwrap();
    engine.release_lease(&lease).unwrap();
    drop(engine);

    let store = open("vectors", OpenMode::Read).await.unwrap();
    match store.verify(None, None).await {
        Err(Error::Unavailable { message }) => assert!(
            message.contains("at position 6") && message.contains("known_at"),
            "{message}"
        ),
        other => {
            panic!("a known_at going back is refused at position 6, and it answered {other:?}")
        }
    }
}

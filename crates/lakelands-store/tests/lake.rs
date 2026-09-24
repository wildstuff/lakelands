//! The store over the in-memory engine: what the conformance suite does
//! not reach through the doors — the lease across opens, verification,
//! checkpoints, the change feed, verbs by risk, ingest's windows,
//! migrations over history, search paging and embedding, and the
//! declarations the door refuses.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::event::EventBody;
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
    Refusal, SourceRecord, WriteContext,
};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

fn name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("store-test-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(MemoryOpener)],
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
    }
}

fn ty(name: &str, fields: Vec<FieldDecl>) -> TypeDecl {
    TypeDecl {
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

#[tokio::test(flavor = "multi_thread")]
async fn a_new_lake_starts_with_its_creation_its_lease_and_its_settings_type() {
    let lake = name();
    let store = open(&lake, OpenMode::Create).await.unwrap();
    let feed = store.feed();
    let batch = feed
        .poll(Position(0), &FeedFilter::default(), 10)
        .await
        .unwrap();
    let kinds: Vec<&str> = batch.events.iter().map(|e| e.body.kind()).collect();
    assert_eq!(kinds, ["lake_created", "lease_taken", "declared"]);
    assert_eq!(
        batch.events[0].fence, 0,
        "position 0 is written before any lease"
    );
    assert_eq!(batch.events[1].fence, 1);
    let v = store.verify(None, None).await.unwrap();
    assert_eq!(v.through, Position(2));
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
            &"i1".into(),
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
            &"i1".into(),
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
            &"i1".into(),
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
    let mut linked = ty("a", vec![]);
    linked.relations.push(RelationDecl {
        name: "to_a".into(),
        target: "a".into(),
        cardinality: Default::default(),
        inverse: None,
        via: None,
        fields: vec![],
        description: None,
    });
    assert!(matches!(
        cat.declare(&o, Declaration::Type(linked)).await,
        Err(Error::Refused {
            reason: Refusal::Invalid { .. },
            ..
        })
    ));
    cat.declare(
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
    .await
    .unwrap();
    let mut computed = field("score", StorageKind::Number);
    computed.function = Some("remote".into());
    computed.authority = Authority::Derived;
    let r = cat
        .declare(&o, Declaration::Type(ty("b", vec![computed])))
        .await;
    assert!(matches!(r, Err(Error::Unbound { .. })), "{r:?}");
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

#[tokio::test(flavor = "multi_thread")]
async fn a_tampered_checkpoint_is_reported_by_verify() {
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

    // A writer that is not the store appends a checkpoint that does not
    // follow the segment it seals.
    let engine = MemoryOpener
        .open(&Location::memory(lake.as_str()), OpenMode::Write)
        .unwrap();
    let lease = engine.take_lease("tamperer").unwrap();
    let (tip, hash) = engine.tip().unwrap().unwrap();
    engine.seal_segment(&lease).unwrap().unwrap();
    let draft = lakelands_store::log::Draft::system(
        lakelands_api::Timestamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros() as i64,
        ),
        EventBody::Checkpoint {
            through: Position(tip.0 - 1),
            hash: hash.clone(),
            segments: engine.segments().unwrap(),
        },
    );
    let (_, canon) =
        lakelands_store::log::seal(&lake.as_str().into(), tip.next(), &hash, lease.fence, draft);
    engine.append(&lease, &[canon]).unwrap();
    engine.release_lease(&lease).unwrap();
    drop(engine);

    let reader = open(&lake, OpenMode::Read).await.unwrap();
    let r = reader.verify(None, None).await;
    assert!(
        matches!(&r, Err(Error::Unavailable { message }) if message.contains("checkpoint")),
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
    let ProposedWrite::Upsert { patch, .. } = &p[0].write else {
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
        match invocation.record.key.as_str() {
            "i1" => Ok(json!({"paid": true})),
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
                &k.into(),
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

//! Walk 1's question on the real store over the in-memory engine, with
//! the answers `docs/walks/walk-1.md` derives by hand from the CSV files.
//!
//! The walk itself runs the CLI and the MCP server from a fresh clone;
//! this test asks the same `invoice_find` and `partner_get` questions
//! through the catalog, so that a wrong expectation in the script, or a
//! store that answers differently from it, shows up before the walk does.
//! The CSV files are read the way `lakelands ingest --map` reads them:
//! every column the map names becomes a text value, which the store
//! normalises by the field's kind.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::read::{AggFunc, AggMeasure, Aggregate, Find, Found};
use lakelands_api::seam::Seams;
use lakelands_api::{
    Catalog, Get, Grants, Location, Position, Principal, PrincipalKind, ReadAt, SourceRecord,
    Timestamp, WriteContext,
};
use lakelands_engine_mem::MemoryOpener;
use serde_json::Value;

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/liquidity")
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

/// The records of one of the example's CSV files through its map, as
/// the CLI makes them (the files hold no quoted cells).
fn records(csv: &str, map: &str) -> Vec<SourceRecord> {
    let data = example().join("data");
    let map: Value =
        serde_json::from_str(&std::fs::read_to_string(data.join(map)).unwrap()).unwrap();
    let text = std::fs::read_to_string(data.join(csv)).unwrap();
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    let col = |name: &str| header.iter().position(|h| *h == name).unwrap();
    lines
        .map(|line| {
            let cells: Vec<&str> = line.split(',').collect();
            let fields = map["fields"]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(c, _)| !cells[col(c)].is_empty())
                .map(|(c, f)| {
                    (
                        f.as_str().unwrap().into(),
                        Value::String(cells[col(c)].into()),
                    )
                })
                .collect();
            SourceRecord {
                upstream: None,
                ty: map["type"].as_str().unwrap().into(),
                key: cells[col(map["key"].as_str().unwrap())].into(),
                fields,
                seen_at: Some(
                    Timestamp::parse(cells[col(map["seen_at"].as_str().unwrap())]).unwrap(),
                ),
            }
        })
        .collect()
}

/// Ingest a file; the position its last commit window went through.
async fn ingest(
    cat: &Arc<dyn Catalog>,
    who: &Principal,
    source: &str,
    rs: Vec<SourceRecord>,
) -> (u32, u32, Position) {
    let s: lakelands_api::SourceStream = Box::pin(Stream(rs.into_iter()));
    let mut out = cat.ingest(who, &source.into(), s).await.unwrap();
    let (mut records, mut duplicates, mut through) = (0, 0, Position(0));
    while let Some(w) = std::future::poll_fn(|cx| out.as_mut().poll_next(cx)).await {
        let w = w.unwrap();
        records += w.records;
        duplicates += w.duplicates;
        through = w.position;
    }
    (records, duplicates, through)
}

/// The walk's `invoice_find`: open invoices more than 30 days past due,
/// gross summed per partner.
fn overdue_by_partner(as_of: Option<Position>) -> Find {
    Find {
        filter: Some("status = 'open' and due_on < now() - 30d".into()),
        aggregate: Some(Aggregate {
            measures: vec![AggMeasure {
                name: "overdue_gross".into(),
                func: AggFunc::Sum,
                field: Some("gross".into()),
            }],
            group_by: vec!["partner".into()],
        }),
        at: ReadAt {
            as_of,
            valid_at: None,
        },
        ..Find::default()
    }
}

/// A `money` value as the store answered it, in cents. `money` is kept at
/// two fractional digits (`scale: 2`), so the answer is a string with
/// exactly two, whatever the arithmetic that produced it.
fn cents(v: &Value) -> i64 {
    let Value::String(s) = v else {
        panic!("a money value is a string of digits, not {v}");
    };
    let (int, frac) = s
        .split_once('.')
        .unwrap_or_else(|| panic!("{s} has no fractional digits"));
    assert_eq!(frac.len(), 2, "{s} is not kept at money's scale of two");
    int.parse::<i64>().unwrap() * 100 + frac.parse::<i64>().unwrap()
}

/// The walk's answer at one position: every debtor's overdue gross and
/// credit limit, in cents, and whether it is over the limit.
async fn answer(
    cat: &Arc<dyn Catalog>,
    who: &Principal,
    as_of: Option<Position>,
) -> BTreeMap<String, (i64, i64, bool)> {
    let found = cat
        .find(who, &"invoice".into(), overdue_by_partner(as_of))
        .await
        .unwrap()
        .data;
    let Found::Groups { rows } = found else {
        panic!("an aggregate find answers groups, not {found:?}");
    };
    let mut out = BTreeMap::new();
    for g in rows {
        let partner = g.by["partner"].as_str().unwrap().to_owned();
        let owed = cents(&g.values["overdue_gross"]);
        let p = cat
            .get(
                who,
                &"partner".into(),
                Get {
                    key: partner.as_str().into(),
                    with_meta: true,
                    at: ReadAt {
                        as_of,
                        valid_at: None,
                    },
                },
            )
            .await
            .unwrap()
            .data;
        let limit = cents(&p.fields["credit_limit"]);
        let meta = &p.meta["credit_limit"];
        assert_eq!(meta.source.as_ref().map(|s| s.as_str()), Some("crm"));
        assert_eq!(
            meta.seen_at.map(|t| t.to_string()).as_deref(),
            Some("2026-08-31T18:00:00.000000Z")
        );
        out.insert(partner, (owed, limit, owed > limit));
    }
    out
}

/// A fresh lake over the in-memory engine, its catalog and its owner.
async fn lake(name: &str) -> (Arc<dyn Catalog>, Principal) {
    let store = lakelands_store::open(
        Location::memory(name),
        Seams {
            engines: vec![Arc::new(MemoryOpener::new())],
            ..Seams::default()
        },
        OpenMode::Create,
    )
    .await
    .unwrap();
    let who = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: name.into(),
        grants: Grants::owner(),
    };
    (store.catalog(WriteContext::default()), who)
}

/// The rift found before the walk (`docs/walks/walk-1.md`): the loader
/// emits types by name, `invoice` before the `partner` it refers to, and
/// `declare` refuses a reference to a type not declared yet. It keeps
/// refusing once `Catalog::apply` exists; `apply` is what declares a
/// bundle whole.
#[tokio::test(flavor = "multi_thread")]
async fn declaring_the_bundle_one_by_one_in_the_loaders_order_is_refused() {
    let (cat, who) = lake("walk-1-one-by-one").await;
    let bundle = lakelands_core::bundle::load_dir(&example()).expect("the bundle loads");
    let mut refused = None;
    for d in bundle.declarations {
        if let Err(e) = cat.declare(&who, d).await {
            refused = Some(e);
            break;
        }
    }
    let refused = refused.expect("one declaration of the bundle is refused");
    assert!(
        refused.to_string().contains("not a declared type"),
        "refused for another reason: {refused}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_walks_question_has_the_answers_the_script_expects() {
    let (cat, who) = lake("walk-1-liquidity").await;

    // The bundle is declared whole, as `Catalog::apply` declares it: in
    // one append, with `invoice`'s reference to `partner` resolved within
    // the set.
    let bundle = lakelands_core::bundle::load_dir(&example()).expect("the bundle loads");
    cat.apply(&who, bundle.declarations).await.unwrap();

    let (r, d, _) = ingest(
        &cat,
        &who,
        "crm",
        records("partners.csv", "partners.map.json"),
    )
    .await;
    assert_eq!((r, d), (4, 0));
    let (r, d, august) = ingest(
        &cat,
        &who,
        "erp",
        records("invoices-august.csv", "invoices.map.json"),
    )
    .await;
    assert_eq!((r, d), (10, 0));
    let (r, d, _) = ingest(
        &cat,
        &who,
        "erp",
        records("invoices-september.csv", "invoices.map.json"),
    )
    .await;
    assert_eq!(
        (r, d),
        (3, 9),
        "September: INV-3002 paid, INV-1004 and INV-4003 new; nine rows unchanged"
    );

    // The derived gross of one invoice, by hand: 4000.00 * 1.19.
    let inv = cat
        .get(
            &who,
            &"invoice".into(),
            Get {
                key: "INV-1001".into(),
                with_meta: true,
                at: ReadAt::default(),
            },
        )
        .await
        .unwrap()
        .data;
    assert_eq!(cents(&inv.fields["gross"]), 476_000);

    let head = answer(&cat, &who, None).await;
    assert_eq!(
        head,
        BTreeMap::from([
            ("P-100".to_owned(), (1_071_000, 1_000_000, true)),
            ("P-200".to_owned(), (595_000, 500_000, true)),
            ("P-300".to_owned(), (476_000, 800_000, false)),
            ("P-400".to_owned(), (1_071_000, 2_000_000, false)),
        ]),
        "at the head"
    );
    let then = answer(&cat, &who, Some(august)).await;
    assert_eq!(
        then,
        BTreeMap::from([
            ("P-100".to_owned(), (1_071_000, 1_000_000, true)),
            ("P-200".to_owned(), (595_000, 500_000, true)),
            ("P-300".to_owned(), (833_000, 800_000, true)),
            ("P-400".to_owned(), (714_000, 2_000_000, false)),
        ]),
        "as of the August ingest, position {august}"
    );
}

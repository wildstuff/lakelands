//! The open book (`examples/open-book/`), kept from rotting: the bundle
//! loads with every lane it holds, and on the real store, in memory and on
//! SQLite, it takes a small business's day and answers its question once.
//!
//! The day is the soak's (lakelands#975) in miniature: partners and
//! invoices from an accounting export, accounts and movements from a bank
//! export, a credit limit set at the desk, a reminder raised, sent and
//! escalated by a second one, and that one's write-off called, proposed
//! and accepted. The answers are derived by hand from the records below:
//! who owes what (the view
//! `debtor`), since when (`days_overdue` and the set `overdue`), and the
//! cash (`cash_position`).

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::location::Location;
use lakelands_api::read::{Find, Found};
use lakelands_api::seam::Seams;
use lakelands_api::write::{Declaration, Outcome, WriteOpts};
use lakelands_api::{
    Catalog, Grants, LakeStore, Principal, PrincipalKind, RecordKey, SourceRecord, Timestamp,
    WriteContext,
};
use serde_json::{Value, json};

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/open-book")
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(f)
}

#[test]
fn the_open_book_bundle_loads_with_every_lane_it_holds() {
    let bundle = lakelands_core::bundle::load_dir(&example()).unwrap_or_else(|problems| {
        let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
        panic!("the open book does not load:\n{}", lines.join("\n"))
    });
    let names: Vec<String> = bundle
        .declarations
        .iter()
        .map(|d| match d {
            Declaration::ValueType(v) => format!("value_type {}", v.name),
            Declaration::Type(t) => format!("type {}", t.name),
            Declaration::Function(f) => format!("function {}", f.name),
            Declaration::Verb(v) => format!("verb {}", v.name),
            Declaration::Trigger(t) => format!("trigger {}", t.name),
            Declaration::ObjectSet(o) => format!("object_set {}", o.name),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        names,
        [
            "value_type direction",
            "value_type dunning_status",
            "value_type invoice_status",
            "value_type money",
            "value_type movement",
            "value_type partner_role",
            "type bank_account",
            "type cash_position",
            "type debtor",
            "type dunning",
            "type invoice",
            "type partner",
            "type transaction",
            "verb escalate_dunning",
            "verb mark_dunning_sent",
            "verb resolve_dunning",
            "verb set_credit_limit",
            "verb write_off_dunning",
            "object_set overdue",
        ]
    );
}

#[test]
fn the_open_book_answers_who_owes_what_since_when_in_memory() {
    block_on(a_day(Location::memory("open-book")));
}

#[test]
fn the_open_book_answers_who_owes_what_since_when_on_sqlite() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("open-book-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    block_on(a_day(Location::sqlite("open-book", dir.join("lake"))));
    let _ = std::fs::remove_dir_all(&dir);
}

fn principal(id: &str, grants: Grants) -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: id.into(),
        lake: "open-book".into(),
        grants,
    }
}

/// The date `n` days from today, in UTC.
fn day(n: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let today = Timestamp(now.as_micros() as i64).date();
    let midnight = Timestamp::from_date(today.0, today.1, today.2).unwrap();
    Timestamp(midnight.0 + n * 86_400_000_000).day().unwrap()
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

/// One export through `ingest`: the records it committed.
async fn ingest(
    cat: &dyn Catalog,
    who: &Principal,
    source: &str,
    rs: Vec<(&str, String, Value)>,
) -> u32 {
    let records = rs.into_iter().map(|(ty, key, fields)| SourceRecord {
        source: None,
        upstream: None,
        ty: ty.into(),
        key: key.as_str().into(),
        fields: serde_json::from_value(fields).unwrap(),
        seen_at: None,
    });
    let s: lakelands_api::SourceStream = Box::pin(Stream(records.collect::<Vec<_>>().into_iter()));
    let mut out = cat.ingest(who, &source.into(), s).await.unwrap();
    let mut records = 0;
    while let Some(w) = std::future::poll_fn(|cx| out.as_mut().poll_next(cx)).await {
        records += w.unwrap().records;
    }
    records
}

async fn found(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &str,
    filter: Option<&str>,
) -> Vec<lakelands_api::Record> {
    let find = Find {
        filter: filter.map(Into::into),
        limit: Some(1000),
        ..Find::default()
    };
    match cat.find(who, &ty.into(), find).await.unwrap().data {
        Found::Records { rows, .. } => rows,
        Found::Groups { .. } => panic!("records"),
    }
}

fn field<'a>(r: &'a lakelands_api::Record, name: &str) -> &'a Value {
    r.fields.get(name).unwrap_or(&Value::Null)
}

async fn a_day(location: Location) {
    let opened = lakelands_cli::open_store(location, OpenMode::Create, Seams::default());
    let store: Arc<dyn LakeStore> = opened.await.unwrap();
    let cat = store.catalog(WriteContext::default());
    let owner = principal("owner", Grants::owner());
    let bundle = lakelands_core::bundle::load_dir(&example()).unwrap();
    cat.apply(&owner, bundle.declarations).await.unwrap();
    for view in ["debtor", "cash_position"] {
        let setting = json!({"key": format!("view_kept.{view}"), "value": true});
        let key = format!("view_kept.{view}");
        cat.upsert(
            &owner,
            &"sys.setting".into(),
            &key.as_str().into(),
            serde_json::from_value(setting).unwrap(),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    }

    // The accounting export: four partners, eight invoices.
    let partners = [
        ("p1", "Alder", "debtor"),
        ("p2", "Birch", "debtor"),
        ("p3", "Cedar", "both"),
        ("p4", "Delta Supplies", "creditor"),
    ];
    let mut rows: Vec<(&str, String, Value)> = partners
        .iter()
        .map(|(id, name, role)| {
            (
                "partner",
                id.to_string(),
                json!({"partner_id": id, "name": name, "role": role, "payment_terms_days": 14}),
            )
        })
        .collect();
    // (id, direction, partner, issued, due, amount, status)
    let invoices = [
        ("i1", "outbound", "p1", -90, -76, "1200.00", "open"),
        ("i2", "outbound", "p1", -50, -36, "300.00", "open"),
        ("i3", "outbound", "p2", -20, -6, "450.50", "open"),
        ("i4", "outbound", "p2", -60, -46, "800.00", "paid"),
        ("i5", "outbound", "p3", -10, 4, "99.99", "open"),
        ("i6", "inbound", "p3", -40, -26, "200.00", "open"),
        ("i7", "inbound", "p4", -15, -1, "5000.00", "open"),
        ("i8", "outbound", "p1", -100, -86, "75.00", "cancelled"),
    ];
    for (id, dir, partner, issued, due, amount, status) in invoices {
        rows.push(("invoice", id.into(), json!({"invoice_id": id, "direction": dir, "partner": partner,
            "issued_on": day(issued), "due_on": day(due), "amount": amount, "currency": "EUR", "status": status})));
    }
    assert_eq!(ingest(cat.as_ref(), &owner, "accounting", rows).await, 12);

    // The bank export: two accounts and three movements.
    let mut rows: Vec<(&str, String, Value)> = vec![
        (
            "bank_account",
            "DE01".into(),
            json!({"iban": "DE01", "account_name": "current", "currency": "EUR",
            "opening_balance": "10000.00", "balance": "10650.00"}),
        ),
        (
            "bank_account",
            "DE02".into(),
            json!({"iban": "DE02", "account_name": "reserve", "currency": "EUR",
            "opening_balance": "2500.00", "balance": "2500.00"}),
        ),
    ];
    for (amount, direction, reference, n) in [
        ("800.00", "credit", "i4", -30),
        ("150.00", "debit", "fee", -5),
    ] {
        let fields = json!({"account": "DE01", "value_on": day(n), "amount": amount, "direction": direction, "reference": reference});
        let key = format!("DE01:{}:{amount}:{reference}", day(n));
        rows.push(("transaction", key, fields));
    }
    assert_eq!(ingest(cat.as_ref(), &owner, "bank", rows).await, 4);

    // The desk: a credit limit, a reminder raised, sent, escalated and
    // resolved, and a write-off proposed and accepted.
    let call = |verb: &'static str, key: &'static str, payload: Value| {
        let (cat, owner) = (cat.clone(), owner.clone());
        async move {
            let key = RecordKey::from(key);
            cat.call(
                &owner,
                &verb.into(),
                Some(&key),
                serde_json::from_value(payload).unwrap(),
                WriteOpts::default(),
            )
            .await
        }
    };
    call("set_credit_limit", "p1", json!({"credit_limit": "1000.00"}))
        .await
        .unwrap();
    let raise = |key: &'static str, level: u32| {
        let (cat, owner) = (cat.clone(), owner.clone());
        async move {
            let fields = json!({"id": key, "invoice": "i1", "partner": "p1", "level": level, "amount": "1200.00", "status": "open"});
            cat.upsert(
                &owner,
                &"dunning".into(),
                &key.into(),
                serde_json::from_value(fields).unwrap(),
                WriteOpts::default(),
            )
            .await
            .unwrap();
        }
    };
    raise("d1", 1).await;
    call("mark_dunning_sent", "d1", json!({"sent_on": day(-10)}))
        .await
        .unwrap();
    raise("d2", 2).await;
    call("escalate_dunning", "d1", json!({})).await.unwrap();
    call("mark_dunning_sent", "d2", json!({"sent_on": day(-2)}))
        .await
        .unwrap();
    // A write-off is high risk: the call is proposed, and runs when a
    // reviewer accepts it.
    let Outcome::Proposed { proposal, .. } =
        call("write_off_dunning", "d2", json!({})).await.unwrap()
    else {
        panic!("a high-risk verb's call is proposed");
    };
    let reviewer = principal("reviewer", Grants::owner());
    cat.accept(&reviewer, &proposal).await.unwrap();

    // Who owes what: the open outbound invoices by partner.
    let debtor = found(cat.as_ref(), &owner, "debtor", None).await;
    let owed: Vec<(String, Value, Value)> = debtor
        .iter()
        .map(|r| {
            (
                r.key.to_string(),
                field(r, "open_receivables").clone(),
                field(r, "open_invoices").clone(),
            )
        })
        .collect();
    assert_eq!(
        owed,
        [
            ("p1".into(), json!("1500.00"), json!(2)),
            ("p2".into(), json!("450.50"), json!(1)),
            ("p3".into(), json!("99.99"), json!(1)),
        ],
        "who owes what: {debtor:?}"
    );
    // Since when: days overdue at the read, and the set of the open
    // receivables more than thirty days past due.
    let late = found(
        cat.as_ref(),
        &owner,
        "invoice",
        Some("direction = 'outbound' and status = 'open'"),
    )
    .await;
    let days: Vec<(String, Value)> = late
        .iter()
        .map(|r| (r.key.to_string(), field(r, "days_overdue").clone()))
        .collect();
    assert_eq!(
        days,
        [
            ("i1".into(), json!(76)),
            ("i2".into(), json!(36)),
            ("i3".into(), json!(6)),
            ("i5".into(), json!(-4))
        ],
        "days overdue at the read: {late:?}"
    );
    let overdue = found(cat.as_ref(), &owner, "invoice", Some("in_set('overdue')")).await;
    let keys: Vec<String> = overdue.iter().map(|r| r.key.to_string()).collect();
    assert_eq!(keys, ["i1", "i2"], "more than thirty days past due");
    // The cash: the last known balances in euros.
    let cash = found(cat.as_ref(), &owner, "cash_position", None).await;
    assert_eq!(cash.len(), 1, "{cash:?}");
    assert_eq!(
        (
            field(&cash[0], "total_balance"),
            field(&cash[0], "accounts")
        ),
        (&json!("13150.00"), &json!(2))
    );
    // The chase reads as a history.
    let chase = found(cat.as_ref(), &owner, "dunning", None).await;
    let chase: Vec<(String, Value, Value)> = chase
        .iter()
        .map(|r| {
            (
                r.key.to_string(),
                field(r, "status").clone(),
                field(r, "heading").clone(),
            )
        })
        .collect();
    assert_eq!(
        chase,
        [
            (
                "d1".into(),
                json!("escalated"),
                json!("Friendly reminder: invoice i1")
            ),
            (
                "d2".into(),
                json!("written_off"),
                json!("Formal notice: invoice i1")
            ),
        ]
    );
    let p1 = found(cat.as_ref(), &owner, "partner", Some("partner_id = 'p1'")).await;
    assert_eq!(field(&p1[0], "credit_limit"), &json!("1000.00"));
}

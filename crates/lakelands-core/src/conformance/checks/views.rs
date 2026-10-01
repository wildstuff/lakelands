//! View types: records computed from another type, read with `find`
//! like any other and written by no one (`lakelands_api::ontology::ViewDecl`).
//!
//! A view is a standing obligation, not a query: the store keeps it
//! current as its source changes, through every door that changes the
//! source. It is a function of the source's state, so the same source
//! state gives the same view record for record, byte for byte, however
//! the source got there. The fixture is one source (`invoice`: a partner
//! and an exact amount) and one view over it (a total and a count per
//! partner).

use std::collections::BTreeMap;
use std::sync::Arc;

use lakelands_api::ontology::{
    Authority, Cardinality, Derived, MatchPair, Measure, RelationDecl, StorageKind, TypeDecl,
    ViewDecl,
};
use lakelands_api::query::{Depth, Pattern, Projection, QueryResult, Start, Step, Walk};
use lakelands_api::read::Find;
use lakelands_api::{FieldName, LakeStore, Principal, TypeName, Value};
use rust_decimal::Decimal;
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, all, all_found, dec, decimal, doors, field, partner_totals, source_record,
    type_decl, upsert, windows_reach_head,
};

check!(
    /// A view record's key is its group.
    VIEW_KEY_IS_ITS_GROUP = a_view_record_is_keyed_by_its_group_by_fields,
    "018 and the log format §2.3: a view record's key is made from its `group_by` fields as any record key is, each value with `%` and `:` escaped, joined by `:` in declared order"
);

check!(
    /// A view groups its source and sums it exactly.
    GROUPS_AND_SUMS = a_view_groups_its_source_and_sums_it_exactly,
    "the ontology: a view type is computed from its source's records, one record per group, measures exact for decimals"
);

check!(
    /// A view does not depend on when it was declared.
    SAME_EITHER_WAY = a_view_is_the_same_whether_declared_before_or_after_its_source_was_written,
    "the ontology: a view is a function of its source's state, so how the source got there cannot change a byte of it"
);

check!(
    /// A group with no members left has no record.
    EMPTIED_GROUP_GOES = a_view_drops_a_group_whose_last_member_is_retired,
    "the ontology: a view holds a record per group that has members; retiring a group's last member removes the group"
);

check!(
    /// A pattern may start on a view.
    PATTERN_STARTS_ON_A_VIEW = a_pattern_started_on_a_view_holds_the_records_find_returns,
    "018 and 026: a view is read like any other type, so a pattern started on it by key, by `where` or with neither has as its graph's nodes exactly the records `find` returns for that selection; a key with no group has none"
);

check!(
    /// A walk reaches a view over a derived relation.
    WALK_REACHES_A_VIEW = a_walk_over_a_derived_relation_reaches_a_views_records_both_ways,
    "018, 026 and 044: a derived relation between a stored type and a view connects their records by matching values like any other, so a walk out from the stored type reaches the view's records and a walk in from the view reaches the stored ones"
);

check!(
    /// Ingest keeps a view current.
    CURRENT_AFTER_INGEST = a_view_over_an_ingested_source_is_current_once_the_ingest_is_committed,
    "016 and the ontology: both write doors are folded by the same rules, so a view over ingested records is kept like any other"
);

/// The source type, its `partner` and `amount` written by `authority`.
fn invoices(authority: Authority) -> TypeDecl {
    let mut partner = field("partner", StorageKind::Text);
    let mut amount = field("amount", StorageKind::Decimal);
    partner.authority = authority;
    amount.authority = authority;
    type_decl("invoice", vec![partner, amount])
}

/// A view's records, by partner: each record's fields as stored.
async fn view_rows(
    store: &dyn LakeStore,
    who: &Principal,
    view: &str,
) -> Result<BTreeMap<String, BTreeMap<FieldName, Value>>, Failure> {
    let cat = doors(store);
    let mut out = BTreeMap::new();
    for rec in all(cat.as_ref(), who, view).await? {
        let Some(partner) = rec.fields.get("partner").and_then(Value::as_str) else {
            return Err(Failure::new(format!(
                "a record of the view `{view}` (key `{}`) does not carry its group field \
                 `partner`: {:?}",
                rec.key, rec.fields
            )));
        };
        out.insert(partner.to_owned(), rec.fields.clone());
    }
    Ok(out)
}

/// A view's totals and counts, by partner, as exact numbers.
async fn view_totals(
    store: &dyn LakeStore,
    who: &Principal,
    view: &str,
) -> Result<Vec<(String, Option<Decimal>, Option<f64>)>, Failure> {
    Ok(view_rows(store, who, view)
        .await?
        .into_iter()
        .map(|(partner, fields)| {
            let total = decimal(fields.get("total"));
            let count = fields.get("invoices").and_then(Value::as_f64);
            (partner, total, count)
        })
        .collect())
}

fn want(rows: &[(&str, &str, f64)]) -> Vec<(String, Option<Decimal>, Option<f64>)> {
    rows.iter()
        .map(|(p, t, n)| ((*p).to_owned(), Some(dec(t)), Some(*n)))
        .collect()
}

async fn write_invoice(
    store: &dyn LakeStore,
    who: &Principal,
    nr: &str,
    partner: &str,
    amount: &str,
) -> Result<(), Failure> {
    let cat = doors(store);
    upsert(
        cat.as_ref(),
        who,
        "invoice",
        nr,
        json!({"partner": partner, "amount": amount}),
    )
    .await?;
    Ok(())
}

async fn a_view_groups_its_source_and_sums_it_exactly(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { own, .. } = &l;
    l.declare(invoices(Authority::Authored)).await?;
    l.declare(partner_totals("partner_totals", StorageKind::Decimal))
        .await?;
    for (nr, partner, amount) in [
        ("i1", "p1", "10.00"),
        ("i2", "p1", "5.50"),
        ("i3", "p2", "3.00"),
    ] {
        write_invoice(store.as_ref(), own, nr, partner, amount).await?;
    }

    let got = view_totals(store.as_ref(), own, "partner_totals").await?;
    let expected = want(&[("p1", "15.50", 2.0), ("p2", "3.00", 1.0)]);
    ensure!(
        got == expected,
        "the view over three invoices must hold p1 = 15.50 from 2 invoices and p2 = 3.00 \
         from 1, exactly; it holds {got:?}. A declared view the store accepted is a view \
         it owes"
    );
    Ok(())
}

async fn a_view_is_the_same_whether_declared_before_or_after_its_source_was_written(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { own, .. } = &l;
    l.declare(invoices(Authority::Authored)).await?;
    l.declare(partner_totals("totals_early", StorageKind::Decimal))
        .await?;
    for (nr, partner, amount) in [
        ("i1", "p1", "10.00"),
        ("i2", "p1", "5.50"),
        ("i3", "p2", "3.00"),
        ("i4", "p3", "100.00"),
    ] {
        write_invoice(store.as_ref(), own, nr, partner, amount).await?;
    }
    // An update in place, a move between groups, and a group's only
    // member restated at a shorter scale: every way the early view must
    // follow its source.
    write_invoice(store.as_ref(), own, "i1", "p1", "20.00").await?;
    write_invoice(store.as_ref(), own, "i2", "p2", "5.50").await?;
    write_invoice(store.as_ref(), own, "i4", "p3", "100.5").await?;
    l.declare(partner_totals("totals_late", StorageKind::Decimal))
        .await?;

    let early = view_rows(store.as_ref(), own, "totals_early").await?;
    let late = view_rows(store.as_ref(), own, "totals_late").await?;
    ensure!(
        early == late,
        "two views over the same source state disagree: declared before the writes it \
         holds {early:?}, declared after them {late:?}. A view is a function of its \
         source's state, byte for byte, trailing zeros included"
    );
    let got = view_totals(store.as_ref(), own, "totals_early").await?;
    let expected = want(&[
        ("p1", "20.00", 1.0),
        ("p2", "8.50", 2.0),
        ("p3", "100.5", 1.0),
    ]);
    ensure!(
        got == expected,
        "after the updates the view must hold p1 = 20.00 (1), p2 = 8.50 (2), \
         p3 = 100.5 (1); it holds {got:?}"
    );
    Ok(())
}

async fn a_view_drops_a_group_whose_last_member_is_retired(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(invoices(Authority::Authored)).await?;
    l.declare(partner_totals("partner_totals", StorageKind::Decimal))
        .await?;
    write_invoice(store.as_ref(), own, "i1", "p1", "10.00").await?;
    write_invoice(store.as_ref(), own, "i2", "p2", "3.00").await?;
    write_invoice(store.as_ref(), own, "i3", "p2", "4.00").await?;

    let invoice = TypeName::from("invoice");
    for nr in ["i2", "i1"] {
        cat.retire(own, &invoice, &nr.into(), Default::default())
            .await
            .must(&format!("retiring invoice `{nr}`"))?;
    }

    let got = view_totals(store.as_ref(), own, "partner_totals").await?;
    let expected = want(&[("p2", "4.00", 1.0)]);
    ensure!(
        got == expected,
        "p1 lost its only invoice and p2 one of two; the view must hold only \
         p2 = 4.00 (1), with no zeroed record for p1; it holds {got:?}"
    );
    Ok(())
}

async fn a_view_over_an_ingested_source_is_current_once_the_ingest_is_committed(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { own, .. } = &l;
    l.declare(invoices(Authority::Source)).await?;
    l.declare(partner_totals("partner_totals", StorageKind::Decimal))
        .await?;

    let delivery = [
        ("i1", "p1", "10.00"),
        ("i2", "p1", "5.50"),
        ("i3", "p2", "3.00"),
        // The same key twice in one delivery: the view reflects the
        // record that stands, not both.
        ("i1", "p1", "20.00"),
    ]
    .into_iter()
    .map(|(nr, partner, amount)| {
        source_record("invoice", nr, json!({"partner": partner, "amount": amount}))
    })
    .collect();
    let windows = l
        .ingest("ledger", delivery)
        .await
        .must("ingesting four invoices from `ledger`")?;
    windows_reach_head(
        store.as_ref(),
        &windows,
        "ingesting four invoices from `ledger`",
    )
    .await?;

    let got = view_totals(store.as_ref(), own, "partner_totals").await?;
    let expected = want(&[("p1", "25.50", 2.0), ("p2", "3.00", 1.0)]);
    ensure!(
        got == expected,
        "after the ingest the view must hold p1 = 25.50 (2) and p2 = 3.00 (1); it holds \
         {got:?}. A door that writes the source and skips what derives from it leaves the \
         view stale"
    );
    Ok(())
}

async fn a_view_record_is_keyed_by_its_group_by_fields(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut source = invoices(Authority::Authored);
    source.fields.push(field("region", StorageKind::Text));
    l.declare(source).await?;
    let mut view = type_decl(
        "by_both",
        vec![
            field("partner", StorageKind::Text),
            field("region", StorageKind::Text),
            field("invoices", StorageKind::Number),
        ],
    );
    view.key = vec![FieldName::from("partner"), FieldName::from("region")];
    view.view = Some(ViewDecl {
        source: TypeName::from("invoice"),
        r#where: None,
        group_by: vec![FieldName::from("partner"), FieldName::from("region")],
        measures: vec![Measure {
            name: FieldName::from("invoices"),
            expr: "count()".into(),
        }],
    });
    l.declare(view).await?;
    for (k, partner, region) in [
        ("1", "acme", "north"),
        ("2", "acme", "south:east"),
        ("3", "acme", "north"),
    ] {
        l.upsert(
            "invoice",
            k,
            json!({"partner": partner, "region": region, "amount": "1"}),
        )
        .await?;
    }
    let mut keys: Vec<String> = all(cat.as_ref(), own, "by_both")
        .await?
        .into_iter()
        .map(|r| r.key.0)
        .collect();
    keys.sort();
    ensure!(
        keys == ["acme:north", "acme:south%3Aeast"],
        "the view grouped by partner and region must key its records `acme:north` and \
         `acme:south%3Aeast` (the `:` inside a value escaped); they are keyed {keys:?}"
    );
    let north = l.read("by_both", "acme:north").await?;
    ensure!(
        north.fields.get("invoices").and_then(Value::as_f64) == Some(2.0),
        "reading the view record `acme:north` by its key must answer 2 invoices; it \
         answers {:?}",
        north.fields
    );
    Ok(())
}

/// A pattern over `ty` from a start, with its steps and projection.
fn pattern(ty: &str, key: Option<&str>, filter: Option<&str>, steps: Vec<Step>) -> Pattern {
    Pattern {
        start: Start {
            ty: ty.into(),
            key: key.map(Into::into),
            filter: filter.map(Into::into),
            search: None,
        },
        project: if steps.is_empty() {
            Projection::Graph
        } else {
            Projection::Rows { fields: vec![] }
        },
        steps,
        to: None,
        order_by: Vec::new(),
        limit: None,
        cursor: None,
        at: Default::default(),
    }
}

/// The `type:key` of every node a graph answers, or every `far` key a
/// walk's rows reach, sorted.
async fn nodes(
    store: &dyn LakeStore,
    who: &Principal,
    p: Pattern,
    far: &str,
) -> Result<Vec<String>, Failure> {
    let what = format!(
        "a pattern started on `{}` (key {:?}, where {:?})",
        p.start.ty, p.start.key, p.start.filter
    );
    let reply = doors(store).query(who, p).await.must(&what)?;
    let mut got: Vec<String> = match reply.data {
        QueryResult::Graph { nodes, .. } => nodes
            .into_iter()
            .map(|n| format!("{}:{}", n.ty, n.key))
            .collect(),
        QueryResult::Rows { rows, .. } => rows
            .iter()
            .filter_map(|r| r.get(&format!("{far}.key")).and_then(Value::as_str))
            .map(|k| format!("{far}:{k}"))
            .collect(),
        QueryResult::Groups { .. } => {
            return Err(Failure::new(format!("{what} answered groups")));
        }
    };
    got.sort();
    got.dedup();
    Ok(got)
}

async fn a_pattern_started_on_a_view_holds_the_records_find_returns(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(invoices(Authority::Authored)).await?;
    l.declare(partner_totals("partner_totals", StorageKind::Decimal))
        .await?;
    for (nr, partner, amount) in [
        ("i1", "p1", "10.00"),
        ("i2", "p2", "3.00"),
        ("i3", "p1", "5.00"),
    ] {
        write_invoice(store.as_ref(), own, nr, partner, amount).await?;
    }
    for (key, filter) in [
        (None, None),
        (None, Some("partner = 'p1'")),
        (Some("p2"), None),
    ] {
        let found = all_found(
            cat.as_ref(),
            own,
            "partner_totals",
            Find {
                filter: filter.map(Into::into),
                ..Find::default()
            },
        )
        .await?;
        let want: Vec<String> = found
            .iter()
            .map(|r| format!("partner_totals:{}", r.key))
            .filter(|n| key.is_none_or(|k| *n == format!("partner_totals:{k}")))
            .collect();
        let got = nodes(
            store.as_ref(),
            own,
            pattern("partner_totals", key, filter, vec![]),
            "",
        )
        .await?;
        ensure!(
            got == want,
            "a pattern started on the view `partner_totals` (key {key:?}, where {filter:?}) must \
             hold as its graph's nodes the records `find` returns, {want:?}; it holds {got:?}. \
             A view read through the pattern query that answers nothing looks exactly like an \
             empty view"
        );
    }
    let none = nodes(
        store.as_ref(),
        own,
        pattern("partner_totals", Some("p9"), None, vec![]),
        "",
    )
    .await?;
    ensure!(
        none.is_empty(),
        "no invoice has partner p9, so the view has no group `p9` and a pattern started on \
         it by that key has no node; it holds {none:?}"
    );
    Ok(())
}

async fn a_walk_over_a_derived_relation_reaches_a_views_records_both_ways(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { own, .. } = &l;
    l.declare(invoices(Authority::Authored)).await?;
    l.declare(partner_totals("partner_totals", StorageKind::Decimal))
        .await?;
    let mut partner = type_decl("partner", vec![field("name", StorageKind::Text)]);
    partner.key = vec![FieldName::from("name")];
    partner.relations = vec![RelationDecl {
        cardinality: Cardinality::OneToOne,
        derived: Some(Derived {
            pairs: vec![MatchPair {
                from: "name".into(),
                to: "partner".into(),
            }],
        }),
        ..RelationDecl::to("totals", "partner_totals")
    }];
    l.declare(partner).await?;
    for name in ["p1", "p2", "p3"] {
        l.upsert("partner", name, json!({"name": name})).await?;
    }
    write_invoice(store.as_ref(), own, "i1", "p1", "10.00").await?;
    write_invoice(store.as_ref(), own, "i2", "p2", "3.00").await?;
    let step = |walk| Step {
        relation: "totals".into(),
        walk,
        ty: None,
        filter: None,
        depth: Depth::default(),
    };
    for (start, key, walk, far, want) in [
        (
            "partner",
            "p1",
            Walk::Out,
            "partner_totals",
            vec!["partner_totals:p1"],
        ),
        ("partner", "p3", Walk::Out, "partner_totals", vec![]),
        (
            "partner_totals",
            "p2",
            Walk::In,
            "partner",
            vec!["partner:p2"],
        ),
    ] {
        let got = nodes(
            store.as_ref(),
            own,
            pattern(start, Some(key), None, vec![step(walk)]),
            far,
        )
        .await?;
        ensure!(
            got == want,
            "walking `totals` {walk:?} from {start} `{key}` must reach {want:?}, the records \
             whose matched value is the same; it reaches {got:?}"
        );
    }
    Ok(())
}

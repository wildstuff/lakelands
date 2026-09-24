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

use lakelands_api::ontology::{Authority, Measure, StorageKind, TypeDecl, ViewDecl};
use lakelands_api::{FieldName, LakeStore, Principal, TypeName, Value};
use rust_decimal::Decimal;
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, all, dec, decimal, declare, doors, field, ingest, owner, source_record, type_decl,
    upsert, windows_reach_head,
};

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

/// A view named `name` over `invoice`: per partner, the exact total and
/// how many invoices went into it.
fn totals(name: &str) -> TypeDecl {
    let mut ty = type_decl(
        name,
        vec![
            field("partner", StorageKind::Text),
            field("total", StorageKind::Decimal),
            field("invoices", StorageKind::Number),
        ],
    );
    ty.key = vec![FieldName::from("partner")];
    ty.view = Some(ViewDecl {
        source: TypeName::from("invoice"),
        r#where: None,
        group_by: vec![FieldName::from("partner")],
        measures: vec![
            Measure {
                name: FieldName::from("total"),
                expr: "sum(amount)".into(),
            },
            Measure {
                name: FieldName::from("invoices"),
                expr: "count()".into(),
            },
        ],
    });
    ty
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
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(cat.as_ref(), &own, invoices(Authority::Authored)).await?;
    declare(cat.as_ref(), &own, totals("partner_totals")).await?;
    for (nr, partner, amount) in [
        ("i1", "p1", "10.00"),
        ("i2", "p1", "5.50"),
        ("i3", "p2", "3.00"),
    ] {
        write_invoice(store.as_ref(), &own, nr, partner, amount).await?;
    }

    let got = view_totals(store.as_ref(), &own, "partner_totals").await?;
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
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(cat.as_ref(), &own, invoices(Authority::Authored)).await?;
    declare(cat.as_ref(), &own, totals("totals_early")).await?;
    for (nr, partner, amount) in [
        ("i1", "p1", "10.00"),
        ("i2", "p1", "5.50"),
        ("i3", "p2", "3.00"),
        ("i4", "p3", "100.00"),
    ] {
        write_invoice(store.as_ref(), &own, nr, partner, amount).await?;
    }
    // An update in place, a move between groups, and a group's only
    // member restated at a shorter scale: every way the early view must
    // follow its source.
    write_invoice(store.as_ref(), &own, "i1", "p1", "20.00").await?;
    write_invoice(store.as_ref(), &own, "i2", "p2", "5.50").await?;
    write_invoice(store.as_ref(), &own, "i4", "p3", "100.5").await?;
    declare(cat.as_ref(), &own, totals("totals_late")).await?;

    let early = view_rows(store.as_ref(), &own, "totals_early").await?;
    let late = view_rows(store.as_ref(), &own, "totals_late").await?;
    ensure!(
        early == late,
        "two views over the same source state disagree: declared before the writes it \
         holds {early:?}, declared after them {late:?}. A view is a function of its \
         source's state, byte for byte, trailing zeros included"
    );
    let got = view_totals(store.as_ref(), &own, "totals_early").await?;
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
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(cat.as_ref(), &own, invoices(Authority::Authored)).await?;
    declare(cat.as_ref(), &own, totals("partner_totals")).await?;
    write_invoice(store.as_ref(), &own, "i1", "p1", "10.00").await?;
    write_invoice(store.as_ref(), &own, "i2", "p2", "3.00").await?;
    write_invoice(store.as_ref(), &own, "i3", "p2", "4.00").await?;

    let invoice = TypeName::from("invoice");
    for nr in ["i2", "i1"] {
        cat.retire(&own, &invoice, &nr.into(), Default::default())
            .await
            .must(&format!("retiring invoice `{nr}`"))?;
    }

    let got = view_totals(store.as_ref(), &own, "partner_totals").await?;
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
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(cat.as_ref(), &own, invoices(Authority::Source)).await?;
    declare(cat.as_ref(), &own, totals("partner_totals")).await?;

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
    let windows = ingest(cat.as_ref(), &own, "ledger", delivery)
        .await
        .must("ingesting four invoices from `ledger`")?;
    windows_reach_head(
        store.as_ref(),
        &windows,
        "ingesting four invoices from `ledger`",
    )
    .await?;

    let got = view_totals(store.as_ref(), &own, "partner_totals").await?;
    let expected = want(&[("p1", "25.50", 2.0), ("p2", "3.00", 1.0)]);
    ensure!(
        got == expected,
        "after the ingest the view must hold p1 = 25.50 (2) and p2 = 3.00 (1); it holds \
         {got:?}. A door that writes the source and skips what derives from it leaves the \
         view stale"
    );
    Ok(())
}

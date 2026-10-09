//! Decimals stay exact (`docs/log-format.md` §5,
//! `docs/decisions/011-storage-kind-and-value-type.md`).
//!
//! A `decimal` value is a string of digits carried as written, so money
//! never passes through binary floating point: it reads back byte for
//! byte, and a sum over it is exact. Written once as a rule here because
//! every engine stores decimals differently, and a float anywhere on the
//! way is invisible until a total is a cent off.

use lakelands_api::ontology::StorageKind;
use lakelands_api::read::{AggFunc, Aggregate, Group, GroupKey};
use lakelands_api::{Find, Found, TypeName};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, agg, all, dec, decimal, field, type_decl, value, view_of,
};
use crate::conformance::{Failure, Held, Store};

check!(
    AS_WRITTEN = a_decimal_reads_back_exactly_as_it_was_written,
    "the log format: a decimal is a string of digits kept as written, trailing zeros and all, beyond any float's precision"
);

check!(
    EXACT_SUM = a_sum_over_decimals_is_exact,
    "the engine seam: sum is exact for decimals, so 0.10 + 0.20 + 0.30 is 0.60 and not a float's approximation"
);

check!(
    SCALE_KEPT = a_scaled_decimal_keeps_its_scale_when_derived_and_summed,
    "011: a value is kept at its value type's scale, so a derived decimal is rounded to it before it is stored, and a sum over a scaled field answers at that scale"
);

check!(
    SUM_SCALE = a_sum_reads_at_the_largest_scale_among_its_values,
    "011: a sum over a field with no declared scale reads at the largest scale among its values, a zero's included, whatever order their keys give them: 5 and 0.00 are 5.00, and 1.10, -1.10 and 6 are 6.00 with an average of 2.00, through `find`'s aggregate and through a view"
);

/// The groups of a sum of `field` over every `ty` record, with no
/// `group_by`; any answer but groups fails, naming `what`.
async fn sum(l: &Lake, ty: &str, field: &str, what: &str) -> Result<Vec<Group>, Failure> {
    let find = Find {
        aggregate: Some(Aggregate {
            measures: vec![agg("total", AggFunc::Sum, Some(field))],
            group_by: Vec::new(),
        }),
        ..Find::default()
    };
    let reply = l.cat.find(&l.own, &TypeName::from(ty), find).await;
    let Found::Groups { rows } = reply.must(what)?.data else {
        return Err(Failure::new(format!(
            "{what}: `find` with an aggregate must answer groups; it answered records"
        )));
    };
    Ok(rows)
}

const AMOUNTS: [&str; 6] = [
    "8500.125",
    "10.00",
    "100.5",
    "-0.10",
    "0",
    "12345678901234567890.123456789",
];

async fn a_decimal_reads_back_exactly_as_it_was_written(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let ty = type_decl("payment", vec![field("amount", StorageKind::Decimal)]);
    l.declare(ty).await?;
    for (i, amount) in AMOUNTS.iter().enumerate() {
        let key = format!("m{i}");
        l.upsert("payment", &key, json!({"amount": amount})).await?;
    }

    let rows = all(cat.as_ref(), own, "payment").await?;
    ensure!(
        rows.len() == AMOUNTS.len(),
        "`find` listed {} payments; {} were written",
        rows.len(),
        AMOUNTS.len()
    );
    for (i, amount) in AMOUNTS.iter().enumerate() {
        let key = format!("m{i}");
        let Some(rec) = rows.iter().find(|r| r.key.as_str() == key) else {
            return Err(Failure::new(format!("`find` did not list payment `{key}`")));
        };
        ensure!(
            value(rec, "amount") == Some(&json!(amount)),
            "the decimal {amount:?} written to `{key}` reads back as {:?}; a decimal is \
             kept exactly as written, as a string",
            value(rec, "amount")
        );
    }
    Ok(())
}

async fn a_sum_over_decimals_is_exact(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let ty = type_decl("payment", vec![field("amount", StorageKind::Decimal)]);
    l.declare(ty).await?;
    for (key, amount) in [("a", "0.10"), ("b", "0.20"), ("c", "0.30")] {
        l.upsert("payment", key, json!({"amount": amount})).await?;
    }

    let rows = sum(
        &l,
        "payment",
        "amount",
        "summing `amount` over the payments",
    )
    .await?;
    let [group] = rows.as_slice() else {
        return Err(Failure::new(format!(
            "a sum without `group_by` is one group; `find` answered {}",
            rows.len()
        )));
    };
    let total = group.values.get("total");
    ensure!(
        decimal(total) == Some(dec("0.60")),
        "the sum of the decimals 0.10, 0.20 and 0.30 must be exactly 0.60, as a decimal \
         string; it is {total:?}"
    );
    Ok(())
}

async fn a_scaled_decimal_keeps_its_scale_when_derived_and_summed(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare_json(json!({
        "declare": "value_type", "name": "money", "kind": "decimal", "scale": 2
    }))
    .await?;
    let money_field = |name: &str| {
        let mut f = field(name, StorageKind::Decimal);
        f.ty.value_type = Some("money".into());
        f
    };
    let mut gross = money_field("gross");
    gross.derived = Some(lakelands_api::expr::Condition::from("net * 1.19"));
    gross.authority = lakelands_api::ontology::Authority::Derived;
    l.declare(type_decl("invoice", vec![money_field("net"), gross]))
        .await?;
    for (key, net) in [("a", "4000.00"), ("b", "5000"), ("c", "0.05")] {
        l.upsert("invoice", key, json!({"net": net})).await?;
    }
    let rows = all(cat.as_ref(), own, "invoice").await?;
    let grosses: Vec<Option<&serde_json::Value>> = ["a", "b", "c"]
        .iter()
        .map(|k| {
            rows.iter()
                .find(|r| r.key.as_str() == *k)
                .and_then(|r| value(r, "gross"))
        })
        .collect();
    ensure!(
        grosses
            == [
                Some(&json!("4760.00")),
                Some(&json!("5950.00")),
                Some(&json!("0.06"))
            ],
        "`gross = net * 1.19` of 4000.00, 5000 and 0.05 in `money` (scale 2) stored {grosses:?}; \
         a derived value is rounded half away from zero to its scale before it is stored: \
         4760.00, 5950.00, 0.06"
    );
    let rows = sum(&l, "invoice", "gross", "summing `gross`").await?;
    let total = rows.first().and_then(|g| g.values.get("total"));
    ensure!(
        total == Some(&json!("10710.06")),
        "the sum of `gross` in `money` (scale 2) is {total:?}; it must be \"10710.06\", at the \
         field's scale"
    );
    Ok(())
}

async fn a_sum_reads_at_the_largest_scale_among_its_values(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let (text, decimal) = (StorageKind::Text, StorageKind::Decimal);
    let entry = vec![field("book", text), field("amount", decimal)];
    l.declare(type_decl("entry", entry)).await?;
    let measured = vec![
        field("book", text),
        field("total", decimal),
        field("mean", decimal),
    ];
    let measures = [("total", "sum(amount)"), ("mean", "avg(amount)")];
    let view = view_of("book_total", "entry", &["book"], measured, &measures);
    l.declare(view).await?;
    // Each pair of books holds the same amounts, their keys in two orders.
    let books: [(&str, &[&str]); 4] = [
        ("b1", &["5", "0.00"]),
        ("b2", &["0.00", "5"]),
        ("b3", &["1.10", "-1.10", "6"]),
        ("b4", &["6", "1.10", "-1.10"]),
    ];
    for (book, amounts) in books {
        for (i, amount) in amounts.iter().enumerate() {
            let fields = json!({"book": book, "amount": amount});
            l.upsert("entry", &format!("{book}-{i}"), fields).await?;
        }
    }
    let want = [
        "b1 5.00 2.50",
        "b2 5.00 2.50",
        "b3 6.00 2.00",
        "b4 6.00 2.00",
    ];
    fn line(book: Option<&serde_json::Value>, values: [Option<&serde_json::Value>; 2]) -> String {
        let text = |v: Option<&serde_json::Value>| v.map_or("-".into(), |v| v.to_string());
        [book, values[0], values[1]]
            .map(text)
            .join(" ")
            .replace('"', "")
    }

    let find = Find {
        aggregate: Some(Aggregate {
            measures: vec![
                agg("total", AggFunc::Sum, Some("amount")),
                agg("mean", AggFunc::Avg, Some("amount")),
            ],
            group_by: vec![GroupKey::Field("book".into())],
        }),
        ..Find::default()
    };
    let reply = cat.find(own, &TypeName::from("entry"), find).await;
    let Found::Groups { rows } = reply.must("summing `amount` by `book`")?.data else {
        return Err(Failure::new("`find` with an aggregate must answer groups"));
    };
    let got: Vec<String> = (rows.iter())
        .map(|g| line(g.by.get("book"), ["total", "mean"].map(|m| g.values.get(m))))
        .collect();
    ensure!(
        got == want,
        "`find`'s sum and average of `amount` by `book` answered {got:?}; a sum reads at the \
         largest scale among its values, a zero's included, whatever order their keys give \
         them, and the average divides that sum: {want:?}"
    );

    let rows = all(cat.as_ref(), own, "book_total").await?;
    let got: Vec<String> = (rows.iter())
        .map(|r| line(value(r, "book"), ["total", "mean"].map(|m| value(r, m))))
        .collect();
    ensure!(
        got == want,
        "the view's `sum(amount)` and `avg(amount)` by `book` hold {got:?}; a view's sum reads \
         as `find`'s does: {want:?}"
    );
    Ok(())
}

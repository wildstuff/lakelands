//! What a `where` admits, and who may probe a field with one
//! (`docs/decisions/023-one-evaluator-decides-what-an-expression-means.md`,
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! `docs/decisions/010-provenance-per-field.md`).

use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::query::Projection;
use lakelands_api::read::{AggFunc, Aggregate, OrderBy};
use lakelands_api::{Find, LakeStore, Refusal, Timestamp};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, agg, field, field_writer, find_where, no_such_field, order, pattern, refused_on,
    source_record, sourced, type_decl, windows_reach_head,
};

check!(
    /// Absence never satisfies a comparison.
    ABSENT_NEVER_MATCHES = absent_fields_never_match_not_even_not_equal,
    "023: a comparison with an absent or null operand is false, `!=` included; `is null` is how absence is asked about"
);

check!(
    /// Numbers and numeric text compare by value.
    NUMBERS_COMPARE_AS_NUMBERS = numbers_and_numeric_text_compare_as_numbers,
    "023: when both sides read as numbers, a JSON number or a decimal string, they compare exactly as numbers, never as text"
);

check!(
    /// Values of kinds that do not compare match nothing.
    KINDS_THAT_DO_NOT_COMPARE = values_of_kinds_that_do_not_compare_match_nothing_and_fail_nothing,
    "023: text that is not a number against a number, a boolean against text: every operator is false, and the read does not fail"
);

check!(
    /// A read may not probe a field its principal may not read.
    PROBING_REFUSED = probing_an_unreadable_field_is_refused_whether_by_filter_order_or_aggregate,
    "024 and 006: filtering, ordering or aggregating over a field the principal sees masked, through `find` or through a pattern's rows and its aggregate, is refused as not granted, naming the field; over one it may not see at all, it is answered as for a field the type does not declare"
);

check!(
    /// Provenance can be filtered and ordered on.
    PROVENANCE_IN_WHERE_AND_ORDER = a_filter_and_an_order_can_read_when_a_source_saw_a_value,
    "010 and 017: `seen_at(f)` reads a field's provenance in `where`, and `order_by` may order by it"
);

async fn lines(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let ty = type_decl(
        "line",
        vec![
            field("label", StorageKind::Text),
            field("qty", StorageKind::Number),
            field("price", StorageKind::Decimal),
            field("note", StorageKind::Text),
            field("flag", StorageKind::Boolean),
        ],
    );
    l.declare(ty).await?;
    Ok(l)
}

async fn keys_where(store: &dyn LakeStore, filter: &str) -> Result<Vec<String>, Failure> {
    let rows = Lake::of(store)
        .all_found("line", find_where(filter))
        .await?;
    let mut keys: Vec<String> = rows.into_iter().map(|r| r.key.0).collect();
    keys.sort();
    Ok(keys)
}

async fn absent_fields_never_match_not_even_not_equal(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lines(store.as_ref()).await?;
    l.upsert("line", "with", json!({"note": "x"})).await?;
    l.upsert("line", "other", json!({"note": "y"})).await?;
    l.upsert("line", "without", json!({"label": "a"})).await?;

    for (filter, want) in [
        ("note = 'x'", vec!["with"]),
        ("note != 'x'", vec!["other"]),
        ("note < 'z'", vec!["other", "with"]),
        ("note is null", vec!["without"]),
        ("note is not null", vec!["other", "with"]),
        ("not (note = 'x')", vec!["other", "without"]),
    ] {
        let got = keys_where(store.as_ref(), filter).await?;
        ensure!(
            got == want,
            "`where {filter}` admitted {got:?}; expected {want:?}. A record without \
             `note` satisfies no comparison on it, `!=` included, only `is null` \
             (and `not` of a false comparison)"
        );
    }
    Ok(())
}

async fn numbers_and_numeric_text_compare_as_numbers(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lines(store.as_ref()).await?;
    l.upsert("line", "nine", json!({"qty": 9, "price": "9.50"}))
        .await?;
    l.upsert("line", "ten", json!({"qty": 10, "price": "10.00"}))
        .await?;
    l.upsert("line", "hundred", json!({"qty": 100, "price": "100"}))
        .await?;

    for (filter, want) in [
        ("qty > 9", vec!["hundred", "ten"]),
        ("price > 9.5", vec!["hundred", "ten"]),
        ("price >= '10'", vec!["hundred", "ten"]),
        ("price = 10", vec!["ten"]),
        ("price < 10.001", vec!["nine", "ten"]),
        ("qty = price", vec!["hundred", "ten"]),
    ] {
        let got = keys_where(store.as_ref(), filter).await?;
        ensure!(
            got == want,
            "`where {filter}` admitted {got:?}; expected {want:?}. Numbers and decimal \
             strings compare as exact numbers, so '100' is above '9.50' and 10.00 \
             equals 10"
        );
    }
    Ok(())
}

async fn values_of_kinds_that_do_not_compare_match_nothing_and_fail_nothing(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lines(store.as_ref()).await?;
    l.upsert("line", "a", json!({"label": "abc", "qty": 1, "flag": true}))
        .await?;

    for filter in [
        "label = 1",
        "label != 1",
        "label > 0",
        "flag = 'true'",
        "flag != 'true'",
        "qty = 'one'",
        "qty != 'one'",
    ] {
        let got = keys_where(store.as_ref(), filter).await?;
        ensure!(
            got.is_empty(),
            "`where {filter}` admitted {got:?}; values of kinds that do not compare \
             (text that is not a number against a number, a boolean against text) \
             match under no operator, `!=` included, and the read must not fail"
        );
    }
    Ok(())
}

async fn probing_an_unreadable_field_is_refused_whether_by_filter_order_or_aggregate(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lines(store.as_ref()).await?;
    let Lake { cat, .. } = &l;
    l.upsert("line", "a", json!({"qty": 1, "note": "secret"}))
        .await?;

    let line = "line".into();
    for access in [lakelands_api::Access::None, lakelands_api::Access::Masked] {
        // Reads everything but `note`, which it sees masked or not at all.
        let mut reader = field_writer(store.as_ref(), "reader", "line", &[]);
        reader
            .grants
            .types
            .get_mut(&lakelands_api::TypeName::from("line"))
            .expect("field_writer grants the type")
            .fields
            .insert("note".into(), access);

        let probes: [(&str, Find); 3] = [
            ("a filter", find_where("note = 'secret'")),
            (
                "an order",
                Find {
                    order_by: vec![order("note", OrderBy::Value, Default::default())],
                    ..Find::default()
                },
            ),
            (
                "an aggregate",
                Find {
                    aggregate: Some(Aggregate {
                        measures: vec![agg("n", AggFunc::Count, None)],
                        group_by: vec!["note".into()],
                    }),
                    ..Find::default()
                },
            ),
        ];
        // A pattern's rows ordered by `line.note`, and summed over it.
        let mut ordered = pattern(
            "line",
            None,
            Vec::new(),
            Projection::Rows { fields: vec![] },
        );
        ordered.order_by = vec![order("line.note", OrderBy::Value, Default::default())];
        let summed = Aggregate {
            measures: vec![agg("s", AggFunc::Sum, Some("line.note"))],
            group_by: Vec::new(),
        };
        let summed = pattern("line", None, Vec::new(), Projection::Aggregate(summed));
        let answers = [
            (
                "a pattern's rows ordered by it",
                cat.query(&reader, ordered).await.map(drop),
            ),
            (
                "a pattern's sum of it",
                cat.query(&reader, summed).await.map(drop),
            ),
        ];
        for (how, r) in answers {
            let answered = match access {
                lakelands_api::Access::None => no_such_field(&r, "note"),
                _ => matches!(refused_on(&r), Some(("note", Refusal::NotGranted))),
            };
            ensure!(
                answered,
                "probing `note` ({access:?}) with {how} is refused as `find` refuses it, never \
                 answered as if complete; it answered {r:?}"
            );
        }
        for (how, find) in probes {
            let err = cat
                .find(&reader, &line, find)
                .await
                .must_fail(&format!("probing the unreadable `note` with {how}"))?;
            let answered = match access {
                lakelands_api::Access::None => no_such_field(&err, "note"),
                _ => matches!(refused_on(&err), Some(("note", Refusal::NotGranted))),
            };
            ensure!(
                answered,
                "probing `note` ({access:?}) with {how} must be refused naming `note`: as not \
             granted where it is masked, as for an undeclared field where it is hidden; it \
             failed with {err:?}"
            );
        }
        cat.find(&reader, &line, find_where("qty = 1"))
            .await
            .must("filtering on the readable `qty`")?;
    }
    Ok(())
}

async fn a_filter_and_an_order_can_read_when_a_source_saw_a_value(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let ty = type_decl("reading", vec![sourced("level", StorageKind::Number)]);
    l.declare(ty).await?;
    let seen = |day: u32| Timestamp::from_date(2026, 3, day);
    let mut records = Vec::new();
    for (key, day) in [("a", 20), ("b", 10), ("c", 15)] {
        let mut r = source_record("reading", key, json!({"level": day}));
        r.seen_at = seen(day);
        records.push(r);
    }
    let windows = l
        .ingest("gauge", records)
        .await
        .must("ingesting three readings with their seen_at")?;
    windows_reach_head(store.as_ref(), &windows, "ingesting three readings").await?;

    let find = Find {
        filter: Some("seen_at(level) < '2026-03-16'".into()),
        order_by: vec![order(
            "level",
            OrderBy::SeenAt,
            lakelands_api::read::Direction::Desc,
        )],
        ..Find::default()
    };
    let rows = l.all_found("reading", find).await?;
    let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    ensure!(
        keys == ["c", "b"],
        "`where seen_at(level) < '2026-03-16'` ordered by `level`'s seen_at, newest \
         first, answered {keys:?}; the source saw `b` on the 10th, `c` on the 15th and \
         `a` on the 20th, so the answer is c, b"
    );
    Ok(())
}

check!(
    /// A cleared value matches nothing; its record is still there.
    CLEARED_FIELD = a_cleared_field_matches_nothing_but_its_record_is_found_by_another,
    "012 and 023: a field set to null is absent, so no comparison on it holds, and the record is found by its other fields as before"
);

async fn a_cleared_field_matches_nothing_but_its_record_is_found_by_another(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lines(store.as_ref()).await?;
    l.upsert("line", "a", json!({"label": "keep", "note": "gone"}))
        .await?;
    l.upsert("line", "a", json!({"note": null})).await?;
    let by_note = keys_where(store.as_ref(), "note = 'gone'").await?;
    let by_label = keys_where(store.as_ref(), "label = 'keep' and note is null").await?;
    ensure!(
        by_note.is_empty() && by_label == ["a"],
        "after `note` was cleared, `where note = 'gone'` must admit nothing and \
         `where label = 'keep' and note is null` must admit `a`; they admitted \
         {by_note:?} and {by_label:?}"
    );
    Ok(())
}

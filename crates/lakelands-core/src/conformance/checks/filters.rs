//! What a `where` admits, and who may probe a field with one
//! (`docs/decisions/023-one-evaluator-decides-what-an-expression-means.md`,
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! `docs/decisions/010-provenance-per-field.md`).

use lakelands_api::seam::SETTING_TYPE;

use crate::conformance::fixture::{order, source_record};
use crate::conformance::prelude::*;

check!(
    ABSENT_NEVER_MATCHES = absent_fields_never_match_not_even_not_equal,
    "023: a comparison with an absent or null operand is false, `!=` included; `is null` is how absence is asked about"
);

check!(
    NUMBERS_COMPARE_AS_NUMBERS = numbers_and_numeric_text_compare_as_numbers,
    "023: when both sides read as numbers, a JSON number or a decimal string, they compare exactly as numbers, never as text"
);

check!(
    KINDS_THAT_DO_NOT_COMPARE = values_of_kinds_that_do_not_compare_match_nothing_and_fail_nothing,
    "023: text that is not a number against a number, a boolean against text: every operator is false, and the read does not fail"
);

check!(
    PROBING_REFUSED = probing_an_unreadable_field_is_refused_whether_by_filter_order_or_aggregate,
    "024 and 006: filtering, ordering or aggregating over a field the principal sees masked, through `find` or through a pattern's rows and its aggregate, is refused as not granted, naming the field; over one it may not see at all, it is answered as for a field the type does not declare"
);

check!(
    PROVENANCE_IN_WHERE_AND_ORDER = a_filter_and_an_order_can_read_when_a_source_saw_a_value,
    "010 and 017: `seen_at(f)` reads a field's provenance in `where`, and `order_by` may order by it"
);

check!(
    LOOKED_UP_OR_SCANNED = a_text_scanned_for_answers_as_one_looked_up,
    "057: a `where` on a text field the exact-text index keeps no rows for answers as it does once `lookup_index.<type>` or a set's condition looks the field up, and again once the setting is retired; the setting refuses a field that is no text, naming it"
);

check!(
    LONG_TEXT_AND_NUL = a_long_text_is_found_and_a_text_holding_nul_is_refused,
    "057: a text of 10 000 characters is found by equality whether its field is looked up or not, and the setting is written over it; a text holding U+0000 is refused as invalid, naming its field"
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

/// What `line` answers by its `label`, unordered, narrowed further and
/// ordered by `qty`, and for a text none holds.
async fn by_label(store: &dyn LakeStore) -> Result<Vec<Vec<String>>, Failure> {
    let mut out = Vec::new();
    for filter in ["label = 'x'", "label = 'x' and qty > 1", "label = 'z'"] {
        out.push(keys_where(store, filter).await?);
    }
    let find = Find {
        order_by: vec![order("qty", OrderBy::Value, Direction::Desc)],
        ..find_where("label = 'x'")
    };
    let rows = Lake::of(store).all_found("line", find).await?;
    out.push(rows.into_iter().map(|r| r.key.0).collect());
    Ok(out)
}

async fn a_text_scanned_for_answers_as_one_looked_up(store: Store) -> Held {
    let l = lines(store.as_ref()).await?;
    for (key, label, qty) in [("l1", "x", 1), ("l2", "y", 2), ("l3", "x", 3)] {
        l.upsert("line", key, json!({"label": label, "qty": qty}))
            .await?;
    }
    let want = [vec!["l1", "l3"], vec!["l3"], vec![], vec!["l3", "l1"]];
    let scanned = by_label(store.as_ref()).await?;
    ensure!(
        scanned == want,
        "scanned for, `label` answered {scanned:?}, not {want:?}"
    );
    let setting = "lookup_index.line";
    l.set(setting, json!(["label"])).await?;
    let looked = by_label(store.as_ref()).await?;
    l.retire(SETTING_TYPE, setting).await?;
    let retired = by_label(store.as_ref()).await?;
    l.declare_as(object_set_on("marked", "line", "label = 'x'"), "a set")
        .await?;
    let by_set = by_label(store.as_ref()).await?;
    for (how, got) in [
        ("the setting", looked),
        ("its retirement", retired),
        ("a set", by_set),
    ] {
        ensure!(
            got == scanned,
            "after {how}, `label` answered {got:?}, scanned {scanned:?}"
        );
    }
    let qty = json!({"key": setting, "value": ["qty"]});
    let refused = try_upsert(l.cat.as_ref(), &l.own, SETTING_TYPE, setting, qty).await;
    ensure!(
        refused_on(&refused).is_some_and(|(f, _)| f == "qty"),
        "a number field asked of the exact-text index: {refused:?}"
    );
    Ok(())
}

async fn a_long_text_is_found_and_a_text_holding_nul_is_refused(store: Store) -> Held {
    let l = lines(store.as_ref()).await?;
    // Ten thousand characters no compression shortens much.
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let long: String = (0..10_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            char::from(b"abcdefghijklmnopqrstuvwxyz0123456789"[(x % 36) as usize])
        })
        .collect();
    l.upsert("line", "long", json!({"note": long})).await?;
    for looked in [false, true] {
        if looked {
            l.set("lookup_index.line", json!(["note"])).await?;
        }
        let found = keys_where(store.as_ref(), &format!("note = '{long}'")).await?;
        ensure!(
            found == ["long"],
            "looked up {looked}, a long text answered {found:?}"
        );
    }
    let nul = json!({"note": "a\u{0}b"});
    let refused = try_upsert(l.cat.as_ref(), &l.own, "line", "nul", nul).await;
    ensure!(
        refused_on(&refused).is_some_and(|(f, _)| f == "note"),
        "a text holding U+0000: {refused:?}"
    );
    Ok(())
}

async fn absent_fields_never_match_not_even_not_equal(store: Store) -> Held {
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

async fn numbers_and_numeric_text_compare_as_numbers(store: Store) -> Held {
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

async fn values_of_kinds_that_do_not_compare_match_nothing_and_fail_nothing(store: Store) -> Held {
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
    store: Store,
) -> Held {
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

        let probes = |f: &str| -> [(&'static str, Find); 3] {
            [
                ("a filter", find_where(&format!("{f} = 'secret'"))),
                (
                    "an order",
                    Find {
                        order_by: vec![order(f, OrderBy::Value, Default::default())],
                        ..Find::default()
                    },
                ),
                (
                    "an aggregate",
                    Find {
                        aggregate: Some(Aggregate {
                            measures: vec![agg("n", AggFunc::Count, None)],
                            group_by: vec![f.into()],
                        }),
                        ..Find::default()
                    },
                ),
            ]
        };
        // A pattern's rows ordered by `line.note`, and summed over it.
        let ordered = |f: &str| {
            let mut ordered = pattern(
                "line",
                None,
                Vec::new(),
                Projection::Rows { fields: vec![] },
            );
            ordered.order_by = vec![order(f, OrderBy::Value, Default::default())];
            ordered
        };
        let summed = |f: &str| {
            let summed = Aggregate {
                measures: vec![agg("s", AggFunc::Sum, Some(f))],
                group_by: Vec::new(),
            };
            pattern("line", None, Vec::new(), Projection::Aggregate(summed))
        };
        let answers = [
            (
                "a pattern's rows ordered by it",
                cat.query(&reader, ordered("line.note")).await.map(drop),
                cat.query(&reader, ordered("line.nosuch")).await.map(drop),
            ),
            (
                "a pattern's sum of it",
                cat.query(&reader, summed("line.note")).await.map(drop),
                cat.query(&reader, summed("line.nosuch")).await.map(drop),
            ),
        ];
        for (how, r, absent) in answers {
            let answered = match access {
                lakelands_api::Access::None => no_such_field(&r, "note"),
                _ => matches!(refused_on(&r), Some(("note", Refusal::NotGranted))),
            };
            // Hidden, word for word and member for member as `nosuch`.
            if let (lakelands_api::Access::None, Err(e), Err(absent)) = (access, &r, &absent) {
                alike_renamed(how, e, absent, ("nosuch", "note"))?;
            }
            ensure!(
                answered,
                "probing `note` ({access:?}) with {how} is refused as `find` refuses it, never \
                 answered as if complete; it answered {r:?}"
            );
        }
        for ((how, find), (_, absent)) in probes("note").into_iter().zip(probes("nosuch")) {
            let err = cat
                .find(&reader, &line, find)
                .await
                .must_fail(&format!("probing the unreadable `note` with {how}"))?;
            // Hidden, word for word and member for member as `nosuch`.
            if access == lakelands_api::Access::None {
                let absent = cat.find(&reader, &line, absent).await;
                let absent = absent.must_fail(&format!("probing `nosuch` with {how}"))?;
                alike_renamed(how, &err, &absent, ("nosuch", "note"))?;
            }
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
        let fut = cat.find(&reader, &line, find_where("qty = 1"));
        fut.await.must("filtering on the readable `qty`")?;
    }
    Ok(())
}

async fn a_filter_and_an_order_can_read_when_a_source_saw_a_value(store: Store) -> Held {
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
    COMPARED_AS_ITS_INSTANT = a_text_compared_with_an_instant_is_read_as_the_instant_it_denotes,
    "023 and the log format §1.3: a text that is only compared or computed with is read as the instant it denotes, out of the years 0000 to 9999 too, so `'9999-12-31T23:30:00-01:00'` is after every instant a field holds and `'0000-01-01T00:30:00+01:00'` before every one, where no field takes either written: at a find, a pattern's `where`, a view's `where` and a measure, a value type's `constraint`, an `expr` function, `in` on either side, a literal wrapped in `if` or `coalesce` or subtracted from, and two literals; a text field's value that denotes an instant compares as one too, and one that denotes none is a text"
);

check!(
    CLEARED_FIELD = a_cleared_field_matches_nothing_but_its_record_is_found_by_another,
    "012 and 023: a field set to null is absent, so no comparison on it holds, and the record is found by its other fields as before"
);

async fn a_cleared_field_matches_nothing_but_its_record_is_found_by_another(store: Store) -> Held {
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

async fn a_text_compared_with_an_instant_is_read_as_the_instant_it_denotes(store: Store) -> Held {
    use StorageKind::{Boolean, Date, DateTime, Number, Text};
    let l = Lake::of(store.as_ref());
    // Half an hour past the last year, and half an hour before the first.
    let (high, low) = ("9999-12-31T23:30:00-01:00", "0000-01-01T00:30:00+01:00");
    let between = |of: &str| format!("{of} < '{high}' and {of} > '{low}'");
    l.declare_json(
        json!({"declare": "value_type", "name": "stamp", "kind": "date_time",
        "constraint": between("value")}),
    )
    .await?;
    l.declare_json(
        json!({"declare": "function", "name": "within", "backing": "expr",
        "inputs": [{"name": "at", "type": {"kind": "date_time"}}], "expr": between("at")}),
    )
    .await?;
    let mut stamped = field("at", DateTime);
    stamped.ty.value_type = Some("stamp".into());
    l.declare(type_decl("stamped", vec![stamped])).await?;
    let fields = vec![
        field("at", DateTime),
        field("on", Date),
        field("name", Text),
        field("group", Text),
        computed_by("inside", Boolean, "within", &["at"]),
    ];
    l.declare(type_decl("thing", fields)).await?;
    let mut grouped = view_of(
        "grouped",
        "thing",
        &["group"],
        vec![
            field("group", Text),
            field("n", Number),
            field("early", Number),
        ],
        &[
            ("n", "count()"),
            ("early", &format!("sum(if(at < '{high}', 1, 0))")),
        ],
    );
    grouped.view.as_mut().expect("a view").r#where = Some(format!("at > '{low}'").as_str().into());
    l.declare(grouped).await?;
    // The first and the last instant a field holds, and one between. The
    // value type's constraint takes each.
    let mut red = Vec::new();
    for (key, at, on, name) in [
        ("first", "0000-01-01T00:00:00Z", "0000-01-01", high),
        ("mid", "2026-09-24T05:57:11Z", "2026-09-24", "plain"),
        ("last", "9999-12-31T23:59:59.999999Z", "9999-12-31", high),
    ] {
        let fields = json!({"at": at, "on": on, "name": name, "group": "g"});
        l.upsert("thing", key, fields).await?;
        let stamped = try_upsert(l.cat.as_ref(), &l.own, "stamped", key, json!({ "at": at }));
        if let Err(e) = stamped.await {
            red.push(format!("the constraint refused {at}: {e}"));
        }
    }
    // The same instant as `high`, written with another offset.
    let same = "9999-12-31T22:30:00-02:00";
    for (filter, n) in [
        (format!("at < '{high}'"), 3),
        (format!("'{high}' > at"), 3),
        (format!("at > '{low}'"), 3),
        (format!("on < '{high}' and on > '{low}'"), 3),
        (format!("at < if(true, '{high}', '{high}')"), 3),
        (format!("at > coalesce('{low}', '{low}')"), 3),
        (format!("'{high}' - at < 4000000d"), 3),
        (format!("'{high}' in ('x', '{same}')"), 3),
        (format!("'{same}' in ('{high}')"), 3),
        (format!("'{high}' = '{same}'"), 3),
        (format!("'{high}' > '9999-12-31T23:45:00+00:00'"), 3),
        (format!("'{low}' < '0000-01-01T00:00:00Z'"), 3),
        (format!("at in ('{high}', '{low}')"), 0),
        // A text field's value that denotes an instant compares as one
        // too, as two literals do; one that denotes none is a text.
        (format!("name = '{same}'"), 2),
        ("name = 'plain'".to_owned(), 1),
        ("name > 'pl' and name < 'pm'".to_owned(), 1),
    ] {
        let found = l.all_found("thing", find_where(&filter)).await?;
        if found.len() != n {
            red.push(format!(
                "a find where {filter} found {} of {n}",
                found.len()
            ));
        }
        let mut walked = pattern("thing", None, Vec::new(), Projection::default());
        walked.start.filter = Some(filter.as_str().into());
        let rows = l.cat.query(&l.own, walked).await;
        let rows = match rows.must("a pattern with the filter")?.data {
            QueryResult::Rows { rows, .. } => rows.len(),
            _ => usize::MAX,
        };
        if rows != n {
            red.push(format!("a pattern where {filter} answered {rows} of {n}"));
        }
    }
    let group = l.read("grouped", "g").await?;
    for measure in ["n", "early"] {
        if value(&group, measure) != Some(&json!(3)) {
            red.push(format!(
                "the view's `{measure}` reads {:?}, not 3",
                value(&group, measure)
            ));
        }
    }
    for key in ["first", "mid", "last"] {
        let inside = l.read("thing", key).await?;
        if value(&inside, "inside") != Some(&json!(true)) {
            red.push(format!(
                "`inside` of `{key}` reads {:?}",
                value(&inside, "inside")
            ));
        }
    }
    ensure!(
        red.is_empty(),
        "`{high}` denotes an instant after every one a field holds and `{low}` one before, \
         and each is read so wherever it is compared: {}",
        red.join("; ")
    );
    Ok(())
}

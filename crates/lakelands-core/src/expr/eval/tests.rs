use super::*;
use crate::expr::parse;
use serde_json::json;

fn fields(v: Value) -> BTreeMap<FieldName, Value> {
    serde_json::from_value(v).unwrap()
}

fn run(text: &str, record: Value) -> Result<Val> {
    let f = fields(record);
    eval(
        &parse(text).unwrap(),
        &RecordScope {
            fields: &f,
            meta: None,
            settings: None,
            now: Timestamp::parse("2026-09-24T10:00:00Z").unwrap(),
        },
    )
}

/// `run`'s answer, which must be a value.
fn ok(text: &str, record: Value) -> Val {
    run(text, record).unwrap()
}

fn num(s: &str) -> Val {
    Val::Num(Decimal::from_str(s).unwrap())
}

fn text(s: &str) -> Val {
    Val::Text(s.into())
}

fn yes(text: &str, record: Value) -> bool {
    run(text, record).map(|v| truthy(&v)).unwrap()
}

#[test]
fn date_bucket_answers_the_utc_start_of_its_bucket() {
    let bucket = |d: &str, unit: &str| ok(&format!("date_bucket(d, '{unit}')"), json!({"d": d}));
    let t = |s: &str| Val::Time(Timestamp::parse(s).unwrap());
    let d = "2026-08-19T17:45:12.250000Z";
    assert_eq!(bucket(d, "day"), t("2026-08-19T00:00:00Z"));
    assert_eq!(bucket(d, "week"), t("2026-08-17T00:00:00Z"));
    assert_eq!(bucket(d, "month"), t("2026-08-01T00:00:00Z"));
    assert_eq!(bucket(d, "quarter"), t("2026-07-01T00:00:00Z"));
    assert_eq!(bucket(d, "year"), t("2026-01-01T00:00:00Z"));
    // ISO weeks start on Monday, across the year's end: 2026-01-01 is
    // a Thursday, 2026-12-31 a Thursday, 2024-12-29 a Sunday.
    assert_eq!(bucket("2026-01-01", "week"), t("2025-12-29T00:00:00Z"));
    assert_eq!(bucket("2026-12-31", "week"), t("2026-12-28T00:00:00Z"));
    assert_eq!(bucket("2024-12-29", "week"), t("2024-12-23T00:00:00Z"));
    assert_eq!(bucket("2024-12-30", "week"), t("2024-12-30T00:00:00Z"));
    // A leap day, and before 1970.
    assert_eq!(bucket("2028-02-29", "month"), t("2028-02-01T00:00:00Z"));
    assert_eq!(
        bucket("1969-12-31T23:59:59Z", "day"),
        t("1969-12-31T00:00:00Z")
    );
    assert_eq!(
        bucket("1969-12-31T23:59:59Z", "week"),
        t("1969-12-29T00:00:00Z")
    );
    // Missing, or not an instant: null.
    assert_eq!(ok("date_bucket(d, 'day')", json!({})), Val::Null);
    assert_eq!(ok("date_bucket(d, 'day')", json!({"d": "soon"})), Val::Null);
    assert_eq!(ok("date_bucket(d, 'day')", json!({"d": 7})), Val::Null);
}

#[test]
fn date_bucket_by_a_width_counts_its_windows_from_the_origin_both_ways() {
    let t = |s: &str| Val::Time(Timestamp::parse(s).unwrap());
    let from = |d: Value, o: Value| ok("date_bucket(d, 30d, o)", json!({"d": d, "o": o}));
    let window = |d: &str| from(json!(d), json!("2026-01-01T00:00:00Z"));
    assert_eq!(window("2026-01-01T00:00:00Z"), t("2026-01-01T00:00:00Z"));
    assert_eq!(window("2026-01-30T23:59:59Z"), t("2026-01-01T00:00:00Z"));
    assert_eq!(window("2026-01-31T00:00:00Z"), t("2026-01-31T00:00:00Z"));
    // Before the origin: the window that starts thirty days before it.
    assert_eq!(window("2025-12-31T23:59:59Z"), t("2025-12-02T00:00:00Z"));
    // The origin may be the clock's: `today()` is 2026-09-24 here.
    let halves = ok(
        "date_bucket(d, 12h, today())",
        json!({"d": "2026-09-23T13:00:00Z"}),
    );
    assert_eq!(halves, t("2026-09-23T12:00:00Z"));
    // No instant, or no origin: null.
    assert_eq!(from(Value::Null, json!("2026-01-01T00:00:00Z")), Val::Null);
    assert_eq!(from(json!("2026-01-01T00:00:00Z"), Value::Null), Val::Null);
}

#[test]
fn date_bucket_takes_one_of_its_units_quoted() {
    for (text, why) in [
        ("date_bucket(d, 'fortnight')", "knows no unit 'fortnight'"),
        ("date_bucket(d, unit)", "takes a calendar unit quoted"),
        ("date_bucket(d, 30d)", "date_bucket(t, 30d, origin)"),
        ("date_bucket(d, '30d', d)", "a width is written unquoted"),
        ("date_bucket(d, 0d, d)", "a width above zero"),
        ("date_bucket(d, 'month', d)", "and no origin"),
        ("date_bucket(d)", "takes 2 to 3 argument(s), not 1"),
    ] {
        // Refused by the parser or by the check, whichever meets it.
        let e = match parse(text) {
            Ok(e) => check(&e, Place::Record).unwrap_err().to_string(),
            Err(e) => e.to_string(),
        };
        assert!(e.contains(why), "{text}: {e}");
    }
}

#[test]
fn absent_fields_never_match_not_even_not_equal() {
    for cond in [
        "status = 'open'",
        "status != 'open'",
        "amount > 1",
        "amount <= 1",
    ] {
        assert!(!yes(cond, json!({})), "{cond} matched an absent field");
        assert!(
            !yes(cond, json!({"status": null, "amount": null})),
            "{cond} matched null"
        );
    }
    assert!(yes("status is null", json!({})));
    assert!(yes("coalesce(status, 'none') = 'none'", json!({})));
}

#[test]
fn instants_compare_as_instants_whatever_their_written_form() {
    let r = json!({"due": "2026-09-01T00:00:00.000000Z"});
    assert!(yes("due = '2026-09-01'", r.clone()));
    assert!(yes("due < '2026-09-01T02:00:00+01:00'", r.clone()));
    assert!(yes("due < today() - 20d", r));
}

#[test]
fn booleans_equal_only_booleans_and_have_no_order() {
    assert!(yes("flag = true", json!({"flag": true})));
    assert!(!yes("flag = 1", json!({"flag": true})));
    assert!(!yes("flag > false", json!({"flag": true})));
    assert!(!yes("flag < false", json!({"flag": true})));
}

#[test]
fn values_of_kinds_that_do_not_compare_never_match_and_never_fail() {
    for cond in [
        "amount = 'abc'",
        "amount != 'abc'",
        "amount < 'abc'",
        "amount >= 'abc'",
        "doc = 1",
        "doc != 1",
        "doc > 1",
        "flag != 'x'",
    ] {
        let r = run(cond, json!({"amount": 5, "doc": {"a": 1}, "flag": true}));
        assert_eq!(r, Ok(Val::Bool(false)), "{cond}");
    }
    assert!(yes(
        "doc = other",
        json!({"doc": {"a": 1}, "other": {"a": 1}})
    ));
    assert!(
        yes("code < 'b'", json!({"code": "a10"})),
        "text against text still orders"
    );
}

#[test]
fn a_typed_call_with_the_wrong_arguments_is_an_error_not_a_panic() {
    use lakelands_api::expr::{Expr, Func};
    let f = BTreeMap::new();
    let scope = RecordScope {
        fields: &f,
        meta: None,
        settings: None,
        now: Timestamp(0),
    };
    for (func, n) in [
        (Func::Upper, 0),
        (Func::If, 1),
        (Func::Substr, 1),
        (Func::Setting, 0),
        (Func::Replace, 2),
        (Func::Today, 1),
    ] {
        let e = Expr::Call {
            func,
            args: vec![
                Expr::Lit {
                    value: lakelands_api::expr::Literal::Null
                };
                n
            ],
        };
        assert!(eval(&e, &scope).is_err(), "{func:?} with {n}");
        assert!(check(&e, Place::Record).is_err(), "{func:?} with {n}");
    }
}

#[test]
fn check_places_guard_functions_and_aggregates() {
    let c = |t: &str, p: Place| check(&parse(t).unwrap(), p);
    assert!(c("changed(status)", Place::Write).is_ok());
    assert!(c("changed(status)", Place::Record).is_err());
    assert!(c("sum(amount) > 1", Place::Group).is_ok());
    assert!(c("sum(amount) > 1", Place::Record).is_err());
    assert!(
        c("sum(changed(amount))", Place::Group).is_err(),
        "an aggregate's argument is per record"
    );
    assert!(c("status in ('a')", Place::Record).is_ok());
    assert!(c("changed(status)", Place::Guard).is_ok());
    assert!(c("retired()", Place::Guard).is_ok());
    assert!(c("retired()", Place::Write).is_err(), "a derived field");
    assert!(c("retired()", Place::Record).is_err());
    assert!(parse("retired(status)").is_err(), "it takes no argument");
}

#[test]
fn a_filter_reads_settings_as_of_the_position_read() {
    let f = fields(json!({"amount": "100"}));
    let settings: BTreeMap<String, Value> = [("floor".to_owned(), json!("50"))].into();
    let scope = RecordScope {
        fields: &f,
        meta: None,
        settings: Some(&settings),
        now: Timestamp(0),
    };
    assert!(holds(&parse("amount > setting('floor')").unwrap(), &scope).unwrap());
    assert!(!holds(&parse("setting('missing') is not null").unwrap(), &scope).unwrap());
}

#[test]
fn a_dotted_path_digs_into_a_json_field_of_the_record() {
    assert!(yes("doc.a = 1", json!({"doc": {"a": 1}})));
    assert!(yes("doc.b is null", json!({"doc": {"a": 1}})));
    assert!(
        yes("name.first is null", json!({"name": "x"})),
        "text has no members"
    );
}

#[test]
fn matches_refuses_what_is_not_i_regexp() {
    assert!(run("matches(a, 'a)|(b')", json!({"a": "b"})).is_err());
    assert!(run("matches(a, '\\d+')", json!({"a": "12"})).is_err());
}

#[test]
fn decimal_arithmetic_is_exact_and_checked() {
    assert_eq!(ok("0.1 + 0.2", json!({})), num("0.3"));
    assert_eq!(ok("a * 1.19", json!({"a": "100.00"})), num("119.0000"));
    assert!(run("1 / 0", json!({})).is_err());
    assert!(run("a + 1", json!({"a": "x"})).is_err());
    assert_eq!(ok("a + 1", json!({})), Val::Null);
}

#[test]
fn a_float_enters_by_its_shortest_decimal_form() {
    assert_eq!(ok("a", json!({"a": 0.1})), num("0.1"));
}

#[test]
fn and_or_and_if_are_lazy() {
    assert!(yes("b != 0 and a / b > 1", json!({"a": 4, "b": 2})));
    assert!(!yes("b != 0 and a / b > 1", json!({"a": 4, "b": 0})));
    assert_eq!(
        ok("if(b != 0, a / b, 0)", json!({"a": 1, "b": 0})),
        Val::Num(Decimal::ZERO)
    );
    assert!(yes("b = 0 or a / b > 1", json!({"a": 1, "b": 0})));
}

#[test]
fn text_functions_count_characters() {
    assert_eq!(
        ok("len(name)", json!({"name": "Müller"})),
        Val::Num(Decimal::from(6))
    );
    assert_eq!(
        ok("substr(name, 1, 3)", json!({"name": "Müller"})),
        text("üll")
    );
    assert_eq!(ok("substr(name, 4)", json!({"name": "Müller"})), text("er"));
    assert_eq!(
        ok("substr(name, 99)", json!({"name": "ab"})),
        Val::Text(String::new())
    );
    assert_eq!(
        ok("replace(name, '', 'x')", json!({"name": "ab"})),
        text("ab")
    );
    assert_eq!(
        ok("concat(a, '-', b)", json!({"a": "x", "b": 2})),
        text("x-2")
    );
    assert_eq!(ok("upper(a)", json!({"a": "straße"})), text("STRASSE"));
}

#[test]
fn matches_is_whole_text() {
    assert!(yes(
        "matches(iban, 'DE[0-9]{20}')",
        json!({"iban": "DE89370400440532013000"})
    ));
    assert!(!yes(
        "matches(iban, 'DE[0-9]{2}')",
        json!({"iban": "DE89370400440532013000"})
    ));
    assert!(run("matches(a, '(')", json!({"a": "x"})).is_err());
}

#[test]
fn round_is_half_away_from_zero() {
    assert_eq!(ok("round(2.345, 2)", json!({})), num("2.35"));
    assert_eq!(ok("round(-2.345, 2)", json!({})), num("-2.35"));
    assert_eq!(ok("round(2.5)", json!({})), Val::Num(Decimal::from(3)));
}

#[test]
fn the_clock_is_the_scopes() {
    assert_eq!(
        ok("now()", json!({})),
        Val::Time(Timestamp::parse("2026-09-24T10:00:00Z").unwrap())
    );
    assert_eq!(
        ok("today()", json!({})),
        Val::Time(Timestamp::parse("2026-09-24").unwrap())
    );
}

#[test]
fn changed_and_previous_read_the_write() {
    let before = fields(json!({"status": "open", "amount": "21.00"}));
    let after = fields(json!({"status": "paid", "amount": 21}));
    let scope = WriteScope {
        after: &after,
        before: Some(&before),
        retiring: false,
        meta: None,
        settings: None,
        now: Timestamp(0),
    };
    let holds_in = |t: &str| holds(&parse(t).unwrap(), &scope).unwrap();
    assert!(holds_in(
        "changed(status) and previous(status) = 'open' and status = 'paid'"
    ));
    assert!(
        !holds_in("changed(amount)"),
        "21.00 and 21 are the same amount"
    );
    let created = WriteScope {
        before: None,
        ..scope
    };
    assert!(holds(&parse("changed(status)").unwrap(), &created).unwrap());
    assert!(holds(&parse("previous(status) is null").unwrap(), &created).unwrap());
}

#[test]
fn retired_reads_whether_the_write_retires() {
    let fields = fields(json!({"status": "closed"}));
    let update = WriteScope {
        after: &fields,
        before: Some(&fields),
        retiring: false,
        meta: None,
        settings: None,
        now: Timestamp(0),
    };
    let retire = WriteScope {
        retiring: true,
        ..update
    };
    let holds_in = |t: &str, s: &WriteScope| holds(&parse(t).unwrap(), s).unwrap();
    assert!(!holds_in("retired()", &update));
    assert!(holds_in("retired()", &retire));
    assert!(holds_in("status = 'closed' and not retired()", &update));
    assert!(!holds_in("status = 'closed' and not retired()", &retire));
    let scope = RecordScope {
        fields: &fields,
        meta: None,
        settings: None,
        now: Timestamp(0),
    };
    assert!(
        eval(&parse("retired()").unwrap(), &scope).is_err(),
        "outside a write it has no meaning"
    );
}

#[test]
fn guard_functions_have_no_meaning_outside_a_write() {
    assert!(run("changed(status)", json!({})).is_err());
    assert!(run("sum(amount)", json!({})).is_err());
}

#[test]
fn settings_and_provenance_come_from_the_scope() {
    let after = fields(json!({"amount": "100"}));
    let settings: BTreeMap<String, Value> = [("vat_rate".to_owned(), json!("0.19"))].into();
    let meta: BTreeMap<FieldName, FieldMeta> = [(
        FieldName::from("amount"),
        FieldMeta {
            upstream: None,
            origin: lakelands_api::Origin::Source,
            known_at: Timestamp::parse("2026-09-01").unwrap(),
            seen_at: None,
            position: lakelands_api::Position(3),
            writer: "service:importer".into(),
            source: None,
            procedure_version: None,
            error: None,
        },
    )]
    .into();
    let scope = WriteScope {
        after: &after,
        before: None,
        retiring: false,
        meta: Some(&meta),
        settings: Some(&settings),
        now: Timestamp(0),
    };
    let v = eval(&parse("amount * setting('vat_rate')").unwrap(), &scope).unwrap();
    assert_eq!(v, num("19.00"));
    assert!(
        holds(
            &parse("known_at(amount) < '2026-09-02' and seen_at(amount) is null").unwrap(),
            &scope
        )
        .unwrap()
    );
}

#[test]
fn time_arithmetic() {
    assert_eq!(
        ok("due + 1d", json!({"due": "2026-09-01T00:00:00Z"})),
        Val::Time(Timestamp::parse("2026-09-02").unwrap())
    );
    assert_eq!(
        ok("b - a", json!({"a": "2026-09-01", "b": "2026-09-02"})),
        Val::Duration(86_400_000_000)
    );
    assert!(run("due * 2d", json!({"due": "2026-09-01"})).is_err());
}

#[test]
fn landing_in_a_field_follows_its_kind() {
    use lakelands_api::ontology::StorageKind as K;
    let n = num("2.345");
    assert_eq!(to_json(&n, K::Decimal, Some(2)), Some(json!("2.35")));
    assert_eq!(to_json(&n, K::Decimal, None), Some(json!("2.345")));
    assert_eq!(
        to_json(&Val::Num(Decimal::from(7)), K::Number, None),
        Some(json!(7))
    );
    assert_eq!(to_json(&text("x"), K::Number, None), None);
    // A whole number past 64 bits lands as its double, not as nothing.
    let wide = Val::Num(Decimal::from(i64::MAX) * Decimal::from(4));
    let double = json!(36_893_488_147_419_103_228.0_f64);
    assert_eq!(to_json(&wide, K::Number, None), Some(double));
    assert_eq!(
        to_json(&text("2026-09-01"), K::DateTime, None),
        Some(json!("2026-09-01T00:00:00.000000Z"))
    );
    assert_eq!(to_json(&Val::Null, K::Text, None), Some(Value::Null));
}

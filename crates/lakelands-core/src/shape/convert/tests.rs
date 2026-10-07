use super::*;
use serde_json::json;

fn kind(k: StorageKind) -> FieldType {
    FieldType::kind(k)
}

fn to(value: Value, target: StorageKind) -> Conversion {
    convert(&value, &kind(StorageKind::Json), &kind(target))
}

fn exact(value: Value, target: StorageKind) -> Value {
    match to(value.clone(), target) {
        Conversion::Exact(v) => v,
        Conversion::Lossy => panic!("{value} should convert to {target:?}"),
    }
}

fn lossy(value: Value, target: StorageKind) {
    assert_eq!(
        to(value.clone(), target),
        Conversion::Lossy,
        "{value} should not convert to {target:?}"
    );
}

fn bytes(v: &Value) -> String {
    String::from_utf8(canonical::to_vec(v)).unwrap()
}

#[test]
fn text_to_number_converts_only_numerically_equal_numerals() {
    assert_eq!(exact(json!("42"), StorageKind::Number), json!(42));
    assert_eq!(exact(json!("4.25"), StorageKind::Number), json!(4.25));
    assert_eq!(bytes(&exact(json!("42.00"), StorageKind::Number)), "42");
    assert_eq!(bytes(&exact(json!("4.50"), StorageKind::Number)), "4.5");
    assert_eq!(bytes(&exact(json!("-1e21"), StorageKind::Number)), "-1e+21");
    for s in [
        "abc",
        "",
        "NaN",
        "inf",
        "1e400",
        " 4",
        "+4",
        "4.",
        "0.10000000000000001",
        "12345678901234567890123",
    ] {
        lossy(json!(s), StorageKind::Number);
    }
}

#[test]
fn text_to_boolean_is_the_literal_true_or_false_only() {
    assert_eq!(exact(json!("true"), StorageKind::Boolean), json!(true));
    assert_eq!(exact(json!("false"), StorageKind::Boolean), json!(false));
    for s in ["TRUE", "1", "yes", ""] {
        lossy(json!(s), StorageKind::Boolean);
    }
    lossy(json!(1), StorageKind::Boolean);
}

#[test]
fn numbers_and_booleans_become_their_canonical_text() {
    assert_eq!(exact(json!(42), StorageKind::Text), json!("42"));
    assert_eq!(exact(json!(4.5), StorageKind::Text), json!("4.5"));
    assert_eq!(exact(json!(1e21), StorageKind::Text), json!("1e+21"));
    assert_eq!(exact(json!(true), StorageKind::Text), json!("true"));
    lossy(json!({"a": 1}), StorageKind::Text);
    lossy(json!([1]), StorageKind::Text);
}

#[test]
fn an_integer_becomes_an_exact_decimal_but_a_fraction_never_does() {
    assert_eq!(exact(json!(42), StorageKind::Decimal), json!("42"));
    assert_eq!(exact(json!(-7), StorageKind::Decimal), json!("-7"));
    assert_eq!(exact(json!(42.0), StorageKind::Decimal), json!("42"));
    assert_eq!(
        exact(json!(u64::MAX), StorageKind::Decimal),
        json!("18446744073709551615")
    );
    lossy(json!(4.2), StorageKind::Decimal);
    lossy(json!(1e300), StorageKind::Decimal);
}

#[test]
fn decimal_text_is_kept_as_written_and_other_text_is_lossy() {
    let dec = kind(StorageKind::Decimal);
    let text = kind(StorageKind::Text);
    assert_eq!(
        convert(&json!("12450.00"), &text, &dec),
        Conversion::Exact(json!("12450.00"))
    );
    for s in ["1.", ".5", "+1", "1e3", "abc", ""] {
        assert_eq!(convert(&json!(s), &text, &dec), Conversion::Lossy, "{s}");
    }
    assert_eq!(
        convert(&json!("12450.00"), &dec, &text),
        Conversion::Exact(json!("12450.00"))
    );
    assert_eq!(
        convert(&json!("0.1"), &dec, &kind(StorageKind::Number)),
        Conversion::Exact(json!(0.1))
    );
}

#[test]
fn text_to_date_time_normalises_utc_and_refuses_offsets() {
    let canonical_form = "2026-01-31T00:00:00.000000Z";
    assert_eq!(
        exact(json!(canonical_form), StorageKind::DateTime),
        json!(canonical_form)
    );
    assert_eq!(
        exact(json!("2026-01-31"), StorageKind::DateTime),
        json!(canonical_form)
    );
    assert_eq!(
        exact(json!("2026-01-31T00:00:00Z"), StorageKind::DateTime),
        json!(canonical_form)
    );
    assert_eq!(
        exact(json!("2026-01-31T00:00:00+00:00"), StorageKind::DateTime),
        json!(canonical_form)
    );
    lossy(json!("2026-01-31T02:00:00+02:00"), StorageKind::DateTime);
    lossy(json!("31.01.2026"), StorageKind::DateTime);
    lossy(json!(1_700_000_000), StorageKind::DateTime);
}

#[test]
fn shape_only_targets_keep_the_value_or_refuse_it() {
    assert_eq!(exact(json!("QUJD"), StorageKind::Bytes), json!("QUJD"));
    assert_eq!(exact(json!("QQ=="), StorageKind::Bytes), json!("QQ=="));
    lossy(json!("abc"), StorageKind::Bytes);
    lossy(json!("QQ==="), StorageKind::Bytes);
    lossy(json!("Q-=="), StorageKind::Bytes);
    assert_eq!(
        exact(json!("inv-1"), StorageKind::Reference),
        json!("inv-1")
    );
    lossy(json!(""), StorageKind::Reference);
    lossy(json!(42), StorageKind::Reference);
    assert_eq!(exact(json!({"a": 1}), StorageKind::Json), json!({"a": 1}));
}

#[test]
fn vectors_and_geometries_are_never_made_only_kept() {
    let three = FieldType {
        dimensions: Some(3),
        ..kind(StorageKind::Vector)
    };
    let json_ty = kind(StorageKind::Json);
    assert_eq!(
        convert(&json!([1, 2, 3]), &json_ty, &three),
        Conversion::Exact(json!([1, 2, 3]))
    );
    assert_eq!(convert(&json!([1, 2]), &json_ty, &three), Conversion::Lossy);
    assert_eq!(convert(&json!(["a"]), &json_ty, &three), Conversion::Lossy);
    // An element no `f32` holds finite is no vector's; zeros are.
    let huge = json!([1e300, 0, 0]);
    assert_eq!(convert(&huge, &json_ty, &three), Conversion::Lossy);
    assert!(matches!(
        convert(&json!([0, 0, 0]), &json_ty, &three),
        Conversion::Exact(_)
    ));
    assert_eq!(
        convert(&json!("1,2,3"), &json_ty, &three),
        Conversion::Lossy
    );

    let point = json!({"type": "Point", "coordinates": [7.1, 50.7]});
    assert_eq!(exact(point.clone(), StorageKind::Geo), point);
    let ring = json!([[0, 0], [1, 0], [1, 1], [0, 0]]);
    let polygon = json!({"type": "Polygon", "coordinates": [ring]});
    assert_eq!(exact(polygon.clone(), StorageKind::Geo), polygon);
    let collection = json!({"type": "GeometryCollection", "geometries": [point, polygon]});
    assert_eq!(exact(collection.clone(), StorageKind::Geo), collection);
    let open = json!({"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [1, 1], [0, 1]]]});
    lossy(open, StorageKind::Geo);
    lossy(
        json!({"type": "Circle", "coordinates": [0, 0]}),
        StorageKind::Geo,
    );
    lossy(json!("POINT(7 50)"), StorageKind::Geo);
}

#[test]
fn a_same_kind_retype_keeps_the_value_or_refuses_it() {
    let three = FieldType {
        dimensions: Some(3),
        ..kind(StorageKind::Vector)
    };
    let two = FieldType {
        dimensions: Some(2),
        ..kind(StorageKind::Vector)
    };
    assert_eq!(convert(&json!([1, 2, 3]), &three, &two), Conversion::Lossy);
    let email = FieldType {
        value_type: Some("email".into()),
        ..kind(StorageKind::Text)
    };
    assert_eq!(
        convert(&json!("a@b.c"), &kind(StorageKind::Text), &email),
        Conversion::Exact(json!("a@b.c"))
    );
    // A number stored as text under a number field is not coerced by a
    // retype that keeps the kind.
    let money = FieldType {
        value_type: Some("money".into()),
        ..kind(StorageKind::Number)
    };
    assert_eq!(
        convert(&json!("42"), &kind(StorageKind::Number), &money),
        Conversion::Lossy
    );
}

#[test]
fn null_stays_null() {
    for k in StorageKind::ALL {
        assert_eq!(
            convert(&Value::Null, &kind(StorageKind::Text), &kind(k)),
            Conversion::Exact(Value::Null)
        );
    }
}

fn samples() -> Vec<Value> {
    vec![
        json!("text"),
        json!("42"),
        json!("4.50"),
        json!("true"),
        json!("2026-01-31"),
        json!("2026-01-31T00:00:00.000000Z"),
        json!("QUJD"),
        json!(42),
        json!(4.5),
        json!(true),
        json!([1, 2, 3]),
        json!({"type": "Point", "coordinates": [0, 0]}),
        json!({"a": 1}),
    ]
}

#[test]
fn every_exact_result_fits_its_target_and_converts_to_itself_again() {
    for from in StorageKind::ALL {
        for target in StorageKind::ALL {
            for v in samples() {
                if let Conversion::Exact(out) = convert(&v, &kind(from), &kind(target)) {
                    assert!(
                        fits(&kind(target), &out),
                        "{v} {from:?}->{target:?} = {out}"
                    );
                    assert_eq!(
                        convert(&out, &kind(target), &kind(target)),
                        Conversion::Exact(out.clone()),
                        "converting {out} again must not change it"
                    );
                }
            }
        }
    }
}

#[test]
fn exact_conversions_round_trip_through_text() {
    let text = kind(StorageKind::Text);
    for (v, k) in [
        (json!(42), StorageKind::Number),
        (json!(4.5), StorageKind::Number),
        (json!(1e21), StorageKind::Number),
        (json!(true), StorageKind::Boolean),
    ] {
        let Conversion::Exact(t) = convert(&v, &kind(k), &text) else {
            panic!("{v} to text");
        };
        assert_eq!(convert(&t, &text, &kind(k)), Conversion::Exact(v));
    }
}

// ---- preflight ----

fn invoice(amount: StorageKind) -> TypeDecl {
    serde_json::from_value(json!({
        "name": "invoice",
        "key": ["number"],
        "fields": [
            { "name": "number", "type": { "kind": "text" }, "required": true },
            { "name": "amount", "type": { "kind": amount } },
            { "name": "note", "type": { "kind": "text" } },
        ],
    }))
    .unwrap()
}

fn record(n: &str, amount: Value) -> (RecordKey, BTreeMap<FieldName, Value>) {
    let mut fields = BTreeMap::new();
    fields.insert("number".into(), json!(n));
    if !amount.is_null() {
        fields.insert("amount".into(), amount);
    }
    fields.insert("note".into(), json!("n"));
    (n.into(), fields)
}

fn retype() -> Migration {
    Migration {
        to: invoice(StorageKind::Decimal),
        renames: vec![],
        relation_renames: Vec::new(),
    }
}

#[test]
fn a_preflight_counts_what_converts_and_samples_what_does_not() {
    let mut records = vec![record("a", json!(1)), record("b", Value::Null)];
    for i in 0..7 {
        records.push(record(&format!("x{i}"), json!(0.5)));
    }
    let before = invoice(StorageKind::Number);
    let report = preflight(&before, &retype(), records).unwrap();
    assert_eq!(report.ty.as_str(), "invoice");
    assert_eq!(report.records_scanned, 9);
    assert_eq!(report.from_schema_hash, schema_hash(&before).unwrap());
    assert_eq!(report.to_schema_hash, schema_hash(&retype().to).unwrap());
    assert!(!report.lossless);
    let [amount] = &report.fields[..] else {
        panic!("{:?}", report.fields);
    };
    assert_eq!(amount.field.as_str(), "amount");
    assert_eq!(amount.from, kind(StorageKind::Number));
    assert_eq!(amount.to, kind(StorageKind::Decimal));
    assert_eq!(amount.convertible, 1);
    assert_eq!(amount.not_convertible, 7);
    let keys: Vec<&str> = amount.sample_keys.iter().map(RecordKey::as_str).collect();
    assert_eq!(keys, ["x0", "x1", "x2", "x3", "x4"]);
}

#[test]
fn a_report_does_not_depend_on_scan_order() {
    let before = invoice(StorageKind::Number);
    let records: Vec<_> = (0..9)
        .map(|i| record(&format!("k{i}"), json!(i as f64 + 0.5)))
        .collect();
    let forward = preflight(&before, &retype(), records.clone()).unwrap();
    let backward = preflight(&before, &retype(), records.into_iter().rev()).unwrap();
    assert_eq!(forward, backward);
}

#[test]
fn the_report_id_covers_the_whole_report() {
    let before = invoice(StorageKind::Number);
    let one = preflight(&before, &retype(), vec![record("a", json!(1))]).unwrap();
    let mut blank = one.clone();
    blank.report_id = String::new();
    assert_eq!(
        one.report_id,
        canonical::sha256_hex(&serde_json::to_value(&blank).unwrap())
    );
    assert_eq!(one.report_id.len(), 64);
    let two = preflight(
        &before,
        &retype(),
        vec![record("a", json!(1)), record("b", Value::Null)],
    )
    .unwrap();
    assert!(one.lossless && two.lossless);
    assert_ne!(one.report_id, two.report_id, "records_scanned differs");
}

#[test]
fn a_lossless_preflight_and_its_migration_agree() {
    let before = invoice(StorageKind::Number);
    let records = vec![record("a", json!(12)), record("b", Value::Null)];
    let report = preflight(&before, &retype(), records.clone()).unwrap();
    assert!(report.lossless);
    let (_, fields) = &records[0];
    let out = migrate_record(&before, &retype(), fields).unwrap().unwrap();
    assert_eq!(out["amount"], json!("12"));
    assert_eq!(out["note"], json!("n"));
    let lossy_record = record("c", json!(0.5)).1;
    assert_eq!(
        migrate_record(&before, &retype(), &lossy_record).unwrap(),
        None
    );
}

#[test]
fn renames_move_values_to_the_new_name() {
    let before = invoice(StorageKind::Text);
    let mut to = invoice(StorageKind::Number);
    to.fields[1].name = "total".into();
    let migration = Migration {
        to,
        renames: vec![("amount".into(), "total".into())],
        relation_renames: Vec::new(),
    };
    let records = vec![record("a", json!("42")), record("b", json!("x"))];
    let report = preflight(&before, &migration, records.clone()).unwrap();
    let [total] = &report.fields[..] else {
        panic!("{:?}", report.fields);
    };
    assert_eq!(total.field.as_str(), "total");
    assert_eq!((total.convertible, total.not_convertible), (1, 1));
    assert_eq!(total.sample_keys, vec![RecordKey::from("b")]);

    let out = migrate_record(&before, &migration, &records[0].1)
        .unwrap()
        .unwrap();
    assert_eq!(out.get("total"), Some(&json!(42)));
    assert!(!out.contains_key("amount"));
}

#[test]
fn a_rename_without_a_retype_is_not_reported_but_still_moves() {
    let before = invoice(StorageKind::Text);
    let mut to = invoice(StorageKind::Text);
    to.fields[2].name = "remark".into();
    let migration = Migration {
        to,
        renames: vec![("note".into(), "remark".into())],
        relation_renames: Vec::new(),
    };
    let report = preflight(&before, &migration, vec![record("a", json!("1"))]).unwrap();
    assert!(report.fields.is_empty());
    assert!(report.lossless);
    let out = migrate_record(&before, &migration, &record("a", json!("1")).1)
        .unwrap()
        .unwrap();
    assert_eq!(out.get("remark"), Some(&json!("n")));
}

#[test]
fn two_fields_may_swap_names() {
    let before = invoice(StorageKind::Text);
    let migration = Migration {
        to: invoice(StorageKind::Text),
        renames: vec![
            ("amount".into(), "note".into()),
            ("note".into(), "amount".into()),
        ],
        relation_renames: Vec::new(),
    };
    let out = migrate_record(&before, &migration, &record("a", json!("1")).1)
        .unwrap()
        .unwrap();
    assert_eq!(out.get("amount"), Some(&json!("n")));
    assert_eq!(out.get("note"), Some(&json!("1")));
}

#[test]
fn a_field_that_becomes_required_counts_records_without_it() {
    let before = invoice(StorageKind::Number);
    let mut to = invoice(StorageKind::Number);
    to.fields[1].required = true;
    let migration = Migration {
        to,
        renames: vec![],
        relation_renames: Vec::new(),
    };
    let records = vec![record("a", json!(1)), record("b", Value::Null)];
    let report = preflight(&before, &migration, records).unwrap();
    let [amount] = &report.fields[..] else {
        panic!("{:?}", report.fields);
    };
    assert_eq!(amount.from, amount.to);
    assert_eq!((amount.convertible, amount.not_convertible), (1, 1));
    assert!(!report.lossless);
}

#[test]
fn a_removed_field_is_dropped_from_the_record() {
    let before = invoice(StorageKind::Number);
    let mut to = invoice(StorageKind::Number);
    to.fields.retain(|f| f.name.as_str() != "note");
    let migration = Migration {
        to,
        renames: vec![],
        relation_renames: Vec::new(),
    };
    let out = migrate_record(&before, &migration, &record("a", json!(1)).1)
        .unwrap()
        .unwrap();
    assert!(!out.contains_key("note"));
}

#[test]
fn a_malformed_migration_is_refused() {
    let before = invoice(StorageKind::Text);
    let renamed = |from: &str, to: &str, name: &str| {
        let mut decl = invoice(StorageKind::Text);
        decl.fields[1].name = name.into();
        Migration {
            to: decl,
            renames: vec![(from.into(), to.into())],
            relation_renames: Vec::new(),
        }
    };
    let err = |m: Migration| preflight(&before, &m, vec![]).unwrap_err();
    assert!(matches!(
        err(renamed("ghost", "total", "total")),
        ShapeError::RenameFromUnknown { .. }
    ));
    assert!(matches!(
        err(renamed("amount", "ghost", "total")),
        ShapeError::RenameToUnknown { .. }
    ));
    assert!(matches!(
        err(renamed("amount", "note", "total")),
        ShapeError::RenameCollides { .. }
    ));
    let mut twice = renamed("amount", "total", "total");
    twice.renames.push(("amount".into(), "total".into()));
    assert!(matches!(err(twice), ShapeError::RenameRepeated { .. }));
    let mut other = retype();
    other.to.name = "receipt".into();
    assert!(matches!(err(other), ShapeError::OtherType { .. }));
}

#[test]
fn numerals_reduce_to_their_value() {
    assert_eq!(normal("4.50"), normal("4.5"));
    assert_eq!(normal("450e-2"), normal("4.5"));
    assert_eq!(normal("0.000"), Some((false, String::new(), 0)));
    assert_eq!(normal("-0"), normal("0"));
    assert_ne!(normal("45"), normal("4.5"));
    assert_eq!(normal("1e+21"), normal("1000000000000000000000"));
    for bad in ["", "-", "1.", ".1", "1e", "e1", "1e+", "0x10", "1_0"] {
        assert_eq!(normal(bad), None, "{bad}");
    }
}

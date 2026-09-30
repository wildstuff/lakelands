use super::*;
use lakelands_api::ontology::{Authority, OnInputChange};
use serde_json::json;

fn kind(k: StorageKind) -> FieldType {
    FieldType::kind(k)
}

fn value_type(name: &str, k: StorageKind) -> ValueTypeDecl {
    ValueTypeDecl {
        name: name.into(),
        kind: k,
        constraint: None,
        one_of: Vec::new(),
        format: None,
        scale: None,
        description: None,
    }
}

fn typed(k: StorageKind, vt: &str) -> FieldType {
    FieldType {
        value_type: Some(vt.into()),
        ..FieldType::kind(k)
    }
}

fn field(name: &str, ty: FieldType) -> FieldDecl {
    FieldDecl {
        annotations: Default::default(),
        name: name.into(),
        ty,
        required: false,
        authority: Authority::Authored,
        derived: None,
        function: None,
        depends_on: Vec::new(),
        on_input_change: OnInputChange::Keep,
        sensitive: false,
        searchable: false,
        description: None,
        label: None,
        embed: None,
    }
}

fn iban() -> ValueTypeDecl {
    ValueTypeDecl {
        format: Some("iban".into()),
        scale: None,
        ..value_type("iban", StorageKind::Text)
    }
}

#[test]
fn null_passes_every_kind() {
    for k in StorageKind::ALL {
        assert_eq!(check(&Value::Null, &kind(k), None), Ok(Value::Null));
    }
    let vt = iban();
    assert_eq!(
        check(&Value::Null, &typed(StorageKind::Text, "iban"), Some(&vt)),
        Ok(Value::Null)
    );
}

#[test]
fn each_kind_takes_its_json_form() {
    let ok = [
        (StorageKind::Text, json!("hello")),
        (StorageKind::Number, json!(1.5)),
        (StorageKind::Number, json!(-3)),
        (StorageKind::Decimal, json!("12450.00")),
        (StorageKind::Boolean, json!(true)),
        (StorageKind::Bytes, json!("aGVsbG8=")),
        (StorageKind::Json, json!({"any": [1, "thing"]})),
        (StorageKind::Reference, json!("inv-1")),
        (
            StorageKind::Geo,
            json!({"type": "Point", "coordinates": [8.5, 47.4]}),
        ),
    ];
    for (k, v) in ok {
        assert_eq!(check(&v, &kind(k), None), Ok(v.clone()), "{k:?} {v}");
    }
}

#[test]
fn each_kind_refuses_another_kinds_form() {
    let bad = [
        (StorageKind::Text, json!(1)),
        (StorageKind::Number, json!("1")),
        (StorageKind::Decimal, json!(12.5)),
        (StorageKind::Boolean, json!("true")),
        (StorageKind::DateTime, json!(0)),
        (StorageKind::Bytes, json!([1, 2])),
        (StorageKind::Reference, json!({"key": "a"})),
        (StorageKind::Vector, json!("1,2")),
        (StorageKind::Geo, json!("POINT(1 2)")),
    ];
    for (k, v) in bad {
        assert!(check(&v, &kind(k), None).is_err(), "{k:?} took {v}");
    }
}

#[test]
fn a_date_time_is_normalised_to_its_one_form() {
    let ty = kind(StorageKind::DateTime);
    for (given, want) in [
        ("2026-09-24T07:57:11+02:00", "2026-09-24T05:57:11.000000Z"),
        ("2026-09-24", "2026-09-24T00:00:00.000000Z"),
        ("2026-09-24T05:57:11.5Z", "2026-09-24T05:57:11.500000Z"),
        ("2026-09-24T05:57:11.123456Z", "2026-09-24T05:57:11.123456Z"),
    ] {
        assert_eq!(check(&json!(given), &ty, None), Ok(json!(want)));
    }
    for bad in ["2026-02-29", "2026-09-24T05:57:11", "yesterday"] {
        assert_eq!(
            check(&json!(bad), &ty, None),
            Err(ValueError::DateTime(bad.into()))
        );
    }
}

#[test]
fn a_decimal_is_kept_exactly_as_written() {
    let ty = kind(StorageKind::Decimal);
    for ok in ["0", "-1", "250.00", "007", "-0.10"] {
        assert_eq!(check(&json!(ok), &ty, None), Ok(json!(ok)));
    }
    for bad in ["", "-", "1.", ".5", "1e3", "+1", "1,5", "1.2.3", " 1"] {
        assert_eq!(
            check(&json!(bad), &ty, None),
            Err(ValueError::Decimal(bad.into())),
            "{bad:?}"
        );
    }
}

#[test]
fn bytes_are_padded_standard_base64_in_one_spelling() {
    let ty = kind(StorageKind::Bytes);
    for ok in ["", "aGVsbG8=", "aGk=", "YWJj", "+/+/"] {
        assert!(check(&json!(ok), &ty, None).is_ok(), "{ok:?}");
    }
    // Unpadded, url-safe, bad padding, and non-zero bits before padding.
    for bad in ["aGVsbG8", "-_-_", "a===", "aGk", "aGl=", "aGVsbG9=", "a=bc"] {
        assert_eq!(
            check(&json!(bad), &ty, None),
            Err(ValueError::Base64),
            "{bad:?}"
        );
    }
}

#[test]
fn a_vector_has_its_declared_length_of_numbers() {
    let ty = FieldType {
        dimensions: Some(3),
        ..FieldType::kind(StorageKind::Vector)
    };
    assert!(check(&json!([0.1, 0.2, 0.3]), &ty, None).is_ok());
    assert_eq!(
        check(&json!([0.1, 0.2]), &ty, None),
        Err(ValueError::VectorLength {
            expected: 3,
            found: 2
        })
    );
    assert!(check(&json!([0.1, "x", 0.3]), &ty, None).is_err());
    assert_eq!(
        check(&json!([1]), &kind(StorageKind::Vector), None),
        Err(ValueError::NoDimensions)
    );
}

#[test]
fn geo_takes_geometries_and_refuses_the_rest() {
    let ty = kind(StorageKind::Geo);
    let polygon = json!({"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [1, 1], [0, 0]]]});
    let collection = json!({"type": "GeometryCollection", "geometries": [polygon.clone()]});
    assert!(check(&polygon, &ty, None).is_ok());
    assert!(check(&collection, &ty, None).is_ok());
    for empty in [
        "Point",
        "LineString",
        "Polygon",
        "MultiPoint",
        "MultiPolygon",
    ] {
        let e = json!({"type": empty, "coordinates": []});
        assert!(check(&e, &ty, None).is_ok(), "{e}");
    }
    let none = json!({"type": "GeometryCollection", "geometries": []});
    assert!(check(&none, &ty, None).is_ok());
    for bad in [
        json!({"coordinates": [0, 0]}),
        json!({"type": "Point"}),
        json!({"type": "Feature", "geometry": polygon}),
        json!({"type": "GeometryCollection", "geometries": [{"type": "Circle"}]}),
        json!({"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [1, 1], [0, 1]]]}),
        json!({"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [0, 0]]]}),
        json!({"type": "Polygon", "coordinates": [[]]}),
        json!({"type": "MultiPolygon", "coordinates": [[]]}),
        json!({"type": "MultiPoint", "coordinates": [[]]}),
        json!({"type": "Point", "coordinates": [7]}),
        json!({"type": "Point", "coordinates": [7, "50"]}),
        json!({"type": "LineString", "coordinates": [[0, 0]]}),
        json!({"type": "MultiPoint", "coordinates": [0, 0]}),
    ] {
        assert!(check(&bad, &ty, None).is_err(), "{bad}");
    }
}

#[test]
fn one_of_admits_only_its_members() {
    let vt = ValueTypeDecl {
        one_of: vec!["open".into(), "paid".into()],
        ..value_type("status", StorageKind::Text)
    };
    let ty = typed(StorageKind::Text, "status");
    assert_eq!(check(&json!("paid"), &ty, Some(&vt)), Ok(json!("paid")));
    assert_eq!(
        check(&json!("void"), &ty, Some(&vt)),
        Err(ValueError::NotOneOf {
            value: "\"void\"".into(),
            allowed: vec!["open".into(), "paid".into()],
        })
    );
}

#[test]
fn a_known_format_is_enforced() {
    let vt = iban();
    let ty = typed(StorageKind::Text, "iban");
    assert!(check(&json!("DE89370400440532013000"), &ty, Some(&vt)).is_ok());
    assert_eq!(
        check(&json!("bogus"), &ty, Some(&vt)),
        Err(ValueError::Format {
            value_type: "iban".into(),
            format: ValueFormat::Iban,
        })
    );
}

#[test]
fn a_format_on_a_non_text_value_type_refuses_what_is_not_a_string() {
    let vt = ValueTypeDecl {
        format: Some("email".into()),
        scale: None,
        ..value_type("odd", StorageKind::Number)
    };
    let ty = typed(StorageKind::Number, "odd");
    assert!(matches!(
        check(&json!(42), &ty, Some(&vt)),
        Err(ValueError::Format { .. })
    ));
}

#[test]
fn numbers_take_their_one_json_form() {
    assert_eq!(
        check(&json!(2.0), &kind(StorageKind::Number), None).unwrap(),
        json!(2)
    );
    let mut v = kind(StorageKind::Vector);
    v.dimensions = Some(2);
    assert_eq!(
        check(&json!([0.0, 1.5]), &v, None).unwrap(),
        json!([0, 1.5])
    );
    assert_eq!(
        check(&json!({"a": 1e0}), &kind(StorageKind::Json), None).unwrap(),
        json!({"a": 1})
    );
}

#[test]
fn integers_beyond_two_to_the_fifty_three_are_refused() {
    let n = kind(StorageKind::Number);
    assert!(check(&json!(9_007_199_254_740_991_i64), &n, None).is_ok());
    assert!(matches!(
        check(&json!(9_007_199_254_740_992_i64), &n, None),
        Err(ValueError::TooLarge(_))
    ));
    assert!(matches!(
        check(&json!(-9_007_199_254_740_992_i64), &n, None),
        Err(ValueError::TooLarge(_))
    ));
    let j = kind(StorageKind::Json);
    assert!(matches!(
        check(&json!({"a": [1, 18_446_744_073_709_551_615_u64]}), &j, None),
        Err(ValueError::TooLarge(_))
    ));
    let g = kind(StorageKind::Geo);
    assert!(check(&json!({"type": "Point", "coordinates": [1, 2]}), &g, None).is_ok());
    assert!(matches!(
        check(
            &json!({"type": "Point", "coordinates": [9_007_199_254_740_992_i64, 2]}),
            &g,
            None
        ),
        Err(ValueError::TooLarge(_))
    ));
}

#[test]
fn money_is_kept_at_its_scale_rounded_half_away_from_zero() {
    let vt = ValueTypeDecl {
        scale: Some(2),
        ..value_type("money", StorageKind::Decimal)
    };
    let ty = typed(StorageKind::Decimal, "money");
    for (written, kept) in [
        ("2.345", "2.35"),
        ("-2.345", "-2.35"),
        ("10", "10.00"),
        ("0.1", "0.10"),
        ("7.004", "7.00"),
    ] {
        assert_eq!(
            check(&json!(written), &ty, Some(&vt)),
            Ok(json!(kept)),
            "{written}"
        );
    }
}

/// An unknown format name is refused when the value type is declared,
/// and again here, should one ever reach a write.
#[test]
fn an_unknown_format_name_is_refused_never_ignored() {
    let vt = ValueTypeDecl {
        format: Some("#,##0.00".into()),
        ..value_type("money", StorageKind::Decimal)
    };
    let ty = typed(StorageKind::Decimal, "money");
    assert!(matches!(
        check(&json!("12.50"), &ty, Some(&vt)),
        Err(ValueError::UnknownFormat { .. })
    ));
}

#[test]
fn a_named_value_type_must_be_the_one_given() {
    let ty = typed(StorageKind::Text, "iban");
    let unknown = Err(ValueError::UnknownValueType("iban".into()));
    assert_eq!(check(&json!("x"), &ty, None), unknown);
    let other = value_type("email", StorageKind::Text);
    assert_eq!(check(&json!("x"), &ty, Some(&other)), unknown);
}

#[test]
fn a_value_type_is_ignored_when_the_field_names_none() {
    let vt = iban();
    assert_eq!(
        check(&json!("bogus"), &kind(StorageKind::Text), Some(&vt)),
        Ok(json!("bogus"))
    );
}

#[test]
fn a_field_of_its_value_types_kind_conforms() {
    let f = field("creditor_iban", typed(StorageKind::Text, "iban"));
    assert_eq!(conformance(&f, &[iban()]), Ok(()));
}

#[test]
fn a_field_of_another_kind_does_not_conform() {
    let f = field("creditor_iban", typed(StorageKind::Number, "iban"));
    assert_eq!(
        conformance(&f, &[iban()]),
        Err(ValueError::KindMismatch {
            field: "creditor_iban".into(),
            value_type: "iban".into(),
            expected: StorageKind::Text,
            found: StorageKind::Number,
        })
    );
}

#[test]
fn a_field_naming_an_undeclared_value_type_does_not_conform() {
    let f = field("creditor_iban", typed(StorageKind::Text, "iban"));
    assert_eq!(
        conformance(&f, &[]),
        Err(ValueError::UnknownValueType("iban".into()))
    );
}

#[test]
fn required_is_not_part_of_the_contract() {
    let mut f = field("creditor_iban", typed(StorageKind::Text, "iban"));
    for required in [true, false] {
        f.required = required;
        assert_eq!(conformance(&f, &[iban()]), Ok(()));
    }
}

#[test]
fn a_field_without_a_value_type_always_conforms() {
    let f = field("note", kind(StorageKind::Text));
    assert_eq!(conformance(&f, &[]), Ok(()));
}

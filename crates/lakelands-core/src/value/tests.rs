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
        ..Default::default()
    }
}

/// `value_type(name, k)` carrying the format `format`.
fn formatted(name: &str, k: StorageKind, format: &str) -> ValueTypeDecl {
    ValueTypeDecl {
        format: Some(format.into()),
        ..value_type(name, k)
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
        name: name.into(),
        ty,
        authority: Authority::Authored,
        on_input_change: OnInputChange::Keep,
        ..Default::default()
    }
}

/// Whether `check` refuses `v` as too large to hold exactly.
fn too_large(v: &serde_json::Value, ty: &FieldType, value_type: Option<&ValueTypeDecl>) -> bool {
    matches!(check(v, ty, value_type), Err(Case::IntegerTooLarge { .. }))
}

fn iban() -> ValueTypeDecl {
    formatted("iban", StorageKind::Text, "iban")
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
fn a_decimal_is_kept_exactly_as_written() {
    let ty = kind(StorageKind::Decimal);
    for ok in ["0", "-1", "250.00", "007", "-0.10"] {
        assert_eq!(check(&json!(ok), &ty, None), Ok(json!(ok)));
    }
    for bad in ["", "-", "1.", ".5", "1e3", "+1", "1,5", "1.2.3", " 1"] {
        assert_eq!(
            check(&json!(bad), &ty, None),
            Err(Case::NotDecimal { value: bad.into() }),
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
        Err(Case::VectorLength {
            expected: 3,
            found: 2
        })
    );
    assert!(check(&json!([0.1, "x", 0.3]), &ty, None).is_err());
    assert_eq!(
        check(&json!([1]), &kind(StorageKind::Vector), None),
        Err(Case::VectorNoDimensions)
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
fn a_known_format_is_enforced() {
    let vt = iban();
    let ty = typed(StorageKind::Text, "iban");
    assert!(check(&json!("DE89370400440532013000"), &ty, Some(&vt)).is_ok());
    assert_eq!(
        check(&json!("bogus"), &ty, Some(&vt)),
        Err(Case::NotFormat {
            format: ValueFormat::Iban.name().into(),
            value_type: "iban".into(),
        })
    );
}

#[test]
fn a_format_on_a_non_text_value_type_refuses_what_is_not_a_string() {
    let vt = formatted("odd", StorageKind::Number, "email");
    let ty = typed(StorageKind::Number, "odd");
    assert!(matches!(
        check(&json!(42), &ty, Some(&vt)),
        Err(Case::NotFormat { .. })
    ));
}

#[test]
fn integers_beyond_two_to_the_fifty_three_are_refused() {
    let n = kind(StorageKind::Number);
    assert!(check(&json!(9_007_199_254_740_991_i64), &n, None).is_ok());
    assert!(too_large(&json!(9_007_199_254_740_992_i64), &n, None));
    assert!(too_large(&json!(-9_007_199_254_740_992_i64), &n, None));
    let j = kind(StorageKind::Json);
    assert!(too_large(
        &json!({"a": [1, 18_446_744_073_709_551_615_u64]}),
        &j,
        None
    ));
    let g = kind(StorageKind::Geo);
    assert!(check(&json!({"type": "Point", "coordinates": [1, 2]}), &g, None).is_ok());
    assert!(too_large(
        &json!({"type": "Point", "coordinates": [9_007_199_254_740_992_i64, 2]}),
        &g,
        None
    ));
}

/// A field's type with the value type it names, as `check` takes them.
type Typed<'a> = (FieldType, Option<&'a ValueTypeDecl>);

/// A value `check` answers is one it answers again, the same: one pass
/// is enough at every door, and a value read from the log checks as it
/// was written.
#[test]
fn a_checked_value_checks_to_itself() {
    let money = ValueTypeDecl {
        scale: Some(2),
        ..value_type("money", StorageKind::Decimal)
    };
    let three = FieldType {
        dimensions: Some(3),
        ..kind(StorageKind::Vector)
    };
    let of = |k| (kind(k), None);
    let cases: Vec<(Typed, Vec<Value>)> = vec![
        (of(StorageKind::Text), vec![json!(""), json!("1e16")]),
        (of(StorageKind::Reference), vec![json!("k")]),
        (of(StorageKind::Boolean), vec![json!(true)]),
        (
            of(StorageKind::Number),
            vec![
                json!(1),
                json!(2.0),
                json!(-0.0),
                json!(0.1),
                json!(1.5e-7),
                json!(9007199254740991.0),
                json!(-9_007_199_254_740_991_i64),
                json!(1e21),
                json!(1e300),
            ],
        ),
        (
            of(StorageKind::Decimal),
            vec![json!("1.50"), json!("-0"), json!("007")],
        ),
        (
            (typed(StorageKind::Decimal, "money"), Some(&money)),
            vec![json!("1.005"), json!("2"), json!("-0.004"), json!("007")],
        ),
        (
            of(StorageKind::DateTime),
            vec![
                json!("2026-09-24T07:57:11+02:00"),
                json!("2026-09-24"),
                json!("2026-09-24t05:57:11.5z"),
                json!("9999-12-31T23:30:00+01:00"),
                json!("0000-01-01T00:30:00-01:00"),
            ],
        ),
        (
            of(StorageKind::Date),
            vec![
                json!("2026-09-24"),
                json!("2026-09-24T02:00:00+02:00"),
                json!("9999-12-31T01:00:00+01:00"),
            ],
        ),
        (of(StorageKind::Bytes), vec![json!("aGk="), json!("")]),
        (
            of(StorageKind::Json),
            vec![json!({"b": [2.0, 1e21, -0.0], "a": null}), json!(1.0)],
        ),
        ((three, None), vec![json!([1.0, 2, 0.5])]),
        (
            of(StorageKind::Geo),
            vec![json!({"type": "Point", "coordinates": [1.0, 2]})],
        ),
    ];
    for k in StorageKind::ALL {
        assert!(cases.iter().any(|((ty, _), _)| ty.kind == k), "no {k}");
    }
    for ((ty, vt), values) in &cases {
        for v in values {
            let once = check(v, ty, *vt).unwrap_or_else(|e| panic!("{v} as {}: {e:?}", ty.kind));
            assert_eq!(
                check(&once, ty, *vt),
                Ok(once.clone()),
                "{v} as {}",
                ty.kind
            );
        }
    }
}

/// What a second check would refuse or change is refused by the first: a
/// number the log writes as an integer too large, and an instant an
/// offset carries out of the years of the one form.
#[test]
fn a_value_its_written_form_would_not_pass_is_refused() {
    let n = kind(StorageKind::Number);
    for v in [1e16, -1e16, 9007199254740993.0, 1e20] {
        assert!(too_large(&json!(v), &n, None), "{v}");
    }
    assert!(too_large(
        &json!({"a": [1e17]}),
        &kind(StorageKind::Json),
        None
    ));
    let three = FieldType {
        dimensions: Some(3),
        ..kind(StorageKind::Vector)
    };
    assert!(too_large(&json!([1e16, 0, 0]), &three, None));
    let point = json!({"type": "Point", "coordinates": [1e16, 2]});
    assert!(too_large(&point, &kind(StorageKind::Geo), None));
    for v in ["9999-12-31T23:30:00-01:00", "0000-01-01T00:30:00+01:00"] {
        let got = check(&json!(v), &kind(StorageKind::DateTime), None);
        assert!(matches!(got, Err(Case::NotDateTime { .. })), "{v}: {got:?}");
    }
    let got = check(
        &json!("9999-12-31T23:00:00-01:00"),
        &kind(StorageKind::Date),
        None,
    );
    assert!(matches!(got, Err(Case::NotDate { .. })), "{got:?}");
}

/// An unknown format name is refused when the value type is declared,
/// and again here, should one ever reach a write.
#[test]
fn an_unknown_format_name_is_refused_never_ignored() {
    let vt = formatted("money", StorageKind::Decimal, "#,##0.00");
    let ty = typed(StorageKind::Decimal, "money");
    assert!(matches!(
        check(&json!("12.50"), &ty, Some(&vt)),
        Err(Case::ValueTypeFormat { .. })
    ));
}

#[test]
fn a_named_value_type_must_be_the_one_given() {
    let ty = typed(StorageKind::Text, "iban");
    let unknown = Err(Case::NoValueType {
        value_type: "iban".into(),
    });
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
        Err(Case::ValueTypeKind {
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
        Err(Case::NoValueType {
            value_type: "iban".into()
        })
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

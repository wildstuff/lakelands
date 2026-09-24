//! Checking one value against its field's type, and one field against
//! the value type it names.
//!
//! [`check`] takes a value as a writer sent it and returns it in its one
//! canonical JSON form (`docs/log-format.md` §5), or says why it does not
//! fit. It covers three things: the storage kind (the JSON shape each
//! kind has, a date-time normalised to its one form, a decimal kept as
//! written, a vector of the declared length), the value type's `one_of`,
//! and the value type's `format` when that names one of the known
//! [`ValueFormat`]s. A value type's `constraint` is a condition in the
//! one expression grammar and is not evaluated here: the store applies it
//! with the evaluator, after `check` has settled the value's form.
//!
//! `null` always passes `check`. Clearing a field is the fold's business,
//! and whether a record may lack a field is its type's (`required`).
//!
//! [`conformance`] holds a field to the value type it names. A value type
//! is a contract: every field that uses it is stored as the value type's
//! kind, so the concept is the same wherever it appears. Whether a field
//! is `required` stays a choice of each type: an `iban` may be mandatory
//! on one type and optional on another while staying the same value type.

use std::fmt;

use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, ValueTypeDecl};
use lakelands_api::{FieldName, Timestamp, Value, ValueTypeName};

mod format;

pub use format::ValueFormat;

/// Why a value does not fit its field, or a field its value type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueError {
    /// The value's JSON shape is not the one its storage kind has.
    WrongShape {
        /// The kind the field is stored as.
        kind: StorageKind,
        /// What the kind's JSON form is, for a person.
        expected: &'static str,
    },
    /// A decimal string is not `-?[0-9]+(\.[0-9]+)?`.
    Decimal(String),
    /// An integer outside ±(2^53 − 1), which not every reader of the log
    /// reads exactly (`docs/log-format.md` §1.2).
    TooLarge(String),
    /// A date-time string is not a UTC instant.
    DateTime(String),
    /// A bytes string is not standard base64 with padding.
    Base64,
    /// A vector field declares no length.
    NoDimensions,
    /// A vector has the wrong length.
    VectorLength {
        /// The declared length.
        expected: u32,
        /// The length written.
        found: usize,
    },
    /// A geo value is not a GeoJSON geometry object.
    Geo(String),
    /// The value is not one of the value type's allowed values.
    NotOneOf {
        /// The value written, as JSON text.
        value: String,
        /// The allowed values.
        allowed: Vec<String>,
    },
    /// The value fails the value type's format.
    Format {
        /// The value type.
        value_type: ValueTypeName,
        /// The format it fails.
        format: ValueFormat,
    },
    /// The field names a value type that is not declared, or a different
    /// one from the declaration given.
    UnknownValueType(ValueTypeName),
    /// The value type names a format that does not exist.
    UnknownFormat {
        /// The value type.
        value_type: ValueTypeName,
        /// The name it gives.
        format: String,
    },
    /// The field is stored as a different kind from its value type.
    KindMismatch {
        /// The field.
        field: FieldName,
        /// Its value type.
        value_type: ValueTypeName,
        /// The value type's kind.
        expected: StorageKind,
        /// The field's kind.
        found: StorageKind,
    },
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValueError::WrongShape { kind, expected } => {
                write!(f, "a {} value must be {expected}", kind_name(*kind))
            }
            ValueError::Decimal(s) => write!(f, "not a decimal: {s:?}"),
            ValueError::TooLarge(n) => write!(
                f,
                "{n} is beyond ±(2^53 − 1), the largest integer every reader reads exactly"
            ),
            ValueError::DateTime(s) => write!(f, "not a UTC date-time: {s:?}"),
            ValueError::Base64 => f.write_str("not standard base64 with padding"),
            ValueError::NoDimensions => f.write_str("the vector field declares no dimensions"),
            ValueError::VectorLength { expected, found } => {
                write!(f, "the vector has {found} numbers, not {expected}")
            }
            ValueError::Geo(why) => write!(f, "not a GeoJSON geometry: {why}"),
            ValueError::NotOneOf { value, allowed } => {
                write!(f, "{value} is not one of {}", allowed.join(", "))
            }
            ValueError::Format { value_type, format } => {
                write!(
                    f,
                    "the value is not a well-formed {} ({value_type})",
                    format.name()
                )
            }
            ValueError::UnknownValueType(name) => write!(f, "no value type named {name}"),
            ValueError::UnknownFormat { value_type, format } => {
                write!(
                    f,
                    "value type {value_type} names the unknown format `{format}`"
                )
            }
            ValueError::KindMismatch {
                field,
                value_type,
                expected,
                found,
            } => write!(
                f,
                "field {field} uses value type {value_type}, which is {}, but the field is {}",
                kind_name(*expected),
                kind_name(*found)
            ),
        }
    }
}

impl std::error::Error for ValueError {}

/// A storage kind's name, as declarations spell it.
pub fn kind_name(kind: StorageKind) -> &'static str {
    match kind {
        StorageKind::Text => "text",
        StorageKind::Number => "number",
        StorageKind::Decimal => "decimal",
        StorageKind::Boolean => "boolean",
        StorageKind::DateTime => "date_time",
        StorageKind::Bytes => "bytes",
        StorageKind::Json => "json",
        StorageKind::Reference => "reference",
        StorageKind::Vector => "vector",
        StorageKind::Geo => "geo",
    }
}

/// Checks one value against its field's type and returns it in its
/// canonical form.
///
/// `value_type` is the declaration `ty.value_type` names; it must be given
/// exactly when the field names one. `check` covers the storage kind, the
/// value type's `one_of` and its known `format`; the value type's
/// `constraint` is applied by the store with the evaluator. `null` is
/// always accepted and returned as is.
pub fn check(
    value: &Value,
    ty: &FieldType,
    value_type: Option<&ValueTypeDecl>,
) -> Result<Value, ValueError> {
    let value_type = resolve(ty, value_type)?;
    if value.is_null() {
        return Ok(Value::Null);
    }
    let mut canonical = check_kind(value, ty)?;
    if let Some(vt) = value_type {
        check_one_of(&canonical, vt)?;
        check_format(&canonical, vt)?;
        if let (Some(scale), Value::String(digits)) = (vt.scale, &canonical) {
            canonical = Value::String(to_scale(digits, scale));
        }
    }
    // Every number in a value takes its one JSON form (`docs/log-format.md`
    // §1.2): `2.0` is `2`, as the log writes it, so a value reads the same
    // live and after a rebuild from the log.
    if matches!(
        ty.kind,
        StorageKind::Number | StorageKind::Json | StorageKind::Vector | StorageKind::Geo
    ) {
        canonical = serde_json::from_slice(&crate::canonical::to_vec(&canonical))
            .expect("canonical JSON reads back");
    }
    Ok(canonical)
}

/// A checked decimal kept at `scale` fractional digits: rounded half away
/// from zero when it carries more, padded with zeros when it carries
/// fewer, so every value of a money type reads with the same digits.
fn to_scale(digits: &str, scale: u32) -> String {
    use rust_decimal::{Decimal, RoundingStrategy};
    match digits.parse::<Decimal>() {
        Ok(d) => {
            let mut r = d.round_dp_with_strategy(scale, RoundingStrategy::MidpointAwayFromZero);
            r.rescale(scale);
            r.to_string()
        }
        Err(_) => digits.to_owned(),
    }
}

/// Refuse an integer anywhere in a value that lies outside ±(2^53 − 1).
fn check_integers(value: &Value) -> Result<(), ValueError> {
    const MAX: u64 = (1 << 53) - 1;
    match value {
        Value::Number(n) => {
            let big = n.as_u64().is_some_and(|u| u > MAX)
                || n.as_i64().is_some_and(|i| i.unsigned_abs() > MAX);
            if big {
                return Err(ValueError::TooLarge(n.to_string()));
            }
            Ok(())
        }
        Value::Array(items) => items.iter().try_for_each(check_integers),
        Value::Object(m) => m.values().try_for_each(check_integers),
        _ => Ok(()),
    }
}

/// The value type the field names, held to the declaration given.
fn resolve<'a>(
    ty: &FieldType,
    given: Option<&'a ValueTypeDecl>,
) -> Result<Option<&'a ValueTypeDecl>, ValueError> {
    match (&ty.value_type, given) {
        (None, _) => Ok(None),
        (Some(name), Some(vt)) if vt.name == *name => Ok(Some(vt)),
        (Some(name), _) => Err(ValueError::UnknownValueType(name.clone())),
    }
}

fn check_kind(value: &Value, ty: &FieldType) -> Result<Value, ValueError> {
    let wrong = |expected| ValueError::WrongShape {
        kind: ty.kind,
        expected,
    };
    match ty.kind {
        StorageKind::Text | StorageKind::Reference => match value {
            Value::String(_) => Ok(value.clone()),
            _ => Err(wrong("a string")),
        },
        StorageKind::Number => match value {
            Value::Number(_) => {
                check_integers(value)?;
                Ok(value.clone())
            }
            _ => Err(wrong("a number")),
        },
        StorageKind::Boolean => match value {
            Value::Bool(_) => Ok(value.clone()),
            _ => Err(wrong("true or false")),
        },
        StorageKind::Json => {
            check_integers(value)?;
            Ok(value.clone())
        }
        StorageKind::Decimal => {
            let s = value.as_str().ok_or_else(|| wrong("a string of digits"))?;
            if is_decimal(s) {
                Ok(value.clone())
            } else {
                Err(ValueError::Decimal(s.to_owned()))
            }
        }
        StorageKind::DateTime => {
            let s = value.as_str().ok_or_else(|| wrong("a date-time string"))?;
            let t = Timestamp::parse(s).map_err(|_| ValueError::DateTime(s.to_owned()))?;
            Ok(Value::String(t.to_string()))
        }
        StorageKind::Bytes => {
            let s = value.as_str().ok_or_else(|| wrong("a base64 string"))?;
            if is_base64(s) {
                Ok(value.clone())
            } else {
                Err(ValueError::Base64)
            }
        }
        StorageKind::Vector => {
            check_integers(value)?;
            check_vector(value, ty)
        }
        StorageKind::Geo => {
            check_geo(value)?;
            check_integers(value)?;
            Ok(value.clone())
        }
    }
}

/// `-?[0-9]+(\.[0-9]+)?`.
fn is_decimal(s: &str) -> bool {
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let unsigned = s.strip_prefix('-').unwrap_or(s);
    match unsigned.split_once('.') {
        Some((int, frac)) => digits(int) && digits(frac),
        None => digits(unsigned),
    }
}

/// Standard base64 (RFC 4648 §4) with padding, in its one canonical
/// spelling: the unused bits before the padding must be zero, so that two
/// different strings never carry the same bytes.
fn is_base64(s: &str) -> bool {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let b = s.as_bytes();
    if b.len() & 3 != 0 {
        return false;
    }
    let pad = b.iter().rev().take_while(|&&c| c == b'=').count();
    if pad > 2 {
        return false;
    }
    let body = &b[..b.len() - pad];
    let mut last = 0;
    for &c in body {
        match ALPHABET.iter().position(|&a| a == c) {
            Some(i) => last = i,
            None => return false,
        }
    }
    match pad {
        1 => last & 0b11 == 0,
        2 => last & 0b1111 == 0,
        _ => true,
    }
}

fn check_vector(value: &Value, ty: &FieldType) -> Result<Value, ValueError> {
    let items = value.as_array().ok_or(ValueError::WrongShape {
        kind: StorageKind::Vector,
        expected: "an array of numbers",
    })?;
    if !items.iter().all(Value::is_number) {
        return Err(ValueError::WrongShape {
            kind: StorageKind::Vector,
            expected: "an array of numbers",
        });
    }
    let expected = ty.dimensions.ok_or(ValueError::NoDimensions)?;
    if items.len() != expected as usize {
        return Err(ValueError::VectorLength {
            expected,
            found: items.len(),
        });
    }
    Ok(value.clone())
}

/// A GeoJSON geometry (RFC 7946 §3.1): an object whose `type` names one
/// of the seven geometry types, with `coordinates` as an array, or for a
/// collection `geometries` as an array of geometries. A `Feature` is not a
/// geometry. Coordinates are not checked position by position.
fn check_geo(value: &Value) -> Result<(), ValueError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ValueError::Geo("not an object".into()))?;
    let ty = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ValueError::Geo("no string member `type`".into()))?;
    match ty {
        "Point" | "MultiPoint" | "LineString" | "MultiLineString" | "Polygon" | "MultiPolygon" => {
            match obj.get("coordinates") {
                Some(Value::Array(_)) => Ok(()),
                _ => Err(ValueError::Geo(format!(
                    "a {ty} needs a `coordinates` array"
                ))),
            }
        }
        "GeometryCollection" => match obj.get("geometries") {
            Some(Value::Array(parts)) => parts.iter().try_for_each(check_geo),
            _ => Err(ValueError::Geo(
                "a GeometryCollection needs a `geometries` array".into(),
            )),
        },
        other => Err(ValueError::Geo(format!("{other:?} is not a geometry type"))),
    }
}

fn check_one_of(value: &Value, vt: &ValueTypeDecl) -> Result<(), ValueError> {
    if vt.one_of.is_empty() {
        return Ok(());
    }
    match value.as_str() {
        Some(s) if vt.one_of.iter().any(|a| a == s) => Ok(()),
        _ => Err(ValueError::NotOneOf {
            value: value.to_string(),
            allowed: vt.one_of.clone(),
        }),
    }
}

fn check_format(value: &Value, vt: &ValueTypeDecl) -> Result<(), ValueError> {
    let Some(name) = vt.format.as_deref() else {
        return Ok(());
    };
    let Some(format) = ValueFormat::parse(name) else {
        return Err(ValueError::UnknownFormat {
            value_type: vt.name.clone(),
            format: name.to_owned(),
        });
    };
    match value.as_str() {
        Some(s) if format.matches(s) => Ok(()),
        _ => Err(ValueError::Format {
            value_type: vt.name.clone(),
            format,
        }),
    }
}

/// Holds a field to the value type it names: the value type must be
/// declared in `value_types`, and the field must be stored as its kind.
/// A field that names no value type always conforms.
pub fn conformance(field: &FieldDecl, value_types: &[ValueTypeDecl]) -> Result<(), ValueError> {
    let Some(name) = &field.ty.value_type else {
        return Ok(());
    };
    let vt = value_types
        .iter()
        .find(|vt| vt.name == *name)
        .ok_or_else(|| ValueError::UnknownValueType(name.clone()))?;
    if vt.kind != field.ty.kind {
        return Err(ValueError::KindMismatch {
            field: field.name.clone(),
            value_type: vt.name.clone(),
            expected: vt.kind,
            found: field.ty.kind,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
        for bad in [
            json!({"coordinates": [0, 0]}),
            json!({"type": "Point"}),
            json!({"type": "Feature", "geometry": polygon}),
            json!({"type": "GeometryCollection", "geometries": [{"type": "Circle"}]}),
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
}

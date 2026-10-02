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
            ValueError::Format { value_type, format } => write!(
                f,
                "the value is not a well-formed {} ({value_type})",
                format.name()
            ),
            ValueError::UnknownValueType(name) => write!(f, "no value type named {name}"),
            ValueError::UnknownFormat { value_type, format } => write!(
                f,
                "value type {value_type} names the unknown format `{format}`"
            ),
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
        StorageKind::Text | StorageKind::Reference if !value.is_string() => Err(wrong("a string")),
        StorageKind::Number if !value.is_number() => Err(wrong("a number")),
        StorageKind::Boolean if !value.is_boolean() => Err(wrong("true or false")),
        StorageKind::Text | StorageKind::Reference | StorageKind::Boolean => Ok(value.clone()),
        StorageKind::Number | StorageKind::Json => {
            check_integers(value)?;
            Ok(value.clone())
        }
        StorageKind::Decimal => {
            let s = value.as_str().ok_or_else(|| wrong("a string of digits"))?;
            let decimal = is_decimal(s).then(|| value.clone());
            decimal.ok_or_else(|| ValueError::Decimal(s.to_owned()))
        }
        StorageKind::DateTime => {
            let s = value.as_str().ok_or_else(|| wrong("a date-time string"))?;
            let t = Timestamp::parse(s).map_err(|_| ValueError::DateTime(s.to_owned()))?;
            Ok(Value::String(t.to_string()))
        }
        StorageKind::Bytes => {
            let s = value.as_str().ok_or_else(|| wrong("a base64 string"))?;
            is_base64(s)
                .then(|| value.clone())
                .ok_or(ValueError::Base64)
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
    let numbers = |items: &&Vec<Value>| items.iter().all(Value::is_number);
    let items = value
        .as_array()
        .filter(numbers)
        .ok_or(ValueError::WrongShape {
            kind: StorageKind::Vector,
            expected: "an array of numbers",
        })?;
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
/// of the seven geometry types, with `coordinates` of that type's shape,
/// or for a collection `geometries` as an array of geometries. A
/// `Feature` is not a geometry. A position is two or three numbers, a
/// line string at least two positions, and a polygon's every ring a
/// closed one: at least four positions, the last equal to the first.
/// Empty is allowed at the top only, the same for every type: empty
/// `coordinates` are the empty geometry of that type, and a collection
/// may hold no geometries, but a part that is present keeps its shape.
pub(crate) fn check_geo(value: &Value) -> Result<(), ValueError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ValueError::Geo("not an object".into()))?;
    let ty = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ValueError::Geo("no string member `type`".into()))?;
    if ty == "GeometryCollection" {
        return match obj.get("geometries") {
            Some(Value::Array(parts)) => parts.iter().try_for_each(check_geo),
            _ => Err(ValueError::Geo(
                "a GeometryCollection needs a `geometries` array".into(),
            )),
        };
    }
    let shape: fn(&Value) -> bool = match ty {
        "Point" => is_position,
        "MultiPoint" => |c| each(c, is_position),
        "LineString" => is_line,
        "MultiLineString" => |c| each(c, is_line),
        "Polygon" => is_polygon,
        "MultiPolygon" => |c| each(c, is_polygon),
        other => return Err(ValueError::Geo(format!("{other:?} is not a geometry type"))),
    };
    match obj.get("coordinates") {
        Some(Value::Array(c)) if c.is_empty() => Ok(()),
        Some(c) if shape(c) => Ok(()),
        Some(_) => Err(ValueError::Geo(format!(
            "the `coordinates` are not those of a {ty}"
        ))),
        None => Err(ValueError::Geo(format!("a {ty} needs `coordinates`"))),
    }
}

fn each(v: &Value, f: fn(&Value) -> bool) -> bool {
    v.as_array().is_some_and(|a| a.iter().all(f))
}

fn is_position(v: &Value) -> bool {
    v.as_array()
        .is_some_and(|a| (a.len() == 2 || a.len() == 3) && a.iter().all(Value::is_number))
}

fn is_line(v: &Value) -> bool {
    v.as_array()
        .is_some_and(|a| a.len() >= 2 && a.iter().all(is_position))
}

/// Linear rings, each closed: four positions or more, the last equal to
/// the first.
fn is_polygon(v: &Value) -> bool {
    v.as_array().is_some_and(|rings| {
        !rings.is_empty()
            && rings.iter().all(|ring| {
                ring.as_array().is_some_and(|ps| {
                    ps.len() >= 4 && ps.first() == ps.last() && ps.iter().all(is_position)
                })
            })
    })
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
mod tests;

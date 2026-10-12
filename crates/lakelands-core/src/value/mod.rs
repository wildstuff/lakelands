//! Checking one value against its field's type, and one field against
//! the value type it names.
//!
//! [`check`] takes a value as a writer sent it and returns it in its one
//! canonical JSON form (`docs/log-format.md` §5), or says why it does not
//! fit. It covers three things: the storage kind (the JSON shape each
//! kind has, a date-time normalised to its one form, a date to its day, a
//! decimal kept as written, a vector of the declared length), the value
//! type's `one_of`, and the value type's `format` when that names one of
//! the known [`ValueFormat`]s. A value type's `constraint` is a condition in the
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

use lakelands_api::error::{Case, Shape};
use lakelands_api::ontology::{
    FieldDecl, FieldType, StorageKind, ValueTypeDecl, is_vector_component,
};
use lakelands_api::{Timestamp, Value};

mod format;

pub use format::ValueFormat;

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
) -> Result<Value, Case> {
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
    let d = digits.parse::<rust_decimal::Decimal>();
    d.map_or_else(|_| digits.to_owned(), |d| at_scale(d, scale).to_string())
}

/// `d` at `scale` fractional digits, rounded half away from zero.
pub(crate) fn at_scale(d: rust_decimal::Decimal, scale: u32) -> rust_decimal::Decimal {
    let mut r =
        d.round_dp_with_strategy(scale, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    r.rescale(scale);
    r
}

/// Refuse an integer anywhere in a value that lies outside ±(2^53 − 1),
/// judged in the form the value is written in: the check answers a value
/// a second check answers the same.
fn check_integers(value: &Value) -> Result<(), Case> {
    match crate::canonical::integer_too_large(value) {
        Some(n) => Err(Case::IntegerTooLarge { n: n.clone() }),
        None => Ok(()),
    }
}

/// The value type the field names, held to the declaration given.
fn resolve<'a>(
    ty: &FieldType,
    given: Option<&'a ValueTypeDecl>,
) -> Result<Option<&'a ValueTypeDecl>, Case> {
    match (&ty.value_type, given) {
        (None, _) => Ok(None),
        (Some(name), Some(vt)) if vt.name == *name => Ok(Some(vt)),
        (Some(name), _) => Err(Case::NoValueType {
            value_type: name.clone(),
        }),
    }
}

/// A value in its canonical form if a field of `ty`'s storage kind takes
/// it written: [`check`] without the value type.
pub fn check_kind(value: &Value, ty: &FieldType) -> Result<Value, Case> {
    let wrong = |shape| Case::ValueShape {
        kind: ty.kind,
        shape,
    };
    match ty.kind {
        StorageKind::Text | StorageKind::Reference if !value.is_string() => Err(wrong(Shape::Text)),
        StorageKind::Number if !value.is_number() => Err(wrong(Shape::Number)),
        StorageKind::Boolean if !value.is_boolean() => Err(wrong(Shape::Boolean)),
        StorageKind::Text | StorageKind::Reference | StorageKind::Boolean => Ok(value.clone()),
        StorageKind::Number | StorageKind::Json => {
            check_integers(value)?;
            Ok(value.clone())
        }
        StorageKind::Decimal => {
            let s = value.as_str().ok_or_else(|| wrong(Shape::Digits))?;
            let decimal = is_decimal(s).then(|| value.clone());
            let value = s.escape_debug().to_string();
            decimal.ok_or(Case::NotDecimal { value })
        }
        StorageKind::DateTime => {
            let s = value.as_str().ok_or_else(|| wrong(Shape::DateTime))?;
            let value = || s.escape_debug().to_string();
            let t = Timestamp::parse(s).map_err(|_| Case::NotDateTime { value: value() })?;
            Ok(Value::String(t.to_string()))
        }
        StorageKind::Date => {
            let s = value.as_str().ok_or_else(|| wrong(Shape::Date))?;
            let day = Timestamp::parse(s).ok().and_then(Timestamp::day);
            let value = || s.escape_debug().to_string();
            day.map(Value::String)
                .ok_or_else(|| Case::NotDate { value: value() })
        }
        StorageKind::Bytes => {
            let s = value.as_str().ok_or_else(|| wrong(Shape::Base64))?;
            is_base64(s).then(|| value.clone()).ok_or(Case::NotBase64)
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

/// `-?[0-9]+(\.[0-9]+)?`: the decimal form of log format §5.
pub(crate) fn is_decimal(s: &str) -> bool {
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

fn check_vector(value: &Value, ty: &FieldType) -> Result<Value, Case> {
    let numbers = |items: &&Vec<Value>| items.iter().all(is_vector_component);
    let items = value.as_array().filter(numbers).ok_or(Case::ValueShape {
        kind: StorageKind::Vector,
        shape: Shape::Numbers,
    })?;
    let expected = ty.dimensions.ok_or(Case::VectorNoDimensions)?;
    if items.len() != expected as usize {
        return Err(Case::VectorLength {
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
pub(crate) fn check_geo(value: &Value) -> Result<(), Case> {
    let obj = value.as_object().ok_or(Case::GeoNotObject)?;
    let ty = obj.get("type").and_then(Value::as_str);
    let ty = ty.ok_or(Case::GeoNoType)?;
    if ty == "GeometryCollection" {
        return match obj.get("geometries") {
            Some(Value::Array(parts)) => parts.iter().try_for_each(check_geo),
            _ => Err(Case::GeoNoGeometries),
        };
    }
    let shape: fn(&Value) -> bool = match ty {
        "Point" => is_position,
        "MultiPoint" => |c| each(c, is_position),
        "LineString" => is_line,
        "MultiLineString" => |c| each(c, is_line),
        "Polygon" => is_polygon,
        "MultiPolygon" => |c| each(c, is_polygon),
        other => {
            let name = other.escape_debug().to_string();
            return Err(Case::GeoUnknownType { name });
        }
    };
    let geometry = ty.to_owned();
    match obj.get("coordinates") {
        Some(Value::Array(c)) if c.is_empty() => Ok(()),
        Some(c) if shape(c) => Ok(()),
        Some(_) => Err(Case::GeoCoordinates { geometry }),
        None => Err(Case::GeoNoCoordinates { geometry }),
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

fn check_one_of(value: &Value, vt: &ValueTypeDecl) -> Result<(), Case> {
    if vt.one_of.is_empty() {
        return Ok(());
    }
    match value.as_str() {
        Some(s) if vt.one_of.iter().any(|a| a == s) => Ok(()),
        _ => Err(Case::NotOneOf {
            value: value.clone(),
            allowed: vt.one_of.iter().collect(),
        }),
    }
}

fn check_format(value: &Value, vt: &ValueTypeDecl) -> Result<(), Case> {
    let Some(name) = vt.format.as_deref() else {
        return Ok(());
    };
    let Some(format) = ValueFormat::parse(name) else {
        return Err(Case::ValueTypeFormat {
            value_type: vt.name.clone(),
            format: name.to_owned(),
        });
    };
    match value.as_str() {
        Some(s) if format.matches(s) => Ok(()),
        _ => Err(Case::NotFormat {
            format: format.name().to_owned(),
            value_type: vt.name.clone(),
        }),
    }
}

/// Holds a field to the value type it names: the value type must be
/// declared in `value_types`, and the field must be stored as its kind.
/// A field that names no value type always conforms.
pub fn conformance(field: &FieldDecl, value_types: &[ValueTypeDecl]) -> Result<(), Case> {
    let Some(name) = &field.ty.value_type else {
        return Ok(());
    };
    let vt = value_types
        .iter()
        .find(|vt| vt.name == *name)
        .ok_or_else(|| Case::NoValueType {
            value_type: name.clone(),
        })?;
    if vt.kind != field.ty.kind {
        return Err(Case::ValueTypeKind {
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

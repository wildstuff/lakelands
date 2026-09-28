//! How stored values convert when a migration changes a field's type,
//! and the preflight that says whether every stored value will.
//!
//! A value converts only when nothing is lost: converting it back gives
//! the same information. The table, over the JSON forms of
//! `docs/log-format.md` §5 (any pair not listed is lossy):
//!
//! | value → target | rule |
//! |---|---|
//! | any value already in the target's form | kept byte for byte, never rewritten |
//! | same storage kind, other value type, target or length | kept when it fits the new type, else lossy |
//! | number → text | its canonical number form (`42`, `4.5`, `1e+21`) |
//! | boolean → text | `"true"` / `"false"` |
//! | text or decimal → number | a numeral whose binary floating-point value is numerically the same (`"4.50"` → `4.5`; `"0.10000000000000001"` is lossy) |
//! | number → decimal | an integer, or an integral float within ±2^53, as its digits |
//! | text → boolean | exactly `"true"` or `"false"` |
//! | text → date-time | a date, or a date-time in UTC, rewritten in the one form of §1.3; a non-zero offset is lossy |
//! | anything → json | kept as it is |
//!
//! Deliberately lossy: a fractional number into a decimal (a binary
//! float only approximates the decimal it was written from, and a
//! decimal promises exactness), a fractional number into anything
//! integral, a boolean into a number, a date-time with an offset (the
//! offset is dropped), and any value into a vector or a geometry unless
//! it already is one.
//!
//! `null` is not a value: it stays `null` under every conversion.
//!
//! [`preflight`] runs the table over every stored record of a type
//! before a migration and reports what it found; [`migrate_record`] is
//! the same plan applied to one record, which is what the fold does once
//! the migration is applied. Both read renames from the migration, so a
//! renamed field's values move to its new name.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, TypeDecl};
use lakelands_api::write::{FieldConversion, Migration, MigrationReport};
use lakelands_api::{FieldName, RecordKey, Timestamp};
use serde_json::Value;

use super::{ShapeError, schema_hash};
use crate::canonical;

/// The outcome of converting one value to a new field type.
#[derive(Clone, Debug, PartialEq)]
pub enum Conversion {
    /// The value converts without loss; this is its new form.
    Exact(Value),
    /// The value would lose information, or has no form in the new type.
    Lossy,
}

/// Convert a stored value from one field type to another, by the table
/// in the module documentation. `from` is the type the value was stored
/// under: when it has the same storage kind as `to`, the value is kept
/// unchanged or not at all.
pub fn convert(value: &Value, from: &FieldType, to: &FieldType) -> Conversion {
    if value.is_null() || fits(to, value) {
        return Conversion::Exact(value.clone());
    }
    if from.kind == to.kind {
        return Conversion::Lossy;
    }
    let out = match (to.kind, value) {
        (StorageKind::Text, Value::Number(n)) => Some(Value::String(number_text(n))),
        (StorageKind::Text, Value::Bool(b)) => Some(Value::String(b.to_string())),
        (StorageKind::Number, Value::String(s)) => numeral_to_number(s),
        (StorageKind::Decimal, Value::Number(n)) => integral_digits(n).map(Value::String),
        (StorageKind::Boolean, Value::String(s)) => match s.as_str() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        (StorageKind::DateTime, Value::String(s)) => utc_instant(s).map(Value::String),
        _ => None,
    };
    match out {
        Some(v) if fits(to, &v) => Conversion::Exact(v),
        _ => Conversion::Lossy,
    }
}

/// Whether a (non-null) value is in the JSON form of a field type
/// (`docs/log-format.md` §5), including a vector's declared length.
/// Value-type constraints are not checked here; they need the value
/// type's declaration.
pub fn fits(ty: &FieldType, value: &Value) -> bool {
    match ty.kind {
        StorageKind::Text => value.is_string(),
        StorageKind::Number => value.is_number(),
        StorageKind::Decimal => value.as_str().is_some_and(is_decimal),
        StorageKind::Boolean => value.is_boolean(),
        StorageKind::DateTime => value
            .as_str()
            .is_some_and(|s| Timestamp::parse(s).is_ok_and(|t| t.to_string() == s)),
        StorageKind::Bytes => value.as_str().is_some_and(is_base64),
        StorageKind::Json => !value.is_null(),
        StorageKind::Reference => value
            .as_str()
            .is_some_and(|s| !s.is_empty() && !s.contains('\0')),
        StorageKind::Vector => value.as_array().is_some_and(|items| {
            items.iter().all(Value::is_number)
                && ty.dimensions.is_none_or(|d| items.len() == d as usize)
        }),
        StorageKind::Geo => is_geometry(value),
    }
}

/// Run a migration's conversions over the stored records of the type it
/// migrates, without changing anything, and report what would happen.
///
/// The report lists every field of the new declaration whose stored
/// values must pass a check: a field whose type changes, and a field
/// that becomes required (or is added as required), where a record
/// without a value counts as not convertible. Up to five record keys per
/// field name records that would not convert, the five smallest, so the
/// report does not depend on the order the records arrive in. Values
/// never appear in the report.
///
/// Fields the new declaration no longer has, and no rename carries, are
/// listed in `dropped` with the number of records holding a value there.
///
/// `report_id` is the SHA-256 of the report's canonical bytes with
/// `report_id` set to `""`: any difference in the report is a different
/// id, so applying a migration by id applies exactly what was shown.
pub fn preflight(
    before: &TypeDecl,
    migration: &Migration,
    records: impl IntoIterator<Item = (RecordKey, BTreeMap<FieldName, Value>)>,
) -> Result<MigrationReport, ShapeError> {
    let plan = Plan::new(before, migration)?;
    let mut tallies: Vec<Tally> = plan.checked().map(|_| Tally::default()).collect();
    // Fields of the old declaration no step of the new one reads from:
    // the migration removes them, and the report names what that discards.
    let kept: std::collections::BTreeSet<&FieldName> = plan
        .steps
        .iter()
        .filter_map(|s| s.from.map(|f| &f.name))
        .collect();
    let mut dropped: Vec<(FieldName, u64)> = before
        .fields
        .iter()
        .filter(|f| !kept.contains(&f.name))
        .map(|f| (f.name.clone(), 0))
        .collect();
    let mut records_scanned = 0u64;
    for (key, fields) in records {
        records_scanned += 1;
        for (name, holding) in dropped.iter_mut() {
            if fields.get(name).is_some_and(|v| !v.is_null()) {
                *holding += 1;
            }
        }
        for (step, tally) in plan.checked().zip(tallies.iter_mut()) {
            match step.convert(&fields) {
                Some(Some(_)) => tally.convertible += 1,
                Some(None) => {}
                None => {
                    tally.not_convertible += 1;
                    tally.sample(&key);
                }
            }
        }
    }
    let fields: Vec<FieldConversion> = plan
        .checked()
        .zip(tallies)
        .map(|(step, tally)| FieldConversion {
            field: step.to.name.clone(),
            from: step
                .from
                .map_or_else(|| step.to.ty.clone(), |f| f.ty.clone()),
            to: step.to.ty.clone(),
            convertible: tally.convertible,
            not_convertible: tally.not_convertible,
            sample_keys: tally.samples.into_iter().collect(),
        })
        .collect();
    let mut report = MigrationReport {
        ty: before.name.clone(),
        from_schema_hash: schema_hash(before)?,
        to_schema_hash: schema_hash(&migration.to)?,
        records_scanned,
        lossless: fields.iter().all(|f| f.not_convertible == 0),
        fields,
        dropped,
        rekeyed: Vec::new(),
        references: Vec::new(),
        sample_collisions: Vec::new(),
        report_id: String::new(),
    };
    report.report_id = canonical::sha256_hex(
        &serde_json::to_value(&report).expect("a migration report serialises to JSON"),
    );
    Ok(report)
}

/// Convert one stored record by a migration: renamed fields move to
/// their new names, retyped fields convert, removed fields are dropped.
/// `None` when any checked field would not convert, which a lossless
/// preflight rules out.
pub fn migrate_record(
    before: &TypeDecl,
    migration: &Migration,
    fields: &BTreeMap<FieldName, Value>,
) -> Result<Option<BTreeMap<FieldName, Value>>, ShapeError> {
    let plan = Plan::new(before, migration)?;
    let mut out = BTreeMap::new();
    for step in &plan.steps {
        let value = if step.checked {
            match step.convert(fields) {
                Some(v) => v,
                None => return Ok(None),
            }
        } else {
            step.stored(fields).cloned()
        };
        if let Some(v) = value {
            out.insert(step.to.name.clone(), v);
        }
    }
    Ok(Some(out))
}

/// Per field of the new declaration: where its values come from and
/// whether they must pass a check.
struct Plan<'a> {
    steps: Vec<Step<'a>>,
}

struct Step<'a> {
    /// The field after the migration.
    to: &'a FieldDecl,
    /// The field its values come from, if any.
    from: Option<&'a FieldDecl>,
    /// Whether the report lists it: its type changes, or it becomes
    /// required.
    checked: bool,
}

impl<'a> Plan<'a> {
    fn new(before: &'a TypeDecl, migration: &'a Migration) -> Result<Plan<'a>, ShapeError> {
        let after = &migration.to;
        if after.name != before.name {
            return Err(ShapeError::OtherType {
                from: before.name.clone(),
                to: after.name.clone(),
            });
        }
        let old: BTreeMap<&FieldName, &FieldDecl> =
            before.fields.iter().map(|f| (&f.name, f)).collect();
        let new: BTreeSet<&FieldName> = after.fields.iter().map(|f| &f.name).collect();
        let mut sources: BTreeMap<&FieldName, &FieldName> = BTreeMap::new();
        let mut renamed: BTreeSet<&FieldName> = BTreeSet::new();
        for (from, to) in &migration.renames {
            if !old.contains_key(from) {
                return Err(ShapeError::RenameFromUnknown { from: from.clone() });
            }
            if !new.contains(to) {
                return Err(ShapeError::RenameToUnknown { to: to.clone() });
            }
            if !renamed.insert(from) {
                return Err(ShapeError::RenameRepeated {
                    field: from.clone(),
                });
            }
            if sources.insert(to, from).is_some() {
                return Err(ShapeError::RenameRepeated { field: to.clone() });
            }
        }
        for to in sources.keys() {
            if old.contains_key(to) && !renamed.contains(to) {
                return Err(ShapeError::RenameCollides { to: (*to).clone() });
            }
        }
        let mut steps: Vec<Step<'a>> = after
            .fields
            .iter()
            .map(|to| {
                let source = match sources.get(&to.name) {
                    Some(from) => Some(*from),
                    None if !renamed.contains(&to.name) => Some(&to.name),
                    None => None,
                };
                let from = source.and_then(|name| old.get(name).copied());
                let retyped = from.is_some_and(|f| f.ty != to.ty);
                let tightened = to.required && !from.is_some_and(|f| f.required);
                Step {
                    to,
                    from,
                    checked: retyped || tightened,
                }
            })
            .collect();
        steps.sort_by(|a, b| a.to.name.cmp(&b.to.name));
        Ok(Plan { steps })
    }

    /// The steps the report lists, in field-name order.
    fn checked(&self) -> impl Iterator<Item = &Step<'a>> {
        self.steps.iter().filter(|s| s.checked)
    }
}

impl Step<'_> {
    /// The stored value this field takes over, if any.
    fn stored<'v>(&self, fields: &'v BTreeMap<FieldName, Value>) -> Option<&'v Value> {
        let from = self.from?;
        fields.get(&from.name).filter(|v| !v.is_null())
    }

    /// The field's value after the migration: `Some(None)` for an absent
    /// optional value, `None` when the record would not match.
    fn convert(&self, fields: &BTreeMap<FieldName, Value>) -> Option<Option<Value>> {
        match (self.stored(fields), self.from) {
            (Some(v), Some(from)) => match convert(v, &from.ty, &self.to.ty) {
                Conversion::Exact(v) => Some(Some(v)),
                Conversion::Lossy => None,
            },
            _ if self.to.required => None,
            _ => Some(None),
        }
    }
}

#[derive(Default)]
struct Tally {
    convertible: u64,
    not_convertible: u64,
    samples: BTreeSet<RecordKey>,
}

impl Tally {
    const SAMPLES: usize = 5;

    fn sample(&mut self, key: &RecordKey) {
        if self.samples.len() < Self::SAMPLES {
            self.samples.insert(key.clone());
        } else if self.samples.last().is_some_and(|last| key < last) {
            self.samples.insert(key.clone());
            self.samples.pop_last();
        }
    }
}

// ---- the JSON forms ------------------------------------------------------

/// `-?[0-9]+(\.[0-9]+)?`: the decimal form of log format §5.
fn is_decimal(s: &str) -> bool {
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let unsigned = s.strip_prefix('-').unwrap_or(s);
    match unsigned.split_once('.') {
        Some((int, frac)) => digits(int) && digits(frac),
        None => digits(unsigned),
    }
}

/// Standard base64 with padding.
fn is_base64(s: &str) -> bool {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(4) {
        return false;
    }
    let pad = b.iter().rev().take_while(|&&c| c == b'=').count();
    pad <= 2
        && b[..b.len() - pad]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'+' || *c == b'/')
}

/// A GeoJSON geometry object, by the rule a patch is checked with.
fn is_geometry(v: &Value) -> bool {
    crate::value::check_geo(v).is_ok()
}

// ---- the conversions -----------------------------------------------------

const MAX_EXACT_INTEGER: f64 = 9_007_199_254_740_992.0; // 2^53

/// A number in its canonical text form: the digits of an integer, the
/// RFC 8785 form of a float.
fn number_text(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else {
        canonical::number(n.as_f64().expect("a JSON number is an integer or a float"))
    }
}

/// A numeral as a number, when the nearest binary float has numerically
/// the same value.
fn numeral_to_number(s: &str) -> Option<Value> {
    let written = normal(s)?;
    let f: f64 = s.parse().ok().filter(|f: &f64| f.is_finite())?;
    if normal(&canonical::number(f))? != written {
        return None;
    }
    if f.fract() == 0.0 && f.abs() <= MAX_EXACT_INTEGER {
        return Some(Value::from(f as i64));
    }
    serde_json::Number::from_f64(f).map(Value::Number)
}

/// An integral number's digits.
fn integral_digits(n: &serde_json::Number) -> Option<String> {
    if let Some(i) = n.as_i64() {
        return Some(i.to_string());
    }
    if let Some(u) = n.as_u64() {
        return Some(u.to_string());
    }
    let f = n.as_f64()?;
    (f.fract() == 0.0 && f.abs() <= MAX_EXACT_INTEGER).then(|| (f as i64).to_string())
}

/// A date, or a date-time in UTC, in the one form of log format §1.3. A
/// date-time with a non-zero offset is refused: the offset would be lost.
fn utc_instant(s: &str) -> Option<String> {
    let t = Timestamp::parse(s).ok()?;
    let offset =
        s.len() > 10 && matches!(s.as_bytes()[s.len() - 6], b'+' | b'-') && !s.ends_with("00:00");
    (!offset).then(|| t.to_string())
}

/// A decimal numeral, `-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?`, reduced to
/// its value: `(negative, significant digits, n)` meaning
/// `0.digits × 10^n`. Zero is `(false, "", 0)`.
fn normal(s: &str) -> Option<(bool, String, i64)> {
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let (negative, unsigned) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((m, e)) => {
            let e = e.strip_prefix('+').unwrap_or(e);
            let magnitude = e.strip_prefix('-').unwrap_or(e);
            if !digits(magnitude) {
                return None;
            }
            (m, e.parse::<i64>().ok()?)
        }
        None => (unsigned, 0),
    };
    let (int, frac) = match mantissa.split_once('.') {
        Some((i, f)) if digits(i) && digits(f) => (i, f),
        None if digits(mantissa) => (mantissa, ""),
        _ => return None,
    };
    let mut all = format!("{int}{frac}");
    let mut n = int.len() as i64 + exponent;
    let leading = all.len() - all.trim_start_matches('0').len();
    all.drain(..leading);
    n -= leading as i64;
    all.truncate(all.trim_end_matches('0').len());
    if all.is_empty() {
        return Some((false, all, 0));
    }
    Some((negative, all, n))
}

#[cfg(test)]
mod tests {
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
}

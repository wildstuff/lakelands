//! How stored values convert when a migration changes a field's type,
//! and the preflight that says whether every stored value will.
//!
//! A value converts only when nothing is lost. The table, over the JSON
//! forms of `docs/log-format.md` §5 (any pair not listed is lossy):
//!
//! | value → target | rule |
//! |---|---|
//! | any value already in the target's form | kept byte for byte, never rewritten |
//! | same storage kind, other value type, target or length | kept when it fits the new type, else lossy; a reference given another target is kept where it names a live record of that target, which the store looks up, else lossy (lakelands#1071) |
//! | number → text | its canonical number form (`42`, `4.5`, `1e+21`) |
//! | boolean → text | `"true"` / `"false"` |
//! | text or decimal → number | a numeral whose binary floating-point value is numerically the same (`"4.50"` → `4.5`; `"0.10000000000000001"` is lossy) |
//! | number → decimal | an integer, or an integral float within ±2^53, as its digits |
//! | text → boolean | exactly `"true"` or `"false"` |
//! | text → date-time | a date, or a date-time in UTC, rewritten in the one form of §1.3; a non-zero offset is lossy |
//! | text or date-time → date | a date, or a date-time in UTC at a midnight, as its date; any other instant, or a non-zero offset, is lossy |
//! | date → date-time | its midnight |
//! | anything → json | kept as it is |
//!
//! Deliberately lossy: a fractional number into a decimal (a binary float
//! only approximates the decimal it was written from), a fractional number
//! into anything integral, a boolean into a number, a date-time with an
//! offset, and any value into a vector or a geometry unless it already is
//! one. `null` stays `null` under every conversion.
//!
//! [`preflight`] runs the table over every stored record of a type before
//! a migration and reports what it found; [`migrate_record`] applies the
//! same plan to one record, as the fold does. Both read renames from the
//! migration.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::error::{Case, Said};
use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, TypeDecl, is_vector_component};
use lakelands_api::write::{FieldConversion, Migration, MigrationReport};
use lakelands_api::{FieldName, RecordKey, Timestamp};
use serde_json::Value;

use super::schema_hash;
use crate::canonical;
use crate::value::{check_geo, is_decimal};

/// The outcome of converting one value to a new field type.
#[derive(Clone, Debug, PartialEq)]
pub enum Conversion {
    /// The value converts without loss; this is its new form.
    Exact(Value),
    /// The value would lose information, or has no form in the new type.
    Lossy,
}

/// Convert a stored value from one field type to another, by the table in
/// the module documentation. `from` is the type it was stored under: with
/// the same storage kind as `to`, the value is kept unchanged or not at all.
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
        (StorageKind::DateTime, Value::String(s)) => utc_instant(s).map(|t| t.to_string().into()),
        (StorageKind::Date, Value::String(s)) => {
            utc_instant(s).and_then(Timestamp::day).map(Value::String)
        }
        _ => None,
    };
    match out {
        Some(v) if fits(to, &v) => Conversion::Exact(v),
        _ => Conversion::Lossy,
    }
}

/// Whether a (non-null) value is in the JSON form of a field type
/// (`docs/log-format.md` §5), including a vector's declared length and the
/// range of an integer (§1.2): what a field would not take written, it does
/// not take computed. Value-type constraints are not checked here.
pub fn fits(ty: &FieldType, value: &Value) -> bool {
    if canonical::integer_too_large(value).is_some() {
        return false;
    }
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
            items.iter().all(is_vector_component)
                && ty.dimensions.is_none_or(|d| items.len() == d as usize)
        }),
        StorageKind::Geo => check_geo(value).is_ok(),
        StorageKind::Date => value.as_str().is_some_and(|s| {
            Timestamp::parse(s).ok().and_then(Timestamp::day).as_deref() == Some(s)
        }),
    }
}

/// Run a migration's conversions over the stored records of the type it
/// migrates, without changing anything, and report what would happen.
///
/// The report lists every field of the new declaration whose stored values
/// must pass a check: a field whose type changes, and one that becomes
/// required, where a record without a value counts as not convertible. Up
/// to the five smallest keys of records that would not convert are named
/// per field, so the report does not depend on arrival order. Values never
/// appear in it.
///
/// Fields the new declaration no longer has, and no rename carries, are
/// listed in `dropped` with the number of records holding a value there.
///
/// `report_id` is the SHA-256 of the report's canonical bytes with
/// `report_id` set to `""`, so applying a migration by id applies exactly
/// what was shown.
pub fn preflight(
    before: &TypeDecl,
    migration: &Migration,
    records: impl IntoIterator<Item = (RecordKey, BTreeMap<FieldName, Value>)>,
    answered: &dyn Fn(&RecordKey) -> bool,
) -> Result<MigrationReport, Said> {
    referring(before, migration, records, answered, &|_, _| false)
}

/// [`preflight`], where `dangling` says of a field's converted value that
/// it names no live record of the field's new target (lakelands#1071).
pub fn referring(
    before: &TypeDecl,
    migration: &Migration,
    records: impl IntoIterator<Item = (RecordKey, BTreeMap<FieldName, Value>)>,
    answered: &dyn Fn(&RecordKey) -> bool,
    dangling: &dyn Fn(&FieldName, &Value) -> bool,
) -> Result<MigrationReport, Said> {
    let plan = Plan::new(before, migration)?;
    let mut tallies: Vec<Tally> = plan.checked().map(|_| Tally::default()).collect();
    // Old fields no step reads from: the migration discards them.
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
                Some(Some(v)) if !dangling(&step.to.name, &v) => tally.convertible += 1,
                Some(None) => {}
                _ => {
                    tally.not_convertible += 1;
                    tally.sample(&key, answered);
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
        breaches: Default::default(),
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
        more: false,
        relations: Vec::new(),
        carried: Vec::new(),
        restated: Vec::new(),
        filled: Vec::new(),
        recomputed: Some(0),
        unlive: Vec::new(),
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
) -> Result<Option<BTreeMap<FieldName, Value>>, Said> {
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
    /// Whether the migration fills it (`Migration::fill`): a record that
    /// holds no value for it takes its `initial` in the fold, or, retired,
    /// is left as it was.
    fill: bool,
}

impl<'a> Plan<'a> {
    fn new(before: &'a TypeDecl, migration: &'a Migration) -> Result<Plan<'a>, Case> {
        let after = &migration.to;
        if after.name != before.name {
            return Err(Case::MigrationOtherType {
                from: before.name.clone(),
                to: after.name.clone(),
            });
        }
        let old: BTreeMap<&FieldName, &FieldDecl> =
            before.fields.iter().map(|f| (&f.name, f)).collect();
        let pairs = migration.renamed_fields(before)?;
        let sources: BTreeMap<&FieldName, &FieldName> =
            pairs.iter().map(|(f, t)| (*t, *f)).collect();
        let renamed: BTreeSet<&FieldName> = pairs.iter().map(|(f, _)| *f).collect();
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
                    fill: migration.fill.contains(&to.name),
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
            _ if self.to.required && !self.fill => None,
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

    /// Keeps the smallest keys among those `answered` admits, asking only
    /// of a key that would be a sample.
    fn sample(&mut self, key: &RecordKey, answered: &dyn Fn(&RecordKey) -> bool) {
        let full = self.samples.len() == Self::SAMPLES;
        let takes = !full || self.samples.last().is_some_and(|last| key < last);
        if takes && answered(key) && self.samples.insert(key.clone()) && full {
            self.samples.pop_last();
        }
    }
}

// ---- the JSON forms ------------------------------------------------------

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

// ---- the conversions -----------------------------------------------------

/// The largest integer every reader reads exactly, 2^53 − 1
/// (`docs/log-format.md` §1.2).
const MAX_EXACT_INTEGER: f64 = 9_007_199_254_740_991.0;

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
    if n.is_i64() || n.is_u64() {
        return Some(number_text(n));
    }
    let f = n.as_f64()?;
    (f.fract() == 0.0 && f.abs() <= MAX_EXACT_INTEGER).then(|| (f as i64).to_string())
}

/// A date, or a date-time in UTC, in the one form of log format §1.3. A
/// date-time with a non-zero offset is refused: the offset would be lost.
fn utc_instant(s: &str) -> Option<Timestamp> {
    let t = Timestamp::parse(s).ok()?;
    let offset =
        s.len() > 10 && matches!(s.as_bytes()[s.len() - 6], b'+' | b'-') && !s.ends_with("00:00");
    (!offset).then_some(t)
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
mod tests;

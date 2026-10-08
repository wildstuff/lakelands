//! When a record holds in the world: the one `valid_at` rule.
//!
//! A valid-timed type names a `date_time` or a `date` field holding when a
//! record became true (`valid_from`) and optionally one holding when it
//! stopped (`valid_to`). A field of any other kind is refused at the
//! declaration (028, extended), so a value that only reads as an instant
//! never sets a record's validity; a date is the instant at its UTC
//! midnight.
//! A read at `valid_at = t` sees a record when
//!
//! ```text
//! valid_from <= t  and  (valid_to is absent  or  t < valid_to)
//! ```
//!
//! The interval is closed at its start and open at its end, so two records
//! where one ends exactly when the next begins never both hold at that
//! instant.
//!
//! A write holds a `date_time` value to a UTC instant and a `date` to a
//! calendar date, so a valid time is an instant, a date or absent; this
//! module still reads any string that parses as an instant, since it is
//! handed values, not their field's kind. A record whose `valid_from` is absent is **not**
//! valid at any `valid_at`: a record that cannot say when it became true
//! cannot claim to have been true at a given moment. Such a record is
//! still read by a query without `valid_at`. An absent `valid_to` means
//! the record has not been shown to end.
//!
//! The rule lives here, once, because every engine that pushes `valid_at`
//! into its own query must admit exactly the records this function
//! admits; the conformance suite compares them.

use lakelands_api::ontology::TypeDecl;
use lakelands_api::{FieldName, Timestamp, Value};
use std::collections::BTreeMap;

/// A value as an instant: any string that parses as one, a date as its
/// UTC midnight. A value that does not, or none, is no anchor.
pub fn anchor(value: Option<&Value>) -> Option<Timestamp> {
    value
        .and_then(Value::as_str)
        .and_then(|s| Timestamp::parse(s).ok())
}

/// Whether an interval holds at `t`.
pub fn holds(valid_from: Option<Timestamp>, valid_to: Option<Timestamp>, t: Timestamp) -> bool {
    valid_from.is_some_and(|from| from <= t && valid_to.is_none_or(|to| t < to))
}

/// Whether a record of `ty` holds at `t`. A type that is not valid-timed
/// holds at every `t`.
pub fn record_holds(ty: &TypeDecl, fields: &BTreeMap<FieldName, Value>, t: Timestamp) -> bool {
    let Some(from_field) = &ty.valid_from else {
        return true;
    };
    let to = ty.valid_to.as_ref().and_then(|f| anchor(fields.get(f)));
    holds(anchor(fields.get(from_field)), to, t)
}

#[cfg(test)]
mod tests;

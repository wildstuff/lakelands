//! When a record holds in the world: the one `valid_at` rule.
//!
//! A valid-timed type names a field holding when a record became true
//! (`valid_from`) and optionally one holding when it stopped (`valid_to`).
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
//! A record whose `valid_from` is absent or is not an instant is **not**
//! valid at any `valid_at`: a record that cannot say when it became true
//! cannot claim to have been true at a given moment. Such a record is
//! still read by a query without `valid_at`. An unreadable `valid_to` is
//! treated as absent — the record has not been shown to end.
//!
//! The rule lives here, once, because every engine that pushes `valid_at`
//! into its own query must admit exactly the records this function
//! admits; the conformance suite compares them.

use lakelands_api::ontology::TypeDecl;
use lakelands_api::{FieldName, Timestamp, Value};
use std::collections::BTreeMap;

/// An anchor value as an instant: a date-time string in any form
/// `Timestamp::parse` accepts. Anything else is not an anchor.
pub fn anchor(value: Option<&Value>) -> Option<Timestamp> {
    value
        .and_then(Value::as_str)
        .and_then(|s| Timestamp::parse(s).ok())
}

/// Whether an interval holds at `t`.
pub fn holds(valid_from: Option<Timestamp>, valid_to: Option<Timestamp>, t: Timestamp) -> bool {
    match valid_from {
        Some(from) => from <= t && valid_to.is_none_or(|to| t < to),
        None => false,
    }
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

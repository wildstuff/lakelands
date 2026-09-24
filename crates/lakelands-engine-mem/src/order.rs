//! How a scan orders records: by its sort keys, missing values last in
//! either direction, then by record key.
//!
//! Values compare as the one evaluator compares them
//! (`lakelands_core::expr::eval::compare`): numbers, and text that reads
//! as a number, numerically; instants, and text that reads as one, as
//! instants; other text as text. A sort needs a total order, and the
//! evaluator's comparison has none across those kinds (`9 < '10'` and
//! `'10' < 'abc'` as text, yet `9` and `'abc'` do not compare). So values
//! are ranked by kind first — numbers, then instants, then text, then
//! booleans (`false` first), then JSON values by their canonical bytes —
//! and compared by the evaluator within a kind.

use std::cmp::Ordering;

use lakelands_api::engine::After;
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::record::{StoredRecord, Value};
pub(crate) use lakelands_core::expr::order::cmp_values;

/// The value a sort key reads from a record: the field's value, or when
/// it was learned or observed. `None` when absent.
pub(crate) fn sort_value(record: &StoredRecord, order: &Order) -> Option<Value> {
    match order.by {
        OrderBy::Value => record
            .fields
            .get(&order.field)
            .filter(|v| !v.is_null())
            .cloned(),
        OrderBy::KnownAt => record
            .meta
            .get(&order.field)
            .map(|m| Value::String(m.known_at.to_string())),
        OrderBy::SeenAt => record
            .meta
            .get(&order.field)
            .and_then(|m| m.seen_at)
            .map(|t| Value::String(t.to_string())),
    }
}

/// One sort key's order between two optional values: missing last in
/// either direction.
fn cmp_key(a: Option<&Value>, b: Option<&Value>, dir: Direction) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => match dir {
            Direction::Asc => cmp_values(a, b),
            Direction::Desc => cmp_values(b, a),
        },
    }
}

/// A row's place in a scan: its sort values and its key.
pub(crate) struct Place<'a> {
    /// The sort keys' values, `None` where absent.
    pub(crate) values: Vec<Option<Value>>,
    /// The record key.
    pub(crate) key: &'a str,
}

impl<'a> Place<'a> {
    /// Where a record sorts under `order_by`.
    pub(crate) fn of(record: &'a StoredRecord, order_by: &[Order]) -> Place<'a> {
        Place {
            values: order_by.iter().map(|o| sort_value(record, o)).collect(),
            key: record.key.as_str(),
        }
    }

    /// Where a keyset continuation sits; a null value there is absent.
    pub(crate) fn after(after: &'a After) -> Place<'a> {
        Place {
            values: after
                .values
                .iter()
                .map(|v| (!v.is_null()).then(|| v.clone()))
                .collect(),
            key: after.key.as_str(),
        }
    }
}

/// Two places under `order_by`; the key, ascending, breaks every tie.
pub(crate) fn cmp_places(a: &Place<'_>, b: &Place<'_>, order_by: &[Order]) -> Ordering {
    order_by
        .iter()
        .enumerate()
        .map(|(i, o)| {
            cmp_key(
                a.values.get(i).and_then(Option::as_ref),
                b.values.get(i).and_then(Option::as_ref),
                o.dir,
            )
        })
        .find(|o| o.is_ne())
        .unwrap_or_else(|| a.key.cmp(b.key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_compare_numerically_even_as_text() {
        assert_eq!(cmp_values(&json!(9), &json!("10")), Ordering::Less);
        assert_eq!(cmp_values(&json!("1.0"), &json!(1)), Ordering::Equal);
        assert_eq!(cmp_values(&json!(2.5), &json!(-3)), Ordering::Greater);
    }

    #[test]
    fn instants_compare_as_instants_and_text_as_text() {
        assert_eq!(
            cmp_values(
                &json!("2026-01-01T01:00:00+02:00"),
                &json!("2026-01-01T00:00:00Z")
            ),
            Ordering::Less
        );
        assert_eq!(
            cmp_values(&json!("apple"), &json!("banana")),
            Ordering::Less
        );
    }

    #[test]
    fn mixed_kinds_order_totally() {
        let mut values = vec![
            json!("abc"),
            json!(true),
            json!("10"),
            json!({"a": 1}),
            json!("2026-01-01"),
            json!(9),
            json!(false),
        ];
        values.sort_by(cmp_values);
        assert_eq!(
            values,
            vec![
                json!(9),
                json!("10"),
                json!("2026-01-01"),
                json!("abc"),
                json!(false),
                json!(true),
                json!({"a": 1}),
            ]
        );
    }

    #[test]
    fn missing_values_sort_last_in_both_directions() {
        let (one, two) = (json!(1), json!(2));
        for dir in [Direction::Asc, Direction::Desc] {
            assert_eq!(cmp_key(None, Some(&one), dir), Ordering::Greater);
            assert_eq!(cmp_key(Some(&one), None, dir), Ordering::Less);
        }
        assert_eq!(
            cmp_key(Some(&one), Some(&two), Direction::Desc),
            Ordering::Greater
        );
    }
}

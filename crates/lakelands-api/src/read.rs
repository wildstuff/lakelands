//! Reads: the time axes every read takes, the shapes of `find` and
//! `get`, and the reply every answer is wrapped in.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::expr::Condition;
use crate::id::{Cursor, FieldName, Position, RecordKey};
use crate::record::{Record, Value};
use crate::time::Timestamp;

/// The header every HTTP transport puts the reply's position in.
pub const POSITION_HEADER: &str = "Lakelands-Position";

/// Every answer the store gives: the data and the position it reflects
/// (`docs/decisions/009-every-response-carries-its-position.md`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reply<T> {
    /// The log coordinate the answer reflects.
    pub position: Position,
    /// The answer.
    pub data: T,
}

/// The two time axes of a read. Absent means the head of the log and
/// now; the two combine and neither stands in for the other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadAt {
    /// Transaction time: read the lake as it was at this position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<Position>,
    /// Business time: read what was true in the world at this instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_at: Option<Timestamp>,
}

/// Sort direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Smallest first.
    #[default]
    Asc,
    /// Largest first.
    Desc,
}

/// What of a field a sort key orders by: its value, or when it was
/// learned or observed (`docs/decisions/010-provenance-per-field.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderBy {
    /// The field's value.
    #[default]
    Value,
    /// The field's `known_at`.
    KnownAt,
    /// The field's `seen_at`.
    SeenAt,
}

/// One sort key. Records without a value for it sort last in either
/// direction; the record key breaks ties.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    /// The field sorted on.
    pub field: FieldName,
    /// What of the field orders: its value unless stated.
    #[serde(default, skip_serializing_if = "is_value")]
    pub by: OrderBy,
    /// The direction.
    #[serde(default)]
    pub dir: Direction,
}

fn is_value(by: &OrderBy) -> bool {
    *by == OrderBy::Value
}

/// An aggregate function over a set of records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggFunc {
    /// How many records.
    Count,
    /// The sum of a field; exact for decimals.
    Sum,
    /// The smallest value of a field.
    Min,
    /// The largest value of a field.
    Max,
    /// The mean of a field.
    Avg,
}

/// One aggregate an answer carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggMeasure {
    /// The name the result is reported under.
    pub name: FieldName,
    /// The function.
    pub func: AggFunc,
    /// The field it folds (none for `count`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<FieldName>,
}

/// An aggregation: measures, optionally per group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aggregate {
    /// The measures.
    pub measures: Vec<AggMeasure>,
    /// What forms one group each: fields' values, or named expressions';
    /// empty is one group.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_by: Vec<GroupKey>,
}

/// One key of an aggregate's grouping. A field name groups by that
/// field's value, as it is written: `"partner"`. A named expression
/// groups by its value per record, answered under `name` in the group's
/// `by`: `{"name": "month", "expr": "date_bucket(due_on, 'month')"}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum GroupKey {
    /// A field's value.
    Field(FieldName),
    /// An expression's value, under a name of its own.
    Named {
        /// The name the group's value is answered under.
        name: FieldName,
        /// The expression, evaluated per record as a `where` is.
        expr: crate::expr::Condition,
    },
}

impl GroupKey {
    /// The name the key's value is answered under.
    pub fn name(&self) -> &FieldName {
        match self {
            GroupKey::Field(f) => f,
            GroupKey::Named { name, .. } => name,
        }
    }
}

impl From<&str> for GroupKey {
    fn from(field: &str) -> GroupKey {
        GroupKey::Field(FieldName::from(field))
    }
}

impl From<FieldName> for GroupKey {
    fn from(field: FieldName) -> GroupKey {
        GroupKey::Field(field)
    }
}

/// `find`: records of one type, filtered, ordered and paged, or
/// aggregated. It subsumes listing, counting, summing, ranges and
/// grouping; a view type is found like any other type.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Find {
    /// Which records, in the one expression grammar.
    #[serde(default, rename = "where", skip_serializing_if = "Option::is_none")]
    pub filter: Option<Condition>,
    /// Sort keys, most significant first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order_by: Vec<Order>,
    /// At most this many rows; the lake's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Continue after an earlier page. The cursor carries the position
    /// its first page was read at, and the read is pinned to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Continue strictly after a sort position the caller names: the
    /// position a cursor carries, said in the open. The read is pinned by
    /// `at` as any `find` is, so without `as_of` it reads the head. Given
    /// with `cursor` or `aggregate`, it is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<After>,
    /// Aggregate instead of listing records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<Aggregate>,
    /// Return each field's provenance with its value.
    #[serde(default)]
    pub with_meta: bool,
    /// The time axes.
    #[serde(default, flatten)]
    pub at: ReadAt,
}

/// A sort position in `find`'s one order: the row that sorts at `values`
/// under `order_by`, with key `key`
/// (`docs/decisions/029-every-answer-has-one-order.md`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct After {
    /// The sort keys' values, in `order_by` order; `null` where the row
    /// has no value for a key.
    pub values: Vec<Value>,
    /// The row's key, which breaks ties.
    pub key: RecordKey,
}

/// The answer to `find`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Found {
    /// A page of records.
    Records {
        /// The records.
        rows: Vec<Record>,
        /// Where the next page starts, if there is one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_cursor: Option<Cursor>,
    },
    /// Aggregated groups.
    Groups {
        /// One row per group.
        rows: Vec<Group>,
    },
}

/// One group of an aggregation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    /// The grouping fields' values.
    pub by: BTreeMap<FieldName, Value>,
    /// The measures' results.
    pub values: BTreeMap<FieldName, Value>,
}

/// `get`: one record by key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Get {
    /// The record's key.
    pub key: RecordKey,
    /// Return each field's provenance with its value.
    #[serde(default)]
    pub with_meta: bool,
    /// The time axes.
    #[serde(default, flatten)]
    pub at: ReadAt,
}

/// How much of one embedding field is embedded, as
/// `LakeStore::vector_coverage` answers it at the head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VectorCoverage {
    /// The field.
    pub field: FieldName,
    /// Live records whose field holds a vector.
    pub embedded: u64,
    /// Live records whose field holds no vector although its `embed.from`
    /// fields hold text: never embedded, failed, or waiting for a retry.
    pub due: u64,
}

#[cfg(test)]
mod group_key_tests {
    use super::GroupKey;

    #[test]
    fn a_group_key_reads_either_form_and_refuses_an_unknown_member() {
        let field: GroupKey = serde_json::from_str(r#""partner""#).unwrap();
        assert_eq!(field, GroupKey::from("partner"));
        let named: GroupKey =
            serde_json::from_str(r#"{"name": "m", "expr": "date_bucket(on, 'month')"}"#).unwrap();
        assert_eq!(named.name().as_str(), "m");
        let extra =
            serde_json::from_str::<GroupKey>(r#"{"name": "m", "expr": "x", "unit": "day"}"#);
        assert!(extra.is_err(), "an unknown member is refused: {extra:?}");
    }
}

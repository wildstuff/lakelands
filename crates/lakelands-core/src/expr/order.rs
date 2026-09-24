//! The one sort order over values, shared by the store and every engine.
//!
//! Values compare as the evaluator compares them (`super::eval::compare`):
//! numbers, and text that reads as a number, numerically; instants, and
//! text that reads as one, as instants; other text as text. A sort needs a
//! total order, and the evaluator's comparison has none across those kinds
//! (`9 < '10'` and `'10' < 'abc'` as text, yet `9` and `'abc'` do not
//! compare). So values are ranked by kind first — numbers, then instants,
//! then text, then booleans (`false` first), then JSON values by their
//! canonical bytes — and compared by the evaluator within a kind. Every
//! engine that orders a scan orders by this function, so a page set is the
//! same on every engine.

use std::cmp::Ordering;

use lakelands_api::Timestamp;
use lakelands_api::expr::CmpOp;
use serde_json::Value;

use super::eval::{Val, compare, from_json};

/// A value's rank among the kinds a sort orders.
fn rank(v: &Val) -> u8 {
    // The evaluator orders two values exactly when they are of one
    // comparable kind, so its answer against a probe of a kind says
    // whether the value reads as that kind.
    let orders_with = |probe: &Val| {
        compare(CmpOp::Le, v, probe).unwrap_or(false)
            || compare(CmpOp::Gt, v, probe).unwrap_or(false)
    };
    match v {
        Val::Bool(_) => 3,
        Val::Json(_) | Val::Null => 4,
        Val::Duration(_) => 5,
        _ if orders_with(&from_json(&Value::from(0))) => 0,
        _ if orders_with(&Val::Time(Timestamp(0))) => 1,
        _ => 2,
    }
}

/// Two present values, in ascending order.
pub fn cmp_values(a: &Value, b: &Value) -> Ordering {
    let (x, y) = (from_json(a), from_json(b));
    let (rx, ry) = (rank(&x), rank(&y));
    if rx != ry {
        return rx.cmp(&ry);
    }
    let holds = |op| compare(op, &x, &y).unwrap_or(false);
    if holds(CmpOp::Lt) {
        Ordering::Less
    } else if holds(CmpOp::Gt) {
        Ordering::Greater
    } else if holds(CmpOp::Eq) || rx < 3 {
        Ordering::Equal
    } else {
        // Booleans and JSON values have no order of their own; their
        // canonical bytes give one (`false` before `true`).
        crate::canonical::to_vec(a).cmp(&crate::canonical::to_vec(b))
    }
}

//! The `expr` function backing: the one [`FunctionBacking`] of `0.1.0`.
//!
//! A function backed by `expr` is an expression in the one grammar over
//! its declared inputs. It is deterministic, so the fold computes a field
//! a function computes in process, with the same evaluator, and a replay
//! recomputes it; nothing is recorded (`docs/seams.md` §`FunctionBacking`,
//! `docs/decisions/022-functions-are-declarations.md`).
//!
//! [`ExprBacking`] is that backing as the seam presents it, for a host or
//! a door that calls a function directly: the call's `config` carries the
//! expression (`{"expr": …}`, in the typed or the text form), its `inputs`
//! the values by input name, and the answer is the value the expression
//! computes, numbers as exact decimal strings.

use std::collections::BTreeMap;

use async_trait::async_trait;
use lakelands_api::error::{self, Case};
use lakelands_api::expr::Condition;
use lakelands_api::seam::{FunctionBacking, FunctionCall};
use lakelands_api::{FieldName, Result, Timestamp, Value};
use lakelands_core::expr::{self, Place, RecordScope};
use lakelands_core::invalid;

/// The `expr` backing.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExprBacking;

/// The backing kind functions name to be computed by an expression.
pub const EXPR: &str = "expr";

impl ExprBacking {
    /// One call, computed at once: the expression in `config.expr` over
    /// the `inputs` object. `now()` reads `now`, as it reads the event's
    /// `known_at` in the fold.
    pub fn compute(call: &FunctionCall, now: Timestamp) -> Result<Value> {
        let on = [error::Place::Function {
            name: call.function.clone(),
        }];
        let wrong = |case: Case| invalid(crate::declare::at(&on, case));
        let condition: Condition = serde_json::from_value(call.config["expr"].clone())
            .map_err(|e| wrong(Case::ExprConfig { fault: (&e).into() }))?;
        let e = crate::declare::condition(&condition, Place::Record, &on)?;
        let inputs: BTreeMap<FieldName, Value> = match &call.inputs {
            Value::Object(m) => m
                .iter()
                .map(|(k, v)| (FieldName::from(k.as_str()), v.clone()))
                .collect(),
            Value::Null => BTreeMap::new(),
            other => {
                return Err(wrong(Case::InputsNotObject {
                    value: other.clone(),
                }));
            }
        };
        let scope = RecordScope {
            fields: &inputs,
            meta: None,
            settings: None,
            now,
        };
        let out = expr::eval(&e, &scope).map_err(wrong)?;
        Ok(expr::aggregate::to_value(&out))
    }
}

#[async_trait]
impl FunctionBacking for ExprBacking {
    fn kind(&self) -> &str {
        EXPR
    }

    async fn call(&self, call: &FunctionCall) -> Result<Value> {
        ExprBacking::compute(call, crate::lake::wall_clock())
    }
}

#[cfg(test)]
mod tests;

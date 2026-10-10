//! The schema hash: one digest that identifies a type's shape.
//!
//! The hashed object is specified member by member in
//! `docs/log-format.md` §2.2, so that any implementation computes the
//! same digest; this module is that specification in code. It is
//! built from an allowlist: a member of [`TypeDecl`] that is not named
//! here never reaches the hash, so a new meaning mark cannot shift the
//! hash of every existing type by accident.
//!
//! Order carries meaning only where it changes a record: fields and
//! relations (and a relation's own fields) are sorted by name, so
//! reordering a declaration is not a change of shape; the key keeps its
//! declared order, because the order of the key fields is part of the
//! key. A view's `group_by` and `measures` keep their declared order too:
//! the grouping order is the order of the view key, and a measure may use
//! an earlier one.
//!
//! A host that keeps declarations in force computes this hash of its own
//! declaration and compares it with the one `describe_type` reports
//! before it declares again, since declaring again is a new version. The
//! store computes the hash with this same function, so the two agree.

use std::fmt;

use serde_json::{Value, json};

use crate::canonical;
use crate::error::{Place, Said};
use crate::expr::{Condition, ParseError};
use crate::id::{FieldName, TypeName};
use crate::ontology::{FieldDecl, Measure, RelationDecl, TypeDecl, ViewDecl};

/// A condition inside a view that does not parse, so its typed form (the
/// form the schema hash covers) does not exist.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewConditionError {
    /// Where the condition sits, outermost first: the `where`, a measure,
    /// either inside a merged source.
    pub at: Vec<Place>,
    /// Why it does not parse.
    pub error: ParseError,
}

/// Its parse case, said where the condition sits.
impl From<ViewConditionError> for Said {
    fn from(e: ViewConditionError) -> Said {
        let case = e.error.case;
        Said { at: e.at, case }
    }
}

impl fmt::Display for ViewConditionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Said::from(self.clone()).fmt(f)
    }
}

impl std::error::Error for ViewConditionError {}

/// The schema hash of a type: SHA-256 over the canonical bytes of its
/// shape object, as 64 lower-case hex digits.
///
/// Fails only when a condition inside the type's view does not parse,
/// because a view is hashed in its typed form.
pub fn schema_hash(ty: &TypeDecl) -> Result<String, ViewConditionError> {
    Ok(canonical::sha256_hex(&shape(ty)?))
}

/// The object the schema hash is taken over. Its key is the one the
/// type's records carry, a view's `group_by` whether or not its
/// declaration writes a `key`, so the two forms of one view hash alike.
pub fn shape(ty: &TypeDecl) -> Result<Value, ViewConditionError> {
    let view = match &ty.view {
        Some(view) => view_shape(view)?,
        None => Value::Null,
    };
    Ok(json!({
        "key": ty.key_fields(),
        "fields": fields(&ty.fields),
        "relations": relations(&ty.relations),
        "view": view,
    }))
}

/// A view's shape: its source, its conditions in typed form, its
/// grouping and its measures.
pub fn view_shape(view: &ViewDecl) -> Result<Value, ViewConditionError> {
    let mut shape = source_shape(
        &view.source,
        view.r#where.as_ref(),
        &view.group_by,
        &view.measures,
        &[],
    )?;
    // Parameters and merged sources join the shape only when declared, so
    // a view without them keeps the hash it had before views could take
    // them.
    if !view.params.is_empty() {
        shape["params"] = json!(view.params);
    }
    if !view.merge.is_empty() {
        let merge = view
            .merge
            .iter()
            .map(|m| {
                let at = [Place::MergedSource {
                    name: m.source.clone(),
                }];
                source_shape(&m.source, m.r#where.as_ref(), &m.group_by, &m.measures, &at)
            })
            .collect::<Result<Vec<_>, ViewConditionError>>()?;
        shape["merge"] = json!(merge);
    }
    Ok(shape)
}

/// One source of a view: its type, its condition in typed form, its
/// grouping and its measures, each typed.
fn source_shape(
    source: &TypeName,
    filter: Option<&Condition>,
    group_by: &[FieldName],
    measures: &[Measure],
    at: &[Place],
) -> Result<Value, ViewConditionError> {
    let within = |place| [at, &[place]].concat();
    let filter = match filter {
        Some(c) => typed(c, || within(Place::Where))?,
        None => Value::Null,
    };
    let measures = measures
        .iter()
        .map(|m| {
            let name = m.name.clone();
            let expr = typed(&m.expr, || within(Place::Measure { name }))?;
            Ok(json!({ "name": m.name, "expr": expr }))
        })
        .collect::<Result<Vec<_>, ViewConditionError>>()?;
    Ok(json!({
        "source": source,
        "where": filter,
        "group_by": group_by,
        "measures": measures,
    }))
}

fn fields(list: &[FieldDecl]) -> Vec<Value> {
    let mut sorted: Vec<&FieldDecl> = list.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    sorted
        .into_iter()
        .map(|f| json!({ "name": f.name, "type": f.ty, "required": f.required }))
        .collect()
}

fn relations(list: &[RelationDecl]) -> Vec<Value> {
    let mut sorted: Vec<&RelationDecl> = list.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    sorted
        .into_iter()
        .map(|r| {
            json!({
                "name": r.name,
                "target": r.target,
                "cardinality": r.cardinality,
                "fields": fields(&r.fields),
            })
        })
        .collect()
}

/// A condition in its typed form, so that the same expression written as
/// text or as a typed tree hashes the same.
fn typed(c: &Condition, at: impl FnOnce() -> Vec<Place>) -> Result<Value, ViewConditionError> {
    let parsed;
    let expr = match c {
        Condition::Expr(e) => e,
        Condition::Text(text) => {
            parsed =
                crate::expr::parse(text).map_err(|error| ViewConditionError { at: at(), error })?;
            &parsed
        }
    };
    Ok(serde_json::to_value(expr).expect("an expression serialises to JSON"))
}

#[cfg(test)]
mod tests;

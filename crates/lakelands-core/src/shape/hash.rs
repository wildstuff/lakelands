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

use lakelands_api::expr::Condition;
use lakelands_api::ontology::{FieldDecl, RelationDecl, TypeDecl, ViewDecl};
use serde_json::{Value, json};

use super::ShapeError;
use crate::canonical;

/// The schema hash of a type: SHA-256 over the canonical bytes of its
/// shape object, as 64 lower-case hex digits.
///
/// Fails only when a condition inside the type's view does not parse,
/// because a view is hashed in its typed form.
pub fn schema_hash(ty: &TypeDecl) -> Result<String, ShapeError> {
    Ok(canonical::sha256_hex(&shape(ty)?))
}

/// The object the schema hash is taken over.
pub(super) fn shape(ty: &TypeDecl) -> Result<Value, ShapeError> {
    let view = match &ty.view {
        Some(view) => view_shape(view)?,
        None => Value::Null,
    };
    Ok(json!({
        "key": ty.key,
        "fields": fields(&ty.fields),
        "relations": relations(&ty.relations),
        "view": view,
    }))
}

/// A view's shape: its source, its conditions in typed form, its
/// grouping and its measures.
pub(super) fn view_shape(view: &ViewDecl) -> Result<Value, ShapeError> {
    let filter = match &view.r#where {
        Some(c) => typed(c, || "view.where".to_owned())?,
        None => Value::Null,
    };
    let measures = view
        .measures
        .iter()
        .map(|m| {
            let expr = typed(&m.expr, || format!("view.measures.{}", m.name))?;
            Ok(json!({ "name": m.name, "expr": expr }))
        })
        .collect::<Result<Vec<_>, ShapeError>>()?;
    Ok(json!({
        "source": view.source,
        "where": filter,
        "group_by": view.group_by,
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
fn typed(c: &Condition, at: impl FnOnce() -> String) -> Result<Value, ShapeError> {
    let parsed;
    let expr = match c {
        Condition::Expr(e) => e,
        Condition::Text(text) => {
            parsed = crate::expr::parse(text)
                .map_err(|error| ShapeError::Condition { at: at(), error })?;
            &parsed
        }
    };
    Ok(serde_json::to_value(expr).expect("an expression serialises to JSON"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lakelands_api::ontology::{Authority, Cardinality, FieldType, OnInputChange, StorageKind};

    /// A small type written the way a bundle would declare it.
    fn invoice() -> TypeDecl {
        serde_json::from_value(json!({
            "name": "invoice",
            "key": ["number"],
            "fields": [
                { "name": "number", "type": { "kind": "text" }, "required": true },
                { "name": "amount", "type": { "kind": "decimal", "value_type": "money" } },
                { "name": "customer", "type": { "kind": "reference", "target": "customer" } },
            ],
            "relations": [
                { "name": "billed_to", "target": "customer", "via": "customer" },
            ],
        }))
        .unwrap()
    }

    fn hash(ty: &TypeDecl) -> String {
        schema_hash(ty).unwrap()
    }

    fn with(edit: impl FnOnce(&mut TypeDecl)) -> TypeDecl {
        let mut ty = invoice();
        edit(&mut ty);
        ty
    }

    fn a_view(filter: Condition, total: Condition) -> TypeDecl {
        serde_json::from_value(json!({
            "name": "revenue",
            "view": {
                "source": "invoice",
                "where": filter,
                "group_by": ["customer"],
                "measures": [{ "name": "total", "expr": total }],
            },
        }))
        .unwrap()
    }

    /// Pinned once, and checked against an independent SHA-256 of the
    /// bytes in the next test: a change here is a change of the log format.
    #[test]
    fn the_hash_is_pinned_for_a_small_type() {
        assert_eq!(
            hash(&invoice()),
            "ce7fc1daa67c9853e3054fae92e9e188aa08039cc0ab8e460e3d73d9edd038d5"
        );
    }

    /// A view's conditions are hashed in their typed form, so the serde
    /// form of `Expr` is part of the log format; pinned for that reason.
    #[test]
    fn the_hash_is_pinned_for_a_view() {
        assert_eq!(
            hash(&a_view("status = 'open'".into(), "sum(amount)".into())),
            "39bc4bc1ad9646574531399af2cc2e01245a57989f4e61280ec94c5c2ec3918b"
        );
    }

    #[test]
    fn the_hashed_object_is_exactly_the_specified_one() {
        let bytes = canonical::to_vec(&shape(&invoice()).unwrap());
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            concat!(
                r#"{"fields":["#,
                r#"{"name":"amount","required":false,"type":{"kind":"decimal","value_type":"money"}},"#,
                r#"{"name":"customer","required":false,"type":{"kind":"reference","target":"customer"}},"#,
                r#"{"name":"number","required":true,"type":{"kind":"text"}}],"#,
                r#""key":["number"],"#,
                r#""relations":[{"cardinality":"many_to_one","fields":[],"name":"billed_to","target":"customer"}],"#,
                r#""view":null}"#,
            )
        );
    }

    #[test]
    fn all_four_members_are_always_present() {
        let empty: TypeDecl = serde_json::from_value(json!({ "name": "empty" })).unwrap();
        assert_eq!(
            canonical::to_vec(&shape(&empty).unwrap()),
            br#"{"fields":[],"key":[],"relations":[],"view":null}"#
        );
    }

    #[test]
    fn meaning_only_changes_leave_the_hash_alone() {
        let bare = hash(&invoice());
        let marked = with(|t| {
            t.description = Some("a bill we sent".into());
            t.label = Some("Invoice".into());
            t.valid_from = Some("issued".into());
            t.valid_to = Some("settled".into());
            t.field_meta = false;
            t.language = Some("de-CH".into());
            let f = &mut t.fields[1];
            f.description = Some("what is owed".into());
            f.label = Some("Amount".into());
            f.sensitive = true;
            f.searchable = true;
            f.authority = Authority::Source;
            f.derived = Some("1 + 1".into());
            f.depends_on = vec!["number".into()];
            f.on_input_change = OnInputChange::Clear;
            let r = &mut t.relations[0];
            r.via = None;
            r.inverse = Some("invoices".into());
            r.description = Some("who pays".into());
        });
        assert_eq!(bare, hash(&marked));
    }

    #[test]
    fn every_shape_change_changes_the_hash() {
        let bare = hash(&invoice());
        let changes: Vec<(&str, TypeDecl)> = vec![
            (
                "a field added",
                with(|t| {
                    let mut f = t.fields[0].clone();
                    f.name = "note".into();
                    f.required = false;
                    t.fields.push(f);
                }),
            ),
            ("a field removed", with(|t| t.fields.truncate(1))),
            (
                "a field made required",
                with(|t| t.fields[1].required = true),
            ),
            (
                "a storage kind changed",
                with(|t| t.fields[0].ty = FieldType::kind(StorageKind::Number)),
            ),
            (
                "a value type changed",
                with(|t| t.fields[1].ty.value_type = Some("price".into())),
            ),
            (
                "a reference target changed",
                with(|t| t.fields[2].ty.target = Some("partner".into())),
            ),
            (
                "a vector length set",
                with(|t| t.fields[1].ty.dimensions = Some(3)),
            ),
            ("the key changed", with(|t| t.key = vec!["customer".into()])),
            ("the key emptied", with(|t| t.key.clear())),
            (
                "a relation retargeted",
                with(|t| t.relations[0].target = "partner".into()),
            ),
            (
                "a cardinality changed",
                with(|t| t.relations[0].cardinality = Cardinality::ManyToMany),
            ),
            (
                "a link field added",
                with(|t| {
                    let f = t.fields[1].clone();
                    t.relations[0].fields.push(f);
                }),
            ),
            ("a relation removed", with(|t| t.relations.clear())),
        ];
        for (what, ty) in changes {
            assert_ne!(bare, hash(&ty), "{what} must change the hash");
        }
    }

    #[test]
    fn field_and_relation_order_is_not_shape() {
        let reordered = with(|t| {
            t.fields.reverse();
            let mut other = t.relations[0].clone();
            other.name = "also_billed_to".into();
            t.relations.push(other);
        });
        let other_order = with(|t| {
            let mut other = t.relations[0].clone();
            other.name = "also_billed_to".into();
            t.relations.insert(0, other);
        });
        assert_eq!(hash(&reordered), hash(&other_order));

        let links = |reverse: bool| {
            with(|t| {
                let mut fs = vec![t.fields[0].clone(), t.fields[1].clone()];
                if reverse {
                    fs.reverse();
                }
                t.relations[0].fields = fs;
            })
        };
        assert_eq!(hash(&links(false)), hash(&links(true)));
    }

    #[test]
    fn key_order_is_shape() {
        let ab = with(|t| t.key = vec!["number".into(), "customer".into()]);
        let ba = with(|t| t.key = vec!["customer".into(), "number".into()]);
        assert_ne!(hash(&ab), hash(&ba));
    }

    #[test]
    fn a_view_written_as_text_hashes_like_its_typed_form() {
        let text = a_view("amount > 0".into(), "sum(amount)".into());
        let typed = a_view(
            crate::expr::parse("amount > 0").unwrap().into(),
            crate::expr::parse("sum(amount)").unwrap().into(),
        );
        assert_eq!(hash(&text), hash(&typed));
        // Whitespace is not shape either: it does not survive the parse.
        let spaced = a_view("amount   >   0".into(), "sum( amount )".into());
        assert_eq!(hash(&text), hash(&spaced));
    }

    #[test]
    fn a_view_change_is_a_shape_change() {
        let bare = hash(&a_view("amount > 0".into(), "sum(amount)".into()));
        assert_ne!(
            bare,
            hash(&a_view("amount > 1".into(), "sum(amount)".into()))
        );
        assert_ne!(bare, hash(&a_view("amount > 0".into(), "count()".into())));
        let plain = with(|_| {});
        assert_ne!(hash(&plain), bare);
    }

    #[test]
    fn a_view_that_does_not_parse_has_no_hash() {
        let broken = a_view("amount >".into(), "sum(amount)".into());
        match schema_hash(&broken) {
            Err(ShapeError::Condition { at, .. }) => assert_eq!(at, "view.where"),
            other => panic!("expected a condition error, got {other:?}"),
        }
        let broken = a_view("amount > 0".into(), "sum(".into());
        match schema_hash(&broken) {
            Err(ShapeError::Condition { at, .. }) => assert_eq!(at, "view.measures.total"),
            other => panic!("expected a condition error, got {other:?}"),
        }
    }

    #[test]
    fn the_hash_is_64_lower_case_hex_digits_and_reproducible() {
        let h = hash(&invoice());
        assert_eq!(h.len(), 64);
        assert!(
            h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_eq!(h, hash(&invoice()));
    }
}

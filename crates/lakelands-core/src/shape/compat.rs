//! Declare or migrate: whether a revision of a type reshapes the records
//! already stored.
//!
//! The schema hash says only *whether* a shape changed. Before a new
//! declaration of a type enters the log, the store needs to know *how*:
//! a `declared` event must never reshape a stored record (log format
//! §2.2), so every revision under which some stored record could stop
//! matching its type goes through a migration, with its preflight, and
//! everything else is simply declared.
//!
//! The rule:
//!
//! - **Declared** — nothing stored has to change: a change of meaning
//!   only (descriptions, labels, authority, indexing marks); a new
//!   optional field; a required field made optional; a new relation; a
//!   relation whose cardinality only widens; a change to a view, which
//!   stores nothing.
//! - **Migrated** — a stored record could stop matching: a field or
//!   relation removed; a field's type changed (its kind, value type,
//!   reference target or vector length); an optional field made
//!   required; a required field added; the key changed, in its fields or
//!   their order; a relation retargeted or its cardinality narrowed; a
//!   stored type turned into a view. The same rules hold for the fields
//!   a relation carries on its links.
//!
//! A rename is not visible here: comparing two declarations, a renamed
//! field reads as one field removed and another added, which needs a
//! migration, and the migration names the rename (`Migration::renames`)
//! so that the values move instead of vanishing.

use std::collections::BTreeMap;
use std::fmt;

use lakelands_api::ontology::{Cardinality, FieldDecl, FieldType, RelationDecl, TypeDecl};
use lakelands_api::shape::view_shape;
use lakelands_api::{FieldName, RelationName, TypeName};
use serde::Serialize;

/// What a revision of a type needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum Change {
    /// The revision may be declared: no stored record is reshaped.
    Declare {
        /// The shape changes it makes, none of which reshapes a stored
        /// record. Empty when only meaning changed.
        changes: Vec<Reason>,
    },
    /// The revision needs a migration.
    Migrate {
        /// Every change that reshapes stored records. Never empty.
        reasons: Vec<Reason>,
    },
}

impl Change {
    /// Whether the revision needs a migration.
    pub fn needs_migration(&self) -> bool {
        matches!(self, Change::Migrate { .. })
    }
}

/// One difference between two declarations of a type. A field reason
/// names the relation when the field is one a relation carries on its
/// links.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Reason {
    /// A field was added. Reshapes when it is required: stored records
    /// do not carry it.
    FieldAdded {
        /// The field.
        field: FieldName,
        /// The relation whose links carry it, if any.
        #[serde(skip_serializing_if = "Option::is_none")]
        relation: Option<RelationName>,
        /// Whether it is required.
        required: bool,
    },
    /// A field was removed. Reshapes: its stored values would be lost.
    FieldRemoved {
        /// The field.
        field: FieldName,
        /// The relation whose links carry it, if any.
        #[serde(skip_serializing_if = "Option::is_none")]
        relation: Option<RelationName>,
    },
    /// A field's type changed. Reshapes: its stored values must convert.
    FieldRetyped {
        /// The field.
        field: FieldName,
        /// The relation whose links carry it, if any.
        #[serde(skip_serializing_if = "Option::is_none")]
        relation: Option<RelationName>,
        /// Its type before.
        from: FieldType,
        /// Its type after.
        to: FieldType,
    },
    /// An optional field became required. Reshapes: stored records
    /// without a value would no longer match.
    FieldRequired {
        /// The field.
        field: FieldName,
        /// The relation whose links carry it, if any.
        #[serde(skip_serializing_if = "Option::is_none")]
        relation: Option<RelationName>,
    },
    /// A required field became optional. Every stored record still
    /// matches.
    FieldOptional {
        /// The field.
        field: FieldName,
        /// The relation whose links carry it, if any.
        #[serde(skip_serializing_if = "Option::is_none")]
        relation: Option<RelationName>,
    },
    /// The key changed: its fields, or their order. Reshapes: every
    /// record would be identified differently.
    KeyChanged {
        /// The key before.
        from: Vec<FieldName>,
        /// The key after.
        to: Vec<FieldName>,
    },
    /// A relation was added.
    RelationAdded {
        /// The relation.
        relation: RelationName,
    },
    /// A relation was removed. Reshapes: its stored links would be lost.
    RelationRemoved {
        /// The relation.
        relation: RelationName,
    },
    /// A relation points at another type. Reshapes: its stored links
    /// name records of the old target.
    RelationRetargeted {
        /// The relation.
        relation: RelationName,
        /// The target before.
        from: TypeName,
        /// The target after.
        to: TypeName,
    },
    /// A relation's cardinality changed. Reshapes when a side that
    /// allowed many now allows one: stored links could exceed it.
    CardinalityChanged {
        /// The relation.
        relation: RelationName,
        /// The cardinality before.
        from: Cardinality,
        /// The cardinality after.
        to: Cardinality,
    },
    /// A view's source, filter, grouping or measures changed. A view
    /// stores nothing, so nothing is reshaped.
    ViewChanged,
    /// A stored type became a view. Reshapes: its stored records would
    /// no longer be readable as written.
    BecameView,
    /// A view became a stored type. It stored nothing, so nothing is
    /// reshaped.
    StoppedBeingView,
}

impl Reason {
    /// Whether this change, on a stored type, reshapes stored records.
    pub fn reshapes(&self) -> bool {
        match self {
            Reason::FieldAdded { required, .. } => *required,
            Reason::CardinalityChanged { from, to, .. } => !widens(*from, *to),
            Reason::FieldOptional { .. }
            | Reason::RelationAdded { .. }
            | Reason::ViewChanged
            | Reason::StoppedBeingView => false,
            Reason::FieldRemoved { .. }
            | Reason::FieldRetyped { .. }
            | Reason::FieldRequired { .. }
            | Reason::KeyChanged { .. }
            | Reason::RelationRemoved { .. }
            | Reason::RelationRetargeted { .. }
            | Reason::BecameView => true,
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let on = |relation: &Option<RelationName>| match relation {
            Some(r) => format!(" of relation `{r}`"),
            None => String::new(),
        };
        match self {
            Reason::FieldAdded {
                field,
                relation,
                required,
            } => {
                let which = if *required { "required" } else { "optional" };
                write!(f, "{which} field `{field}`{} added", on(relation))
            }
            Reason::FieldRemoved { field, relation } => {
                write!(f, "field `{field}`{} removed", on(relation))
            }
            Reason::FieldRetyped {
                field,
                relation,
                from,
                to,
            } => write!(
                f,
                "field `{field}`{} changes type from {} to {}",
                on(relation),
                describe(from),
                describe(to)
            ),
            Reason::FieldRequired { field, relation } => {
                write!(f, "field `{field}`{} becomes required", on(relation))
            }
            Reason::FieldOptional { field, relation } => {
                write!(f, "field `{field}`{} becomes optional", on(relation))
            }
            Reason::KeyChanged { from, to } => {
                write!(f, "key changes from {} to {}", list(from), list(to))
            }
            Reason::RelationAdded { relation } => write!(f, "relation `{relation}` added"),
            Reason::RelationRemoved { relation } => write!(f, "relation `{relation}` removed"),
            Reason::RelationRetargeted { relation, from, to } => {
                write!(
                    f,
                    "relation `{relation}` points at `{to}` instead of `{from}`"
                )
            }
            Reason::CardinalityChanged { relation, from, to } => write!(
                f,
                "relation `{relation}` changes cardinality from {} to {}",
                json_name(from),
                json_name(to)
            ),
            Reason::ViewChanged => f.write_str("the view changes"),
            Reason::BecameView => f.write_str("a stored type becomes a view"),
            Reason::StoppedBeingView => f.write_str("a view becomes a stored type"),
        }
    }
}

fn json_name<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(_) => String::from("?"),
    }
}

fn describe(ty: &FieldType) -> String {
    let mut s = json_name(&ty.kind);
    if let Some(v) = &ty.value_type {
        s.push_str(&format!(" ({v})"));
    }
    if let Some(t) = &ty.target {
        s.push_str(&format!(" of `{t}`"));
    }
    if let Some(d) = ty.dimensions {
        s.push_str(&format!("[{d}]"));
    }
    s
}

fn list(names: &[FieldName]) -> String {
    let inner: Vec<&str> = names.iter().map(FieldName::as_str).collect();
    format!("[{}]", inner.join(", "))
}

/// Compare two declarations of one type: may `after` be declared over
/// `before`, or does it need a migration?
///
/// Only shape is compared, exactly the members the schema hash covers,
/// so a revision whose schema hash is unchanged is always
/// `Declare { changes: [] }`. When both declarations are views nothing is
/// stored, and every change is declared.
pub fn classify(before: &TypeDecl, after: &TypeDecl) -> Change {
    let mut all = Vec::new();
    diff_fields(&before.fields, &after.fields, None, &mut all);
    if before.key != after.key {
        all.push(Reason::KeyChanged {
            from: before.key.clone(),
            to: after.key.clone(),
        });
    }
    diff_relations(&before.relations, &after.relations, &mut all);
    match (&before.view, &after.view) {
        (None, None) => {}
        (None, Some(_)) => all.push(Reason::BecameView),
        (Some(_), None) => all.push(Reason::StoppedBeingView),
        (Some(a), Some(b)) => {
            let same = match (view_shape(a), view_shape(b)) {
                (Ok(x), Ok(y)) => x == y,
                _ => a == b,
            };
            if !same {
                all.push(Reason::ViewChanged);
            }
        }
    }

    let stores_nothing = before.view.is_some() && after.view.is_some();
    let (reasons, changes): (Vec<Reason>, Vec<Reason>) = all
        .into_iter()
        .partition(|r| !stores_nothing && r.reshapes());
    if reasons.is_empty() {
        Change::Declare { changes }
    } else {
        Change::Migrate { reasons }
    }
}

fn by_name(fields: &[FieldDecl]) -> BTreeMap<&FieldName, &FieldDecl> {
    fields.iter().map(|f| (&f.name, f)).collect()
}

/// The field differences, in field-name order.
fn diff_fields(
    before: &[FieldDecl],
    after: &[FieldDecl],
    relation: Option<&RelationName>,
    out: &mut Vec<Reason>,
) {
    let old = by_name(before);
    let new = by_name(after);
    let relation = || relation.cloned();
    for (name, o) in &old {
        let Some(n) = new.get(name) else {
            out.push(Reason::FieldRemoved {
                field: (*name).clone(),
                relation: relation(),
            });
            continue;
        };
        if o.ty != n.ty {
            out.push(Reason::FieldRetyped {
                field: (*name).clone(),
                relation: relation(),
                from: o.ty.clone(),
                to: n.ty.clone(),
            });
        }
        match (o.required, n.required) {
            (false, true) => out.push(Reason::FieldRequired {
                field: (*name).clone(),
                relation: relation(),
            }),
            (true, false) => out.push(Reason::FieldOptional {
                field: (*name).clone(),
                relation: relation(),
            }),
            _ => {}
        }
    }
    for (name, n) in &new {
        if !old.contains_key(name) {
            out.push(Reason::FieldAdded {
                field: (*name).clone(),
                relation: relation(),
                required: n.required,
            });
        }
    }
}

/// The relation differences, in relation-name order.
fn diff_relations(before: &[RelationDecl], after: &[RelationDecl], out: &mut Vec<Reason>) {
    let old: BTreeMap<&RelationName, &RelationDecl> = before.iter().map(|r| (&r.name, r)).collect();
    let new: BTreeMap<&RelationName, &RelationDecl> = after.iter().map(|r| (&r.name, r)).collect();
    for (name, o) in &old {
        let Some(n) = new.get(name) else {
            out.push(Reason::RelationRemoved {
                relation: (*name).clone(),
            });
            continue;
        };
        if o.target != n.target {
            out.push(Reason::RelationRetargeted {
                relation: (*name).clone(),
                from: o.target.clone(),
                to: n.target.clone(),
            });
        }
        if o.cardinality != n.cardinality {
            out.push(Reason::CardinalityChanged {
                relation: (*name).clone(),
                from: o.cardinality,
                to: n.cardinality,
            });
        }
        diff_fields(&o.fields, &n.fields, Some(name), out);
    }
    for name in new.keys() {
        if !old.contains_key(name) {
            out.push(Reason::RelationAdded {
                relation: (*name).clone(),
            });
        }
    }
}

/// Whether each side of a relation may link many: (this type's side,
/// the target's side). `many_to_one` means many records of this type
/// link one target, so each record links at most one target.
fn sides(c: Cardinality) -> (bool, bool) {
    match c {
        Cardinality::OneToOne => (false, false),
        Cardinality::OneToMany => (false, true),
        Cardinality::ManyToOne => (true, false),
        Cardinality::ManyToMany => (true, true),
    }
}

/// Whether moving from `from` to `to` only allows more links: no side
/// that allowed many allows one afterwards.
fn widens(from: Cardinality, to: Cardinality) -> bool {
    let (a, b) = sides(from);
    let (c, d) = sides(to);
    (!a || c) && (!b || d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::schema_hash;
    use lakelands_api::ontology::{Authority, StorageKind};
    use serde_json::json;

    fn base() -> TypeDecl {
        serde_json::from_value(json!({
            "name": "t",
            "key": ["a"],
            "fields": [
                { "name": "a", "type": { "kind": "text" }, "required": true },
                { "name": "b", "type": { "kind": "number" }, "required": true },
                { "name": "c", "type": { "kind": "text" } },
            ],
        }))
        .unwrap()
    }

    fn field(name: &str, kind: StorageKind, required: bool) -> FieldDecl {
        serde_json::from_value(json!({
            "name": name,
            "type": FieldType::kind(kind),
            "required": required,
        }))
        .unwrap()
    }

    fn relation(name: &str, target: &str) -> RelationDecl {
        serde_json::from_value(json!({ "name": name, "target": target, "via": "a" })).unwrap()
    }

    fn with(edit: impl FnOnce(&mut TypeDecl)) -> TypeDecl {
        let mut t = base();
        edit(&mut t);
        t
    }

    fn declares(after: &TypeDecl) -> Vec<Reason> {
        match classify(&base(), after) {
            Change::Declare { changes } => changes,
            other => panic!("expected a declaration, got {other:?}"),
        }
    }

    fn migrates(before: &TypeDecl, after: &TypeDecl) -> Vec<Reason> {
        match classify(before, after) {
            Change::Migrate { reasons } => reasons,
            other => panic!("expected a migration, got {other:?}"),
        }
    }

    #[test]
    fn an_identical_revision_is_declared_with_no_changes() {
        assert_eq!(declares(&base()), vec![]);
    }

    #[test]
    fn a_change_of_meaning_is_declared_with_no_changes() {
        let after = with(|t| {
            t.description = Some("a thing".into());
            t.fields[0].label = Some("A".into());
            t.fields[1].authority = Authority::Derived;
            t.fields[2].searchable = true;
            t.fields[2].sensitive = true;
            t.fields.reverse();
        });
        assert_eq!(declares(&after), vec![]);
        assert_eq!(schema_hash(&base()), schema_hash(&after));
    }

    #[test]
    fn adding_an_optional_field_is_declared() {
        let after = with(|t| t.fields.push(field("d", StorageKind::DateTime, false)));
        assert_eq!(
            declares(&after),
            vec![Reason::FieldAdded {
                field: "d".into(),
                relation: None,
                required: false
            }]
        );
    }

    #[test]
    fn adding_a_required_field_needs_a_migration() {
        let after = with(|t| t.fields.push(field("d", StorageKind::DateTime, true)));
        assert_eq!(
            migrates(&base(), &after),
            vec![Reason::FieldAdded {
                field: "d".into(),
                relation: None,
                required: true
            }]
        );
    }

    #[test]
    fn adding_a_relation_is_declared() {
        let after = with(|t| t.relations.push(relation("links", "other")));
        assert_eq!(
            declares(&after),
            vec![Reason::RelationAdded {
                relation: "links".into()
            }]
        );
    }

    #[test]
    fn required_to_optional_is_declared() {
        let after = with(|t| t.fields[1].required = false);
        assert_eq!(
            declares(&after),
            vec![Reason::FieldOptional {
                field: "b".into(),
                relation: None
            }]
        );
    }

    #[test]
    fn optional_to_required_needs_a_migration() {
        let after = with(|t| t.fields[2].required = true);
        assert_eq!(
            migrates(&base(), &after),
            vec![Reason::FieldRequired {
                field: "c".into(),
                relation: None
            }]
        );
    }

    #[test]
    fn removing_a_field_needs_a_migration() {
        let after = with(|t| t.fields.retain(|f| f.name.as_str() != "b"));
        assert_eq!(
            migrates(&base(), &after),
            vec![Reason::FieldRemoved {
                field: "b".into(),
                relation: None
            }]
        );
    }

    #[test]
    fn every_part_of_a_field_type_is_a_retype() {
        let edits: [fn(&mut FieldType); 4] = [
            |ty| ty.kind = StorageKind::Decimal,
            |ty| ty.value_type = Some("email".into()),
            |ty| ty.target = Some("other".into()),
            |ty| ty.dimensions = Some(4),
        ];
        for edit in edits {
            let after = with(|t| edit(&mut t.fields[2].ty));
            let reasons = migrates(&base(), &after);
            assert!(
                matches!(&reasons[..], [Reason::FieldRetyped { field, .. }] if field.as_str() == "c"),
                "{reasons:?}"
            );
        }
    }

    #[test]
    fn a_key_change_needs_a_migration_and_key_order_counts() {
        let two = with(|t| t.key = vec!["a".into(), "b".into()]);
        let swapped = with(|t| t.key = vec!["b".into(), "a".into()]);
        assert!(matches!(
            &migrates(&base(), &two)[..],
            [Reason::KeyChanged { .. }]
        ));
        assert!(matches!(
            &migrates(&two, &swapped)[..],
            [Reason::KeyChanged { .. }]
        ));
    }

    #[test]
    fn removing_a_relation_needs_a_migration() {
        let before = with(|t| t.relations.push(relation("links", "other")));
        assert_eq!(
            migrates(&before, &base()),
            vec![Reason::RelationRemoved {
                relation: "links".into()
            }]
        );
    }

    #[test]
    fn retargeting_a_relation_needs_a_migration() {
        let before = with(|t| t.relations.push(relation("links", "other")));
        let after = with(|t| t.relations.push(relation("links", "elsewhere")));
        assert!(matches!(
            &migrates(&before, &after)[..],
            [Reason::RelationRetargeted { .. }]
        ));
    }

    #[test]
    fn a_widening_cardinality_is_declared_a_narrowing_one_migrated() {
        let at = |c: Cardinality| {
            with(|t| {
                let mut r = relation("links", "other");
                r.cardinality = c;
                t.relations.push(r);
            })
        };
        use Cardinality::*;
        for (from, to) in [
            (OneToOne, ManyToOne),
            (OneToOne, OneToMany),
            (ManyToOne, ManyToMany),
        ] {
            assert!(
                !classify(&at(from), &at(to)).needs_migration(),
                "{from:?} -> {to:?}"
            );
        }
        for (from, to) in [
            (ManyToMany, ManyToOne),
            (ManyToOne, OneToOne),
            (OneToMany, ManyToOne),
        ] {
            assert!(
                classify(&at(from), &at(to)).needs_migration(),
                "{from:?} -> {to:?}"
            );
        }
    }

    #[test]
    fn link_fields_follow_the_field_rules() {
        let before = with(|t| t.relations.push(relation("links", "other")));
        let add = |required: bool| {
            let mut t = before.clone();
            t.relations[0]
                .fields
                .push(field("since", StorageKind::DateTime, required));
            t
        };
        assert!(!classify(&before, &add(false)).needs_migration());
        assert_eq!(
            migrates(&before, &add(true)),
            vec![Reason::FieldAdded {
                field: "since".into(),
                relation: Some("links".into()),
                required: true
            }]
        );
        assert_eq!(
            migrates(&add(false), &before),
            vec![Reason::FieldRemoved {
                field: "since".into(),
                relation: Some("links".into())
            }]
        );
    }

    #[test]
    fn keeping_the_old_field_while_adding_its_successor_is_declared() {
        // Moving from a single `scheme` to a list `schemes`: declared while
        // `scheme` stays, a migration once it goes.
        let before = with(|t| t.fields.push(field("scheme", StorageKind::Text, false)));
        let kept = {
            let mut t = before.clone();
            t.fields.push(field("schemes", StorageKind::Json, false));
            t
        };
        assert!(!classify(&before, &kept).needs_migration());
        let dropped = {
            let mut t = kept.clone();
            t.fields.retain(|f| f.name.as_str() != "scheme");
            t
        };
        assert!(classify(&before, &dropped).needs_migration());
    }

    #[test]
    fn a_rename_reads_as_a_removal_and_an_addition() {
        let after = with(|t| t.fields[2].name = "c2".into());
        assert_eq!(
            migrates(&base(), &after),
            vec![Reason::FieldRemoved {
                field: "c".into(),
                relation: None
            }]
        );
    }

    #[test]
    fn a_migration_lists_every_reshaping_reason_and_only_those() {
        let after = with(|t| {
            t.fields.retain(|f| f.name.as_str() != "c");
            t.fields[1].ty = FieldType::kind(StorageKind::Decimal);
            t.fields.push(field("d", StorageKind::Text, false));
            t.key = vec!["b".into()];
        });
        let reasons = migrates(&base(), &after);
        assert_eq!(reasons.len(), 3, "{reasons:?}");
        assert!(reasons.iter().all(Reason::reshapes));
    }

    fn view(filter: &str) -> TypeDecl {
        serde_json::from_value(json!({
            "name": "v",
            "view": { "source": "t", "where": filter, "measures": [{ "name": "n", "expr": "count()" }] },
        }))
        .unwrap()
    }

    #[test]
    fn a_view_stores_nothing_so_its_changes_are_declared() {
        assert_eq!(
            classify(&view("b > 0"), &view("b > 1")),
            Change::Declare {
                changes: vec![Reason::ViewChanged]
            }
        );
        // Text and typed forms of the same view are the same view.
        let typed = {
            let mut v = view("b > 0");
            v.view.as_mut().unwrap().r#where = Some(crate::expr::parse("b > 0").unwrap().into());
            v
        };
        assert_eq!(
            classify(&view("b > 0"), &typed),
            Change::Declare { changes: vec![] }
        );
    }

    #[test]
    fn a_stored_type_turned_into_a_view_needs_a_migration() {
        let v = with(|t| t.view = view("b > 0").view);
        assert!(migrates(&base(), &v).contains(&Reason::BecameView));
        assert!(!classify(&v, &base()).needs_migration());
    }

    #[test]
    fn an_unchanged_hash_is_always_declared_without_changes() {
        let revisions = [
            with(|t| t.fields.reverse()),
            with(|t| t.fields[0].description = Some("x".into())),
            with(|t| t.relations.push(relation("r", "o"))),
            with(|t| t.fields[1].required = false),
        ];
        for after in revisions {
            let same = schema_hash(&base()) == schema_hash(&after);
            let empty = classify(&base(), &after) == Change::Declare { changes: vec![] };
            assert_eq!(same, empty, "{after:?}");
        }
    }

    #[test]
    fn reasons_read_as_sentences() {
        let r = Reason::FieldRetyped {
            field: "b".into(),
            relation: None,
            from: FieldType::kind(StorageKind::Number),
            to: FieldType::kind(StorageKind::Decimal),
        };
        assert_eq!(
            r.to_string(),
            "field `b` changes type from number to decimal"
        );
    }
}

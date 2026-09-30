//! Grants applied: what one principal may see of a record, what a read may
//! probe, and what a write may carry
//! (`docs/decisions/006-grants-not-roles.md`).
//!
//! Three rules, each enforced here and nowhere else:
//!
//! - **Reading.** A field the principal may read is returned in full. A
//!   field it may not read is absent — or, if the field is declared
//!   `sensitive` or the grant says `masked`, listed as masked so the reader
//!   knows a value exists without seeing it. A restricted record is
//!   withheld from a principal without the `restricted` grant; a retired
//!   record from everyone.
//! - **Probing.** A read that filters on a field, orders by it or
//!   aggregates over it learns something about its values even if the
//!   field is masked in the answer: a filter answers "is there a record
//!   whose salary is above X" record by record, and a sum over one record
//!   is that record's value. So a read that probes a field the principal
//!   may not read is refused outright, never answered with masking.
//! - **Writing.** A write names the door it comes through — authored
//!   (`upsert`, a verb) or source (`ingest`). Every field it carries must
//!   exist, be written by that door's authority, and be granted to the
//!   principal; one field that fails refuses the whole write
//!   (`docs/decisions/012-writes-touch-only-their-fields.md`).

use std::collections::BTreeMap;

use lakelands_api::ontology::{Authority, FieldDecl, TypeDecl};
use lakelands_api::record::StoredRecord;
use lakelands_api::{Access, Error, FieldName, Principal, Record, Refusal, TypeName, Value};

/// The prefix every system type's name carries
/// (`docs/decisions/003-reservation-by-namespace.md`).
pub const SYSTEM_PREFIX: &str = "sys.";

/// Whether a type name is reserved for system types.
pub fn is_system(ty: &TypeName) -> bool {
    ty.as_str().starts_with(SYSTEM_PREFIX)
}

/// One principal's access to one declared field: the grant, with a
/// `sensitive` field shown masked rather than dropped when the grant gives
/// no read.
pub fn field_access(who: &Principal, ty: &TypeName, field: &FieldDecl) -> Access {
    match who.grants.access(ty, &field.name) {
        Access::None if field.sensitive => Access::Masked,
        a => a,
    }
}

/// Whether the principal may retire records of a type: every field of the
/// type is writable as its grants resolve it, field by field, whatever the
/// field's authority. Retiring removes every field of a record, so a
/// principal who may write only some of them may not; one granted each
/// field one by one may. A type with no field falls back to its type-level
/// grant. A view's records are computed, and are never retired.
pub fn may_retire(who: &Principal, decl: &TypeDecl) -> bool {
    if decl.view.is_some() {
        return false;
    }
    if decl.fields.is_empty() {
        return type_access(who, &decl.name).writes();
    }
    decl.fields
        .iter()
        .all(|f| who.grants.access(&decl.name, &f.name).writes())
}

/// The principal's access to a type as a whole: its type-level grant, or
/// the lake-wide default.
pub fn type_access(who: &Principal, ty: &TypeName) -> Access {
    who.grants
        .types
        .get(ty)
        .and_then(|t| t.default)
        .unwrap_or(who.grants.default)
}

/// A stored record as one principal may see it; `None` when the record is
/// withheld from it (retired, or restricted without the grant).
pub fn visible(
    who: &Principal,
    decl: &TypeDecl,
    stored: &StoredRecord,
    with_meta: bool,
) -> Option<Record> {
    if stored.retired || (stored.restricted && !who.grants.restricted) {
        return None;
    }
    let mut fields = BTreeMap::new();
    let mut masked = Vec::new();
    let mut meta = BTreeMap::new();
    for f in &decl.fields {
        let Some(value) = stored.fields.get(&f.name).filter(|v| !v.is_null()) else {
            // A computed field whose function failed holds no value; its
            // metadata says why, to whoever may read the field. An
            // embedding with nothing to embed keeps metadata without an
            // error, which says so.
            if with_meta
                && let Some(m) = stored
                    .meta
                    .get(&f.name)
                    .filter(|m| m.error.is_some() || f.embed.is_some())
                && field_access(who, &decl.name, f).reads()
            {
                meta.insert(f.name.clone(), m.clone());
            }
            continue;
        };
        match field_access(who, &decl.name, f) {
            a if a.reads() => {
                fields.insert(f.name.clone(), value.clone());
                if with_meta && let Some(m) = stored.meta.get(&f.name) {
                    meta.insert(f.name.clone(), m.clone());
                }
            }
            Access::Masked => masked.push(f.name.clone()),
            _ => {}
        }
    }
    Some(Record {
        ty: stored.ty.clone(),
        key: stored.key.clone(),
        version: stored.version,
        fields,
        masked,
        meta,
    })
}

/// Refuse a read that probes a field the principal may not read, or a
/// field the type does not declare. `probed` are the fields a filter,
/// an ordering or an aggregate touches.
pub fn check_probe<'a>(
    who: &Principal,
    decl: &TypeDecl,
    probed: impl IntoIterator<Item = &'a FieldName>,
) -> Result<(), Error> {
    for name in probed {
        let Some(field) = decl.fields.iter().find(|f| &f.name == name) else {
            return Err(Error::Refused {
                field: Some(name.clone()),
                reason: Refusal::Invalid {
                    message: format!("`{}` has no field `{name}`", decl.name),
                },
            });
        };
        if !field_access(who, &decl.name, field).reads() {
            return Err(Error::Refused {
                field: Some(name.clone()),
                reason: Refusal::NotGranted,
            });
        }
    }
    Ok(())
}

/// Refuse a search the principal may not make: lexical search probes
/// every searchable field of the type at once, so each must be readable;
/// a vector search probes the vector field it names and, since a vector
/// embedded from text encodes that text, every field it is embedded from.
pub fn check_search(
    who: &Principal,
    decl: &TypeDecl,
    vector_field: Option<&FieldName>,
) -> Result<(), Error> {
    let searched = decl.fields.iter().filter(|f| f.searchable).map(|f| &f.name);
    let embedded_from = vector_field
        .and_then(|v| decl.fields.iter().find(|f| &f.name == v))
        .and_then(|f| f.embed.as_ref())
        .into_iter()
        .flat_map(|e| e.from.iter());
    check_probe(who, decl, searched.chain(vector_field).chain(embedded_from))
}

/// The door a write comes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Door {
    /// `upsert`, a verb, an accepted proposal.
    Authored,
    /// `ingest`.
    Source,
}

impl Door {
    /// Whether a field of this authority may be written through this door.
    pub fn admits(self, authority: Authority) -> bool {
        matches!(
            (self, authority),
            (_, Authority::Shared)
                | (Door::Authored, Authority::Authored)
                | (Door::Source, Authority::Source)
        )
    }
}

/// Refuse a write, whole, unless every field it carries exists, is written
/// through this door, and is granted to the principal. Fields are checked
/// in name order, so the refusal names the same field every time.
pub fn check_write(
    who: &Principal,
    decl: &TypeDecl,
    door: Door,
    patch: &BTreeMap<FieldName, Value>,
) -> Result<(), Error> {
    if door == Door::Source && !who.grants.ingest {
        return Err(Error::Refused {
            field: None,
            reason: Refusal::NotGranted,
        });
    }
    if decl.view.is_some() {
        return Err(Error::Refused {
            field: None,
            reason: Refusal::Invalid {
                message: format!("`{}` is a view; it is computed, never written", decl.name),
            },
        });
    }
    for name in patch.keys() {
        let Some(field) = decl.fields.iter().find(|f| &f.name == name) else {
            return Err(Error::Refused {
                field: Some(name.clone()),
                reason: Refusal::Invalid {
                    message: format!("`{}` has no field `{name}`", decl.name),
                },
            });
        };
        if !door.admits(field.authority) {
            return Err(Error::Refused {
                field: Some(name.clone()),
                reason: Refusal::WrongDoor,
            });
        }
        if !who.grants.access(&decl.name, name).writes() {
            return Err(Error::Refused {
                field: Some(name.clone()),
                reason: Refusal::NotGranted,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lakelands_api::ontology::{FieldType, StorageKind};
    use lakelands_api::principal::{Grants, PrincipalKind, TypeGrants};
    use lakelands_api::{FieldMeta, Origin, Position, Timestamp, Version};
    use serde_json::json;

    fn field(name: &str, authority: Authority, sensitive: bool) -> FieldDecl {
        FieldDecl {
            annotations: Default::default(),
            name: name.into(),
            ty: FieldType::kind(StorageKind::Text),
            required: false,
            authority,
            derived: None,
            function: None,
            depends_on: vec![],
            on_input_change: Default::default(),
            sensitive,
            searchable: false,
            description: None,
            label: None,
            embed: None,
        }
    }

    fn invoice() -> TypeDecl {
        TypeDecl {
            annotations: Default::default(),
            name: "invoice".into(),
            key: vec![],
            fields: vec![
                field("amount", Authority::Source, false),
                field("verdict", Authority::Authored, false),
                field("iban", Authority::Source, true),
                field("note", Authority::Shared, false),
                field("score", Authority::Derived, false),
            ],
            relations: vec![],
            view: None,
            valid_from: None,
            valid_to: None,
            field_meta: true,
            language: None,
            description: None,
            label: None,
        }
    }

    fn principal(default: Access, fields: &[(&str, Access)]) -> Principal {
        let mut grants = Grants {
            default,
            ..Grants::default()
        };
        grants.types.insert(
            "invoice".into(),
            TypeGrants {
                default: None,
                fields: fields
                    .iter()
                    .map(|(f, a)| (FieldName::from(*f), *a))
                    .collect(),
            },
        );
        Principal {
            kind: PrincipalKind::Agent,
            id: "a".into(),
            lake: "finance".into(),
            grants,
        }
    }

    fn stored() -> StoredRecord {
        let meta = FieldMeta {
            upstream: None,
            origin: Origin::Source,
            known_at: Timestamp(0),
            seen_at: None,
            position: Position(1),
            writer: "service:feed".into(),
            source: None,
            procedure_version: None,
            error: None,
        };
        StoredRecord {
            ty: "invoice".into(),
            key: "INV-1".into(),
            version: Version(2),
            fields: serde_json::from_value(json!({
                "amount": "10.00", "verdict": "ok", "iban": "DE00", "note": null
            }))
            .unwrap(),
            meta: [("amount".into(), meta.clone()), ("iban".into(), meta)].into(),
            position: Position(1),
            retired: false,
            restricted: false,
        }
    }

    #[test]
    fn a_reader_sees_granted_fields_and_a_sensitive_field_only_as_masked() {
        let who = principal(Access::None, &[("amount", Access::Read)]);
        let r = visible(&who, &invoice(), &stored(), true).unwrap();
        assert_eq!(r.fields.keys().collect::<Vec<_>>(), ["amount"]);
        assert_eq!(r.masked, [FieldName::from("iban")]);
        assert_eq!(
            r.meta.keys().collect::<Vec<_>>(),
            ["amount"],
            "no meta for a masked field"
        );
    }

    #[test]
    fn meta_comes_only_when_asked() {
        let who = principal(Access::Read, &[]);
        assert!(
            visible(&who, &invoice(), &stored(), false)
                .unwrap()
                .meta
                .is_empty()
        );
    }

    #[test]
    fn retired_and_restricted_records_are_withheld() {
        let who = principal(Access::Read, &[]);
        let mut s = stored();
        s.restricted = true;
        assert!(visible(&who, &invoice(), &s, false).is_none());
        let mut reviewer = who.clone();
        reviewer.grants.restricted = true;
        assert!(visible(&reviewer, &invoice(), &s, false).is_some());
        s.retired = true;
        assert!(visible(&reviewer, &invoice(), &s, false).is_none());
    }

    #[test]
    fn probing_an_unreadable_field_is_refused_not_masked() {
        let who = principal(Access::Read, &[("iban", Access::Masked)]);
        assert!(check_probe(&who, &invoice(), [&FieldName::from("amount")]).is_ok());
        assert_eq!(
            check_probe(&who, &invoice(), [&FieldName::from("iban")]),
            Err(Error::Refused {
                field: Some("iban".into()),
                reason: Refusal::NotGranted
            })
        );
        assert!(matches!(
            check_probe(&who, &invoice(), [&FieldName::from("nope")]),
            Err(Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            })
        ));
    }

    #[test]
    fn a_search_probes_every_searchable_field_and_the_vector_it_names() {
        let mut decl = invoice();
        decl.fields[2].searchable = true; // iban, sensitive
        let reader = principal(Access::Read, &[("iban", Access::Masked)]);
        assert_eq!(
            check_search(&reader, &decl, None),
            Err(Error::Refused {
                field: Some("iban".into()),
                reason: Refusal::NotGranted
            })
        );
        let granted = principal(Access::Read, &[]);
        assert!(check_search(&granted, &decl, None).is_ok());
        assert!(check_search(&granted, &decl, Some(&"nope".into())).is_err());
    }

    #[test]
    fn a_search_over_an_embedded_vector_probes_the_fields_it_is_embedded_from() {
        let mut decl = invoice();
        decl.fields.push(FieldDecl {
            embed: Some(lakelands_api::ontology::EmbedDecl {
                from: vec!["verdict".into(), "note".into()],
                model: "m".into(),
            }),
            ..field("vec", Authority::Derived, false)
        });
        let vec = FieldName::from("vec");
        let reader = principal(Access::Read, &[("note", Access::Masked)]);
        assert_eq!(
            check_search(&reader, &decl, Some(&vec)),
            Err(Error::Refused {
                field: Some("note".into()),
                reason: Refusal::NotGranted
            }),
            "the vector is readable, but it encodes `note`"
        );
        assert!(
            check_search(&reader, &decl, None).is_ok(),
            "no field is searchable"
        );
        assert!(check_search(&principal(Access::Read, &[]), &decl, Some(&vec)).is_ok());
    }

    #[test]
    fn a_write_is_refused_whole_by_its_first_failing_field() {
        let owner = principal(Access::Write, &[]);
        let patch = |v: serde_json::Value| -> BTreeMap<FieldName, Value> {
            serde_json::from_value(v).unwrap()
        };
        assert!(
            check_write(
                &owner,
                &invoice(),
                Door::Authored,
                &patch(json!({"verdict": "x", "note": "y"}))
            )
            .is_ok()
        );
        assert_eq!(
            check_write(
                &owner,
                &invoice(),
                Door::Authored,
                &patch(json!({"verdict": "x", "amount": "1"}))
            ),
            Err(Error::Refused {
                field: Some("amount".into()),
                reason: Refusal::WrongDoor
            })
        );
        assert_eq!(
            check_write(
                &owner,
                &invoice(),
                Door::Authored,
                &patch(json!({"score": 1}))
            ),
            Err(Error::Refused {
                field: Some("score".into()),
                reason: Refusal::WrongDoor
            })
        );
        let limited = principal(Access::Read, &[("note", Access::Write)]);
        assert_eq!(
            check_write(
                &limited,
                &invoice(),
                Door::Authored,
                &patch(json!({"note": "y", "verdict": "x"}))
            ),
            Err(Error::Refused {
                field: Some("verdict".into()),
                reason: Refusal::NotGranted
            })
        );
    }

    #[test]
    fn ingest_needs_the_ingest_grant_and_writes_source_and_shared_fields_only() {
        let mut feed = principal(Access::Write, &[]);
        let patch: BTreeMap<FieldName, Value> =
            serde_json::from_value(json!({"amount": "1", "note": "n"})).unwrap();
        assert_eq!(
            check_write(&feed, &invoice(), Door::Source, &patch),
            Err(Error::Refused {
                field: None,
                reason: Refusal::NotGranted
            })
        );
        feed.grants.ingest = true;
        assert!(check_write(&feed, &invoice(), Door::Source, &patch).is_ok());
        let authored: BTreeMap<FieldName, Value> =
            serde_json::from_value(json!({"verdict": "x"})).unwrap();
        assert!(matches!(
            check_write(&feed, &invoice(), Door::Source, &authored),
            Err(Error::Refused {
                reason: Refusal::WrongDoor,
                ..
            })
        ));
    }

    #[test]
    fn system_types_are_recognised_by_prefix_alone() {
        assert!(is_system(&"sys.setting".into()));
        assert!(is_system(&"sys.anything_new".into()));
        assert!(!is_system(&"system_note".into()));
        assert!(!is_system(&"a.sys.b".into()));
    }
}

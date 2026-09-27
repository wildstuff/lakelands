//! `describe`, assembled: the lake's declarations as one principal may see
//! them, on the six axes of `lakelands_api::describe` — structure, access,
//! rights, actions, lineage, lifecycle.
//!
//! The answer is built from the same rules that gate reads and writes
//! (`crate::rights`), so `describe` never promises access a read or a
//! write would then refuse, and the tools generated from it
//! (`lakelands_api::tools::tools`) never offer a door that is shut. What a
//! principal holds no grant for is absent, not listed as forbidden.

use lakelands_api::describe::{
    FieldDescription, LakeDescription, LakeRights, TypeAccess, TypeDescription, TypeLifecycle,
    TypeLineage, TypeRights, VerbDescription,
};
use lakelands_api::engine::Declared;
use lakelands_api::ontology::{Authority, TypeDecl, VerbDecl, VerbEffect};
use lakelands_api::write::Declaration;
use lakelands_api::{Access, LakeId, Principal, TypeName};

use crate::rights::{field_access, type_access};

/// The lake as `who` may see it.
pub fn lake(lake: &LakeId, declared: &[Declared], who: &Principal) -> LakeDescription {
    let types: Vec<TypeDescription> = declared
        .iter()
        .filter_map(|d| match &d.declaration {
            Declaration::Type(t) => type_description(t, d, declared, who),
            _ => None,
        })
        .collect();
    let verbs = declared
        .iter()
        .filter_map(|d| match &d.declaration {
            Declaration::Verb(v) => verb_description(v, declared, who),
            _ => None,
        })
        .collect();
    let value_types = declared
        .iter()
        .filter_map(|d| match &d.declaration {
            Declaration::ValueType(v) => Some(v.clone()),
            _ => None,
        })
        .collect();
    let triggers = declared
        .iter()
        .filter_map(|d| match &d.declaration {
            Declaration::Trigger(t) if types.iter().any(|ty| ty.name == t.on) => {
                Some(t.name.clone())
            }
            _ => None,
        })
        .collect();
    LakeDescription {
        lake: lake.clone(),
        types,
        verbs,
        value_types,
        triggers,
        may: LakeRights {
            review: who.grants.review,
            schema: who.grants.schema,
            ingest: who.grants.ingest,
        },
    }
}

/// One type as `who` may see it; `None` when it may see none of it.
pub fn one_type(ty: &TypeName, declared: &[Declared], who: &Principal) -> Option<TypeDescription> {
    declared.iter().find_map(|d| match &d.declaration {
        Declaration::Type(t) if &t.name == ty => type_description(t, d, declared, who),
        _ => None,
    })
}

fn find_type<'a>(declared: &'a [Declared], ty: &TypeName) -> Option<&'a TypeDecl> {
    declared.iter().find_map(|d| match &d.declaration {
        Declaration::Type(t) if &t.name == ty => Some(t),
        _ => None,
    })
}

fn type_description(
    t: &TypeDecl,
    d: &Declared,
    declared: &[Declared],
    who: &Principal,
) -> Option<TypeDescription> {
    let fields: Vec<FieldDescription> = t
        .fields
        .iter()
        .map(|f| (f, field_access(who, &t.name, f)))
        .filter(|(_, a)| *a != Access::None)
        .map(|(f, a)| FieldDescription::of(f, a))
        .collect();
    let whole = type_access(who, &t.name);
    if fields.is_empty() && whole == Access::None {
        return None;
    }
    let reads = fields.iter().any(|f| f.access.reads()) || whole.reads();
    let is_view = t.view.is_some();
    // A link type's records are written through its relation's link
    // doors, never through `upsert` or `retire`.
    let is_link = crate::links::is_link_type(&t.name);
    let may = TypeRights {
        find: reads,
        get: reads,
        upsert: !is_view && !is_link && fields.iter().any(FieldDescription::authored_writable),
        retire: !is_link && crate::rights::may_retire(who, t),
        link: t
            .relations
            .iter()
            .filter(|r| {
                crate::links::carried_by_links(r)
                    && r.authority == Authority::Authored
                    && find_type(declared, &r.target).is_some()
            })
            .filter(|r| {
                let lt = crate::links::link_type(&t.name, r);
                lt.fields
                    .iter()
                    .filter(|f| {
                        f.name.as_str() == crate::links::FROM || f.name.as_str() == crate::links::TO
                    })
                    .all(|f| field_access(who, &lt.name, f).writes())
            })
            .map(|r| r.name.clone())
            .collect(),
    };
    let verbs = declared
        .iter()
        .filter_map(|d| match &d.declaration {
            Declaration::Verb(v) if v.on == t.name && verb_callable(v, t, who) => {
                Some(v.name.clone())
            }
            _ => None,
        })
        .collect();
    Some(TypeDescription {
        name: t.name.clone(),
        description: t.description.clone(),
        relations: t
            .relations
            .iter()
            .filter(|r| find_type(declared, &r.target).is_some())
            .cloned()
            .collect(),
        access: TypeAccess {
            key: t.key.clone(),
            valid_from: t.valid_from.clone(),
            valid_to: t.valid_to.clone(),
            searchable: t.fields.iter().any(|f| f.searchable),
            field_meta: t.field_meta,
        },
        may,
        verbs,
        lineage: TypeLineage {
            sourced: t
                .fields
                .iter()
                .any(|f| matches!(f.authority, Authority::Source | Authority::Shared)),
            view_of: t.view.as_ref().map(|v| v.source.clone()),
        },
        lifecycle: TypeLifecycle {
            version: d.version,
            schema_hash: d.schema_hash.clone().unwrap_or_default(),
            sorted: vec![],
        },
        fields,
    })
}

/// A verb is callable when every field its effect writes is granted to the
/// principal, or — for a verb that reaches outside the lake — when the
/// principal may write the type as a whole.
fn verb_callable(v: &VerbDecl, t: &TypeDecl, who: &Principal) -> bool {
    match &v.effect {
        VerbEffect::Write { set } => {
            !set.is_empty()
                && set
                    .iter()
                    .all(|a| who.grants.access(&t.name, &a.field).writes())
        }
        VerbEffect::External { .. } => type_access(who, &t.name).writes(),
    }
}

fn verb_description(
    v: &VerbDecl,
    declared: &[Declared],
    who: &Principal,
) -> Option<VerbDescription> {
    let t = find_type(declared, &v.on)?;
    if !verb_callable(v, t, who) {
        return None;
    }
    Some(VerbDescription {
        name: v.name.clone(),
        on: v.on.clone(),
        // The payload is the caller's own input: every field of it is its to
        // write.
        payload: v
            .payload
            .iter()
            .map(|f| FieldDescription::of(f, Access::Write))
            .collect(),
        risk: v.risk,
        external: matches!(v.effect, VerbEffect::External { .. }),
        description: v.description.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lakelands_api::ontology::{Assignment, FieldDecl, FieldType, Risk, StorageKind};
    use lakelands_api::principal::{Grants, PrincipalKind, TypeGrants};
    use lakelands_api::tools::{Door, tools};
    use lakelands_api::{FieldName, Position};

    fn field(name: &str, authority: Authority) -> FieldDecl {
        FieldDecl {
            name: name.into(),
            ty: FieldType::kind(StorageKind::Text),
            required: false,
            authority,
            derived: None,
            function: None,
            depends_on: vec![],
            on_input_change: Default::default(),
            sensitive: false,
            searchable: name == "note",
            description: None,
            label: None,
            embed: None,
        }
    }

    fn declared() -> Vec<Declared> {
        let invoice = TypeDecl {
            name: "invoice".into(),
            key: vec![],
            fields: vec![
                field("amount", Authority::Source),
                field("verdict", Authority::Authored),
                field("note", Authority::Authored),
            ],
            relations: vec![],
            view: None,
            valid_from: None,
            valid_to: None,
            field_meta: true,
            language: None,
            description: Some("A bill".into()),
            label: None,
        };
        let secret = TypeDecl {
            name: "salary".into(),
            fields: vec![field("amount", Authority::Authored)],
            ..invoice.clone()
        };
        let settle = VerbDecl {
            name: "settle".into(),
            on: "invoice".into(),
            payload: vec![],
            risk: Risk::Low,
            requires: None,
            effect: VerbEffect::Write {
                set: vec![Assignment {
                    field: "verdict".into(),
                    value: "'settled'".into(),
                }],
            },
            description: None,
        };
        [
            Declaration::Type(invoice),
            Declaration::Type(secret),
            Declaration::Verb(settle),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, declaration)| Declared {
            declaration,
            version: 1,
            schema_hash: Some(format!("h{i}")),
            since: Position(i as u64),
        })
        .collect()
    }

    fn who(fields: &[(&str, Access)]) -> Principal {
        let mut grants = Grants::default();
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

    #[test]
    fn what_a_principal_holds_no_grant_for_is_absent() {
        let d = lake(
            &"finance".into(),
            &declared(),
            &who(&[("amount", Access::Read)]),
        );
        assert_eq!(d.types.len(), 1, "salary is not listed at all");
        let t = &d.types[0];
        assert_eq!(
            t.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["amount"]
        );
        assert!(t.may.find && t.may.get && !t.may.upsert && !t.may.retire);
        assert!(d.verbs.is_empty() && t.verbs.is_empty());
    }

    #[test]
    fn a_writer_of_the_verbs_fields_may_call_it() {
        let d = lake(
            &"finance".into(),
            &declared(),
            &who(&[("verdict", Access::Write)]),
        );
        assert_eq!(d.verbs.len(), 1);
        assert_eq!(
            d.types[0].verbs,
            ["settle".into()] as [lakelands_api::VerbName; 1]
        );
        assert!(d.types[0].may.upsert);
    }

    #[test]
    fn the_owner_sees_everything_and_the_tools_follow() {
        let mut owner = who(&[]);
        owner.grants = Grants::owner();
        let d = lake(&"finance".into(), &declared(), &owner);
        assert_eq!(d.types.len(), 2);
        assert!(d.types.iter().all(|t| t.may.retire));
        assert_eq!(d.types[0].lifecycle.schema_hash, "h0");
        assert!(d.types[0].access.searchable);
        let doors: Vec<Door> = tools(&d).unwrap().into_iter().map(|t| t.door).collect();
        assert!(doors.contains(&Door::Retire {
            ty: "salary".into()
        }));
        assert!(doors.contains(&Door::Call {
            verb: "settle".into()
        }));
    }

    #[test]
    fn describe_type_is_the_same_answer_for_one_type() {
        let reader = who(&[("amount", Access::Read)]);
        let d = lake(&"finance".into(), &declared(), &reader);
        assert_eq!(
            one_type(&"invoice".into(), &declared(), &reader).as_ref(),
            d.types.first()
        );
        assert!(one_type(&"salary".into(), &declared(), &reader).is_none());
    }
}

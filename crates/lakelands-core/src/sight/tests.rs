use super::*;
use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind};
use lakelands_api::principal::{Grants, PrincipalKind, TypeGrants};
use lakelands_api::{Access, Position};

/// `owner`, `invoice` with a reference to it, and `note`.
fn declared() -> Vec<Declared> {
    let field = |name: &str, target: Option<&str>| FieldDecl {
        name: name.into(),
        ty: FieldType {
            target: target.map(Into::into),
            ..FieldType::kind(match target {
                Some(_) => StorageKind::Reference,
                None => StorageKind::Text,
            })
        },
        ..Default::default()
    };
    let ty = |name: &str, fields| Declared {
        declaration: Declaration::Type(TypeDecl {
            name: name.into(),
            fields,
            ..Default::default()
        }),
        version: 1,
        schema_hash: None,
        since: Position::GENESIS,
    };
    vec![
        ty("owner", vec![field("name", None)]),
        ty(
            "invoice",
            vec![field("by", Some("owner")), field("n", None)],
        ),
        ty("note", vec![field("text", None)]),
    ]
}

/// One who reads the lake but `owner`, and `invoice.was` once renamed to `n`.
fn who() -> Principal {
    let mut grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let none = TypeGrants {
        default: Some(Access::None),
        ..Default::default()
    };
    grants.types.insert("owner".into(), none);
    let was = [("was".into(), Access::None)].into();
    let held = TypeGrants {
        fields: was,
        ..Default::default()
    };
    grants.types.insert("invoice".into(), held);
    Principal {
        kind: PrincipalKind::Agent,
        id: "a".into(),
        lake: "finance".into(),
        grants,
    }
}

#[test]
fn a_type_not_shown_is_answered_as_a_name_that_is_not_declared() {
    let declared = declared();
    let sight = Sight::new(who(), &[], None, &declared);
    let said = |name: &str| format!("{:?}", sight.ty(&name.into())).replace(name, "x");
    assert_eq!(said("owner"), said("ghost"));
    // Asked again, it is answered the same.
    assert!(sight.ty(&"owner".into()).is_err() && sight.ty(&"note".into()).is_ok());
    assert!(sight.ty(&"note".into()).is_ok());
}

#[test]
fn a_sight_is_made_with_its_grants_followed_and_its_references_hidden() {
    let declared = declared();
    let renamed = Renamed {
        ty: "invoice".into(),
        to: None,
        renames: vec![("was".into(), "n".into())],
        at: Position(3),
    };
    let sight = Sight::new(who(), &[renamed], None, &declared);
    let invoice = sight.ty(&"invoice".into()).expect("shown");
    let access = |f: &str| {
        let field = invoice.fields.iter().find(|x| x.name.as_str() == f);
        rights::field_access(sight.who(), invoice, field.expect("declared"))
    };
    // The grant on the name it bore holds the field, and the reference to
    // a type it is not shown is a field it may not know of.
    assert_eq!((access("n"), access("by")), (Access::None, Access::None));
    assert_eq!(sight.hidden(), [("invoice".into(), "by".into())]);
}

#[test]
fn a_relation_and_a_verb_are_judged_by_the_sight() {
    use lakelands_api::ontology::Assignment;
    let mut declared = declared();
    let Declaration::Type(invoice) = &mut declared[1].declaration else {
        unreachable!("the second declaration is `invoice`");
    };
    invoice.relations.push(RelationDecl {
        name: "owned_by".into(),
        target: "owner".into(),
        via: Some("by".into()),
        ..Default::default()
    });
    let invoice = invoice.clone();
    let verb = |set: Vec<Assignment>| VerbDecl {
        name: "mark".into(),
        on: "invoice".into(),
        scope: Default::default(),
        payload: vec![],
        risk: lakelands_api::ontology::Risk::Low,
        preconditions: Vec::new(),
        requires: None,
        effect: VerbEffect::Write { set },
        description: None,
        granted: false,
    };
    let n = Assignment {
        field: "n".into(),
        value: "'x'".into(),
    };
    let mut writer = who();
    writer.grants = Grants::owner();
    for (who, shown) in [(who(), false), (writer, true)] {
        let sight = Sight::new(who, &[], None, &declared);
        assert_eq!(sight.relation(&invoice, &invoice.relations[0]), shown);
        assert_eq!(sight.callable(&invoice, &verb(vec![n.clone()])), shown);
        // A verb that writes nothing is no one's to call.
        assert!(!sight.callable(&invoice, &verb(vec![])));
    }
}

#[test]
fn one_who_holds_only_neither_authors_a_shape_nor_sees_every_record() {
    let declared = declared();
    let only = TypeGrants {
        only: [("n".into(), "x".into())].into(),
        ..Default::default()
    };
    let judged = |schema: bool, restricted: bool, holds_only: bool| {
        let mut who = who();
        (who.grants.schema, who.grants.restricted) = (schema, restricted);
        if holds_only {
            who.grants.types.insert("note".into(), only.clone());
        }
        let sight = Sight::held(&who, &declared);
        (sight.authors(), sight.sees_all())
    };
    assert_eq!(judged(true, false, false), (true, false));
    assert_eq!(judged(false, true, false), (false, true));
    assert_eq!(judged(true, true, false), (true, true));
    assert_eq!(judged(true, true, true), (false, false));
    assert_eq!(judged(false, false, false), (false, false));
}

#[test]
fn one_who_holds_restricted_is_shown_restricted_records_its_only_aside() {
    let declared = declared();
    let only = TypeGrants {
        only: [("n".into(), "x".into())].into(),
        ..Default::default()
    };
    let judged = |restricted: bool, holds_only: bool| {
        let mut who = who();
        who.grants.restricted = restricted;
        if holds_only {
            who.grants.types.insert("note".into(), only.clone());
        }
        let sight = Sight::held(&who, &declared);
        (sight.sees_restricted(), sight.sees_all())
    };
    assert_eq!(judged(true, false), (true, true));
    assert_eq!(judged(true, true), (true, false));
    assert_eq!(judged(false, false), (false, false));
}

#[test]
fn a_retire_a_pin_and_an_only_are_judged_of_the_sights_principal() {
    let declared = declared();
    let note = declared.ty(&"note".into()).expect("declared").clone();
    let mut derived = note.clone();
    derived.fields[0].authority = lakelands_api::ontology::Authority::Derived;
    let (reader, mut owner) = (who(), who());
    owner.grants = Grants::owner();
    let mut holder = owner.clone();
    let only = TypeGrants {
        only: [("text".into(), "x".into())].into(),
        ..Default::default()
    };
    holder.grants.types.insert("note".into(), only);
    let judged = |who: &Principal| {
        let sight = Sight::held(who, &declared);
        let pins = (sight.pinnable(&note, None), sight.pinnable(&derived, None));
        (sight.may_retire(&note), sight.holds_only(), pins)
    };
    assert_eq!(judged(&reader), (false, false, (true, true)));
    assert_eq!(judged(&owner), (true, false, (true, true)));
    // A pair on a computed field would move off itself at the accept.
    assert_eq!(judged(&holder), (true, true, (true, false)));
}

#[test]
fn a_migration_is_known_as_the_sights_principal_reads_it() {
    let declared = declared();
    let to = declared.ty(&"invoice".into()).expect("declared").clone();
    let mut was = to.clone();
    was.fields[1].name = "was".into();
    let mut m = Migration::to(to);
    m.renames.push(("was".into(), "n".into()));
    let (reader, mut owner) = (who(), who());
    owner.grants = Grants::owner();
    let find = |n: &TypeName| declared.ty(n);
    let judged = |who: &Principal| {
        let sight = Sight::held(who, &declared);
        let mut known = m.clone();
        sight.known_migration(&was, &mut known, &find);
        let mut report = MigrationReport {
            ty: "invoice".into(),
            from_schema_hash: "whole".into(),
            ..Default::default()
        };
        sight.shown_hashes(&was, &m, &mut report, &find);
        let (reads, whole) = (sight.reads_both(&was, &m, &"n".into()), "whole");
        (reads, known.renames, report.from_schema_hash == whole)
    };
    // The reader may not know `n` by the name it had.
    assert_eq!(judged(&reader), (false, vec![], false));
    assert_eq!(judged(&owner), (true, m.renames.clone(), true));
}

#[test]
fn a_report_is_shown_through_the_sights_own_lookup_and_description() {
    use lakelands_api::write::{Carried, Rekeyed};
    let declared = declared();
    let sight = Sight::new(who(), &[], None, &declared);
    let invoice = declared.ty(&"invoice".into()).expect("declared").clone();
    let rekeyed = |ty: &str| Rekeyed {
        ty: ty.into(),
        records: 1,
        collisions: 0,
        missing: 0,
    };
    let carried = |name: &str| Carried {
        kind: "type".into(),
        name: name.into(),
    };
    let mut report = MigrationReport {
        ty: "invoice".into(),
        rekeyed: vec![rekeyed("owner"), rekeyed("note")],
        carried: vec![carried("owner"), carried("note")],
        ..Default::default()
    };
    sight.shown_report(&invoice, &Migration::to(invoice.clone()), &mut report);
    // `owner`, which it is not shown, is neither found nor described.
    assert_eq!(report.rekeyed, [rekeyed("note")]);
    assert_eq!(report.carried, [carried("note")]);
    assert!(report.more);
}

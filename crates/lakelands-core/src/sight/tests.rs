use super::*;
use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, ViewDecl};
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
fn a_report_is_shown_only_as_its_principal_may_know_it() {
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
    let m = Migration::to(invoice.clone());
    let described = || sight.describe(&who().lake);
    sight.shown_report(&invoice, &m, &mut report, described, |n| sight.ty(n).ok());
    // `owner`, which it is not shown, is neither found nor described.
    assert_eq!(report.rekeyed, [rekeyed("note")]);
    assert_eq!(report.carried, [carried("note")]);
    assert!(report.more);
}

#[test]
fn a_read_names_searches_and_describes_only_what_its_principal_may_know() {
    let (declared, who) = (declared(), who());
    let sight = Sight::held(&who, &declared);
    let ty = |n: &str| declared.ty(&n.into()).expect("declared").clone();
    let (owner, mut invoice) = (ty("owner"), ty("invoice"));
    // It may not know of `owner.name`, and knows `invoice.n`.
    assert!(sight.check_known(&invoice, &"n".into()).is_ok());
    assert!(sight.check_known(&owner, &"name".into()).is_err());
    // Text weights name some searchable fields it reads.
    let weights = |f: &str| [(f.into(), 1.0)].into();
    let none = BTreeMap::new();
    assert!(sight.check_text_weights(&invoice, &none).is_err());
    assert!(sight.check_text_weights(&invoice, &weights("n")).is_err());
    invoice.fields[1].searchable = true;
    assert!(sight.check_text_weights(&invoice, &weights("n")).is_ok());
    // `owner` is described to it without a field it may not know of.
    let mut described = owner.clone();
    sight.described_fields(&mut described, &|n| declared.ty(n));
    assert!(described.fields.is_empty() && !owner.fields.is_empty());
}

#[test]
fn an_only_and_a_verb_are_judged_for_a_read_by_the_sight() {
    let mut declared = declared();
    let view = ViewDecl {
        source: "note".into(),
        params: Vec::new(),
        r#where: None,
        group_by: Vec::new(),
        measures: Vec::new(),
        merge: Vec::new(),
    };
    let (by_note, note) = (TypeName::from("by_note"), TypeName::from("note"));
    let Declaration::Type(t) = declared[2].declaration.clone() else {
        unreachable!("the third declaration is `note`");
    };
    let mut d = declared[2].clone();
    d.declaration = Declaration::Type(TypeDecl {
        name: by_note.clone(),
        view: Some(Box::new(view)),
        ..t.clone()
    });
    declared.push(d);
    let verb = VerbDecl {
        name: "mark".into(),
        on: note.clone(),
        scope: Default::default(),
        payload: vec![],
        risk: lakelands_api::ontology::Risk::Low,
        preconditions: Vec::new(),
        requires: Some("text = 'x'".into()),
        effect: VerbEffect::Retire,
        description: None,
        granted: true,
    };
    let only = |f: &str| TypeGrants {
        only: [(f.into(), "x".into())].into(),
        ..Default::default()
    };
    let find = |n: &TypeName| declared.ty(n);
    let judged = |grant: Option<TypeGrants>| {
        let mut who = who();
        who.grants.types.extend(grant.map(|g| (note.clone(), g)));
        let sight = Sight::held(&who, &declared);
        let reads = sight.reads_what_verb_reads(&t, &verb, &find);
        let held = sight.check_only(&t).is_ok();
        let only = (held, sight.computed_only(&by_note, &find));
        (only, sight.verb_granted(&verb), reads)
    };
    assert_eq!(judged(None), ((true, false), false, true));
    // The view of a type held to some records is computed for it.
    assert_eq!(judged(Some(only("text"))), ((true, true), false, true));
    // An `only` on a field `note` does not declare cannot be applied.
    assert_eq!(judged(Some(only("ghost"))).0, (false, true));
    let mut named = TypeGrants {
        verbs: ["mark".into()].into(),
        ..Default::default()
    };
    assert_eq!(judged(Some(named.clone())), ((true, false), true, true));
    // One kept from the field the verb reads may not call it.
    named.fields.insert("text".into(), Access::None);
    assert!(!judged(Some(named)).2);
}

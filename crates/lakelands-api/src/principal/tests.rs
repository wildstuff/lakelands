use super::*;

#[test]
fn a_field_grant_beats_the_type_default_which_beats_the_lake_default() {
    let invoice = TypeName::from("invoice");
    let mut g = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    g.types.insert(
        invoice.clone(),
        TypeGrants {
            default: Some(Access::Masked),
            fields: [(FieldName::from("verdict"), Access::Write)].into(),
            ..TypeGrants::default()
        },
    );
    assert_eq!(g.access(&invoice, &"verdict".into()), Access::Write);
    assert_eq!(g.access(&invoice, &"amount".into()), Access::Masked);
    assert_eq!(g.access(&"partner".into(), &"name".into()), Access::Read);
}

#[test]
fn reading_everything_needs_every_grant_to_read_and_restricted() {
    assert!(Grants::owner().reads_everything());
    let reader = Grants {
        default: Access::Read,
        restricted: true,
        ..Grants::default()
    };
    assert!(
        reader.reads_everything(),
        "read is enough; write is not needed"
    );
    let narrowed = |default: Option<Access>, field: Option<Access>| {
        let mut g = reader.clone();
        g.types.insert(
            "invoice".into(),
            TypeGrants {
                default,
                fields: field.map(|a| ("amount".into(), a)).into_iter().collect(),
                ..TypeGrants::default()
            },
        );
        g
    };
    assert!(narrowed(Some(Access::Write), Some(Access::Read)).reads_everything());
    assert!(!narrowed(Some(Access::Masked), None).reads_everything());
    assert!(!narrowed(None, Some(Access::Masked)).reads_everything());
    assert!(!narrowed(None, Some(Access::None)).reads_everything());
    assert!(
        !Grants {
            restricted: false,
            ..reader.clone()
        }
        .reads_everything(),
        "withheld records are part of the log"
    );
    assert!(
        !Grants {
            default: Access::Masked,
            ..reader.clone()
        }
        .reads_everything()
    );
    assert!(!Grants::default().reads_everything());
    let mut only = reader.clone();
    let owner = TypeGrants {
        only: [("owner".into(), "alice".into())].into(),
        ..TypeGrants::default()
    };
    only.types.insert("claim".into(), owner);
    assert!(
        !only.reads_everything(),
        "the feed is every owner's records"
    );
}

#[test]
fn only_follows_renames_and_two_names_that_differ_admit_nothing() {
    let entry = |at, ty: &str, renames: &[(&str, &str)], to: Option<&str>| Renamed {
        at: Position(at),
        ty: ty.into(),
        renames: renames
            .iter()
            .map(|(a, b)| ((*a).into(), (*b).into()))
            .collect(),
        to: to.map(Into::into),
    };
    let only = |pairs: &[(&str, &str)]| TypeGrants {
        only: pairs
            .iter()
            .map(|(f, v)| ((*f).into(), (*v).into()))
            .collect(),
        ..TypeGrants::default()
    };
    let mut g = Grants::default();
    g.types.insert("claim".into(), only(&[("holder", "alice")]));
    g.types.insert("expense".into(), only(&[("owner", "bob")]));
    let list = [
        entry(3, "claim", &[("owner", "holder")], None),
        entry(5, "expense", &[], Some("claim")),
    ];
    let at = |p| {
        let mut g = g.clone();
        g.follow(&list, p);
        g.types
            .get(&TypeName::from("claim"))
            .map(|t| t.only.clone())
    };
    let said = |pairs: &[(&str, crate::Value)]| {
        Some(
            pairs
                .iter()
                .map(|(f, v)| ((*f).into(), v.clone()))
                .collect(),
        )
    };
    assert_eq!(at(Some(Position(2))), said(&[("owner", "alice".into())]));
    assert_eq!(at(Some(Position(4))), said(&[("holder", "alice".into())]));
    // The expense type became `claim`: its grant and claim's unite, and
    // `owner` is still a name of the same field, so two values meet on it.
    let mut both = at(None).expect("claim");
    assert_eq!(
        both.remove(&FieldName::from("holder")),
        Some(crate::Value::Null)
    );
}

#[test]
fn nothing_is_granted_by_default() {
    let g = Grants::default();
    assert_eq!(g.access(&"invoice".into(), &"amount".into()), Access::None);
    assert!(!g.review && !g.schema && !g.ingest && !g.restricted);
}

#[test]
fn a_verb_grant_stays_with_the_type_name_it_is_given_under() {
    let renamed = Renamed {
        at: Position(4),
        ty: "invoice".into(),
        renames: Vec::new(),
        to: Some("bill".into()),
    };
    let verbs = |names: &[&str]| TypeGrants {
        verbs: names.iter().map(|n| (*n).into()).collect(),
        ..TypeGrants::default()
    };
    let mut g = Grants::default();
    g.types.insert("invoice".into(), verbs(&["pay"]));
    g.types.insert("bill".into(), verbs(&["send"]));
    let at = |p, ty: &str| {
        let mut g = g.clone();
        g.follow(std::slice::from_ref(&renamed), p);
        let held = g.types.get(&TypeName::from(ty)).map(|t| t.verbs.clone());
        held.unwrap_or_default()
            .into_iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
    };
    // Before the rename the type is `invoice`, after it `bill`: each name's
    // verbs are its own, never the other's.
    assert_eq!(at(Some(Position(3)), "invoice"), ["pay"]);
    assert_eq!(at(None, "bill"), ["send"]);
}

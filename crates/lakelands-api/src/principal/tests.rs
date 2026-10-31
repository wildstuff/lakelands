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
    let mut set = reader.clone();
    let owners = TypeGrants {
        only_any: [("owner".into(), ["alice".to_owned()].into())].into(),
        ..TypeGrants::default()
    };
    set.types.insert("claim".into(), owners);
    assert!(
        !set.reads_everything(),
        "the feed is every owner's records, outside a set too"
    );
    let mut hiding = reader.clone();
    let hidden = HiddenWhere {
        field: "doc".into(),
        values: ["secret".to_owned()].into(),
        fields: [("body".into(), Access::None)].into(),
    };
    let chunk = TypeGrants {
        hide_where: vec![hidden],
        ..TypeGrants::default()
    };
    hiding.types.insert("chunk".into(), chunk);
    assert!(
        !hiding.reads_everything(),
        "the feed is every record's every field"
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

#[test]
fn a_set_follows_renames_and_two_names_admit_only_what_both_sets_hold() {
    let list = [
        Renamed {
            at: Position(3),
            ty: "claim".into(),
            renames: vec![("owner".into(), "holder".into())],
            to: None,
        },
        Renamed {
            at: Position(5),
            ty: "expense".into(),
            renames: Vec::new(),
            to: Some("claim".into()),
        },
    ];
    let set = |f: &str, values: &[&str], whole| TypeGrants {
        only_any: [(f.into(), values.iter().map(|v| (*v).to_owned()).collect())].into(),
        whole_sets_only: whole,
        ..TypeGrants::default()
    };
    let mut g = Grants::default();
    g.types
        .insert("claim".into(), set("holder", &["alice", "bob"], false));
    g.types
        .insert("expense".into(), set("owner", &["bob", "carol"], true));
    let at = |p| {
        let mut g = g.clone();
        g.follow(&list, p);
        let t = g
            .types
            .get(&TypeName::from("claim"))
            .cloned()
            .unwrap_or_default();
        let any = t.only_any.into_iter();
        let any: Vec<(String, Vec<String>)> = any
            .map(|(f, v)| (f.to_string(), v.into_iter().collect()))
            .collect();
        (any, t.whole_sets_only)
    };
    let said = |f: &str, v: &[&str], whole| {
        let v = v.iter().map(|v| (*v).to_owned()).collect();
        (vec![(f.to_owned(), v)], whole)
    };
    assert_eq!(
        at(Some(Position(2))),
        said("owner", &["alice", "bob"], false)
    );
    assert_eq!(
        at(Some(Position(4))),
        said("holder", &["alice", "bob"], false)
    );
    // The expense type became `claim`: a record is admitted only where
    // both grants admit it, and either name asking for whole sets holds.
    assert_eq!(at(None), said("holder", &["bob"], true));
}

#[test]
fn dating_by_a_hidden_valid_time_holds_only_where_every_name_of_the_type_says_it() {
    let (to, at) = (Some("offer".into()), Position(4));
    let renamed = [Renamed {
        at,
        ty: "quote".into(),
        renames: Vec::new(),
        to,
    }];
    let at = |quote: Option<bool>, offer: bool| {
        let mut g = Grants::default();
        let dated = |yes| TypeGrants {
            dates_by_hidden_valid_time: yes,
            ..TypeGrants::default()
        };
        (quote.map(dated)).map(|q| g.types.insert("quote".into(), q));
        g.types.insert("offer".into(), dated(offer));
        g.follow(&renamed, None);
        g.types[&TypeName::from("offer")].dates_by_hidden_valid_time
    };
    // A grant under another name the type bore only takes away.
    assert!(at(None, true) && at(Some(true), true));
    assert!(!at(Some(false), true) && !at(Some(true), false));
}

#[test]
fn a_grant_refuses_a_key_it_does_not_know_at_every_level() {
    let refused = |grants: serde_json::Value, key: &str| {
        let said = serde_json::from_value::<Grants>(grants).unwrap_err();
        assert!(said.to_string().contains(key), "{said}");
    };
    let hiding = serde_json::json!({"field": "a", "values": ["x"], "fields": {"b": "none"}});
    let mut extra = hiding.clone();
    extra["notes"] = "no".into();
    refused(serde_json::json!({"typez": {}}), "typez");
    refused(serde_json::json!({"types": {"t": {"onyl": {}}}}), "onyl");
    refused(
        serde_json::json!({"types": {"t": {"hide_where": [extra]}}}),
        "notes",
    );
    // What every key of the three types spells stays readable.
    let whole = serde_json::json!({"default": "read", "review": true, "schema": true,
        "ingest": true, "restricted": true, "types": {"t": {"default": "read",
        "fields": {"a": "read"}, "only": {"a": "x"}, "only_any": {"b": ["x"]},
        "writes_only": {"a": "x"}, "verbs": ["v"], "hide_where": [hiding],
        "dates_by_hidden_valid_time": true, "whole_sets_only": true}}});
    let read: Grants = serde_json::from_value(whole.clone()).unwrap();
    assert_eq!(serde_json::to_value(&read).unwrap(), whole);
}

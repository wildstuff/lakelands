use super::*;
use lakelands_api::engine::Declared;
use lakelands_api::ontology::{RelationDecl, StorageKind};
use lakelands_api::principal::{Grants, PrincipalKind, TypeGrants};
use lakelands_api::{FieldMeta, Origin, Position, Timestamp, Version};
use serde_json::json;

fn field(name: &str, authority: Authority, sensitive: bool) -> FieldDecl {
    FieldDecl {
        authority,
        sensitive,
        ..FieldDecl::of(name, StorageKind::Text)
    }
}

fn invoice() -> TypeDecl {
    TypeDecl {
        fields: vec![
            field("amount", Authority::Source, false),
            field("verdict", Authority::Authored, false),
            field("iban", Authority::Source, true),
            field("note", Authority::Shared, false),
            field("score", Authority::Derived, false),
        ],
        field_meta: true,
        ..TypeDecl::named("invoice")
    }
}

fn principal(default: Access, fields: &[(&str, Access)]) -> Principal {
    let fields = fields.iter().map(|(f, a)| (FieldName::from(*f), *a));
    let mut grants = Grants {
        default,
        ..Grants::default()
    };
    grants.types.insert(
        "invoice".into(),
        TypeGrants {
            default: None,
            fields: fields.collect(),
            ..Default::default()
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
        procedure: None,
        procedure_version: None,
        error: None,
        erased: false,
        on_behalf_of: None,
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
        made: Position(1),
        retired: false,
        restricted: false,
    }
}

/// The sight of `who`, as it stands, over no declarations.
fn held(who: &Principal) -> Sight<'_> {
    Sight::held(who, const { &Vec::<Declared>::new() })
}

/// A refusal naming `field`, for `reason`.
fn refused_on<T>(field: &str, reason: Refusal) -> Result<T, Error> {
    Err(Error::Refused {
        field: Some(field.into()),
        reason,
    })
}

#[test]
fn a_reader_sees_granted_fields_and_a_sensitive_field_only_as_masked() {
    let who = principal(Access::None, &[("amount", Access::Read)]);
    let r = held(&who).visible(&invoice(), &stored(), true).unwrap();
    assert_eq!(r.fields.keys().collect::<Vec<_>>(), ["amount"]);
    assert_eq!(r.masked, [FieldName::from("iban")]);
    assert_eq!(
        r.meta.keys().collect::<Vec<_>>(),
        ["amount"],
        "no meta for a masked field"
    );
}

#[test]
fn probing_an_unreadable_field_is_refused_not_masked() {
    let who = principal(Access::Read, &[("iban", Access::Masked)]);
    let probe = |f: &str| held(&who).check_probe(&invoice(), [&FieldName::from(f)]);
    assert!(probe("amount").is_ok());
    assert_eq!(probe("iban"), refused_on("iban", Refusal::NotGranted));
    assert!(matches!(
        probe("nope"),
        Err(Error::Refused {
            reason: Refusal::Invalid { .. },
            ..
        })
    ));
}

#[test]
fn a_search_probes_every_searchable_field_it_reads_and_the_vector_it_names() {
    let mut decl = invoice();
    decl.fields[2].searchable = true; // iban, sensitive
    let reader = principal(Access::Read, &[("iban", Access::Masked)]);
    let search = |who, v: &[&FieldName]| held(who).check_search(&decl, None, v);
    assert!(search(&reader, &[]).is_ok(), "`iban` is not searched");
    assert_eq!(held(&reader).searched_fields(&decl).count(), 0);
    let granted = principal(Access::Read, &[]);
    assert!(search(&granted, &[]).is_ok());
    assert!(search(&granted, &[&"nope".into()]).is_err());
}

#[test]
fn a_search_limited_to_named_fields_probes_those_and_judges_reading_first() {
    let mut decl = invoice();
    decl.fields[2].searchable = true; // iban, sensitive
    decl.fields[3].searchable = true; // note
    let reader = principal(Access::Read, &[("iban", Access::Masked)]);
    let not_granted = |field: &str| refused_on(field, Refusal::NotGranted);
    let invalid =
        |field: &str, case: Case| refused_on(field, Refusal::Invalid(Box::new(case.into())));
    let search = |decl: &TypeDecl, named: &[&str]| {
        let named: Vec<FieldName> = named.iter().map(|&n| n.into()).collect();
        held(&reader).check_search(decl, Some(&named), &[])
    };
    let ty = || decl.name.clone();
    assert!(search(&decl, &["note"]).is_ok(), "`iban` is not named");
    assert_eq!(search(&decl, &["note", "iban"]), not_granted("iban"));
    let (ty, field) = (ty(), "verdict".into());
    let unsearchable = invalid("verdict", Case::NotSearchable { field, ty });
    assert_eq!(search(&decl, &["verdict"]), unsearchable);
    let (ty, field) = (decl.name.clone(), "nope".into());
    let unknown = invalid("nope", Case::UnknownField { ty, field });
    assert_eq!(search(&decl, &["nope"]), unknown);
    let empty = Err(lakelands_api::Error::invalid(Case::SearchFieldsEmpty));
    assert_eq!(search(&decl, &[]), empty);
    decl.fields[2].searchable = false;
    assert_eq!(
        search(&decl, &["iban"]),
        not_granted("iban"),
        "an unreadable field is not granted, whether it is searchable or not"
    );
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
    let search = |who, v: &[&FieldName]| held(who).check_search(&decl, None, v);
    assert_eq!(
        search(&reader, &[&vec]),
        refused_on("note", Refusal::NotGranted),
        "the vector is readable, but it encodes `note`"
    );
    assert!(search(&reader, &[]).is_ok(), "no field is searchable");
    assert!(search(&principal(Access::Read, &[]), &[&vec]).is_ok());
}

#[test]
fn a_write_is_refused_whole_by_its_first_failing_field() {
    let owner = principal(Access::Write, &[]);
    let write = |who: &Principal, v: serde_json::Value| {
        let patch: BTreeMap<FieldName, Value> = serde_json::from_value(v).unwrap();
        held(who).check_write(&invoice(), (Door::Authored, None), &patch)
    };
    assert!(write(&owner, json!({"verdict": "x", "note": "y"})).is_ok());
    assert_eq!(
        write(&owner, json!({"verdict": "x", "amount": "1"})),
        refused_on("amount", Refusal::WrongDoor)
    );
    assert_eq!(
        write(&owner, json!({"score": 1})),
        refused_on("score", Refusal::WrongDoor)
    );
    let limited = principal(Access::Read, &[("note", Access::Write)]);
    assert_eq!(
        write(&limited, json!({"note": "y", "verdict": "x"})),
        refused_on("verdict", Refusal::NotGranted)
    );
}

#[test]
fn ingest_needs_the_ingest_grant_and_writes_source_and_shared_fields_only() {
    let mut feed = principal(Access::Write, &[]);
    let patch: BTreeMap<FieldName, Value> =
        serde_json::from_value(json!({"amount": "1", "note": "n"})).unwrap();
    let crm = SourceId::from("crm");
    let from = |s| (Door::Source, Some(s));
    assert_eq!(
        held(&feed).check_write(&invoice(), from(&crm), &patch),
        Err(Error::Refused {
            field: None,
            reason: Refusal::NotGranted
        })
    );
    feed.grants.ingest = true;
    let feed = held(&feed);
    assert!(feed.check_write(&invoice(), from(&crm), &patch).is_ok());
    let authored: BTreeMap<FieldName, Value> =
        serde_json::from_value(json!({"verdict": "x"})).unwrap();
    assert!(matches!(
        feed.check_write(&invoice(), from(&crm), &authored),
        Err(Error::Refused {
            reason: Refusal::WrongDoor,
            ..
        })
    ));
    // An authored field that admits a source takes that source alone (069).
    let mut admitting = invoice();
    let verdict = admitting
        .fields
        .iter_mut()
        .find(|f| f.name.as_str() == "verdict");
    verdict.unwrap().admits = vec!["samples".into()];
    let samples = SourceId::from("samples");
    let from_source = |s| feed.check_write(&admitting, from(s), &authored);
    assert!(from_source(&samples).is_ok());
    assert_eq!(from_source(&crm), refused_on("verdict", Refusal::WrongDoor));
}

#[test]
fn system_types_are_recognised_by_prefix_alone() {
    assert!(is_system(&"sys.setting".into()));
    assert!(is_system(&"sys.anything_new".into()));
    assert!(!is_system(&"system_note".into()));
    assert!(!is_system(&"a.sys.b".into()));
}

#[test]
fn a_view_carries_its_source_only_where_it_groups_by_the_owner() {
    let decl = |v: serde_json::Value| -> TypeDecl { serde_json::from_value(v).expect("a type") };
    let view = |name: &str, by: &[&str], merge: bool| {
        let merge = if merge {
            json!([{"source": "claim", "measures": []}])
        } else {
            json!([])
        };
        decl(
            json!({"name": name, "view": {"source": "claim", "group_by": by, "merge": merge,
                    "measures": [{"name": "n", "expr": "count()"}]}}),
        )
    };
    let types = [
        decl(json!({"name": "claim", "fields": [{"name": "owner", "type": {"kind": "text"}}]})),
        view("by_owner", &["owner"], false),
        view("all", &[], false),
        view("merged", &["owner"], true),
    ];
    let type_of = |t| Declared {
        declaration: lakelands_api::write::Declaration::Type(t),
        version: 1,
        schema_hash: None,
        since: Position(1),
    };
    let force: Vec<_> = types.map(type_of).into();
    let mut who = principal(Access::Read, &[]);
    let alice = TypeGrants {
        only: [("owner".into(), json!("alice"))].into(),
        ..TypeGrants::default()
    };
    who.grants.types.insert("claim".into(), alice);
    let pairs = vec![(FieldName::from("owner"), json!("alice"))];
    let admit = |who: &Principal, ty: &str| Sight::held(who, &force).admit(&ty.into());
    let only = Admit::Only(pairs);
    assert_eq!(admit(&who, "claim"), only);
    assert_eq!(admit(&who, "by_owner"), only, "rule 16 maps the pairs");
    assert_eq!(
        admit(&who, "sys:kept.by_owner"),
        only,
        "a kept name is its view"
    );
    let computed = Admit::Computed(vec![]);
    assert_eq!(admit(&who, "all"), computed, "no group holds the owner");
    assert_eq!(admit(&who, "merged"), computed, "a merge computes");
    assert_eq!(admit(&who, "invoice"), Admit::All);
    // It fails closed: a pair no engine can hold a record to admits none,
    // of the type and of a view over it, whatever a record holds.
    let grant = who.grants.types.get_mut("claim").expect("granted");
    grant.only.insert("gone".into(), json!("x"));
    let held = [
        ("owner".into(), json!("alice")),
        ("gone".into(), json!("x")),
    ]
    .into();
    for ty in ["claim", "by_owner", "sys:kept.by_owner", "all", "merged"] {
        assert_eq!(admit(&who, ty), Admit::Nothing, "{ty}");
        assert!(!admit(&who, ty).holds(&held) && admit(&who, ty).pairs().is_none());
    }
    assert!(only.holds(&held) && computed.holds(&held) && !only.holds(&[].into()));
}

/// A type `invoice` of written fields `f0`, `f1`, …, each listing the
/// fields of `reads` it is built from, the `sensitive` ones marked.
fn built(reads: &[&[usize]], sensitive: &[usize]) -> TypeDecl {
    let name = |i: usize| format!("f{i}");
    let of = |(i, from): (usize, &&[usize])| FieldDecl {
        depends_on: from.iter().map(|j| name(*j).as_str().into()).collect(),
        ..field(&name(i), Authority::Authored, sensitive.contains(&i))
    };
    TypeDecl {
        fields: reads.iter().enumerate().map(of).collect(),
        ..TypeDecl::named("invoice")
    }
}

/// The rule's one step: `f`'s grant lowered by what `of` answers for
/// each field it is built from, a sensitive one nobody named raised.
fn ruled(
    who: &Principal,
    decl: &TypeDecl,
    f: &FieldDecl,
    of: &dyn Fn(&FieldDecl) -> Access,
) -> Access {
    let mut access = who.grants.access(&decl.name, &f.name);
    let mut from_source = false;
    for d in &f.depends_on {
        let source = decl.fields.iter().find(|x| &x.name == d).expect("declared");
        access = match of(source) {
            s if s.reads() => access,
            _ if access.writes() => Access::WriteOnly,
            Access::Masked if access.reads() => Access::Masked,
            Access::Masked => access,
            _ => {
                from_source = true;
                Access::None
            }
        };
    }
    let named = who.grants.types["invoice"].fields.contains_key(&f.name);
    match access {
        Access::None if f.sensitive && !named && !from_source => Access::Masked,
        a => a,
    }
}

/// A field's access with every source asked again on every way that
/// leads to it: the rule where no field is built from itself.
fn plainly(who: &Principal, decl: &TypeDecl, f: &FieldDecl) -> Access {
    ruled(who, decl, f, &|source| plainly(who, decl, source))
}

/// The walk as it was: a field on the way the walk came lowers nothing.
fn by_its_way(who: &Principal, decl: &TypeDecl, f: &FieldDecl, way: &[&FieldName]) -> Access {
    let way = [way, &[&f.name]].concat();
    let of = |s: &FieldDecl| match way.contains(&&s.name) {
        true => Access::Read,
        false => by_its_way(who, decl, s, &way),
    };
    ruled(who, decl, f, &of)
}

/// The widest answers under which the rule holds for every field of
/// `decl`: every field taken as read, then the rule's step until none
/// moves.
fn widest(who: &Principal, decl: &TypeDecl) -> Vec<(String, Access)> {
    let mut all: Vec<_> = (decl.fields.iter())
        .map(|f| (f.name.to_string(), Access::Read))
        .collect();
    loop {
        let of = |s: &FieldDecl| {
            all.iter()
                .find(|a| a.0 == s.name.as_str())
                .expect("a field")
                .1
        };
        let step = |f: &FieldDecl| (f.name.to_string(), ruled(who, decl, f, &of));
        let next: Vec<_> = decl.fields.iter().map(step).collect();
        if next == all {
            all.sort();
            return all;
        }
        all = next;
    }
}

const KINDS: [Access; 5] = [
    Access::None,
    Access::Masked,
    Access::Read,
    Access::Write,
    Access::WriteOnly,
];

/// A type of two to seven fields and a principal, the `round`th of a
/// fixed sequence: each field built from some later ones, and where
/// `cycles` from earlier ones too.
fn drawn(round: u64, cycles: bool) -> (TypeDecl, Principal) {
    let mut x = 0x9e37_79b9_7f4a_7c15 ^ (round + 1).wrapping_mul(0xff51_afd7_ed55_8ccd);
    let mut next = |m: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % m) as usize
    };
    let n = 2 + next(6);
    let mut reads = vec![Vec::new(); n];
    for (i, from) in reads.iter_mut().enumerate() {
        from.extend((0..n).filter(|j| (*j > i || cycles && *j != i) && next(3) == 0));
    }
    let reads: Vec<&[usize]> = reads.iter().map(Vec::as_slice).collect();
    let sensitive: Vec<usize> = (0..n).filter(|_| next(3) == 0).collect();
    let names: Vec<String> = (0..n).map(|i| format!("f{i}")).collect();
    let mut grants = Vec::new();
    for f in &names {
        if next(2) == 0 {
            grants.push((f.as_str(), KINDS[next(5)]));
        }
    }
    (
        built(&reads, &sensitive),
        principal(KINDS[next(5)], &grants),
    )
}

#[test]
fn a_field_many_others_are_built_from_is_asked_once_and_answers_as_the_rule_does() {
    // Under every kind of grant, the walk that asks a field once answers
    // as the rule asked plainly does.
    for round in 0..2_000 {
        let (decl, who) = drawn(round, false);
        for f in &decl.fields {
            let (is, rule) = (field_access(&who, &decl, f), plainly(&who, &decl, f));
            assert_eq!(is, rule, "{} of {decl:?} for {:?}", f.name, who.grants);
        }
    }
    // Two ways to one masked field: the second lowers as the first does.
    // `f1` is masked by it, `f2`, which its reader writes, is not read,
    // and `f0`, built from both, is hidden by the second.
    let decl = built(&[&[1, 2], &[3], &[3], &[]], &[]);
    let who = principal(
        Access::Read,
        &[("f2", Access::Write), ("f3", Access::Masked)],
    );
    let all: Vec<_> = (decl.fields.iter())
        .map(|f| field_access(&who, &decl, f))
        .collect();
    let (hidden, unread) = (Access::None, Access::WriteOnly);
    assert_eq!(all, [hidden, Access::Masked, unread, Access::Masked]);
    // Twenty fields, each built from every later one: 2^18 ways lead from
    // the first to the last, and one masked field at their end masks all.
    let reads: Vec<Vec<usize>> = (0..20).map(|i| (i + 1..20).collect()).collect();
    let reads: Vec<&[usize]> = reads.iter().map(Vec::as_slice).collect();
    let decl = built(&reads, &[]);
    let who = principal(Access::Read, &[("f19", Access::Masked)]);
    for f in &decl.fields {
        assert_eq!(field_access(&who, &decl, f), Access::Masked, "{}", f.name);
        assert_eq!(granted_access(&who, &decl, f), Access::Masked, "{}", f.name);
    }
}

#[test]
fn fields_that_list_each_other_have_one_answer_each_and_the_rule_holds_for_all() {
    use Access::{Masked, None as Hidden, Read, Write, WriteOnly};
    let answers = |who: &Principal, decl: &TypeDecl| -> Vec<(String, Access)> {
        let of = |f: &FieldDecl| (f.name.to_string(), field_access(who, decl, f));
        let mut all: Vec<_> = decl.fields.iter().map(of).collect();
        all.sort();
        all
    };
    // Two written fields, each listing the other. Read whole, the cycle
    // lowers nothing.
    let pair = built(&[&[1], &[0]], &[]);
    let of = |who: &Principal| {
        answers(who, &pair)
            .into_iter()
            .map(|a| a.1)
            .collect::<Vec<_>>()
    };
    assert_eq!(of(&principal(Read, &[("f1", Write)])), [Read, Write]);
    // `f0` masked: `f1` is built from it, so it is written and not read;
    // `f0` is built from `f1`, which is not read, so it is hidden. A walk
    // that left out the field it came from answered `f0` masked.
    assert_eq!(
        of(&principal(Masked, &[("f1", Write)])),
        [Hidden, WriteOnly]
    );
    // A sensitive field nobody granted masks the one built from it, and
    // stays masked itself.
    let sensitive = built(&[&[1], &[0]], &[0]);
    let who = principal(Hidden, &[("f1", Read)]);
    assert_eq!(
        answers(&who, &sensitive),
        [("f0".to_string(), Masked), ("f1".to_string(), Masked)]
    );
    for round in 0..2_000 {
        let (decl, who) = drawn(round, true);
        let all = answers(&who, &decl);
        // The answers are the widest under which the rule holds for every
        // field, and none is wider than the walk that left out the fields
        // on its way answered: read where that read, shown where that
        // showed, written alike.
        assert_eq!(all, widest(&who, &decl), "{decl:?}");
        for (f, (_, is)) in decl.fields.iter().zip(&all) {
            let was = by_its_way(&who, &decl, f, &[]);
            let wider = is.reads() && !was.reads() || is.appears() && !was.appears();
            assert!(
                !wider && is.writes() == was.writes(),
                "{}: {was:?}, now {is:?}",
                f.name
            );
        }
        // And the same in whatever order the type lists its fields and a
        // field what it is built from.
        let mut turned = decl.clone();
        turned.fields.reverse();
        (turned.fields.iter_mut()).for_each(|f| f.depends_on.reverse());
        assert_eq!(answers(&who, &turned), all, "{decl:?}");
    }
}

/// `keeps_from` says whether `known_fields` takes anything out, for a
/// reader who knows everything, one kept from a field, from a link field,
/// and from the type a relation reaches (lakelands#943).
#[test]
fn keeps_from_says_whether_known_fields_takes_anything_out() {
    let mut paid = RelationDecl::to("paid", "payment");
    paid.fields = vec![field("fee", Authority::Authored, false)];
    let t = TypeDecl {
        relations: vec![paid.clone()],
        ..invoice()
    };
    let payment = TypeDecl {
        fields: vec![field("sum", Authority::Authored, false)],
        ..TypeDecl::named("payment")
    };
    let find = |n: &TypeName| (n.as_str() == "payment").then_some(&payment);
    let link = crate::links::link_type(&t.name, &paid).name;
    let kept = |ty: &TypeName, f: &str| {
        let mut who = principal(Access::Read, &[]);
        let fields = [(FieldName::from(f), Access::None)].into();
        let grants = TypeGrants {
            default: None,
            fields,
            ..Default::default()
        };
        who.grants.types.insert(ty.clone(), grants);
        who
    };
    let mut unshown = principal(Access::Read, &[]);
    let none = TypeGrants {
        default: Some(Access::None),
        ..Default::default()
    };
    unshown.grants.types.insert("payment".into(), none);
    for (who, keeps) in [
        (principal(Access::Read, &[]), false),
        (principal(Access::Read, &[("note", Access::None)]), true),
        (kept(&link, "fee"), true),
        (unshown, true),
    ] {
        let mut known = t.clone();
        known_fields(&who, &mut known, &find);
        assert_eq!(
            (keeps_from(&who, &t, &find), known != t),
            (keeps, keeps),
            "{:?}",
            who.grants
        );
    }
}

#[test]
fn one_reads_all_whom_no_grant_holds_below_read_and_no_only_bounds() {
    let reads = |d, fields: &[(&str, Access)]| held(&principal(d, fields)).reads_all_fields();
    assert!(reads(Access::Write, &[]) && reads(Access::Read, &[("iban", Access::Write)]));
    for (default, field) in [
        (Access::None, Access::Read),
        (Access::Masked, Access::Read),
        (Access::WriteOnly, Access::Read),
        (Access::Write, Access::None),
        (Access::Write, Access::Masked),
        (Access::Write, Access::WriteOnly),
    ] {
        assert!(
            !reads(default, &[("iban", field)]),
            "{default:?} with iban {field:?}"
        );
    }
    let mut typed = principal(Access::Write, &[]);
    typed.grants.types.get_mut("invoice").unwrap().default = Some(Access::Masked);
    let mut only = principal(Access::Write, &[]);
    let invoice = only.grants.types.get_mut("invoice").unwrap();
    invoice.only.insert("team".into(), json!("a"));
    assert!(
        !held(&typed).reads_all_fields() && !held(&only).reads_all_fields(),
        "a type's default, an `only`"
    );
}

#[test]
fn a_quarantined_cell_is_read_as_its_field_and_lowered_by_a_hiding_on_either_record() {
    let mut who = principal(
        Access::None,
        &[("amount", Access::Read), ("note", Access::Read)],
    );
    let cells =
        |v: serde_json::Value| -> BTreeMap<FieldName, Value> { serde_json::from_value(v).unwrap() };
    let sent = cells(json!({"amount": "x", "iban": "DE00", "verdict": "ok", "ghost": 1}));
    let came = [FieldName::from("verdict"), FieldName::from("ghost")];
    let (shown, masked) = held(&who).quarantined_cells(&invoice(), &sent, &came, None);
    assert_eq!(shown.keys().collect::<Vec<_>>(), ["amount"]);
    assert_eq!(masked, ["ghost", "iban", "verdict"].map(FieldName::from));
    let (_, masked) = held(&who).quarantined_cells(&invoice(), &sent, &[], None);
    assert_eq!(
        masked,
        [FieldName::from("iban")],
        "no raise it did not come with"
    );

    let grant = who
        .grants
        .types
        .get_mut(&TypeName::from("invoice"))
        .unwrap();
    grant.hide_where.push(HiddenWhere {
        field: "note".into(),
        values: ["secret".to_owned()].into(),
        fields: [("amount".into(), Access::None)].into(),
    });
    let sight = held(&who);
    let amount = |cells: &BTreeMap<FieldName, Value>, live: Option<&BTreeMap<FieldName, Value>>| {
        sight
            .quarantined_cells(&invoice(), cells, &[], live)
            .0
            .contains_key("amount")
    };
    assert!(!amount(
        &cells(json!({"amount": "1", "note": "secret"})),
        None
    ));
    assert!(
        !amount(&cells(json!({"amount": "1", "note": 5})), None),
        "no text selects"
    );
    let live = cells(json!({"note": "secret"}));
    assert!(!amount(
        &cells(json!({"amount": "1", "note": "open"})),
        Some(&live)
    ));
    assert!(amount(&cells(json!({"amount": "1"})), None));
    assert!(amount(&cells(json!({"amount": "1", "note": "open"})), None));
}

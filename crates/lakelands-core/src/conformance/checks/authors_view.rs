//! A migration's shape is its author's view of the type
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md` and
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! each extended): what the author may not know of is not its to remove.

use lakelands_api::error::Case;
use lakelands_api::ontology::{Derived, MatchPair, RelationDecl, StorageKind, TypeDecl};
use lakelands_api::principal::TypeGrants;
use lakelands_api::write::{Declaration, Migration, ProposalFilter, ProposedWrite};
use lakelands_api::{Access, Catalog, FieldName, LakeStore, Outcome, Principal, ProposalId};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, alike_renamed, derived, field, field_access, invalid_case, invalid_like, migration,
    reference, refused_on, type_of, value,
};
use crate::conformance::{Failure, Held, Store};
use crate::links::link_type_name;

check!(
    DECLARED_SINCE_NOT_TAKEN = a_bundle_takes_no_place_declared_since_it_was_proposed,
    "027 and 013, extended (lakelands#915): a bundle takes no place declared since it was proposed, as a migration changes no type changed since: a trigger, a verb, a type and an object set proposed while their names were free, and a verb under a name declared since on a type every reviewer is shown, are refused at the accept of a reviewer shown both as `declared_since_proposed`, naming what and which name, appending nothing, and stay pending; its reject is taken; to a reviewer kept from the type what took the name stands on, and to their proposer, kept alike, the first four are not listed, and its accept and reject answer as an id never proposed; a bundle whose name stood unchanged since before it was proposed is accepted; and a verb taken since on a type a reviewer is shown, reading a field it may not, leaves the bundle not there to it, as `describe` lists it no such verb"
);

check!(
    HIDDEN_LINK_TYPE_NOT_GRANTED = a_link_type_its_author_is_not_shown_takes_no_field_from_it,
    "027 and 024, extended (lakelands#965): a relation its author is shown whose link type it is not shown takes no link field from it: sent with one named as a field the link type holds, and with one named otherwise, by `declare`, `apply`, `propose` and a migration's preview, it is not granted, naming nothing, word for word alike, and the head does not move; sent without fields it is applied, the link type's field kept; the owner adds the field"
);

check!(
    HIDDEN_FIELD_KEPT = a_migration_keeps_the_fields_its_author_may_not_know_of,
    "027 and 024, extended: a migration's `to` is read as its author's view, so a field hidden from the author stays as declared, with its values, whether the migration is applied by a principal with the schema grant or proposed and accepted; `dropped` lists only what the author saw and removed; the proposer reads its own proposal without the field, a reviewer who may see it reads it there"
);

check!(
    HIDDEN_NAME_NOT_LEARNED = a_change_of_shape_never_names_a_field_its_author_may_not_know_of,
    "027 and 024, extended: a type declared again from its author's view is taken, keeping the fields and the link fields hidden from the author, and the relations it is not shown, a reference to a type it is not shown kept as a field it may not know of (lakelands#718), whose target it names in vain, answered as one no type is (lakelands#716); a shape that names a hidden field is refused as a field declared twice; one that removes a field a hidden field or relation reads is refused on that field and names no other part; a rename from a hidden field is a rename from no field, and one from a hidden relation from no relation"
);

fn invoice(fields: &[&str]) -> TypeDecl {
    let fields: Vec<_> = fields.iter().map(|f| (*f, StorageKind::Text)).collect();
    type_of("invoice", &fields)
}

/// An agent `id` that reads everything but `invoice.salary`, which it may
/// not know of.
fn without_salary(store: &dyn LakeStore, id: &str, schema: bool) -> Principal {
    let mut who = field_access(store, id, "invoice", &[("salary", Access::None)]);
    who.grants.schema = schema;
    who
}

/// Refuse unless `invoice` holds exactly the fields `want` and `i1` its
/// salary still.
async fn kept(l: &Lake, want: &[&str], after: &str) -> Held {
    let fields = l.describe_type("invoice").await?.fields;
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    let i1 = l.read("invoice", "i1").await?;
    ensure!(
        names == want && value(&i1, "salary") == Some(&json!("100")),
        "{after}, `invoice` must hold {want:?} and `i1` its salary; it holds {names:?} and `i1` \
         reads {:?}",
        i1.fields
    );
    Ok(())
}

/// The fields of the shape proposal `id` migrates to, and what its report
/// lists as dropped, as `who` reads the proposal.
async fn shown(
    cat: &dyn Catalog,
    who: &Principal,
    id: &ProposalId,
) -> Result<(Vec<FieldName>, Vec<(FieldName, u64)>), Failure> {
    let listed = cat.proposals(who, ProposalFilter::default()).await;
    let listed = listed.must("listing the proposals")?.data;
    match listed
        .into_iter()
        .find(|p| &p.id == id)
        .and_then(|p| p.write)
    {
        Some(ProposedWrite::Migrate { migration, report }) => {
            let fields = migration.to.fields.into_iter().map(|f| f.name).collect();
            Ok((fields, report.dropped))
        }
        other => Err(Failure::new(format!("the proposal is listed as {other:?}"))),
    }
}

async fn a_migration_keeps_the_fields_its_author_may_not_know_of(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(invoice(&["no", "note", "salary", "team"]))
        .await?;
    let all = json!({"no": "1", "note": "n", "salary": "100", "team": "t"});
    l.upsert("invoice", "i1", all).await?;

    // Applied: a principal with the schema grant removes `team`.
    let schemer = without_salary(store.as_ref(), "schemer", true);
    let to = migration(invoice(&["no", "note"]));
    let report = cat.migrate(&schemer, to.clone()).await;
    let report = report.must("the preflight of removing `team`")?.data;
    ensure!(
        report.dropped == [("team".into(), 1)],
        "the preflight of a shape without `team`, by a principal who may not know of `salary`, \
         must list `team` alone as dropped; it listed {:?}",
        report.dropped
    );
    let fut = cat.migrate_apply(&schemer, to, &report.report_id);
    fut.await.must("applying the migration without `team`")?;
    kept(&l, &["no", "note", "salary"], "after the applied migration").await?;

    // Proposed: a principal without the grant removes `note`.
    let reader = without_salary(store.as_ref(), "reader", false);
    let write = ProposedWrite::Migrate {
        migration: Box::new(migration(invoice(&["no"]))),
        report: Default::default(),
    };
    let proposed = cat.propose(&reader, write, "no notes".into()).await;
    let Outcome::Proposed { proposal, .. } = proposed.must("proposing a shape without `note`")?
    else {
        return Err(Failure::new("a proposed migration answers `proposed`"));
    };
    let (mine, _) = shown(cat.as_ref(), &reader, &proposal).await?;
    let (whole, dropped) = shown(cat.as_ref(), own, &proposal).await?;
    ensure!(
        mine == ["no"] && whole == ["no", "salary"] && dropped == [("note".into(), 1)],
        "the proposer must read its proposal as `no`, and a reviewer who may see `salary` as \
         `no` and `salary` with `note` alone dropped; they read {mine:?}, and {whole:?} with \
         {dropped:?}"
    );
    let fut = cat.accept(own, &proposal);
    fut.await.must("accepting the shape without `note`")?;
    kept(&l, &["no", "salary"], "after the accepted proposal").await
}

/// `approved_by`, a relation of `invoice` to itself with the link
/// `fields`; `grade` is the one the author may not know of.
fn approved_by(fields: &[&str]) -> RelationDecl {
    RelationDecl {
        name: "approved_by".into(),
        target: "invoice".into(),
        fields: fields.iter().map(|f| field(f, StorageKind::Text)).collect(),
        ..Default::default()
    }
}

/// `invoice` with `fields` as its author sees it: `payee` refers to
/// `payroll`, which the author is not shown, so it is a field the author
/// may not know of (lakelands#718), and so are `paid` and `matched`, which
/// `ref` is read by.
fn seen(fields: &[&str]) -> TypeDecl {
    let mut t = invoice(fields);
    t.relations = vec![approved_by(&["since"])];
    t
}

/// Declare `invoice` whole, with what its author may not know of: `salary`,
/// `net`, which is built from it and `gross`, the link field `grade`, the
/// reference `payee`, and the relations `paid` and `matched`; and answer
/// that author, `schemer`.
async fn hiding(l: &Lake, store: &dyn LakeStore) -> Result<Principal, Failure> {
    l.declare(type_of("payroll", &[("code", StorageKind::Text)]))
        .await?;
    let mut whole = seen(&["no", "gross", "salary", "ref"]);
    whole.fields.push(reference("payee", "payroll"));
    let net = derived("net", StorageKind::Text, "gross", &["gross", "salary"]);
    whole.fields.push(net);
    let pairs = vec![MatchPair {
        from: "ref".into(),
        to: "code".into(),
    }];
    let matched = RelationDecl {
        derived: Some(Derived {
            pairs,
            filter: None,
        }),
        ..RelationDecl::to("matched", "payroll")
    };
    let paid = RelationDecl::to("paid", "payroll");
    whole.relations = vec![approved_by(&["since", "grade"]), paid, matched];
    l.declare(whole).await?;
    let mut who = without_salary(store, "schemer", true);
    let none = TypeGrants {
        default: Some(Access::None),
        ..Default::default()
    };
    who.grants.types.insert("payroll".into(), none);
    let grade = [("grade".into(), Access::None)].into();
    let links = TypeGrants {
        default: None,
        fields: grade,
        ..Default::default()
    };
    let link_type = link_type_name(&"invoice".into(), &"approved_by".into());
    who.grants.types.insert(link_type, links);
    Ok(who)
}

async fn a_change_of_shape_never_names_a_field_its_author_may_not_know_of(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let who = hiding(&l, store.as_ref()).await?;

    // Declared again from the author's view, with one field more.
    cat.declare(
        &who,
        Declaration::Type(seen(&["no", "gross", "ref", "memo"])),
    )
    .await
    .must("declaring `invoice` again from its author's view, with `memo`")?;
    let after = l.describe_type("invoice").await?;
    let fields: Vec<&str> = after.fields.iter().map(|f| f.name.as_str()).collect();
    let relations: Vec<&str> = after.relations.iter().map(|r| r.name.as_str()).collect();
    let link_fields: Vec<&str> = (after.relations.iter())
        .flat_map(|r| r.fields.iter().map(|f| f.name.as_str()))
        .collect();
    ensure!(
        fields == ["no", "gross", "salary", "ref", "payee", "net", "memo"]
            && relations == ["approved_by", "paid", "matched"]
            && link_fields == ["since", "grade"],
        "the type declared again from its author's view must keep `salary`, `net`, `payee`, the \
         link field `grade` and the relations `paid` and `matched`, each at its place; it holds \
         {fields:?}, the relations {relations:?} and the link fields {link_fields:?}"
    );

    let shape = |to: TypeDecl, renames: &[(&str, &str)]| Migration {
        renames: (renames.iter().map(|(a, b)| ((*a).into(), (*b).into()))).collect(),
        ..Migration::to(to)
    };
    let mut settled = seen(&["no", "gross", "ref", "memo"]);
    settled
        .relations
        .push(RelationDecl::to("settled", "invoice"));
    let settled = Migration {
        relation_renames: vec![("paid".into(), "settled".into())],
        ..shape(settled, &[])
    };
    for (what, m, field, says) in [
        (
            "naming the hidden `salary`",
            shape(seen(&["no", "gross", "ref", "memo", "salary"]), &[]),
            Some("salary"),
            json!({"code": "declared_twice", "what": "field", "name": "salary"}),
        ),
        (
            "removing `gross`, which the hidden `net` is built from",
            shape(seen(&["no", "ref", "memo"]), &[]),
            Some("gross"),
            json!({"code": "field_read_elsewhere", "field": "gross"}),
        ),
        (
            "removing `ref`, which the hidden `matched` reads",
            shape(seen(&["no", "gross", "memo"]), &[]),
            Some("ref"),
            json!({"code": "field_read_elsewhere", "field": "ref"}),
        ),
        (
            "renaming the hidden `salary`",
            shape(
                seen(&["no", "gross", "ref", "memo", "pay"]),
                &[("salary", "pay")],
            ),
            None,
            json!({"code": "rename_from_unknown", "from": "salary"}),
        ),
        (
            "renaming the hidden `paid`",
            settled,
            None,
            json!({"code": "relation_rename_names_none"}),
        ),
    ] {
        let answer = cat.migrate(&who, m).await;
        let named = refused_on(&answer).map(|(f, _)| f);
        // No member of its wire form, its sentence among them, names one.
        let wire = answer.as_ref().err().map(|e| json!(e).to_string());
        let wire = wire.unwrap_or_default();
        let hidden = ["net", "matched", "payroll"];
        ensure!(
            named == field
                && invalid_like(&answer, &says)
                && !hidden.iter().any(|h| wire.contains(h)),
            "a migration {what} must be refused as invalid on {field:?}, as {says} and \
             naming none of {hidden:?}; it answered {answer:?}"
        );
    }
    // An author held from `payee` learns nothing of its target by naming
    // it: `payroll`, another hidden type and no type answer alike, never as
    // a field declared twice (lakelands#716).
    l.declare(type_of("ledger", &[("code", StorageKind::Text)]))
        .await?;
    let mut blind = who.clone();
    blind.id = "blind".into();
    (blind.grants.types).insert("ledger".into(), who.grants.types["payroll"].clone());
    let held = blind.grants.types.entry("invoice".into()).or_default();
    held.fields.insert("payee".into(), Access::None);
    let mut said = Vec::new();
    for target in ["ghost", "payroll", "ledger"] {
        let mut t = seen(&["no", "gross", "ref", "memo"]);
        t.fields.retain(|f| f.name.as_str() != "payee");
        t.fields.push(reference("payee", target));
        said.push((target, cat.declare(&blind, Declaration::Type(t)).await));
    }
    // Member for member and word for word as for `ghost`, under its name.
    let alike = |(target, answer): &(&str, lakelands_api::Result<_>)| match (answer, &said[0].1) {
        (Err(e), Err(ghost)) => {
            let twice = matches!(invalid_case(e), Some(Case::DeclaredTwice { .. }));
            !twice && alike_renamed("", e, ghost, ("ghost", target)).is_ok()
        }
        _ => false,
    };
    ensure!(
        said.iter().all(alike),
        "an author held from `payee` must be answered alike for `payroll`, another hidden type and \
         no type, never as a field declared twice; it was answered {said:?}"
    );
    let _ = own;
    Ok(())
}

check!(
    HIDDEN_NAME_JUDGED_AS_ABSENT =
        a_migrations_faults_come_in_one_order_for_a_hidden_name_and_an_absent_one,
    "027 and 024, extended (lakelands#750): a migration's renames are judged against its author's view of the type, at the step that judges the lake's own, so a hidden field or relation renamed answers word for word as an absent one, alone and beside a fault judged before or after it; a shape naming a hidden field answers as one naming a field twice, and one also removing a field a hidden field reads is refused on that field first"
);

async fn a_migrations_faults_come_in_one_order_for_a_hidden_name_and_an_absent_one(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let who = hiding(&l, store.as_ref()).await?;
    let m = |fields: &[&str], renames: &[(&str, &str)], relations: &[(&str, &str)]| {
        let mut to = seen(fields);
        for (_, new) in relations {
            to.relations.push(RelationDecl::to(*new, "invoice"));
        }
        Migration {
            renames: (renames.iter().map(|(a, b)| ((*a).into(), (*b).into()))).collect(),
            relation_renames: (relations.iter().map(|(a, b)| ((*a).into(), (*b).into()))).collect(),
            ..Migration::to(to)
        }
    };
    let base = ["no", "gross", "ref"];
    let with = |more: &[&'static str]| [&base[..], more].concat();
    // `numbered`, shown to the author, reads `no`, which nothing hidden
    // reads: taking `no` away is a fault judged after the shape is read
    // and before any rename.
    let numbered = json!({"declare": "object_set", "name": "numbered", "of": "invoice",
        "where": "no = 'x'"});
    let numbered = serde_json::from_value(numbered).map_err(|e| Failure::new(e.to_string()))?;
    let fut = l.cat.declare(&l.own, numbered);
    fut.await.must("declaring `numbered`")?;
    // Each row: the migration naming the hidden name, the one naming an
    // absent name in its place, and the two names; ("", "") where the
    // answer names neither.
    let rows = [
        (
            "renaming the hidden `salary`",
            m(&with(&["pay"]), &[("salary", "pay")], &[]),
            m(&with(&["pay"]), &[("ghost", "pay")], &[]),
            ("ghost", "salary"),
        ),
        (
            "renaming the hidden `paid`",
            m(&base, &[], &[("paid", "settled")]),
            m(&base, &[], &[("nope", "settled")]),
            ("nope", "paid"),
        ),
        (
            "naming the hidden `salary`, as `memo` named twice",
            m(&with(&["salary"]), &[], &[]),
            m(&with(&["memo", "memo"]), &[], &[]),
            ("memo", "salary"),
        ),
        (
            "renaming the hidden `paid` and the absent `ghostf`",
            m(&with(&["x"]), &[("ghostf", "x")], &[("paid", "settled")]),
            m(&with(&["x"]), &[("ghostf", "x")], &[("nope", "settled")]),
            ("", ""),
        ),
        (
            "renaming `no` to no field, then the hidden `salary`",
            m(&with(&["x"]), &[("no", "nosuchto"), ("salary", "x")], &[]),
            m(&with(&["x"]), &[("no", "nosuchto"), ("ghost", "x")], &[]),
            ("", ""),
        ),
        (
            "renaming the hidden `salary` and taking away `no`, which `numbered` reads",
            m(&["gross", "ref", "pay"], &[("salary", "pay")], &[]),
            m(&["gross", "ref", "pay"], &[("ghost", "pay")], &[]),
            ("", ""),
        ),
    ];
    // Each by `migrate`, by `migrate_apply` and by `propose`, which judge
    // the migration in one path.
    let send = |door: &'static str, m: Migration| {
        let (cat, who) = (l.cat.clone(), who.clone());
        async move {
            match door {
                "migrate" => cat.migrate(&who, m).await.map(drop),
                "apply" => cat.migrate_apply(&who, m, "none").await.map(drop),
                _ => {
                    let write = ProposedWrite::Migrate {
                        migration: Box::new(m),
                        report: Default::default(),
                    };
                    cat.propose(&who, write, "a migration".into())
                        .await
                        .map(drop)
                }
            }
        }
    };
    for (what, hidden, absent, names) in rows {
        for door in ["migrate", "apply", "propose"] {
            let (h, a) = (
                send(door, hidden.clone()).await,
                send(door, absent.clone()).await,
            );
            let (Err(he), Err(ae)) = (&h, &a) else {
                return Err(Failure::new(format!(
                    "a migration {what} must be refused by `{door}` as its absent twin is; they \
                     answered {h:?} and {a:?}"
                )));
            };
            alike_renamed(&format!("a migration {what}, by `{door}`"), he, ae, names)?;
        }
    }
    // A hidden field named again, and `gross` removed, which the hidden
    // `net` reads: the read is judged first, on the field.
    let answer = l.cat.migrate(&who, m(&["no", "ref", "salary"], &[], &[]));
    let answer = answer.await;
    let says = json!({"code": "field_read_elsewhere", "field": "gross"});
    ensure!(
        refused_on(&answer).map(|(f, _)| f) == Some("gross") && invalid_like(&answer, &says),
        "a migration naming the hidden `salary` and removing `gross` must be refused on `gross` as \
         {says}; it answered {answer:?}"
    );
    Ok(())
}

check!(
    HIDDEN_NAME_NOT_IN_THE_SHAPE = a_shape_names_no_field_its_author_may_not_know_of,
    "027 and 024, extended (lakelands#726): a valid time, a key or a relation's `via` its author gives anew names a field of its view, so a field it may not know of answers word for word as a name no field has, by `declare` and by `migrate`, and the type in force is unmoved; what stands as declared is taken, valid times left out as `describe` gives them, and a rename from a relation the author is not shown, or a relation of that name sent to a hidden type, answers as for no relation and no type; a valid time sent where one over such a field stands, the one in force restated too, is judged by its name first, answering as on a type with none, and only a field its author may name is refused, as the bound taken (lakelands#692), but for an end sent with no start shown, which is refused as one alone, as on a type with none (028, lakelands#909); a valid-time bound over a field its reader may not know of is not described, and a read at a `valid_at`, its total and a `get` answer by the bounds shown, as on a type declared with those alone"
);

async fn a_shape_names_no_field_its_author_may_not_know_of(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, .. } = &l;
    // `veiled` and `cloak` are hidden from the author, and so is `payroll`,
    // so `payee`, which refers to it, is too (lakelands#718): `goes` and
    // `pays`, carried by them, are not shown to it (lakelands#727).
    // `stamped` is valid from `veiled` to `cloak`.
    fn goes(name: &str, via: &str) -> RelationDecl {
        let via = Some(via.into());
        RelationDecl {
            via,
            ..RelationDecl::to(name, "other")
        }
    }
    let seen = || {
        type_of(
            "ev",
            &[("no", StorageKind::Text), ("from", StorageKind::DateTime)],
        )
    };
    let stamped = |more: (&str, StorageKind)| {
        let mut t = type_of("stamped", &[("no", StorageKind::Text), more]);
        (t.valid_from, t.valid_to) = (Some("veiled".into()), Some("cloak".into()));
        t
    };
    let mut whole = seen();
    whole.fields.push(field("veiled", StorageKind::DateTime));
    whole
        .fields
        .extend([reference("cloak", "other"), reference("payee", "payroll")]);
    whole.relations = vec![goes("goes", "cloak"), goes("pays", "payee")];
    let mut timed = stamped(("veiled", StorageKind::DateTime));
    timed.fields.push(field("cloak", StorageKind::DateTime));
    // `plain` is `stamped` with no valid time.
    let plain = TypeDecl {
        name: "plain".into(),
        valid_from: None,
        valid_to: None,
        ..timed.clone()
    };
    let other = |name| type_of(name, &[("x", StorageKind::Text)]);
    for t in [other("other"), other("payroll"), whole, timed, plain] {
        l.declare(t).await?;
    }
    let later =
        json!({"no": "1", "veiled": "2030-01-01T00:00:00Z", "cloak": "2031-01-01T00:00:00Z"});
    for ty in ["stamped", "plain"] {
        l.upsert(ty, "s1", later.clone()).await?;
    }
    let hidden = [("veiled", Access::None), ("cloak", Access::None)];
    let mut who = field_access(store.as_ref(), "schemer", "ev", &hidden);
    let held = who.grants.types.values().next().expect("granted").clone();
    (who.grants.types).insert("stamped".into(), held.clone());
    (who.grants.types).insert("plain".into(), held);
    let none = TypeGrants {
        default: Some(Access::None),
        ..Default::default()
    };
    (who.grants.types).insert("payroll".into(), none);
    who.grants.schema = true;
    // A valid time over a field its author may not know of is not named
    // to it, and a read at a valid time answers as for a type with none.
    use lakelands_api::{Find, ReadAt, Timestamp};
    let read = |who: Principal, ty: &'static str| {
        let at = Timestamp::parse("2026-06-01T00:00:00Z").expect("an instant");
        // With its total, which the engine counts by the bounds shown.
        let find = Find {
            at: ReadAt {
                valid_at: Some(at),
                ..Default::default()
            },
            total: true,
            ..Default::default()
        };
        let get = lakelands_api::Get {
            key: "h1".into(),
            with_meta: false,
            at: find.at,
            params: Default::default(),
        };
        let cat = cat.clone();
        async move {
            let found = cat.find(&who, &ty.into(), find).await.map(|r| r.data);
            let got = cat.get(&who, &ty.into(), get).await.map(|r| r.data);
            format!("{found:?} {got:?}").replace(ty, "T")
        }
    };
    let shown = cat
        .describe_type(&who, &"stamped".into(), Default::default())
        .await;
    let shown = shown
        .must("describing `stamped` to its author")?
        .data
        .access;
    let (stamped_at, plain_at) = (
        read(who.clone(), "stamped").await,
        read(who.clone(), "plain").await,
    );
    ensure!(
        (shown.valid_from, shown.valid_to) == (None, None) && stamped_at == plain_at,
        "`stamped`, valid from and to fields its reader may not know of, must be described \
         with no valid time and read at one as `plain`, which has none: {stamped_at} | {plain_at}"
    );
    // One bound over a field it may not know of: only that bound is
    // dropped. `half` reads as `halfplain`, valid from `from` alone,
    // `mhalf`, from the masked `veiled`, as `mplain`, and so do bounds
    // named as keywords are (lakelands#726).
    let bounded = |name: &str, from: &str, to: Option<&str>| {
        let mut t = type_of(name, &[("no", StorageKind::Text)]);
        let timed = ["from", "veiled", "cloak", "in", "null"];
        (t.fields).extend(timed.map(|f| field(f, StorageKind::DateTime)));
        (t.valid_from, t.valid_to) = (Some(from.into()), to.map(Into::into));
        t
    };
    let late = "2030-01-01T00:00:00Z";
    let late = json!({"no": "1", "from": late, "veiled": late, "in": late, "null": late});
    let mut half = who.clone();
    let fields = [
        ("veiled".into(), Access::Masked),
        ("cloak".into(), Access::None),
    ]
    .into();
    let held = TypeGrants {
        fields,
        ..Default::default()
    };
    let pairs = [
        ("half", "halfplain", "from"),
        ("mhalf", "mplain", "veiled"),
        ("inhalf", "inplain", "in"),
        ("nullhalf", "nullplain", "null"),
    ];
    let mut apart = Vec::new();
    for (a, b, from) in pairs {
        for (name, to) in [(a, Some("cloak")), (b, None)] {
            l.declare(bounded(name, from, to)).await?;
            l.upsert(name, "h1", late.clone()).await?;
            half.grants.types.insert(name.into(), held.clone());
        }
        let (x, y) = (read(half.clone(), a).await, read(half.clone(), b).await);
        if x != y {
            apart.push(format!("`{a}` | `{b}`: {x} | {y}"));
        }
    }
    ensure!(
        apart.is_empty(),
        "a valid time with one bound hidden reads as the other alone:\n{}",
        apart.join("\n")
    );
    // What stands as declared is taken: the valid times left out, as
    // `stamped` is described. Restated, a valid time answers as on `plain`
    // (below, lakelands#692).
    let described = TypeDecl {
        valid_from: None,
        valid_to: None,
        ..stamped(("memo", StorageKind::Text))
    };
    for t in [seen(), described] {
        let what = format!("declaring `{}` again as its author is shown it", t.name);
        cat.declare(&who, Declaration::Type(t)).await.must(&what)?;
    }
    type Names = fn(&mut TypeDecl, &str);
    let members: [(&str, Names); 4] = [
        ("veiled", |t, n| t.valid_from = Some(n.into())),
        ("veiled", |t, n| {
            (t.valid_from, t.valid_to) = (Some("from".into()), Some(n.into()))
        }),
        ("veiled", |t, n| t.key = vec![n.into()]),
        ("cloak", |t, n| t.relations.push(goes("runs", n))),
    ];
    let mut said = Vec::new();
    for ((name, set), door) in members
        .iter()
        .flat_map(|m| [(m, "declare"), (m, "migrate")])
    {
        let mut answers = Vec::new();
        for n in [*name, "nowhere"] {
            let mut t = seen();
            set(&mut t, n);
            let answer = match door {
                "declare" => cat.declare(&who, Declaration::Type(t)).await.map(drop),
                _ => cat.migrate(&who, migration(t)).await.map(drop),
            };
            let answer = answer.map_or_else(|e| e.to_string(), |()| "taken".into());
            answers.push(answer.replace(n, "N"));
        }
        said.push(answers);
    }
    // A valid time sent where one over a field its author may not know of
    // stands is judged by its name first, as on `plain`, which has none:
    // `zz` and a hidden field answer word for word as there, and so does
    // leaving it out. Only a field the author may name is refused, as the
    // bound taken, as a name a hidden field holds is (lakelands#692).
    let send = |t: TypeDecl, door: &'static str| {
        let (cat, who) = (cat.clone(), who.clone());
        async move {
            match door {
                "declare" => cat.declare(&who, Declaration::Type(t)).await.map(drop),
                _ => cat.migrate(&who, migration(t)).await.map(drop),
            }
        }
    };
    let shape = |name: &str, axis: &str, to: Option<&str>| {
        let mut t = TypeDecl {
            name: name.into(),
            valid_from: None,
            valid_to: None,
            ..stamped(("memo", StorageKind::Text))
        };
        t.fields.push(field("d2", StorageKind::DateTime));
        *match axis {
            "valid_from" => &mut t.valid_from,
            _ => &mut t.valid_to,
        } = to.map(Into::into);
        t
    };
    // Each axis: another hidden field, and the bound's own, restated.
    for (axis, hidden, own) in [
        ("valid_from", "cloak", "veiled"),
        ("valid_to", "veiled", "cloak"),
    ] {
        for door in ["declare", "migrate"] {
            for to in ["zz", hidden, own] {
                let (on_stamped, on_plain) = (
                    send(shape("stamped", axis, Some(to)), door).await,
                    send(shape("plain", axis, Some(to)), door).await,
                );
                let what = format!("`{axis}: {to}` sent by `{door}` on `stamped`");
                match (&on_stamped, &on_plain) {
                    (Err(a), Err(b)) => alike_renamed(&what, a, b, ("plain", "stamped"))?,
                    _ => {
                        return Err(Failure::new(format!(
                            "{what} must be refused as on `plain`; they answered {on_stamped:?} \
                             and {on_plain:?}"
                        )));
                    }
                }
            }
        }
        let left = send(shape("stamped", axis, None), "migrate").await;
        let plain = send(shape("plain", axis, None), "migrate").await;
        ensure!(
            left.is_ok() && plain.is_ok(),
            "`{axis}` left out is taken on `stamped` as on `plain`: {left:?} | {plain:?}"
        );
        // An end with no start shown is one alone, refused on both as on a
        // type with no valid time (028, lakelands#909); a start is the
        // bound taken.
        let says = match axis {
            "valid_to" => json!({"code": "valid_to_alone", "field": "d2"}),
            _ => json!({"code": "valid_time_stands", "axis": axis}),
        };
        for door in ["declare", "migrate"] {
            let answer = send(shape("stamped", axis, Some("d2")), door).await;
            ensure!(
                invalid_like(&answer, &says),
                "`{axis}: d2` sent by `{door}` on `stamped` must be refused as {says}; it \
                 answered {answer:?}"
            );
        }
        let plain = send(shape("plain", axis, Some("d2")), "migrate").await;
        let taken = axis == "valid_from" && plain.is_ok();
        ensure!(
            taken || invalid_like(&plain, &says),
            "`{axis}: d2` on `plain` is taken, an end alone refused: {plain:?}"
        );
    }
    // A field of its own named like a hidden one, sent as a valid time, is
    // a field declared twice, as on `plain`, and never the bound taken: the
    // answer does not tell which hidden field a bound names (lakelands#692).
    for axis in ["valid_from", "valid_to"] {
        for door in ["declare", "migrate"] {
            let named = |name: &str| {
                let mut t = shape(name, axis, Some("veiled"));
                t.fields.push(field("veiled", StorageKind::DateTime));
                t
            };
            let (on_stamped, on_plain) = (
                send(named("stamped"), door).await,
                send(named("plain"), door).await,
            );
            let what = format!("its own `veiled` as `{axis}`, by `{door}`, on `stamped`");
            match (&on_stamped, &on_plain) {
                (Err(a), Err(b)) => alike_renamed(&what, a, b, ("plain", "stamped"))?,
                _ => {
                    return Err(Failure::new(format!(
                        "{what} must be refused as on `plain`; they answered {on_stamped:?} \
                         and {on_plain:?}"
                    )));
                }
            }
        }
    }
    // With one bound hidden, its restatement answers as on the type with the
    // other bound alone.
    for door in ["declare", "migrate"] {
        let mut said = Vec::new();
        for name in ["half", "halfplain"] {
            let t = bounded(name, "from", Some("cloak"));
            said.push(match door {
                "declare" => cat.declare(&half, Declaration::Type(t)).await.map(drop),
                _ => cat.migrate(&half, migration(t)).await.map(drop),
            });
        }
        let what = format!("`half`'s `valid_to: cloak` restated by `{door}`");
        match (&said[0], &said[1]) {
            (Err(a), Err(b)) => alike_renamed(&what, a, b, ("halfplain", "half"))?,
            _ => {
                return Err(Failure::new(format!(
                    "{what} must be refused as on `halfplain`; they answered {said:?}"
                )));
            }
        }
    }
    let (ev, timed) = (
        l.describe_type("ev").await?,
        l.describe_type("stamped").await?,
    );
    let times = |t: &lakelands_api::describe::TypeDescription| {
        (t.access.valid_from.clone(), t.access.valid_to.clone())
    };
    ensure!(
        said.iter().all(|a| a[0] == a[1])
            && (ev.access.key.is_empty() && times(&ev) == (None, None) && ev.relations.len() == 2)
            && times(&timed) == (Some("veiled".into()), Some("cloak".into())),
        "a valid time, a key or a `via` naming a field its author may not know of must answer \
         as one naming no field, and `ev` and `stamped` stay as declared; hidden | none, by \
         `declare` and by `migrate`: {said:?}; `ev` holds {:?} and {:?}, `stamped` {:?}",
        ev.access,
        ev.relations,
        timed.access
    );
    // A rename from a relation the author is not shown is one from no
    // relation, and one sent to the hidden `payroll` answers as to `zzt`.
    let mut answers = Vec::new();
    for from in ["goes", "pays", "nothing"] {
        let mut t = seen();
        t.relations.push(goes("went", "from"));
        let relation_renames = vec![(from.into(), "went".into())];
        let renamed = Migration {
            relation_renames,
            ..migration(t)
        };
        let answer = cat.migrate(&who, renamed).await;
        answers.push(
            answer
                .map_or_else(|e| e.to_string(), |_| "taken".into())
                .replace(from, "N"),
        );
    }
    for target in ["payroll", "zzt"] {
        let mut t = seen();
        t.relations = vec![RelationDecl::to("pays", target)];
        let answer = cat.declare(&who, Declaration::Type(t)).await;
        answers.push(
            answer
                .map_or_else(|e| e.to_string(), |_| "taken".into())
                .replace(target, "N"),
        );
    }
    ensure!(
        answers[0] == answers[2] && answers[1] == answers[2] && answers[3] == answers[4],
        "a rename from `goes` or `pays`, which the author is not shown, must answer as one from \
         no relation, and `pays` sent to `payroll` as to `zzt`: {answers:?}"
    );
    Ok(())
}

check!(
    TAKEN_NAME_NOT_GRANTED =
        a_trigger_or_a_verb_under_a_name_its_author_is_not_shown_is_not_granted,
    "027 and 024, extended (lakelands#893): a trigger and a verb are known by their name alone, so one declared under the name of one its author is not shown would take its place; it is not granted, naming nothing and word for word alike, to a holder of `schema` kept from the type it stands on by `declare` and by `apply`, to a proposer kept from that type by `propose`, and to a proposer that is shown the type and may not know of the field the trigger's `when` reads and the verb sets, a verb being judged by what it names and not by who may call it, so that one who reads the type and may not call the verb proposes it; so is a function proposed under the name of one that computes a field of that type, by one kept from the type and by one shown it that may not know of that field or of its input, and declared or applied by a holder of `schema` kept from the type, which is shown every function, with a new body and with an input named as the field it computes, where it declares one that computes no field again (lakelands#917), where one that computes no field is proposed again, alone and with a type that uses it, as a new one is; the head does not move, so nothing is computed again, and `describe` shows them as they stood; proposed by one shown both types, the trigger and the verb under those names, and a set whose name was taken on that type since, are not there to a reviewer kept from the type the one in force stands on, in its listing, at its accept and at its reject, word for word as an id that does not exist, appending nothing, and the trigger still fires there; a name none bears is declared and proposed; and an author shown both types moves its trigger by declaring it again, after which a write to the type it stood on fires it no more"
);

async fn a_trigger_or_a_verb_under_a_name_its_author_is_not_shown_is_not_granted(
    store: Store,
) -> Held {
    use crate::conformance::fixture::{
        computed_by, head_unmoved, object_set_on, trigger_on, type_decl, write_verb,
    };
    use lakelands_api::{Error, Missing, Refusal};
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let half = |name: &str, body: &str| -> Declaration {
        let n = json!([{"name": "n", "type": {"kind": "number"}}]);
        let f = json!({"declare": "function", "name": name, "backing": "expr", "inputs": n,
            "expr": body});
        serde_json::from_value(f).expect("a function reads")
    };
    for (name, body) in [("half", "n / 2"), ("third", "n / 3")] {
        l.declare_as(half(name, body), "a function").await?;
    }
    let (text, number) = (StorageKind::Text, StorageKind::Number);
    let h = computed_by("h", number, "half", &["n"]);
    let vault = vec![field("state", text), field("n", number), h];
    l.declare(type_decl("vault", vault)).await?;
    l.declare(type_of("desk", &[("state", text)])).await?;
    let when = Some("state is not null");
    let on = |w, v, ty| [trigger_on(w, ty, when), write_verb(v, ty, "state", "'x'")];
    for d in on("watch", "seal", "vault") {
        l.declare_as(d, "what stands on `vault`").await?;
    }
    // `author` holds `schema` and reviews, `proposer` neither, each kept
    // from `vault`; `reader` is shown it and may not know of what the two
    // name; `caller` reads it and may not call `seal`.
    let kept = |id: &str, both: bool, vault: serde_json::Value| {
        let types = json!({"vault": vault});
        let grants = json!({"default": "read", "schema": both, "review": both, "types": types});
        super::only::holder(s, id, grants)
    };
    let none = || json!({"default": "none"});
    let author = kept("author", true, none());
    let proposer = kept("proposer", false, none());
    let reader = kept("reader", false, json!({"fields": {"state": "none"}}));
    let caller = kept("caller", false, json!({"default": "read"}));
    let stood = |d: lakelands_api::describe::LakeDescription| {
        let triggers = d.triggers.iter().map(|t| format!("{} on {}", t.name, t.on));
        let verbs = d.verbs.iter().map(|v| format!("{} on {}", v.name, v.on));
        let functions = (d.functions.iter()).map(|f| format!("{} is {:?}", f.name, f.expr));
        triggers.chain(verbs).chain(functions).collect::<Vec<_>>()
    };
    let before = s.head().await.must("reading the head")?;
    let was = stood(l.describe().await?);
    let proposed = |who: &Principal, declarations: Vec<Declaration>| {
        let (who, cat, write) = (
            who.clone(),
            &l.cat,
            ProposedWrite::Declare {
                declarations,
                plan: None,
            },
        );
        async move { cat.propose(&who, write, "mine".into()).await }
    };
    let mut told = Vec::new();
    for d in on("watch", "seal", "desk") {
        told.push(l.cat.declare(&author, d.clone()).await.err());
        told.push(l.cat.apply(&author, vec![d.clone()]).await.err());
        for who in [&proposer, &reader] {
            told.push(proposed(who, vec![d.clone()]).await.err());
        }
    }
    told.push(proposed(&proposer, vec![half("half", "n * 3")]).await.err());
    // Nor to a holder of `schema` kept from `vault`, shown every function
    // (lakelands#917), and so before an input named as the field it
    // computes would tell that the field reads itself.
    let mut own = half("half", "h / 3");
    if let Declaration::Function(f) = &mut own {
        f.inputs[0].field.name = "h".into();
    }
    for d in [half("half", "n * 3"), own] {
        told.push(l.cat.declare(&author, d.clone()).await.err());
        told.push(l.cat.apply(&author, vec![d]).await.err());
    }
    // One shown `vault` that may not know of the field `half` computes, or
    // of its input, is not shown the function either.
    for f in ["h", "n"] {
        let blind = kept(f, false, json!({"fields": {f: "none"}}));
        told.push(proposed(&blind, vec![half("half", "n * 3")]).await.err());
    }
    let refused = Some(Error::Refused {
        reason: Refusal::NotGranted,
        field: None,
    });
    let stands = stood(l.describe().await?);
    ensure!(
        told.iter().all(|t| *t == refused) && stands == was && was.len() == 4,
        "a trigger and a verb under the names of the ones on `vault`, declared and applied by \
         a holder of `schema` and proposed by one without, each kept from `vault`, and \
         proposed by one kept from the field they read, and a function under the name of the \
         one that computes a field of `vault`, proposed by one kept from it, from that field \
         and from its input, and declared and applied by the holder of `schema`, are not \
         granted, \
         naming nothing ({told:?}), and what is declared stands as it stood ({was:?}); it stands \
         as {stands:?}"
    );
    head_unmoved(
        s,
        before,
        "a trigger, a verb and a function under a taken name",
    )
    .await?;
    // Proposed by one shown both types, they are not there to a reviewer
    // kept from `vault`: in its listing, at its accept, at its reject. The
    // set was proposed while its name was free.
    let [moved, sealed] = on("watch", "seal", "desk");
    let late = |of| object_set_on("late", of, "state is not null");
    let mut offers = Vec::new();
    for d in [moved.clone(), sealed.clone(), late("desk")] {
        let offer = proposed(&l.own, vec![d]).await;
        let Outcome::Proposed { proposal, .. } = offer.must("the owner proposes")? else {
            return Err(Failure::new("a proposal answers `proposed`"));
        };
        offers.push(proposal);
    }
    l.declare_as(late("vault"), "the set's name, taken since")
        .await?;
    let at = s.head().await.must("reading the head")?;
    let listed = l.cat.proposals(&author, Default::default()).await;
    let listed = listed.must("a reviewer's listing")?.data;
    let (mut decided, mut absent) = (Vec::new(), Vec::new());
    for id in &offers {
        decided.push(l.cat.accept(&author, id).await.err());
        decided.push(l.cat.reject(&author, id, "no".into()).await.err());
        let missing = Missing::Proposal { id: id.clone() };
        absent.extend([(); 2].map(|()| {
            Some(Error::NotFound {
                missing: missing.clone(),
            })
        }));
    }
    ensure!(
        listed.iter().all(|p| !offers.contains(&p.id)) && decided == absent,
        "a trigger `watch` and a verb `seal` on `desk`, proposed while they stand on `vault`, \
         and a set `late` of `desk`, declared of `vault` since, are not listed to a reviewer \
         kept from `vault` ({listed:?}), and the accept and the reject of each answer as an id \
         that does not exist ({decided:?})"
    );
    head_unmoved(
        s,
        at,
        "an accept and a reject of an entry that is not there",
    )
    .await?;
    // A name none bears is theirs to declare and to propose, and one shown
    // both types moves its own trigger by declaring it again.
    let [heed, stow] = on("heed", "stow", "desk");
    let declared = l.cat.declare(&author, heed).await;
    let idle = l.cat.declare(&author, half("third", "n * 5")).await;
    let shelf = |by: &str| {
        let fields = vec![field("n", number), computed_by("h", number, by, &["n"])];
        vec![
            half(by, "n * 3"),
            Declaration::Type(type_decl("shelf", fields)),
        ]
    };
    let free = [
        proposed(&proposer, vec![stow.clone()]).await,
        proposed(&reader, vec![stow]).await,
        proposed(&caller, vec![sealed]).await,
        proposed(&proposer, vec![half("third", "n * 3")]).await,
        proposed(&proposer, shelf("third")).await,
        proposed(&proposer, shelf("fourth")).await,
    ];
    let fired = |o: Outcome| {
        let Outcome::Applied { set_off, .. } = o else {
            return false;
        };
        set_off.fired.iter().any(|t| t.as_str() == "watch")
    };
    let still = fired(l.upsert("vault", "v0", json!({"state": "a"})).await?);
    l.declare_as(moved, "moving `watch`").await?;
    let old = fired(l.upsert("vault", "v1", json!({"state": "a"})).await?);
    let new = fired(l.upsert("desk", "d1", json!({"state": "a"})).await?);
    ensure!(
        declared.is_ok()
            && idle.is_ok()
            && free
                .iter()
                .all(|o| matches!(o, Ok(Outcome::Proposed { .. })))
            && (still && !old && new),
        "a trigger `heed` and `third` are declared ({declared:?}, {idle:?}); a verb `stow`, twice, `seal` on `desk` by \
         one that reads `vault`, and `third`, which computes no field, alone, with a type that \
         uses it, and as a new `fourth` is, are proposed ({free:?}); `watch` fires on `vault` \
         until it is moved ({still}), then no more ({old}), and on `desk` ({new})"
    );
    Ok(())
}

check!(
    TAKEN_TOOL_NOT_GRANTED = a_tool_name_taken_by_what_its_author_is_not_shown_is_not_granted,
    "027 and 024, extended (lakelands#896): a tool's name is taken as a trigger's is, so a declaration whose tool would bear the name of one that comes from a type, a relation or a verb its author is not shown is not granted, naming nothing and word for word as declaring that type is: a type under the name of a verb's tool, a verb under the name of the type's tool, of its relation's, and of a relation its author is not shown of a type it is, alone and in a set that declares that type again as its author knows it, and a set that brings a link type under the name of a verb's tool, by `declare`, by `apply` and by `propose`, and a migration that adds a relation under the name of a verb's tool, previewed and proposed; the head does not move; a proposal, a set's and a migration's, whose tool's name was taken since by what its reviewer is not shown is not there to it, in its listing, at its accept and at its reject, word for word as an id that does not exist (lakelands#915); a collision between what its author is shown is refused to it as to the owner, as `tool_names`, by `declare`, by `propose` and at the accept, and to one that is listed a verb and may not call it, and a relation under the name of one its author is not shown as declared twice, as before; a free name is declared and proposed"
);

async fn a_tool_name_taken_by_what_its_author_is_not_shown_is_not_granted(store: Store) -> Held {
    use crate::conformance::fixture::{head_unmoved, write_verb};
    use lakelands_api::{Error, Refusal};
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let shape = |name: &str, to: &[(&str, &str)]| {
        let mut t = type_of(name, &[("state", StorageKind::Text)]);
        t.relations = to.iter().map(|(r, t)| RelationDecl::to(*r, *t)).collect();
        t
    };
    let ty = |name: &str, to: &[(&str, &str)]| Declaration::Type(shape(name, to));
    let verb = |name: &str| write_verb(name, "desk", "state", "'x'");
    let propose = |who: &Principal, write: ProposedWrite| {
        let (who, cat) = (who.clone(), &l.cat);
        async move { cat.propose(&who, write, "mine".into()).await }
    };
    let proposed = |who, declarations| {
        propose(
            who,
            ProposedWrite::Declare {
                declarations,
                plan: None,
            },
        )
    };
    let moved = |to: &[(&str, &str)]| ProposedWrite::Migrate {
        migration: Box::new(migration(shape("desk", to))),
        report: Default::default(),
    };
    // `desk` holds `vault`, which is with `desk`; `box` and a relation
    // `near` of `desk` are proposed while their tools' names are free, and
    // verbs on `vault` take them since, and that of a link type's door.
    let (vault, desk) = (
        ty("vault", &[("with", "desk")]),
        ty("desk", &[("holds", "vault")]),
    );
    let applied = l.cat.apply(&l.own, vec![vault, desk]).await;
    applied.must("two types, each with a relation to the other")?;
    // A tool's name is the generator's, read off the lake's own tools.
    let made = lakelands_api::tools::tools(&l.describe().await?).unwrap_or_default();
    let tool = |of: &str| {
        let mut made = made.iter().map(|t| t.name.as_str());
        made.find(|n| n.starts_with(of)).unwrap_or_default()
    };
    let (door, link) = (tool("desk_"), tool("desk_holds_"));
    let far = link_type_name(&"desk".into(), &"far".into());
    let far = lakelands_api::tools::tool_base(far.as_str());
    let mut offers = Vec::new();
    for write in [
        ProposedWrite::Declare {
            declarations: vec![ty("box", &[])],
            plan: None,
        },
        moved(&[("holds", "vault"), ("near", "desk")]),
    ] {
        let offer = propose(&l.own, write).await;
        let Outcome::Proposed { proposal, .. } = offer.must("the owner proposes")? else {
            return Err(Failure::new("a proposal answers `proposed`"));
        };
        offers.push(proposal);
    }
    for (name, was, is) in [
        (door, "desk", "box"),
        (link, "holds", "near"),
        (door, "desk", &far),
    ] {
        let taken = write_verb(&name.replacen(was, is, 1), "vault", "state", "'x'");
        l.declare_as(taken, "a verb on `vault`").await?;
    }
    let kept = |id: &str, both: bool| {
        let types = json!({"vault": {"default": "none"}});
        let grants = json!({"default": "read", "schema": both, "review": both, "types": types});
        super::only::holder(s, id, grants)
    };
    let (author, proposer) = (kept("author", true), kept("proposer", false));
    let before = s.head().await.must("reading the head")?;
    let sets = [
        vec![ty("vault", &[])],
        vec![ty("box", &[])],
        vec![verb(tool("vault_"))],
        vec![verb(tool("vault_with_"))],
        vec![verb(link)],
        vec![ty("desk", &[]), verb(link)],
        vec![ty("desk", &[("far", "desk")]), ty("crate", &[])],
    ];
    let near = migration(shape("desk", &[("near", "desk")]));
    // Proposed while the names were free, they are not there to it: its
    // accept and its reject answer as an id that does not exist
    // (lakelands#915).
    let mut gone = Vec::new();
    for id in &offers {
        let missing = lakelands_api::Missing::Proposal { id: id.clone() };
        let absent = Some(Error::NotFound { missing });
        gone.push(l.cat.accept(&author, id).await.err() == absent);
        gone.push(l.cat.reject(&author, id, "no".into()).await.err() == absent);
    }
    ensure!(
        gone.iter().all(|g| *g),
        "`box` and `near`, proposed before their tools' names were taken on `vault`, are not \
         there to one kept from it: {gone:?}"
    );
    let mut told = vec![
        l.cat.migrate(&author, near).await.err(),
        propose(&proposer, moved(&[("near", "desk")])).await.err(),
    ];
    for set in &sets {
        if let [d] = &set[..] {
            told.push(l.cat.declare(&author, d.clone()).await.err());
        }
        told.push(l.cat.apply(&author, set.clone()).await.err());
        told.push(proposed(&proposer, set.clone()).await.err());
    }
    let refused = Some(Error::Refused {
        reason: Refusal::NotGranted,
        field: None,
    });
    // Between what each is shown the collision is told, as to the owner; a
    // relation under the name of one its author is not shown is declared
    // twice, as a field is (027).
    let both = verb(door);
    let mut shown = Vec::new();
    for (who, to) in [(&author, &proposer), (&l.own, &l.own)] {
        shown.push(l.cat.declare(who, both.clone()).await.err());
        shown.push(proposed(to, vec![both.clone()]).await.err());
    }
    for id in &offers {
        shown.push(l.cat.accept(&l.own, id).await.err());
    }
    // A verb its author is listed and may not call is one it is shown.
    let caller = super::only::holder(s, "caller", json!({"default": "read"}));
    shown.push(proposed(&caller, vec![ty("box", &[])]).await.err());
    let names = |e: &Option<Error>| {
        let told = json!({"code": "tool_names"});
        e.as_ref().is_some_and(|e| invalid_like(e, &told))
    };
    let again = vec![ty("desk", &[("holds", "desk")])];
    let twice = l.cat.apply(&author, again).await;
    ensure!(
        told.iter().all(|t| *t == refused) && told.len() == 21,
        "a type `vault` its author is not shown, a type `box` where a verb under its door's \
         name stands on it, on `desk` verbs under the names of a door of `vault`, of its \
         relation's and of that of `holds` ({link}), the last also in a set with `desk` as \
         its author knows it, and a set that brings the link type of a relation `far` where \
         a verb bears its door's name, declared and applied by a holder of `schema` and \
         proposed by one without, each kept from `vault`, a relation `near` of `desk` by a \
         migration previewed and proposed, are not granted, naming nothing: {told:?}"
    );
    ensure!(
        shown[..2] == shown[2..4]
            && shown.iter().all(names)
            && invalid_like(&twice, &json!({"code": "declared_twice", "name": "holds"})),
        "a verb `{door}` on `desk` is refused to one kept from `vault` as to the owner, and \
         the owner's accepts of `box` and of `near`, and `box` to one that reads `vault` and \
         may not call the verb on it, each as `tool_names` ({shown:?}), and a relation `holds` \
         of `desk` is declared twice to one not shown the one in force ({twice:?})"
    );
    head_unmoved(s, before, "a declaration under a tool's taken name").await?;
    // Nor is either listed to it.
    let listed = l.cat.proposals(&author, Default::default()).await;
    let listed = listed.must("a reviewer's listing")?.data;
    ensure!(
        listed.iter().all(|p| !offers.contains(&p.id)),
        "one kept from `vault` is listed neither `box` nor `near`: {listed:?}"
    );
    let free = [
        l.cat.apply(&author, vec![ty("crate", &[])]).await.err(),
        proposed(&proposer, vec![verb("ghost")]).await.err(),
    ];
    ensure!(
        free == [None, None],
        "a type `crate` is applied and a verb `ghost` proposed by one kept from `vault`: \
         {free:?}"
    );
    Ok(())
}

check!(
    NO_TOOL_PROBLEM_LEFT = no_door_leaves_a_lake_with_a_tool_name_problem,
    "027, extended (lakelands#896), and 014: a declaration is checked with the names of the link types it brings, alone as in a set, so no door leaves a lake with a problem among its tools' names: a type declared again with a relation whose link type's door would bear the name of another type's tool, and with one whose link type's door would be too long a name, is refused as `tool_names` by `declare`, by `apply` and by `propose`, as a migration that adds the relation and as one that renames a relation into it, each previewed, applied under an id a preview answered before and proposed, and at the accept of a proposal made while the name was free; in a set of two, beside a plain type, it is refused by `apply`, by `propose` and at such an accept as `set_refused` with two problems, the plain type's too (lakelands#893); after each a plain type is declared by the owner and by one kept from the relation's target, naming nothing, and the lake's tools are generated (lakelands#918); the same declaration alone, where the other side is a verb its author is not shown, is not granted, naming nothing, as a set's is, where the owner is told `tool_names`, and under a free name it is applied"
);

async fn no_door_leaves_a_lake_with_a_tool_name_problem(store: Store) -> Held {
    use crate::conformance::fixture::write_verb;
    use lakelands_api::tools::{tool_base, tools};
    use lakelands_api::{Error, Refusal};
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let shape = |name: &str, to: &[(&str, &str)]| {
        let mut t = type_of(name, &[("state", StorageKind::Text)]);
        t.relations = to.iter().map(|(r, t)| RelationDecl::to(*r, *t)).collect();
        t
    };
    let ty = |name: &str, to: &[(&str, &str)]| Declaration::Type(shape(name, to));
    let propose = |write: ProposedWrite| {
        let (who, cat) = (l.own.clone(), &l.cat);
        async move { cat.propose(&who, write, "mine".into()).await }
    };
    let set = |declarations| ProposedWrite::Declare {
        declarations,
        plan: None,
    };
    // `desk` is near `vault`; a relation `far` of it is proposed while its
    // link type's doors are free, then a type takes their names. A
    // relation of 48 characters has link doors, and its link type's doors
    // are too long.
    let both = vec![ty("vault", &[]), ty("desk", &[("near", "vault")])];
    l.cat.apply(&l.own, both).await.must("two types")?;
    let with = |relation| [("near", "vault"), (relation, "vault")];
    let adds = |relation| migration(shape("desk", &with(relation)));
    let renames = |relation: &str| {
        let mut renames = migration(shape("desk", &[(relation, "vault")]));
        renames.relation_renames = vec![("near".into(), relation.into())];
        renames
    };
    let two = |relation| vec![ty("desk", &with(relation)), ty("spare", &[])];
    // Proposed, alone and in a set of two, and previewed while the names
    // are free.
    let (mut offers, mut ids) = (Vec::new(), Vec::new());
    for declarations in [vec![ty("desk", &with("far"))], two("far")] {
        let offer = propose(set(declarations)).await;
        let Outcome::Proposed { proposal, .. } = offer.must("the owner proposes")? else {
            return Err(Failure::new("a proposal answers `proposed`"));
        };
        offers.push(proposal);
    }
    for m in [adds("far"), renames("far")] {
        let report = l.cat.migrate(&l.own, m).await.must("a preview")?;
        ids.push(report.data.report_id);
    }
    let base =
        |relation: &str| tool_base(link_type_name(&"desk".into(), &relation.into()).as_str());
    l.declare(shape(&base("far"), &[])).await?;
    let long = "r".repeat(48);
    let grants =
        json!({"default": "read", "schema": true, "types": {"vault": {"default": "none"}}});
    let kept = super::only::holder(s, "author", grants);
    let (mut told, mut whole, mut free) = (Vec::new(), Vec::new(), Vec::new());
    for relation in ["far", &long] {
        for door in 0..13 {
            let single = ty("desk", &with(relation));
            // The adding migration at the doors 5 to 7, the renaming one after.
            let moved = match door < 8 {
                true => adds(relation),
                false => renames(relation),
            };
            let (migration, report) = (Box::new(moved.clone()), Default::default());
            let answer = match door {
                0 => l.cat.declare(&l.own, single).await.err(),
                1 => l.cat.apply(&l.own, vec![single]).await.err(),
                2 => propose(set(vec![single])).await.err(),
                3 => l.cat.apply(&l.own, two(relation)).await.err(),
                4 => propose(set(two(relation))).await.err(),
                5 | 8 => l.cat.migrate(&l.own, moved).await.err(),
                6 | 9 => (l.cat.migrate_apply(&l.own, moved, &ids[door / 8]).await).err(),
                7 | 10 => (propose(ProposedWrite::Migrate { migration, report }).await).err(),
                _ if relation == "far" => l.cat.accept(&l.own, &offers[door - 11]).await.err(),
                _ => continue,
            };
            match door {
                3 | 4 | 12 => whole.push(answer),
                _ => told.push(answer),
            }
            // The lake takes a declaration still, and names nothing.
            for (who, name) in [(&l.own, "crate"), (&kept, "box")] {
                let plain = ty(&format!("{name}{}", free.len()), &[]);
                free.push(l.cat.declare(who, plain).await.err());
            }
        }
    }
    let names = |e: &Option<Error>| {
        let told = json!({"code": "tool_names"});
        e.as_ref().is_some_and(|e| invalid_like(e, &told))
    };
    let both = |e: &Option<Error>| {
        let problem = json!({"reason": {"code": "tool_names"}});
        let told = json!({"code": "set_refused", "count": 2, "problems": [problem, problem]});
        e.as_ref().is_some_and(|e| invalid_like(e, &told))
    };
    let made = tools(&l.describe().await?);
    let each = told.iter().all(names) && told.len() == 19;
    let sets = whole.iter().all(both) && whole.len() == 5;
    ensure!(
        each && sets && free.iter().all(Option::is_none) && made.is_ok(),
        "`desk` with a relation `far`, whose link type's doors a type `{}` names, and with one \
         of 48 characters, whose link type's doors are too long, by `declare`, `apply`, \
         `propose`, a migration that adds it and one that renames `near` into it, each \
         previewed, applied under an id a preview answered before and proposed, and the \
         accept of `far` proposed before, are each refused as `tool_names` ({told:?}), in a \
         set of two beside a plain `spare` as `set_refused` with two problems ({whole:?}), \
         after each a plain type is declared by the owner and by one kept from `vault` \
         ({free:?}), and the lake's tools are generated ({:?})",
        base("far"),
        made.as_ref().err()
    );
    // Where the other side is a verb its author is not shown, the single
    // declaration is not granted, as a set's is (lakelands#896).
    let made = made.unwrap_or_default();
    let mut doors = made.iter().map(|t| t.name.as_str());
    let door = doors.find(|n| n.starts_with("desk_")).unwrap_or_default();
    let taken = write_verb(
        &door.replacen("desk", &base("by"), 1),
        "vault",
        "state",
        "'x'",
    );
    l.declare_as(taken, "a verb on `vault`").await?;
    let hidden = l
        .cat
        .declare(&kept, ty("desk", &[("by", "desk")]))
        .await
        .err();
    let whole = l
        .cat
        .declare(&l.own, ty("desk", &[("near", "vault"), ("by", "desk")]));
    let (whole, twin) = (
        whole.await.err(),
        l.cat.declare(&kept, ty("desk", &[("to", "desk")])),
    );
    let (twin, refused) = (twin.await.err(), Refusal::NotGranted);
    ensure!(
        hidden
            == Some(Error::Refused {
                reason: refused,
                field: None
            })
            && names(&whole)
            && twin.is_none(),
        "`desk` with a relation `by`, whose link type's door a verb on `vault` names, declared \
         alone by one kept from `vault`, is not granted, naming nothing ({hidden:?}), the \
         owner is told `tool_names` ({whole:?}), and with a relation `to` it is applied \
         ({twin:?})"
    );
    Ok(())
}

async fn a_bundle_takes_no_place_declared_since_it_was_proposed(store: Store) -> Held {
    use crate::conformance::fixture::{head_unmoved, object_set_on, trigger_on, write_verb};
    use lakelands_api::error::Naming;
    use lakelands_api::{Error, Missing};
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let text = StorageKind::Text;
    for ty in ["desk", "vault"] {
        l.declare(type_of(ty, &[("state", text)])).await?;
    }
    l.declare_as(write_verb("mark", "desk", "state", "'m'"), "`mark`")
        .await?;
    // `carol` and `blind` are kept from `vault` and from `cell`.
    let reviewer = |id: &str, kept: serde_json::Value| {
        let types = json!({"vault": kept, "cell": kept});
        let grants = json!({"default": "read", "schema": true, "review": true, "types": types});
        super::only::holder(s, id, grants)
    };
    let none = || json!({"default": "none"});
    let declared = |mut t: serde_json::Value| {
        t["declare"] = json!("type");
        serde_json::from_value::<Declaration>(t).expect("a type reads")
    };
    let (carol, blind, both) = (
        reviewer("carol", none()),
        reviewer("blind", none()),
        reviewer("both", json!({})),
    );
    let when = Some("state is not null");
    let cell = |fields: &[(&str, StorageKind)]| Declaration::Type(type_of("cell", fields));
    let offered = [
        trigger_on("watch", "desk", when),
        write_verb("seal", "desk", "state", "'x'"),
        cell(&[("state", text)]),
        object_set_on("late", "desk", "state is not null"),
        write_verb("stamp", "desk", "state", "'y'"),
        write_verb("mark", "desk", "state", "'z'"),
    ];
    let mut offers = Vec::new();
    for d in &offered {
        let write = ProposedWrite::Declare {
            declarations: vec![d.clone()],
            plan: None,
        };
        match l.cat.propose(&carol, write, "mine".into()).await {
            Ok(Outcome::Proposed { proposal, .. }) => offers.push(proposal),
            other => return Err(Failure::new(format!("`carol` proposing {d:?}: {other:?}"))),
        }
    }
    // The names are taken since: four on what `carol` is not shown, one
    // on `desk`.
    for d in [
        trigger_on("watch", "vault", when),
        write_verb("seal", "vault", "state", "'x'"),
        cell(&[("state", text), ("note", text)]),
        object_set_on("late", "vault", "state is not null"),
        write_verb("stamp", "desk", "state", "'w'"),
    ] {
        l.declare_as(d, "a name taken since").await?;
    }
    let names = [
        (Naming::Trigger, "watch"),
        (Naming::Verb, "seal"),
        (Naming::Type, "cell"),
        (Naming::ObjectSet, "late"),
        (Naming::Verb, "stamp"),
    ];
    let ids = &offers;
    let listed = |who: &Principal| {
        let (who, cat) = (who.clone(), &l.cat);
        async move {
            let listed = cat.proposals(&who, ProposalFilter::default()).await;
            let listed = listed.must("a listing")?.data;
            let found = ids.iter().map(|id| listed.iter().find(|p| &p.id == id));
            let cells = found.map(|p| p.map(|p| (p.write.is_some(), p.more)));
            Ok::<_, Failure>(cells.collect::<Vec<_>>())
        }
    };
    let whole = [Some((true, false)); 6];
    let kept = [
        None,
        None,
        None,
        None,
        Some((true, false)),
        Some((true, false)),
    ];
    let seen = (
        listed(&both).await?,
        listed(&blind).await?,
        listed(&carol).await?,
    );
    ensure!(
        seen == (whole.to_vec(), kept.to_vec(), kept.to_vec()),
        "`both` is listed all six whole, `blind` and `carol` only the two on `desk`; they are \
         listed {seen:?}"
    );
    let before = s.head().await.must("reading the head")?;
    let mut told = Vec::new();
    for (id, (what, name)) in offers.iter().zip(names) {
        let accepted = l.cat.accept(&both, id).await;
        let due = Case::DeclaredSinceProposed {
            what,
            name: name.into(),
        };
        if invalid_case(&accepted) != Some(&due) {
            told.push(format!("{name}: {accepted:?}"));
        }
    }
    for id in &offers[..4] {
        let missing = Missing::Proposal { id: id.clone() };
        let absent = Some(Error::NotFound { missing });
        let decided = [
            l.cat.accept(&blind, id).await.err(),
            l.cat.reject(&blind, id, "no".into()).await.err(),
        ];
        if decided != [absent.clone(), absent] {
            told.push(format!("`blind` on {id}: {decided:?}"));
        }
    }
    ensure!(
        told.is_empty(),
        "each bundle under a name taken since is refused `declared_since_proposed` at the accept \
         of `both`, naming it, and is not there to `blind`: {told:?}"
    );
    head_unmoved(s, before, "accepts of bundles under names taken since").await?;
    let rejected = l.cat.reject(&both, &offers[1], "no".into()).await;
    let accepted = l.cat.accept(&both, &offers[5]).await;
    ensure!(
        rejected.is_ok() && matches!(accepted, Ok(Outcome::Applied { .. })),
        "a refused bundle is rejected ({rejected:?}), and `mark`, unchanged since before it was \
         proposed, is accepted ({accepted:?})"
    );
    // A verb taken since on a type its reviewer is shown, reading a field
    // it may not: the bundle is not there to it, as `describe` lists no
    // such verb, so its refusal names nothing.
    l.declare_as(
        declared(json!({"name": "safe", "fields": [{"name": "secret", "type": {"kind": "text"}}]})),
        "`safe`",
    )
    .await?;
    let r = json!({"name": "r", "type": {"kind": "reference", "target": "safe"}});
    l.declare_as(
        declared(
            json!({"name": "pad", "fields": [r, {"name": "state", "type": {"kind": "text"}}]}),
        ),
        "`pad`",
    )
    .await?;
    let peek = write_verb("peek", "pad", "state", "'p'");
    let write = ProposedWrite::Declare {
        declarations: vec![peek],
        plan: None,
    };
    let offer = match l.cat.propose(&carol, write, "mine".into()).await {
        Ok(Outcome::Proposed { proposal, .. }) => proposal,
        other => return Err(Failure::new(format!("`carol` proposing `peek`: {other:?}"))),
    };
    let taken = json!({"declare": "verb", "name": "peek", "on": "pad", "risk": "high",
        "effect": "write", "requires": "r.secret = 'x'",
        "set": [{"field": "state", "value": "'q'"}]});
    l.declare_as(
        serde_json::from_value(taken).expect("a verb reads"),
        "`peek` reading `safe.secret`",
    )
    .await?;
    let mid = super::only::holder(
        s,
        "mid",
        json!({"default": "read", "schema": true, "review": true,
        "types": {"safe": {"fields": {"secret": "none"}}}}),
    );
    let listed = l.cat.proposals(&mid, ProposalFilter::default()).await;
    let listed = listed.must("`mid`'s listing")?.data;
    let absent = Some(Error::NotFound {
        missing: Missing::Proposal { id: offer.clone() },
    });
    let decided = [
        l.cat.accept(&mid, &offer).await.err(),
        l.cat.reject(&mid, &offer, "no".into()).await.err(),
    ];
    ensure!(
        listed.iter().all(|p| p.id != offer) && decided == [absent.clone(), absent],
        "`peek` on `pad`, taken since by a verb reading `safe.secret`, is not there to `mid`, \
         kept from that field: listed {listed:?}, decided {decided:?}"
    );
    Ok(())
}

async fn a_link_type_its_author_is_not_shown_takes_no_field_from_it(store: Store) -> Held {
    use super::only::de;
    use crate::conformance::fixture::head_unmoved;
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let t = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    let deal = |fields: serde_json::Value| -> TypeDecl {
        let with = json!([{"name": "with", "target": "party", "fields": fields}]);
        de(json!({"name": "deal", "fields": [t("name")], "relations": with}))
    };
    l.declare(de(json!({"name": "party", "fields": [t("name")]})))
        .await?;
    l.declare(deal(json!([t("note")]))).await?;
    let types = json!({"sys.link.deal.with": {"default": "none"}});
    let grants = json!({"default": "read", "schema": true, "restricted": true, "types": types});
    let author = super::only::holder(s, "author", grants);
    let declare =
        async |who: &Principal, fields| l.cat.declare(who, Declaration::Type(deal(fields))).await;
    let before = s.head().await.must("reading the head")?;
    let mut told = Vec::new();
    for field in ["note", "memo"] {
        let d = Declaration::Type(deal(json!([t(field)])));
        let write = ProposedWrite::Declare {
            declarations: vec![d.clone()],
            plan: None,
        };
        let moved = migration(deal(json!([t(field)])));
        told.push(declare(&author, json!([t(field)])).await.err());
        told.push(l.cat.apply(&author, vec![d]).await.err());
        told.push(l.cat.propose(&author, write, "mine".into()).await.err());
        told.push(l.cat.migrate(&author, moved).await.err());
    }
    let refused = lakelands_api::Error::Refused {
        reason: lakelands_api::Refusal::NotGranted,
        field: None,
    };
    ensure!(
        told.iter().all(|t| t.as_ref() == Some(&refused)),
        "`with` sent with `note` and with `memo` to a link type its author is not shown is not \
         granted, naming nothing: {told:?}"
    );
    head_unmoved(s, before, "a link field sent to a hidden link type").await?;
    let kept = declare(&author, json!([])).await;
    let added = declare(&l.own, json!([t("note"), t("memo")])).await;
    let link = "sys.link.deal.with".into();
    let link = l.cat.describe_type(&l.own, &link, Default::default()).await;
    let fields = link.must("describing the link type")?.data.fields;
    let names: Vec<_> = fields.iter().map(|f| f.name.as_str()).collect();
    ensure!(
        kept.is_ok() && added.is_ok() && names == ["from", "to", "note", "memo"],
        "`with` without fields is applied ({kept:?}) and keeps `note`, and the owner adds \
         `memo` ({added:?}): the link type holds {names:?}"
    );
    Ok(())
}

check!(
    UNLISTED_NOT_TAKEN = a_declaration_in_force_listed_to_no_one_is_declared_again_by_no_one,
    "027 and 024, extended (lakelands#1055): a verb in force that is listed to no one, as one a log the store did not write holds with a `requires` that does not parse, is not granted to be declared again, naming nothing, to the owner, who reads every field, word for word as to an author kept from a field; nor is a type whose tool would bear the name of such a verb's; and the head does not move",
    given
);

async fn a_declaration_in_force_listed_to_no_one_is_declared_again_by_no_one(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use crate::conformance::fixture::{declared_below, head_unmoved, write_verb};
    let l = Lake::of(store.as_ref());
    l.declare(type_of(
        "job",
        &[("state", StorageKind::Text), ("secret", StorageKind::Text)],
    ))
    .await?;
    // `close` declared below the store, its `requires` not parsing.
    let mut close = write_verb("close", "job", "state", "'shut'");
    if let Declaration::Verb(v) = &mut close {
        v.requires = Some("state = ((".into());
    }
    // And a verb whose tool bears the name of `job2`'s `find` door.
    let door = format!("{}_{}", lakelands_api::tools::tool_base("job2"), "find");
    let mut found = write_verb(&door, "job", "state", "'seen'");
    if let Declaration::Verb(v) = &mut found {
        v.requires = Some("state = ((".into());
    }
    let store = declared_below(store, &given, vec![close.clone(), found]).await?;
    let l = Lake::of(store.as_ref());
    let hidden = [(FieldName::from("secret"), Access::None)].into();
    let mut grants = lakelands_api::Grants {
        default: Access::Write,
        schema: true,
        ..Default::default()
    };
    grants.types.insert(
        "job".into(),
        TypeGrants {
            fields: hidden,
            ..Default::default()
        },
    );
    let kept = crate::conformance::fixture::principal(
        store.as_ref(),
        lakelands_api::PrincipalKind::Agent,
        "kept",
        grants,
    );
    let mut again = write_verb("close", "job", "state", "'done'");
    if let Declaration::Verb(v) = &mut again {
        v.requires = Some("state = 'open'".into());
    }
    let head = store.head().await.must("reading the head")?;
    let by = |who: Principal| {
        let (cat, d) = (l.cat.clone(), again.clone());
        async move { cat.declare(&who, d).await }
    };
    let (owner, other) = (by(l.own.clone()).await, by(kept.clone()).await);
    let refused = Err(lakelands_api::Error::Refused {
        field: None,
        reason: lakelands_api::Refusal::NotGranted,
    });
    ensure!(
        owner == refused && other == refused,
        "`close`, listed to no one, is declared again by neither the owner nor one kept from \
         `job.secret`, each refused as not granted naming nothing; the owner was answered \
         {owner:?}, the other {other:?}"
    );
    // Nor does a type take the name of its tool.
    let job2 = || Declaration::Type(type_of("job2", &[("state", StorageKind::Text)]));
    let (owner, other) = (
        l.cat.declare(&l.own, job2()).await,
        l.cat.declare(&kept, job2()).await,
    );
    ensure!(
        owner == refused && other == refused,
        "`job2`, whose `find` bears the name of a verb's tool, listed to no one, is \
         declared by neither, each refused as not granted naming nothing; the owner was \
         answered {owner:?}, the other {other:?}"
    );
    head_unmoved(store.as_ref(), head, "the refused declarations").await
}

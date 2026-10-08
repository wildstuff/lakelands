//! A migration's shape is its author's view of the type
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md` and
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! each extended): what the author may not know of is not its to remove.

use std::sync::Arc;

use lakelands_api::error::Case;
use lakelands_api::ontology::{Derived, MatchPair, RelationDecl, StorageKind, TypeDecl};
use lakelands_api::principal::TypeGrants;
use lakelands_api::write::{Declaration, Migration, ProposalFilter, ProposedWrite};
use lakelands_api::{Access, Catalog, FieldName, LakeStore, Outcome, Principal, ProposalId};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, alike_renamed, derived, field, field_access, invalid_case, invalid_like, migration,
    reference, refused_on, type_of, value,
};
use crate::links::link_type_name;

check!(
    /// A migration keeps the fields its author may not know of.
    HIDDEN_FIELD_KEPT = a_migration_keeps_the_fields_its_author_may_not_know_of,
    "027 and 024, extended: a migration's `to` is read as its author's view, so a field hidden from the author stays as declared, with its values, whether the migration is applied by a principal with the schema grant or proposed and accepted; `dropped` lists only what the author saw and removed; the proposer reads its own proposal without the field, a reviewer who may see it reads it there"
);

check!(
    /// No refusal of a change of shape names a hidden field.
    HIDDEN_NAME_NOT_LEARNED = a_change_of_shape_never_names_a_field_its_author_may_not_know_of,
    "027 and 024, extended: a type declared again from its author's view is taken, keeping the fields and the link fields hidden from the author, and the relations it is not shown, its reference to a type it is not shown as declared (lakelands#662), where it may know of the field: one held from it names its target in vain, the target answered as one no type is (lakelands#716); a shape that names a hidden field is refused as a field declared twice; one that removes a field a hidden field or relation reads is refused on that field and names no other part; a rename from a hidden field is a rename from no field, and one from a hidden relation from no relation"
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
async fn kept(l: &Lake, want: &[&str], after: &str) -> Result<(), Failure> {
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

async fn a_migration_keeps_the_fields_its_author_may_not_know_of(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
    cat.migrate_apply(&schemer, to, &report.report_id)
        .await
        .must("applying the migration without `team`")?;
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
    cat.accept(own, &proposal)
        .await
        .must("accepting the shape without `note`")?;
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
/// `payroll`, which the author is not shown, nor `paid` and `matched`,
/// which `ref` is read by.
fn seen(fields: &[&str]) -> TypeDecl {
    let mut t = invoice(fields);
    t.fields.push(reference("payee", "payroll"));
    t.relations = vec![approved_by(&["since"])];
    t
}

/// Declare `invoice` whole, with what its author may not know of: `salary`,
/// `net`, which is built from it and `gross`, the link field `grade`, and
/// the relations `paid` and `matched`; and answer that author, `schemer`.
async fn hiding(l: &Lake, store: &dyn LakeStore) -> Result<Principal, Failure> {
    l.declare(type_of("payroll", &[("code", StorageKind::Text)]))
        .await?;
    let mut whole = seen(&["no", "gross", "salary", "ref"]);
    let net = derived("net", StorageKind::Text, "gross", &["gross", "salary"]);
    whole.fields.push(net);
    let pairs = vec![MatchPair {
        from: "ref".into(),
        to: "code".into(),
    }];
    let matched = RelationDecl {
        derived: Some(Derived { pairs }),
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

async fn a_change_of_shape_never_names_a_field_its_author_may_not_know_of(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
        fields == ["no", "gross", "salary", "ref", "memo", "net", "payee"]
            && relations == ["approved_by", "paid", "matched"]
            && link_fields == ["since", "grade"],
        "the type declared again from its author's view must keep `salary`, `net`, `payee`, the \
         link field `grade` and the relations `paid` and `matched`, each at its place; it holds \
         {fields:?}, the relations {relations:?} and the link fields {link_fields:?}"
    );

    let shape = |to: TypeDecl, renames: &[(&str, &str)]| Migration {
        to,
        renames: (renames.iter().map(|(a, b)| ((*a).into(), (*b).into()))).collect(),
        relation_renames: Vec::new(),
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
    /// A migration's faults come in one order for a hidden name and an absent one.
    HIDDEN_NAME_JUDGED_AS_ABSENT = a_migrations_faults_come_in_one_order_for_a_hidden_name_and_an_absent_one,
    "027 and 024, extended (lakelands#750): a migration's renames are judged against its author's view of the type, at the step that judges the lake's own, so a hidden field or relation renamed answers word for word as an absent one, alone and beside a fault judged before or after it; a shape naming a hidden field answers as one naming a field twice, and one also removing a field a hidden field reads is refused on that field first"
);

async fn a_migrations_faults_come_in_one_order_for_a_hidden_name_and_an_absent_one(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let who = hiding(&l, store.as_ref()).await?;
    let m = |fields: &[&str], renames: &[(&str, &str)], relations: &[(&str, &str)]| {
        let mut to = seen(fields);
        for (_, new) in relations {
            to.relations.push(RelationDecl::to(*new, "invoice"));
        }
        Migration {
            to,
            renames: (renames.iter().map(|(a, b)| ((*a).into(), (*b).into()))).collect(),
            relation_renames: (relations.iter().map(|(a, b)| ((*a).into(), (*b).into()))).collect(),
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
    l.cat
        .declare(&l.own, numbered)
        .await
        .must("declaring `numbered`")?;
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
    let answer = l
        .cat
        .migrate(&who, m(&["no", "ref", "salary"], &[], &[]))
        .await;
    let says = json!({"code": "field_read_elsewhere", "field": "gross"});
    ensure!(
        refused_on(&answer).map(|(f, _)| f) == Some("gross") && invalid_like(&answer, &says),
        "a migration naming the hidden `salary` and removing `gross` must be refused on `gross` as \
         {says}; it answered {answer:?}"
    );
    Ok(())
}

check!(
    /// A shape names no field its author may not know of.
    HIDDEN_NAME_NOT_IN_THE_SHAPE = a_shape_names_no_field_its_author_may_not_know_of,
    "027 and 024, extended (lakelands#726): a valid time, a key or a relation's `via` its author gives anew names a field of its view, so a field it may not know of answers word for word as a name no field has, by `declare` and by `migrate`, and the type in force is unmoved; what stands as declared is taken, valid times restated or left out as `describe` gives them and a relation whole but for its rename, while one retargeted answers as to a type that does not exist, and a rename from a relation the author is not shown answers as one from no relation; a valid-time bound over a field its reader may not know of is not described, and a read at a `valid_at`, its total and a `get` answer by the bounds shown, as on a type declared with those alone"
);

async fn a_shape_names_no_field_its_author_may_not_know_of(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, .. } = &l;
    // `veiled` and `cloak` are hidden from the author, and so is `payroll`:
    // `goes`, carried by `cloak`, is not shown to it (lakelands#727), and
    // `pays` is carried by `payee`, which refers to `payroll`. `stamped` is
    // valid from `veiled` to `cloak`.
    fn goes(name: &str, via: &str) -> RelationDecl {
        let via = Some(via.into());
        RelationDecl {
            via,
            ..RelationDecl::to(name, "other")
        }
    }
    let seen = || {
        let mut t = type_of(
            "ev",
            &[("no", StorageKind::Text), ("from", StorageKind::DateTime)],
        );
        t.fields.push(reference("payee", "payroll"));
        t.relations = vec![goes("pays", "payee")];
        t
    };
    let stamped = |more: (&str, StorageKind)| {
        let mut t = type_of("stamped", &[("no", StorageKind::Text), more]);
        (t.valid_from, t.valid_to) = (Some("veiled".into()), Some("cloak".into()));
        t
    };
    let mut whole = seen();
    whole.fields.push(field("veiled", StorageKind::DateTime));
    whole.fields.push(reference("cloak", "other"));
    whole.relations.insert(0, goes("goes", "cloak"));
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
    // What stands as declared is taken: `pays`, the valid times restated,
    // and the valid times left out, as `stamped` is described.
    let memo = stamped(("memo", StorageKind::Text));
    let described = TypeDecl {
        valid_from: None,
        valid_to: None,
        ..memo.clone()
    };
    for t in [seen(), memo, described] {
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
    // A relation renamed keeps its `via`, which stands as declared; one
    // the author is not shown is renamed as no relation.
    let renamed = |from: &str, to: &str, via: &str| {
        let mut t = seen();
        t.relations.push(goes(to, via));
        let relation_renames = vec![(from.into(), to.into())];
        Migration {
            relation_renames,
            ..migration(t)
        }
    };
    let mut answers = Vec::new();
    for from in ["goes", "nothing"] {
        let answer = cat.migrate(&who, renamed(from, "went", "cloak")).await;
        let answer = answer.map_or_else(|e| e.to_string(), |_| "taken".into());
        answers.push(answer.replace(from, "N"));
    }
    ensure!(
        answers[0] == answers[1],
        "a rename from `goes`, which the author is not shown, must answer as one from no \
         relation: {answers:?}"
    );
    let mut paid = renamed("pays", "paid", "payee");
    paid.to.relations.remove(0);
    let report = cat.migrate(&who, paid.clone()).await;
    let report = report.must("renaming `pays`, carried by `payee`")?.data;
    let applied = cat.migrate_apply(&who, paid, &report.report_id).await;
    applied.must("applying the rename of `pays`")?;
    let ev = l.describe_type("ev").await?;
    ensure!(
        ev.relations
            .iter()
            .any(|r| r.name.as_str() == "paid" && r.via == Some("payee".into())),
        "`pays` renamed to `paid` must be carried by `payee` still; `ev` holds {:?}",
        ev.relations
    );
    // `paid` sent to another target stands no more: the hidden `payroll`
    // answers as `zzt`, which is no type.
    let mut answers = Vec::new();
    for door in ["declare", "migrate"] {
        for target in ["payroll", "zzt"] {
            let mut t = seen();
            t.relations = vec![RelationDecl {
                via: Some("payee".into()),
                ..RelationDecl::to("paid", target)
            }];
            let answer = match door {
                "declare" => cat.declare(&who, Declaration::Type(t)).await.map(drop),
                _ => cat.migrate(&who, migration(t)).await.map(drop),
            };
            answers.push(
                answer
                    .map_or_else(|e| e.to_string(), |()| "taken".into())
                    .replace(target, "N"),
            );
        }
    }
    ensure!(
        answers[0] == answers[1]
            && answers[2] == answers[3]
            && !answers.iter().any(|a| a == "taken"),
        "`paid` sent to the hidden `payroll` must answer as to `zzt`, by `declare` and by \
         `migrate`: {answers:?}"
    );
    Ok(())
}

//! A migration's shape is its author's view of the type
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md` and
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! each extended): what the author may not know of is not its to remove.

use std::sync::Arc;

use lakelands_api::ontology::{Derived, MatchPair, RelationDecl, StorageKind, TypeDecl};
use lakelands_api::principal::TypeGrants;
use lakelands_api::write::{Declaration, Migration, ProposalFilter, ProposedWrite};
use lakelands_api::{
    Access, Catalog, FieldName, LakeStore, Outcome, Principal, ProposalId, Refusal,
};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, derived, field, field_access, migration, reference, refused_on, type_of, value,
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

async fn a_change_of_shape_never_names_a_field_its_author_may_not_know_of(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    // `net` is hidden with `salary`, which it is built from; `grade` is a
    // link field of `approved_by`.
    let approved_by = |fields: &[&str]| RelationDecl {
        name: "approved_by".into(),
        target: "invoice".into(),
        fields: fields.iter().map(|f| field(f, StorageKind::Text)).collect(),
        ..Default::default()
    };
    // `payee` refers to `payroll`, which the author is not shown, nor
    // `paid` and `matched`, which `ref` is read by.
    let seen = |fields: &[&str]| {
        let mut t = invoice(fields);
        t.fields.push(reference("payee", "payroll"));
        t.relations = vec![approved_by(&["since"])];
        t
    };
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
    let mut who = without_salary(store.as_ref(), "schemer", true);
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
            "twice",
        ),
        (
            "removing `gross`, which the hidden `net` is built from",
            shape(seen(&["no", "ref", "memo"]), &[]),
            Some("gross"),
            "read by another part",
        ),
        (
            "removing `ref`, which the hidden `matched` reads",
            shape(seen(&["no", "gross", "memo"]), &[]),
            Some("ref"),
            "read by another part",
        ),
        (
            "renaming the hidden `salary`",
            shape(
                seen(&["no", "gross", "ref", "memo", "pay"]),
                &[("salary", "pay")],
            ),
            None,
            "no such field before the migration",
        ),
        (
            "renaming the hidden `paid`",
            settled,
            None,
            "the first names a relation the type has",
        ),
    ] {
        let answer = cat.migrate(&who, m).await;
        let named = refused_on(&answer).map(|(f, _)| f);
        let message = match &answer {
            Err(lakelands_api::Error::Refused {
                reason: Refusal::Invalid { message },
                ..
            }) => message.as_str(),
            _ => "",
        };
        let hidden = ["net", "matched", "payroll"];
        ensure!(
            named == field && message.contains(says) && !hidden.iter().any(|h| message.contains(h)),
            "a migration {what} must be refused as invalid on {field:?}, saying \"{says}\" and \
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
    for target in ["payroll", "ledger", "ghost"] {
        let mut t = seen(&["no", "gross", "ref", "memo"]);
        t.fields.retain(|f| f.name.as_str() != "payee");
        t.fields.push(reference("payee", target));
        let answer = cat.declare(&blind, Declaration::Type(t)).await;
        said.push(
            answer
                .map_or_else(|e| e.to_string(), |_| "taken".into())
                .replace(target, "T"),
        );
    }
    ensure!(
        said.iter().all(|s| s == &said[2] && !s.contains("twice")),
        "an author held from `payee` must be answered alike for `payroll`, another hidden type and \
         no type, never as a field declared twice; it was answered {said:?}"
    );
    let _ = own;
    Ok(())
}

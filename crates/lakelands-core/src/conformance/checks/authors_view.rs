//! A migration's shape is its author's view of the type
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md` and
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! each extended): what the author may not know of is not its to remove.

use std::sync::Arc;

use lakelands_api::ontology::{RelationDecl, StorageKind, TypeDecl};
use lakelands_api::principal::TypeGrants;
use lakelands_api::write::{Declaration, Migration, ProposalFilter, ProposedWrite};
use lakelands_api::{
    Access, Catalog, FieldName, LakeStore, Outcome, Principal, ProposalId, Refusal,
};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, derived, field, field_access, migration, refused_on, type_of, value,
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
    "027 and 024, extended: a type declared again from its author's view is taken, keeping the fields and the link fields hidden from the author; a shape that names a hidden field is refused as a field declared twice; one that removes a field a hidden one is built from is refused on that field and names no other; a rename from a hidden field is a rename from no field"
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
    match listed.into_iter().find(|p| &p.id == id).map(|p| p.write) {
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
    let seen = |fields: &[&str]| TypeDecl {
        relations: vec![approved_by(&["since"])],
        ..invoice(fields)
    };
    let mut whole = seen(&["no", "gross", "salary"]);
    let net = derived("net", StorageKind::Text, "gross", &["gross", "salary"]);
    whole.fields.push(net);
    whole.relations = vec![approved_by(&["since", "grade"])];
    l.declare(whole).await?;
    let mut who = without_salary(store.as_ref(), "schemer", true);
    let grade = [("grade".into(), Access::None)].into();
    let links = TypeGrants {
        default: None,
        fields: grade,
    };
    let link_type = link_type_name(&"invoice".into(), &"approved_by".into());
    who.grants.types.insert(link_type, links);

    // Declared again from the author's view, with one field more.
    cat.declare(&who, Declaration::Type(seen(&["no", "gross", "memo"])))
        .await
        .must("declaring `invoice` again from its author's view, with `memo`")?;
    let after = l.describe_type("invoice").await?;
    let fields: Vec<&str> = after.fields.iter().map(|f| f.name.as_str()).collect();
    let link_fields: Vec<&str> = (after.relations.iter())
        .flat_map(|r| r.fields.iter().map(|f| f.name.as_str()))
        .collect();
    ensure!(
        fields == ["no", "gross", "salary", "net", "memo"] && link_fields == ["since", "grade"],
        "the type declared again from its author's view must keep `salary`, `net` and the link \
         field `grade`, each at its place; it holds {fields:?} and the link fields {link_fields:?}"
    );

    let shape = |to: TypeDecl, renames: &[(&str, &str)]| Migration {
        to,
        renames: (renames.iter().map(|(a, b)| ((*a).into(), (*b).into()))).collect(),
    };
    for (what, m, field, says) in [
        (
            "naming the hidden `salary`",
            shape(seen(&["no", "gross", "memo", "salary"]), &[]),
            Some("salary"),
            "twice",
        ),
        (
            "removing `gross`, which the hidden `net` is built from",
            shape(seen(&["no", "memo"]), &[]),
            Some("gross"),
            "read by another field",
        ),
        (
            "renaming the hidden `salary`",
            shape(seen(&["no", "gross", "memo", "pay"]), &[("salary", "pay")]),
            None,
            "no such field before the migration",
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
        ensure!(
            named == field && message.contains(says) && !message.contains("net"),
            "a migration {what} must be refused as invalid on {field:?}, saying \"{says}\" and \
             not naming `net`; it answered {answer:?}"
        );
    }
    let _ = own;
    Ok(())
}

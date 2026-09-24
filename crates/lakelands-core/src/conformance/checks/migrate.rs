//! Migrations: a preflight first, applied only against the report shown
//! and only when it is lossless
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`).

use std::sync::Arc;

use lakelands_api::ontology::{FieldType, StorageKind, TypeDecl};
use lakelands_api::write::{Migration, MigrationReport};
use lakelands_api::{Error, LakeStore, Refusal};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, declare, doors, field, head_unmoved, owner, read, type_decl, upsert, value,
};

check!(
    /// A migration applies only against the report it was shown.
    APPLY_NEEDS_ITS_REPORT = migrate_apply_requires_the_report_it_was_shown,
    "027: `migrate_apply` names the preflight's report; another report id, or a lake that moved since the report, is refused"
);

check!(
    /// A lossy migration is refused, with its report.
    LOSSY_REFUSED = a_migration_that_would_lose_information_is_refused_with_its_report,
    "027: a migration applies only when every stored value converts without loss; otherwise it is refused and the report names the records"
);

check!(
    /// Values follow the conversion table, and renames keep them.
    CONVERSIONS_AND_RENAMES = migrated_values_follow_the_conversion_table_and_renames_keep_them,
    "027: a number becomes text in its canonical form, numeric text becomes the equal number, and a renamed field keeps its values"
);

async fn memo(store: &dyn LakeStore) -> Result<TypeDecl, Failure> {
    let cat = doors(store);
    let ty = type_decl(
        "memo",
        vec![
            field("title", StorageKind::Text),
            field("pages", StorageKind::Number),
            field("ref", StorageKind::Text),
        ],
    );
    declare(cat.as_ref(), &owner(store), ty.clone()).await?;
    Ok(ty)
}

async fn preflight(store: &dyn LakeStore, m: &Migration) -> Result<MigrationReport, Failure> {
    Ok(doors(store)
        .migrate(&owner(store), m.clone())
        .await
        .must("the preflight of the migration")?
        .data)
}

fn retype(ty: &TypeDecl, name: &str, kind: StorageKind) -> TypeDecl {
    let mut to = ty.clone();
    for f in &mut to.fields {
        if f.name.as_str() == name {
            f.ty = FieldType::kind(kind);
        }
    }
    to
}

async fn migrate_apply_requires_the_report_it_was_shown(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let ty = memo(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    upsert(cat.as_ref(), &own, "memo", "a", json!({"pages": 3})).await?;
    let m = Migration {
        to: retype(&ty, "pages", StorageKind::Text),
        renames: Vec::new(),
    };
    let report = preflight(store.as_ref(), &m).await?;
    ensure!(
        report.lossless && report.records_scanned == 1,
        "the preflight of retyping `pages` from number to text over one record must \
         be lossless and scan one record; it answered {report:?}"
    );

    let before = store.head().await.must("reading the head")?;
    cat.migrate_apply(&own, m.clone(), "not-the-report")
        .await
        .must_fail("applying the migration with a report id it was never shown")?;
    head_unmoved(store.as_ref(), before, "applying with another report id").await?;

    // The lake moves; the report no longer describes it.
    upsert(cat.as_ref(), &own, "memo", "b", json!({"pages": 5})).await?;
    let before = store.head().await.must("reading the head")?;
    cat.migrate_apply(&own, m.clone(), &report.report_id)
        .await
        .must_fail("applying the migration against a report the lake has moved past")?;
    head_unmoved(store.as_ref(), before, "applying a stale report").await?;

    let fresh = preflight(store.as_ref(), &m).await?;
    cat.migrate_apply(&own, m, &fresh.report_id)
        .await
        .must("applying the migration against a fresh report")?;
    Ok(())
}

async fn a_migration_that_would_lose_information_is_refused_with_its_report(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let ty = memo(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    upsert(cat.as_ref(), &own, "memo", "ok", json!({"ref": "12"})).await?;
    upsert(cat.as_ref(), &own, "memo", "bad", json!({"ref": "twelve"})).await?;
    let m = Migration {
        to: retype(&ty, "ref", StorageKind::Number),
        renames: Vec::new(),
    };
    let report = preflight(store.as_ref(), &m).await?;
    let conv = report.fields.iter().find(|f| f.field.as_str() == "ref");
    ensure!(
        !report.lossless
            && conv.is_some_and(|c| c.convertible == 1
                && c.not_convertible == 1
                && c.sample_keys.iter().map(|k| k.as_str()).collect::<Vec<_>>() == ["bad"]),
        "retyping `ref` to number over '12' and 'twelve' must report one convertible \
         value, one that is not, name `bad`, and not be lossless; it reported {report:?}"
    );
    let before = store.head().await.must("reading the head")?;
    let err = cat
        .migrate_apply(&own, m, &report.report_id)
        .await
        .must_fail("applying a lossy migration")?;
    head_unmoved(store.as_ref(), before, "applying a lossy migration").await?;
    ensure!(
        matches!(&err, Error::Refused { reason: Refusal::Migration { report: r }, .. } if !r.lossless),
        "a lossy migration must be refused with its report; it failed with {err:?}"
    );
    Ok(())
}

async fn migrated_values_follow_the_conversion_table_and_renames_keep_them(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let ty = memo(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    upsert(
        cat.as_ref(),
        &own,
        "memo",
        "a",
        json!({"title": "Plan", "pages": 12, "ref": "42"}),
    )
    .await?;

    let mut to = retype(
        &retype(&ty, "pages", StorageKind::Text),
        "ref",
        StorageKind::Number,
    );
    for f in &mut to.fields {
        if f.name.as_str() == "title" {
            f.name = "heading".into();
        }
    }
    let m = Migration {
        to,
        renames: vec![("title".into(), "heading".into())],
    };
    let report = preflight(store.as_ref(), &m).await?;
    cat.migrate_apply(&own, m, &report.report_id)
        .await
        .must("applying the lossless migration")?;

    let r = read(cat.as_ref(), &own, "memo", "a").await?;
    ensure!(
        value(&r, "pages") == Some(&json!("12"))
            && value(&r, "ref") == Some(&json!(42))
            && value(&r, "heading") == Some(&json!("Plan"))
            && value(&r, "title").is_none(),
        "after the migration `a` must read pages \"12\" (a number as canonical text), \
         ref 42 (numeric text as the equal number) and heading \"Plan\" (renamed from \
         title); it reads {:?}",
        r.fields
    );
    Ok(())
}

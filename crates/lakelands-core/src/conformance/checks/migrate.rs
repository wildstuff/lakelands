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

check!(
    /// A relation's `via` cannot be migrated away.
    VIA_REMOVAL_REFUSED = a_migration_removing_the_field_a_relation_walks_is_refused,
    "027 and 018: a relation is carried by its `via` field; a migration that removes that field would leave the relation dangling and is refused"
);

check!(
    /// The preflight names every field a migration drops.
    DROPPED_REPORTED = the_preflight_names_every_dropped_field_and_an_empty_type_migrates_cleanly,
    "027 and MigrationReport.dropped: the preflight names every removed field with how many values it discards, and a type with no records reports nothing to lose"
);

async fn a_migration_removing_the_field_a_relation_walks_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::ontology::{FieldDecl, RelationDecl};
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl("partner", vec![field("name", StorageKind::Text)]),
    )
    .await?;
    let mut invoice = type_decl(
        "invoice",
        vec![
            field("title", StorageKind::Text),
            FieldDecl {
                ty: FieldType {
                    target: Some("partner".into()),
                    ..FieldType::kind(StorageKind::Reference)
                },
                ..field("partner", StorageKind::Text)
            },
        ],
    );
    invoice.relations = vec![RelationDecl {
        name: "billed_by".into(),
        target: "partner".into(),
        cardinality: Default::default(),
        inverse: None,
        via: Some("partner".into()),
        fields: Vec::new(),
        authority: Default::default(),
        description: None,
    }];
    declare(cat.as_ref(), &own, invoice.clone()).await?;
    let mut to = invoice;
    to.fields.retain(|f| f.name.as_str() != "partner");
    let m = Migration {
        to,
        renames: Vec::new(),
    };
    let before = store.head().await.must("reading the head")?;
    let refused = match cat.migrate(&own, m.clone()).await {
        Err(_) => true,
        Ok(report) => cat
            .migrate_apply(&own, m, &report.data.report_id)
            .await
            .is_err(),
    };
    head_unmoved(store.as_ref(), before, "migrating away a relation's `via`").await?;
    ensure!(
        refused,
        "removing `partner`, the `via` of the relation `billed_by`, must be refused, at \
         the preflight or when applied; it was applied"
    );
    Ok(())
}

async fn the_preflight_names_every_dropped_field_and_an_empty_type_migrates_cleanly(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let wide = type_decl(
        "memo",
        vec![
            field("title", StorageKind::Text),
            field("pages", StorageKind::Number),
            field("ref", StorageKind::Text),
        ],
    );
    declare(cat.as_ref(), &own, wide.clone()).await?;
    let mut narrow = wide.clone();
    narrow.fields.retain(|f| f.name.as_str() == "title");
    let m = Migration {
        to: narrow.clone(),
        renames: Vec::new(),
    };
    let empty = preflight(store.as_ref(), &m).await?;
    ensure!(
        empty.records_scanned == 0 && empty.fields.is_empty() && empty.lossless,
        "the preflight over a type with no records must scan none, convert nothing \
         and be lossless; it answered {empty:?}"
    );

    upsert(
        cat.as_ref(),
        &own,
        "memo",
        "a",
        json!({"title": "A", "pages": 3, "ref": "x"}),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &own,
        "memo",
        "b",
        json!({"title": "B", "pages": 5}),
    )
    .await?;
    upsert(cat.as_ref(), &own, "memo", "c", json!({"title": "C"})).await?;
    let report = preflight(store.as_ref(), &m).await?;
    let mut dropped: Vec<(String, u64)> = report
        .dropped
        .iter()
        .map(|(f, n)| (f.to_string(), *n))
        .collect();
    dropped.sort();
    ensure!(
        dropped == [("pages".to_owned(), 2), ("ref".to_owned(), 1)] && report.records_scanned == 3,
        "dropping `pages` (held by two records) and `ref` (held by one) must be \
         reported as pages 2, ref 1 over three records; it reported {:?} over {}",
        report.dropped,
        report.records_scanned
    );
    Ok(())
}

//! Migrations: a preflight first, applied only against the report shown
//! and only when it is lossless
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`).

use std::sync::Arc;

use lakelands_api::ontology::{FieldType, StorageKind, TypeDecl};
use lakelands_api::write::{Migration, MigrationReport};
use lakelands_api::{LakeStore, Refusal};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, count_view, declare, doors, field, head_unmoved, measure, migration, owner, reason,
    type_decl, value,
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

/// `m` preflighted, then applied as its report says: the first refusal.
async fn migrated(l: &Lake, m: Migration) -> lakelands_api::Result<()> {
    let report = l.cat.migrate(&l.own, m.clone()).await?;
    let applied = l.cat.migrate_apply(&l.own, m, &report.data.report_id);
    applied.await.map(|_| ())
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
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.upsert("memo", "a", json!({"pages": 3})).await?;
    let m = migration(retype(&ty, "pages", StorageKind::Text));
    let report = preflight(store.as_ref(), &m).await?;
    ensure!(
        report.lossless && report.records_scanned == 1,
        "the preflight of retyping `pages` from number to text over one record must \
         be lossless and scan one record; it answered {report:?}"
    );

    let before = store.head().await.must("reading the head")?;
    cat.migrate_apply(own, m.clone(), "not-the-report")
        .await
        .must_fail("applying the migration with a report id it was never shown")?;
    head_unmoved(store.as_ref(), before, "applying with another report id").await?;

    // The lake moves; the report no longer describes it.
    l.upsert("memo", "b", json!({"pages": 5})).await?;
    let before = store.head().await.must("reading the head")?;
    cat.migrate_apply(own, m.clone(), &report.report_id)
        .await
        .must_fail("applying the migration against a report the lake has moved past")?;
    head_unmoved(store.as_ref(), before, "applying a stale report").await?;

    let fresh = preflight(store.as_ref(), &m).await?;
    cat.migrate_apply(own, m, &fresh.report_id)
        .await
        .must("applying the migration against a fresh report")?;
    Ok(())
}

async fn a_migration_that_would_lose_information_is_refused_with_its_report(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let ty = memo(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.upsert("memo", "ok", json!({"ref": "12"})).await?;
    l.upsert("memo", "bad", json!({"ref": "twelve"})).await?;
    let m = migration(retype(&ty, "ref", StorageKind::Number));
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
        .migrate_apply(own, m, &report.report_id)
        .await
        .must_fail("applying a lossy migration")?;
    head_unmoved(store.as_ref(), before, "applying a lossy migration").await?;
    ensure!(
        matches!(reason(&err), Some(Refusal::Migration { report: r }) if !r.lossless),
        "a lossy migration must be refused with its report; it failed with {err:?}"
    );
    Ok(())
}

async fn migrated_values_follow_the_conversion_table_and_renames_keep_them(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let ty = memo(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.upsert(
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
    cat.migrate_apply(own, m, &report.report_id)
        .await
        .must("applying the lossless migration")?;

    let r = l.read("memo", "a").await?;
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

check!(
    /// A view is never migrated.
    VIEW_NOT_MIGRATED = a_view_is_never_migrated,
    "027, extended: a migration reshapes stored records, and a view stores none, so a migration that would turn a stored type into a view, or a view into a stored type, is refused as invalid naming the type, at the preflight or when applied, and nothing is appended; a view changes, and becomes a stored type whatever its fields become, by being declared again"
);

async fn a_view_is_never_migrated(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl(
        "invoice",
        vec![
            field("partner", StorageKind::Text),
            field("amount", StorageKind::Decimal),
        ],
    ))
    .await?;
    let mut tally = type_decl(
        "tally",
        vec![
            field("partner", StorageKind::Text),
            field("n", StorageKind::Number),
        ],
    );
    tally.key = vec!["partner".into()];
    l.declare(tally.clone()).await?;
    l.upsert("tally", "zz", json!({"partner": "zz", "n": 1}))
        .await?;
    l.declare(count_view("pt", "invoice", "partner")).await?;
    let mut stored_pt = type_decl(
        "pt",
        vec![
            field("partner", StorageKind::Text),
            field("n", StorageKind::Number),
        ],
    );
    stored_pt.key = vec!["partner".into()];
    for (ty, to) in [
        ("tally", count_view("tally", "invoice", "partner")),
        ("pt", stored_pt),
    ] {
        let before = store.head().await.must("reading the head")?;
        let answer = migrated(&l, migration(to)).await;
        ensure!(
            matches!(reason(&answer), Some(Refusal::Invalid { message }) if message.contains(ty)),
            "a migration of `{ty}` across a view and a stored type must be refused as invalid, \
             naming `{ty}`; it answered {answer:?}"
        );
        head_unmoved(store.as_ref(), before, &format!("migrating `{ty}`")).await?;
    }
    // A view changes by being declared again.
    let mut narrowed = count_view("pt", "invoice", "partner");
    narrowed.view.as_mut().expect("a view").r#where = Some("amount > 0".into());
    l.declare(narrowed).await?;
    // And becomes a stored type by being declared as one, with a required
    // field it never computed: it stored nothing, so nothing is reshaped.
    let mut stored = type_decl(
        "pt",
        vec![
            field("partner", StorageKind::Text),
            field("n", StorageKind::Number),
            lakelands_api::ontology::FieldDecl {
                required: true,
                ..field("owner", StorageKind::Text)
            },
        ],
    );
    stored.key = vec!["partner".into()];
    l.declare(stored).await?;
    Ok(())
}

check!(
    /// A migration leaves every view over the type readable.
    VIEW_SOURCE_KEPT = a_migration_taking_a_field_a_view_reads_is_refused,
    "027 and 028, extended: a view over a type, as its source or a merged one, reads the shape a migration leaves, so the preflight refuses a migration that removes or renames a field the view reads, naming the view and the field as `declare` would refuse the view against the new shape; a field no view reads migrates as before"
);

async fn a_migration_taking_a_field_a_view_reads_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::ontology::{ViewDecl, ViewSource};
    use lakelands_api::{FieldName, TypeName};
    let l = Lake::of(store.as_ref());
    let invoice = type_decl(
        "invoice",
        vec![
            field("partner", StorageKind::Text),
            field("amount", StorageKind::Decimal),
            field("note", StorageKind::Text),
        ],
    );
    let credit = type_decl(
        "credit",
        vec![
            field("partner", StorageKind::Text),
            field("amount", StorageKind::Decimal),
        ],
    );
    l.declare(invoice.clone()).await?;
    l.declare(credit.clone()).await?;
    l.upsert(
        "invoice",
        "i1",
        json!({"partner": "p1", "amount": "10.00", "note": "n"}),
    )
    .await?;
    l.upsert("credit", "c1", json!({"partner": "p1", "amount": "2.00"}))
        .await?;
    let by_partner = vec![FieldName::from("partner")];
    l.declare(TypeDecl {
        key: by_partner.clone(),
        view: Some(Box::new(ViewDecl {
            source: TypeName::from("invoice"),
            params: Vec::new(),
            r#where: None,
            group_by: by_partner.clone(),
            measures: vec![measure("total", "sum(amount)"), measure("n", "count()")],
            merge: vec![ViewSource {
                source: TypeName::from("credit"),
                r#where: None,
                group_by: by_partner,
                measures: vec![measure("credited", "sum(amount)")],
            }],
        })),
        ..type_decl(
            "pt",
            vec![
                field("partner", StorageKind::Text),
                field("total", StorageKind::Decimal),
                field("n", StorageKind::Number),
                field("credited", StorageKind::Decimal),
            ],
        )
    })
    .await?;
    let renamed = |t: &TypeDecl, from: &str, to: &str| {
        let mut t = t.clone();
        for f in &mut t.fields {
            if f.name.as_str() == from {
                f.name = to.into();
            }
        }
        Migration {
            to: t,
            renames: vec![(from.into(), to.into())],
        }
    };
    let dropped = |t: &TypeDecl, name: &str| {
        let mut t = t.clone();
        t.fields.retain(|f| f.name.as_str() != name);
        migration(t)
    };
    let migrated = |m: Migration| migrated(&l, m);
    for (what, m, read) in [
        (
            "renaming `partner` of `invoice`",
            renamed(&invoice, "partner", "counterparty"),
            "partner",
        ),
        (
            "dropping `amount` of `invoice`",
            dropped(&invoice, "amount"),
            "amount",
        ),
        (
            "renaming `partner` of `credit`",
            renamed(&credit, "partner", "who"),
            "partner",
        ),
        (
            "dropping `amount` of `credit`",
            dropped(&credit, "amount"),
            "amount",
        ),
    ] {
        let before = store.head().await.must("reading the head")?;
        let answer = migrated(m).await;
        ensure!(
            matches!(reason(&answer), Some(Refusal::Invalid { message })
                if message.contains("pt") && message.contains(read)),
            "{what}, which the view `pt` reads, must be refused as invalid, naming `pt` and \
             `{read}`; it answered {answer:?}"
        );
        head_unmoved(store.as_ref(), before, what).await?;
    }
    // A field no view reads migrates as before.
    let answer = migrated(dropped(&invoice, "note")).await;
    ensure!(
        answer.is_ok(),
        "dropping `note`, which no view reads, must migrate; it answered {answer:?}"
    );
    Ok(())
}

async fn a_migration_removing_the_field_a_relation_walks_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::ontology::{FieldDecl, RelationDecl};
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("partner", vec![field("name", StorageKind::Text)]))
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
        via: Some("partner".into()),
        ..Default::default()
    }];
    l.declare(invoice.clone()).await?;
    let mut to = invoice;
    to.fields.retain(|f| f.name.as_str() != "partner");
    let before = store.head().await.must("reading the head")?;
    let refused = migrated(&l, migration(to)).await.is_err();
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
    let l = Lake::of(store.as_ref());
    let wide = memo(store.as_ref()).await?;
    let mut narrow = wide.clone();
    narrow.fields.retain(|f| f.name.as_str() == "title");
    let m = migration(narrow.clone());
    let empty = preflight(store.as_ref(), &m).await?;
    ensure!(
        empty.records_scanned == 0 && empty.fields.is_empty() && empty.lossless,
        "the preflight over a type with no records must scan none, convert nothing \
         and be lossless; it answered {empty:?}"
    );

    l.upsert("memo", "a", json!({"title": "A", "pages": 3, "ref": "x"}))
        .await?;
    l.upsert("memo", "b", json!({"title": "B", "pages": 5}))
        .await?;
    l.upsert("memo", "c", json!({"title": "C"})).await?;
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

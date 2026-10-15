//! Migrations: a preflight first, applied only against the report shown
//! and only when it is lossless
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`).

use lakelands_api::error::Case;
use lakelands_api::ontology::{FieldType, StorageKind, TypeDecl};
use lakelands_api::write::{Migration, MigrationReport};
use lakelands_api::{LakeStore, Refusal};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, count_view, declare, doors, field, head_unmoved, invalid_case, invalid_like,
    measure, migration, owner, reason, type_decl, value,
};
use crate::conformance::{Failure, Held, Store};

check!(
    FILLED = a_migration_fills_a_fields_initial_into_the_records_that_hold_none,
    "050 and 027, extended (lakelands#1023): a migration naming a field in `fill` writes its `initial`, of origin `initial` at the migration's position, into each live record that holds no value for it, a restricted one too and a retired one not, which takes it when written again; its preflight counts them in `filled`, naming only keys its reader is answered, and a field made required with `initial` is lossless with `fill` and refused without; an aging rule then moves the old records as the new; `fill` naming a field `to` does not declare, one without `initial`, one twice, or a renamed field by its old name is refused before a record is read; a trigger whose guard reads the field does not fire at the migration, as for every record a migration converts; and a rebuild and an imported copy answer the values and their origin alike",
    given
);

check!(
    FILL_IN_FORCE = a_fill_alone_fills_the_type_in_force,
    "050 and 027, extended (lakelands#1104): a migration to the type in force that names a field in `fill` fills each live record holding none, through `migrate_apply` and through an accepted proposal alike, with origin `initial` at its position, a record held from its author too, whose preview names only the keys that author is shown; the type's version and schema hash move as a fill inside a changing migration moves them; the same migration again counts no record and its apply appends nothing"
);

check!(
    APPLY_NEEDS_ITS_REPORT = migrate_apply_requires_the_report_it_was_shown,
    "027: `migrate_apply` names the preflight's report; another report id, or a lake that moved since the report, is refused, after the preflight's own refusals (027, extended)"
);

check!(
    LOSSY_REFUSED = a_migration_that_would_lose_information_is_refused_with_its_report,
    "027: a migration applies only when every stored value converts without loss; otherwise it is refused and the report names the records, after its report id is compared (027, extended)"
);

check!(
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

async fn migrate_apply_requires_the_report_it_was_shown(store: Store) -> Held {
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
    // The preflight before the report: an undeclared type is not found
    // whatever report is named (027, extended).
    let missing = migration(lakelands_api::ontology::TypeDecl::named("nothing"));
    let first = cat.migrate_apply(own, missing, "not-the-report").await;
    ensure!(
        matches!(first, Err(lakelands_api::Error::NotFound { .. })),
        "a migration of an undeclared type is refused as not found before its report is \
         compared; it answered {first:?}"
    );

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

check!(
    DIGITS_BEYOND_EXACT_NOT_CONVERTIBLE =
        text_digits_no_reader_reads_exactly_are_reported_as_not_convertible_to_a_number,
    "027 and the log format §1.2: a migration converts a value only into one its new field would take written, so text digits beyond ±(2^53 − 1) (`9007199254740992`, its negative, `1e16`) are reported as not convertible to a number, naming their record, and the report is not lossless; over ±(2^53 − 1) alone it is lossless, applies, and the numbers read back exactly"
);

async fn text_digits_no_reader_reads_exactly_are_reported_as_not_convertible_to_a_number(
    store: Store,
) -> Held {
    let ty = memo(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let max = 9_007_199_254_740_991_i64;
    l.upsert("memo", "exact", json!({"ref": max.to_string()}))
        .await?;
    let m = migration(retype(&ty, "ref", StorageKind::Number));
    for digits in ["9007199254740992", "-9007199254740992", "1e16"] {
        l.upsert("memo", "beyond", json!({"ref": digits})).await?;
        let report = preflight(store.as_ref(), &m).await?;
        let conv = report.fields.iter().find(|f| f.field.as_str() == "ref");
        ensure!(
            !report.lossless
                && conv.is_some_and(|c| c.convertible == 1
                    && c.not_convertible == 1
                    && c.sample_keys.iter().map(|k| k.as_str()).collect::<Vec<_>>() == ["beyond"]),
            "retyping `ref` to number over '{max}' and '{digits}' must report one convertible \
             value, one that is not, name `beyond`, and not be lossless: a number field takes \
             no integer beyond ±(2^53 − 1); it reported {report:?}"
        );
    }
    l.upsert("memo", "beyond", json!({"ref": (-max).to_string()}))
        .await?;
    let moved = migrated(&l, m).await;
    moved.must("migrating over ±(2^53 − 1) alone")?;
    for (key, n) in [("exact", max), ("beyond", -max)] {
        let read = l.read("memo", key).await?;
        ensure!(
            value(&read, "ref") == Some(&json!(n)),
            "'{n}' migrated to a number must read {n}; it reads {:?}",
            value(&read, "ref")
        );
    }
    Ok(())
}

async fn a_migration_that_would_lose_information_is_refused_with_its_report(store: Store) -> Held {
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
    let err = cat.migrate_apply(own, m.clone(), &report.report_id);
    let err = err.await.must_fail("applying a lossy migration")?;
    // The report before the loss: another report id is refused as such.
    let other = cat.migrate_apply(own, m, "not-the-report").await;
    head_unmoved(store.as_ref(), before, "applying a lossy migration").await?;
    ensure!(
        matches!(reason(&err), Some(Refusal::Migration { report: r }) if !r.lossless)
            && matches!(reason(&other), Some(Refusal::Invalid(..))),
        "a lossy migration must be refused with its report, and under another report id \
         for that id first (027, extended); it failed with {err:?} and {other:?}"
    );
    Ok(())
}

async fn migrated_values_follow_the_conversion_table_and_renames_keep_them(store: Store) -> Held {
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
        renames: vec![("title".into(), "heading".into())],
        ..Migration::to(to)
    };
    let report = preflight(store.as_ref(), &m).await?;
    let fut = cat.migrate_apply(own, m, &report.report_id);
    fut.await.must("applying the lossless migration")?;

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
    VIA_REMOVAL_REFUSED = a_migration_removing_the_field_a_relation_walks_is_refused,
    "027 and 018: a relation is carried by its `via` field; a migration that removes that field would leave the relation dangling and is refused"
);

check!(
    DROPPED_REPORTED = the_preflight_names_every_dropped_field_and_an_empty_type_migrates_cleanly,
    "027 and MigrationReport.dropped: the preflight names every removed field with how many values it discards, and a type with no records reports nothing to lose; the report is lossless and the apply discards them"
);

check!(
    VIEW_NOT_MIGRATED = a_view_is_never_migrated,
    "027, extended: a migration reshapes stored records, and a view stores none, so a migration that would turn a stored type into a view, or a view into a stored type, is refused as invalid naming the type, at the preflight or when applied, and nothing is appended; a view changes, and becomes a stored type whatever its fields become, by being declared again"
);

async fn a_view_is_never_migrated(store: Store) -> Held {
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
            matches!(invalid_case(&answer),
                Some(Case::MigrationToView { ty: t } | Case::MigrationViewToStored { ty: t })
                    if t == ty),
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
    VIEW_SOURCE_KEPT = a_migration_taking_a_field_a_view_reads_is_refused,
    "027 and 028, extended: a view over a type, as its source or a merged one, reads the shape a migration leaves, so the preflight refuses a migration that removes or renames a field the view reads, naming the view and the field as `declare` would refuse the view against the new shape; a field no view reads migrates as before"
);

async fn a_migration_taking_a_field_a_view_reads_is_refused(store: Store) -> Held {
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
            renames: vec![(from.into(), to.into())],
            ..Migration::to(t)
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
            invalid_like(
                &answer,
                &json!({"code": "unknown_field", "field": read,
                                          "at": [{"code": "type", "name": "pt"}]})
            ),
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

async fn a_migration_removing_the_field_a_relation_walks_is_refused(store: Store) -> Held {
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
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let wide = memo(store.as_ref()).await?;
    let mut narrow = wide.clone();
    narrow.fields.retain(|f| f.name.as_str() == "title");
    let m = migration(narrow.clone());
    let empty = preflight(store.as_ref(), &m).await?;
    ensure!(
        empty.records_scanned + empty.discards() == 0 && empty.fields.is_empty() && empty.lossless,
        "the preflight over a type with no records must scan none, convert nothing, \
         be lossless and discard nothing; it answered {empty:?}"
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
        dropped == [("pages".to_owned(), 2), ("ref".to_owned(), 1)]
            && report.records_scanned == 3
            && report.lossless
            && report.discards() == 3,
        "dropping `pages` (held by two records) and `ref` (held by one) must be reported as \
         pages 2, ref 1 over three records, lossless, discarding 3; it reported {report:?}"
    );
    migrated(&l, m).await.must("the apply")?;
    let a = l.read("memo", "a").await?.fields;
    ensure!(
        a.len() == 1 && a.get("title") == Some(&json!("A")),
        "the apply discards what `dropped` counts: `a` must hold its title alone; it holds {a:?}"
    );
    Ok(())
}

async fn a_migration_fills_a_fields_initial_into_the_records_that_hold_none(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use crate::conformance::checks::export::Scratch;
    use crate::conformance::checks::report_names::preview;
    use crate::conformance::fixture::{events_from, field_access, principal, restrict};
    use lakelands_api::event::EventBody;
    use lakelands_api::principal::{Access, Grants, PrincipalKind};
    use lakelands_api::record::Origin;
    use lakelands_api::write::{Declaration, Outcome, ProposedWrite, WriteOpts};
    use serde_json::Value;
    let l = Lake::of(store.as_ref());
    let s = store.as_ref();
    // `status` is declared before it has an `initial`: the records hold
    // none. `loud` and `shout` are computed from it.
    let computed: Vec<lakelands_api::ontology::FieldDecl> = serde_json::from_value(json!([
        {"name": "loud", "type": {"kind": "text"}, "authority": "derived",
         "derived": "concat(status, '!')", "depends_on": ["status"]},
        {"name": "shout", "type": {"kind": "text"}, "authority": "derived",
         "function": "shout", "depends_on": ["status"]}]))
    .unwrap();
    let task = |initial: Option<serde_json::Value>| {
        let mut status = field("status", StorageKind::Text);
        status.initial = initial;
        let fields = [
            vec![field("title", StorageKind::Text), status],
            computed.clone(),
        ];
        type_decl("task", fields.concat())
    };
    l.declare_json(
        json!({"declare": "function", "name": "shout", "backing": "expr",
        "inputs": [{"name": "status", "type": {"kind": "text"}}], "expr": "upper(status)"}),
    )
    .await?;
    l.declare(task(None)).await?;
    for k in ["t1", "t2", "t4"] {
        l.upsert("task", k, json!({"title": k})).await?;
    }
    l.upsert("task", "t0", json!({"title": "t0", "status": "done"}))
        .await?;
    restrict(s, "task", "t2", "held").await?;
    l.retire("task", "t4").await?;
    let opened =
        json!({"declare": "trigger", "name": "opened", "on": "task", "when": "status = 'open'"});
    l.cat
        .declare(
            &l.own,
            serde_json::from_value::<Declaration>(opened).unwrap(),
        )
        .await
        .must("the trigger")?;
    l.declare_json(
        json!({"declare": "object_set", "name": "open_tasks", "of": "task",
        "where": "status = 'open'"}),
    )
    .await?;
    let mut wrong = Vec::new();
    // The refusals, before a record is read.
    let m = |fill: &[&str], renames: &[(&str, &str)]| Migration {
        fill: fill.iter().map(|f| (*f).into()).collect(),
        renames: renames
            .iter()
            .map(|(a, b)| ((*a).into(), (*b).into()))
            .collect(),
        ..migration(task(Some(json!("open"))))
    };
    let mut bare = migration(task(None));
    bare.fill = vec!["status".into()];
    let mut renamed = m(&["title"], &[("title", "heading")]);
    renamed.to.fields[0].name = "heading".into();
    for (what, migration) in [
        ("an undeclared field", m(&["nope"], &[])),
        ("a field without `initial`", bare),
        ("a field twice", m(&["status", "status"], &[])),
        ("a renamed field's old name", renamed),
    ] {
        let refused = l.cat.migrate(&l.own, migration).await;
        if !matches!(
            invalid_case(&refused),
            Some(
                Case::FillUnknown { .. }
                    | Case::FillNoInitial { .. }
                    | Case::FillRepeated { .. }
                    | Case::FillRenamed { .. }
            )
        ) {
            wrong.push(format!("filling {what}: {refused:?}"));
        }
    }
    // An author kept from `b` is refused it as a field that is not there,
    // with or without its `initial`; a swap fills a name `to` still has.
    let (a, mut b) = (field("a", StorageKind::Text), field("b", StorageKind::Text));
    b.initial = Some(json!("x"));
    l.declare(type_decl("pad", vec![a.clone(), b.clone()]))
        .await?;
    let mut hider = field_access(s, "hider", "pad", &[("b", Access::None)]);
    hider.grants.schema = true;
    let fills = |f: &str| Migration {
        fill: vec![f.into()],
        ..migration(type_decl("pad", vec![a.clone()]))
    };
    let hidden = format!("{:?}", l.cat.migrate(&hider, fills("b")).await);
    let hidden = hidden.replace("`b`", "`nope`").replace("\"b\"", "\"nope\"");
    let absent = format!("{:?}", l.cat.migrate(&hider, fills("nope")).await);
    let swap = Migration {
        fill: vec!["b".into()],
        renames: vec![("a".into(), "b".into()), ("b".into(), "a".into())],
        ..migration(type_decl("pad", vec![a, b]))
    };
    let swapped = l.cat.migrate(&l.own, swap).await;
    if hidden != absent || !absent.contains("FillUnknown") || swapped.is_err() {
        wrong.push(format!(
            "kept from `b`: {hidden}, where {absent}; a swap: {swapped:?}"
        ));
    }
    // Required with `initial`: lossy without `fill`, lossless with it.
    let mut required = task(Some(json!("open")));
    required
        .fields
        .iter_mut()
        .for_each(|f| f.required |= f.name.as_str() == "status");
    let lossy = preview(&l, &l.own, migration(required.clone())).await?;
    let filling = Migration {
        fill: vec!["status".into()],
        ..migration(required)
    };
    // A reviewer kept from `status` is listed neither its fill nor its count.
    let mut blind = field_access(s, "blind", "task", &[("status", Access::None)]);
    (blind.grants.schema, blind.grants.review) = (true, true);
    let migration = Box::new(filling.clone());
    let write = ProposedWrite::Migrate {
        migration,
        report: Default::default(),
    };
    let proposed = l.cat.propose(&l.own, write, "fill".into()).await;
    proposed.must("proposing the fill")?;
    let listed = l.cat.proposals(&blind, Default::default()).await;
    let blind: Vec<_> = (listed.must("listing")?.data.into_iter())
        .filter_map(|p| match p.write {
            Some(ProposedWrite::Migrate { migration, report }) => {
                Some((migration.fill, report.filled))
            }
            _ => None,
        })
        .collect();
    let report = preview(&l, &l.own, filling.clone()).await?;
    // A reader with `schema` but no `restricted` is told the count, not `t2`.
    let grants = Grants {
        schema: true,
        restricted: false,
        ..Grants::owner()
    };
    let schemer = principal(s, PrincipalKind::Agent, "schemer", grants);
    let told = preview(&l, &schemer, filling.clone()).await?;
    let filled = |r: &MigrationReport| {
        r.filled
            .iter()
            .map(|f| format!("{} {} {:?}", f.field, f.records, f.sample_keys))
            .collect::<Vec<_>>()
    };
    if lossy.lossless
        || !report.lossless
        || filled(&report) != [r#"status 2 [RecordKey("t1"), RecordKey("t2")]"#]
        || filled(&told) != [r#"status 2 [RecordKey("t1")]"#]
        || !told.more
        || blind != [(vec![], vec![])]
    {
        wrong.push(format!(
            "the preflights: lossy {}, filling {} {:?}, told {:?} more {}, blind {blind:?}",
            lossy.lossless,
            report.lossless,
            filled(&report),
            filled(&told),
            told.more
        ));
    }
    let applied = l
        .cat
        .migrate_apply(&l.own, filling, &report.report_id)
        .await
        .must("the apply")?;
    let Outcome::Applied {
        position: at,
        set_off,
        ..
    } = applied
    else {
        return Err(Failure::new(format!("the apply answered {applied:?}")));
    };
    let fired = set_off.fired;
    // The records the fill moves into a set join it in the same append.
    let joined: Vec<String> = (events_from(s, at).await?.into_iter())
        .filter_map(|e| match e.body {
            EventBody::ObjectSetChanged {
                set,
                key,
                joined: true,
                ..
            } if set == "open_tasks" => Some(key.to_string()),
            _ => None,
        })
        .collect();
    l.upsert("task", "t3", json!({"title": "t3"})).await?;
    l.upsert("task", "t4", json!({"title": "t4"})).await?;
    // Each record's status and its origin, as `who` reads it.
    let read = |who: lakelands_api::Principal, s: &dyn LakeStore| {
        let cat = s.catalog(Default::default());
        async move {
            let mut out = Vec::new();
            for k in ["t0", "t1", "t2", "t3", "t4"] {
                let get = serde_json::from_value(json!({"key": k, "with_meta": true}));
                let get = get.map_err(|e| Failure::new(e.to_string()))?;
                let r = cat
                    .get(&who, &"task".into(), get)
                    .await
                    .must("reading a task")?
                    .data;
                let meta = r.meta.get("status");
                let filled = meta.is_some_and(|m| m.origin == Origin::Initial && m.position == at);
                let shown = |f: &str| r.fields.get(f).map_or("-".into(), Value::to_string);
                let computed = format!("{} {}", shown("loud"), shown("shout"));
                out.push(format!("{k} {} {filled} {computed}", shown("status")));
            }
            Ok::<_, Failure>(out)
        }
    };
    let own = owner(s);
    let here = read(own.clone(), s).await?;
    let (open, done) = ("\"open\"", "t0 \"done\" false \"done!\" \"DONE\"");
    let filled = |k: &str, at: bool| format!("{k} {open} {at} \"open!\" \"OPEN\"");
    let want = [done.into(), filled("t1", true), filled("t2", true)];
    let want = [&want[..], &[filled("t3", false), filled("t4", false)]].concat();
    if here != want || !fired.is_empty() || joined != ["t1", "t2"] {
        wrong.push(format!(
            "after the fill: {here:?}, the trigger fired {fired:?}, `open_tasks` joined {joined:?}"
        ));
    }
    // A rebuild and an imported copy read the fill and its origin alike.
    s.rebuild().await.must("rebuilding")?;
    let rebuilt = read(own.clone(), s).await?;
    let scratch = Scratch::new(given.scratch.clone());
    let from = scratch.dir("from");
    crate::export::write(s, &from).await.must("exporting")?;
    let copy = (given.import)(from, store.lake().clone(), None, Default::default()).await;
    let copy = copy
        .map_err(|e| Failure::new(format!("importing: {e}")))?
        .store;
    let imported = read(owner(copy.as_ref()), copy.as_ref()).await?;
    if rebuilt != here || imported != here {
        wrong.push(format!(
            "rebuilt {rebuilt:?} and imported {imported:?}, where {here:?}"
        ));
    }
    // An aging rule moves the old records with the new.
    let rule = json!({"name": "ages", "on": "task", "field": "status", "from": "open", "to": "late", "when": "true"});
    let rule = serde_json::from_value(rule).unwrap();
    l.cat
        .upsert(
            &l.own,
            &"sys.aging_rule".into(),
            &"ages".into(),
            rule,
            WriteOpts::default(),
        )
        .await
        .must("the rule")?;
    let aged = s
        .age(lakelands_api::Timestamp(1_799_971_200_000_000))
        .await
        .must("aging")?;
    let moved: u32 = aged.rules.iter().map(|r| r.moved).sum();
    if moved != 4 {
        wrong.push(format!("the aging rule moved {moved} of 4"));
    }
    ensure!(wrong.is_empty(), "{}", wrong.join("; "));
    Ok(())
}

async fn a_fill_alone_fills_the_type_in_force(store: Store) -> Held {
    use crate::conformance::checks::report_names::preview;
    use crate::conformance::fixture::{principal, restrict};
    use lakelands_api::principal::{Grants, PrincipalKind};
    use lakelands_api::record::Origin;
    use lakelands_api::write::{Outcome, ProposedWrite};
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let with = |n: &str| {
        let mut f = field(n, StorageKind::Text);
        f.initial = Some(json!("open"));
        f
    };
    let no = field("no", StorageKind::Text);
    l.declare(type_decl("t", vec![no.clone()])).await?;
    for k in ["f1", "f2"] {
        l.upsert("t", k, json!({"no": k})).await?;
    }
    restrict(s, "t", "f2", "held").await?;
    // `status` and `size` are added with an `initial`, and fill nothing.
    let t = type_decl("t", vec![no, with("status"), with("size")]);
    l.migrate(t.clone()).await?;
    let life = || async {
        let d = l.describe_type("t").await?;
        Ok::<_, Failure>((d.lifecycle.version, d.lifecycle.schema_hash))
    };
    let fill = |f: &str| Migration {
        fill: vec![f.into()],
        ..migration(t.clone())
    };
    let grants = Grants {
        schema: true,
        restricted: false,
        ..Grants::owner()
    };
    let author = principal(s, PrincipalKind::Agent, "author", grants);
    let before = life().await?;
    let report = preview(&l, &author, fill("status")).await?;
    let told = json!(report.filled).to_string();
    let applied = l
        .cat
        .migrate_apply(&author, fill("status"), &report.report_id);
    let Outcome::Applied { position: at, .. } = applied.await.must("the fill's apply")? else {
        return Err(Failure::new("the fill's apply answered no position"));
    };
    let after = life().await?;
    let write = ProposedWrite::Migrate {
        migration: Box::new(fill("size")),
        report: Default::default(),
    };
    let proposed = l.cat.propose(&l.own, write, "size".into()).await;
    let Ok(Outcome::Proposed { proposal, .. }) = proposed else {
        return Err(Failure::new(format!("proposing the fill: {proposed:?}")));
    };
    let accepted = l.cat.accept(&l.own, &proposal).await;
    let Ok(Outcome::Applied { position: by, .. }) = accepted else {
        return Err(Failure::new(format!("accepting the fill: {accepted:?}")));
    };
    // A version per migration, and the hash of the same declaration.
    let moved = [after, life().await?].map(|(v, h)| (v - before.0, h == before.1));
    let mut got = Vec::new();
    for k in ["f1", "f2"] {
        let r = l.read("t", k).await?;
        for (f, pos) in [("status", at), ("size", by)] {
            let meta = r.meta.get(f);
            let initial = meta.is_some_and(|m| m.origin == Origin::Initial && m.position == pos);
            got.push(format!("{k}.{f} {:?} {initial}", r.fields.get(f)));
        }
    }
    let again = preview(&l, &l.own, fill("status")).await?;
    let head = store.head().await.must("reading the head")?;
    let twice = l
        .cat
        .migrate_apply(&l.own, fill("status"), &again.report_id)
        .await;
    let unmoved = matches!(twice, Ok(Outcome::Applied { position, .. }) if position == head);
    let open = |k: &str, f: &str| format!("{k}.{f} Some(String(\"open\")) true");
    let want = ["f1", "f2"]
        .map(|k| [open(k, "status"), open(k, "size")])
        .concat();
    ensure!(
        told == r#"[{"field":"status","records":2,"sample_keys":["f1"]}]"#
            && got == want
            && again.filled.iter().all(|f| f.records == 0)
            && unmoved
            && moved == [(1, true), (2, true)],
        "a fill alone fills `t`: the author is told {told}, the records read {got:?}, the \
         version moved by and the hash stayed {moved:?}, and again it counts {:?} and its \
         apply answers {twice:?}",
        again.filled
    );
    Ok(())
}

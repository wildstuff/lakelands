//! A migration declares again the views over its type in its own append
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! lakelands#1726).
//!
//! A migration that takes away a field a kept view reads is refused while
//! the view reads it. Named in the migration's `views`, the view's
//! re-declaration is judged against the migrated type in the same plan,
//! and lands with the `migrated` or not at all; one that reads as the lake
//! holds it is kept, not declared again.

use lakelands_api::write::Migration;

use crate::conformance::fixture::source_record;
use crate::conformance::prelude::*;

check!(
    MIGRATE_WITH_ITS_VIEWS = a_migration_lands_with_the_views_it_declares_again,
    "027, extended (lakelands#1726): a migration that takes away a field a kept view reads lands, in one append, with the view's re-declaration named in its `views`, which no longer reads it: the view is kept and answers what it computes over the migrated records, and a view named as the lake holds it is not declared again; a view named that still reads the field is refused with the report of a migration that names none, a view not named keeps that refusal, and a view that reads another type is refused `migration_view_not_over`, and an apply naming other views than its preview is refused, each appending nothing",
    given
);

check!(
    MIGRATE_WITH_ITS_VIEWS_UNAPPLIED = a_migration_with_its_views_left_unapplied_leaves_both_old,
    "027, extended (lakelands#1726): a migration with its views' re-declarations whose preflight was answered, and whose writer went before its apply, leaves the type and the view as they were: the lake opened again holds both old and the view answers as before; the apply sent then lands both",
    given
);

check!(
    MIGRATE_WITH_ITS_VIEWS_JUDGED = a_view_a_migration_declares_again_is_judged_as_declared,
    "027, extended (lakelands#1726): a view a migration declares again is judged as `declare` judges it: one moved off the type it migrates is refused `migration_view_not_over`, and one that reads a field its author may not read is not granted, as its declaration by that author is, each appending nothing",
    given
);

/// The type `reading`, its records, and the kept view `by_meter` reading
/// `level` and `value`, and `steady` reading `value` alone.
async fn readings(l: &Lake) -> Result<(), Failure> {
    l.declare(reading(true)).await?;
    let records = (0..12)
        .map(|i| {
            let fields = json!({"meter": format!("m{}", i % 3), "level": i, "value": 2 * i});
            source_record("reading", &format!("r{i:02}"), fields)
        })
        .collect();
    l.ingest("load", records)
        .await
        .must("ingesting the readings")?;
    l.declare(by_meter(true)).await?;
    l.declare(steady()).await?;
    l.set("view_kept.by_meter", json!(true)).await?;
    Ok(())
}

/// `reading`, with or without `level`.
fn reading(level: bool) -> TypeDecl {
    let mut fields = vec![sourced("meter", StorageKind::Text)];
    fields.extend(level.then(|| sourced("level", StorageKind::Number)));
    fields.push(sourced("value", StorageKind::Number));
    type_decl("reading", fields)
}

/// `by_meter`: each meter's count and total, and its top `level` where
/// `level` is read.
fn by_meter(level: bool) -> TypeDecl {
    let mut fields = vec![
        field("meter", StorageKind::Text),
        field("c", StorageKind::Number),
        field("total", StorageKind::Number),
    ];
    let mut measures = vec![("c", "count()"), ("total", "sum(value)")];
    if level {
        fields.push(field("top", StorageKind::Number));
        measures.push(("top", "max(level)"));
    }
    view_of("by_meter", "reading", &["meter"], fields, &measures)
}

/// `steady`: the total of every reading, reading `value` alone.
fn steady() -> TypeDecl {
    let fields = vec![field("total", StorageKind::Number)];
    view_of("steady", "reading", &[], fields, &[("total", "sum(value)")])
}

/// The migration of `reading` without `level`, declaring `views` again.
fn dropping(views: Vec<TypeDecl>) -> Migration {
    Migration {
        views,
        ..Migration::to(reading(false))
    }
}

/// What `by_meter` answers: each meter's count and total, sorted.
async fn totals(l: &Lake) -> Result<Vec<String>, Failure> {
    let mut out = Vec::new();
    for meter in ["m0", "m1", "m2"] {
        let r = l.read("by_meter", meter).await?;
        let of = |f: &str| r.fields.get(f).cloned().unwrap_or(Value::Null);
        out.push(format!(
            "{meter} c={} total={} top={}",
            of("c"),
            of("total"),
            of("top")
        ));
    }
    Ok(out)
}

/// The refusal a migration's preflight answers, as said on the wire.
async fn said(l: &Lake, m: Migration) -> Result<Value, Failure> {
    let r = l.cat.migrate(&l.own, m).await;
    let e = r
        .err()
        .ok_or_else(|| Failure::new("the preflight was answered, not refused"))?;
    let said = e
        .case()
        .ok_or_else(|| Failure::new(format!("{e} says no case")))?;
    Ok(serde_json::to_value(said).expect("a case is JSON"))
}

async fn a_migration_lands_with_the_views_it_declares_again(store: Store, _: Given) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    readings(&l).await?;
    let before = s.head().await.must("the head")?;
    // Today's refusal, and the same where the view named still reads `level`.
    let alone = said(&l, dropping(Vec::new())).await?;
    ensure!(
        alone["code"] == "migration_takes_read" || alone["code"] == "unknown_field",
        "a migration taking `level` from under `by_meter` is refused; it said {alone}"
    );
    let same = said(&l, dropping(vec![by_meter(true)])).await?;
    ensure!(
        same == alone,
        "a view named that still reads `level` is refused as one not named is.\n  \
         alone: {alone}\n  named: {same}"
    );
    let unnamed = said(&l, dropping(vec![steady()])).await?;
    ensure!(
        unnamed == alone,
        "`by_meter`, not named, keeps its refusal.\n  alone: {alone}\n  said: {unnamed}"
    );
    let other = view_of("other", "elsewhere", &[], vec![], &[]);
    let not_over = said(&l, dropping(vec![other])).await?;
    let want = json!({"code": "migration_view_not_over", "view": "other", "type": "reading"});
    ensure!(
        ["code", "view", "type"]
            .iter()
            .all(|k| not_over[k] == want[k]),
        "a view that reads no `reading` is refused {want}; it said {not_over}"
    );
    head_unmoved(s, before, "each migration").await?;
    // Named without `level`, it lands with the migration, and `steady`,
    // named as it is held, is not declared again.
    let m = dropping(vec![by_meter(false), steady()]);
    let id = l
        .cat
        .migrate(&l.own, m.clone())
        .await
        .must("the preflight")?;
    let id = id.data.report_id;
    // The preview's id binds the views it was made with.
    let other = l
        .cat
        .migrate_apply(&l.own, dropping(vec![by_meter(false)]), &id);
    let other = other.await;
    ensure!(
        other.is_err(),
        "an apply naming other views than its preview is refused: {other:?}"
    );
    head_unmoved(s, before, "an apply of other views").await?;
    l.cat
        .migrate_apply(&l.own, m, &id)
        .await
        .must("the migration with its views")?;
    let after = s.head().await.must("the head")?;
    ensure!(
        after.0 == before.0 + 2,
        "the migration and `by_meter`'s re-declaration are two events of one append, \
         `steady` none: the head moved from {before} to {after}"
    );
    let said = l.describe_type("by_meter").await?.lifecycle.kept;
    ensure!(
        matches!(said, Some(lakelands_api::describe::Kept::Kept)),
        "`by_meter` declared again with the migration is kept: `describe_type` says {said:?}"
    );
    let got = totals(&l).await?;
    let want = [
        "m0 c=4 total=36 top=null",
        "m1 c=4 total=44 top=null",
        "m2 c=4 total=52 top=null",
    ];
    ensure!(
        got == want,
        "`by_meter` answers each meter's count and total over the migrated records: {got:?}"
    );
    Ok(())
}

async fn a_migration_with_its_views_left_unapplied_leaves_both_old(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    readings(&l).await?;
    let before = totals(&l).await?;
    let m = dropping(vec![by_meter(false)]);
    l.cat
        .migrate(&l.own, m.clone())
        .await
        .must("the preflight")?;
    // The writer goes between the preflight and the apply.
    drop(l);
    let store = (given.reopen)(store, Default::default()).await;
    let store = store.map_err(|e| Failure::new(format!("reopening the lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    // The open appends its lease, and nothing of the migration.
    let head = store.head().await.must("the head")?;
    ensure!(
        l.describe_type("reading")
            .await?
            .fields
            .iter()
            .any(|f| f.name.as_str() == "level"),
        "`reading` still holds `level`"
    );
    let still = totals(&l).await?;
    ensure!(
        still == before,
        "`by_meter` answers as before.\n  before: {before:?}\n  after:  {still:?}"
    );
    let id = l
        .cat
        .migrate(&l.own, m.clone())
        .await
        .must("the preflight again")?;
    let id = id.data.report_id;
    l.cat
        .migrate_apply(&l.own, m, &id)
        .await
        .must("the apply")?;
    let after = store.head().await.must("the head")?;
    ensure!(
        after.0 == head.0 + 2,
        "the apply lands the migration and the view in one append: {head} to {after}"
    );
    Ok(())
}

async fn a_view_a_migration_declares_again_is_judged_as_declared(store: Store, _: Given) -> Held {
    use lakelands_api::principal::Access;
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    readings(&l).await?;
    let before = s.head().await.must("the head")?;
    // Moved to a source that is not the type, it is no view over it.
    let mut moved = by_meter(false);
    if let Some(v) = moved.view.as_deref_mut() {
        v.source = "nowhere".into();
    }
    let said = said(&l, dropping(vec![moved])).await?;
    ensure!(
        said["code"] == "migration_view_not_over" && said["view"] == "by_meter",
        "`by_meter` moved off `reading` is refused `migration_view_not_over`; it said {said}"
    );
    // An author who may not read `value` may not declare a view that sums it.
    let mut masked = field_access(s, "masked", "reading", &[("value", Access::Masked)]);
    (masked.grants.schema, masked.grants.restricted) = (true, true);
    let declared = l
        .cat
        .declare(&masked, Declaration::Type(by_meter(false)))
        .await;
    let migrated = (l.cat)
        .migrate(&masked, dropping(vec![by_meter(false)]))
        .await;
    let granted = |r: Option<(&str, &Refusal)>| matches!(r, Some(("value", Refusal::NotGranted)));
    ensure!(
        granted(refused_on(&declared)) && granted(refused_on(&migrated)),
        "a view summing `value` is not granted to one it is masked from, declared or \
         declared again with a migration.\n  declared: {declared:?}\n  migrated: {migrated:?}"
    );
    head_unmoved(s, before, "each refusal").await?;
    Ok(())
}

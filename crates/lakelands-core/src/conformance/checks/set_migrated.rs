//! A migration decides the membership of the records it migrates again
//! (`docs/decisions/004-the-change-feed-is-the-log.md`, extended).

use lakelands_api::error::Case;
use lakelands_api::event::EventBody;
use lakelands_api::location::Limits;
use lakelands_api::ontology::StorageKind::{self, Boolean, Text};
use lakelands_api::ontology::{RelationDecl, TypeDecl};
use lakelands_api::seam::Seams;
use lakelands_api::{Error, LakeStore, Outcome, Position, Refusal};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, events_from, head_unmoved, limits, migration, object_set_on, refused_on, type_of,
};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    SET_MIGRATED = a_migration_decides_the_membership_of_the_records_it_migrates,
    "004, extended: a migration appends, directly after its `migrated` and caused by it, an `object_set_changed` for each record whose membership under a set of the type changes as migrated, and none when it changes nothing a filter reads; its answer names each set joined or left once and `through` the changes, also when it adds a relation carried by links; a re-keyed member stays a member under its new key, after a rebuild too; past the lake's `max_set_changes` a migration is refused `too_large`, naming a set and the ceiling, and appends nothing; one that is also applied with a stale report, or is not lossless, is refused for that first",
    given
);

/// `item` keyed by `key` (none: the writer's), its `flag` of `kind`.
fn item(kind: StorageKind, key: &[&str]) -> TypeDecl {
    let mut t = type_of("item", &[("flag", kind), ("code", Text), ("status", Text)]);
    t.key = key.iter().map(|k| (*k).into()).collect();
    for f in t
        .fields
        .iter_mut()
        .filter(|f| key.contains(&f.name.as_str()))
    {
        f.required = true;
    }
    t
}

/// The membership changes caused by the event at `at`: (set, key, joined).
async fn changed(
    store: &dyn LakeStore,
    at: Position,
) -> Result<Vec<(String, String, bool)>, Failure> {
    let events = events_from(store, at).await?;
    let caused = events.into_iter().filter_map(|e| match e.body {
        EventBody::ObjectSetChanged {
            set, key, joined, ..
        } if e.cause == Some(at) => Some((set, key.to_string(), joined)),
        _ => None,
    });
    Ok(caused.collect())
}

/// Whether a migration's answer names `joined` and `left` once each, and
/// `through` its changes (015, extended).
fn names(answer: &Outcome, joined: &str, left: &str) -> bool {
    matches!(answer, Outcome::Applied { set_off, .. } if set_off.joined == [joined]
        && set_off.left == [left] && set_off.through.is_some())
}

async fn a_migration_decides_the_membership_of_the_records_it_migrates(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(item(Text, &[])).await?;
    // A boolean equals only a boolean, so `flag`'s kind decides which holds.
    for (name, filter) in [
        ("word", "flag = 'true'"),
        ("yes", "flag = true"),
        ("open", "status = 'open'"),
    ] {
        l.declare_as(object_set_on(name, "item", filter), name)
            .await?;
    }
    l.upsert(
        "item",
        "a",
        json!({"flag": "true", "code": "A", "status": "open"}),
    )
    .await?;
    l.upsert(
        "item",
        "b",
        json!({"flag": "true", "code": "B", "status": "shut"}),
    )
    .await?;
    // `flag` becomes a boolean: `a` and `b` leave `word` and join `yes`;
    // `open` reads no `flag`. A record's sets are decided in name order.
    let moves = |a: &str, b: &str, word: bool| {
        [
            ("word", a, word),
            ("yes", a, !word),
            ("word", b, word),
            ("yes", b, !word),
        ]
        .map(|(s, k, j)| (s.to_owned(), k.to_owned(), j))
    };
    let at = store.head().await.must("the head")?.next();
    let answer = l.migrate(item(Boolean, &[])).await?;
    let (got, want) = (changed(store.as_ref(), at).await?, moves("a", "b", false));
    ensure!(
        got == want,
        "converting `flag` to a boolean must change {want:?}; its append changed {got:?}"
    );
    ensure!(
        names(&answer, "yes", "word"),
        "the migration answers `yes` joined and `word` left, once each, `through` them; it \
         answered {answer:?}"
    );
    // Keyed by `code`: `a` moves to `A`, and no filter's truth changes.
    let at = store.head().await.must("the head")?.next();
    l.migrate(item(Boolean, &["code"])).await?;
    let got = changed(store.as_ref(), at).await?;
    ensure!(
        got.is_empty(),
        "a re-key that changes no filter's truth changes nothing; it changed {got:?}"
    );
    let at = store.head().await.must("the head")?.next();
    l.upsert(
        "item",
        "A",
        json!({"flag": true, "code": "A", "status": "shut"}),
    )
    .await?;
    let left = events_from(store.as_ref(), at).await?.into_iter().any(|e| {
        matches!(e.body, EventBody::ObjectSetChanged { joined: false, ref key, ref set, .. }
            if key.as_str() == "A" && set == "open")
    });
    ensure!(
        left,
        "`a`, re-keyed to `A`, is in `open` under its new key and leaves it"
    );
    // A rebuild folds the joins and the re-key in one page, and leaves `A`
    // and `B` in `yes` as the live fold did.
    store.rebuild().await.must("rebuilding the projection")?;
    // Back to text, with a relation carried by links added: the link type's
    // `declared` rides in the append, and the answer still reads the changes.
    let linked = |kind| {
        let mut t = item(kind, &["code"]);
        t.relations = vec![RelationDecl::to("next", "item")];
        t
    };
    let at = store.head().await.must("the head")?.next();
    let answer = l.migrate(linked(Text)).await?;
    let (got, want) = (changed(store.as_ref(), at).await?, moves("A", "B", true));
    ensure!(
        got == want && names(&answer, "word", "yes"),
        "converting `flag` back while adding a relation must change {want:?} and answer \
         them; its append changed {got:?} and it answered {answer:?}"
    );
    // A boolean again: four changes, past a ceiling of one.
    let lower = Limits {
        max_set_changes: 1,
        ..limits()
    };
    let store = (given.relimit)(store, Seams::default(), lower)
        .await
        .map_err(|e| Failure::new(format!("opening the lake again with a lower ceiling: {e}")))?;
    let l = Lake::of(store.as_ref());
    let m = migration(linked(Boolean));
    // Past the ceiling too, a stale report and a record that does not
    // convert are refused first: `C`'s `flag` is no boolean.
    let c = |flag: &str| json!({"flag": flag, "code": "C", "status": "shut"});
    l.upsert("item", "C", c("maybe")).await?;
    let preflight = |m| async { Ok(l.cat.migrate(&l.own, m).await.must("the preflight")?.data) };
    let stale = l.cat.migrate_apply(&l.own, m.clone(), "stale").await;
    refused_as!(
        "a stale report past the ceiling",
        stale,
        Case::MigrationStale
    )?;
    let report = preflight(m.clone()).await?;
    let r = l
        .cat
        .migrate_apply(&l.own, m.clone(), &report.report_id)
        .await;
    ensure!(
        matches!(
            &r,
            Err(Error::Refused {
                reason: Refusal::Migration { .. },
                ..
            })
        ),
        "a migration past the ceiling that is not lossless is refused with its report; it \
         answered {r:?}"
    );
    l.upsert("item", "C", c("false")).await?;
    let before = store.head().await.must("the head")?;
    let report = preflight(m.clone()).await?;
    let r = l.cat.migrate_apply(&l.own, m, &report.report_id).await;
    ensure!(
        matches!(
            refused_on(&r),
            Some(("word" | "yes", Refusal::TooLarge { max: 1 }))
        ) && r.as_ref().is_err_and(|e| e
            .to_string()
            .ends_with("1 of the set's memberships would change")),
        "a migration that would change 4 memberships under a `max_set_changes` of 1 is \
         refused too_large naming a set, in words true of a migration; it answered {r:?}"
    );
    head_unmoved(store.as_ref(), before, "the refused migration").await
}

//! The fold of a thinning on the kept views of its type: every group a
//! taken record's versions held is stranded and keeps the folds it had, a
//! later erasure folds again from the log, and a view made again carries
//! its stranded groups
//! (`docs/decisions/102-a-thinned-series-keeps-its-roll-ups.md`). Each row
//! appends the act's part below the store, as the walk would.

use super::erasable::{Keys, again, with};
use crate::conformance::Lines;
use crate::conformance::fixture::{canonical_line, source_record, try_upsert};
use crate::conformance::prelude::*;
use lakelands_api::ontology::FieldType;
use lakelands_api::seam::{SETTING_TYPE, VIEW_KEPT_SETTING};
use lakelands_api::write::{DeclarationKind, Withdrawal};

check!(
    THIN_GROUP_STRANDED = a_group_a_taken_record_held_is_stranded_and_answers_as_before,
    "102 (lakelands#1639): a thinning takes `t1`; `t4`, which moved from `b` to `c`; `t5`, retired; `t6` to `t9`, each of whose states stood in groups under a `where` of `daily`, or a value of the `floor` it names, in force from the state up to the next; and `t10`, whose first state was put while `daily` stood withdrawn. The kept `daily` then answers every group it answered before as before, and `describe_type` counts twelve stranded groups, one for each group a taken state stood in. `find` of the type omits the taken keys, and a rebuild answers the same",
    given
);

check!(
    THIN_ERASURE_RESTATES_STRANDED_FOLDS = an_erasure_folds_a_stranded_group_again,
    "102 (lakelands#1639): a kept view's group stranded by the thinning of `crm`'s `a` folds again when `crm`'s values are removed — its sum drops `a`'s 9 and its maximum falls to the next holder's 5 — and when `erp` is forgotten its sum drops `erp`'s values, the group stranded after each",
    given
);

check!(
    THIN_STRANDED_MAKE_CARRIES = a_view_made_again_carries_its_stranded_groups,
    "102 (lakelands#1639): a declaration of the source that makes the kept `daily` again carries its stranded group `a`, whose taken `t1` held its maximum, and counts none of its members twice. A later member folds in, and when it retires the maximum falls back to `t1`'s carried 9. Redeclaring `daily` with another measure, migrating a field it reads, and setting `floor`, which its `where` names, are each refused `stranded_make` naming `daily`",
    given
);

/// The horizon every row's act cuts at.
const HORIZON: &str = "2026-06-01T00:00:00Z";

/// Appends below `store` a part of a thinning of `ty` by `at` taking
/// `keys`, as the act's walk would, and opens the lake again with `seams`.
async fn thin(
    (store, seams): (Store, Seams),
    given: &Given,
    ty: &str,
    keys: &[&str],
) -> Result<Store, Failure> {
    use lakelands_api::engine::OpenMode;
    use lakelands_api::event::EventId;
    let lake = store.lake().clone();
    let events = log_events(store.as_ref()).await?;
    let taken = |e: &&Event| matches!(e.body, EventBody::LeaseTaken { .. });
    let (Some(template), Some(last)) = (events.iter().find(taken).cloned(), events.last()) else {
        return Err(Failure::new(
            "the lake's log holds its writer's `lease_taken`",
        ));
    };
    let body = EventBody::ValuesThinned {
        ty: ty.into(),
        by: "at".into(),
        before: Timestamp::parse(HORIZON).expect("an instant"),
        keys: keys.iter().map(|k| RecordKey::from(*k)).collect(),
    };
    let known_at = last.known_at;
    let line: Lines = Box::new(move |(tip, prev), fence| {
        let position = tip.next();
        let event = Event {
            position,
            id: EventId(format!("{:032x}", position.0)),
            prev,
            known_at,
            fence,
            body,
            ..template
        };
        (vec![canonical_line(event)], 0)
    });
    let c = (given.continued)(Some(store), lake, line, OpenMode::Write, seams);
    let c = c
        .await
        .map_err(|e| Failure::new(format!("appending the thinning of {keys:?}: {e}")))?;
    c.opened
        .map_err(|e| Failure::new(format!("opening the lake after the thinning: {e}")))
}

/// `tick`, its instants `at`, its group `m` and its value `n`, with
/// `more` fields beside them.
fn tick(more: &[FieldDecl]) -> TypeDecl {
    let mut fields = vec![
        field("at", StorageKind::DateTime),
        field("m", StorageKind::Text),
        field("n", StorageKind::Number),
    ];
    fields.extend_from_slice(more);
    type_decl("tick", fields)
}

/// `daily` over `tick` by `m`: `c` its count, `t` the measure `sum`.
fn daily(r#where: Option<&str>, sum: &str) -> TypeDecl {
    let fields = vec![
        field("m", StorageKind::Text),
        field("c", StorageKind::Number),
        field("t", StorageKind::Number),
    ];
    let mut d = view_of(
        "daily",
        "tick",
        &["m"],
        fields,
        &[("c", "count()"), ("t", sum)],
    );
    if let Some(view) = d.view.as_mut() {
        view.r#where = r#where.map(Into::into);
    }
    d
}

/// Writes `tick` `key` in group `m` holding `n`, on day `day` of January.
async fn tick_at(l: &Lake, key: &str, m: &str, n: u64, day: u32) -> Held {
    let at = format!("2026-01-{day:02}T00:00:00Z");
    l.upsert("tick", key, json!({"at": at, "m": m, "n": n}))
        .await?;
    Ok(())
}

/// Each group `view` answers, with its fields `fields`.
async fn answers(l: &Lake, view: &str, fields: &[&str]) -> Result<Vec<String>, Failure> {
    let rs = l.all_found(view, find_at(None)).await?;
    Ok(rs
        .iter()
        .map(|r| {
            let f = fields
                .iter()
                .map(|f| r.fields.get(*f).cloned().unwrap_or(Value::Null));
            let f: Vec<String> = f.map(|v| v.to_string()).collect();
            format!("{} {}", r.key, f.join("/"))
        })
        .collect())
}

/// How many of `view`'s groups `describe_type` counts stranded.
async fn stranded(l: &Lake, view: &str) -> Result<u64, Failure> {
    Ok(l.describe_type(view).await?.stranded)
}

/// `view` answers `want` of its `fields` and counts `strands` stranded
/// groups, or `red` says otherwise, `when` naming the step.
async fn held(
    l: &Lake,
    (view, fields): (&str, &[&str]),
    (want, strands): (&[&str], u64),
    when: &str,
    red: &mut Vec<String>,
) -> Held {
    let now = answers(l, view, fields).await?;
    if now != want {
        red.push(format!("{when} `{view}` answers {now:?}, not {want:?}"));
    }
    let n = stranded(l, view).await?;
    if n != strands {
        red.push(format!(
            "{when} `describe_type` counts {n} stranded groups, not {strands}"
        ));
    }
    Ok(())
}

async fn a_group_a_taken_record_held_is_stranded_and_answers_as_before(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    // Each of `t6` to `t9` stood in a group only under a rule in force
    // after its state was put: `daily` made again with another `where`,
    // or `floor` set to another value; `t8`'s `i` and `t9`'s `k` until its
    // next state only. `t10`'s first state stood in none.
    l.declare(tick(&[])).await?;
    l.set("floor", json!(0)).await?;
    l.declare(daily(Some("n > 9"), "sum(n)")).await?;
    l.set(&format!("{VIEW_KEPT_SETTING}daily"), json!(true))
        .await?;
    for (key, m, n) in [
        ("t6", "e", 1),
        ("t7", "h", 20),
        ("t7", "g", 1),
        ("t8", "i", 1),
    ] {
        tick_at(&l, key, m, n, 6).await?;
    }
    l.declare(daily(Some("n > setting('floor')"), "sum(n)"))
        .await?;
    tick_at(&l, "t8", "j", 50, 8).await?;
    tick_at(&l, "t9", "k", 0, 9).await?;
    l.set("floor", json!(-1)).await?;
    tick_at(&l, "t9", "l", 7, 9).await?;
    l.set("floor", json!(0)).await?;
    // `t10`'s `x` stood in no group: `daily` was withdrawn while it was
    // `t10`'s state.
    let withdrawn = Declaration::Withdraw(Withdrawal {
        kind: DeclarationKind::Type,
        name: "daily".into(),
    });
    l.declare_as(withdrawn, "withdrawing `daily`").await?;
    tick_at(&l, "t10", "x", 30, 10).await?;
    tick_at(&l, "t10", "y", 30, 10).await?;
    l.declare(daily(Some("n > setting('floor')"), "sum(n)"))
        .await?;
    for (key, m, n, day) in [("t1", "a", 1, 1), ("t2", "a", 2, 2), ("t3", "c", 5, 3)] {
        tick_at(&l, key, m, n, day).await?;
    }
    tick_at(&l, "t4", "b", 3, 4).await?;
    tick_at(&l, "t4", "c", 3, 4).await?;
    tick_at(&l, "t5", "d", 4, 5).await?;
    l.retire("tick", "t5").await?;
    // A source declared again makes `daily` once more, after them all.
    l.declare(tick(&[field("note", StorageKind::Text)])).await?;
    let was = answers(&l, "daily", &["c", "t"]).await?;
    let store = thin(
        (store, Seams::default()),
        &given,
        "tick",
        &["t1", "t4", "t5", "t6", "t7", "t8", "t9", "t10"],
    )
    .await?;
    let l = Lake::of(store.as_ref());
    let mut red = Vec::new();
    for when in ["after the act", "after a rebuild"] {
        if when == "after a rebuild" {
            store.rebuild().await.must("rebuilding the projection")?;
        }
        let now = answers(&l, "daily", &["c", "t"]).await?;
        if now != was {
            red.push(format!("{when} `daily` answers {now:?}, before it {was:?}"));
        }
        let n = stranded(&l, "daily").await?;
        if n != 12 {
            red.push(format!(
                "{when} `describe_type` counts {n} stranded groups, not 12"
            ));
        }
        let found = l.all_found("tick", find_at(None)).await?;
        let keys: Vec<&str> = found.iter().map(|r| r.key.as_str()).collect();
        if keys != ["t2", "t3"] {
            red.push(format!("{when} `tick` finds {keys:?}, not `t2` and `t3`"));
        }
    }
    ensure!(
        red.is_empty(),
        "a group a taken record held is stranded and answers as before: {}",
        red.join("; ")
    );
    Ok(())
}

async fn an_erasure_folds_a_stranded_group_again(store: Store, given: Given) -> Held {
    // `n` is sealed under its source's key, which forgetting destroys.
    let keys = Arc::new(Keys::default());
    let store = again(&given.reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    let n = FieldDecl {
        erasable: true,
        ..sourced("n", StorageKind::Number)
    };
    let at = field("at", StorageKind::DateTime);
    l.declare(type_decl(
        "line",
        vec![at, field("g", StorageKind::Text), n],
    ))
    .await?;
    let fields = vec![
        field("g", StorageKind::Text),
        field("c", StorageKind::Number),
        field("t", StorageKind::Number),
        field("high", StorageKind::Number),
    ];
    let measures = [("c", "count()"), ("t", "sum(n)"), ("high", "max(n)")];
    l.declare(view_of("sums", "line", &["g"], fields, &measures))
        .await?;
    l.set(&format!("{VIEW_KEPT_SETTING}sums"), json!(true))
        .await?;
    l.ingest("crm", vec![source_record("line", "a", json!({"n": 9}))])
        .await
        .must("feeding `crm`'s `a`")?;
    let erp = vec![
        source_record("line", "b", json!({"n": 5})),
        source_record("line", "c", json!({"n": 1})),
    ];
    l.ingest("erp", erp).await.must("feeding `erp`'s two")?;
    for key in ["a", "b", "c"] {
        let at = "2026-01-01T00:00:00Z";
        l.upsert("line", key, json!({"at": at, "g": "x"})).await?;
    }
    let store = thin((store, with(&keys)), &given, "line", &["a"]).await?;
    let l = Lake::of(store.as_ref());
    let mut red = Vec::new();
    let sums = ("sums", &["c", "t", "high"][..]);
    held(&l, sums, (&["x 3/15/9"], 1), "after the act", &mut red).await?;
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    let (crm, erp) = (SourceId::from("crm"), SourceId::from("erp"));
    let removed = store.remove_values(&crm, &all, false, None);
    removed.await.must("removing what `crm` wrote")?;
    held(
        &l,
        sums,
        (&["x 3/6/5"], 1),
        "after `crm`'s values are removed",
        &mut red,
    )
    .await?;
    let forgot = store.forget_source(&erp, None);
    forgot.await.must("forgetting `erp`")?;
    held(
        &l,
        sums,
        (&["x 3/0/null"], 1),
        "after `erp` is forgotten",
        &mut red,
    )
    .await?;
    ensure!(
        red.is_empty(),
        "an erasure folds a stranded group again from the log: {}",
        red.join("; ")
    );
    Ok(())
}

async fn a_view_made_again_carries_its_stranded_groups(store: Store, given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(tick(&[])).await?;
    l.set("floor", json!(0)).await?;
    l.declare(daily(Some("n > setting('floor')"), "max(n)"))
        .await?;
    l.set(&format!("{VIEW_KEPT_SETTING}daily"), json!(true))
        .await?;
    for (key, m, n, day) in [("t1", "a", 9, 1), ("t2", "a", 2, 2), ("t3", "b", 4, 3)] {
        tick_at(&l, key, m, n, day).await?;
    }
    let store = thin((store, Seams::default()), &given, "tick", &["t1"]).await?;
    let l = Lake::of(store.as_ref());
    let mut red = Vec::new();
    let kept = ("daily", &["c", "t"][..]);
    held(
        &l,
        kept,
        (&["a 2/9", "b 1/4"], 1),
        "after the act",
        &mut red,
    )
    .await?;
    // A field the view reads nothing of: the make carries `a`'s record.
    l.declare(tick(&[field("note", StorageKind::Text)])).await?;
    held(&l, kept, (&["a 2/9", "b 1/4"], 1), "made again", &mut red).await?;
    tick_at(&l, "t6", "a", 20, 6).await?;
    let after = (&["a 3/20", "b 1/4"][..], 1);
    held(&l, kept, after, "a later member folded in", &mut red).await?;
    // Its maximum falls back to the taken holder's entry, carried.
    l.retire("tick", "t6").await?;
    let back = (&["a 2/9", "b 1/4"][..], 1);
    held(&l, kept, back, "the later member retired", &mut red).await?;
    let measure = daily(Some("n > setting('floor')"), "sum(n)");
    let measure = l.cat.declare(&l.own, Declaration::Type(measure)).await;
    let mut moved = tick(&[field("note", StorageKind::Text)]);
    if let Some(f) = moved.fields.iter_mut().find(|f| f.name.as_str() == "n") {
        f.ty = FieldType::kind(StorageKind::Decimal);
    }
    let floor = json!({"key": "floor", "value": 1});
    let refused = [
        ("redeclaring `daily` with `sum(n)`", measure.map(|_| ())),
        (
            "migrating `tick`'s `n` to decimals",
            migrated(&l, moved).await,
        ),
        (
            "setting `floor`",
            try_upsert(l.cat.as_ref(), &l.own, SETTING_TYPE, "floor", floor)
                .await
                .map(|_| ()),
        ),
    ];
    for (what, done) in refused {
        let ok = matches!(
            invalid_case(&done),
            Some(Case::StrandedMake { view, .. }) if view.as_str() == "daily"
        );
        if !ok {
            red.push(format!(
                "{what} answered {done:?}, not `stranded_make` naming `daily`"
            ));
        }
    }
    held(
        &l,
        kept,
        (&["a 2/9", "b 1/4"], 1),
        "after the refusals",
        &mut red,
    )
    .await?;
    ensure!(
        red.is_empty(),
        "a view made again carries its stranded groups: {}",
        red.join("; ")
    );
    Ok(())
}

/// Migrates the lake to `to`, its preflight and its apply, as the owner.
async fn migrated(l: &Lake, to: TypeDecl) -> lakelands_api::Result<()> {
    let m = migration(to);
    let report = l.cat.migrate(&l.own, m.clone()).await?.data.report_id;
    l.cat.migrate_apply(&l.own, m, &report).await.map(|_| ())
}

//! What a trigger's firing says, and when it fires: past the depth a
//! reaction could write at it drops instead
//! (`docs/decisions/021-the-write-context-belongs-to-the-host.md`).

use lakelands_api::event::{ChangeKind, DepthBound, EventBody};
use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::FeedFilter;
use lakelands_api::{Position, Refusal, SourceId, TypeName};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, events_from, field, followed, head_unmoved, limits, reactive, reference,
    refused_on, source_record, trigger_on, try_upsert, type_decl,
};
use crate::conformance::{Failure, Held, Store};

check!(
    TRIGGER_DROPPED = a_trigger_past_its_depth_drops_instead_of_firing,
    "021, extended (lakelands#1216): a write whose depth is at or above a trigger's `max_depth`, or the lake's `max_reactive_depth`, appends `trigger_dropped` naming the trigger, the record, the write's depth, the bound that applied, the lower of the two, and whose it is, in place of `trigger_fired`; below both the trigger fires, and a reaction one deeper naming that firing is written, while one naming a drop is refused `too_deep` naming the trigger, appending nothing; the feed tells `trigger_dropped` to a consumer that asks for it"
);

check!(
    TRIGGER_STRICT = a_strict_guard_fails_where_a_field_it_reads_has_no_value,
    "004, extended (lakelands#1215): a trigger declared `when_strict` fires only where every field its `when` reads, and every `previous(f)`, has a value: `not (status = 'paid')` fires on a record without `status` by default and not when strict, and a guard reading `previous(status)` fires on a create by default and not when strict; a field written null fails it as one absent; a strict guard reads every operand of an `or`, so `amount > 100 or status = 'paid'` fails on a record without `status` in either order; once the fields have values the two fire alike"
);

check!(
    TRIGGER_ACROSS = a_firing_across_an_edge_drops_reads_strictly_and_names_its_kind,
    "055 and 004, extended (lakelands#1216, lakelands#1215, lakelands#1217): a trigger whose `when` reads across a reference, fired by a write of the far record, follows the rules of a firing at its own record: past its `max_depth` it drops, strict it fails where the far field has no value or the far record is retired, it names the far write's kind and says it made no record, and an `on_changes` that leaves that kind out keeps it silent"
);

check!(
    TRIGGER_CHANGE = a_firing_says_what_kind_of_change_fired_it,
    "004, extended (lakelands#1217): a `trigger_fired` says the kind of the write that fired it, `upserted`, `ingested`, `retired`, `purged`, `forgotten` or `recomputed`, and whether it made the record: a create and a revival did, an update, a retirement and a fill did not; `created()` holds in a guard on a create and a revival alone; a trigger with `on_changes: [upserted, ingested]` fires on those and stays silent on a retirement, a purge, a forgetting and a fill"
);

/// What the write at `at` set off, each firing as `(trigger, dropped by)`:
/// `None` for one that fired, the depth, the bound and whose it is for one
/// dropped.
async fn set_off(
    s: &dyn lakelands_api::LakeStore,
    at: Position,
) -> Result<Vec<(String, Option<(u32, u32, DepthBound)>)>, Failure> {
    let mut out = Vec::new();
    for e in events_from(s, at)
        .await?
        .into_iter()
        .filter(|e| e.cause == Some(at))
    {
        match e.body {
            EventBody::TriggerFired { trigger, .. } => out.push((trigger.to_string(), None)),
            EventBody::TriggerDropped {
                trigger,
                depth,
                max_depth,
                bound,
                ..
            } => out.push((trigger.to_string(), Some((depth, max_depth, bound)))),
            _ => {}
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

async fn a_trigger_past_its_depth_drops_instead_of_firing(store: Store) -> Held {
    let s = store.as_ref();
    let l = Lake::of(s);
    l.declare(type_decl("job", vec![field("n", StorageKind::Number)]))
        .await?;
    let lakelands_api::write::Declaration::Trigger(mut own) = trigger_on("own", "job", None) else {
        unreachable!("trigger_on declares a trigger")
    };
    own.max_depth = Some(2);
    let fut = l
        .cat
        .declare(&l.own, lakelands_api::write::Declaration::Trigger(own));
    fut.await.must("declaring `own` with max_depth 2")?;
    let fut = l.cat.declare(&l.own, trigger_on("lake", "job", None));
    fut.await.must("declaring `lake`")?;
    let max = limits().max_reactive_depth;
    let (trigger, lake) = (DepthBound::Trigger, DepthBound::Lake);
    let (mut fired_at, mut dropped_at) = (None, None);
    for (depth, want) in [
        (1, [None, None]),
        (2, [None, Some((2, 2, trigger))]),
        (max - 1, [None, Some((max - 1, 2, trigger))]),
        (max, [Some((max, max, lake)), Some((max, 2, trigger))]),
    ] {
        let cat = s.catalog(reactive(depth, None));
        let key = format!("j{depth}");
        let wrote = try_upsert(cat.as_ref(), &l.own, "job", &key, json!({"n": depth})).await;
        let at = wrote.must(&format!("a write at depth {depth}"))?.position();
        let got = set_off(s, at).await?;
        let want = vec![("lake".to_owned(), want[0]), ("own".to_owned(), want[1])];
        ensure!(
            got == want,
            "a write at depth {depth} (`own`'s max_depth 2, the lake's {max}) sets off {want:?}; \
             it set off {got:?}"
        );
        match depth {
            1 => fired_at = Some(at),
            2 => dropped_at = Some(at),
            _ => {}
        }
    }
    // A reaction to a firing the store appended is written: one deeper
    // than the write that fired it is within the trigger's bound.
    let at = fired_at.expect("the write at depth 1");
    let fired = events_from(s, at).await?.into_iter().find(|e| {
        e.cause == Some(at)
            && matches!(&e.body, EventBody::TriggerFired { trigger, .. } if trigger.as_str() == "own")
    });
    let fired = fired.ok_or_else(|| Failure::new("the write at depth 1 fires `own`"))?;
    let cat = s.catalog(reactive(2, Some(fired.position)));
    let reacted = try_upsert(cat.as_ref(), &l.own, "job", "r", json!({"n": 0})).await;
    reacted.must("a reaction at depth 2 to `own`'s firing at depth 1")?;
    // A host that reacts to a drop all the same is refused at the append,
    // naming the trigger: the old check stays as the second line.
    let at = dropped_at.expect("the write at depth 2");
    let dropped = events_from(s, at).await?.into_iter().find(|e| {
        e.cause == Some(at)
            && matches!(&e.body, EventBody::TriggerDropped { trigger, .. } if trigger.as_str() == "own")
    });
    let dropped = dropped.ok_or_else(|| Failure::new("the write at depth 2 drops `own`"))?;
    let before = s.head().await.must("the head")?;
    let cat = s.catalog(reactive(3, Some(dropped.position)));
    let r = try_upsert(cat.as_ref(), &l.own, "job", "d", json!({"n": 0})).await;
    ensure!(
        matches!(refused_on(&r), Some(("own", Refusal::TooDeep { max: 2 }))),
        "a reaction at depth 3 naming `own`'s drop at depth 2 is refused too_deep naming `own`; \
         it answered {r:?}"
    );
    head_unmoved(s, before, "the reaction to a drop").await?;
    // The feed tells the drops to a consumer that asks for them.
    let filter = FeedFilter {
        kinds: vec!["trigger_dropped".into()],
        types: vec![TypeName::from("job")],
        ..FeedFilter::default()
    };
    let told = followed(s, Position::GENESIS, &filter).await?;
    let told = told
        .iter()
        .filter(|e| matches!(e.body, EventBody::TriggerDropped { .. }));
    ensure!(
        told.count() == 5,
        "the feed tells the five drops to a consumer of `trigger_dropped`"
    );
    Ok(())
}

async fn a_strict_guard_fails_where_a_field_it_reads_has_no_value(store: Store) -> Held {
    let s = store.as_ref();
    let l = Lake::of(s);
    let fields = vec![
        field("status", StorageKind::Text),
        field("amount", StorageKind::Number),
    ];
    l.declare(type_decl("bill", fields)).await?;
    for (name, when, strict) in [
        ("unpaid", "not (status = 'paid')", false),
        ("unpaid_strict", "not (status = 'paid')", true),
        (
            "moved",
            "previous(status) is null or status = 'paid'",
            false,
        ),
        (
            "moved_strict",
            "previous(status) is null or status = 'paid'",
            true,
        ),
        ("big_strict", "amount > 100 or status = 'paid'", true),
        (
            "big_strict_swapped",
            "status = 'paid' or amount > 100",
            true,
        ),
    ] {
        let lakelands_api::write::Declaration::Trigger(mut t) =
            trigger_on(name, "bill", Some(when))
        else {
            unreachable!("trigger_on declares a trigger")
        };
        t.when_strict = strict;
        let fut = l
            .cat
            .declare(&l.own, lakelands_api::write::Declaration::Trigger(t));
        fut.await.must(&format!("declaring `{name}`"))?;
    }
    for (key, fields, want) in [
        ("b1", json!({"amount": 5}), vec!["moved", "unpaid"]),
        (
            "b1",
            json!({"status": "open"}),
            vec!["moved", "unpaid", "unpaid_strict"],
        ),
        (
            "b1",
            json!({"status": "paid"}),
            vec!["big_strict", "big_strict_swapped", "moved", "moved_strict"],
        ),
        // A strict guard reads every operand, whatever their order, and a
        // field written null has no value.
        ("b2", json!({"amount": 200}), vec!["moved", "unpaid"]),
        ("b2", json!({"status": null}), vec!["moved", "unpaid"]),
    ] {
        let at = l.upsert("bill", key, fields.clone()).await?.position();
        let got = set_off(s, at).await?;
        let got: Vec<&str> = got.iter().map(|(t, _)| t.as_str()).collect();
        ensure!(
            got == want,
            "`{key}` written {fields} fires {want:?}; it fired {got:?}"
        );
    }
    Ok(())
}

/// Every firing appended from `from` on, each `(trigger, change, created)`,
/// in the log's order.
async fn fired(
    s: &dyn lakelands_api::LakeStore,
    from: Position,
) -> Result<Vec<(String, Option<ChangeKind>, Option<bool>)>, Failure> {
    let events = events_from(s, from).await?.into_iter();
    let fired = events.filter_map(|e| match e.body {
        EventBody::TriggerFired {
            trigger,
            change,
            created,
            ..
        } => Some((trigger.to_string(), change, created)),
        _ => None,
    });
    Ok(fired.collect())
}

async fn a_firing_says_what_kind_of_change_fired_it(store: Store) -> Held {
    use ChangeKind::{Forgotten, Ingested, Purged, Recomputed, Retired, Upserted};
    let s = store.as_ref();
    let config = json!({"token": "trigger_change", "echo": "code"});
    let l = super::backings::places(s, config).await?;
    let sourced = |n: &str| {
        let mut f = field(n, StorageKind::Text);
        f.authority = lakelands_api::ontology::Authority::Source;
        f
    };
    l.declare(type_decl("reading", vec![sourced("site")]))
        .await?;
    for ty in ["place", "reading"] {
        for (name, when, quiet) in [("any", None, false), ("made", Some("created()"), false)]
            .into_iter()
            .chain([("quiet", None, true)])
        {
            let lakelands_api::write::Declaration::Trigger(mut t) =
                trigger_on(&format!("{name}_{ty}"), ty, when)
            else {
                unreachable!("trigger_on declares a trigger")
            };
            t.on_changes = quiet.then(|| vec![Upserted, Ingested]);
            let fut = l
                .cat
                .declare(&l.own, lakelands_api::write::Declaration::Trigger(t));
            fut.await.must(&format!("declaring `{name}_{ty}`"))?;
        }
    }
    let row = |t: &str, change, created| (t.to_owned(), Some(change), Some(created));
    let step = async |what: &str,
                      want: Vec<(String, Option<ChangeKind>, Option<bool>)>,
                      from: Position|
           -> Held {
        let got = fired(s, from).await?;
        ensure!(got == want, "{what} fires {want:?}; it fired {got:?}");
        Ok(())
    };
    // A create, then the fill its call makes.
    let at = s.head().await.must("the head")?.next();
    l.upsert("place", "p1", json!({"code": "AT"})).await?;
    let want = vec![
        row("any_place", Upserted, true),
        row("made_place", Upserted, true),
        row("quiet_place", Upserted, true),
        row("any_place", Recomputed, false),
    ];
    step("`p1` written and filled", want, at).await?;
    // An update: the write found `p1` live, so `created()` does not hold;
    // the fill the new code calls for may follow.
    let at = s.head().await.must("the head")?.next();
    l.upsert("place", "p1", json!({"code": "DE"})).await?;
    let updated = [
        row("any_place", Upserted, false),
        row("quiet_place", Upserted, false),
    ];
    let fired_now = fired(s, at).await?;
    ensure!(
        fired_now.starts_with(&updated),
        "`p1` updated fires {updated:?} first; it fired {fired_now:?}"
    );
    let at = s.head().await.must("the head")?.next();
    l.retire("place", "p1").await?;
    step("`p1` retired", vec![row("any_place", Retired, false)], at).await?;
    // A revival is a creation to a guard; its fill reads the output the
    // lake holds already.
    let at = s.head().await.must("the head")?.next();
    l.upsert("place", "p1", json!({"code": "AT"})).await?;
    let fired_again = fired(s, at).await?;
    let revived = [
        row("any_place", Upserted, true),
        row("made_place", Upserted, true),
        row("quiet_place", Upserted, true),
    ];
    ensure!(
        fired_again.starts_with(&revived),
        "`p1` revived fires {revived:?} first; it fired {fired_again:?}"
    );
    // A source's records: ingested, then purged; ingested from another,
    // then forgotten.
    for (source, key) in [("meter", "r1"), ("panel", "r2")] {
        let at = s.head().await.must("the head")?.next();
        let record = source_record("reading", key, json!({"site": "north"}));
        l.ingest(source, vec![record])
            .await
            .must(&format!("ingesting `{key}`"))?;
        let want = vec![
            row("any_reading", Ingested, true),
            row("made_reading", Ingested, true),
            row("quiet_reading", Ingested, true),
        ];
        step(&format!("`{key}` ingested"), want, at).await?;
    }
    let at = s.head().await.must("the head")?.next();
    let purged = s.purge_source(&SourceId::from("meter"), None, None).await;
    purged.must("purging `meter`")?;
    step(
        "`meter` purged",
        vec![row("any_reading", Purged, false)],
        at,
    )
    .await?;
    let at = s.head().await.must("the head")?.next();
    let forgot = s.forget_source(&SourceId::from("panel"), None).await;
    forgot.must("forgetting `panel`")?;
    step(
        "`panel` forgotten",
        vec![row("any_reading", Forgotten, false)],
        at,
    )
    .await
}

async fn a_firing_across_an_edge_drops_reads_strictly_and_names_its_kind(store: Store) -> Held {
    use lakelands_api::write::Declaration;
    let s = store.as_ref();
    let l = Lake::of(s);
    let text = StorageKind::Text;
    l.declare(type_decl("customer", vec![field("grade", text)]))
        .await?;
    let invoice = vec![field("note", text), reference("customer", "customer")];
    l.declare(type_decl("invoice", invoice)).await?;
    let (x, not_x) = ("customer.grade = 'x'", "not (customer.grade = 'x')");
    for (name, when, depth, strict, on) in [
        ("graded", x, Some(1), false, None),
        ("ingested", x, None, false, Some(vec![ChangeKind::Ingested])),
        ("lax", not_x, None, false, None),
        ("strict", not_x, None, true, None),
    ] {
        let Declaration::Trigger(mut t) = trigger_on(name, "invoice", Some(when)) else {
            unreachable!("trigger_on declares a trigger")
        };
        (t.max_depth, t.when_strict, t.on_changes) = (depth, strict, on);
        let fut = l.cat.declare(&l.own, Declaration::Trigger(t));
        fut.await.must(&format!("declaring `{name}`"))?;
    }
    l.upsert("customer", "c1", json!({"grade": "y"})).await?;
    l.upsert("invoice", "i1", json!({"customer": "c1"})).await?;
    let own = &l.own;
    let fired_at = |depth: u32, grade: serde_json::Value| async move {
        let cat = s.catalog(reactive(depth, None));
        let wrote = try_upsert(cat.as_ref(), own, "customer", "c1", json!({"grade": grade}));
        let what = format!("`c1` graded {grade} at depth {depth}");
        let at = wrote.await.must(&what)?.position();
        Ok::<_, Failure>((what, set_off(s, at).await?, fired(s, at).await?))
    };
    let upserted = |t: &str| (t.to_owned(), Some(ChangeKind::Upserted), Some(false));
    let dropped = Some((1, 1, DepthBound::Trigger));
    for (depth, grade, want, kinds) in [
        // The customer's write fires `graded` at the invoice, an update of
        // another record, and not `ingested`, which waits for an ingest.
        (
            0,
            json!("x"),
            vec![("graded", None)],
            vec![upserted("graded")],
        ),
        // Its grade gone, the strict guard does not fire where the lax one
        // does.
        (0, json!(null), vec![("lax", None)], vec![upserted("lax")]),
        // One deeper than `graded` allows a reaction to, it drops.
        (1, json!("x"), vec![("graded", dropped)], vec![]),
    ] {
        let (what, got, fired) = fired_at(depth, grade).await?;
        let want: Vec<_> = want.into_iter().map(|(t, d)| (t.to_owned(), d)).collect();
        ensure!(
            got == want && fired == kinds,
            "{what} sets off {want:?} at `i1`, each firing saying {kinds:?}; it set off \
             {got:?}, saying {fired:?}"
        );
    }
    // The customer retired reads null across the edge: again the lax guard
    // fires and the strict one does not, and the firing says so.
    let at = l.retire("customer", "c1").await?.position();
    let (got, kinds) = (set_off(s, at).await?, fired(s, at).await?);
    let retired = ("lax".to_owned(), Some(ChangeKind::Retired), Some(false));
    ensure!(
        got == [("lax".to_owned(), None)] && kinds == [retired],
        "`c1` retired sets off `lax` alone at `i1`, saying it was retired; it set off {got:?}, \
         saying {kinds:?}"
    );
    Ok(())
}

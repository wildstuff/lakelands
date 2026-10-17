//! What a trigger's firing says, and when it fires: past the depth a
//! reaction could write at it drops instead
//! (`docs/decisions/021-the-write-context-belongs-to-the-host.md`).

use lakelands_api::event::{DepthBound, EventBody};
use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::FeedFilter;
use lakelands_api::{Position, TypeName};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, events_from, field, followed, limits, reactive, trigger_on, try_upsert, type_decl,
};
use crate::conformance::{Failure, Held, Store};

check!(
    TRIGGER_DROPPED = a_trigger_past_its_depth_drops_instead_of_firing,
    "021, extended (lakelands#1216): a write whose depth is at or above a trigger's `max_depth`, or the lake's `max_reactive_depth`, appends `trigger_dropped` naming the trigger, the record, the write's depth, the bound that applied, the lower of the two, and whose it is, in place of `trigger_fired`; below both the trigger fires, and a reaction one deeper naming that firing is written; the feed tells `trigger_dropped` to a consumer that asks for it"
);

check!(
    TRIGGER_STRICT = a_strict_guard_fails_where_a_field_it_reads_has_no_value,
    "004, extended (lakelands#1215): a trigger declared `when_strict` fires only where every field its `when` reads, and every `previous(f)`, has a value: `not (status = 'paid')` fires on a record without `status` by default and not when strict, and a guard reading `previous(status)` fires on a create by default and not when strict; a field written null fails it as one absent; a strict guard reads every operand of an `or`, so `amount > 100 or status = 'paid'` fails on a record without `status` in either order; once the fields have values the two fire alike"
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
    let mut fired_at = None;
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
        if depth == 1 {
            fired_at = Some(at);
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

//! What a trigger's firing says, and when it fires: past the depth a
//! reaction could write at it drops instead
//! (`docs/decisions/021-the-write-context-belongs-to-the-host.md`).

use lakelands_api::event::{DepthBound, EventBody};
use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::FeedFilter;
use lakelands_api::{Position, Refusal, TypeName};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, events_from, field, followed, head_unmoved, limits, reactive, refused_on,
    trigger_on, try_upsert, type_decl,
};
use crate::conformance::{Failure, Held, Store};

check!(
    TRIGGER_DROPPED = a_trigger_past_its_depth_drops_instead_of_firing,
    "021, extended (lakelands#1216): a write whose depth is at or above a trigger's `max_depth`, or the lake's `max_reactive_depth`, appends `trigger_dropped` naming the trigger, the record, the write's depth, the bound that applied, the lower of the two, and whose it is, in place of `trigger_fired`; below both the trigger fires, and a reaction one deeper naming that firing is written, while one naming a drop is refused `too_deep` naming the trigger, appending nothing; the feed tells `trigger_dropped` to a consumer that asks for it"
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

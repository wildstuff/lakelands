//! A set's declaration decides the membership of the records already
//! there (`docs/decisions/004-the-change-feed-is-the-log.md`, extended).

use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::tools::Door;

use crate::conformance::fixture::type_of;
use crate::conformance::prelude::*;

check!(
    SET_DECLARED_OVER_RECORDS = a_sets_declaration_decides_the_membership_of_the_records_there,
    "004, extended: a set declared over records already written appends, in its declaration's append and caused by its `declared`, an `object_set_changed` joining each live record its filter holds for and no other; declared again with another filter, it joins and leaves exactly the records the change moves; declared again unchanged, it appends none; and a later write decides against the membership so kept; past the lake's `max_set_changes` a declaration is refused `too_large`, naming the set and the ceiling, and appends nothing, at `declare` and at the accept of its proposal, where a name that became a tool's since is refused first (013, lakelands#935)",
    given
);

check!(
    SET_RECOMPUTED = a_type_declared_again_decides_the_sets_its_computed_fields_move,
    "004, extended (lakelands#941): a type declared again with another derived expression appends, in its declaration's append and caused by its `declared`, the `object_set_changed` of each record whose set the recomputed field moves, a set that names such a set too, and a later write of the record appends none; a set declared or declared again in the same bundle is decided by its new filter over the records as the bundle computes them; a field computed outside the lake keeps its value through that walk, and its set moves where its output lands; past the lake's `max_set_changes` the declaration is refused `too_large` naming a set, and appends nothing",
    given
);

async fn a_sets_declaration_decides_the_membership_of_the_records_there(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_of("invoice", &[("status", Text), ("amount", Number)]))
        .await?;
    let rows = [
        ("i1", "open", 50),
        ("i2", "open", 200),
        ("i3", "paid", 300),
        ("i4", "open", 0),
    ];
    for (key, status, amount) in rows {
        l.upsert("invoice", key, json!({"status": status, "amount": amount}))
            .await?;
    }
    l.retire("invoice", "i4").await?;
    let set = || object_set_on("still_open", "invoice", "status = 'open'");
    // The membership changes of the append from `from`, as (key, joined),
    // those caused by its `declared`.
    let changes = |from| {
        let store = store.clone();
        async move {
            let events = events_from(store.as_ref(), from).await?;
            let declared = events
                .iter()
                .find(|e| matches!(e.body, EventBody::Declared { .. }));
            let at = declared.map(|e| e.position);
            let changed = events.into_iter().filter_map(|e| match e.body {
                EventBody::ObjectSetChanged { key, joined, .. } if e.cause == at => {
                    Some((key.to_string(), joined))
                }
                _ => None,
            });
            Ok::<_, Failure>(changed.collect::<Vec<_>>())
        }
    };
    let pair = |k: &str, joined| (k.to_owned(), joined);
    for (filter, want) in [
        ("status = 'open'", vec![pair("i1", true), pair("i2", true)]),
        ("amount > 100", vec![pair("i1", false), pair("i3", true)]),
        ("amount > 100", vec![]),
    ] {
        let from = store.head().await.must("reading the head")?.next();
        let answer = l.declare_as(object_set_on("open", "invoice", filter), filter);
        let answer = answer.await?;
        let got = changes(from).await?;
        // The answer names `open` once however many records it moves, and
        // `through` covers them (015, extended).
        let named = |n: &[String]| n.iter().all(|s| s == "open") && n.len() <= 1;
        ensure!(
            matches!(&answer, Outcome::Applied { set_off, .. } if named(&set_off.joined)
                && named(&set_off.left) && set_off.through.is_some() == !want.is_empty()),
            "declaring `open` as {filter:?} answers each set once, `through` its changes; it \
             answered {answer:?}"
        );
        ensure!(
            got == want,
            "declaring `open` as {filter:?} over i1–i4 (i4 retired) must change {want:?}; its \
             append changed {got:?}"
        );
    }
    let from = store.head().await.must("reading the head")?.next();
    l.upsert("invoice", "i3", json!({"amount": 10})).await?;
    let left = events_from(store.as_ref(), from).await?.into_iter().any(|e| {
        matches!(e.body, EventBody::ObjectSetChanged { joined: false, ref key, .. } if key.as_str() == "i3")
    });
    ensure!(
        left,
        "`i3`, a member by the declaration, leaves `open` when it no longer holds"
    );
    // Proposed now and accepted past the ceiling: the set alone, and with
    // a verb named as the find tool of `memo`, declared since.
    // The name from the one generator, for the lake as if `memo` were declared.
    let mut lake = l.describe().await?;
    let mut memo = lake.types[0].clone();
    memo.name = "memo".into();
    lake.types.push(memo);
    let find = Door::Find { ty: "memo".into() };
    let name = tools(&lake)
        .unwrap_or_default()
        .into_iter()
        .find(|t| t.door == find);
    let name = name
        .ok_or_else(|| Failure::new("no find tool for `memo`"))?
        .name;
    let verb = json!({"declare": "verb", "name": name, "on": "invoice", "risk": "low",
                      "effect": "write", "set": [{"field": "status", "value": "'x'"}]});
    let verb: Declaration = serde_json::from_value(verb).expect("a verb reads");
    let mut ids = Vec::new();
    for declarations in [vec![set(), verb], vec![set()]] {
        let proposed = l.cat.propose(
            &l.own,
            ProposedWrite::Declare {
                declarations,
                plan: None,
            },
            "why".into(),
        );
        let Outcome::Proposed { proposal, .. } = proposed.await.must("a proposal")? else {
            return Err(Failure::new("a declaration is proposed"));
        };
        ids.push(proposal);
    }
    l.declare(type_of("memo", &[("n", Text)])).await?;
    // `i1` and `i2` would join `still_open`: two changes, past a ceiling of one.
    let lower = Limits {
        max_set_changes: 1,
        ..limits()
    };
    let store = (given.relimit)(store, Seams::default(), lower)
        .await
        .map_err(|e| Failure::new(format!("opening the lake again with a lower ceiling: {e}")))?;
    let (l, before) = (
        Lake::of(store.as_ref()),
        store.head().await.must("the head")?,
    );
    let named = l.cat.accept(&l.own, &ids[0]).await;
    let large = l.cat.accept(&l.own, &ids[1]).await;
    // Each declaration of the set is refused for the name, the set's too.
    let named_only = match invalid_case(&named) {
        Some(Case::SetRefused { problems, .. }) => {
            (problems.iter()).all(|p| matches!(invalid_case(p), Some(Case::ToolNames { .. })))
        }
        _ => false,
    };
    ensure!(
        named_only
            && matches!(
                refused_on(&large),
                Some(("still_open", Refusal::TooLarge { max: 1 }))
            ),
        "the accept of `still_open` past the ceiling is refused too_large, and with a verb \
         named as a tool first for the name; they answered {named:?} and {large:?}"
    );
    let r = l.cat.declare(&l.own, set()).await;
    ensure!(
        matches!(
            refused_on(&r),
            Some(("still_open", Refusal::TooLarge { max: 1 }))
        ),
        "a set that would change 2 memberships under a `max_set_changes` of 1 is refused \
         too_large naming it; it answered {r:?}"
    );
    head_unmoved(store.as_ref(), before, "the refused declaration").await
}

async fn a_type_declared_again_decides_the_sets_its_computed_fields_move(
    store: Store,
    given: Given,
) -> Held {
    let bill = |k: u32| -> Declaration {
        let twice = format!("amount * {k}");
        serde_json::from_value(json!({"declare": "type", "name": "bill", "fields": [
            {"name": "amount", "type": {"kind": "number"}},
            {"name": "note", "type": {"kind": "text"}},
            {"name": "twice", "type": {"kind": "number"}, "authority": "derived", "derived": twice}]}))
        .expect("a type")
    };
    let l = Lake::of(store.as_ref());
    (l.cat.declare(&l.own, bill(2)).await).must("bill")?;
    l.declare_as(object_set_on("big", "bill", "twice > 150"), "big")
        .await?;
    l.declare_as(object_set_on("named", "bill", "in_set('big')"), "named")
        .await?;
    for (key, amount) in [("b1", 100), ("b2", 50), ("b3", 400)] {
        l.upsert("bill", key, json!({"amount": amount})).await?;
    }
    // Twice now `amount`: `b1` leaves both sets, two changes; past a
    // ceiling of one, the declaration is refused and appends nothing.
    let lower = Limits {
        max_set_changes: 1,
        ..limits()
    };
    let opened = (given.relimit)(store, Seams::default(), lower).await;
    let store = opened.map_err(|e| Failure::new(format!("opening at a ceiling of one: {e}")))?;
    let (l, before) = (
        Lake::of(store.as_ref()),
        store.head().await.must("the head")?,
    );
    let r = l.cat.declare(&l.own, bill(1)).await;
    ensure!(
        matches!(
            refused_on(&r),
            Some(("big" | "named", Refusal::TooLarge { max: 1 }))
        ),
        "`bill` declared again, moving 2 memberships under a `max_set_changes` of 1, is refused \
         too_large naming a set; it answered {r:?}"
    );
    head_unmoved(store.as_ref(), before, "the refused declaration").await?;
    let backed = Seams {
        backings: crate::conformance::backing::backings(),
        ..Seams::default()
    };
    let opened = (given.relimit)(store, backed, limits()).await;
    let store = opened.map_err(|e| Failure::new(format!("opening again: {e}")))?;
    let l = Lake::of(store.as_ref());
    let from = store.head().await.must("the head")?.next();
    (l.cat.declare(&l.own, bill(1)).await).must("bill again")?;
    let after = store.head().await.must("the head")?.next();
    l.upsert("bill", "b1", json!({"note": "1"})).await?;
    let (moved, later) = (
        changes(store.as_ref(), from).await?,
        events_from(store.as_ref(), after).await?,
    );
    let late = (later.iter()).any(|e| matches!(e.body, EventBody::ObjectSetChanged { .. }));
    ensure!(
        moved == ["big b1 false", "named b1 false"] && !late,
        "`bill` declared again leaves `b1` from `big` and from `named` in its own append, \
         caused by its `declared`, and `b1`'s next write leaves nothing; it moved {moved:?}, \
         and the write {}",
        ["appended none", "appended a change"][usize::from(late)]
    );
    // A bundle decides its own sets by their new filters over the records
    // as it computes them: `twice` doubled again (`b1` 200, `b2` 100, `b3`
    // 800), `big` past 250, so `b1` does not join by the old filter, and
    // `fresh` past 150 joins `b1` by its new `twice`.
    let set = |name, filter| object_set_on(name, "bill", filter);
    let bundle = vec![
        bill(2),
        set("big", "twice > 250"),
        set("fresh", "twice > 150"),
    ];
    let bundled = store.head().await.must("the head")?.next();
    (l.cat.apply(&l.own, bundle).await).must("the bundle")?;
    let got = changes(store.as_ref(), bundled).await?;
    ensure!(
        got == ["fresh b1 true", "fresh b3 true"],
        "a bundle of `bill` and its sets decides them by their new filters over the records \
         it computes: `fresh` joins `b1` and `b3`, `big` and `named` move none; it moved {got:?}"
    );
    // A field computed outside the lake keeps its value through the walk:
    // `ebig` reads `ext`, an echo of `twice`, and nothing moves at the
    // declaration; `t1` leaves where its output lands, `t3` never.
    let echo = json!({"declare": "function", "name": "echo", "backing": crate::conformance::backing::KIND,
        "inputs": [{"name": "twice", "type": {"kind": "number"}}],
        "config": {"token": "set_recomputed", "echo": "twice"}});
    l.declare_json(echo).await?;
    let tick = |k: u32| -> Declaration {
        let twice = format!("amount * {k}");
        serde_json::from_value(json!({"declare": "type", "name": "tick", "fields": [
            {"name": "amount", "type": {"kind": "number"}},
            {"name": "twice", "type": {"kind": "number"}, "authority": "derived", "derived": twice},
            {"name": "ext", "type": {"kind": "number"}, "authority": "derived",
             "function": "echo", "depends_on": ["twice"]}]}))
        .expect("a type")
    };
    (l.cat.declare(&l.own, tick(2)).await).must("tick")?;
    let ebig = object_set_on("ebig", "tick", "ext > 150");
    l.declare_as(ebig, "ebig").await?;
    for (key, amount) in [("t1", 100), ("t3", 400)] {
        l.upsert("tick", key, json!({"amount": amount})).await?;
    }
    let from = store.head().await.must("the head")?.next();
    (l.cat.declare(&l.own, tick(1)).await).must("tick again")?;
    let at_declared = changes(store.as_ref(), from).await?;
    let events = events_from(store.as_ref(), from).await?;
    let left: Vec<String> = (events.iter())
        .filter_map(|e| match &e.body {
            EventBody::ObjectSetChanged { key, joined, .. } => Some(format!("{key} {joined}")),
            _ => None,
        })
        .collect();
    ensure!(
        at_declared.is_empty() && !left.iter().any(|c| c == "t3 false"),
        "a field computed outside the lake keeps its value through the walk: nothing moves \
         at the declaration ({at_declared:?}) and `t3`, 800 either way, never leaves \
         ({left:?})"
    );
    Ok(())
}

/// The set changes caused by the last `declared` of the appends from
/// `from`, where a bundle's walk decides them, as `set key joined`.
async fn changes(
    store: &dyn lakelands_api::LakeStore,
    from: lakelands_api::Position,
) -> Result<Vec<String>, Failure> {
    let events = events_from(store, from).await?;
    let at = (events.iter())
        .rfind(|e| matches!(e.body, EventBody::Declared { .. }))
        .map(|e| e.position);
    let changed = (events.iter()).filter_map(|e| match &e.body {
        EventBody::ObjectSetChanged {
            set, key, joined, ..
        } if e.cause == at => Some(format!("{set} {key} {joined}")),
        _ => None,
    });
    Ok(changed.collect())
}

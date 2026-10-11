//! A set's declaration decides the membership of the records already
//! there (`docs/decisions/004-the-change-feed-is-the-log.md`, extended).

use lakelands_api::error::Case;
use lakelands_api::event::EventBody;
use lakelands_api::location::Limits;
use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::seam::Seams;
use lakelands_api::tools::{Door, tools};
use lakelands_api::write::{Declaration, ProposedWrite};
use lakelands_api::{Outcome, Refusal};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, events_from, head_unmoved, invalid_case, limits, object_set_on, refused_on, type_of,
};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    SET_DECLARED_OVER_RECORDS = a_sets_declaration_decides_the_membership_of_the_records_there,
    "004, extended: a set declared over records already written appends, in its declaration's append and caused by its `declared`, an `object_set_changed` joining each live record its filter holds for and no other; declared again with another filter, it joins and leaves exactly the records the change moves; declared again unchanged, it appends none; and a later write decides against the membership so kept; past the lake's `max_set_changes` a declaration is refused `too_large`, naming the set and the ceiling, and appends nothing, at `declare` and at the accept of its proposal, where a name that became a tool's since is refused first (013, lakelands#935)",
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
            ProposedWrite::Declare { declarations },
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

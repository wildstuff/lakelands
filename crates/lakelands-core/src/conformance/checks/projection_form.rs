//! What a projection may hold in a form of its own and still answer as it
//! did (`docs/decisions/098-a-lake-stores-what-it-cannot-derive.md`): a
//! record's flags kept beside its state, its key named by a number made
//! in the order keys arrive, and one order index walked both ways. Each is
//! where an engine that stores less may drift from one that stores more,
//! each consistent with itself: a count that reads a flag the state no
//! longer says, a page in the order keys arrived instead of the order of
//! their text, a window down whose ties come out reversed.

use lakelands_api::read::Found;

use crate::conformance::fixture::{order, type_of};
use crate::conformance::prelude::*;

check!(
    FLAGS_FOLLOW_THE_STATE = a_retire_a_restriction_and_their_undoing_move_the_count,
    "098: a retire, a restriction and their undoing move the count of a type, at the head and at an earlier position: `find` with `total` and `retired` answers the live records and the retired apart, at the head after each step for the owner and for a reader without `restricted`, who is not counted a held record, and as of each step's position for the owner; a record lifted from its hold, or written again after its retire, counts as live again from the position that undid it"
);

check!(
    KEY_ORDER_THROUGH_KEYS = a_page_in_key_order_and_a_tie_answer_by_the_keys_text,
    "098: a page in key order and a tie in a window answer as they did: keys written in an order other than their text's, under two types sharing them, are paged by the text, at the head and at an earlier position, by one and by three; records tied on the order field ascend by key in either direction, with an order index and without; and a feed asked for one key answers that key's events under both types and no other"
);

check!(
    DESCENDING_WITHOUT_ITS_INDEX = a_window_down_is_a_window_up_reversed_ties_ascending,
    "098: a window down answers as a window up reversed, ties ascending: over a field with an order index and over one without, paged by one, two, three and five, so that a run of tied records crosses a page's end, and at an earlier position, a window down by the field answers its values descending and each run of tied records by key ascending, records without the field last"
);

/// `ty`, a number `n` and a text `tag`.
async fn declared(l: &Lake, ty: &str) -> Held {
    let t = type_of(
        ty,
        &[("n", StorageKind::Number), ("tag", StorageKind::Text)],
    );
    l.declare(t).await.map(drop)
}

/// The total and the retired count a first page of `find` answers for
/// `who`, at `as_of`.
async fn counted(
    l: &Lake,
    who: &Principal,
    ty: &str,
    as_of: Option<Position>,
) -> Result<(u64, u64), Failure> {
    let find = Find {
        total: true,
        retired: true,
        limit: Some(1),
        at: read_at(as_of),
        ..Find::default()
    };
    let found = l.cat.find(who, &ty.into(), find).await;
    match found.must(&format!("a count of `{ty}` at {as_of:?}"))?.data {
        Found::Records {
            total: Some(n),
            retired: Some(r),
            ..
        } => Ok((n, r)),
        other => Err(Failure::new(format!(
            "a page of `{ty}` with `total` and `retired` answered no counts: {other:?}"
        ))),
    }
}

/// The keys every page of `find` answers, walked to the end.
async fn walked(l: &Lake, ty: &str, find: Find) -> Result<Vec<String>, Failure> {
    let rows = l.all_found(ty, find).await?;
    Ok(rows.iter().map(|r| r.key.to_string()).collect())
}

async fn a_retire_a_restriction_and_their_undoing_move_the_count(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    declared(&l, "flagged").await?;
    for key in ["a", "b", "c", "d"] {
        l.upsert("flagged", key, json!({"n": 1})).await?;
    }
    let reader = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let reader = principal(store.as_ref(), PrincipalKind::Agent, "reader", reader);
    // After each step, the live and the retired a page counts at the head
    // for the owner and for a reader without `restricted`; each step's
    // position is read again at the end, as the owner.
    let mut at = Vec::new();
    for step in 0..5 {
        match step {
            0 => {}
            1 => drop(l.retire("flagged", "b").await?),
            2 => drop(restrict(store.as_ref(), "flagged", "c", "held").await?),
            3 => {
                let lifted = store.unrestrict(&"flagged".into(), &"c".into()).await;
                lifted.must("lifting the hold on `c`")?;
            }
            _ => drop(l.upsert("flagged", "b", json!({"n": 2})).await?),
        }
        at.push(store.head().await.must("the head")?);
        let want = [
            [(4, 0), (4, 0)], // all four in force
            [(3, 1), (3, 1)], // `b` retired
            [(3, 1), (2, 1)], // `c` restricted
            [(3, 1), (3, 1)], // `c` lifted
            [(4, 0), (4, 0)], // `b` written again
        ][step];
        let got = [
            counted(&l, &l.own, "flagged", None).await?,
            counted(&l, &reader, "flagged", None).await?,
        ];
        ensure!(
            got == want,
            "after step {step} the counts of `flagged` at the head, (live, retired) for the \
             owner and for a reader without `restricted`, are {want:?}; it answered {got:?}"
        );
    }
    let want = [(4, 0), (3, 1), (3, 1), (3, 1), (4, 0)];
    for (&pos, &want) in at.iter().zip(&want) {
        let got = counted(&l, &l.own, "flagged", Some(pos)).await?;
        ensure!(
            got == want,
            "the owner's counts of `flagged` as of {pos}, (live, retired), are {want:?}; it \
             answered {got:?}"
        );
    }
    Ok(())
}

async fn a_page_in_key_order_and_a_tie_answer_by_the_keys_text(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    declared(&l, "first").await?;
    declared(&l, "second").await?;
    l.set("sort_index.second", json!(["n"])).await?;
    // Written in an order that is neither the keys' text nor its reverse,
    // the two types interleaved, so a number made per key in arrival
    // order sorts differently from the text, under either type.
    let arrival = ["m", "c", "x", "a", "q", "e", "z", "b", "k", "d"];
    let mut half = None;
    for (i, key) in arrival.iter().enumerate() {
        let n = i % 3;
        l.upsert("first", key, json!({"n": n})).await?;
        l.upsert("second", key, json!({"n": n})).await?;
        if i == arrival.len() / 2 - 1 {
            half = Some(store.head().await.must("the head")?);
        }
    }
    let mut sorted: Vec<String> = arrival.iter().map(|k| (*k).to_owned()).collect();
    sorted.sort();
    let mut early: Vec<String> = arrival[..arrival.len() / 2]
        .iter()
        .map(|k| (*k).to_owned())
        .collect();
    early.sort();
    for ty in ["first", "second"] {
        for (as_of, want) in [(None, &sorted), (half, &early)] {
            for limit in [1, 3] {
                let find = Find {
                    limit: Some(limit),
                    at: read_at(as_of),
                    ..Find::default()
                };
                let got = walked(&l, ty, find).await?;
                ensure!(
                    got == *want,
                    "a walk of `{ty}` in key order by {limit} as of {as_of:?} answers the keys \
                     in the order of their text, {want:?}; it answered {got:?}"
                );
            }
        }
        for dir in [Direction::Asc, Direction::Desc] {
            let mut want: Vec<(u64, String)> = arrival
                .iter()
                .enumerate()
                .map(|(i, k)| ((i % 3) as u64, (*k).to_owned()))
                .collect();
            want.sort_by(|a, b| match dir {
                Direction::Asc => a.cmp(b),
                Direction::Desc => b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)),
            });
            let want: Vec<String> = want.into_iter().map(|(_, k)| k).collect();
            let find = Find {
                order_by: vec![order("n", OrderBy::Value, dir)],
                limit: Some(2),
                ..Find::default()
            };
            let got = walked(&l, ty, find).await?;
            ensure!(
                got == want,
                "a walk of `{ty}` by `n` {dir:?} by 2 answers each tie by key ascending, \
                 {want:?}; it answered {got:?}"
            );
        }
    }
    let one = FeedFilter {
        keys: vec!["q".into()],
        ..FeedFilter::default()
    };
    let events = followed(store.as_ref(), Position::GENESIS, &one).await?;
    let about: Vec<(String, String)> = events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::RecordUpserted { ty, key, .. } => Some((ty.to_string(), key.to_string())),
            _ => None,
        })
        .collect();
    let want = [("first", "q"), ("second", "q")].map(|(t, k)| (t.to_owned(), k.to_owned()));
    ensure!(
        about == want && events.len() == want.len(),
        "a feed asked for the key `q` answers its write under `first` and under `second` and \
         nothing else; it answered {about:?} of {} events",
        events.len()
    );
    Ok(())
}

async fn a_window_down_is_a_window_up_reversed_ties_ascending(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    declared(&l, "indexed").await?;
    declared(&l, "plain").await?;
    l.set("sort_index.indexed", json!(["n"])).await?;
    // Runs of ties of one to four records, written out of key order, and
    // two records without `n`.
    let values: [(&str, Option<u32>); 12] = [
        ("k07", Some(2)),
        ("k02", Some(5)),
        ("k11", Some(2)),
        ("k04", Some(1)),
        ("k09", None),
        ("k01", Some(2)),
        ("k05", Some(5)),
        ("k10", Some(3)),
        ("k03", Some(2)),
        ("k08", Some(5)),
        ("k06", None),
        ("k12", Some(1)),
    ];
    let mut half = None;
    for (i, (key, n)) in values.iter().enumerate() {
        let fields = n.map_or_else(|| json!({"tag": "none"}), |n| json!({"n": n}));
        l.upsert("indexed", key, fields.clone()).await?;
        l.upsert("plain", key, fields).await?;
        if i == 7 {
            half = Some(store.head().await.must("the head")?);
        }
    }
    let expect = |upto: usize| {
        let mut valued: Vec<(u32, &str)> = values[..upto]
            .iter()
            .filter_map(|(k, n)| n.map(|n| (n, *k)))
            .collect();
        valued.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        let mut none: Vec<&str> = values[..upto]
            .iter()
            .filter(|(_, n)| n.is_none())
            .map(|(k, _)| *k)
            .collect();
        none.sort_unstable();
        let mut keys: Vec<String> = valued.iter().map(|(_, k)| (*k).to_owned()).collect();
        keys.extend(none.iter().map(|k| (*k).to_owned()));
        keys
    };
    for ty in ["indexed", "plain"] {
        for (as_of, want) in [(None, expect(values.len())), (half, expect(8))] {
            for limit in [1, 2, 3, 5] {
                let find = Find {
                    order_by: vec![order("n", OrderBy::Value, Direction::Desc)],
                    limit: Some(limit),
                    at: read_at(as_of),
                    ..Find::default()
                };
                let got = walked(&l, ty, find).await?;
                ensure!(
                    got == want,
                    "a window down `{ty}` by `n` by {limit} as of {as_of:?} answers the values \
                     descending, each tie by key ascending and the records without `n` last, \
                     {want:?}; it answered {got:?}"
                );
            }
        }
    }
    Ok(())
}

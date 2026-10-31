//! Communities, the pattern query's whole-graph measure of which records
//! are more linked among themselves than to the rest: Louvain in integers
//! over the links a walk would follow, records visited in `(type, key)`
//! order and each community split into its connected parts, so every
//! engine answers the same rows whatever order it reads links in
//! (`docs/decisions/018-one-pattern-query.md`,
//! `docs/decisions/026-the-store-walks-the-graph.md`).

use super::graph_rank::{PAGES, ring};
use crate::conformance::fixture::{doors, owner, step};
use crate::conformance::prelude::*;

check!(
    COMMUNITIES_PINNED = communities_answer_the_pinned_partition_of_two_dense_groups_and_a_bridge,
    "018 and 026: `communities` over two groups of four, every pair in each linked and one link between them, and a record linked to nothing answers each group as a community and the lone record as one of its own, numbered and ordered as `components` orders them, where `components` answers the groups as one; with no start type the lone record is no community"
);

check!(
    COMMUNITIES_ANY_ORDER =
        communities_are_the_same_whatever_order_links_were_written_in_and_seeds_fall_into_pages,
    "026: `communities` over the 300 records `page_rank` ranks in any order, whose seeds fall in two pages, answers byte for byte the same rows on two lakes whose records and links were written in opposite orders",
    given
);

check!(
    COMMUNITY_CONNECTED = a_community_is_connected,
    "026: where the integer Louvain leaves `k01`, `k05`, `k08` and `k09` one community though no link joins `k01` and `k09` to `k05` and `k08`, `communities` answers its two connected parts as two communities, every community connected"
);

check!(
    COMMUNITIES_TOO_LARGE = communities_past_max_graph_nodes_are_refused_too_large,
    "018 and the location's limits: `communities` over a graph of more records than `max_graph_nodes` is refused too_large naming the ceiling, nothing partial is answered, and the lake's head does not move",
    given
);

/// Declares `page`, whose `links` are link records.
async fn declare(l: &Lake) -> Held {
    let mut page = type_decl("page", vec![field("name", StorageKind::Text)]);
    page.relations = vec![RelationDecl {
        cardinality: Cardinality::ManyToMany,
        ..RelationDecl::to("links", "page")
    }];
    l.declare(page).await?;
    Ok(())
}

/// The pages `keys`, and a `links` record from each `from` to its `to`.
async fn write(l: &Lake, keys: &[&str], links: &[(&str, &str)]) -> Held {
    for p in keys {
        l.upsert("page", p, json!({"name": p})).await?;
    }
    for (from, to) in links {
        link(l, from, to).await?;
    }
    Ok(())
}

async fn link(l: &Lake, from: &str, to: &str) -> Held {
    l.link("page", from, "links", to).await?;
    Ok(())
}

/// `project` from the pages over `links` both ways.
fn over_links(project: Projection) -> Pattern {
    pattern("page", None, vec![step("links", Walk::Both)], project)
}

/// The rows a query answers, each as `number/size/key`.
async fn found(
    cat: &dyn Catalog,
    who: &Principal,
    p: Pattern,
    what: &str,
) -> Result<Vec<String>, Failure> {
    let column = match p.project {
        Projection::Components => "component",
        _ => "community",
    };
    let reply = cat.query(who, p).await.must(what)?;
    let QueryResult::Rows { rows, .. } = reply.data else {
        return Err(Failure::new(format!(
            "{what} answered something other than rows"
        )));
    };
    let cell = |row: &BTreeMap<String, Value>, c: &str| match row.get(c) {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => "-".to_owned(),
    };
    let show = |r| format!("{}/{}/{}", cell(r, column), cell(r, "size"), cell(r, "key"));
    Ok(rows.iter().map(show).collect())
}

/// Two groups of four, every pair linked, and one link between them.
const GROUPS: [(&str, &str); 13] = [
    ("a", "b"),
    ("a", "c"),
    ("a", "d"),
    ("b", "c"),
    ("b", "d"),
    ("c", "d"),
    ("d", "e"),
    ("e", "f"),
    ("e", "g"),
    ("e", "h"),
    ("f", "g"),
    ("f", "h"),
    ("g", "h"),
];

const KEYS: [&str; 9] = ["a", "b", "c", "d", "e", "f", "g", "h", "z"];

async fn communities_answer_the_pinned_partition_of_two_dense_groups_and_a_bridge(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    declare(&l).await?;
    write(&l, &KEYS, &GROUPS).await?;
    let Lake { cat, own } = &l;
    let p = over_links(Projection::Communities);
    let got = found(cat.as_ref(), own, p, "the pages' communities").await?;
    let want = [
        "0/4/a", "0/4/b", "0/4/c", "0/4/d", "1/4/e", "1/4/f", "1/4/g", "1/4/h", "2/1/z",
    ];
    ensure!(
        got == want,
        "the two groups are two communities and z one of its own, largest first, then by \
         their least key; it answered {got:?}"
    );
    let p = over_links(Projection::Components);
    let parts = found(cat.as_ref(), own, p, "the pages' components").await?;
    ensure!(
        parts.len() == 9 && parts[7] == "0/8/h",
        "`components` answers the two groups as one component of eight; it answered {parts:?}"
    );
    let mut unstarted = over_links(Projection::Communities);
    unstarted.start.ty = None;
    let got = found(cat.as_ref(), own, unstarted, "unstarted communities").await?;
    ensure!(
        got == want[..8],
        "with no start type the eight linked pages are the two communities, and z, linked to \
         nothing, is none; it answered {got:?}"
    );
    Ok(())
}

async fn communities_are_the_same_whatever_order_links_were_written_in_and_seeds_fall_into_pages(
    store: Store,
    given: Given,
) -> Held {
    ring(store.as_ref(), false).await?;
    let twin = (given.twin)(store.lake().clone(), Default::default()).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    ring(twin.as_ref(), true).await?;
    let all = || {
        let steps = ["to_next", "to_jump", "cites"].map(|r| step(r, Walk::Both));
        Pattern {
            limit: Some(PAGES as u32),
            ..pattern("page", None, steps.to_vec(), Projection::Communities)
        }
    };
    let mut answers = Vec::new();
    for (lake, what) in [(&store, "written in order"), (&twin, "written in reverse")] {
        let reply = doors(lake.as_ref())
            .query(&owner(lake.as_ref()), all())
            .await
            .must(&format!("communities of the ring {what}"))?;
        let QueryResult::Rows { rows, .. } = reply.data else {
            return Err(Failure::new(format!(
                "communities of the ring {what} answered something other than rows"
            )));
        };
        let last = rows.last().and_then(|r| r.get("community"));
        ensure!(
            rows.len() == PAGES && last.and_then(Value::as_u64) > Some(1),
            "communities of the ring {what} hold all {PAGES} pages in more than two \
             communities; it answered {} rows, the last numbered {last:?}",
            rows.len()
        );
        answers.push(serde_json::to_string(&rows).expect("rows serialise"));
    }
    ensure!(
        answers[0] == answers[1],
        "communities answer the same bytes whatever order the ring was written in; in order \
         they answered {}, in reverse {}",
        answers[0],
        answers[1]
    );
    Ok(())
}

async fn a_community_is_connected(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    declare(&l).await?;
    let keys = [
        "k00", "k01", "k02", "k03", "k04", "k05", "k06", "k07", "k08", "k09", "k10", "k11",
    ];
    let one_way = [
        ("k09", "k01"),
        ("k02", "k04"),
        ("k07", "k06"),
        ("k00", "k04"),
        ("k07", "k03"),
        ("k00", "k11"),
        ("k02", "k01"),
        ("k06", "k09"),
        ("k07", "k10"),
    ];
    let both_ways = [
        ("k00", "k09"),
        ("k05", "k08"),
        ("k00", "k02"),
        ("k00", "k10"),
        ("k10", "k11"),
        ("k06", "k05"),
        ("k06", "k03"),
        ("k02", "k11"),
        ("k00", "k08"),
    ];
    let mut links = one_way.to_vec();
    links.extend(both_ways.iter().flat_map(|&(u, v)| [(u, v), (v, u)]));
    write(&l, &keys, &links).await?;
    let Lake { cat, own } = &l;
    let p = over_links(Projection::Communities);
    let got = found(cat.as_ref(), own, p, "the twelve pages' communities").await?;
    let want = [
        "0/5/k00", "0/5/k02", "0/5/k04", "0/5/k10", "0/5/k11", "1/3/k03", "1/3/k06", "1/3/k07",
        "2/2/k01", "2/2/k09", "3/2/k05", "3/2/k08",
    ];
    ensure!(
        got == want,
        "the community Louvain leaves as k01, k05, k08 and k09 is answered as its connected \
         parts, k01 and k09, and k05 and k08; it answered {got:?}"
    );
    Ok(())
}

async fn communities_past_max_graph_nodes_are_refused_too_large(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    declare(&l).await?;
    write(&l, &KEYS, &GROUPS).await?;
    let limits = |max_graph_nodes| Limits {
        max_graph_nodes,
        ..crate::conformance::fixture::limits()
    };
    let store = (given.relimit)(store, Seams::default(), limits(9)).await;
    let store = store.map_err(|e| Failure::new(format!("reopening the lake: {e}")))?;
    let got = found(
        doors(store.as_ref()).as_ref(),
        &owner(store.as_ref()),
        over_links(Projection::Communities),
        "communities of nine pages under a ceiling of 9",
    )
    .await?;
    ensure!(
        got.len() == 9,
        "under a ceiling of 9 communities hold the nine pages; they answered {got:?}"
    );
    let store = (given.relimit)(store, Seams::default(), limits(8)).await;
    let store = store.map_err(|e| Failure::new(format!("reopening the lake: {e}")))?;
    let before = store.head().await.must("reading the head")?;
    let err = doors(store.as_ref())
        .query(&owner(store.as_ref()), over_links(Projection::Communities))
        .await
        .must_fail("communities past a ceiling of 8")?;
    ensure!(
        matches!(reason(&err), Some(Refusal::TooLarge { max: 8 })),
        "communities of nine pages past a ceiling of 8 are refused too_large naming 8; they \
         failed with {err:?}"
    );
    crate::conformance::fixture::head_unmoved(
        store.as_ref(),
        before,
        "communities past the ceiling",
    )
    .await
}

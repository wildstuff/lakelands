//! PageRank, the pattern query's whole-graph measure of how much each
//! record is linked to: computed in integers over the links a walk would
//! follow, so every engine answers the same bits whatever order it reads
//! links in (`docs/decisions/018-one-pattern-query.md`,
//! `docs/decisions/026-the-store-walks-the-graph.md`).

use lakelands_api::Value;
use lakelands_api::write::WriteOpts;

use crate::conformance::fixture::{doors, owner, reference, source_record, step};
use crate::conformance::prelude::*;

check!(
    RANK_PINNED = page_rank_answers_the_integer_scores_of_a_small_graph_highest_first,
    "018 and 026: `page_rank` over a cycle, a record linked to itself, one linked to by others and linking nowhere, and one linked to nothing answers every record the start reaches with its score to the bit, highest first and ties by (type, key), the scores summing to exactly 1; a start by key reaches what its walk reaches only"
);

check!(
    RANK_ANY_ORDER = page_rank_is_the_same_whatever_order_its_links_were_written_in,
    "026: `page_rank` over 300 records, whose seeds fall in two pages, answers byte for byte the same rows on two lakes whose records and links were written in opposite orders, and its scores sum to exactly 1",
    given
);

check!(
    RANK_NO_START = page_rank_with_no_start_type_grows_from_every_linked_record,
    "018: `page_rank` with no start type ranks every record that holds a link it may see over the steps' relations, and no record linked to nothing, which a typed start counts; a step naming an inverse walked one way ranks as the relation walked the other"
);

check!(
    RANK_TOO_LARGE = page_rank_past_max_graph_nodes_is_refused_too_large,
    "018 and the location's limits: `page_rank` over a graph of more records than `max_graph_nodes` is refused too_large naming the ceiling, nothing partial is answered, and the lake's head does not move",
    given
);

/// Pages a..f, linked by `cites` records, whose inverse is `cited_by`:
/// a → b → c → a is a cycle, c cites d, which cites nothing, e cites
/// itself and a, and f is cited by none and cites none.
async fn lake(store: &dyn LakeStore) -> Held {
    let l = Lake::of(store);
    let mut page = type_decl("page", vec![field("name", StorageKind::Text)]);
    page.relations = vec![RelationDecl {
        cardinality: Cardinality::ManyToMany,
        inverse: Some("cited_by".into()),
        ..RelationDecl::to("cites", "page")
    }];
    l.declare(page).await?;
    for p in ["a", "b", "c", "d", "e", "f"] {
        l.upsert("page", p, json!({"name": p})).await?;
    }
    for (from, to) in [
        ("a", "b"),
        ("b", "c"),
        ("c", "a"),
        ("c", "d"),
        ("e", "e"),
        ("e", "a"),
    ] {
        cite(&l, from, to).await?;
    }
    Ok(())
}

async fn cite(l: &Lake, from: &str, to: &str) -> Held {
    l.cat
        .link(
            &l.own,
            &"page".into(),
            &from.into(),
            &"cites".into(),
            &to.into(),
            patch(json!({})),
            WriteOpts::default(),
        )
        .await
        .must(&format!("linking {from} to {to}"))?;
    Ok(())
}

/// `page_rank` from the pages, or the one at `key`, over `cites` out.
fn ranked(key: Option<&str>) -> Pattern {
    let steps = vec![step("cites", Walk::Out)];
    pattern("page", key, steps, Projection::PageRank)
}

/// The rows a query answers, each as `key=page_rank`.
async fn ranks(
    cat: &dyn Catalog,
    who: &Principal,
    p: Pattern,
    what: &str,
) -> Result<Vec<String>, Failure> {
    let reply = cat.query(who, p).await.must(what)?;
    let QueryResult::Rows { rows, .. } = reply.data else {
        return Err(Failure::new(format!(
            "{what} answered something other than rows"
        )));
    };
    Ok(rows.iter().map(show).collect())
}

fn show(row: &BTreeMap<String, Value>) -> String {
    let cell = |c: &str| match row.get(c) {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => "-".to_owned(),
    };
    format!("{}={}", cell("key"), cell("page_rank"))
}

/// The scores of `rows` in units of 10^-12, summed: a score is an
/// integer of those units, which its float holds exactly enough to give
/// back.
fn mass(rows: &[String]) -> Option<u64> {
    let unit = |r: &String| {
        let (_, v) = r.rsplit_once('=')?;
        let v: f64 = v.parse().ok()?;
        Some((v * 1e12).round() as u64)
    };
    rows.iter().map(unit).sum()
}

async fn page_rank_answers_the_integer_scores_of_a_small_graph_highest_first(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let got = ranks(cat.as_ref(), own, ranked(None), "the pages' PageRank").await?;
    let want = [
        "c=0.250876597769",
        "b=0.229313324066",
        "a=0.20394476677",
        "d=0.162582826365",
        "e=0.097322212717",
        "f=0.055960272313",
    ];
    ensure!(
        got == want,
        "PageRank over `cites` answers every page, damped at 85/100 in integers of 10^-12 \
         over 100 rounds, highest first: c, then b, a, d (which cites nothing), e (which \
         cites itself) and f (linked to nothing); it answered {got:?}"
    );
    ensure!(
        mass(&got) == Some(1_000_000_000_000),
        "the scores sum to exactly 1; they sum to {:?} units of 10^-12",
        mass(&got)
    );
    // From c the walk out reaches a, b and d; a and d come apart by one
    // unit, the one the even spread hands the first record by (type, key).
    let got = ranks(cat.as_ref(), own, ranked(Some("c")), "PageRank from c").await?;
    let want = [
        "c=0.307853403141",
        "b=0.264622288706",
        "a=0.213762154077",
        "d=0.213762154076",
    ];
    ensure!(
        got == want,
        "PageRank from c ranks what its walk reaches, a, b, c and d, and not e; it answered \
         {got:?}"
    );
    Ok(())
}

/// The records of the larger lake.
pub(super) const PAGES: usize = 300;

/// The source that writes the larger lake.
const RING: &str = "ring";

/// 300 pages `p000`… : each links to the next by `next` but every fifth,
/// every third jumps 37 on by `jump`, every fiftieth from 7 jumps to
/// itself, and every seventh `cites` the page 13 times its number on;
/// the records and the links written in that order, or `reversed`.
pub(super) async fn ring(store: &dyn LakeStore, reversed: bool) -> Held {
    let l = Lake::of(store);
    let by_source = |name: &str| FieldDecl {
        authority: Authority::Source,
        ..reference(name, "page")
    };
    let carried = |name: &str, via: &str| RelationDecl {
        cardinality: Cardinality::ManyToOne,
        via: Some(via.into()),
        ..RelationDecl::to(name, "page")
    };
    let fields = vec![
        sourced("name", StorageKind::Text),
        by_source("next"),
        by_source("jump"),
    ];
    let mut page = type_decl("page", fields);
    page.relations = vec![
        carried("to_next", "next"),
        carried("to_jump", "jump"),
        RelationDecl {
            cardinality: Cardinality::ManyToMany,
            ..RelationDecl::to("cites", "page")
        },
    ];
    l.declare(page).await?;
    let key = |i: usize| format!("p{:03}", i % PAGES);
    // The pages first, then their references, which name live records.
    let (mut pages, mut linked, mut cites) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..PAGES {
        let mut fields = json!({"name": key(i)});
        pages.push(source_record("page", &key(i), fields.clone()));
        if i % 5 != 4 {
            fields["next"] = json!(key(i + 1));
        }
        if i % 50 == 7 {
            fields["jump"] = json!(key(i));
        } else if i % 3 == 0 {
            fields["jump"] = json!(key(i * 37 + 11));
        }
        if i % 7 == 0 {
            cites.push((key(i), key(i * 13 + 5)));
        }
        linked.push(source_record("page", &key(i), fields));
    }
    if reversed {
        pages.reverse();
        linked.reverse();
        cites.reverse();
    }
    l.ingest(RING, pages).await.must("ingesting the ring")?;
    l.ingest(RING, linked).await.must("linking the ring")?;
    for (from, to) in cites {
        cite(&l, &from, &to).await?;
    }
    Ok(())
}

async fn page_rank_is_the_same_whatever_order_its_links_were_written_in(
    store: Store,
    given: Given,
) -> Held {
    ring(store.as_ref(), false).await?;
    let twin = (given.twin)(store.lake().clone(), Default::default()).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    ring(twin.as_ref(), true).await?;
    let all = || {
        let steps = ["to_next", "to_jump", "cites"].map(|r| step(r, Walk::Out));
        Pattern {
            limit: Some(PAGES as u32),
            ..pattern("page", None, steps.to_vec(), Projection::PageRank)
        }
    };
    let mut answers = Vec::new();
    for (lake, what) in [(&store, "written in order"), (&twin, "written in reverse")] {
        let reply = doors(lake.as_ref())
            .query(&owner(lake.as_ref()), all())
            .await
            .must(&format!("PageRank over the ring {what}"))?;
        let QueryResult::Rows { rows, .. } = reply.data else {
            return Err(Failure::new(format!(
                "PageRank over the ring {what} answered something other than rows"
            )));
        };
        let shown: Vec<String> = rows.iter().map(show).collect();
        ensure!(
            rows.len() == PAGES && mass(&shown) == Some(1_000_000_000_000),
            "PageRank over the ring {what} ranks all {PAGES} pages, summing to exactly 1; it \
             answered {} rows summing to {:?} units of 10^-12",
            rows.len(),
            mass(&shown)
        );
        answers.push(serde_json::to_string(&rows).expect("rows serialise"));
    }
    ensure!(
        answers[0] == answers[1],
        "PageRank answers the same bytes whatever order the ring was written in; in order it \
         answered {}, in reverse {}",
        answers[0],
        answers[1]
    );
    Ok(())
}

async fn page_rank_with_no_start_type_grows_from_every_linked_record(store: Store) -> Held {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut unstarted = ranked(None);
    unstarted.start.ty = None;
    let got = ranks(cat.as_ref(), own, unstarted, "unstarted PageRank").await?;
    let want = [
        "c=0.265747923961",
        "b=0.242906434275",
        "a=0.216034093469",
        "d=0.17222032251",
        "e=0.103091225785",
    ];
    ensure!(
        got == want,
        "with no start type PageRank ranks the five pages that hold a link, and not f, \
         linked to nothing; it answered {got:?}"
    );
    let typed = ranks(cat.as_ref(), own, ranked(None), "PageRank from the pages").await?;
    ensure!(
        typed.len() == 6 && typed[5] == "f=0.055960272313",
        "from every page, f is a start and is ranked; it answered {typed:?}"
    );
    // A step naming the inverse is `cites` walked from its owner's end, so
    // `cited_by` walked one way ranks as `cites` walked the other.
    let unstarted = |relation: &str, walk| {
        let mut p = pattern(
            "page",
            None,
            vec![step(relation, walk)],
            Projection::PageRank,
        );
        p.start.ty = None;
        p
    };
    for (walk, back) in [(Walk::In, Walk::Out), (Walk::Out, Walk::In)] {
        let what = format!("unstarted PageRank over `cited_by` walked {walk:?}");
        let inverse = ranks(cat.as_ref(), own, unstarted("cited_by", walk), &what).await?;
        let what = format!("unstarted PageRank over `cites` walked {back:?}");
        let cites = ranks(cat.as_ref(), own, unstarted("cites", back), &what).await?;
        ensure!(
            inverse == cites,
            "`cited_by` walked {walk:?} ranks as `cites` walked {back:?}, {cites:?}; it \
             answered {inverse:?}"
        );
        ensure!(
            walk == Walk::Out || inverse == want,
            "`cited_by` walked in ranks as `cites` walked out, {want:?}; it answered {inverse:?}"
        );
    }
    let back = ranks(
        cat.as_ref(),
        own,
        unstarted("cites", Walk::In),
        "`cites` in",
    )
    .await?;
    ensure!(
        back != want,
        "`cites` walked in ranks otherwise than walked out, so the pairs above tell the ways \
         apart; it answered {back:?}"
    );
    Ok(())
}

async fn page_rank_past_max_graph_nodes_is_refused_too_large(store: Store, given: Given) -> Held {
    lake(store.as_ref()).await?;
    let limits = |max_graph_nodes| Limits {
        max_graph_nodes,
        ..crate::conformance::fixture::limits()
    };
    let store = (given.relimit)(store, Seams::default(), limits(6)).await;
    let store = store.map_err(|e| Failure::new(format!("reopening the lake: {e}")))?;
    let got = ranks(
        doors(store.as_ref()).as_ref(),
        &owner(store.as_ref()),
        ranked(None),
        "PageRank over six pages under a ceiling of 6",
    )
    .await?;
    ensure!(
        got.len() == 6,
        "under a ceiling of 6 PageRank ranks the six pages; it answered {got:?}"
    );
    let store = (given.relimit)(store, Seams::default(), limits(5)).await;
    let store = store.map_err(|e| Failure::new(format!("reopening the lake: {e}")))?;
    let before = store.head().await.must("reading the head")?;
    let err = doors(store.as_ref())
        .query(&owner(store.as_ref()), ranked(None))
        .await
        .must_fail("PageRank past a ceiling of 5")?;
    ensure!(
        matches!(reason(&err), Some(Refusal::TooLarge { max: 5 })),
        "PageRank over six pages past a ceiling of 5 is refused too_large naming 5; it failed \
         with {err:?}"
    );
    let mut unstarted = ranked(None);
    unstarted.start.ty = None;
    let err = doors(store.as_ref())
        .query(&owner(store.as_ref()), unstarted)
        .await;
    ensure!(
        err.is_ok(),
        "with no start type the five linked pages fit a ceiling of 5; it answered {err:?}"
    );
    crate::conformance::fixture::head_unmoved(store.as_ref(), before, "PageRank past the ceiling")
        .await
}

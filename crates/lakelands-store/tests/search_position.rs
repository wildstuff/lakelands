//! A search's candidates and its judgement are of one state of the head
//! (lakelands#826, `docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
//!
//! The engines' indexes describe the head as it stands when they are
//! asked, and a search takes its head before it asks. A write that lands
//! in between was in the candidates and not in the judgement: a visible
//! record was answered with a held record's score. The engine now says the
//! position its rankings were read at, and the search is judged there.
//!
//! The rows run through the doors on the memory engine, behind the wrapper
//! (`probed`) that lets a write land just before the engine answers a
//! `ranked`, and counts them.

mod probed;

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::ontology::{Cardinality, FieldDecl, RelationDecl, StorageKind, TypeDecl};
use lakelands_api::query::{Projection, Search, SearchMode, Walk};
use lakelands_api::seam::Seams;
use lakelands_api::write::{Migration, WriteOpts};
use lakelands_api::{Access, Cursor, LakeStore, Location, Position, Principal, PrincipalKind};
use lakelands_core::conformance::fixture::{
    Lake, doors, field, field_access, field_writer, owner, patch, pattern, principal, read_at,
    restrict, searchable, step, type_decl, vector,
};
use lakelands_engine_mem::MemoryOpener;
use probed::{Probe, ProbedOpener};
use serde_json::json;

/// `keyed`: its key made of `key` (`code` or `alt`), a number `n`, a
/// searchable `body` and a vector named `e`.
fn keyed(key: &str, e: &str) -> TypeDecl {
    let [mut code, mut alt] = ["code", "alt"].map(|n| field(n, StorageKind::Text));
    (code.required, alt.required) = (true, true);
    let n = field("n", StorageKind::Number);
    TypeDecl {
        key: vec![key.into()],
        ..type_decl(
            "keyed",
            vec![code, alt, n, searchable("body"), vector(e, 2)],
        )
    }
}

/// A new lake on the probed memory engine, with `keyed` by `code`.
async fn lake(name: &str) -> (Arc<dyn LakeStore>, Arc<Probe>) {
    let probe = Arc::new(Probe::default());
    let opener: Arc<dyn EngineOpener> = Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()));
    let seams = Seams {
        engines: vec![opener],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory(name), seams, OpenMode::Create)
        .await
        .expect("a handle on the lake");
    Lake::of(store.as_ref())
        .declare(keyed("code", "e"))
        .await
        .unwrap();
    (store, probe)
}

/// The record `code` of `keyed`, written by the owner.
async fn put(store: &dyn LakeStore, code: &str, alt: &str, body: &str, e: [f32; 2], n: u32) {
    let fields = json!({"code": code, "alt": alt, "body": body, "e": e, "n": n});
    let l = Lake::of(store);
    l.upsert("keyed", code, fields).await.unwrap();
}

/// Lets `write` land just before the engine answers the `n`th `ranked`
/// from now: after the search took its head, before its rankings are read.
fn pause<F, W>(probe: &Probe, n: usize, store: &Arc<dyn LakeStore>, write: F)
where
    F: FnOnce(Arc<dyn LakeStore>) -> W + Send + 'static,
    W: Future<Output = ()>,
{
    let (runtime, store) = (tokio::runtime::Handle::current(), store.clone());
    let write = Box::new(move || runtime.block_on(write(store)));
    *probe.before_ranked.lock().unwrap() = Some((n, write));
    probe.ranked.store(0, Ordering::SeqCst);
}

/// A search's hits, each key with its score in ten-thousandths, the
/// position, and the cursor.
type Searched = (Vec<(String, i64)>, Position, Option<Cursor>);

/// What `who`'s search of `ty` answers.
async fn search_of(
    store: &dyn LakeStore,
    who: &Principal,
    ty: &str,
    mode: &SearchMode,
    at: Option<Position>,
    page: (u32, Option<&str>, Option<Cursor>),
) -> lakelands_api::Result<Searched> {
    let (k, filter, cursor) = page;
    let s = Search {
        ty: ty.into(),
        mode: mode.clone(),
        k,
        cursor,
        filter: filter.map(Into::into),
        with_meta: false,
        matches: false,
        at: read_at(at),
        rerank: None,
    };
    let got = doors(store).search(who, s).await?;
    let scored = |h: &lakelands_api::query::Hit| {
        (h.record.key.0.clone(), (h.score * 10_000.0).round() as i64)
    };
    let hits = got.data.hits.iter().map(scored).collect();
    Ok((hits, got.position, got.data.next_cursor))
}

/// What `who`'s search of `keyed` answers.
async fn search(
    store: &dyn LakeStore,
    who: &Principal,
    mode: &SearchMode,
    at: Option<Position>,
    page: (u32, Option<&str>, Option<Cursor>),
) -> Searched {
    let searched = search_of(store, who, "keyed", mode, at, page).await;
    searched.expect("the search")
}

/// The keys a search answered.
fn keys(searched: &Searched) -> Vec<&str> {
    searched.0.iter().map(|(k, _)| k.as_str()).collect()
}

/// The three searches of the finding: for `quay`, for the nearest to
/// (1, 0), and for both, each with the score `Z` alone answers it at.
fn searches() -> [(SearchMode, Option<i64>); 3] {
    let near = SearchMode::vector("e", vec![1.0, 0.0]);
    let mut both = SearchMode::hybrid("quay", "e");
    if let SearchMode::Hybrid { vector, .. } = &mut both {
        *vector = Some(vec![1.0, 0.0]);
    }
    [
        (SearchMode::lexical("quay"), None),
        (near, Some(6_000)),
        (both, Some(167)),
    ]
}

/// The finding's scenario: `a` is held and holds `quay` and (1, 0); `z`
/// is visible and holds `harbour` and (0.6, 0.8). A migration to the key
/// `alt` lands between the head view and the rankings, and exchanges
/// their keys. A reader without `restricted` is answered the visible
/// record by its own text and vector, never by the held one's, as of the
/// position before and at the head, where the answer is of the position
/// the rankings were read at and names the record by its key there. The
/// plan asks the same, so the engine is asked once, but for a vector read
/// as of the position before: it was the head when the engine was first
/// asked, and judged by the engine there, and is judged by the store once
/// the head moved past it, so the engine is asked again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rekey_between_the_head_and_the_rankings_is_judged_where_they_were_read() {
    for as_of in [true, false] {
        for (i, (mode, score)) in searches().into_iter().enumerate() {
            let (store, probe) = lake(&format!("rekey-{as_of}-{i}")).await;
            let s = store.as_ref();
            put(s, "a", "z", "quay", [1.0, 0.0], 0).await;
            put(s, "z", "a", "harbour", [0.6, 0.8], 0).await;
            restrict(s, "keyed", "a", "legal hold").await.unwrap();
            let p = s.head().await.unwrap();
            pause(&probe, 1, &store, |s| async move {
                let l = Lake::of(s.as_ref());
                l.migrate(keyed("alt", "e")).await.unwrap();
            });
            let w = field_writer(s, "w", "keyed", &[]);
            let (hits, at, _) = search(s, &w, &mode, as_of.then_some(p), (10, None, None)).await;
            let head = s.head().await.unwrap();
            // The visible record's key: `z` at `p`, `a` once re-keyed.
            let (key, read_at) = if as_of { ("z", p) } else { ("a", head) };
            let want: Vec<_> = score.map(|s| (key.to_owned(), s)).into_iter().collect();
            assert_eq!(hits, want, "as of: {as_of}, {mode:?}");
            assert_eq!(at, read_at, "as of: {as_of}, {mode:?}");
            assert!(head > p, "the migration landed in the pause");
            let asked = 1 + usize::from(as_of && score.is_some());
            assert_eq!(probe.ranked.load(Ordering::SeqCst), asked, "{mode:?}");
        }
    }
}

/// A hold placed between the head view and the rankings: a visible record
/// is restricted and written again, and the indexes rank it by what it
/// holds now. A reader without `restricted` is not answered it, by its
/// text or by its vector, at the head or as of the position before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hold_placed_between_the_head_and_the_rankings_keeps_its_record_out() {
    let near = SearchMode::vector("e", vec![0.0, 1.0]);
    let modes = [(SearchMode::lexical("cove"), None), (near, Some(8_000))];
    for as_of in [true, false] {
        for (i, (mode, score)) in modes.iter().enumerate() {
            let (store, probe) = lake(&format!("held-{as_of}-{i}")).await;
            let s = store.as_ref();
            put(s, "v", "x", "quay", [1.0, 0.0], 0).await;
            put(s, "u", "y", "harbour", [0.6, 0.8], 0).await;
            let p = s.head().await.unwrap();
            pause(&probe, 1, &store, |s| async move {
                restrict(s.as_ref(), "keyed", "v", "legal hold")
                    .await
                    .unwrap();
                put(s.as_ref(), "v", "x", "cove", [0.0, 1.0], 0).await;
            });
            let w = field_writer(s, "w", "keyed", &[]);
            let (hits, at, _) = search(s, &w, mode, as_of.then_some(p), (10, None, None)).await;
            let want: Vec<_> = score.map(|s| ("u".to_owned(), s)).into_iter().collect();
            assert_eq!(hits, want, "as of: {as_of}, {mode:?}");
            let head = s.head().await.unwrap();
            assert_eq!(at, if as_of { p } else { head }, "as of: {as_of}");
        }
    }
}

/// A hold lifted between the head view and the rankings: the record is
/// answered as the later position says, at the head and as of the
/// position before, where a record released since is there (006).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hold_lifted_between_the_head_and_the_rankings_answers_its_record() {
    for as_of in [true, false] {
        let (store, probe) = lake(&format!("lifted-{as_of}")).await;
        let s = store.as_ref();
        put(s, "v", "x", "quay", [1.0, 0.0], 0).await;
        restrict(s, "keyed", "v", "legal hold").await.unwrap();
        let p = s.head().await.unwrap();
        pause(&probe, 1, &store, |s| async move {
            let (ty, key) = ("keyed".into(), "v".into());
            s.unrestrict(&ty, &key).await.unwrap();
        });
        let w = field_writer(s, "w", "keyed", &[]);
        let mode = SearchMode::lexical("quay");
        let (hits, at, _) = search(s, &w, &mode, as_of.then_some(p), (10, None, None)).await;
        // ln(4/3): BM25F's idf of a token in the one record (025).
        assert_eq!(hits, [("v".to_owned(), 2_877)], "as of: {as_of}");
        let head = s.head().await.unwrap();
        assert_eq!(at, if as_of { p } else { head }, "as of: {as_of}");
    }
}

/// A write between two pulls of one search. Seventy records tie, and the
/// `where` keeps the last four, so the first pull of 64 candidates shows
/// too few and the search pulls 256. Before that pull is answered, a
/// record of the first batch is written into the `where` and one is
/// added. The page is judged from the second pull alone, at its position:
/// the answer and its cursor name that position, the pages hold each
/// record the `where` keeps there once and none twice, and the engine is
/// asked twice, never a third time for the read made again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_between_two_pulls_is_in_the_page_once() {
    let (store, probe) = lake("pulls").await;
    let s = store.as_ref();
    for i in 0..70 {
        put(
            s,
            &format!("r{i:02}"),
            &format!("x{i:02}"),
            "quay",
            [1.0, 0.0],
            i,
        )
        .await;
    }
    pause(&probe, 2, &store, |s| async move {
        put(s.as_ref(), "r00", "x00", "quay", [1.0, 0.0], 99).await;
        put(s.as_ref(), "r66a", "x66a", "quay", [1.0, 0.0], 66).await;
    });
    let (own, mode) = (owner(s), SearchMode::lexical("quay"));
    let first = search(s, &own, &mode, None, (3, Some("n >= 66"), None)).await;
    let head = s.head().await.unwrap();
    assert_eq!(first.1, head, "the page is of the second pull's position");
    assert_eq!(probe.ranked.load(Ordering::SeqCst), 2);
    let rest = search(s, &own, &mode, None, (3, Some("n >= 66"), first.2)).await;
    assert_eq!((rest.1, &rest.2), (head, &None), "the cursor reads there");
    let keys: Vec<String> = (first.0.into_iter().chain(rest.0))
        .map(|(k, _)| k)
        .collect();
    assert_eq!(keys, ["r00", "r66", "r66a", "r67", "r68", "r69"]);
}

/// A declaration changed between the head view and the rankings: a
/// migration renames the vector field. The rankings were asked under the
/// name the earlier head gave, which holds no vectors any more; the read
/// made again plans under the new name, asks again, and answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_field_renamed_between_the_head_and_the_rankings_is_asked_again() {
    let (store, probe) = lake("renamed").await;
    let s = store.as_ref();
    put(s, "a", "z", "quay", [1.0, 0.0], 0).await;
    let p = s.head().await.unwrap();
    pause(&probe, 1, &store, |s| async move {
        let l = Lake::of(s.as_ref());
        l.migrate_renaming(keyed("code", "f"), &[("e", "f")])
            .await
            .unwrap();
    });
    let mode = SearchMode::vector("e", vec![1.0, 0.0]);
    let (hits, at, _) = search(s, &owner(s), &mode, Some(p), (10, None, None)).await;
    assert_eq!((hits, at), (vec![("a".to_owned(), 10_000)], p));
    assert_eq!(probe.ranked.load(Ordering::SeqCst), 2);
}

/// `keyed` by `code` with more: `fields`, and a relation `near` to itself
/// carried by link records, which hold the vector `lv`. `relation` names
/// it otherwise.
fn keyed_with(fields: Vec<FieldDecl>, relation: &str) -> TypeDecl {
    let mut ty = keyed("code", "e");
    ty.fields.extend(fields);
    ty.relations = vec![RelationDecl {
        name: relation.into(),
        target: "keyed".into(),
        cardinality: Cardinality::ManyToMany,
        fields: vec![vector("lv", 2)],
        ..Default::default()
    }];
    ty
}

/// Links `from` to `to` over `near`, the link holding (1, 0).
async fn link(store: &dyn LakeStore, from: &str, to: &str) {
    let (cat, own) = (doors(store), owner(store));
    let (ty, relation, opts) = ("keyed".into(), "near".into(), WriteOpts::default());
    let (from, to, fields) = (from.into(), to.into(), patch(json!({"lv": [1.0, 0.0]})));
    let linked = cat.link(&own, &ty, &from, &relation, &to, fields, opts);
    linked.await.expect("the link");
}

/// The scenario of the rows that place a hold in the pause: `v`, `u` and
/// `t` are visible, `v` and `t` each linked to `u`. In the pause `v` is
/// restricted and written again, holding `cove`, (0, 1) and, where the
/// type has `g`, (0, 1) there: every ranking for those puts it first. `u`
/// holds (0.6, 0.8) and `t` (1, 0). Answers the position before the pause.
async fn hold_placed(store: &Arc<dyn LakeStore>, probe: &Probe, more: Vec<FieldDecl>) -> Position {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let fused = !more.is_empty();
    l.migrate(keyed_with(more, "near")).await.unwrap();
    for (code, alt, e) in [
        ("v", "x", [1.0, 0.0]),
        ("u", "y", [0.6, 0.8]),
        ("t", "w", [1.0, 0.0]),
    ] {
        let mut fields = json!({"code": code, "alt": alt, "body": "quay", "e": e, "n": 0});
        if fused {
            fields["g"] = json!(e);
        }
        l.upsert("keyed", code, fields).await.unwrap();
    }
    link(s, "v", "u").await;
    link(s, "t", "u").await;
    let p = s.head().await.unwrap();
    pause(probe, 1, store, move |s| async move {
        restrict(s.as_ref(), "keyed", "v", "legal hold")
            .await
            .unwrap();
        let mut fields = json!({"code": "v", "alt": "x", "body": "cove", "e": [0.0, 1.0], "n": 0});
        if fused {
            fields["g"] = json!([0.0, 1.0]);
        }
        let l = Lake::of(s.as_ref());
        l.upsert("keyed", "v", fields).await.unwrap();
    });
    p
}

/// The pattern door starts by a search as the search door does. A hold is
/// placed in the pause on the record every ranking puts first. A reader
/// without `restricted` asks for rows, a graph, degrees and components
/// from the nearest to (0, 1), over `near` both ways, as of the position
/// before and at the head: each answers what the same pattern answers
/// when nothing lands in it, which holds `u` and `t` and never `v`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hold_placed_in_the_pause_keeps_its_record_out_of_a_pattern_started_by_a_search() {
    let projections = || {
        let rows = Projection::Rows { fields: Vec::new() };
        [
            rows,
            Projection::Graph,
            Projection::Degree,
            Projection::Components,
        ]
    };
    for as_of in [true, false] {
        for (i, project) in projections().into_iter().enumerate() {
            let (store, probe) = lake(&format!("pattern-{as_of}-{i}")).await;
            let p = hold_placed(&store, &probe, Vec::new()).await;
            let s = store.as_ref();
            let w = field_writer(s, "w", "keyed", &[]);
            let read = || async {
                let mut near = step("near", Walk::Both);
                // A row binds each record under a name of its own.
                if matches!(project, Projection::Rows { .. }) {
                    near.named = Some("peer".into());
                }
                let mut asked = pattern("keyed", None, vec![near], project.clone());
                asked.start.search = Some(SearchMode::vector("e", vec![0.0, 1.0]));
                asked.at = read_at(as_of.then_some(p));
                let got = doors(s).query(&w, asked).await.expect("the pattern");
                (json!(got.data).to_string(), got.position)
            };
            let (raced, calm) = (read().await, read().await);
            assert_eq!(raced, calm, "as of: {as_of}, {project:?}");
            let holds = |key: &str| raced.0.contains(&format!("\"{key}\""));
            assert!(
                holds("u") && holds("t") && !holds("v"),
                "as of: {as_of}, {project:?}: {raced:?}"
            );
            assert!(s.head().await.unwrap() > p, "the hold landed in the pause");
        }
    }
}

/// A fused search, a text with a vector and a vector with another, with
/// the hold placed in the pause: the held record is in every list the
/// engine answers, and moves no score and is no hit. Each answers what it
/// answers when nothing lands in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hold_placed_in_the_pause_keeps_its_record_out_of_a_fused_search() {
    let near = json!({"field": "e", "vector": [0.0, 1.0]});
    let hybrid = json!({"mode": "hybrid", "text": "cove", "field": "e", "vector": [0.0, 1.0]});
    let two = json!({"mode": "vector", "field": "g", "vector": [0.0, 1.0], "also": [near]});
    for as_of in [true, false] {
        for (i, mode) in [&hybrid, &two].into_iter().enumerate() {
            let mode: SearchMode = serde_json::from_value(mode.clone()).unwrap();
            let (store, probe) = lake(&format!("fused-{as_of}-{i}")).await;
            let p = hold_placed(&store, &probe, vec![vector("g", 2)]).await;
            let s = store.as_ref();
            let w = field_writer(s, "w", "keyed", &[]);
            let at = as_of.then_some(p);
            let raced = search(s, &w, &mode, at, (10, None, None)).await;
            let calm = search(s, &w, &mode, at, (10, None, None)).await;
            assert_eq!(raced, calm, "as of: {as_of}, {mode:?}");
            assert_eq!(keys(&raced), ["u", "t"], "as of: {as_of}, {mode:?}");
            assert!(s.head().await.unwrap() > p, "the hold landed in the pause");
        }
    }
}

/// A principal who reads only the records whose `alt` is `alice`. In the
/// pause the record every ranking puts first is given to another: it is
/// written again with `alt` `bob`, holding `cove` and (0, 1). The holder
/// is answered its own record `u`, by its text and by its vector, and not
/// the one given away, as of the position before and at the head.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_given_to_another_in_the_pause_is_no_hit_of_who_held_it() {
    let modes = [
        SearchMode::lexical("cove"),
        SearchMode::vector("e", vec![0.0, 1.0]),
    ];
    for as_of in [true, false] {
        for (i, mode) in modes.iter().enumerate() {
            let (store, probe) = lake(&format!("only-{as_of}-{i}")).await;
            let s = store.as_ref();
            put(s, "v", "alice", "quay", [1.0, 0.0], 0).await;
            put(s, "u", "alice", "cove", [0.6, 0.8], 0).await;
            let p = s.head().await.unwrap();
            pause(&probe, 1, &store, |s| async move {
                put(s.as_ref(), "v", "bob", "cove", [0.0, 1.0], 0).await;
            });
            let only = json!({"default": "read", "types": {"keyed": {"only": {"alt": "alice"}}}});
            let grants = serde_json::from_value(only).unwrap();
            let alice = principal(s, PrincipalKind::Agent, "alice", grants);
            let at = as_of.then_some(p);
            let raced = search(s, &alice, mode, at, (10, None, None)).await;
            let calm = search(s, &alice, mode, at, (10, None, None)).await;
            assert_eq!(raced, calm, "as of: {as_of}, {mode:?}");
            assert_eq!(keys(&raced), ["u"], "as of: {as_of}, {mode:?}");
            assert!(s.head().await.unwrap() > p, "the write landed in the pause");
        }
    }
}

/// A declaration changed in the pause, and with it what a reader may be
/// matched in: a migration declares a searchable field the reader may not
/// read, and a record is written with `cove` in it. The rankings were
/// asked as the earlier head planned them, over every searchable field,
/// which now takes in the new one. The read made again plans over the
/// fields the reader may read, asks again, and answers nothing; the owner
/// is answered the record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_field_the_reader_may_not_read_declared_in_the_pause_matches_nothing_for_it() {
    let (store, probe) = lake("unreadable").await;
    let s = store.as_ref();
    put(s, "v", "x", "quay", [1.0, 0.0], 0).await;
    pause(&probe, 1, &store, |s| async move {
        let l = Lake::of(s.as_ref());
        let mut to = keyed("code", "e");
        to.fields.push(searchable("secret"));
        l.migrate(to).await.unwrap();
        let fields = json!({"code": "v", "alt": "x", "body": "quay", "secret": "cove", "e": [1.0, 0.0], "n": 0});
        l.upsert("keyed", "v", fields).await.unwrap();
    });
    let w = field_access(s, "w", "keyed", &[("secret", Access::None)]);
    let mode = SearchMode::lexical("cove");
    let (hits, at, _) = search(s, &w, &mode, None, (10, None, None)).await;
    assert_eq!((hits, at), (Vec::new(), s.head().await.unwrap()));
    assert_eq!(probe.ranked.load(Ordering::SeqCst), 2);
    let (hits, ..) = search(s, &owner(s), &mode, None, (10, None, None)).await;
    // ln 2: one record over two searched fields, N = 2 (025).
    assert_eq!(hits, [("v".to_owned(), 6_931)]);
}

/// A type renamed in the pause: a link type bears its relation's name,
/// and a migration renames the relation. A search of the link type as it
/// was named, as of the position before, asked the rankings under the
/// name the earlier head gave, which holds no vectors any more; the read
/// made again asks under the name the type bears now, and answers the
/// link. At the head the old name is asked as after the rename.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_link_type_renamed_between_the_head_and_the_rankings_is_asked_again() {
    for as_of in [true, false] {
        let (store, probe) = lake(&format!("link-type-{as_of}")).await;
        let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
        l.migrate(keyed_with(Vec::new(), "near")).await.unwrap();
        put(s, "a", "z", "quay", [1.0, 0.0], 0).await;
        put(s, "z", "a", "harbour", [0.6, 0.8], 0).await;
        link(s, "a", "z").await;
        let p = s.head().await.unwrap();
        pause(&probe, 1, &store, |s| async move {
            let l = Lake::of(s.as_ref());
            let m = Migration {
                relation_renames: vec![("near".into(), "beside".into())],
                ..Migration::to(keyed_with(Vec::new(), "beside"))
            };
            let report = l.cat.migrate(&l.own, m.clone()).await.unwrap();
            let id = report.data.report_id;
            l.cat.migrate_apply(&l.own, m, &id).await.unwrap();
        });
        let (own, mode) = (owner(s), SearchMode::vector("lv", vec![1.0, 0.0]));
        let read = || {
            search_of(
                s,
                &own,
                "sys.link.keyed.near",
                &mode,
                as_of.then_some(p),
                (10, None, None),
            )
        };
        let (raced, calm) = (read().await, read().await);
        assert_eq!(format!("{raced:?}"), format!("{calm:?}"), "as of: {as_of}");
        assert!(
            s.head().await.unwrap() > p,
            "the rename landed in the pause"
        );
        if as_of {
            let (hits, at, _) = raced.expect("the search as of the position before");
            assert_eq!((hits.len(), at), (1, p));
            assert_eq!(
                probe.ranked.load(Ordering::SeqCst),
                3,
                "twice, and once undisturbed"
            );
        }
    }
}

/// A reranked search (067): a write that lands inside the read of its
/// window, just before the engine answers the rankings, and edits a record
/// outside the window so that it ranks into it. The window is read again
/// at that write's position, the reranker is asked about that window, and
/// the answer is every hit it scored, in its order, read where the window
/// was: none is lost to the ranking moving after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_inside_a_reranked_searchs_window_read_loses_no_scored_hit() {
    let (store, probe) = lake("rerank-mid-window").await;
    store
        .bind_rerank(lakelands_core::conformance::backing::reranker())
        .unwrap();
    for (code, body) in [
        ("a1", "invoice overdue"),
        ("a2", "invoice overdue"),
        ("a3", "invoice"),
        ("a4", "invoice"),
        ("z9", "invoice"),
    ] {
        put(store.as_ref(), code, code, body, [1.0, 0.0], 0).await;
    }
    let before = lakelands_core::conformance::backing::rerank_calls().len();
    pause(&probe, 1, &store, |s| async move {
        put(
            s.as_ref(),
            "z9",
            "z9",
            "invoice overdue reminder",
            [1.0, 0.0],
            0,
        )
        .await;
    });
    let search = |cursor| Search {
        ty: "keyed".into(),
        mode: SearchMode::lexical("invoice overdue"),
        k: 1,
        cursor,
        filter: None,
        with_meta: false,
        matches: false,
        at: read_at(None),
        rerank: Some(lakelands_api::query::Rerank { top: 3, text: None }),
    };
    let own = owner(store.as_ref());
    let (mut got, mut cursor) = (Vec::new(), None);
    for _ in 0..5 {
        let page = doors(store.as_ref())
            .search(&own, search(cursor))
            .await
            .unwrap();
        got.extend(page.data.hits.iter().map(|h| h.record.key.0.clone()));
        // A write between pages that would rank `a4` into the window.
        put(
            store.as_ref(),
            "a4",
            "a4",
            "invoice overdue overdue",
            [1.0, 0.0],
            0,
        )
        .await;
        cursor = page.data.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    let calls = lakelands_core::conformance::backing::rerank_calls().split_off(before);
    assert_eq!(calls.len(), 1, "one call for the search");
    let window: Vec<&str> = calls[0].candidates.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(
        window,
        ["a1", "a2", "z9"],
        "the window is read again at the write that landed in its read"
    );
    assert_eq!(
        got,
        ["z9", "a1", "a2"],
        "every scored hit, in the reranker's order, none lost to the writes after the window"
    );
}

//! A vector index asked for answers its set
//! (`docs/decisions/043-an-index-asked-for-answers-its-set.md`,
//! `docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`,
//! `docs/decisions/029-every-answer-has-one-order.md`).
//!
//! A field the lake's `vector_index.<type>` setting names is searched by
//! the engine's index where it keeps one: the answer is the index's set of
//! candidates, ranked by the one cosine, ties by key, and says
//! `ranked_by: index`. The set may miss a neighbour the exact ranking
//! finds; how often is held here, per engine, as the recall of the index
//! against the brute-force reference, averaged over fixed queries. An
//! engine that keeps no such index answers the reference itself, and is
//! held to a recall of 1.
//!
//! **The corpus.** 4 000 vectors of 32 dimensions around 40 centres, so
//! that neighbourhoods are dense and the index has to find its way through
//! them; recall over uniform noise says little. After the setting asks for
//! the index, 40 records are retired, 40 vectors cleared, 40 rewritten
//! towards another centre with their group moved, and 200 records added.
//! 50 queries lie near the centres or between two of them.

use lakelands_api::query::VectorTerm;

use super::search::doc_search;
use crate::conformance::fixture;
use crate::conformance::fixture::doors;
use crate::conformance::prelude::*;

check!(
    VECTOR_INDEX_RECALL = an_indexed_vector_field_answers_its_index_set_at_the_engines_recall_floor,
    "043, 025 and 029: a search over a vector field the `vector_index.<type>` setting names says `ranked_by: index`, ranks the index's candidates by the one cosine with ties by key, answers the same bytes when asked again, and finds on average at least the engine's floor of the brute-force reference's hits for k = 1, 10 and 100, with and without a filter, at the head and at an earlier position, after records were retired, vectors cleared and rewritten and records added; an engine without such an index answers the reference, a recall of 1; a vector search fused over several fields says `index` when any of them is named; it prints, and does not judge, how many hits of the queries' first twelve pages of ten are a record answered before, and how many records of one page of 120 they leave out, since under an index a page is the index's answer at the depth that page asks for",
    given
);

check!(
    INDEXED_TIE = under_an_index_a_tie_it_hands_out_is_paged_once_and_by_key,
    "043 and 029 (lakelands#827): under an index the members of a tie that the index hands out at a page's depth are pulled before the keys cut it, and come once and in key order; and an index that hands out fewer candidates than it was asked for has not shown the field's end: over two hundred records whose vectors are alike and a hundred others, with the index asked for, following the cursor at k = 10 says `ranked_by: index` on every page and answers every record once, the two hundred first and in key order; and over seven hundred alike, of which PostgreSQL's index hands out a part, following it at k = 100 does so too"
);

const TY: &str = "point";
const SOURCE: &str = "points";
const DIMENSIONS: usize = 32;
const CENTRES: u64 = 40;
const POINTS: u64 = 4_000;
const ADDED: u64 = 200;
const QUERIES: u64 = 50;

/// The recall an engine's index holds at `k`, averaged over the queries.
/// PostgreSQL's HNSW index (pgvector, `m = 16`, `ef_construction = 64`,
/// `ef_search` at the candidates pulled, at least 200) measured 0.98 at k = 1, 0.97 to 0.98 at
/// k = 10 and 1.00 at k = 100 on this corpus over several builds, and at
/// least 0.996 with the filter; the floors keep a margin below that. An engine without an approximate index
/// answers the reference.
fn floor(harness: &str, k: u32) -> f64 {
    match (harness, k) {
        ("pg", 1 | 10) => 0.95,
        ("pg", _) => 0.98,
        _ => 1.0,
    }
}

/// A number from `seed`, uniform in [-1, 1), exact in `f32`.
fn uniform(seed: u64) -> f32 {
    let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    ((z >> 40) as f32) / (1u64 << 23) as f32 - 1.0
}

/// Centre `c` of the corpus.
fn centre(c: u64) -> Vec<f32> {
    (0..DIMENSIONS as u64)
        .map(|d| uniform(c * 1_000 + d))
        .collect()
}

/// A point near centre `c`, its offset drawn from `seed`.
fn near(c: u64, seed: u64, spread: f32) -> Vec<f32> {
    centre(c)
        .into_iter()
        .enumerate()
        .map(|(d, x)| x + spread * uniform(seed * 7_919 + d as u64 + 1_000_000))
        .collect()
}

/// The `i`th vector of the corpus.
fn corpus_vector(i: u64) -> Vec<f32> {
    near(i % CENTRES, i, 0.35)
}

/// Query `q`: near a centre, or halfway between two.
fn query(q: u64) -> Vec<f32> {
    if q.is_multiple_of(2) {
        near(q % CENTRES, 900_000 + q, 0.2)
    } else {
        let (a, b) = (centre(q % CENTRES), centre((q * 7 + 3) % CENTRES));
        a.iter().zip(&b).map(|(x, y)| (x + y) / 2.0).collect()
    }
}

/// One `point` as the check wrote it.
struct Point {
    key: String,
    /// Its vector and group at the head; `None` once cleared.
    vector: Option<Vec<f32>>,
    group: u32,
    retired: bool,
    /// Its group at the earlier position, when it existed then.
    then: Option<u32>,
}

fn source_record(key: &str, vector: Option<&[f32]>, group: u32) -> SourceRecord {
    fixture::source_record(TY, key, json!({"v": vector, "group": group}))
}

/// The `i`th `point` of the corpus, as first written.
fn corpus_point(i: u64) -> Point {
    Point {
        key: format!("p{i:05}"),
        vector: Some(corpus_vector(i)),
        group: (i % 4) as u32,
        retired: false,
        then: None,
    }
}

/// A search's hits as their keys and the bits of their scores.
fn bits(h: &lakelands_api::query::Hits) -> Vec<(String, u64)> {
    let hit = |h: &lakelands_api::query::Hit| (h.record.key.0.clone(), h.score.to_bits());
    h.hits.iter().map(hit).collect()
}

/// A vector search of `point.v` for `q`, followed by its cursor at `k`
/// for at most `pages` pages: the keys answered, in the pages' order, and
/// whether every page said it ranked by the index.
async fn paged(l: &Lake, q: &[f32], k: u32, pages: u32) -> Result<(Vec<String>, bool), Failure> {
    let (mut seen, mut cursor, mut indexed) = (Vec::new(), None, true);
    for _ in 0..pages {
        let asked = Search {
            ty: TypeName::from(TY),
            k,
            cursor,
            ..doc_search(SearchMode::vector("v", q.to_vec()))
        };
        let page = l.cat.search(&l.own, asked).await;
        let page = page.must("a page under the index")?.data;
        indexed &= page.ranked_by == "index";
        seen.extend(page.hits.iter().map(|h| h.record.key.0.clone()));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok((seen, indexed))
}

async fn deliver(store: &dyn LakeStore, own: &Principal, records: Vec<SourceRecord>) -> Held {
    let what = format!("ingesting {} points", records.len());
    crate::conformance::fixture::ingest(doors(store).as_ref(), own, SOURCE, records)
        .await
        .must(&what)?;
    Ok(())
}

/// The reference: every vector at the head scored by the one cosine,
/// ranked by `rank` over the records the read sees (live at the head with
/// a vector there, existing at the earlier position when the read is
/// earlier, in `group` when it filters), at most `k`.
fn reference(
    points: &[Point],
    q: &[f32],
    earlier: bool,
    group: Option<u32>,
    k: u32,
) -> Vec<String> {
    let mut all: Vec<(lakelands_api::RecordKey, f64)> = points
        .iter()
        .filter(|p| !p.retired)
        .filter_map(|p| Some((p, p.vector.as_ref()?)))
        .filter(|(p, _)| {
            let seen = if earlier { p.then } else { Some(p.group) };
            seen.is_some_and(|g| group.is_none_or(|want| g == want))
        })
        .map(|(p, v)| (p.key.as_str().into(), lakelands_api::query::cosine(q, v)))
        .collect();
    crate::engines::rank(&mut all, k);
    all.into_iter().map(|(key, _)| key.0).collect()
}

async fn an_indexed_vector_field_answers_its_index_set_at_the_engines_recall_floor(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let source = |f: FieldDecl| FieldDecl {
        authority: Authority::Source,
        ..f
    };
    let v = source(vector("v", DIMENSIONS as u32));
    // A second vector field, which the setting does not name and no
    // record holds.
    let w = source(vector("w", DIMENSIONS as u32));
    let group = source(field("group", StorageKind::Number));
    l.declare(type_decl(TY, vec![v, w, group])).await?;
    let mut points: Vec<Point> = (0..POINTS).map(corpus_point).collect();
    deliver(
        store.as_ref(),
        own,
        points
            .iter()
            .map(|p| source_record(&p.key, p.vector.as_deref(), p.group))
            .collect(),
    )
    .await?;
    l.set("vector_index.point", json!(["v"])).await?;
    let then = store.head().await.must("reading the head")?;
    for p in &mut points {
        p.then = Some(p.group);
    }
    // After the earlier position: retired, cleared, rewritten, added.
    let point = TypeName::from(TY);
    let mut changed = Vec::new();
    for (i, p) in points.iter_mut().enumerate() {
        match i % 100 {
            5 => {
                p.retired = true;
                l.retire(point.as_str(), p.key.as_str()).await?;
            }
            17 => {
                p.vector = Some(near((i as u64 + 11) % CENTRES, i as u64 + 50_000, 0.35));
                p.group = (p.group + 1) % 4;
                changed.push(source_record(&p.key, p.vector.as_deref(), p.group));
            }
            29 => {
                p.vector = None;
                changed.push(source_record(&p.key, None, p.group));
            }
            _ => {}
        }
    }
    for i in POINTS..POINTS + ADDED {
        let p = corpus_point(i);
        changed.push(source_record(&p.key, p.vector.as_deref(), p.group));
        points.push(p);
    }
    deliver(store.as_ref(), own, changed).await?;

    let search = |q: Vec<f32>, k: u32, group: Option<u32>, earlier: bool| {
        let cat = cat.clone();
        let own = own.clone();
        async move {
            cat.search(
                &own,
                Search {
                    ty: TypeName::from(TY),
                    k,
                    filter: group.map(|g| format!("group = {g}").as_str().into()),
                    at: read_at(earlier.then_some(Position(then.0))),
                    ..doc_search(SearchMode::vector("v", q))
                },
            )
            .await
            .map(|r| r.data)
        }
    };
    let by_key: std::collections::HashMap<&str, &Point> =
        points.iter().map(|p| (p.key.as_str(), p)).collect();
    let mut short = Vec::new();
    for earlier in [false, true] {
        for group in [None, Some(1)] {
            for k in [1, 10, 100] {
                let what = format!(
                    "k = {k}{}{}",
                    group.map_or(String::new(), |g| format!(", where group = {g}")),
                    if earlier {
                        format!(", as of {}", then.0)
                    } else {
                        String::new()
                    }
                );
                let mut found = 0.0;
                for qi in 0..QUERIES {
                    let q = query(qi);
                    let got = search(q.clone(), k, group, earlier)
                        .await
                        .must(&format!("a vector search, {what}, query {qi}"))?;
                    ensure!(
                        got.ranked_by == "index",
                        "a search over the indexed `v`, {what}, must say it ranked by the index; \
                         it said {:?}",
                        got.ranked_by
                    );
                    let hits = bits(&got);
                    for (key, bits) in &hits {
                        let head = by_key
                            .get(key.as_str())
                            .and_then(|p| p.vector.as_ref())
                            .map(|v| lakelands_api::query::cosine(&q, v).to_bits());
                        ensure!(
                            head == Some(*bits),
                            "{what}, query {qi}: `{key}` must score the one cosine of its vector \
                             at the head"
                        );
                    }
                    let mut ranked: Vec<(lakelands_api::RecordKey, f64)> = got
                        .hits
                        .iter()
                        .map(|h| (h.record.key.clone(), h.score))
                        .collect();
                    crate::engines::rank(&mut ranked, u32::MAX);
                    ensure!(
                        ranked
                            .iter()
                            .map(|(k, s)| (k.0.clone(), s.to_bits()))
                            .collect::<Vec<_>>()
                            == hits,
                        "{what}, query {qi}: the index's candidates must come best first by the \
                         one cosine, ties by key; they came {hits:?}"
                    );
                    if qi == 0 {
                        let again = search(q.clone(), k, group, earlier)
                            .await
                            .must(&format!("the same search again, {what}"))?;
                        let again = bits(&again);
                        ensure!(
                            again == hits,
                            "{what}: the same search asked twice must answer the same bytes; \
                             it answered {hits:?}, then {again:?}"
                        );
                    }
                    let want = reference(&points, &q, earlier, group, k);
                    let hit = want
                        .iter()
                        .filter(|w| hits.iter().any(|(h, _)| h == *w))
                        .count();
                    found += hit as f64 / want.len().max(1) as f64;
                }
                let recall = found / QUERIES as f64;
                let floor = floor(&given.harness, k);
                eprintln!(
                    "vector index on `{}`: recall {recall:.3} at {what} (floor {floor})",
                    given.harness
                );
                if recall < floor {
                    short.push(format!("{recall:.3} at {what}"));
                }
            }
        }
    }
    // Fused over `w` and, in `also`, the indexed `v`, the answer is the
    // index's set too, and says so (046, extended).
    let fused = SearchMode::Vector {
        field: "w".into(),
        vector: query(0),
        weight: None,
        also: vec![VectorTerm {
            field: "v".into(),
            vector: query(0),
            weight: None,
            model: None,
        }],
        model: None,
    };
    let fused = Search {
        ty: TypeName::from(TY),
        ..doc_search(fused)
    };
    let got = cat.search(own, fused).await;
    let got = got.must("a vector search over `w` and `v`")?.data;
    ensure!(
        got.ranked_by == "index" && !got.hits.is_empty(),
        "a vector search fused over `w` and the indexed `v` must say it ranked by the index; \
         it said {:?} over {} hits",
        got.ranked_by,
        got.hits.len()
    );
    // Pages of one query are not promised to be parts of one sequence
    // (043): a measure, printed and not judged.
    let (mut twice, mut never) = (0, 0);
    for qi in 0..QUERIES {
        let (seen, _) = paged(&l, &query(qi), 10, 12).await?;
        let (whole, _) = paged(&l, &query(qi), 120, 1).await?;
        let once: std::collections::HashSet<&String> = seen.iter().collect();
        twice += seen.len() - once.len();
        never += whole.iter().filter(|key| !once.contains(key)).count();
    }
    eprintln!(
        "vector index on `{}`: twelve pages of ten of each of {QUERIES} queries answered \
         {twice} hits a second time and left out {never} records of one page of 120",
        given.harness
    );
    ensure!(
        short.is_empty(),
        "an indexed field on `{}` must find on average at least its floor of the reference's \
         hits; it found {}",
        given.harness,
        short.join("; ")
    );
    Ok(())
}

async fn under_an_index_a_tie_it_hands_out_is_paged_once_and_by_key(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let source = |f: FieldDecl| FieldDecl {
        authority: Authority::Source,
        ..f
    };
    let group = source(field("group", StorageKind::Number));
    l.declare(type_decl(TY, vec![source(vector("v", 2)), group]))
        .await?;
    // `alike` vectors at the query, the `s` keys, and a hundred apart,
    // the `t` keys: the answer is every `s` in key order, then every `t`.
    let alike = |n: std::ops::Range<u32>| n.map(|i| (format!("s{i:03}"), [1.0, 0.0]));
    let apart = (0..100).map(|i| (format!("t{i:02}"), [1.0, i as f32 + 1.0]));
    let want = |n| alike(0..n).chain(apart.clone()).map(|(key, _)| key);
    let records = |of: Vec<(String, [f32; 2])>| {
        let record = |(key, v): &(String, [f32; 2])| source_record(key, Some(v), 0);
        of.iter().map(record).collect()
    };
    // Two hundred alike: the first page's pull holds 64 of them, and
    // four times as many candidates hold them all and sixty others.
    let first = records(alike(0..200).chain(apart.clone()).collect());
    deliver(store.as_ref(), &l.own, first).await?;
    l.set("vector_index.point", json!(["v"])).await?;
    let (seen, indexed) = paged(&l, &[1.0, 0.0], 10, 30).await?;
    ensure!(
        indexed && seen.iter().cloned().eq(want(200)),
        "pages of ten over two hundred vectors alike and a hundred apart say they ranked by \
         the index ({indexed}) and answer every record once, the two hundred first and in key \
         order; they answered {seen:?}"
    );
    // Seven hundred alike, of which PostgreSQL's index, built over them,
    // hands out a part: its short answer is not the field's end.
    deliver(store.as_ref(), &l.own, records(alike(200..700).collect())).await?;
    l.set("vector_index.point", json!([])).await?;
    l.set("vector_index.point", json!(["v"])).await?;
    let (seen, indexed) = paged(&l, &[1.0, 0.0], 100, 8).await?;
    ensure!(
        indexed && seen.iter().cloned().eq(want(700)),
        "pages of a hundred over seven hundred vectors alike and a hundred apart say they \
         ranked by the index ({indexed}) and answer every record once, the seven hundred \
         first and in key order; they answered {} hits: {seen:?}",
        seen.len()
    );
    Ok(())
}

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
    "043, 025 and 029: a search over a vector field the `vector_index.<type>` setting names says `ranked_by: index`, ranks the index's candidates by the one cosine with ties by key, answers the same bytes when asked again, and finds on average at least the engine's floor of the brute-force reference's hits for k = 1, 10 and 100, with and without a filter, at the head and at an earlier position, after records were retired, vectors cleared and rewritten and records added; an engine without such an index answers the reference, a recall of 1; a vector search fused over several fields says `index` when any of them is named",
    given
);

check!(
    INDEXED_TIE = under_an_index_a_tie_it_hands_out_is_paged_once_and_by_key,
    "078, 043 and 029 (lakelands#827): under an index the members of a tie that the stream hands out are read before the keys cut it, and come once and in key order; and what the stream does not hand out follows it, ranked the same way: over two hundred records whose vectors are alike and a hundred others, with the index asked for, following the cursor at k = 10 says `ranked_by: index` on every page and answers every record once, the two hundred first and in key order; and over seven hundred alike, of which PostgreSQL's graph may hand out a part, following it at k = 100 answers the keys one page of 800 answers, every record once, best first with equal scores by key but at most once, where the rest begins"
);

check!(
    ONE_RANKING_PAGED = under_an_index_a_search_pages_one_ranking_per_class,
    "078: over four thousand vectors with the index asked for and a hundred records held, a vector search followed by its cursor at k = 10 for twelve pages answers, for the owner, who holds `restricted`, and for a reader without it, for each of ten queries, with no filter, a filter hiding a quarter and one keeping about nine hundred, the keys and score bits one page of 120 answers, every page saying `ranked_by: index`, no key twice, the scores never rising; the reader's pages hold no held record and the owner's are the reader's with the held records it may see among them",
    given
);

check!(
    UNSEEN_WRITE_MOVES_NO_PAGE = a_write_the_reader_may_not_see_moves_none_of_its_pages,
    "078: with the index asked for over twelve hundred vectors and some held, a reader without `restricted`, with a filter hiding a quarter and without, followed by the cursor for twelve pages of ten, is answered the same keys and score bits before and after writes it may not see: one held record's vector rewritten at the query, and another's rewritten there and the record retired"
);

check!(
    HOLD_KEEPS_THE_SEQUENCE = a_hold_keeps_the_readers_sequence_across_its_swap,
    "078: with the index asked for over twelve hundred vectors, a reader without `restricted` follows a search's cursor for three pages of ten; a record of its fifth page is held; the pages it then follows, and the whole sequence read again until a second has passed, before and after the engine answers from a graph built without the record, are the sequence of before the hold without that record, to the bit; the owner's sequence keeps it throughout"
);

check!(
    ONLY_IS_RANKED_EXACTLY = a_reader_holding_only_is_ranked_exactly_over_its_pairs,
    "078: with the index asked for over twelve hundred vectors and the text they are paired by looked up, a reader holding `only` on that text, followed by its cursor for twelve pages of ten, is answered the first 120 of the exact ranking of the records holding the text: the keys and score bits of the one cosine, equal scores by key"
);

check!(
    UPGRADE_KEEPS_THE_RANKING = an_upgrade_rebuilds_the_graphs_beside_with_answers_unchanged,
    "078 and 071: a lake whose index is asked for, with a record held, its projection marked one layout back by the harness where it can, opened again by a writer, answers the owner's and a reader's twelve pages of ten of a search, with and without a filter, as before, every page saying `ranked_by: index`",
    given
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

/// One page's hits: keys and score bits.
type Paged = Vec<(String, u64)>;

/// A vector search of `point.v` for `q` by `who`, filtered by `filter`,
/// followed by its cursor at `k` for at most `pages` pages: the hits
/// answered, in the pages' order, and whether every page said it ranked
/// by the index.
async fn paged(
    l: &Lake,
    who: &Principal,
    (q, filter): (&[f32], Option<&str>),
    k: u32,
    pages: u32,
) -> Result<(Paged, bool), Failure> {
    let (mut seen, mut cursor, mut indexed) = (Vec::new(), None, true);
    for _ in 0..pages {
        let asked = Search {
            ty: TypeName::from(TY),
            k,
            cursor,
            filter: filter.map(Into::into),
            ..doc_search(SearchMode::vector("v", q.to_vec()))
        };
        let page = l.cat.search(who, asked).await;
        let page = page.must("a page under the index")?.data;
        indexed &= page.ranked_by == "index";
        seen.extend(bits(&page));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok((seen, indexed))
}

/// The keys of hits.
fn keys(hits: &Paged) -> Vec<String> {
    hits.iter().map(|(k, _)| k.clone()).collect()
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
        text: None,
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
    let (seen, indexed) = paged(&l, &l.own, (&[1.0, 0.0], None), 10, 30).await?;
    let seen = keys(&seen);
    ensure!(
        indexed && seen.iter().cloned().eq(want(200)),
        "pages of ten over two hundred vectors alike and a hundred apart say they ranked by \
         the index ({indexed}) and answer every record once, the two hundred first and in key \
         order; they answered {seen:?}"
    );
    // Seven hundred alike, of which PostgreSQL's graph, built over them,
    // may hand out a part: the rest follows the stream, so the order
    // starts again once at most.
    deliver(store.as_ref(), &l.own, records(alike(200..700).collect())).await?;
    l.set("vector_index.point", json!([])).await?;
    l.set("vector_index.point", json!(["v"])).await?;
    let q: &[f32] = &[1.0, 0.0];
    let (seen, indexed) = paged(&l, &l.own, (q, None), 100, 8).await?;
    let (whole, _) = paged(&l, &l.own, (q, None), 800, 1).await?;
    let restarts = (seen.windows(2))
        .filter(|w| {
            let (a, b) = (f64::from_bits(w[0].1), f64::from_bits(w[1].1));
            b > a || (b == a && w[1].0 < w[0].0)
        })
        .count();
    let once: BTreeSet<String> = keys(&seen).into_iter().collect();
    ensure!(
        indexed
            && seen == whole
            && once.len() == seen.len()
            && once.into_iter().eq(want(700).collect::<BTreeSet<_>>())
            && restarts <= 1,
        "pages of a hundred over seven hundred vectors alike and a hundred apart say they \
         ranked by the index ({indexed}), answer one page of 800, every record once, best \
         first with equal scores by key but at most once ({restarts}); they answered {} hits: \
         {:?}, the page {:?}",
        seen.len(),
        keys(&seen),
        keys(&whole)
    );
    Ok(())
}

/// A point's `bucket`: one in forty.
fn bucket(i: u64) -> u64 {
    i % 40
}

/// Whether the `i`th point is held.
fn held(i: u64) -> bool {
    i % 40 == 7
}

/// `point` with its source fields, the setting asking for `v`'s index,
/// and `n` points of the corpus, `bucket` beside `group`, those [`held`]
/// held.
async fn lake(store: &dyn LakeStore, n: u64) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let source = |f: FieldDecl| FieldDecl {
        authority: Authority::Source,
        ..f
    };
    let fields = vec![
        source(vector("v", DIMENSIONS as u32)),
        source(field("group", StorageKind::Number)),
        source(field("bucket", StorageKind::Number)),
    ];
    l.declare(type_decl(TY, fields)).await?;
    let record = |i: u64| {
        let p = corpus_point(i);
        let v = p.vector.as_deref();
        fixture::source_record(
            TY,
            &p.key,
            json!({"v": v, "group": p.group, "bucket": bucket(i)}),
        )
    };
    deliver(store, &l.own, (0..n).map(record).collect()).await?;
    l.set("vector_index.point", json!(["v"])).await?;
    for i in (0..n).filter(|i| held(*i)) {
        fixture::restrict(store, TY, corpus_point(i).key, "held").await?;
    }
    Ok(l)
}

/// Twelve pages of ten and one page of 120 of the same search agree, say
/// `index`, hold no key twice and never rise; the pages' hits.
async fn one_sequence(
    l: &Lake,
    who: &Principal,
    (q, filter): (&[f32], Option<&str>),
    what: &str,
) -> Result<Paged, Failure> {
    let (pages, indexed) = paged(l, who, (q, filter), 10, 12).await?;
    let (whole, whole_indexed) = paged(l, who, (q, filter), 120, 1).await?;
    let once: BTreeSet<&String> = pages.iter().map(|(k, _)| k).collect();
    let falling = pages
        .windows(2)
        .all(|w| f64::from_bits(w[0].1) >= f64::from_bits(w[1].1));
    ensure!(
        indexed && whole_indexed && pages == whole && once.len() == pages.len() && falling,
        "{what}: twelve pages of ten say `index` ({indexed}, {whole_indexed}) and answer one \
         page of 120, each key once, the scores never rising; the pages answered {:?}, the \
         page {:?}",
        keys(&pages),
        keys(&whole)
    );
    Ok(pages)
}

const FILTERS: [Option<&str>; 3] = [None, Some("group != 0"), Some("bucket < 9")];

async fn under_an_index_a_search_pages_one_ranking_per_class(store: Store, given: Given) -> Held {
    let l = lake(store.as_ref(), POINTS).await?;
    let reader = fixture::reads_all(store.as_ref(), "reader");
    let held_keys: BTreeSet<String> = (0..POINTS)
        .filter(|i| held(*i))
        .map(|i| corpus_point(i).key)
        .collect();
    for qi in 0..10 {
        let q = query(qi);
        for filter in FILTERS {
            let what = format!("query {qi}, filter {filter:?}");
            let read =
                one_sequence(&l, &reader, (&q, filter), &format!("the reader, {what}")).await?;
            let owned =
                one_sequence(&l, &l.own, (&q, filter), &format!("the owner, {what}")).await?;
            let unheld: Paged = owned
                .iter()
                .filter(|(k, _)| !held_keys.contains(k))
                .cloned()
                .collect();
            ensure!(
                !read.iter().any(|(k, _)| held_keys.contains(k)) && read.starts_with(&unheld),
                "{what}: the reader's pages hold no held record, and the owner's are the \
                 reader's with held records among them; the reader's {:?}, the owner's {:?}",
                keys(&read),
                keys(&owned)
            );
        }
    }
    eprintln!("one ranking per class on `{}`: held", given.harness);
    Ok(())
}

/// Twelve hundred points: the corpus of the rows that write after it.
const SMALL: u64 = 1_200;

async fn a_write_the_reader_may_not_see_moves_none_of_its_pages(store: Store) -> Held {
    let l = lake(store.as_ref(), SMALL).await?;
    let q = query(0);
    let reader = fixture::reads_all(store.as_ref(), "reader");
    let filters = [Some("group != 0"), None];
    let mut before = Vec::new();
    for filter in filters {
        before.push(paged(&l, &reader, (&q, filter), 10, 12).await?.0);
    }
    let at = |i: u64| {
        let p = corpus_point(i);
        fixture::source_record(TY, &p.key, json!({"v": q, "group": p.group, "bucket": 0}))
    };
    let (kept, gone) = (7, 47);
    ensure!(held(kept) && held(gone), "the records written are held");
    deliver(store.as_ref(), &l.own, vec![at(kept), at(gone)]).await?;
    l.retire(TY, &corpus_point(gone).key).await?;
    for (filter, a) in filters.into_iter().zip(&before) {
        let b = paged(&l, &reader, (&q, filter), 10, 12).await?.0;
        ensure!(
            *a == b,
            "filter {filter:?}: writes the reader may not see move none of its pages; before \
             them it was answered {:?}, after {:?}",
            keys(a),
            keys(&b)
        );
    }
    Ok(())
}

/// Whether two sequences agree as far as both reach.
fn same(a: &Paged, b: &Paged) -> bool {
    let n = a.len().min(b.len());
    a[..n] == b[..n]
}

async fn a_hold_keeps_the_readers_sequence_across_its_swap(store: Store) -> Held {
    let l = lake(store.as_ref(), SMALL).await?;
    let reader = fixture::reads_all(store.as_ref(), "reader");
    let q = query(2);
    let before = paged(&l, &reader, (&q, None), 120, 1).await?.0;
    let owned = paged(&l, &l.own, (&q, None), 120, 1).await?.0;
    let Some((x, _)) = before.get(45).cloned() else {
        return Err(Failure::new("the reader's sequence holds 46 hits"));
    };
    let without: Paged = before.iter().filter(|(k, _)| *k != x).cloned().collect();
    // Three pages, the hold, and the cursor followed on.
    let mut cursor = None;
    let mut seen = Vec::new();
    for page in 0..12 {
        if page == 3 {
            fixture::restrict(store.as_ref(), TY, x.as_str(), "held").await?;
        }
        let asked = Search {
            ty: TypeName::from(TY),
            k: 10,
            cursor,
            ..doc_search(SearchMode::vector("v", q.clone()))
        };
        let got = l.cat.search(&reader, asked).await.must("a page")?.data;
        seen.extend(bits(&got));
        cursor = got.next_cursor;
    }
    ensure!(
        same(&seen, &without),
        "the pages followed across the hold are the sequence of before without `{x}`: they \
         answered {:?}",
        keys(&seen)
    );
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(1) {
        let read = paged(&l, &reader, (&q, None), 10, 12).await?.0;
        let own = paged(&l, &l.own, (&q, None), 120, 1).await?.0;
        ensure!(
            same(&read, &without) && own == owned,
            "read again after the hold, the reader's sequence is that of before without `{x}`, \
             the owner's as it was: the reader's {:?}, the owner's {:?}",
            keys(&read),
            keys(&own)
        );
    }
    Ok(())
}

async fn a_reader_holding_only_is_ranked_exactly_over_its_pairs(store: Store) -> Held {
    use lakelands_api::principal::TypeGrants;
    const SIDED: &str = "sided";
    let l = Lake::of(store.as_ref());
    let source = |f: FieldDecl| FieldDecl {
        authority: Authority::Source,
        ..f
    };
    let fields = vec![
        source(vector("v", DIMENSIONS as u32)),
        source(field("side", StorageKind::Text)),
    ];
    l.declare(type_decl(SIDED, fields)).await?;
    let side = |i: u64| ["a", "b", "c"][(i % 3) as usize];
    let records = (0..SMALL)
        .map(|i| {
            let v = corpus_vector(i);
            fixture::source_record(SIDED, &format!("s{i:05}"), json!({"v": v, "side": side(i)}))
        })
        .collect();
    crate::conformance::fixture::ingest(doors(store.as_ref()).as_ref(), &l.own, SOURCE, records)
        .await
        .must("ingesting sided points")?;
    l.set("lookup_index.sided", json!(["side"])).await?;
    l.set("vector_index.sided", json!(["v"])).await?;
    let grants = Grants {
        default: Access::Read,
        types: [(
            TypeName::from(SIDED),
            TypeGrants {
                only: [("side".into(), json!("a"))].into(),
                ..Default::default()
            },
        )]
        .into(),
        ..Grants::default()
    };
    let reader = fixture::principal(store.as_ref(), PrincipalKind::Agent, "a-side", grants);
    for qi in 0..4 {
        let q = query(qi);
        let mut all: Vec<(lakelands_api::RecordKey, f64)> = (0..SMALL)
            .filter(|i| side(*i) == "a")
            .map(|i| {
                let key = format!("s{i:05}");
                (
                    key.as_str().into(),
                    lakelands_api::query::cosine(&q, &corpus_vector(i)),
                )
            })
            .collect();
        crate::engines::rank(&mut all, 120);
        let want: Paged = all.into_iter().map(|(k, s)| (k.0, s.to_bits())).collect();
        let (mut got, mut cursor) = (Vec::new(), None);
        for _ in 0..12 {
            let asked = Search {
                ty: TypeName::from(SIDED),
                k: 10,
                cursor,
                ..doc_search(SearchMode::vector("v", q.clone()))
            };
            let page = l.cat.search(&reader, asked).await.must("a page")?.data;
            got.extend(bits(&page));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        ensure!(
            got == want,
            "query {qi}: the reader holding `only` is answered the exact ranking of its pairs; \
             it was answered {:?}, the exact ranking is {:?}",
            keys(&got),
            keys(&want)
        );
    }
    Ok(())
}

async fn an_upgrade_rebuilds_the_graphs_beside_with_answers_unchanged(
    store: Store,
    given: Given,
) -> Held {
    let l = lake(store.as_ref(), SMALL).await?;
    let read = |l: Lake, store: Store| async move {
        let reader = fixture::reads_all(store.as_ref(), "reader");
        let mut all = Vec::new();
        for filter in [None, Some("group != 0")] {
            for who in [&l.own, &reader] {
                let (hits, indexed) = paged(&l, who, (&query(4), filter), 10, 12).await?;
                ensure!(indexed, "every page says `ranked_by: index`");
                all.push(hits);
            }
        }
        Ok::<_, Failure>(all)
    };
    let before = read(l, store.clone()).await?;
    if let Some(location) = given.location.as_ref() {
        (given.lower)(location);
    }
    let store = (given.reopen)(store, Seams::default()).await;
    let store = store.map_err(|e| Failure::new(format!("the upgrade's open: {e}")))?;
    let after = read(Lake::of(store.as_ref()), store.clone()).await?;
    ensure!(
        before == after,
        "opened again after its projection was marked one layout back, the lake answers the \
         owner's and the reader's pages as before"
    );
    Ok(())
}

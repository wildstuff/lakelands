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

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_core::Stream;
use lakelands_api::ontology::{Authority, FieldDecl, FieldType, StorageKind};
use lakelands_api::query::{Search, SearchMode};
use lakelands_api::{LakeStore, Position, Principal, ReadAt, SourceId, SourceRecord, TypeName};
use serde_json::json;

use crate::conformance::fixture::{Must, declare, doors, field, owner, type_decl, upsert};
use crate::conformance::{Failure, Given, next};

check!(
    /// An indexed vector field answers its index's set, at the engine's recall floor.
    VECTOR_INDEX_RECALL = an_indexed_vector_field_answers_its_index_set_at_the_engines_recall_floor,
    "043, 025 and 029: a search over a vector field the `vector_index.<type>` setting names says `ranked_by: index`, ranks the index's candidates by the one cosine with ties by key, answers the same bytes when asked again, and finds on average at least the engine's floor of the brute-force reference's hits for k = 1, 10 and 100, with and without a filter, at the head and at an earlier position, after records were retired, vectors cleared and rewritten and records added; an engine without such an index answers the reference, a recall of 1",
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
/// `ef_search = max(k, 200)`) measured 0.98 at k = 1, 0.97 to 0.98 at
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
    SourceRecord {
        ty: TypeName::from(TY),
        key: key.into(),
        fields: crate::conformance::fixture::patch(json!({"v": vector, "group": group})),
        seen_at: None,
    }
}

/// A delivery of records already made.
struct Delivery(std::vec::IntoIter<SourceRecord>);

impl Stream for Delivery {
    type Item = SourceRecord;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
        Poll::Ready(self.0.next())
    }
}

async fn deliver(
    store: &dyn LakeStore,
    own: &Principal,
    records: Vec<SourceRecord>,
) -> Result<(), Failure> {
    let what = format!("ingesting {} points", records.len());
    let mut commits = doors(store)
        .ingest(
            own,
            &SourceId::from(SOURCE),
            Box::pin(Delivery(records.into_iter())),
        )
        .await
        .map_err(|e| Failure::new(format!("{what} was refused: {e}")))?;
    while let Some(w) = next(&mut commits).await {
        w.must(&what)?;
    }
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
        .map(|(p, v)| (p.key.as_str().into(), crate::engines::cosine(q, v)))
        .collect();
    crate::engines::rank(&mut all, k);
    all.into_iter().map(|(key, _)| key.0).collect()
}

async fn an_indexed_vector_field_answers_its_index_set_at_the_engines_recall_floor(
    store: Arc<dyn LakeStore>,
    given: Given,
) -> Result<(), Failure> {
    use lakelands_api::WriteOpts;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let source = |f: FieldDecl| FieldDecl {
        authority: Authority::Source,
        ..f
    };
    let v = source(FieldDecl {
        ty: FieldType {
            dimensions: Some(DIMENSIONS as u32),
            ..FieldType::kind(StorageKind::Vector)
        },
        ..field("v", StorageKind::Text)
    });
    let group = source(field("group", StorageKind::Number));
    declare(cat.as_ref(), &own, type_decl(TY, vec![v, group])).await?;
    let mut points: Vec<Point> = (0..POINTS)
        .map(|i| Point {
            key: format!("p{i:05}"),
            vector: Some(corpus_vector(i)),
            group: (i % 4) as u32,
            retired: false,
            then: None,
        })
        .collect();
    deliver(
        store.as_ref(),
        &own,
        points
            .iter()
            .map(|p| source_record(&p.key, p.vector.as_deref(), p.group))
            .collect(),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &own,
        lakelands_api::seam::SETTING_TYPE,
        "vector_index.point",
        json!({"key": "vector_index.point", "value": ["v"]}),
    )
    .await?;
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
                cat.retire(&own, &point, &p.key.as_str().into(), WriteOpts::default())
                    .await
                    .must(&format!("retiring `{}`", p.key))?;
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
        let p = Point {
            key: format!("p{i:05}"),
            vector: Some(corpus_vector(i)),
            group: (i % 4) as u32,
            retired: false,
            then: None,
        };
        changed.push(source_record(&p.key, p.vector.as_deref(), p.group));
        points.push(p);
    }
    deliver(store.as_ref(), &own, changed).await?;

    let search = |q: Vec<f32>, k: u32, group: Option<u32>, earlier: bool| {
        let cat = cat.clone();
        let own = own.clone();
        async move {
            cat.search(
                &own,
                Search {
                    ty: TypeName::from(TY),
                    mode: SearchMode::Vector {
                        field: "v".into(),
                        vector: q,
                    },
                    k,
                    cursor: None,
                    filter: group.map(|g| format!("group = {g}").as_str().into()),
                    with_meta: false,
                    at: ReadAt {
                        as_of: earlier.then_some(Position(then.0)),
                        valid_at: None,
                    },
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
                    let hits: Vec<(String, u64)> = got
                        .hits
                        .iter()
                        .map(|h| (h.record.key.0.clone(), h.score.to_bits()))
                        .collect();
                    for (key, bits) in &hits {
                        let head = by_key
                            .get(key.as_str())
                            .and_then(|p| p.vector.as_ref())
                            .map(|v| crate::engines::cosine(&q, v).to_bits());
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
                        let again: Vec<(String, u64)> = again
                            .hits
                            .iter()
                            .map(|h| (h.record.key.0.clone(), h.score.to_bits()))
                            .collect();
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
    ensure!(
        short.is_empty(),
        "an indexed field on `{}` must find on average at least its floor of the reference's \
         hits; it found {}",
        given.harness,
        short.join("; ")
    );
    Ok(())
}

//! Ranked search: one token pipeline, one order, and no probing
//! (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! `docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`,
//! `docs/decisions/029-every-answer-has-one-order.md`).

use lakelands_api::error::Case;
use lakelands_api::ontology::StorageKind;
use lakelands_api::principal::PrincipalKind;
use lakelands_api::query::{Hits, Search, SearchMode};
use lakelands_api::{Access, Error, LakeStore, ReadAt, Refusal, TypeName};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, doors, embedding, field, field_access, field_writer, get_at, invalid_like,
    no_such_field, owner, principal, read_at, refused_alike, refused_on, restrict, searchable,
    type_decl, vector,
};
use crate::conformance::{Failure, Held, Reopen, Store, backing};

check!(
    MATCH_SET = full_text_matches_words_whatever_their_case_inflection_or_accents,
    "025: the store's one token pipeline splits, lower-cases, stems and folds diacritics, so the same query matches the same records on every engine"
);

check!(
    SEARCH_ORDER = search_is_ordered_by_score_then_key,
    "029: ranked search answers best first, and equal scores in key order"
);

check!(
    SEARCH_PROBE_REFUSED = searching_a_type_whose_searchable_field_is_unreadable_is_refused,
    "024: a search that names a searchable field the principal may not read is refused, naming the field; one that names none searches only the fields it reads, so over a type whose every searchable field it sees masked it answers no hits"
);

async fn articles(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let mut ty = type_decl("article", vec![searchable("text")]);
    // Stemming follows the type's language (025); without one, words are
    // only split and lower-cased.
    ty.language = Some("en".into());
    l.declare(ty).await?;
    Ok(l)
}

fn lexical(text: &str) -> Search {
    let ty = TypeName::from("article");
    Search {
        ty,
        k: 20,
        ..doc_search(SearchMode::lexical(text))
    }
}

async fn hits(store: &dyn LakeStore, text: &str) -> Result<Hits, Failure> {
    Ok(doors(store)
        .search(&owner(store), lexical(text))
        .await
        .must(&format!("searching for {text:?}"))?
        .data)
}

pub(super) fn keys(h: &Hits) -> Vec<&str> {
    h.hits.iter().map(|h| h.record.key.as_str()).collect()
}

async fn full_text_matches_words_whatever_their_case_inflection_or_accents(store: Store) -> Held {
    let l = articles(store.as_ref()).await?;
    l.upsert(
        "article",
        "cafe",
        json!({"text": "The Café was Running late"}),
    )
    .await?;
    l.upsert("article", "other", json!({"text": "Nothing to see here"}))
        .await?;

    for query in ["cafe", "CAFÉ", "runs", "running"] {
        let got = hits(store.as_ref(), query).await?;
        ensure!(
            keys(&got) == ["cafe"],
            "searching {query:?} must match exactly `cafe` (\"The Café was Running \
             late\"), since words are lower-cased, stemmed and folded; it matched {:?}",
            keys(&got)
        );
    }
    let none = hits(store.as_ref(), "tea").await?;
    ensure!(
        none.hits.is_empty(),
        "searching \"tea\" must match nothing; it matched {:?}",
        keys(&none)
    );
    Ok(())
}

async fn search_is_ordered_by_score_then_key(store: Store) -> Held {
    let l = articles(store.as_ref()).await?;
    for (key, text) in [
        ("b", "lake"),
        ("a", "lake"),
        ("c", "lake lake lake shore"),
        ("d", "mountain"),
    ] {
        l.upsert("article", key, json!({ "text": text })).await?;
    }
    let got = hits(store.as_ref(), "lake").await?;
    let ranked: Vec<(f64, &str)> = got
        .hits
        .iter()
        .map(|h| (h.score, h.record.key.as_str()))
        .collect();
    let mut keys: Vec<&str> = ranked.iter().map(|(_, k)| *k).collect();
    keys.sort();
    ensure!(
        keys == ["a", "b", "c"],
        "searching \"lake\" must match `a`, `b` and `c`; it matched {ranked:?}"
    );
    ensure!(
        ranked
            .windows(2)
            .all(|w| w[0].0 > w[1].0 || (w[0].0 == w[1].0 && w[0].1 < w[1].1)),
        "hits must come by score, highest first, and equal scores in key order; they \
         came {ranked:?}"
    );
    let a = ranked.iter().find(|(_, k)| *k == "a").map(|(s, _)| *s);
    let b = ranked.iter().find(|(_, k)| *k == "b").map(|(s, _)| *s);
    ensure!(
        a == b,
        "`a` and `b` hold the same text and must score the same; they scored {a:?} and {b:?}"
    );
    Ok(())
}

async fn searching_a_type_whose_searchable_field_is_unreadable_is_refused(store: Store) -> Held {
    let l = articles(store.as_ref()).await?;
    let cat = &l.cat;
    l.upsert("article", "a", json!({"text": "secret plan"}))
        .await?;
    let masked = [("text", Access::Masked)];
    let reader = field_access(store.as_ref(), "reader", "article", &masked);
    let none = cat.search(&reader, lexical("secret")).await;
    let none = none.must("searching `article`, whose `text` the reader sees only masked")?;
    ensure!(
        none.data.hits.is_empty(),
        "a search naming no field, over the masked `text` alone, must answer no hits; it \
         found {:?}",
        keys(&none.data)
    );
    let mut named = lexical("secret");
    if let SearchMode::Lexical { fields, .. } = &mut named.mode {
        *fields = Some(vec!["text".into()]);
    }
    let err = (cat.search(&reader, named).await)
        .must_fail("searching `article` in `text`, which the reader sees only masked")?;
    ensure!(
        matches!(refused_on(&err), Some(("text", Refusal::NotGranted))),
        "a search over the masked `text` must be refused as not granted, naming `text`; \
         it failed with {err:?}"
    );
    Ok(())
}

check!(
    HYBRID_PROBE_REFUSED = a_hybrid_search_over_an_unreadable_searchable_field_is_refused_whole,
    "024: a hybrid search that names a searchable field the principal may not read is refused, naming the field, whether it brings a vector or not; it never answers from its vector half. One that names none leaves that field out of its text half, so the text it brings ranks nothing"
);

check!(
    VECTOR_PROBE_REFUSED = a_vector_search_over_an_unreadable_vector_field_is_refused,
    "024: a vector or hybrid search that names a vector field the principal may not read is refused, naming that field; a query vector of another length than the field declares is refused by the declaration, so a holder of `only` admitted none of the stored vectors is answered the same whole error as over the type while it held none, naming no record"
);

/// `reader` of `doc`, reading everything but `masked`, which it sees
/// only masked.
pub(super) fn masking(store: &dyn LakeStore, masked: &str) -> lakelands_api::Principal {
    field_access(store, "reader", "doc", &[(masked, Access::Masked)])
}

/// `search` by `who` must be refused as not granted, naming `field`.
pub(super) async fn refused(
    store: &dyn LakeStore,
    who: &lakelands_api::Principal,
    search: Search,
    field: &str,
    what: &str,
) -> Held {
    match doors(store).search(who, search).await {
        Err(e) if matches!(refused_on(&e), Some((f, Refusal::NotGranted)) if f == field) => Ok(()),
        Err(e) => Err(Failure::new(format!(
            "{what} must be refused as not granted, naming `{field}`; it failed with {e:?}"
        ))),
        Ok(r) => Err(Failure::new(format!(
            "{what} must be refused as not granted, naming `{field}`; it answered {:?}, \
             ranked by {:?}",
            keys(&r.data),
            r.data.ranked_by
        ))),
    }
}

async fn a_hybrid_search_over_an_unreadable_searchable_field_is_refused_whole(
    store: Store,
) -> Held {
    two_docs(store.as_ref()).await?;
    // The vector [0, 1] points along `a` alone: an answer from the vector
    // half would find `a` whatever the text says.
    let own = doors(store.as_ref())
        .search(
            &owner(store.as_ref()),
            doc_search(hybrid("quayside", Some(vec![0.0, 1.0]))),
        )
        .await
        .must("the owner's hybrid search for \"quayside\" along `a`")?
        .data;
    ensure!(
        keys(&own).first() == Some(&"a"),
        "the owner's hybrid search for \"quayside\" along [0, 1] must rank `a` first; \
         it found {:?}",
        keys(&own)
    );
    let reader = masking(store.as_ref(), "body");
    let in_body = |vector| {
        let mut mode = hybrid("quayside", vector);
        if let SearchMode::Hybrid { fields, .. } = &mut mode {
            *fields = Some(vec!["body".into()]);
        }
        doc_search(mode)
    };
    for (vector, what) in [
        (Some(vec![0.0, 1.0]), "bringing a vector"),
        (None, "without a vector"),
    ] {
        let what =
            format!("a hybrid search in `body` {what}, by a reader who sees it only masked,");
        refused(store.as_ref(), &reader, in_body(vector), "body", &what).await?;
    }
    // Naming none, the text it brings ranks nothing: the vector half alone.
    let mut found = Vec::new();
    for text in ["quayside", "harbour"] {
        let along_b = doc_search(hybrid(text, Some(vec![1.0, 0.0])));
        let hits = doors(store.as_ref()).search(&reader, along_b).await;
        let hits = hits
            .must("the reader's hybrid search naming no field")?
            .data;
        found.push(
            keys(&hits)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        );
    }
    ensure!(
        found[0] == found[1] && found[0].first().map(String::as_str) == Some("b"),
        "a hybrid search naming no field, by a reader who sees `body` only masked, ranks by \
         its vector alone, whatever its text; along [1, 0] it found {found:?}"
    );
    Ok(())
}

async fn a_vector_search_over_an_unreadable_vector_field_is_refused(store: Store) -> Held {
    let only = json!({"default": "read", "types": {"doc": {"only": {"body": "none"}}}});
    let only = serde_json::from_value(only).expect("grants read");
    let holder = principal(store.as_ref(), PrincipalKind::Agent, "holder", only);
    let long = || doc_search(SearchMode::vector("v", vec![1.0, 0.0, 0.0]));
    let wrong_length = || async { doors(store.as_ref()).search(&holder, long()).await.err() };
    doc_type(store.as_ref()).await?;
    let empty = wrong_length().await;
    two_docs(store.as_ref()).await?;
    let held = wrong_length().await;
    let json = |e: &Option<Error>| serde_json::to_value(e).ok();
    let case = held.as_ref().and_then(|e| Some(e.case()?.case.clone()));
    let declared = Case::QueryDimensions { query: 3, held: 2 };
    ensure!(
        empty.is_some() && held == empty && json(&held) == json(&empty) && case == Some(declared),
        "a vector of 3 dimensions searched over `v`, of 2, by a holder of `only` admitted no \
         record, is refused by the declaration alike over the type empty and holding vectors; \
         it answered {empty:?} and {held:?}"
    );
    let along_a = || SearchMode::vector("v", vec![0.0, 1.0]);
    let own = doors(store.as_ref())
        .search(&owner(store.as_ref()), doc_search(along_a()))
        .await
        .must("the owner's vector search along `a`")?
        .data;
    ensure!(
        keys(&own) == ["a", "b"],
        "the owner's vector search along [0, 1] must find `a`, then `b`; it found {:?}",
        keys(&own)
    );
    let reader = masking(store.as_ref(), "v");
    refused(
        store.as_ref(),
        &reader,
        doc_search(along_a()),
        "v",
        "a vector search over `v`, by a reader who sees `v` only masked,",
    )
    .await?;
    refused(
        store.as_ref(),
        &reader,
        doc_search(hybrid("quayside", Some(vec![0.0, 1.0]))),
        "v",
        "a hybrid search over `v`, by a reader who sees `v` only masked,",
    )
    .await
}

check!(
    EMBED_PROBE_REFUSED = a_search_over_a_vector_embedded_from_an_unreadable_field_is_refused,
    "024: a vector embedded from a field encodes that field, so a vector or hybrid search over it is refused, naming the field, when the principal may not read one of the fields it is embedded from, even though it may read the vector; a vector field named in `also` is probed like the first, and one the principal may not know of is answered as undeclared",
    embedding
);

async fn a_search_over_a_vector_embedded_from_an_unreadable_field_is_refused(store: Store) -> Held {
    use crate::conformance::backing::EMBED_MODEL;

    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    // Neither text is searchable: only the embedding reads `body`.
    let vec = embedding("vec", 2, &["title", "body"], EMBED_MODEL);
    l.declare(type_decl(
        "doc",
        vec![
            field("title", StorageKind::Text),
            field("body", StorageKind::Text),
            vec,
            vector("v", 2),
        ],
    ))
    .await?;
    l.upsert(
        "doc",
        "a",
        json!({"title": "Minutes", "body": "the salary of the harbour master"}),
    )
    .await?;
    let along = || SearchMode::vector("vec", vec![1.0, 0.0]);
    let mine = cat
        .search(own, doc_search(along()))
        .await
        .must("the owner's vector search over `vec`")?
        .data;
    ensure!(
        keys(&mine) == ["a"],
        "the owner's vector search over the embedded `vec` must find `a`; it found {:?}",
        keys(&mine)
    );
    // The reader may read `title` and `vec`, but sees `body` only masked.
    let reader = masking(store.as_ref(), "body");
    refused(
        store.as_ref(),
        &reader,
        doc_search(along()),
        "body",
        "a vector search over `vec`, embedded from `title` and `body`, by a reader who \
         sees `body` only masked,",
    )
    .await?;
    refused(
        store.as_ref(),
        &reader,
        doc_search(SearchMode::Hybrid {
            text: "harbour".into(),
            field: "vec".into(),
            vector: Some(vec![1.0, 0.0]),
            fields: None,
            weight: None,
            text_weight: None,
        }),
        "body",
        "a hybrid search over `vec`, embedded from `title` and `body`, by a reader who \
         sees `body` only masked,",
    )
    .await?;
    // Named in `also`, `vec` is probed as the first field is.
    let also = || doc_search(super::fusion::along(None, "vec", None));
    let what = "a vector search over `v` and, in `also`, `vec`, by a reader who sees `body` \
                only masked,";
    refused(store.as_ref(), &reader, also(), "body", what).await?;
    let blind = field_access(store.as_ref(), "blind", "doc", &[("vec", Access::None)]);
    let answer = cat.search(&blind, also()).await;
    ensure!(
        no_such_field(&answer, "vec"),
        "a vector search naming `vec` in `also`, by a principal who may not know of `vec`, \
         must be answered as for a field `doc` does not declare; it answered {answer:?}"
    );
    let nosuch = doc_search(super::fusion::along(None, "nosuch", None));
    let nosuch = cat.search(&blind, nosuch).await;
    refused_alike("a search naming `vec`", &answer, nosuch, ("nosuch", "vec"))?;
    Ok(())
}

check!(
    SEARCH_FOLLOWS_TEXT = a_search_finds_a_record_by_its_current_text_and_nothing_by_none,
    "025: the index follows every write, so a record is found by its new words and no longer by its old ones, and a query of no words matches nothing"
);

check!(
    WITHHELD_NEVER_HITS = a_retired_or_restricted_record_is_never_a_hit,
    "the log format §2.3 and 006: a retired record is invisible, and a restricted one invisible to a principal without the `restricted` grant; neither is ever a search hit"
);

check!(
    HYBRID_WITHOUT_BACKEND =
        a_hybrid_search_without_a_vector_or_a_backend_ranks_lexically_and_says_so,
    "the embedding seam (docs/seams.md): with no embedding backend bound, a hybrid search that brings no vector over a field a model fills ranks by its text alone, and its answer and its one axis say it ranked lexically",
    reopens
);

check!(
    HYBRID_THROUGH_BACKEND =
        a_bound_backend_embeds_a_hybrid_search_without_a_vector_and_nothing_else,
    "the embedding seam (docs/seams.md): a hybrid search without a vector ranks through the bound backend and says `hybrid`; a vector search or a hybrid one bringing its vector never calls it; a backend that fails, whatever error it answers, or that answers a vector of no direction, makes the search unavailable, never lexical or invalid",
    embedding
);

check!(
    VECTOR_LEAVES_THE_NEIGHBOURS =
        a_retired_record_or_a_cleared_vector_is_no_longer_a_nearest_neighbour,
    "the engine seam's vector index: it follows the projection, so a retired record or a cleared vector field is no longer among the nearest"
);

check!(
    VECTOR_TOP_K = a_vector_search_answers_the_brute_force_ranking_to_the_bit,
    "025 and 029, the engine seam's vector index: for k = 1, 10 and 100, with and without a filter, at the head and at an earlier position, after records were retired, vectors cleared and rewritten and records added, a vector search answers the keys, order and scores of the brute-force reference: the one cosine over every vector at the head, best first, ties by key, kept where the read sees the record"
);

async fn a_search_finds_a_record_by_its_current_text_and_nothing_by_none(store: Store) -> Held {
    let l = articles(store.as_ref()).await?;
    l.upsert("article", "a", json!({"text": "harbour crane"}))
        .await?;
    l.upsert("article", "a", json!({"text": "mountain goat"}))
        .await?;
    let old = hits(store.as_ref(), "harbour").await?;
    let new = hits(store.as_ref(), "goat").await?;
    let none = hits(store.as_ref(), "").await?;
    ensure!(
        keys(&old).is_empty() && keys(&new) == ["a"] && none.hits.is_empty(),
        "after `a` was rewritten from \"harbour crane\" to \"mountain goat\", \
         \"harbour\" must find nothing ({:?}), \"goat\" must find `a` ({:?}), and an \
         empty query nothing ({:?})",
        keys(&old),
        keys(&new),
        keys(&none)
    );
    Ok(())
}

async fn a_retired_or_restricted_record_is_never_a_hit(store: Store) -> Held {
    let l = articles(store.as_ref()).await?;
    let Lake { cat, .. } = &l;
    for k in ["live", "gone", "held"] {
        l.upsert("article", k, json!({"text": "lake report"}))
            .await?;
    }
    let article = TypeName::from("article");
    l.retire(article.as_str(), "gone").await?;
    restrict(store.as_ref(), article.as_str(), "held", "legal hold").await?;
    let reader = field_writer(store.as_ref(), "reader", "article", &[]);
    let seen = cat
        .search(&reader, lexical("lake"))
        .await
        .must("searching as a reader without the restricted grant")?
        .data;
    ensure!(
        keys(&seen) == ["live"],
        "a reader searching \"lake\" must find only `live`: `gone` is retired and \
         `held` restricted from it; it found {:?}",
        keys(&seen)
    );
    let owner_sees = hits(store.as_ref(), "lake").await?;
    let mut both = keys(&owner_sees);
    both.sort();
    ensure!(
        both == ["held", "live"],
        "the owner, holding the restricted grant, must find `held` and `live` but \
         never the retired `gone`; it found {both:?}"
    );
    Ok(())
}

async fn a_retired_record_or_a_cleared_vector_is_no_longer_a_nearest_neighbour(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let embedding = vector("embedding", 2);
    l.declare(type_decl("doc", vec![embedding])).await?;
    for (k, v) in [
        ("east", [1.0, 0.0]),
        ("north", [0.0, 1.0]),
        ("near", [0.9, 0.1]),
    ] {
        l.upsert("doc", k, json!({ "embedding": v })).await?;
    }
    let nearest = |store: Store| async move {
        let s = doc_search(SearchMode::vector("embedding", vec![1.0, 0.0]));
        doors(store.as_ref())
            .search(&owner(store.as_ref()), s)
            .await
            .must("searching for the nearest to (1, 0)")
            .map(|r| {
                r.data
                    .hits
                    .into_iter()
                    .map(|h| h.record.key.0)
                    .collect::<Vec<_>>()
            })
    };
    let first = nearest(store.clone()).await?;
    ensure!(
        first.first().map(String::as_str) == Some("east") && first.contains(&"near".to_owned()),
        "nearest to (1, 0) must start with `east` and include `near`; it answered {first:?}"
    );
    let doc = TypeName::from("doc");
    l.retire(doc.as_str(), "east").await?;
    l.upsert("doc", "near", json!({"embedding": null})).await?;
    let after = nearest(store.clone()).await?;
    ensure!(
        !after.contains(&"east".to_owned()) && !after.contains(&"near".to_owned()),
        "after `east` was retired and `near`'s vector cleared, neither may be a \
         neighbour; the nearest were {after:?}"
    );
    Ok(())
}

/// The query of [`a_vector_search_answers_the_brute_force_ranking_to_the_bit`],
/// and the first of the directions its tied records point in.
const QUERY: [f32; 3] = [3.0, -1.0, 2.0];
const DIRECTIONS: [[f32; 3]; 5] = [
    QUERY,
    [1.0, 1.0, 1.0],
    [-2.0, 0.0, 1.0],
    [0.0, 0.0, 1.0],
    [-3.0, 1.0, -2.0],
];
/// How many `point` records the corpus starts with, and how many it gains
/// after the earlier position.
const POINTS: usize = 240;
const ADDED: usize = 20;

/// The `i`th `point`, pointing along `vector`, in group `i % 4`.
fn corpus_point(i: usize, vector: [f32; 3]) -> Point {
    Point {
        key: format!("p{i:03}"),
        vector: Some(vector),
        group: (i % 4) as u32,
        retired: false,
        then: None,
    }
}

/// One `point` of the corpus as the check wrote it.
#[derive(Clone)]
struct Point {
    key: String,
    /// Its vector and group at the head; `None` once cleared.
    vector: Option<[f32; 3]>,
    group: u32,
    retired: bool,
    /// Its group at the earlier position, when it existed then.
    then: Option<u32>,
}

/// The `i`th vector of the corpus. Every third record points in one of
/// [`DIRECTIONS`], scaled by a power of two, which leaves its cosine
/// unchanged to the bit, so each direction is a group of equal scores;
/// the rest hold small whole numbers, and `p007` is all zeros. Every
/// component is exact in `f32`.
fn corpus_vector(i: usize) -> [f32; 3] {
    if i == 7 {
        return [0.0; 3];
    }
    if i.is_multiple_of(3) {
        let d = DIRECTIONS[(i / 3) % DIRECTIONS.len()];
        let m = [1.0, 2.0, 4.0][(i / 15) % 3];
        return d.map(|x| x * m);
    }
    let mut z = (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    [0, 1, 2].map(|_| {
        z = (z ^ (z >> 29)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        ((z >> 40) % 13) as f32 - 6.0
    })
}

/// What a vector search must answer, by the reference: every vector the
/// index holds at the head, scored by the one cosine, ranked by
/// [`crate::engines::rank`] over the records the read sees (live at the
/// head with a vector there, existing at `then` when the read is earlier,
/// in `group` when it filters), at most `k`.
fn reference(points: &[Point], earlier: bool, group: Option<u32>, k: u32) -> Vec<(String, f64)> {
    let mut all: Vec<(lakelands_api::RecordKey, f64)> = points
        .iter()
        .filter(|p| !p.retired)
        .filter_map(|p| Some((p, p.vector?)))
        .filter(|(p, _)| {
            let seen = if earlier { p.then } else { Some(p.group) };
            seen.is_some_and(|g| group.is_none_or(|want| g == want))
        })
        .map(|(p, v)| {
            (
                p.key.as_str().into(),
                lakelands_api::query::cosine(&QUERY, &v),
            )
        })
        .collect();
    crate::engines::rank(&mut all, k);
    all.into_iter().map(|(key, score)| (key.0, score)).collect()
}

async fn a_vector_search_answers_the_brute_force_ranking_to_the_bit(store: Store) -> Held {
    use lakelands_api::Position;

    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let v = vector("v", 3);
    let group = field("group", StorageKind::Number);
    l.declare(type_decl("point", vec![v, group])).await?;
    let mut points: Vec<Point> = (0..POINTS)
        .map(|i| corpus_point(i, corpus_vector(i)))
        .collect();
    for p in &points {
        l.upsert("point", &p.key, json!({"v": p.vector, "group": p.group}))
            .await?;
    }
    let then = store.head().await.must("reading the head")?;
    for p in &mut points {
        p.then = Some(p.group);
    }
    // After the earlier position: vectors rewritten into the top group and
    // groups moved, vectors cleared, records retired, records added.
    let point = TypeName::from("point");
    for (i, p) in points.iter_mut().enumerate() {
        if i % 17 == 8 {
            p.vector = Some(QUERY.map(|x| x * 8.0));
            p.group = (p.group + 1) % 4;
            l.upsert("point", &p.key, json!({"v": p.vector, "group": p.group}))
                .await?;
        }
        if i % 13 == 6 {
            p.vector = None;
            l.upsert("point", &p.key, json!({"v": null})).await?;
        }
        if i % 11 == 5 {
            p.retired = true;
            l.retire(point.as_str(), p.key.as_str()).await?;
        }
    }
    for i in POINTS..POINTS + ADDED {
        let p = corpus_point(i, if i % 2 == 0 { QUERY } else { corpus_vector(i) });
        l.upsert("point", &p.key, json!({"v": p.vector, "group": p.group}))
            .await?;
        points.push(p);
    }
    // The fixture must put equal scores across a cut, or the check would
    // not test ties.
    let whole = reference(&points, false, None, u32::MAX);
    ensure!(
        whole.len() > 100 && whole[9].1 == whole[10].1 && whole[0].1 == whole[1].1,
        "the corpus must hold over 100 neighbours with equal scores across the cut at k = 1 \
         and k = 10; its reference ranks {:?}",
        &whole[..whole.len().min(12)]
    );
    for earlier in [false, true] {
        for group in [None, Some(1)] {
            for k in [1, 10, 100] {
                let s = Search {
                    ty: point.clone(),
                    k,
                    filter: group.map(|g| format!("group = {g}").as_str().into()),
                    at: read_at(earlier.then_some(Position(then.0))),
                    ..doc_search(SearchMode::vector("v", QUERY.to_vec()))
                };
                let what = format!(
                    "a vector search for {QUERY:?} with k = {k}{}{}",
                    group.map_or(String::new(), |g| format!(", where group = {g}")),
                    if earlier {
                        format!(", as of {}", then.0)
                    } else {
                        String::new()
                    }
                );
                let got: Vec<(String, u64)> = cat
                    .search(own, s)
                    .await
                    .must(&what)?
                    .data
                    .hits
                    .into_iter()
                    .map(|h| (h.record.key.0, h.score.to_bits()))
                    .collect();
                let want: Vec<(String, u64)> = reference(&points, earlier, group, k)
                    .into_iter()
                    .map(|(key, score)| (key, score.to_bits()))
                    .collect();
                ensure!(
                    got == want,
                    "{what} must answer the brute-force reference, keys, order and the scores' \
                     bits; it answered {got:?}, the reference {want:?}"
                );
            }
        }
    }
    Ok(())
}

/// Two documents, each with a text and a vector: `a` matches the text
/// `quayside` and points north, `b` matches nothing and points east.
/// `doc`: a searchable `body` and `v`, a vector of 2 dimensions.
async fn doc_type(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let v = vector("v", 2);
    l.declare(type_decl("doc", vec![searchable("body"), v]))
        .await?;
    Ok(l)
}

async fn two_docs(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = doc_type(store).await?;
    l.upsert("doc", "a", json!({"body": "quayside", "v": [0.0, 1.0]}))
        .await?;
    l.upsert("doc", "b", json!({"body": "harbour", "v": [1.0, 0.0]}))
        .await?;
    Ok(l)
}

/// [`two_docs`], with `e`, the embedding of `body` by the suite's model: a
/// hybrid search without a vector reads a field a model fills
/// (lakelands#743). It is declared, and its records written, where the
/// suite's backend is bound; a check searching without one reopens the
/// lake after.
async fn two_embedded_docs(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let e = embedding("e", 2, &["body"], crate::conformance::backing::EMBED_MODEL);
    l.declare(type_decl(
        "doc",
        vec![searchable("body"), vector("v", 2), e],
    ))
    .await?;
    l.upsert("doc", "a", json!({"body": "quayside", "v": [0.0, 1.0]}))
        .await?;
    l.upsert("doc", "b", json!({"body": "harbour", "v": [1.0, 0.0]}))
        .await?;
    Ok(l)
}

/// A hybrid search for `text` over the embedding `e`, without a vector.
fn over_e(text: &str) -> SearchMode {
    SearchMode::Hybrid {
        text: text.into(),
        field: "e".into(),
        vector: None,
        fields: None,
        weight: None,
        text_weight: None,
    }
}

pub(super) fn doc_search(mode: SearchMode) -> Search {
    Search {
        ty: TypeName::from("doc"),
        mode,
        k: 10,
        cursor: None,
        filter: None,
        with_meta: false,
        at: ReadAt::default(),
    }
}

fn hybrid(text: &str, vector: Option<Vec<f32>>) -> SearchMode {
    SearchMode::Hybrid {
        text: text.into(),
        field: "v".into(),
        vector,
        fields: None,
        weight: None,
        text_weight: None,
    }
}

async fn a_hybrid_search_without_a_vector_or_a_backend_ranks_lexically_and_says_so(
    store: Store,
    reopen: Reopen,
) -> Held {
    use super::embeddings::{reopen_failed, with_embedder};
    // `e` is declared, and its records written, where the suite's backend
    // is bound; the search is made where none is.
    let store = reopen(store, with_embedder(Some(backing::embedder())))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend", e))?;
    two_embedded_docs(store.as_ref()).await?;
    let store = reopen(store, with_embedder(None))
        .await
        .map_err(|e| reopen_failed("with no embedding backend", e))?;
    let got = doors(store.as_ref())
        .search(&owner(store.as_ref()), doc_search(over_e("quayside")))
        .await
        .must("a hybrid search for \"quayside\" without a vector, and no backend bound")?
        .data;
    let lexical =
        got.axes.len() == 1 && got.axes[0].axis == "lexical" && got.axes[0].field.is_none();
    ensure!(
        keys(&got) == ["a"] && got.ranked_by.starts_with("lexical") && lexical,
        "with no embedding backend bound, a hybrid search for \"quayside\" without a \
         vector must rank by its text alone, finding only `a`, and say `lexical`; it \
         found {:?}, ranked by {:?}",
        keys(&got),
        got.ranked_by
    );
    Ok(())
}

async fn a_bound_backend_embeds_a_hybrid_search_without_a_vector_and_nothing_else(
    store: Store,
) -> Held {
    use crate::conformance::backing::{NOWHERE, UNREACHABLE, embed_calls};
    let l = two_embedded_docs(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    // The backend makes every text (1, 0), so both records' `e` point
    // along it: `b`, whose text does not match, is a hit only if the
    // search ranked over the backend's vector. The calls are counted from
    // after the records' own embedding.
    let before = embed_calls();
    let got = cat
        .search(own, doc_search(over_e("quayside")))
        .await
        .must("a hybrid search for \"quayside\" without a vector, a backend bound")?
        .data;
    let mut found = keys(&got);
    found.sort();
    ensure!(
        found == ["a", "b"] && got.ranked_by == "hybrid" && embed_calls() == before + 1,
        "with an embedding backend bound, a hybrid search without a vector must embed \
         its text in one call and rank over text and vector, finding `a` by its text and \
         `b` by its vector, and say `hybrid`; it found {found:?}, ranked by {:?}, with {} \
         calls to the backend",
        got.ranked_by,
        embed_calls() - before
    );
    let before = embed_calls();
    cat.search(own, doc_search(SearchMode::vector("v", vec![1.0, 0.0])))
        .await
        .must("a vector search")?;
    let brought = cat
        .search(own, doc_search(hybrid("quayside", Some(vec![0.0, 1.0]))))
        .await
        .must("a hybrid search bringing its vector")?
        .data;
    ensure!(
        embed_calls() == before && brought.ranked_by == "hybrid",
        "a vector search, and a hybrid search that brings its own vector, must never call \
         the embedding backend; they made {} calls",
        embed_calls() - before
    );
    // A vector of zeros is the backend's fault, not the caller's.
    for text in [
        format!("{UNREACHABLE} quayside"),
        format!("{NOWHERE} quayside"),
    ] {
        match cat.search(own, doc_search(over_e(&text))).await {
            Err(Error::Unavailable { .. }) => {}
            other => Err(Failure::new(format!(
                "a hybrid search for {text:?} whose bound embedding backend fails or \
                 answers a vector of no direction must be unavailable, never ranked \
                 lexically or refused as the caller's: the same query must not rank \
                 differently by the backend's weather; it answered {:?}",
                other.map(|r| (keys(&r.data).join(","), r.data.ranked_by))
            )))?,
        }
    }
    Ok(())
}

check!(
    SEARCH_READS_A_VECTOR_FIELD =
        a_vector_ranking_reads_a_vector_field_and_makes_no_call_it_cannot_use,
    "046, extended (lakelands#743): a vector search, an `also` term or a hybrid search over a field that holds no vectors is refused as invalid on that field, naming its kind; a hybrid search without a vector over a vector field no model fills is refused as invalid on that field, with a backend bound or not; neither asks the backend for a vector, and the caller's rights come first: a hidden field answers as an absent one, a masked one as not granted; a vector, or a hybrid search's own, over a vector field is answered as before",
    reopens
);

async fn a_vector_ranking_reads_a_vector_field_and_makes_no_call_it_cannot_use(
    store: Store,
    reopen: Reopen,
) -> Held {
    use super::embeddings::{reopen_failed, with_embedder};
    use crate::conformance::backing::embedded;
    use lakelands_api::query::VectorTerm;
    let store = reopen(store, with_embedder(Some(backing::embedder())))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend", e))?;
    let l = Lake::of(store.as_ref());
    let fields = vec![
        searchable("title"),
        searchable("secret"),
        field("n", StorageKind::Number),
        vector("plain", 2),
        vector("wide", 3),
    ];
    l.declare(type_decl("doc", fields)).await?;
    let a = json!({"title": "harbour", "n": 1, "plain": [1.0, 0.0], "wide": [1.0, 0.0, 0.0]});
    l.upsert("doc", "a", a).await?;
    let hybrid = |field: &str, text: &str, vector: Option<Vec<f32>>| SearchMode::Hybrid {
        text: text.into(),
        field: field.into(),
        vector,
        fields: None,
        weight: None,
        text_weight: None,
    };
    let along = |field: &str, also: &[&str]| SearchMode::Vector {
        field: field.into(),
        vector: vec![1.0, 0.0],
        weight: None,
        also: (also.iter())
            .map(|f| VectorTerm {
                field: (*f).into(),
                vector: vec![1.0, 0.0],
                weight: None,
            })
            .collect(),
    };
    // Each row: what, the search, its text if it has one, and the
    // refusal's case as said on the wire.
    let not_vector = |field: &str, kind: &str| json!({"code": "search_not_vector", "field": field, "kind": kind});
    let needs = |field: &str| json!({"code": "search_needs_vector", "field": field});
    let mut rows = vec![
        (
            "a vector search over `title`",
            along("title", &[]),
            None,
            not_vector("title", "text"),
        ),
        (
            "a vector search over `n`",
            along("n", &[]),
            None,
            not_vector("n", "number"),
        ),
        (
            "an `also` term over `title`",
            along("plain", &["title"]),
            None,
            not_vector("title", "text"),
        ),
        (
            "a hybrid search over `title` with a vector",
            hybrid("title", "vr-with", Some(vec![1.0, 0.0])),
            Some("vr-with"),
            not_vector("title", "text"),
        ),
        (
            "a hybrid search over `title` without one",
            hybrid("title", "vr-without", None),
            Some("vr-without"),
            not_vector("title", "text"),
        ),
        (
            "a hybrid search over `plain` without a vector, a backend bound",
            hybrid("plain", "vr-plain", None),
            Some("vr-plain"),
            needs("plain"),
        ),
        (
            "a hybrid search over `wide` without a vector, a backend bound",
            hybrid("wide", "vr-wide", None),
            Some("vr-wide"),
            needs("wide"),
        ),
    ];
    let mut wrong = Vec::new();
    let judge = |store: &Store,
                 rows: Vec<(
        &'static str,
        SearchMode,
        Option<&'static str>,
        serde_json::Value,
    )>| {
        let store = store.clone();
        async move {
            let mut wrong = Vec::new();
            for (what, mode, text, says) in rows {
                let answer = doors(store.as_ref())
                    .search(&owner(store.as_ref()), doc_search(mode))
                    .await;
                let on = says["field"].as_str().unwrap_or_default();
                let called = text.map_or(0, embedded);
                if refused_on(&answer).map(|(f, _)| f) != Some(on)
                    || !invalid_like(&answer, &says)
                    || called != 0
                {
                    let answer = answer.map(|r| keys(&r.data).join(","));
                    wrong.push(format!("{what}: {answer:?}, {called} calls"));
                }
            }
            wrong
        }
    };
    wrong.extend(judge(&store, std::mem::take(&mut rows)).await);
    // Where no backend is bound, the same.
    let store = reopen(store, with_embedder(None))
        .await
        .map_err(|e| reopen_failed("with no embedding backend", e))?;
    let unbound = vec![
        (
            "a hybrid search over `plain` without a vector, no backend",
            hybrid("plain", "vr-plain-none", None),
            None,
            needs("plain"),
        ),
        (
            "a hybrid search over `wide` without a vector, no backend",
            hybrid("wide", "vr-wide-none", None),
            None,
            needs("wide"),
        ),
    ];
    wrong.extend(judge(&store, unbound).await);
    ensure!(
        wrong.is_empty(),
        "a vector ranking reads a vector field: {wrong:?}"
    );
    // The caller's rights first: `secret` hidden answers as `nosuch`, which
    // the type does not have; `plain` masked is not granted.
    let blind = field_access(
        store.as_ref(),
        "blind",
        "doc",
        &[("secret", Access::None), ("plain", Access::Masked)],
    );
    let cat = doors(store.as_ref());
    let hidden = cat.search(&blind, doc_search(along("secret", &[]))).await;
    let absent = cat.search(&blind, doc_search(along("nosuch", &[]))).await;
    refused_alike(
        "a vector search over the hidden `secret`",
        &hidden,
        absent,
        ("nosuch", "secret"),
    )?;
    let masked = cat
        .search(&blind, doc_search(hybrid("plain", "vr-masked", None)))
        .await;
    ensure!(
        matches!(refused_on(&masked), Some((f, Refusal::NotGranted)) if f == "plain"),
        "a hybrid search over the masked `plain` must be refused as not granted first; it \
         answered {:?}",
        masked.map(|r| keys(&r.data).join(","))
    );
    // A caller's vector over a vector field is answered as before.
    for (what, mode) in [
        ("a vector search over `plain`", along("plain", &[])),
        (
            "a hybrid search over `plain` with a vector",
            hybrid("plain", "harbour", Some(vec![1.0, 0.0])),
        ),
    ] {
        let got = cat.search(&owner(store.as_ref()), doc_search(mode)).await;
        ensure!(
            got.as_ref().is_ok_and(|r| keys(&r.data) == ["a"]),
            "{what} must find `a`; it answered {:?}",
            got.map(|r| keys(&r.data).join(","))
        );
    }
    Ok(())
}

check!(
    HIT_IS_THE_RECORD = every_hit_is_the_record_get_answers_in_its_declared_shape,
    "README, Search: a search never returns a shape the ontology did not declare; every hit, lexical, vector or hybrid, is the record `get` answers the same principal, with only the type's declared fields and only those it may read"
);

async fn every_hit_is_the_record_get_answers_in_its_declared_shape(store: Store) -> Held {
    use lakelands_api::Get;
    let l = two_docs(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    // A field no search reads, written on one record only.
    let note = field("note", StorageKind::Text);
    cat.add_field(own, &TypeName::from("doc"), note)
        .await
        .must("adding `note` to `doc`")?;
    l.upsert("doc", "a", json!({"note": "berth 4"})).await?;
    // A reader of `body` and `v` only: `note` is not in its shape.
    let reader = field_access(store.as_ref(), "reader", "doc", &[("note", Access::None)]);

    for who in [own, &reader] {
        for (what, mode) in [
            ("lexical", SearchMode::lexical("quayside harbour")),
            ("vector", SearchMode::vector("v", vec![1.0, 1.0])),
            ("hybrid", hybrid("quayside harbour", Some(vec![1.0, 1.0]))),
        ] {
            let hits = cat
                .search(who, doc_search(mode))
                .await
                .must(&format!("a {what} search as `{}`", who.id))?
                .data;
            ensure!(
                !hits.hits.is_empty(),
                "a {what} search as `{}` over two docs answered no hit",
                who.id
            );
            for hit in &hits.hits {
                let got = cat
                    .get(
                        who,
                        &TypeName::from("doc"),
                        Get {
                            with_meta: false,
                            ..get_at(hit.record.key.as_str(), None)
                        },
                    )
                    .await
                    .must(&format!("getting `{}` as `{}`", hit.record.key, who.id))?
                    .data;
                ensure!(
                    hit.record == got,
                    "a {what} search as `{}` answered {:?} for `{}`, where `get` answers \
                     {got:?}: a hit is the record in the type's declared shape, as its \
                     reader may see it",
                    who.id,
                    hit.record,
                    hit.record.key
                );
            }
        }
    }
    Ok(())
}

//! A hybrid search fuses its lexical and its vector ranking by reciprocal
//! rank, each over the hits the read may see and to the lake's
//! `max_limit` of them, and a vector search over several fields fuses
//! theirs, each ranking weighed as the search asks
//! (`docs/decisions/046-a-hybrid-search-fuses-the-ranks-a-read-may-see.md`).

use lakelands_api::error::{Aimless, Case};
use lakelands_api::ontology::StorageKind;
use lakelands_api::query::{Hits, Search, SearchMode, VectorTerm};
use lakelands_api::{LakeStore, Principal};
use serde_json::json;

use super::search::doc_search;
use crate::conformance::fixture::{
    Lake, Must, field, field_writer, refused_like, refused_on, restrict, searchable, type_decl,
    vector,
};
use crate::conformance::{Failure, Held, Store};

check!(
    HIDDEN_MOVES_NO_SCORE = a_record_the_read_may_not_see_moves_no_hybrid_score,
    "046 and 024: a hybrid search ranks each list among the hits the read may see, so a restricted record, or one its `where` leaves out, ranked above a visible hit moves neither that hit's rank nor its score"
);

check!(
    PAGES_ONE_RANKING = every_page_of_a_hybrid_search_reads_one_ranking,
    "046 and 029: a hybrid search fuses each list to the lake's `max_limit` ranks whatever the page, so its pages through a cursor answer the one ranking a single page answers, keys and the scores' bits"
);

check!(
    SAME_LIST_UNREAD = a_search_that_reads_no_record_to_rank_answers_what_reading_them_answers,
    "046: at the head, a hybrid search by a principal who sees every live record, without `where` or `valid_at`, ranks without reading a record, and answers the keys and the scores' bits the same search answers under a `where` that holds for every record, with restricted and retired records in the lake"
);

check!(
    FUSED_BY_WEIGHT = a_search_fuses_its_rankings_each_by_its_weight,
    "046, extended, and 024: a vector search that names further fields in `also` ranks each field among the hits the read may see and fuses the rankings by weighted reciprocal rank, each adding `weight / (60 + rank)` in the order named, an absent weight being 1; it answers the keys and the scores' bits of that rule on every engine, a record without a vector in a field gains nothing from it, and a restricted record first in every ranking moves no score; a hybrid search weighs its text (`text_weight`) and its vector (`weight`) by the same rule, and weights of 1 answer what no weights answer"
);

check!(
    WEIGHTS_REFUSED = a_weight_that_cannot_weigh_or_a_field_named_twice_is_refused_naming_it,
    "046, extended: a weight of 0, below 0 or above 1 000 000, a `weight` on a vector search that names no other field, a vector field named twice, and a query vector in `vector`, in `also` or in a hybrid search whose components are all 0 or one is not a finite number once read as an `f32` (`(0, 0)`, `(1e-200, 0)`, `(1e300, 0)`) are each refused as invalid, naming the field or `text_weight`, before the principal's rights to the field are looked at"
);

/// A lake of `doc`s: each a key, a text, a unit vector that many degrees
/// from the query's `(1, 0)`, and a number `n`. A second vector field,
/// `w`, is left empty.
pub(super) async fn docs(
    store: &dyn LakeStore,
    rows: impl IntoIterator<Item = (String, &str, u32, u32)>,
) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let fields = vec![
        searchable("body"),
        vector("v", 2),
        vector("w", 2),
        field("n", StorageKind::Number),
    ];
    l.declare(type_decl("doc", fields)).await?;
    for (key, body, degrees, n) in rows {
        let a = f64::from(degrees).to_radians();
        let v = json!({"body": body, "v": [a.cos(), a.sin()], "n": n});
        l.upsert("doc", &key, v).await?;
    }
    Ok(l)
}

/// `who`'s hybrid search for "harbour" along `(1, 0)`.
async fn harbour(
    l: &Lake,
    who: &Principal,
    filter: Option<&str>,
    s: Search,
) -> Result<Hits, Failure> {
    let s = Search {
        mode: weighed(None, None),
        filter: filter.map(Into::into),
        ..s
    };
    Ok(l.cat
        .search(who, s)
        .await
        .must("a hybrid search for \"harbour\"")?
        .data)
}

/// A page of `k` from `cursor`.
fn page(k: u32, cursor: Option<lakelands_api::Cursor>) -> Search {
    Search {
        k,
        cursor,
        ..doc_search(SearchMode::lexical(""))
    }
}

/// The hits as keys and the bits of their scores.
fn scored(h: &Hits) -> Vec<(String, u64)> {
    (h.hits.iter())
        .map(|h| (h.record.key.0.clone(), h.score.to_bits()))
        .collect()
}

/// The fused score of a record at these ranks, one per list that ranks
/// it, summed as the store sums them: the lexical list first (046).
fn rrf(ranks: &[u32]) -> u64 {
    (ranks.iter())
        .fold(0.0f64, |s, r| s + 1.0 / (60.0 + f64::from(*r)))
        .to_bits()
}

async fn a_record_the_read_may_not_see_moves_no_hybrid_score(store: Store) -> Held {
    // `held` and `out` point along the query, `a` beside it: in the
    // vector list both rank above `a`. Only `held` and `a` hold the word.
    let rows = [
        ("held", "harbour", 0, 0),
        ("out", "quay", 0, 1),
        ("a", "harbour", 5, 0),
    ];
    let l = docs(
        store.as_ref(),
        rows.map(|(k, b, d, n)| (k.to_owned(), b, d, n)),
    )
    .await?;
    restrict(store.as_ref(), "doc", "held", "legal hold").await?;
    let reader = field_writer(store.as_ref(), "reader", "doc", &[]);
    // Under `where n = 0` the reader sees `a` alone, first in both lists;
    // without it, `out` too, first in the vector list only.
    for (filter, want) in [
        (Some("n = 0"), vec![("a", rrf(&[0, 0]))]),
        (None, vec![("a", rrf(&[0, 1])), ("out", rrf(&[0]))]),
    ] {
        let got = scored(&harbour(&l, &reader, filter, page(10, None)).await?);
        let want: Vec<(String, u64)> = want.into_iter().map(|(k, s)| (k.to_owned(), s)).collect();
        ensure!(
            got == want,
            "a reader without the restricted grant, searching {filter:?}: `held` (restricted) \
             and, under the `where`, `out` (n = 1) must move neither rank nor score; it \
             answered {got:?}, not {want:?} (scores as bits)"
        );
    }
    Ok(())
}

async fn every_page_of_a_hybrid_search_reads_one_ranking(store: Store) -> Held {
    // `r00` to `r79` point ever further from the query, a degree apart;
    // only `r70` holds the word, so it is the lexical list's one hit and
    // the vector list's 71st: it fuses to 1/60 + 1/130 and ranks first,
    // whichever page reads it.
    let rows = (0..80).map(|i| {
        (
            format!("r{i:02}"),
            if i == 70 { "harbour" } else { "quay" },
            i,
            0,
        )
    });
    let l = docs(store.as_ref(), rows).await?;
    let whole = scored(&harbour(&l, &l.own, None, page(100, None)).await?);
    let (mut paged, mut cursor) = (Vec::new(), None);
    for _ in 0..3 {
        let one = harbour(&l, &l.own, None, page(1, cursor)).await?;
        paged.extend(scored(&one));
        cursor = one.next_cursor;
    }
    let first = ("r70".to_owned(), rrf(&[0, 70]));
    ensure!(
        whole.len() == 80 && whole.first() == Some(&first) && paged[..] == whole[..3],
        "a hybrid search for \"harbour\" must rank `r70` first at 1/60 + 1/130, in one page \
         of 100 and in three pages of one alike; one page began {:?}, three pages answered \
         {paged:?}",
        &whole[..whole.len().min(3)]
    );
    Ok(())
}

async fn a_search_that_reads_no_record_to_rank_answers_what_reading_them_answers(
    store: Store,
) -> Held {
    // Twelve records five degrees apart, every other one holding the word;
    // two restricted, which the owner sees, and two retired, which no one does.
    let rows = (0..12).map(|i| {
        (
            format!("r{i:02}"),
            if i % 2 == 0 { "harbour" } else { "quay" },
            i * 5,
            i,
        )
    });
    let l = docs(store.as_ref(), rows).await?;
    for k in ["r03", "r06"] {
        restrict(store.as_ref(), "doc", k, "legal hold").await?;
    }
    for k in ["r04", "r09"] {
        l.retire("doc", k).await?;
    }
    // Without a `where` the owner reads no record to rank; with one that
    // holds for every record it reads each candidate.
    let unread = scored(&harbour(&l, &l.own, None, page(20, None)).await?);
    let read = scored(&harbour(&l, &l.own, Some("n >= 0"), page(20, None)).await?);
    let keys: Vec<&str> = unread.iter().map(|(k, _)| k.as_str()).collect();
    ensure!(
        unread == read && keys.len() == 10 && keys.contains(&"r06") && !keys.contains(&"r04"),
        "the owner's hybrid search must answer the ten live records, restricted ones \
         included, with the same keys and scores' bits whether or not a `where` that holds \
         for every record makes it read them; without it answered {unread:?}, with it {read:?}"
    );
    Ok(())
}

/// What weighted reciprocal rank answers for these rankings, each a
/// weight and its keys best first: every key's sum in the rankings'
/// order, best first, equal sums by key (046).
fn fused(lists: &[(f64, &[&str])]) -> Vec<(String, u64)> {
    let mut sums: Vec<(String, f64)> = Vec::new();
    for (weight, keys) in lists {
        for (rank, key) in keys.iter().enumerate() {
            let gain = weight / (60.0 + rank as f64);
            match sums.iter_mut().find(|(k, _)| k == key) {
                Some((_, sum)) => *sum += gain,
                None => sums.push(((*key).to_owned(), gain)),
            }
        }
    }
    sums.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    sums.into_iter().map(|(k, s)| (k, s.to_bits())).collect()
}

/// A vector search along `(1, 0)` over `v` and, in `also`, over `field`.
pub(super) fn along(weight: Option<f64>, field: &str, also: Option<f64>) -> SearchMode {
    let (field, vector) = (field.into(), vec![1.0, 0.0]);
    SearchMode::Vector {
        field: "v".into(),
        vector: vector.clone(),
        weight,
        also: vec![VectorTerm {
            field,
            vector,
            weight: also,
            model: None,
        }],
        model: None,
    }
}

/// A hybrid search for "harbour" along `(1, 0)` over `v`.
fn weighed(text_weight: Option<f64>, weight: Option<f64>) -> SearchMode {
    SearchMode::Hybrid {
        text: "harbour".into(),
        field: "v".into(),
        vector: Some(vec![1.0, 0.0]),
        fields: None,
        text_fields: None,
        weight,
        text_weight,
        model: None,
    }
}

async fn a_search_fuses_its_rankings_each_by_its_weight(store: Store) -> Held {
    // Along the query, `v` ranks held, a, b, c and `w` ranks held, b, a;
    // `c` holds no `w`, and it alone holds the word.
    let rows = [
        ("held", "quay", 0),
        ("a", "quay", 10),
        ("b", "quay", 20),
        ("c", "harbour", 30),
    ];
    let l = docs(
        store.as_ref(),
        rows.map(|(k, b, d)| (k.to_owned(), b, d, 0)),
    )
    .await?;
    for (key, degrees) in [("held", 0), ("a", 40), ("b", 5)] {
        let a = f64::from(degrees).to_radians();
        l.upsert("doc", key, json!({"w": [a.cos(), a.sin()]}))
            .await?;
    }
    restrict(store.as_ref(), "doc", "held", "legal hold").await?;
    let reader = field_writer(store.as_ref(), "reader", "doc", &[]);
    let (v, w, text): (&[&str], &[&str], &[&str]) = (&["a", "b", "c"], &["b", "a"], &["c"]);
    let one = |weight: Option<f64>| weight.unwrap_or(1.0);
    let fields = [(None, None), (Some(2.0), None), (Some(1.0), Some(3.0))].map(|(weight, also)| {
        let want = fused(&[(one(weight), v), (one(also), w)]);
        (along(weight, "w", also), "vector", want)
    });
    let halves = [
        (None, None),
        (Some(1.0), Some(1.0)),
        (Some(3.0), None),
        (None, Some(100.0)),
    ];
    let halves = halves.map(|(text_weight, weight)| {
        let want = fused(&[(one(text_weight), text), (one(weight), v)]);
        (weighed(text_weight, weight), "hybrid", want)
    });
    for (mode, by, want) in fields.into_iter().chain(halves) {
        let what = format!("{mode:?}");
        let got = l.cat.search(&reader, doc_search(mode));
        let got = got.await.must(&what)?.data;
        ensure!(
            scored(&got) == want && got.ranked_by == by,
            "a reader without the restricted grant, searching {what}, must be answered the \
             weighted reciprocal ranks of what it may see, {want:?} (scores as bits), ranked \
             by `{by}`; it answered {:?}, ranked by `{}`",
            scored(&got),
            got.ranked_by
        );
    }
    Ok(())
}

async fn a_weight_that_cannot_weigh_or_a_field_named_twice_is_refused_naming_it(
    store: Store,
) -> Held {
    let l = docs(store.as_ref(), [("a".to_owned(), "harbour", 0, 0)]).await?;
    // The reader sees `v` only masked: a search over it would be refused
    // as not granted, were its shape not judged first.
    let reader = super::search::masking(store.as_ref(), "v");
    let lone = SearchMode::Vector {
        field: "v".into(),
        vector: vec![1.0, 0.0],
        weight: Some(2.0),
        also: Vec::new(),
        model: None,
    };
    let range = |f: &str| json!({"code": "weight_range", "field": f});
    let (twice, text) = (
        json!({"code": "vector_field_twice"}),
        json!({"code": "text_weight_range"}),
    );
    for (mode, on, case) in [
        (lone, Some("v"), json!({"code": "weight_alone"})),
        (along(Some(0.0), "w", None), Some("v"), range("v")),
        (along(None, "w", Some(-1.0)), Some("w"), range("w")),
        (along(None, "w", Some(1_000_001.0)), Some("w"), range("w")),
        (along(None, "v", None), Some("v"), twice),
        (weighed(Some(0.0), None), None, text),
        (weighed(None, Some(-2.0)), Some("v"), range("v")),
    ] {
        let what = format!("{mode:?}, by a reader who sees `v` only masked,");
        let answer = l.cat.search(&reader, doc_search(mode)).await;
        let field = refused_on(&answer).map(|(f, _)| f);
        ensure!(
            on.is_none_or(|on| field == Some(on)),
            "{what} must be refused on {on:?}; it answered {answer:?}"
        );
        refused_like(&what, answer.map(|r| scored(&r.data)), &case)?;
    }
    // A query vector read from JSON as a caller's is: `1e-200` is 0 as an
    // `f32`, and `1e300` is infinite.
    for (written, why) in [
        (json!([0, 0]), Aimless::Zero),
        (json!([1e-200, 0]), Aimless::Zero),
        (json!([1e300, 0]), Aimless::NotFinite),
    ] {
        let aimless: Vec<f32> = serde_json::from_value(written.clone())
            .map_err(|e| Failure::new(format!("{written} does not read as `f32`s: {e}")))?;
        let (mut also, mut hybrid) = (along(None, "w", None), weighed(None, None));
        if let SearchMode::Vector { also: terms, .. } = &mut also {
            terms[0].vector = aimless.clone();
        }
        if let SearchMode::Hybrid { vector, .. } = &mut hybrid {
            *vector = Some(aimless.clone());
        }
        let lone = SearchMode::vector("v", aimless);
        for (mode, on) in [(lone, "v"), (also, "w"), (hybrid, "v")] {
            let what = format!("{mode:?}, its vector written {written},");
            let answer = l.cat.search(&reader, doc_search(mode)).await;
            refused_as!(
                &what,
                answer.map(|r| scored(&r.data)),
                Case::VectorAimless { field, why: w } if field.as_str() == on && *w == why
            )?;
        }
    }
    Ok(())
}

//! Ranked search: one token pipeline, one order, and no probing
//! (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! `docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`,
//! `docs/decisions/029-every-answer-has-one-order.md`).

use lakelands_api::key::link_key;
use lakelands_api::principal::PrincipalKind;
use lakelands_api::write::WriteOpts;

use crate::conformance::fixture::{doors, get_at, owner};
use crate::conformance::prelude::*;

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
            text_fields: None,
            weight: None,
            text_weight: None,
            model: None,
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
    "025: the index follows every write, so a record is found by its new words and no longer by its old ones, and a query of no words matches nothing; as of an earlier position the words are the head's and the hits the records there then: one written since is none, and one retired since is no candidate"
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
    "the engine seam's vector index: it follows the projection, so a retired record or a cleared vector field is no longer among the nearest; a field a migration renames is ranked under its new name, and one it removes by no name, though a field of that name is declared again, and a re-keyed record's vectors are ranked and counted under its new key alone (lakelands#768)"
);

check!(
    VECTOR_TOP_K = a_vector_search_answers_the_brute_force_ranking_to_the_bit,
    "025 and 029, the engine seam's vector index: for k = 1, 10 and 100, with and without a filter, at the head and at an earlier position, after records were retired, vectors cleared and rewritten and records added, a vector search answers the keys, order and scores of the brute-force reference: the one cosine over every vector at the head, best first, ties by key, kept where the read sees the record"
);

async fn a_search_finds_a_record_by_its_current_text_and_nothing_by_none(store: Store) -> Held {
    let l = articles(store.as_ref()).await?;
    l.upsert("article", "a", json!({"text": "harbour crane"}))
        .await?;
    let goat = json!({"text": "mountain goat"});
    l.upsert("article", "c", goat.clone()).await?;
    let then = store.head().await.must("reading the head")?;
    l.upsert("article", "a", goat.clone()).await?;
    l.upsert("article", "b", goat).await?;
    l.retire("article", "c").await?;
    let old = hits(store.as_ref(), "harbour").await?;
    let new = hits(store.as_ref(), "goat").await?;
    let none = hits(store.as_ref(), "").await?;
    ensure!(
        keys(&old).is_empty() && keys(&new) == ["a", "b"] && none.hits.is_empty(),
        "after `a` was rewritten from \"harbour crane\" to \"mountain goat\", \
         \"harbour\" must find nothing ({:?}), \"goat\" must find `a` and `b` ({:?}), and \
         an empty query nothing ({:?})",
        keys(&old),
        keys(&new),
        keys(&none)
    );
    // As of the position before: ranked by the words at the head, kept
    // where the record was there then.
    for (text, want) in [("goat", &["a"][..]), ("harbour", &[])] {
        let asked = Search {
            at: read_at(Some(then)),
            ..lexical(text)
        };
        let (cat, own) = (&l.cat, &l.own);
        let got = cat.search(own, asked).await;
        let got = got.must("searching as of the earlier position")?;
        ensure!(
            keys(&got.data) == want,
            "as of the position before `a` was rewritten, `b` written and `c` retired, {text:?} \
             found {:?}, not {want:?}: the words are the head's, `b` was not there then, and \
             `c`, retired since, is no candidate",
            keys(&got.data)
        );
    }
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
        near(store.as_ref(), s).await
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
    // A migration takes the index along (lakelands#768).
    let moved = |e: &str| type_decl("moved", vec![searchable("body"), vector(e, 2)]);
    l.declare(moved("e")).await?;
    l.upsert("moved", "a", json!({"body": "quay", "e": [1.0, 0.0]}))
        .await?;
    let along = |field: &str| Search {
        ty: TypeName::from("moved"),
        ..doc_search(SearchMode::vector(field, vec![1.0, 0.0]))
    };
    l.migrate_renaming(moved("f"), &[("e", "f")]).await?;
    let renamed = near(store.as_ref(), along("f")).await?;
    l.migrate(type_decl("moved", vec![searchable("body")]))
        .await?;
    l.declare(moved("f")).await?;
    let again = near(store.as_ref(), along("f")).await?;
    ensure!(
        renamed == ["a"] && again.is_empty(),
        "`a`'s vector in `e` must be ranked under `f` once a migration renamed `e` to \
         `f` ({renamed:?}), and by no name once one removed `f`, though `f` is declared \
         again ({again:?})"
    );
    // A re-key takes them to the record's new key, and leaves none at the
    // old one to be counted among the candidates.
    l.declare(keyed("code")).await?;
    let a = json!({"code": "a", "alt": "z", "body": "quay", "e": [1.0, 0.0]});
    l.upsert("keyed", "a", a).await?;
    l.migrate(keyed("alt")).await?;
    let s = Search {
        ty: TypeName::from("keyed"),
        ..doc_search(hybrid_over("e", "harbour", Some(vec![1.0, 0.0])))
    };
    let got = doors(store.as_ref())
        .search(&owner(store.as_ref()), s)
        .await;
    let got = got.must("a hybrid search over the re-keyed `keyed`")?.data;
    let counted: Vec<_> = got.axes.iter().map(|a| a.candidates).collect();
    ensure!(
        keys(&got) == ["z"] && counted == [Some(0), Some(1)],
        "after a re-key moved `a` to `z`, its vector must rank `z`, and be counted once: \
         it found {:?}, with {counted:?} candidates",
        keys(&got)
    );
    Ok(())
}

/// `keyed`, its key made of `key`: `code` or `alt`.
fn keyed(key: &str) -> TypeDecl {
    keyed_as("keyed", key)
}

/// [`keyed`] under the name `name`.
fn keyed_as(name: &str, key: &str) -> TypeDecl {
    let [mut code, mut alt] = ["code", "alt"].map(|n| field(n, StorageKind::Text));
    (code.required, alt.required) = (true, true);
    let fields = vec![code, alt, searchable("body"), vector("e", 2)];
    TypeDecl {
        key: vec![key.into()],
        ..type_decl(name, fields)
    }
}

/// The keys of the owner's search `s`, best first.
async fn near(store: &dyn LakeStore, s: Search) -> Result<Vec<String>, Failure> {
    let got = doors(store).search(&owner(store), s).await;
    let got = got.must("searching for the nearest to (1, 0)")?;
    Ok(got.data.hits.into_iter().map(|h| h.record.key.0).collect())
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
        text_fields: None,
        weight: None,
        text_weight: None,
        model: None,
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
        matches: false,
        at: ReadAt::default(),
        rerank: None,
        text_as_of: false,
    }
}

fn hybrid(text: &str, vector: Option<Vec<f32>>) -> SearchMode {
    SearchMode::Hybrid {
        text: text.into(),
        field: "v".into(),
        vector,
        fields: None,
        text_fields: None,
        weight: None,
        text_weight: None,
        model: None,
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
    let fut = cat.search(own, doc_search(SearchMode::vector("v", vec![1.0, 0.0])));
    fut.await.must("a vector search")?;
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
        text_fields: None,
        weight: None,
        text_weight: None,
        model: None,
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
                model: None,
            })
            .collect(),
        model: None,
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
    let masked = cat.search(&blind, doc_search(hybrid("plain", "vr-masked", None)));
    let masked = masked.await;
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

check!(
    SEARCH_RANKS_BY_THE_HEAD = a_search_as_of_a_position_ranks_by_the_heads_declaration,
    "025 and 051, extended (lakelands#756): a search reads its names, its caller's rights and its `where` at the position it reads, and its rankings by the head's declaration, whose indexes they read: a field's kind, its dimensions and whether a model fills it are the head's, and each field it names is ranked under the name it bears there; a field the head does not declare, or does not show the caller, holds no vectors there and no model fills it, so a vector ranking over it has no candidate and a hybrid search without a vector over it is refused, asking the backend for nothing; a record re-keyed since is ranked by its own text and vectors under its key at the position read, and one held at the head is no candidate to a reader without `restricted` (lakelands#797), equal scores in the order of the keys at the head, which its pages follow, so each of more ties than a batch is answered once (029); so is a link of a re-keyed end, whose key moves with it, at the head too; a pattern that starts by a search ranks as `search` does",
    reopens
);

/// A search of `ty` as `who`, as of `at`.
async fn as_of(
    store: &dyn LakeStore,
    who: &Principal,
    ty: &str,
    mode: SearchMode,
    at: Position,
) -> lakelands_api::Result<Hits> {
    let at = ReadAt {
        as_of: Some(at),
        ..ReadAt::default()
    };
    let s = Search {
        ty: ty.into(),
        at,
        ..doc_search(mode)
    };
    doors(store).search(who, s).await.map(|r| r.data)
}

/// A hybrid search for `text` over `field`.
fn hybrid_over(field: &str, text: &str, vector: Option<Vec<f32>>) -> SearchMode {
    SearchMode::Hybrid {
        text: text.into(),
        field: field.into(),
        vector,
        fields: None,
        text_fields: None,
        weight: None,
        text_weight: None,
        model: None,
    }
}

async fn a_search_as_of_a_position_ranks_by_the_heads_declaration(
    store: Store,
    reopen: Reopen,
) -> Held {
    use super::embeddings::{reopen_failed, with_embedder};
    use crate::conformance::backing::{EMBED_MODEL, embedded};
    let store = reopen(store, with_embedder(Some(backing::embedder())))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend", e))?;
    let s = store.as_ref();
    let (l, own) = (Lake::of(s), owner(s));
    let model = |from: &str| embedding("e", 2, &[from], EMBED_MODEL);
    let needs = json!({"code": "search_needs_vector", "field": "e"});
    let mut wrong = Vec::new();
    let mut row = |what: &str, ok: bool, got: &lakelands_api::Result<Hits>| {
        if !ok {
            let got = got.as_ref().map(|h| (keys(h), h.ranked_by.clone()));
            wrong.push(format!("{what}: {got:?}"));
        }
    };
    let found = |got: &lakelands_api::Result<Hits>, want: &[&str]| {
        got.as_ref().is_ok_and(|h| {
            let mut k = keys(h);
            k.sort();
            k == want
        })
    };

    // A model filled `e` at P, and fills it no more at the head.
    l.declare(type_decl("s1", vec![searchable("body"), model("body")]))
        .await?;
    l.upsert("s1", "a", json!({"body": "quayside"})).await?;
    let p = s.head().await.must("the head")?;
    l.declare(type_decl("s1", vec![searchable("body"), vector("e", 2)]))
        .await?;
    let got = as_of(s, &own, "s1", hybrid_over("e", "rh-s1", None), p).await;
    let ok = invalid_like(&got, &needs) && embedded("rh-s1") == 0;
    row(
        "S1, a hybrid search without a vector over `e`, which no model fills at the head",
        ok,
        &got,
    );

    // A model fills `e` at the head, and filled it not at P.
    l.declare(type_decl("s2", vec![searchable("body"), vector("e", 2)]))
        .await?;
    l.upsert("s2", "a", json!({"body": "quayside", "e": [0.0, 1.0]}))
        .await?;
    l.upsert("s2", "b", json!({"body": "harbour", "e": [0.0, 1.0]}))
        .await?;
    let p = s.head().await.must("the head")?;
    l.declare(type_decl("s2", vec![searchable("body"), model("body")]))
        .await?;
    let recomputed = l.cat.recompute(&l.own, Recompute::Type("s2".into()));
    recomputed.await.must("recomputing `s2`")?;
    let got = as_of(s, &own, "s2", hybrid_over("e", "quayside rh-s2", None), p).await;
    let ok = found(&got, &["a", "b"])
        && got.as_ref().is_ok_and(|h| h.ranked_by == "hybrid")
        && embedded("quayside rh-s2") == 1;
    row(
        "S2, a hybrid search without a vector over `e`, which the model fills at the head",
        ok,
        &got,
    );

    // `e` holds vectors of 2 at P and of 3 at the head; `a` wrote it after.
    l.declare(type_decl("s3", vec![searchable("body"), vector("e", 2)]))
        .await?;
    l.upsert("s3", "a", json!({"body": "quayside"})).await?;
    let p = s.head().await.must("the head")?;
    l.migrate(type_decl("s3", vec![searchable("body"), vector("e", 3)]))
        .await?;
    l.upsert("s3", "a", json!({"body": "quayside", "e": [0.0, 1.0, 0.0]}))
        .await?;
    let got = as_of(
        s,
        &own,
        "s3",
        SearchMode::vector("e", vec![0.0, 1.0, 0.0]),
        p,
    )
    .await;
    row("S3, three numbers over `e`", found(&got, &["a"]), &got);
    let got = as_of(s, &own, "s3", SearchMode::vector("e", vec![0.0, 1.0]), p).await;
    let ok = matches!(
        invalid_case(&got),
        Some(Case::QueryDimensions { query: 2, held: 3 })
    );
    row(
        "S3, two numbers over `e`, which holds vectors of 3 at the head",
        ok,
        &got,
    );

    // `e` is removed at the head.
    l.declare(type_decl("s4", vec![searchable("body"), vector("e", 2)]))
        .await?;
    l.upsert("s4", "a", json!({"body": "quayside", "e": [0.0, 1.0]}))
        .await?;
    let p = s.head().await.must("the head")?;
    l.migrate(type_decl("s4", vec![searchable("body")])).await?;
    let got = as_of(s, &own, "s4", SearchMode::vector("e", vec![0.0, 1.0]), p).await;
    row(
        "S4, a vector search over the removed `e`",
        found(&got, &[]),
        &got,
    );
    let mode = hybrid_over("e", "quayside", Some(vec![0.0, 1.0]));
    let got = as_of(s, &own, "s4", mode, p).await;
    row(
        "S4, a hybrid search with a vector, by its text",
        found(&got, &["a"]),
        &got,
    );
    let got = as_of(s, &own, "s4", hybrid_over("e", "rh-s4", None), p).await;
    let ok = invalid_like(&got, &needs) && embedded("rh-s4") == 0;
    row(
        "S4, a hybrid search without a vector over the removed `e`",
        ok,
        &got,
    );

    // `e` holds no vectors at the head: no record held one when it changed.
    l.declare(type_decl("s7", vec![searchable("body"), vector("e", 2)]))
        .await?;
    l.upsert("s7", "a", json!({"body": "quayside"})).await?;
    let p = s.head().await.must("the head")?;
    l.migrate(type_decl(
        "s7",
        vec![searchable("body"), field("e", StorageKind::Text)],
    ))
    .await?;
    let got = as_of(s, &own, "s7", SearchMode::vector("e", vec![0.0, 1.0]), p).await;
    let ok = invalid_like(
        &got,
        &json!({"code": "search_not_vector", "field": "e", "kind": "text"}),
    );
    row("a vector search over `e`, text at the head", ok, &got);

    // `e` is renamed `f`, and the text `t` renamed `u`, at the head.
    let t5 =
        |e: &str, t: &str| type_decl("s5", vec![searchable("body"), searchable(t), vector(e, 2)]);
    l.declare(t5("e", "t")).await?;
    let a = json!({"body": "harbour", "t": "quayside", "e": [0.0, 1.0]});
    l.upsert("s5", "a", a).await?;
    let p = s.head().await.must("the head")?;
    l.migrate_renaming(t5("f", "u"), &[("e", "f"), ("t", "u")])
        .await?;
    let got = as_of(s, &own, "s5", SearchMode::vector("e", vec![0.0, 1.0]), p).await;
    row(
        "S5, a vector search over `e`, `f` at the head",
        found(&got, &["a"]),
        &got,
    );
    let mode = SearchMode::Lexical {
        text: "quayside".into(),
        fields: Some(vec!["t".into()]),
        text_fields: None,
    };
    let got = as_of(s, &own, "s5", mode, p).await;
    row(
        "S6, a lexical search over `t`, `u` at the head",
        found(&got, &["a"]),
        &got,
    );

    // The rights stay at P, and a ranking reaches no field the head does not
    // show: `e`, plain at P, is made from the hidden `secret` at the head,
    // and `g`, made from it at P, is plain at the head.
    let blind = field_access(s, "blind", "s8", &[("secret", Access::None)]);
    let g = |from: Option<&str>| match from {
        Some(from) => embedding("g", 2, &[from], EMBED_MODEL),
        None => vector("g", 2),
    };
    let t8 = |e: FieldDecl, g: FieldDecl| {
        type_decl(
            "s8",
            vec![searchable("body"), field("secret", StorageKind::Text), e, g],
        )
    };
    l.declare(t8(vector("e", 2), g(Some("secret")))).await?;
    let a = json!({"body": "quayside", "secret": "berth", "e": [1.0, 0.0]});
    l.upsert("s8", "a", a).await?;
    let p = s.head().await.must("the head")?;
    l.declare(t8(model("secret"), g(None))).await?;
    let recomputed = l.cat.recompute(&l.own, Recompute::Type("s8".into()));
    recomputed.await.must("recomputing `s8`")?;
    let along = || SearchMode::vector("e", vec![1.0, 0.0]);
    let got = as_of(s, &own, "s8", along(), p).await;
    row(
        "the owner's vector search over `e`",
        found(&got, &["a"]),
        &got,
    );
    let got = as_of(s, &blind, "s8", along(), p).await;
    row(
        "a vector search over `e`, hidden at the head",
        found(&got, &[]),
        &got,
    );
    let got = as_of(s, &blind, "s8", hybrid_over("e", "rh-s8", None), p).await;
    let ok = invalid_like(&got, &needs) && embedded("rh-s8") == 0;
    row(
        "a hybrid search without a vector over `e`, hidden at the head",
        ok,
        &got,
    );
    // A record re-keyed since P is ranked by its own vector and text, under
    // its key at P, and one held at the head is no candidate to a reader
    // without `restricted` (lakelands#797). So is a link of a re-keyed end,
    // whose key moves with it, at the head as well.
    l.declare(keyed("code")).await?;
    let a = json!({"code": "a", "alt": "z", "body": "quay", "e": [1.0, 0.0]});
    l.upsert("keyed", "a", a).await?;
    let z = json!({"code": "z", "alt": "q", "body": "harbour", "e": [0.6, 0.8]});
    l.upsert("keyed", "z", z).await?;
    let pk = s.head().await.must("the head")?;
    l.migrate(keyed("alt")).await?;
    restrict(s, "keyed", "z", "legal hold").await?;
    let v = json!([{"name": "v", "type": {"kind": "vector", "dimensions": 2}}]);
    let lv = json!([{"name": "lv", "target": "keyed", "cardinality": "many_to_many", "fields": v}]);
    let ends = |key: &str| {
        let text = |n: &str| json!({"name": n, "type": {"kind": "text"}, "required": true});
        let ends = json!({"name": "ends", "key": [key], "fields": [text("code"), text("alt")],
                          "relations": lv});
        serde_json::from_value::<TypeDecl>(ends).expect("a type")
    };
    l.declare(ends("code")).await?;
    for (code, alt, v) in [("a", "z", [1.0, 0.0]), ("z", "a", [0.6, 0.8])] {
        l.upsert("ends", code, json!({"code": code, "alt": alt}))
            .await?;
        let (ty, key, on, to) = ("ends".into(), code.into(), "lv".into(), "q".into());
        let (v, opts) = (patch(json!({ "v": v })), WriteOpts::default());
        let linked = l.cat.link(&l.own, &ty, &key, &on, &to, v, opts);
        linked.await.must(&format!("linking `{code}`"))?;
    }
    let pl = s.head().await.must("the head")?;
    l.migrate(ends("alt")).await?;
    let (links, held) = ("sys.link.ends.lv", link_key(&"z".into(), &"q".into()));
    restrict(s, links, held, "legal hold").await?;
    let w = field_writer(s, "w", "keyed", &[]);
    // Each hit in the order answered, its score scaled and rounded.
    let scored = |got: &lakelands_api::Result<Hits>, scale: f64| {
        let scaled =
            |h: &lakelands_api::query::Hit| (h.record.key.0.clone(), (h.score * scale).round());
        Some(
            got.as_ref()
                .ok()?
                .hits
                .iter()
                .map(scaled)
                .collect::<Vec<_>>(),
        )
    };
    let near = |field: &str| SearchMode::vector(field, vec![1.0, 0.0]);
    let hybrid = hybrid_over("e", "harbour", Some(vec![1.0, 0.0]));
    type Scored<'a> = &'a [(&'a str, f64)];
    type Rekeyed<'a> = (
        &'a str,
        &'a str,
        Option<Position>,
        SearchMode,
        f64,
        Scored<'a>,
        Scored<'a>,
    );
    // What, the type, the position, the search, its scores' scale, and the
    // hits of the owner and of `w`, to whom the head's `z` is held.
    let rekeyed: [Rekeyed; 6] = [
        (
            "vector",
            "keyed",
            Some(pk),
            near("e"),
            10.0,
            &[("a", 10.0), ("z", 6.0)],
            &[("z", 6.0)],
        ),
        (
            // A tie, in the order of the keys at the head: `q`, then `z`.
            // Each token is held by one of two heads, ln(6/3), and to `w`
            // by the one head it is counted over, ln(4/3) (025).
            "lexical, both",
            "keyed",
            Some(pk),
            SearchMode::lexical("quay harbour"),
            10.0,
            &[("z", 7.0), ("a", 7.0)],
            &[("z", 3.0)],
        ),
        (
            "lexical, the held one",
            "keyed",
            Some(pk),
            SearchMode::lexical("quay"),
            10.0,
            &[("a", 7.0)],
            &[],
        ),
        // Fused by reciprocal rank: `z` by its text and second by its
        // vector, `a` by its vector alone; alone, `z` is first in both.
        (
            "hybrid",
            "keyed",
            Some(pk),
            hybrid,
            10_000.0,
            &[("z", 331.0), ("a", 167.0)],
            &[("z", 333.0)],
        ),
        (
            "vector over links, at the head",
            links,
            None,
            near("v"),
            10.0,
            &[("z:q", 10.0), ("a:q", 6.0)],
            &[("a:q", 6.0)],
        ),
        (
            "vector over links",
            links,
            Some(pl),
            near("v"),
            10.0,
            &[("a:q", 10.0), ("z:q", 6.0)],
            &[("z:q", 6.0)],
        ),
    ];
    for (what, ty, at, mode, scale, owned, unheld) in rekeyed {
        for (who, want) in [(&own, owned), (&w, unheld)] {
            let asked = Search {
                ty: ty.into(),
                at: read_at(at),
                ..doc_search(mode.clone())
            };
            let got = doors(s).search(who, asked).await.map(|r| r.data);
            let want: Vec<_> = want.iter().map(|(k, t)| ((*k).to_owned(), *t)).collect();
            let what = format!("{what}, as `{}`, over `{ty}` re-keyed since", who.id);
            row(&what, scored(&got, scale) == Some(want), &got);
        }
    }
    // More equal scores than one batch pulls, re-keyed since P into the
    // reverse order: the pages follow the head's keys, so every record is
    // answered once and none twice (029).
    l.declare(keyed_as("tied", "code")).await?;
    for i in 0..70 {
        let (code, alt) = (format!("k{i:02}"), format!("r{:02}", 69 - i));
        let r = json!({"code": code, "alt": alt, "body": "quay", "e": [1.0, 0.0]});
        l.upsert("tied", &code, r).await?;
    }
    let pt = s.head().await.must("the head")?;
    l.migrate(keyed_as("tied", "alt")).await?;
    let fused = hybrid_over("e", "quay", Some(vec![1.0, 0.0]));
    for (what, mode) in [
        ("lexical", SearchMode::lexical("quay")),
        ("vector", near("e")),
        ("hybrid", fused),
    ] {
        let (mut seen, mut cursor) = (Vec::new(), None);
        loop {
            let asked = Search {
                ty: "tied".into(),
                at: read_at(Some(pt)),
                cursor,
                ..doc_search(mode.clone())
            };
            let Ok(page) = doors(s).search(&own, asked).await.map(|r| r.data) else {
                break;
            };
            seen.extend(page.hits.iter().map(|h| h.record.key.0.clone()));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        let all = seen.len();
        seen.sort();
        seen.dedup();
        if (all, seen.len()) != (70, 70) {
            let distinct = seen.len();
            wrong.push(format!(
                "{what} pages over 70 tied records re-keyed since: {all} hits, {distinct} distinct"
            ));
        }
    }
    // A pattern that starts by a search ranks as the search does.
    let mut counted = Vec::new();
    for who in [&blind, &own] {
        let mut q = pattern("s8", None, vec![], Projection::Rows { fields: Vec::new() });
        (q.start.search, q.at) = (Some(along()), read_at(Some(p)));
        counted.push(match doors(s).query(who, q).await.map(|r| r.data) {
            Ok(QueryResult::Rows { rows, .. }) => rows.len(),
            _ => usize::MAX,
        });
    }
    if counted != [0, 1] {
        wrong.push(format!("a pattern started by a vector search over `e`: {counted:?} rows for `blind` and the owner"));
    }
    ensure!(
        wrong.is_empty(),
        "a search as of a position ranks by the head's declaration: {wrong:?}"
    );
    let hidden = as_of(s, &blind, "s8", SearchMode::vector("g", vec![1.0, 0.0]), p).await;
    let absent = as_of(
        s,
        &blind,
        "s8",
        SearchMode::vector("nosuch", vec![1.0, 0.0]),
        p,
    )
    .await;
    refused_alike(
        "a vector search over `g`, made at P from the hidden `secret` and plain at the head",
        &hidden,
        absent,
        ("nosuch", "secret"),
    )
}

check!(
    TEXT_AS_OF_FINDS_THE_TEXT_THEN = a_search_with_text_as_of_ranks_the_text_the_records_held_then,
    "089 (lakelands#1541): a lexical search as of a position with `text_as_of` ranks the records its `where` selects there by the text they held there, so of two pages whose words a later write swapped it finds the one that held them then, its matches in that text; without `text_as_of` it ranks by the head, as before, and at the head it finds the page that holds them now"
);

check!(
    TEXT_AS_OF_SAME_AT_EVERY_HEAD = a_search_with_text_as_of_answers_the_same_at_every_head,
    "089 (lakelands#1541): a search with `text_as_of` answers the same pages, scores to the bit and matches at every head: writes after its position to the records it ranks, and a record written after it, move none of it, its cursor's next page neither; a record its `where` leaves out weighs in no score"
);

check!(
    TEXT_AS_OF_REFUSED = text_as_of_is_refused_where_it_cannot_rank_the_text_then,
    "089 (lakelands#1541): `text_as_of` without `as_of`, without `where`, beside a vector axis or beside `rerank` is refused as invalid naming `text_as_of`, before the `where`, the fields or a reranker are judged; a `where` that selects more records at the position read than the lake's `limits.max_text_as_of` is refused as `limit_range` naming the setting, never cut short, and one that selects as many is answered whole, however many it selects at the head; a location whose `limits.max_text_as_of` is above 1 000 000 is refused at open, naming the setting",
    given
);

check!(
    TEXT_AS_OF_HOLDS_BOTH_POSITIONS =
        text_as_of_ranks_a_field_only_where_it_is_shown_at_both_positions,
    "089 and 025 (lakelands#1541): a search with `text_as_of` ranks and matches a field of a record only where its reader is shown it there both at the position read and at the head: a field a hiding lowers on the record at the head, or lowered on it at the position, is neither matched nor ranked nor counted, the record scored as one that never held it; a record retired at the head since is ranked as it was"
);

check!(
    TEXT_AS_OF_DECLARED_THEN = a_search_with_text_as_of_reads_the_declaration_then,
    "089 and 025 (lakelands#1541): a search with `text_as_of` tokenises in the language its type declared at the position read, not the head's, and ranks and matches a field only where it is searchable both there and at the head"
);

/// `page`: `doc` names its document, `state` its state, `title` and
/// `text` are searchable.
async fn pages(store: &dyn LakeStore, rows: &[(&str, Value)]) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let text = |name| field(name, StorageKind::Text);
    let fields = vec![
        text("doc"),
        text("state"),
        searchable("title"),
        searchable("text"),
    ];
    l.declare(type_decl("page", fields)).await?;
    for (key, fields) in rows {
        l.upsert("page", key, fields.clone()).await?;
    }
    Ok(l)
}

/// A search of `page` for `text` in `doc` d1, with its matches, as of
/// `at`, with `text_as_of` where `then`.
fn dated(text: &str, at: Option<Position>, then: bool) -> Search {
    Search {
        ty: "page".into(),
        filter: Some("doc = 'd1'".into()),
        matches: true,
        text_as_of: then,
        at: ReadAt {
            as_of: at,
            ..ReadAt::default()
        },
        ..doc_search(SearchMode::lexical(text))
    }
}

/// Each hit's key, and each field it matched with its spans.
fn matched(h: &Hits) -> String {
    let hit = |h: &lakelands_api::query::Hit| {
        let fields = h.matches.iter().map(|(f, m)| {
            let spans = m.spans.iter().map(|s| format!("{}-{}", s.start, s.end));
            format!("{f}@{}", spans.collect::<Vec<_>>().join(","))
        });
        let fields: Vec<String> = fields.collect();
        format!("{}:{}", h.record.key.as_str(), fields.join(" "))
    };
    h.hits.iter().map(hit).collect::<Vec<_>>().join(" / ")
}

async fn a_search_with_text_as_of_ranks_the_text_the_records_held_then(store: Store) -> Held {
    let s = store.as_ref();
    let page = |text: &str| json!({"doc": "d1", "text": text});
    let (cover, invoice) = ("Cover letter", "Invoice 4711, total");
    let l = pages(s, &[("p1", page(cover)), ("p2", page(invoice))]).await?;
    let before = s.head().await.must("the head")?;
    // The second ingest swaps the pages' words.
    l.upsert("page", "p1", page(invoice)).await?;
    l.upsert("page", "p2", page(cover)).await?;
    let own = owner(s);
    let mut wrong = Vec::new();
    let rows = [
        (
            "before, with `text_as_of`",
            Some(before),
            true,
            "p2:text@8-12",
        ),
        ("before, without it", Some(before), false, "p1:"),
        ("at the head", None, false, "p1:text@8-12"),
    ];
    for (what, at, then, want) in rows {
        let got = l.cat.search(&own, dated("4711", at, then)).await;
        let got = got.must(what)?.data;
        let said = matched(&got);
        if said != want {
            wrong.push(format!("{what}: want {want:?}, got {said:?}"));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a search for `4711` in d1, whose two pages swapped their words after `before`, must \
         find the page that held it at the position it ranks the text of; {}",
        wrong.join("; ")
    );
    Ok(())
}

/// A page's matches and its scores' bits.
fn scored(h: &Hits) -> String {
    let bits = h.hits.iter().map(|h| format!("{:x}", h.score.to_bits()));
    format!("{} {}", matched(h), bits.collect::<Vec<_>>().join(" "))
}

/// Every page of `asked`, `k` 1, each its keys, scores' bits and
/// matches, and the first page's cursor.
async fn walked(l: &Lake, asked: Search) -> Result<(Vec<String>, Option<Cursor>), Failure> {
    let (mut said, mut cursor, mut first) = (Vec::new(), None, None);
    loop {
        let s = Search {
            k: 1,
            cursor: cursor.clone(),
            ..asked.clone()
        };
        let got = l.cat.search(&l.own, s).await.must("a page")?.data;
        said.push(scored(&got));
        first = first.or(got.next_cursor.clone());
        cursor = got.next_cursor;
        if cursor.is_none() || said.len() > 4 {
            return Ok((said, first));
        }
    }
}

async fn a_search_with_text_as_of_answers_the_same_at_every_head(store: Store) -> Held {
    let s = store.as_ref();
    let page = |doc: &str, text: &str| json!({"doc": doc, "text": text});
    let rows = [
        ("p1", page("d1", "Cover letter")),
        ("p2", page("d1", "Invoice 4711, total")),
        ("p3", page("d1", "Total due in 30 days")),
        ("p4", page("d2", "4711 4711 total total")),
    ];
    let l = pages(s, &rows).await?;
    let before = s.head().await.must("the head")?;
    l.upsert("page", "p1", page("d1", "Invoice 4711, total"))
        .await?;
    l.upsert("page", "p2", page("d1", "Cover letter")).await?;
    let asked = dated("4711 total", Some(before), true);
    let (first, cursor) = walked(&l, asked.clone()).await?;
    // p4, which d1's `where` leaves out, moves the scores where it is in.
    let wide = Search {
        filter: Some("doc in ('d1', 'd2')".into()),
        ..asked.clone()
    };
    let wide = (l.cat.search(&l.own, wide).await).must("d1 and d2")?.data;
    let p2 = |h: &Hits| {
        (h.hits.iter())
            .find(|h| h.record.key.as_str() == "p2")
            .map(|h| h.score)
    };
    let wide = p2(&wide);
    ensure!(
        first.len() == 2
            && first[0].starts_with("p2:")
            && first[1].starts_with("p3:")
            && wide.is_some_and(|w| !first[0].ends_with(&format!(" {:x}", w.to_bits()))),
        "a search for `4711 total` in d1 as of before, with `text_as_of`, must answer p2, then \
         p3, a page each, p2 scored over d1's pages alone, not as over d1's and d2's \
         ({wide:?}); it answered {first:?}"
    );
    // More writes to the records it ranks, and one it did not hold.
    l.upsert("page", "p2", page("d1", "4711 4711 4711 total"))
        .await?;
    l.upsert("page", "p3", page("d2", "Total")).await?;
    l.upsert("page", "p5", page("d1", "4711 total")).await?;
    let (again, _) = walked(&l, asked.clone()).await?;
    let next = Search {
        k: 1,
        cursor,
        ..asked
    };
    let next = (l.cat.search(&l.own, next).await).must("the cursor's next page")?;
    let next = scored(&next.data);
    ensure!(
        again == first && first.get(1) == Some(&next),
        "a search with `text_as_of` must answer the same at every head, as must its cursor's \
         next page: it answered {first:?}, after more writes {again:?}, and its cursor then \
         {next:?}"
    );
    Ok(())
}

async fn text_as_of_is_refused_where_it_cannot_rank_the_text_then(
    store: Store,
    given: Given,
) -> Held {
    let page = |doc: &str| json!({"doc": doc, "text": "4711"});
    let rows = [
        ("p1", page("d1")),
        ("p2", page("d1")),
        ("p3", page("d1")),
        ("q1", page("d2")),
        ("q2", page("d2")),
    ];
    let l = pages(store.as_ref(), &rows).await?;
    let before = store.head().await.must("the head")?;
    l.upsert("page", "q3", page("d2")).await?;
    // Each `where` names a field `page` does not declare, and the rerank
    // has no reranker bound: neither is reached.
    let nowhere = |mut s: Search| {
        s.filter = s.filter.map(|_| "nope = 'x'".into());
        s
    };
    let needs = |member: &str| Case::TextAsOfNeeds {
        member: member.into(),
    };
    let beside = |member: &str| Case::TextAsOfBeside {
        member: member.into(),
    };
    let then = dated("4711", Some(before), true);
    let (mut no_where, mut vector, mut rerank) = (then.clone(), then.clone(), then.clone());
    no_where.filter = None;
    vector.mode = hybrid("4711", Some(vec![1.0, 0.0]));
    rerank.rerank = Some(lakelands_api::query::Rerank {
        top: u32::MAX,
        text: None,
    });
    for (what, asked, want) in [
        ("without `as_of`", dated("4711", None, true), needs("as_of")),
        ("without `where`", no_where, needs("where")),
        ("with a vector axis", vector, beside("vector")),
        ("with `rerank`", rerank, beside("rerank")),
    ] {
        let err = (l.cat.search(&l.own, nowhere(asked)).await).must_fail(what)?;
        ensure!(
            refused_on(&err).is_some_and(|(f, _)| f == "text_as_of")
                && invalid_case(&err) == Some(&want),
            "a search with `text_as_of` {what} must be refused as invalid naming `text_as_of` \
             with {want:?}, before its `where` is read; it failed with {err:?}"
        );
    }
    let two = Limits {
        max_text_as_of: 2,
        ..crate::conformance::fixture::limits()
    };
    let store = (given.relimit)(store, Seams::default(), two).await;
    let store = store.map_err(|e| Failure::new(format!("reopening with two: {e:?}")))?;
    let l = Lake::of(store.as_ref());
    let err = (l.cat.search(&l.own, then.clone()).await)
        .must_fail("a `where` selecting three pages, past the lake's two")?;
    ensure!(
        refused_on(&err).is_some_and(|(f, _)| f == "limits.max_text_as_of")
            && invalid_case(&err) == Some(&Case::LimitRange { max: 2 }),
        "a `where` selecting three records at the position read, past the lake's \
         `limits.max_text_as_of` of 2, must be refused as `limit_range` naming the setting; it \
         failed with {err:?}"
    );
    let d2 = Search {
        filter: Some("doc = 'd2'".into()),
        ..then
    };
    let got = l.cat.search(&l.own, d2).await.must("two pages of d2")?.data;
    ensure!(
        keys(&got) == ["q1", "q2"],
        "a `where` selecting two records at the position read, three at the head, must be \
         answered whole under a `limits.max_text_as_of` of 2; it answered {:?}",
        keys(&got)
    );
    let above = Limits {
        max_text_as_of: Limits::TEXT_AS_OF_CEILING + 1,
        ..crate::conformance::fixture::limits()
    };
    let err = (given.relimit)(store, Seams::default(), above).await;
    let err = err.err().ok_or_else(|| {
        Failure::new("a location whose `limits.max_text_as_of` is above 1 000 000 must not be opened; it was")
    })?;
    ensure!(
        matches!(&err, Error::Refused { field: Some(f), .. } if f == "limits.max_text_as_of")
            && invalid_case(&err)
                == Some(&Case::LimitRange {
                    max: Limits::TEXT_AS_OF_CEILING
                }),
        "a location whose `limits.max_text_as_of` is above 1 000 000 must be refused at open \
         as `limit_range`, naming the setting; it failed with {err:?}"
    );
    Ok(())
}

async fn text_as_of_ranks_a_field_only_where_it_is_shown_at_both_positions(store: Store) -> Held {
    let s = store.as_ref();
    let page = |doc: &str, state: &str, text: bool| {
        let mut p = json!({"doc": doc, "state": state, "title": "4711", "text": "4711 total"});
        _ = (!text).then(|| p.as_object_mut().map(|o| o.remove("text")));
        p
    };
    // `a1` is sealed at the head, `a2` before, `a3` never; d2's `b1` and
    // `b2` are their twins that never held `text`.
    let rows = [
        ("a1", page("d1", "open", true)),
        ("a2", page("d1", "sealed", true)),
        ("a3", page("d1", "open", true)),
        ("b1", page("d2", "open", false)),
        ("b2", page("d2", "open", false)),
        ("b3", page("d2", "open", true)),
    ];
    let l = pages(s, &rows).await?;
    let before = s.head().await.must("the head")?;
    l.upsert("page", "a1", json!({"state": "sealed"})).await?;
    l.upsert("page", "a2", json!({"state": "open"})).await?;
    let sealed = json!({"field": "state", "values": ["sealed"], "fields": {"text": "none"}});
    let grants = json!({"default": "read", "types": {"page": {"hide_where": [sealed]}}});
    let bea = super::only::holder(s, "bea", grants);
    let search = |doc: &str| Search {
        filter: Some(format!("doc = '{doc}'").as_str().into()),
        ..dated("4711 total", Some(before), true)
    };
    let bits = |h: &Hits| h.hits.iter().map(|h| h.score.to_bits()).collect::<Vec<_>>();
    let got = l
        .cat
        .search(&bea, search("d1"))
        .await
        .must("bea's search")?
        .data;
    let twin = l.cat.search(&bea, search("d2")).await.must("d2")?.data;
    // A page retired at the head was there at the position.
    l.retire("page", "a3").await?;
    let then = l
        .cat
        .search(&bea, search("d1"))
        .await
        .must("after a3's retiring")?
        .data;
    let said = matched(&got);
    let want = "a3:text@0-4,5-10 title@0-4 / a1:title@0-4 / a2:title@0-4";
    ensure!(
        said == want && bits(&got) == bits(&twin) && scored(&then) == scored(&got),
        "bea, shown `text` on a page only where it is not sealed, must find in d1 as of \
         before, with `text_as_of`, `a3`'s text and every title, and neither `a1`'s text, \
         sealed at the head, nor `a2`'s, sealed before, scored as d2's twins that never held \
         it, and the same after `a3` is retired at the head: want {want:?}, got {said:?}, \
         scored {:?} against {:?}, then {:?}",
        bits(&got),
        bits(&twin),
        scored(&then)
    );
    Ok(())
}

async fn a_search_with_text_as_of_reads_the_declaration_then(store: Store) -> Held {
    let s = store.as_ref();
    let l = Lake::of(s);
    // `title` is searchable before only, `note` at the head only, and
    // English is dropped since.
    let decl = |then: bool| {
        let text = |name| field(name, StorageKind::Text);
        let mut fields = vec![text("doc"), text("title"), searchable("text"), text("note")];
        fields[if then { 1 } else { 3 }].searchable = true;
        let mut decl = type_decl("page", fields);
        decl.language = then.then(|| "en".into());
        decl
    };
    l.declare(decl(true)).await?;
    let page =
        |text: &str| json!({"doc": "d1", "title": "running", "text": text, "note": "running"});
    l.upsert("page", "p1", page("running invoices")).await?;
    l.upsert("page", "p2", page("invoices")).await?;
    let before = s.head().await.must("the head")?;
    l.declare(decl(false)).await?;
    let (mut said, want) = (Vec::new(), "p1:text@0-7");
    for named in [None, Some(vec!["text".into(), "title".into()])] {
        let mut s = dated("run", Some(before), true);
        if let SearchMode::Lexical { fields, .. } = &mut s.mode {
            *fields = named;
        }
        said.push(matched(
            &l.cat.search(&l.own, s).await.must("the search")?.data,
        ));
    }
    ensure!(
        said.iter().all(|s| s == want),
        "a search for `run` as of before, with `text_as_of`, naming no field and naming `text` \
         and `title`, must stem by the English the type declared then and rank neither `title`, \
         searchable then alone, nor `note`, searchable at the head alone: want {want:?}, got \
         {said:?}"
    );
    Ok(())
}

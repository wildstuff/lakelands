//! Ranked search: one token pipeline, one order, and no probing
//! (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! `docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`,
//! `docs/decisions/029-every-answer-has-one-order.md`).

use std::sync::Arc;

use lakelands_api::ontology::{FieldDecl, StorageKind};
use lakelands_api::query::{Hits, Search, SearchMode};
use lakelands_api::{Access, Error, LakeStore, ReadAt, Refusal, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, declare, doors, field, field_writer, owner, type_decl, upsert,
};

check!(
    /// Words match by the one pipeline: case, stem and diacritics fold.
    MATCH_SET = full_text_matches_words_whatever_their_case_inflection_or_accents,
    "025: the store's one token pipeline splits, lower-cases, stems and folds diacritics, so the same query matches the same records on every engine"
);

check!(
    /// Hits come by score, then by key.
    SEARCH_ORDER = search_is_ordered_by_score_then_key,
    "029: ranked search answers best first, and equal scores in key order"
);

check!(
    /// Search cannot probe an unreadable field.
    SEARCH_PROBE_REFUSED = searching_a_type_whose_searchable_field_is_unreadable_is_refused,
    "024: a search over a searchable field the principal may not read is refused, naming the field"
);

async fn articles(store: &dyn LakeStore) -> Result<(), Failure> {
    let cat = doors(store);
    let mut ty = type_decl(
        "article",
        vec![FieldDecl {
            searchable: true,
            ..field("text", StorageKind::Text)
        }],
    );
    // Stemming follows the type's language (025); without one, words are
    // only split and lower-cased.
    ty.language = Some("en".into());
    declare(cat.as_ref(), &owner(store), ty).await?;
    Ok(())
}

fn lexical(text: &str) -> Search {
    Search {
        ty: TypeName::from("article"),
        mode: SearchMode::Lexical { text: text.into() },
        k: 20,
        cursor: None,
        filter: None,
        with_meta: false,
        at: ReadAt::default(),
    }
}

async fn hits(store: &dyn LakeStore, text: &str) -> Result<Hits, Failure> {
    Ok(doors(store)
        .search(&owner(store), lexical(text))
        .await
        .must(&format!("searching for {text:?}"))?
        .data)
}

fn keys(h: &Hits) -> Vec<&str> {
    h.hits.iter().map(|h| h.record.key.as_str()).collect()
}

async fn full_text_matches_words_whatever_their_case_inflection_or_accents(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    articles(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    upsert(
        cat.as_ref(),
        &own,
        "article",
        "cafe",
        json!({"text": "The Café was Running late"}),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &own,
        "article",
        "other",
        json!({"text": "Nothing to see here"}),
    )
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

async fn search_is_ordered_by_score_then_key(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    articles(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    for (key, text) in [
        ("b", "lake"),
        ("a", "lake"),
        ("c", "lake lake lake shore"),
        ("d", "mountain"),
    ] {
        upsert(cat.as_ref(), &own, "article", key, json!({ "text": text })).await?;
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

async fn searching_a_type_whose_searchable_field_is_unreadable_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    articles(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    upsert(
        cat.as_ref(),
        &owner(store.as_ref()),
        "article",
        "a",
        json!({"text": "secret plan"}),
    )
    .await?;
    let mut reader = field_writer(store.as_ref(), "reader", "article", &[]);
    reader
        .grants
        .types
        .get_mut(&TypeName::from("article"))
        .expect("field_writer grants the type")
        .fields
        .insert("text".into(), Access::Masked);
    let err = cat
        .search(&reader, lexical("secret"))
        .await
        .must_fail("searching `article`, whose `text` the reader sees only masked")?;
    ensure!(
        matches!(&err, Error::Refused { field: Some(f), reason: Refusal::NotGranted } if f.as_str() == "text"),
        "a search over the masked `text` must be refused as not granted, naming `text`; \
         it failed with {err:?}"
    );
    Ok(())
}

check!(
    /// Search follows the text as it now stands.
    SEARCH_FOLLOWS_TEXT = a_search_finds_a_record_by_its_current_text_and_nothing_by_none,
    "025: the index follows every write, so a record is found by its new words and no longer by its old ones, and a query of no words matches nothing"
);

check!(
    /// Withheld records are never hits.
    WITHHELD_NEVER_HITS = a_retired_or_restricted_record_is_never_a_hit,
    "the log format §2.3 and 006: a retired record is invisible, and a restricted one invisible to a principal without the `restricted` grant; neither is ever a search hit"
);

check!(
    /// A vector that is gone is not a neighbour.
    VECTOR_LEAVES_THE_NEIGHBOURS = a_retired_record_or_a_cleared_vector_is_no_longer_a_nearest_neighbour,
    "the engine seam's vector index: it follows the projection, so a retired record or a cleared vector field is no longer among the nearest"
);

async fn a_search_finds_a_record_by_its_current_text_and_nothing_by_none(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    articles(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    upsert(
        cat.as_ref(),
        &own,
        "article",
        "a",
        json!({"text": "harbour crane"}),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &own,
        "article",
        "a",
        json!({"text": "mountain goat"}),
    )
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

async fn a_retired_or_restricted_record_is_never_a_hit(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::WriteOpts;
    articles(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    for k in ["live", "gone", "held"] {
        upsert(
            cat.as_ref(),
            &own,
            "article",
            k,
            json!({"text": "lake report"}),
        )
        .await?;
    }
    let article = TypeName::from("article");
    cat.retire(&own, &article, &"gone".into(), WriteOpts::default())
        .await
        .must("retiring `gone`")?;
    store
        .restrict(&article, &"held".into(), "legal hold".into())
        .await
        .must("restricting `held`")?;
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
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::WriteOpts;
    use lakelands_api::ontology::FieldType;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let embedding = FieldDecl {
        ty: FieldType {
            dimensions: Some(2),
            ..FieldType::kind(StorageKind::Vector)
        },
        ..field("embedding", StorageKind::Text)
    };
    declare(cat.as_ref(), &own, type_decl("doc", vec![embedding])).await?;
    for (k, v) in [
        ("east", [1.0, 0.0]),
        ("north", [0.0, 1.0]),
        ("near", [0.9, 0.1]),
    ] {
        upsert(cat.as_ref(), &own, "doc", k, json!({ "embedding": v })).await?;
    }
    let nearest = |store: Arc<dyn LakeStore>| async move {
        let s = Search {
            ty: TypeName::from("doc"),
            mode: SearchMode::Vector {
                field: "embedding".into(),
                vector: vec![1.0, 0.0],
            },
            k: 10,
            cursor: None,
            filter: None,
            with_meta: false,
            at: ReadAt::default(),
        };
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
    cat.retire(&own, &doc, &"east".into(), WriteOpts::default())
        .await
        .must("retiring `east`")?;
    upsert(
        cat.as_ref(),
        &own,
        "doc",
        "near",
        json!({"embedding": null}),
    )
    .await?;
    let after = nearest(store.clone()).await?;
    ensure!(
        !after.contains(&"east".to_owned()) && !after.contains(&"near".to_owned()),
        "after `east` was retired and `near`'s vector cleared, neither may be a \
         neighbour; the nearest were {after:?}"
    );
    Ok(())
}

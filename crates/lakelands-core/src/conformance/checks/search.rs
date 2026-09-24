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

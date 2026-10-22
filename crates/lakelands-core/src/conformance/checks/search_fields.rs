//! A search limited to named fields: it matches, ranks and probes in
//! those fields only, and the vector half of a hybrid search is not
//! narrowed (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! `docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).

use super::search::{doc_search, keys, masking, refused};
use crate::conformance::fixture::{doors, owner};
use crate::conformance::prelude::*;

check!(
    FIELDS_NARROW_THE_MATCH = a_search_limited_to_named_fields_matches_and_ranks_in_those_only,
    "024 and 025: a lexical or hybrid search that names `fields` matches, scores and ties over those searchable fields only, the same on every engine; without `fields` it searches every searchable field; a hybrid search without a vector or a backend, over a field a model fills, falls back to its text in those fields",
    reopens
);

check!(
    FIELDS_LIFT_THE_PROBE = a_search_limited_to_readable_fields_is_answered_beside_a_masked_one,
    "024: a search that names `fields` probes only those, and one that names none searches the searchable fields its principal reads, so a searchable field the principal may not read neither refuses nor ranks a search that does not name it; the field stays masked in every hit"
);

check!(
    FIELDS_REFUSED = naming_an_undeclared_unsearchable_or_unreadable_field_is_refused,
    "024: a named field that is not declared or not searchable is invalid, one the principal may not read is not granted, judged before its searchability, and a search naming no field is invalid"
);

check!(
    FIELDS_KEEP_THE_EMBED_PROBE =
        a_hybrid_search_limited_to_named_fields_still_probes_what_its_vector_is_embedded_from,
    "024: `fields` narrows only the text half of a hybrid search; its vector still encodes every field it is embedded from, so a hybrid search over it is refused, naming the field, when the principal may not read one, whatever `fields` names",
    embedding
);

/// `doc` with two searchable texts, `note` and `secret`, a text that is
/// not searchable, `label`, and a plain vector `w`. For "harbour
/// quayside", `a` and `b` hold both words and `c` one over every
/// searchable field; `b` holds both in `note`, `a` one, `c` none.
async fn accounts(store: &dyn LakeStore) -> Held {
    let l = Lake::of(store);
    let w = vector("w", 2);
    l.declare(type_decl(
        "doc",
        vec![
            searchable("note"),
            searchable("secret"),
            field("label", StorageKind::Text),
            w,
        ],
    ))
    .await?;
    for (key, fields) in [
        (
            "a",
            json!({"note": "harbour", "secret": "quayside harbour", "w": [0.0, 1.0]}),
        ),
        ("b", json!({"note": "harbour quayside", "w": [1.0, 0.0]})),
        ("c", json!({"secret": "harbour", "label": "harbour"})),
    ] {
        l.upsert("doc", key, fields).await?;
    }
    Ok(())
}

fn named(names: &[&str]) -> Option<Vec<lakelands_api::FieldName>> {
    Some(names.iter().map(|&n| n.into()).collect())
}

fn lexical_in(names: &[&str]) -> SearchMode {
    SearchMode::Lexical {
        text: "harbour quayside".into(),
        fields: named(names),
        text_fields: None,
    }
}

/// A hybrid search for `text` along `field`, its text half in `names`.
fn hybrid_in(text: &str, field: &str, vector: Option<Vec<f32>>, names: &[&str]) -> SearchMode {
    let (text, field, fields) = (text.into(), field.into(), named(names));
    SearchMode::Hybrid {
        text,
        field,
        vector,
        fields,
        text_fields: None,
        weight: None,
        text_weight: None,
        model: None,
    }
}

fn scored(h: &Hits) -> Vec<(&str, f64)> {
    h.hits
        .iter()
        .map(|h| (h.record.key.as_str(), h.score))
        .collect()
}

async fn answer(
    store: &dyn LakeStore,
    who: &Principal,
    mode: SearchMode,
    what: &str,
) -> Result<Hits, Failure> {
    Ok(doors(store)
        .search(who, doc_search(mode))
        .await
        .must(what)?
        .data)
}

async fn a_search_limited_to_named_fields_matches_and_ranks_in_those_only(
    store: Store,
    reopen: Reopen,
) -> Held {
    accounts(store.as_ref()).await?;
    let own = owner(store.as_ref());
    let every = answer(
        store.as_ref(),
        &own,
        SearchMode::lexical("harbour quayside"),
        "a search in every searchable field",
    )
    .await?;
    ensure!(
        scored(&every) == [("a", 2.0), ("b", 2.0), ("c", 1.0)],
        "searching \"harbour quayside\" in every searchable field answered {:?}",
        scored(&every)
    );
    let note = answer(
        store.as_ref(),
        &own,
        lexical_in(&["note"]),
        "a search in `note`",
    )
    .await?;
    ensure!(
        scored(&note) == [("b", 2.0), ("a", 1.0)],
        "searching \"harbour quayside\" in `note` answered {:?}; `b` holds both words there, \
         `a` one, and `c` holds them only in `secret`, which was not named",
        scored(&note)
    );
    let secret = answer(
        store.as_ref(),
        &own,
        lexical_in(&["secret"]),
        "a search in `secret`",
    )
    .await?;
    ensure!(
        scored(&secret) == [("a", 2.0), ("c", 1.0)],
        "searching \"harbour quayside\" in `secret` answered {:?}",
        scored(&secret)
    );
    // Without a vector, over `e`, the embedding of `note`: declared where
    // the suite's backend is bound, searched where none is (lakelands#743).
    use super::embeddings::{reopen_failed, with_embedder};
    let store = reopen(store, with_embedder(Some(backing::embedder())))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend", e))?;
    let e = embedding("e", 2, &["note"], backing::EMBED_MODEL);
    let fields = vec![
        searchable("note"),
        searchable("secret"),
        field("label", StorageKind::Text),
        vector("w", 2),
        e,
    ];
    Lake::of(store.as_ref())
        .declare(type_decl("doc", fields))
        .await?;
    let store = reopen(store, with_embedder(None))
        .await
        .map_err(|e| reopen_failed("with no embedding backend", e))?;
    let fallback = answer(
        store.as_ref(),
        &own,
        hybrid_in("harbour quayside", "e", None, &["note"]),
        "a hybrid search in `note` without a vector",
    )
    .await?;
    ensure!(
        scored(&fallback) == [("b", 2.0), ("a", 1.0)] && fallback.ranked_by.starts_with("lexical"),
        "a hybrid search in `note` without a vector or a backend falls back to lexical over \
         `note`; it answered {:?}, ranked by {:?}",
        scored(&fallback),
        fallback.ranked_by
    );
    Ok(())
}

async fn a_search_limited_to_readable_fields_is_answered_beside_a_masked_one(store: Store) -> Held {
    accounts(store.as_ref()).await?;
    let reader = masking(store.as_ref(), "secret");
    let every = answer(
        store.as_ref(),
        &reader,
        SearchMode::lexical("harbour quayside"),
        "the reader's search naming no field",
    )
    .await?;
    let note = answer(
        store.as_ref(),
        &reader,
        lexical_in(&["note"]),
        "the reader's search in `note`",
    )
    .await?;
    ensure!(
        scored(&note) == [("b", 2.0), ("a", 1.0)],
        "the reader's search in `note` answered {:?}; it must answer as the owner's does",
        scored(&note)
    );
    ensure!(
        scored(&every) == scored(&note),
        "a search naming no field, by a reader who sees `secret` only masked, searches          `note` alone; it answered {:?}",
        scored(&every)
    );
    let a = &note.hits[1].record;
    ensure!(
        a.masked.iter().any(|f| f.as_str() == "secret") && !a.fields.contains_key("secret"),
        "`a` holds `secret`, and the reader must see it masked in the hit; the hit carried \
         {:?}, masked {:?}",
        a.fields.keys().collect::<Vec<_>>(),
        a.masked
    );
    let hybrid = answer(
        store.as_ref(),
        &reader,
        hybrid_in("harbour quayside", "w", Some(vec![0.0, 1.0]), &["note"]),
        "the reader's hybrid search in `note` along `w`",
    )
    .await?;
    ensure!(
        keys(&hybrid).contains(&"a") && keys(&hybrid).contains(&"b"),
        "the reader's hybrid search in `note` along [0, 1] must find `a` and `b`; it found {:?}",
        keys(&hybrid)
    );
    Ok(())
}

/// `search` by `who` must be refused as `invalid`, naming `field`.
async fn invalid(
    store: &dyn LakeStore,
    who: &Principal,
    mode: SearchMode,
    field: Option<&str>,
    what: &str,
) -> Held {
    match doors(store).search(who, doc_search(mode)).await {
        Err(Error::Refused {
            field: f,
            reason: Refusal::Invalid(..),
        }) if f.as_ref().map(|f| f.as_str()) == field => Ok(()),
        Err(e) => Err(Failure::new(format!(
            "{what} must be refused as invalid, naming {field:?}; it failed with {e:?}"
        ))),
        Ok(r) => Err(Failure::new(format!(
            "{what} must be refused as invalid, naming {field:?}; it answered {:?}",
            keys(&r.data)
        ))),
    }
}

async fn naming_an_undeclared_unsearchable_or_unreadable_field_is_refused(store: Store) -> Held {
    accounts(store.as_ref()).await?;
    let s = store.as_ref();
    let own = owner(s);
    invalid(
        s,
        &own,
        lexical_in(&["label"]),
        Some("label"),
        "a search in `label`, which is not searchable,",
    )
    .await?;
    invalid(
        s,
        &own,
        lexical_in(&["nope"]),
        Some("nope"),
        "a search in `nope`, which `doc` does not declare,",
    )
    .await?;
    invalid(s, &own, lexical_in(&[]), None, "a search naming no field").await?;
    let reader = masking(s, "secret");
    refused(
        s,
        &reader,
        doc_search(lexical_in(&["note", "secret"])),
        "secret",
        "a search in `note` and `secret`, by a reader who sees `secret` only masked,",
    )
    .await?;
    refused(
        s,
        &masking(s, "label"),
        doc_search(lexical_in(&["label"])),
        "label",
        "a search in `label`, which is not searchable, by a reader who sees it only masked, \
         must not tell whether it is searchable: it",
    )
    .await
}

async fn a_hybrid_search_limited_to_named_fields_still_probes_what_its_vector_is_embedded_from(
    store: Store,
) -> Held {
    use crate::conformance::backing::EMBED_MODEL;

    let l = Lake::of(store.as_ref());
    let vec = embedding("vec", 2, &["secret"], EMBED_MODEL);
    let fields = vec![searchable("note"), field("secret", StorageKind::Text), vec];
    l.declare(type_decl("doc", fields)).await?;
    l.upsert(
        "doc",
        "a",
        json!({"note": "harbour", "secret": "the salary of the harbour master"}),
    )
    .await?;
    let reader = masking(store.as_ref(), "secret");
    let lexical = answer(
        store.as_ref(),
        &reader,
        SearchMode::Lexical {
            text: "harbour".into(),
            fields: named(&["note"]),
            text_fields: None,
        },
        "the reader's lexical search in `note`",
    )
    .await?;
    ensure!(
        keys(&lexical) == ["a"],
        "the reader's lexical search in `note` must find `a`; it found {:?}",
        keys(&lexical)
    );
    refused(
        store.as_ref(),
        &reader,
        doc_search(hybrid_in("harbour", "vec", Some(vec![1.0, 0.0]), &["note"])),
        "secret",
        "a hybrid search in `note` over `vec`, embedded from `secret`, by a reader who sees \
         `secret` only masked,",
    )
    .await
}

//! A search limited to named fields: it matches, ranks and probes in
//! those fields only, and the vector half of a hybrid search is not
//! narrowed (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! `docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).

use std::sync::Arc;

use lakelands_api::ontology::{FieldDecl, StorageKind};
use lakelands_api::query::{Hits, SearchMode};
use lakelands_api::{Error, LakeStore, Principal, Refusal};
use serde_json::json;

use super::search::{doc_search, keys, masking, refused};
use crate::conformance::Failure;
use crate::conformance::fixture::{Must, declare, doors, field, owner, type_decl, upsert};

check!(
    /// Named fields narrow the match and the ranking.
    FIELDS_NARROW_THE_MATCH = a_search_limited_to_named_fields_matches_and_ranks_in_those_only,
    "024 and 025: a lexical or hybrid search that names `fields` matches, scores and ties over those searchable fields only, the same on every engine; without `fields` it searches every searchable field"
);

check!(
    /// Named fields lift the probe of the fields not named.
    FIELDS_LIFT_THE_PROBE = a_search_limited_to_readable_fields_is_answered_beside_a_masked_one,
    "024: a search that names `fields` probes only those, so a searchable field the principal may not read no longer refuses it when it is not named; the field stays masked in every hit"
);

check!(
    /// Named fields refuse what they may not name.
    FIELDS_REFUSED = naming_an_undeclared_unsearchable_or_unreadable_field_is_refused,
    "024: a named field that is not declared or not searchable is invalid, one the principal may not read is not granted, judged before its searchability, and a search naming no field is invalid"
);

check!(
    /// Named fields leave the vector's own probe in place.
    FIELDS_KEEP_THE_EMBED_PROBE = a_hybrid_search_limited_to_named_fields_still_probes_what_its_vector_is_embedded_from,
    "024: `fields` narrows only the text half of a hybrid search; its vector still encodes every field it is embedded from, so a hybrid search over it is refused, naming the field, when the principal may not read one, whatever `fields` names",
    embedding
);

/// `doc` with two searchable texts, `note` and `secret`, a text that is
/// not searchable, `label`, and a plain vector `w`. For "harbour
/// quayside", `a` and `b` hold both words and `c` one over every
/// searchable field; `b` holds both in `note`, `a` one, `c` none.
async fn accounts(store: &dyn LakeStore) -> Result<(), Failure> {
    use lakelands_api::ontology::FieldType;
    let cat = doors(store);
    let own = owner(store);
    let searchable = |name| FieldDecl {
        searchable: true,
        ..field(name, StorageKind::Text)
    };
    let w = FieldDecl {
        ty: FieldType {
            dimensions: Some(2),
            ..FieldType::kind(StorageKind::Vector)
        },
        ..field("w", StorageKind::Text)
    };
    declare(
        cat.as_ref(),
        &own,
        type_decl(
            "doc",
            vec![
                searchable("note"),
                searchable("secret"),
                field("label", StorageKind::Text),
                w,
            ],
        ),
    )
    .await?;
    for (key, fields) in [
        (
            "a",
            json!({"note": "harbour", "secret": "quayside harbour", "w": [0.0, 1.0]}),
        ),
        ("b", json!({"note": "harbour quayside", "w": [1.0, 0.0]})),
        ("c", json!({"secret": "harbour", "label": "harbour"})),
    ] {
        upsert(cat.as_ref(), &own, "doc", key, fields).await?;
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
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    accounts(store.as_ref()).await?;
    let own = owner(store.as_ref());
    let every = answer(
        store.as_ref(),
        &own,
        SearchMode::Lexical {
            text: "harbour quayside".into(),
            fields: None,
        },
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
    let fallback = answer(
        store.as_ref(),
        &own,
        SearchMode::Hybrid {
            text: "harbour quayside".into(),
            field: "w".into(),
            vector: None,
            fields: named(&["note"]),
        },
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

async fn a_search_limited_to_readable_fields_is_answered_beside_a_masked_one(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    accounts(store.as_ref()).await?;
    let reader = masking(store.as_ref(), "secret");
    refused(
        store.as_ref(),
        &reader,
        doc_search(SearchMode::Lexical {
            text: "harbour quayside".into(),
            fields: None,
        }),
        "secret",
        "a search in every searchable field, by a reader who sees `secret` only masked,",
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
        SearchMode::Hybrid {
            text: "harbour quayside".into(),
            field: "w".into(),
            vector: Some(vec![0.0, 1.0]),
            fields: named(&["note"]),
        },
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
) -> Result<(), Failure> {
    match doors(store).search(who, doc_search(mode)).await {
        Err(Error::Refused {
            field: f,
            reason: Refusal::Invalid { .. },
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

async fn naming_an_undeclared_unsearchable_or_unreadable_field_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use crate::conformance::backing::EMBED_MODEL;
    use lakelands_api::ontology::{Authority, EmbedDecl, FieldType};
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let vec = FieldDecl {
        ty: FieldType {
            dimensions: Some(2),
            ..FieldType::kind(StorageKind::Vector)
        },
        authority: Authority::Derived,
        embed: Some(EmbedDecl {
            from: vec!["secret".into()],
            model: EMBED_MODEL.into(),
        }),
        ..field("vec", StorageKind::Text)
    };
    let note = FieldDecl {
        searchable: true,
        ..field("note", StorageKind::Text)
    };
    declare(
        cat.as_ref(),
        &own,
        type_decl("doc", vec![note, field("secret", StorageKind::Text), vec]),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &own,
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
        doc_search(SearchMode::Hybrid {
            text: "harbour".into(),
            field: "vec".into(),
            vector: Some(vec![1.0, 0.0]),
            fields: named(&["note"]),
        }),
        "secret",
        "a hybrid search in `note` over `vec`, embedded from `secret`, by a reader who sees \
         `secret` only masked,",
    )
    .await
}

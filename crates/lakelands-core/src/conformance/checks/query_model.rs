//! A query vector may name the model that made it, and over a field that is
//! a model's embedding it must be that model (lakelands#994; 025, extended,
//! and 006).

use lakelands_api::error::Case;
use lakelands_api::ontology::StorageKind;
use lakelands_api::query::{Axis, Search, SearchMode, VectorTerm};
use lakelands_api::{Access, Error, LakeStore, Principal, PrincipalKind, Refusal};
use serde_json::json;

use crate::conformance::backing::{self, EMBED_MODEL};
use crate::conformance::fixture::{
    Lake, Must, embedding, field, head_unmoved, principal, type_decl, vector,
};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    QUERY_MODEL_CHECKED = a_query_vector_of_another_model_than_its_fields_is_refused,
    "025, extended: a query vector the caller names a model for is refused before anything is ranked where the field is the embedding of another model, after its length is judged, for a vector search, a term of `also` and a hybrid search alike; a hybrid search that brings no vector and names a model is refused by its shape, calling no embedding backend; a pattern started from such a search is refused alike; the field's own model ranks exactly as naming none, every hit and score alike for a vector search and a hybrid search, and over a field that is no embedding the model is not judged but named in the axis",
    embedding
);

check!(
    QUERY_MODEL_AFTER_RIGHTS = a_query_models_refusal_names_no_model_to_one_not_shown_the_field,
    "025, extended (and 006): a caller kept from a field is refused a vector naming another model for that field, exactly as in a twin lake where the field is a plain vector, and, at `none`, as in one with no such field: the model is judged only after the caller is shown the field; a hybrid search naming a model without a vector is refused alike in all three, to every caller",
    given,
    embedding
);

/// The `title` of a `note`: text the search can read.
fn titled() -> lakelands_api::ontology::FieldDecl {
    let mut title = field("title", StorageKind::Text);
    title.searchable = true;
    title
}

/// `note`: `title`, searchable, `e` its embedding by the suite's model, and
/// `plain`, a vector field no model fills. Three records, which the text
/// ("crane heron" ranks n2 first) and `plain` rank apart (every vector of
/// `e` is the suite model's one direction).
async fn notes(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(type_decl(
        "note",
        vec![
            titled(),
            embedding("e", 2, &["title"], EMBED_MODEL),
            vector("plain", 2),
        ],
    ))
    .await?;
    filled(&l).await?;
    Ok(l)
}

async fn filled(l: &Lake) -> Held {
    for (key, title, plain) in [
        ("n1", "crane", [0.0, 1.0]),
        ("n2", "crane heron", [1.0, 0.0]),
        ("n3", "heron", [0.6, 0.8]),
    ] {
        l.upsert("note", key, json!({"title": title, "plain": plain}))
            .await?;
    }
    Ok(())
}

fn search(mode: SearchMode) -> Search {
    serde_json::from_value(json!({"type": "note", "mode": "lexical", "text": "x"}))
        .map(|mut s: Search| {
            s.mode = mode;
            s
        })
        .expect("a search reads")
}

fn vector_of(field: &str, v: Vec<f32>, model: Option<&str>) -> SearchMode {
    let mut m = SearchMode::vector(field, v);
    if let SearchMode::Vector { model: named, .. } = &mut m {
        *named = model.map(str::to_owned);
    }
    m
}

fn term(field: &str, model: Option<&str>) -> VectorTerm {
    VectorTerm {
        field: field.into(),
        vector: vec![1.0, 0.0],
        weight: None,
        model: model.map(str::to_owned),
    }
}

fn plain_term(v: Vec<f32>) -> VectorTerm {
    VectorTerm {
        field: "plain".into(),
        vector: v,
        weight: None,
        model: None,
    }
}

fn hybrid_of(text: &str, model: Option<&str>) -> SearchMode {
    let mut m = SearchMode::hybrid(text, "e");
    if let SearchMode::Hybrid {
        vector,
        model: named,
        ..
    } = &mut m
    {
        *vector = Some(vec![1.0, 0.0]);
        *named = model.map(str::to_owned);
    }
    m
}

fn with_also(mut m: SearchMode, also: Vec<VectorTerm>) -> SearchMode {
    if let SearchMode::Vector { also: a, .. } = &mut m {
        *a = also;
    }
    m
}

fn case_of(e: &Result<impl std::fmt::Debug, Error>) -> Option<Case> {
    e.as_ref().err().and_then(|e| Some(e.case()?.case.clone()))
}

async fn a_query_vector_of_another_model_than_its_fields_is_refused(store: Store) -> Held {
    let l = notes(store.as_ref()).await?;
    let own = &l.own;
    let other = Case::QueryModel {
        query: "other".into(),
        held: EMBED_MODEL.into(),
    };
    let before = store.head().await.must("reading the head")?;
    for (what, mode) in [
        (
            "a vector search",
            vector_of("e", vec![1.0, 0.0], Some("other")),
        ),
        (
            "a term of `also`",
            with_also(
                vector_of("plain", vec![0.0, 1.0], None),
                vec![term("e", Some("other"))],
            ),
        ),
        (
            "a hybrid search's vector",
            hybrid_of("crane", Some("other")),
        ),
    ] {
        let answer = l.cat.search(own, search(mode)).await;
        ensure!(
            case_of(&answer) == Some(other.clone()),
            "{what} over `e`, the embedding of `{EMBED_MODEL}`, with a vector said to be of \
             `other` is refused as `query_model`, naming both; it answered {answer:?}"
        );
    }
    let long = l.cat.search(
        own,
        search(vector_of("e", vec![1.0, 0.0, 0.0], Some("other"))),
    );
    let long = long.await;
    ensure!(
        case_of(&long) == Some(Case::QueryDimensions { query: 3, held: 2 }),
        "a vector of 3 dimensions said to be of `other` is refused for its length first; it \
         answered {long:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused search").await?;

    // A hybrid search that brings no vector has no vector to name a model
    // for: refused by its shape, before the backend is asked.
    let calls = backing::embed_calls();
    let mut alone = SearchMode::hybrid("crane", "e");
    if let SearchMode::Hybrid { model, .. } = &mut alone {
        *model = Some(EMBED_MODEL.into());
    }
    let answer = l.cat.search(own, search(alone)).await;
    let on_e = matches!(
        answer.as_ref().err().map(Error::unwrapped),
        Some(Error::Refused { field: Some(f), .. }) if f.as_str() == "e"
    );
    ensure!(
        case_of(&answer) == Some(Case::ModelWithoutVector)
            && on_e
            && backing::embed_calls() == calls,
        "a hybrid search over `e` naming a model and bringing no vector is refused as \
         `model_without_vector` on `e`, calling no backend; it answered {answer:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused search").await?;

    // A pattern that starts from a search judges its mode alike.
    let mut alone = SearchMode::hybrid("crane", "e");
    if let SearchMode::Hybrid { model, .. } = &mut alone {
        *model = Some(EMBED_MODEL.into());
    }
    for (mode, want) in [
        (vector_of("e", vec![1.0, 0.0], Some("other")), other.clone()),
        (alone, Case::ModelWithoutVector),
    ] {
        let started = json!({"start": {"type": "note", "search": mode}});
        let pattern = serde_json::from_value(started).expect("a pattern reads");
        let answer = l.cat.query(own, pattern).await;
        ensure!(
            case_of(&answer) == Some(want.clone()),
            "a pattern started from {mode:?} is refused as a search is, `{want:?}`; it \
             answered {answer:?}"
        );
    }
    head_unmoved(store.as_ref(), before, "a refused pattern").await?;

    // The field's own model ranks exactly as none named: every hit and its
    // score, and the axis names the model as it was named.
    let on = |axes: &[Axis], f: &str| {
        let a = axes
            .iter()
            .find(|a| a.field.as_ref().is_some_and(|n| n.as_str() == f));
        a.and_then(|a| a.model.clone())
    };
    let fused = |model| {
        let m = vector_of("e", vec![1.0, 0.0], model);
        with_also(m, vec![plain_term(vec![0.6, 0.8])])
    };
    for (what, none, named) in [
        ("a vector search", fused(None), fused(Some(EMBED_MODEL))),
        (
            "a hybrid search",
            hybrid_of("crane heron", None),
            hybrid_of("crane heron", Some(EMBED_MODEL)),
        ),
    ] {
        let bare = l.cat.search(own, search(none)).await;
        let bare = bare.must("a search naming no model")?.data;
        let named = l.cat.search(own, search(named)).await;
        let named = named.must("a search naming the field's model")?.data;
        ensure!(
            bare.hits.len() >= 2
                && bare.hits == named.hits
                && bare.ranked_by == named.ranked_by
                && on(&named.axes, "e").as_deref() == Some(EMBED_MODEL)
                && on(&bare.axes, "e").is_none(),
            "{what} naming the field's own model ranks every hit as naming none, at the same \
             scores, and its axis over `e` names the model as it was named; answered {named:?} \
             and {bare:?}"
        );
    }
    // Over a field no model fills, the model is not judged, and is named.
    let fused = with_also(
        vector_of("plain", vec![0.0, 1.0], Some("host")),
        vec![term("e", Some(EMBED_MODEL))],
    );
    let answer = l
        .cat
        .search(own, search(fused))
        .await
        .must("a search naming `host`")?;
    ensure!(
        on(&answer.data.axes, "plain").as_deref() == Some("host")
            && on(&answer.data.axes, "e").as_deref() == Some(EMBED_MODEL),
        "`plain` is no embedding, so a vector over it said to be of `host` is ranked and its \
         axis names `host`; the axes are {:?}",
        answer.data.axes
    );
    Ok(())
}

async fn a_query_models_refusal_names_no_model_to_one_not_shown_the_field(
    store: Store,
    given: Given,
) -> Held {
    // A: `e` the embedding of the suite's model. B, its twin: `e` a plain
    // vector, which no model is judged against. C: no `e` at all.
    let a = notes(store.as_ref()).await?;
    let mut lakes = vec![store.clone()];
    for e in [Some(vector("e", 2)), None] {
        let twin = (given.twin)(store.lake().clone(), Default::default()).await;
        let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
        let t = Lake::of(twin.as_ref());
        let fields = std::iter::once(titled())
            .chain(e)
            .chain([vector("plain", 2)]);
        t.declare(type_decl("note", fields.collect())).await?;
        filled(&t).await?;
        if lakes.len() == 1 {
            t.upsert("note", "n1", json!({"e": [1.0, 0.0]})).await?;
        }
        lakes.push(twin);
    }
    let mut alone = SearchMode::hybrid("crane", "e");
    if let SearchMode::Hybrid { model, .. } = &mut alone {
        *model = Some("other".into());
    }
    let asked = [
        vector_of("e", vec![1.0, 0.0], Some("other")),
        vector_of("e", vec![1.0, 0.0], Some(EMBED_MODEL)),
        with_also(
            vector_of("plain", vec![0.0, 1.0], None),
            vec![term("e", Some("other"))],
        ),
        hybrid_of("crane", Some("other")),
        alone,
    ];
    let reader = |store: &Store, access: Option<Access>| -> Principal {
        let Some(access) = access else {
            return a.own.clone();
        };
        let grants = json!({"default": "read", "types": {"note": {"fields": {"e": access}}}});
        let grants = serde_json::from_value(grants).expect("grants read");
        let id = format!("{access:?}");
        principal(store.as_ref(), PrincipalKind::Agent, &id, grants)
    };
    let answers = async |access: Option<Access>, mode: &SearchMode| {
        let mut out = Vec::new();
        for store in &lakes {
            let l = Lake::of(store.as_ref());
            let who = reader(store, access);
            out.push(l.cat.search(&who, search(mode.clone())).await);
        }
        out
    };
    let json = |a: &Result<_, Error>| serde_json::to_value(a.as_ref().err()).ok();
    // A caller kept from `e` is answered as in the twins: at `none`, as
    // where `e` is a plain vector and where there is none; masked, as where
    // it is a plain vector. Nothing it is told names a model.
    // The refusal is the field's own: unknown at `none`, not granted
    // when masked, so the row reaches the rights and not an earlier guard.
    let unknown = Case::UnknownField {
        ty: "note".into(),
        field: "e".into(),
    };
    for (access, alike) in [(Access::None, 3), (Access::Masked, 2)] {
        for mode in &asked[..4] {
            let got = answers(Some(access), mode).await;
            let seen = got[..alike].iter().map(json).collect::<Vec<_>>();
            let rights = match access {
                Access::None => case_of(&got[0]) == Some(unknown.clone()),
                _ => matches!(
                    got[0].as_ref().err().map(Error::unwrapped),
                    Some(Error::Refused { field: Some(f), reason: Refusal::NotGranted })
                        if f.as_str() == "e"
                ),
            };
            ensure!(
                rights && seen.iter().all(|j| *j == seen[0]),
                "a reader that sees `e` as {access:?} is refused {mode:?} for `e` as in the \
                 first {alike} of the lakes where `e` is an embedding, a plain vector and \
                 absent, and is told no model; it answered {got:?}"
            );
        }
    }
    // The control: the owner of the lake where `e` is an embedding is
    // refused `other`, and of the one where it is a plain vector ranks it.
    let got = answers(None, &asked[0]).await;
    ensure!(
        case_of(&got[0])
            == Some(Case::QueryModel {
                query: "other".into(),
                held: EMBED_MODEL.into()
            })
            && got[1].as_ref().is_ok_and(|h| !h.data.hits.is_empty()),
        "the owner is refused a vector of `other` over the embedding `e`, and is answered \
         its hits over the plain vector `e`; it answered {got:?}"
    );
    // A model without a vector is refused by the search's shape, so every
    // caller is answered alike in all three lakes.
    for access in [None, Some(Access::None), Some(Access::Masked)] {
        let got = answers(access, &asked[4]).await;
        let seen = got.iter().map(json).collect::<Vec<_>>();
        ensure!(
            case_of(&got[0]) == Some(Case::ModelWithoutVector)
                && seen.iter().all(|j| *j == seen[0]),
            "a hybrid search naming a model and bringing no vector is refused as \
             `model_without_vector` alike in the three lakes, to the reader at {access:?}; it \
             answered {got:?}"
        );
    }
    Ok(())
}

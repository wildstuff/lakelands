//! Fields declared as the embedding of text fields of their record
//! (`docs/decisions/022-functions-are-declarations.md`, extended;
//! `docs/log-format.md` §2.5; `docs/seams.md` §EmbedBackend).
//!
//! The store embeds a record's text through the bound backend after the
//! write, records each vector as `output_recorded` under `embed:<model>`,
//! and fills the field from the record, never from a second call. The
//! checks use the suite's embedding backend
//! ([`crate::conformance::backing::embedder`]), bound in the lakes of the
//! checks that ask for it. It makes every text `(1, 0)` and counts how often
//! each text was sent; each check uses texts of its own.

use std::sync::Arc;

use lakelands_api::event::{Event, EventBody};
use lakelands_api::ontology::{Authority, EmbedDecl, FieldDecl, FieldType, StorageKind};
use lakelands_api::query::{Search, SearchMode};
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::Declaration;
use lakelands_api::{Error, LakeStore, Position};
use serde_json::{Value, json};

use crate::conformance::backing::{EMBED_MODEL, embedded, plug_text, unplug_text};
use crate::conformance::fixture::{
    Must, declare, doors, field, ingest, owner, read, source_record, type_decl, upsert, value,
};
use crate::conformance::{Failure, Reopen, backing};

check!(
    /// A write embeds its text and records the vector.
    EMBED_WRITE = a_write_embeds_its_text_and_records_the_vector,
    "022, extended, and the log format: the store embeds the joined `from` text after the write, records the vector as output_recorded under embed:<model> naming the field, and the field holds it",
    embedding
);

check!(
    /// One text, one call.
    EMBED_ONCE = the_same_text_is_embedded_once_whichever_record_asks,
    "002 and 022, extended: a vector recorded for a text under a model is read back for every later record with that text, and after a record's text changed and came back; the log holds one output per distinct text",
    embedding
);

check!(
    /// Rebuild reads vectors back.
    EMBED_REBUILD = a_rebuild_reads_the_vectors_back_without_embedding,
    "001 and 002: a rebuild folds the recorded vectors; the fields read as before and nothing is embedded",
    embedding
);

check!(
    /// Another model is unbound.
    EMBED_OTHER_MODEL = an_embedding_whose_model_the_backend_does_not_make_is_refused_at_declare,
    "022, extended: an embedding is declared only where the bound backend names its model and makes vectors of its dimensions; otherwise the declaration is refused as unbound",
    embedding
);

check!(
    /// No backend is unbound.
    EMBED_NO_BACKEND = an_embedding_without_a_backend_is_refused_at_declare,
    "022, extended: with no embedding backend bound, declaring an embedding is refused as unbound, naming the model"
);

check!(
    /// A changed embedding is due everywhere.
    EMBED_REDECLARED = a_changed_embedding_is_due_on_every_live_record_and_an_unchanged_one_embeds_nothing_new,
    "022, extended: a change of `from` or `model` is a redeclaration that makes the field due on every live record, one call per distinct text; declaring it again unchanged reads the recorded vectors back",
    embedding
);

check!(
    /// Nothing to embed.
    EMBED_NO_TEXT = a_record_with_nothing_to_embed_holds_no_vector_and_embeds_nothing,
    "022, extended: with no `from` field holding text, the field holds no value and keeps metadata without an error, and nothing is called; the text written later makes it due",
    embedding
);

check!(
    /// A failure clears, and a redeclaration retries it.
    EMBED_FAILURE_RETRIED = a_failed_embedding_clears_the_field_and_a_redeclaration_retries_it,
    "022, extended: a failure is recorded, clears the field and puts its reason in the metadata; a redeclaration calls again for that text once, the later output holds and the vector fills every record holding it",
    embedding
);

check!(
    /// A vector's fill reacts.
    EMBED_FILL_REACTS = an_embedded_vector_recomputes_the_derived_fields_that_read_it,
    "012 and 022, extended: the field a vector fills is changed through the fold's ordinary path, so a derived field reading it is recomputed",
    embedding
);

/// The embedding field of `note`: `vec`, made from `title` and `body`
/// with `model`, of `dimensions`.
fn vec_field(from: &[&str], model: &str, dimensions: u32) -> FieldDecl {
    FieldDecl {
        ty: FieldType {
            dimensions: Some(dimensions),
            ..FieldType::kind(StorageKind::Vector)
        },
        authority: Authority::Derived,
        embed: Some(EmbedDecl {
            from: from.iter().map(|f| (*f).into()).collect(),
            model: model.into(),
        }),
        ..field("vec", StorageKind::Text)
    }
}

fn notes_with(vec: FieldDecl, extra: Vec<FieldDecl>) -> lakelands_api::ontology::TypeDecl {
    let mut fields = vec![
        field("title", StorageKind::Text),
        field("body", StorageKind::Text),
        vec,
    ];
    fields.extend(extra);
    type_decl("note", fields)
}

/// Declare `note`, whose `vec` embeds `title` and `body`.
async fn notes(store: &dyn LakeStore) -> Result<(), Failure> {
    let cat = doors(store);
    declare(
        cat.as_ref(),
        &owner(store),
        notes_with(vec_field(&["title", "body"], EMBED_MODEL, 2), vec![]),
    )
    .await?;
    Ok(())
}

/// Every `output_recorded` in the log.
async fn outputs(store: &dyn LakeStore) -> Result<Vec<Event>, Failure> {
    Ok(store
        .feed()
        .poll(
            Position::GENESIS,
            &FeedFilter {
                kinds: vec!["output_recorded".into()],
                types: vec![],
                keys: Vec::new(),
            },
            u32::MAX,
        )
        .await
        .must("reading the recorded outputs from the feed")?
        .events)
}

async fn vec_of(store: &dyn LakeStore, key: &str) -> Result<Option<Value>, Failure> {
    let r = read(doors(store).as_ref(), &owner(store), "note", key).await?;
    Ok(value(&r, "vec").cloned())
}

/// Whether a value is the vector (1, 0), however its numbers are written.
fn is_east(v: &Option<Value>) -> bool {
    v.as_ref()
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().map(Value::as_f64).collect::<Vec<_>>() == [Some(1.0), Some(0.0)])
}

async fn a_write_embeds_its_text_and_records_the_vector(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    notes(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    upsert(
        cat.as_ref(),
        &own,
        "note",
        "n1",
        json!({"title": "Write harbour", "body": "A write crane."}),
    )
    .await?;
    let text = "Write harbour\n\nA write crane.";
    let got = vec_of(store.as_ref(), "n1").await?;
    ensure!(
        is_east(&got) && embedded(text) == 1,
        "`vec` embeds `title` and `body` joined with one blank line; after writing `n1` \
         the text {text:?} must have been embedded once ({} times) and `vec` read (1, 0), \
         but it reads {got:?}",
        embedded(text)
    );
    let recorded = outputs(store.as_ref()).await?;
    let [event] = recorded.as_slice() else {
        return Err(Failure::new(format!(
            "one text was embedded, so the log holds one `output_recorded`; it holds {}",
            recorded.len()
        )));
    };
    let EventBody::OutputRecorded {
        seam, field: named, ..
    } = &event.body
    else {
        return Err(Failure::new(
            "the feed answered another kind for `output_recorded`",
        ));
    };
    let want = format!("embed:{EMBED_MODEL}");
    ensure!(
        seam == &want
            && named.as_ref().is_some_and(|(t, k, f)| t.as_str() == "note"
                && k.as_str() == "n1"
                && f.as_str() == "vec")
            && event.procedure.as_ref().map(|p| p.name.as_str()) == Some(EMBED_MODEL),
        "the vector is recorded under the seam {want:?}, naming the field `note` `n1` `vec`, \
         with the model as its procedure; the log holds seam {seam:?}, field {named:?}, \
         procedure {:?}",
        event.procedure
    );
    Ok(())
}

async fn the_same_text_is_embedded_once_whichever_record_asks(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    notes(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let text = "Once harbour";
    for key in ["n1", "n2"] {
        upsert(cat.as_ref(), &own, "note", key, json!({"title": text})).await?;
    }
    upsert(
        cat.as_ref(),
        &own,
        "note",
        "n1",
        json!({"title": "Once elsewhere"}),
    )
    .await?;
    upsert(cat.as_ref(), &own, "note", "n1", json!({"title": text})).await?;
    let (n1, n2) = (
        vec_of(store.as_ref(), "n1").await?,
        vec_of(store.as_ref(), "n2").await?,
    );
    ensure!(
        embedded(text) == 1 && is_east(&n1) && is_east(&n2),
        "{text:?} was written to `n1`, to `n2`, and to `n1` again after another text: it \
         must have been embedded once, not {} times, and both records hold its vector \
         (they hold {n1:?} and {n2:?})",
        embedded(text)
    );
    let recorded = outputs(store.as_ref()).await?.len();
    ensure!(
        recorded == 2,
        "two distinct texts were embedded, so the log holds two `output_recorded`; a \
         record written with a text whose vector is recorded is filled in its own write. \
         It holds {recorded}"
    );
    Ok(())
}

async fn a_rebuild_reads_the_vectors_back_without_embedding(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    notes(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let text = "Rebuilt harbour";
    upsert(cat.as_ref(), &own, "note", "n1", json!({"title": text})).await?;
    let calls = embedded(text);
    store.rebuild().await.must("rebuilding the lake")?;
    let got = vec_of(store.as_ref(), "n1").await?;
    ensure!(
        is_east(&got) && embedded(text) == calls,
        "after a rebuild `vec` must read the recorded (1, 0) with no new call; it reads \
         {got:?}, and the text was sent {} more times",
        embedded(text) - calls
    );
    Ok(())
}

fn is_unbound<T>(r: &lakelands_api::Result<T>) -> bool {
    matches!(r, Err(Error::Unbound { .. }))
}

/// What a declaration answered, for a failure message.
fn answered<T>(r: &lakelands_api::Result<T>) -> String {
    match r {
        Ok(_) => "it was accepted".into(),
        Err(e) => format!("\"{e}\""),
    }
}

async fn an_embedding_whose_model_the_backend_does_not_make_is_refused_at_declare(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let other_model = cat
        .declare(
            &own,
            Declaration::Type(notes_with(
                vec_field(&["title"], "another-model", 2),
                vec![],
            )),
        )
        .await;
    let other_dimensions = cat
        .declare(
            &own,
            Declaration::Type(notes_with(vec_field(&["title"], EMBED_MODEL, 3), vec![])),
        )
        .await;
    ensure!(
        is_unbound(&other_model) && is_unbound(&other_dimensions),
        "the bound backend makes `{EMBED_MODEL}` vectors of 2 dimensions; declaring an \
         embedding with another model, or with 3 dimensions, must be refused as unbound; \
         they answered {} and {}",
        answered(&other_model),
        answered(&other_dimensions)
    );
    Ok(())
}

async fn an_embedding_without_a_backend_is_refused_at_declare(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let got = cat
        .declare(
            &owner(store.as_ref()),
            Declaration::Type(notes_with(vec_field(&["title"], EMBED_MODEL, 2), vec![])),
        )
        .await;
    ensure!(
        matches!(&got, Err(Error::Unbound { seam }) if seam.contains(EMBED_MODEL)),
        "with no embedding backend bound, declaring an embedding must be refused as \
         unbound, naming the model `{EMBED_MODEL}`; it answered {}",
        answered(&got)
    );
    Ok(())
}

async fn a_changed_embedding_is_due_on_every_live_record_and_an_unchanged_one_embeds_nothing_new(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    notes(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let records = [
        ("n1", "Redeclared north", "Redeclared shared body"),
        ("n2", "Redeclared south", "Redeclared shared body"),
        ("n3", "Redeclared west", "Redeclared own body"),
    ];
    for (key, title, body) in records {
        upsert(
            cat.as_ref(),
            &own,
            "note",
            key,
            json!({"title": title, "body": body}),
        )
        .await?;
    }
    // Now made from `body` alone: two distinct texts for three records.
    declare(
        cat.as_ref(),
        &own,
        notes_with(vec_field(&["body"], EMBED_MODEL, 2), vec![]),
    )
    .await?;
    let (shared, own_body) = (
        embedded("Redeclared shared body"),
        embedded("Redeclared own body"),
    );
    let mut filled = Vec::new();
    for (key, _, _) in records {
        filled.push(vec_of(store.as_ref(), key).await?);
    }
    ensure!(
        shared == 1 && own_body == 1 && filled.iter().all(is_east),
        "after `vec` was declared again from `body` alone, every live record is due: the \
         two distinct bodies must each be embedded once (they were {shared} and \
         {own_body} times) and every record hold a vector ({filled:?})"
    );
    declare(
        cat.as_ref(),
        &own,
        notes_with(vec_field(&["body"], EMBED_MODEL, 2), vec![]),
    )
    .await?;
    ensure!(
        embedded("Redeclared shared body") == 1 && embedded("Redeclared own body") == 1,
        "declaring `vec` again unchanged reads the recorded vectors back; the bodies were \
         embedded {} and {} times",
        embedded("Redeclared shared body"),
        embedded("Redeclared own body")
    );
    Ok(())
}

async fn a_record_with_nothing_to_embed_holds_no_vector_and_embeds_nothing(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    notes(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    upsert(cat.as_ref(), &own, "note", "n1", json!({"title": ""})).await?;
    let r = read(cat.as_ref(), &own, "note", "n1").await?;
    let meta = r.meta.get("vec");
    ensure!(
        value(&r, "vec").is_none()
            && meta.is_some_and(|m| m.error.is_none())
            && outputs(store.as_ref()).await?.is_empty(),
        "`n1` has no text to embed: `vec` holds no value, keeps metadata without an error, \
         and nothing is recorded; it reads {:?} with metadata {meta:?}",
        value(&r, "vec")
    );
    let text = "Later harbour";
    upsert(cat.as_ref(), &own, "note", "n1", json!({"title": text})).await?;
    let got = vec_of(store.as_ref(), "n1").await?;
    ensure!(
        is_east(&got) && embedded(text) == 1,
        "writing a title later makes `vec` due: {text:?} must be embedded once ({} times) \
         and `vec` read (1, 0); it reads {got:?}",
        embedded(text)
    );
    Ok(())
}

async fn a_failed_embedding_clears_the_field_and_a_redeclaration_retries_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    notes(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let text = "Retried harbour";
    unplug_text(text);
    upsert(cat.as_ref(), &own, "note", "n1", json!({"title": text})).await?;
    // A second record with the same text reads the recorded failure.
    upsert(cat.as_ref(), &own, "note", "n2", json!({"title": text})).await?;
    let r = read(cat.as_ref(), &own, "note", "n1").await?;
    let meta = r.meta.get("vec");
    ensure!(
        value(&r, "vec").is_none()
            && meta.is_some_and(|m| m.error.is_some())
            && embedded(text) == 1,
        "the backend failed on {text:?}: `vec` holds no value and its metadata carries the \
         reason; it reads {:?} with metadata {meta:?}, after {} calls",
        value(&r, "vec"),
        embedded(text)
    );
    // A later write of another field does not retry it.
    upsert(cat.as_ref(), &own, "note", "n1", json!({"body": ""})).await?;
    let before = embedded(text);
    plug_text(text);
    declare(
        cat.as_ref(),
        &own,
        notes_with(vec_field(&["title", "body"], EMBED_MODEL, 2), vec![]),
    )
    .await?;
    let (n1, n2) = (
        vec_of(store.as_ref(), "n1").await?,
        vec_of(store.as_ref(), "n2").await?,
    );
    ensure!(
        before == 1 && embedded(text) == 2 && is_east(&n1) && is_east(&n2),
        "declaring `note` again retries the recorded failure: {text:?} must be sent once \
         more for both records that hold it (twice in all, not {}), and the later output \
         holds, so `vec` reads (1, 0) on both; they read {n1:?} and {n2:?}",
        embedded(text)
    );
    Ok(())
}

async fn an_embedded_vector_recomputes_the_derived_fields_that_read_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let has = FieldDecl {
        authority: Authority::Derived,
        derived: Some("vec is not null".into()),
        depends_on: vec!["vec".into()],
        ..field("embedded", StorageKind::Boolean)
    };
    declare(
        cat.as_ref(),
        &own,
        notes_with(vec_field(&["title"], EMBED_MODEL, 2), vec![has]),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &own,
        "note",
        "n1",
        json!({"title": "Reacting harbour"}),
    )
    .await?;
    let r = read(cat.as_ref(), &own, "note", "n1").await?;
    ensure!(
        value(&r, "embedded") == Some(&json!(true)),
        "`embedded` is `vec is not null`; the vector's fill is a change of `vec`, so it \
         must read true, but reads {:?}",
        value(&r, "embedded")
    );
    Ok(())
}

fn with_embedder(embed: Option<Arc<dyn lakelands_api::seam::EmbedBackend>>) -> Seams {
    Seams {
        backings: backing::backings(),
        embed,
        ..Seams::default()
    }
}

fn reopen_failed(what: &str, e: Error) -> Failure {
    Failure::new(format!("opening the lake again {what} failed: {e} ({e:?})"))
}

check!(
    /// A hybrid search through a backend of another model is unbound.
    EMBED_SEARCH_OTHER_BACKEND = a_hybrid_search_over_an_embedding_the_bound_backend_does_not_make_is_unbound,
    "seams, EmbedBackend: a hybrid search over an embed field whose model or dimensions are not the bound backend's is refused as unbound when it is made, not ranked with vectors of another space",
    reopens
);

async fn a_hybrid_search_over_an_embedding_the_bound_backend_does_not_make_is_unbound(
    store: Arc<dyn LakeStore>,
    reopen: Reopen,
) -> Result<(), Failure> {
    let mut store = reopen(store, with_embedder(Some(backing::embedder())))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend", e))?;
    notes(store.as_ref()).await?;
    upsert(
        doors(store.as_ref()).as_ref(),
        &owner(store.as_ref()),
        "note",
        "n1",
        json!({"title": "Quayside crane"}),
    )
    .await?;
    for (model, dimensions) in [("another-model", 2), (EMBED_MODEL, 3)] {
        store = reopen(
            store,
            with_embedder(Some(backing::embedder_as(model, dimensions))),
        )
        .await
        .map_err(|e| reopen_failed(&format!("with a backend of `{model}`/{dimensions}"), e))?;
        let got = doors(store.as_ref())
            .search(
                &owner(store.as_ref()),
                Search {
                    ty: "note".into(),
                    mode: SearchMode::Hybrid {
                        text: "quayside".into(),
                        field: "vec".into(),
                        vector: None,
                    },
                    k: 10,
                    cursor: None,
                    filter: None,
                    with_meta: false,
                    at: lakelands_api::ReadAt::default(),
                },
            )
            .await;
        // The refusal names both: what the field was declared with, and
        // what is bound.
        let names_both = |seam: &str| {
            seam.contains(&format!("`{EMBED_MODEL}` with 2"))
                && seam.contains(&format!("`{model}` with {dimensions}"))
        };
        ensure!(
            matches!(&got, Err(Error::Unbound { seam }) if names_both(seam)),
            "`vec` holds `{EMBED_MODEL}` vectors of 2 dimensions; with a backend of \
             `{model}` and {dimensions} dimensions bound, a hybrid search over it must be \
             refused as unbound, naming both; it answered {}",
            answered(&got)
        );
    }
    Ok(())
}

check!(
    /// A due embedding waits while no backend makes it.
    EMBED_WAITS = a_due_embedding_waits_while_no_backend_makes_it,
    "seams, EmbedBackend: a due embedding whose backend is gone or makes another model or other dimensions is not made; the field waits, with neither a value nor metadata, until its record is written again or its type is declared again with the backend bound",
    reopens
);

async fn a_due_embedding_waits_while_no_backend_makes_it(
    store: Arc<dyn LakeStore>,
    reopen: Reopen,
) -> Result<(), Failure> {
    let mut store = reopen(store, with_embedder(Some(backing::embedder())))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend", e))?;
    notes(store.as_ref()).await?;
    for (key, embed, what) in [
        ("gone", None, "with no embedding backend"),
        (
            "other",
            Some(backing::embedder_as("another-model", 2)),
            "with a backend of another model",
        ),
        (
            "wider",
            Some(backing::embedder_as(EMBED_MODEL, 3)),
            "with a backend of other dimensions",
        ),
    ] {
        store = reopen(store, with_embedder(embed))
            .await
            .map_err(|e| reopen_failed(what, e))?;
        let text = format!("Waiting {key} bollard");
        upsert(
            doors(store.as_ref()).as_ref(),
            &owner(store.as_ref()),
            "note",
            key,
            json!({"title": text}),
        )
        .await?;
        let r = read(
            doors(store.as_ref()).as_ref(),
            &owner(store.as_ref()),
            "note",
            key,
        )
        .await?;
        ensure!(
            value(&r, "vec").is_none() && !r.meta.contains_key("vec") && embedded(&text) == 0,
            "`{key}` was written {what} bound: its `vec` must wait, with neither a value \
             nor metadata, and its text sent nowhere; it reads {:?} with metadata {:?}, \
             and the text was sent {} times",
            value(&r, "vec"),
            r.meta.get("vec"),
            embedded(&text)
        );
    }
    // The backend is back. Opening the lake calls nothing, and neither
    // does a write to another record: a waiting field is made when its
    // record is written again or its type is declared again.
    store = reopen(store, with_embedder(Some(backing::embedder())))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend again", e))?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    upsert(
        cat.as_ref(),
        &own,
        "note",
        "later",
        json!({"title": "Later quay"}),
    )
    .await?;
    let still = vec_of(store.as_ref(), "gone").await?;
    ensure!(
        still.is_none(),
        "opening the lake and writing another record call nothing for `gone`, whose \
         `vec` waits; it reads {still:?}"
    );
    upsert(
        cat.as_ref(),
        &own,
        "note",
        "gone",
        json!({"body": "touched"}),
    )
    .await?;
    let touched = vec_of(store.as_ref(), "gone").await?;
    ensure!(
        is_east(&touched),
        "`gone` was written again with its backend bound, so its waiting `vec` is made; \
         it reads {touched:?}"
    );
    notes(store.as_ref()).await?;
    let (other, wider) = (
        vec_of(store.as_ref(), "other").await?,
        vec_of(store.as_ref(), "wider").await?,
    );
    ensure!(
        is_east(&other) && is_east(&wider),
        "declaring `note` again with its backend bound makes every waiting `vec`; `other` \
         and `wider` read {other:?} and {wider:?}"
    );
    Ok(())
}

check!(
    /// An embedding is refused unless it is the one shape it can be.
    EMBED_RULES = an_embedding_that_is_not_a_derived_vector_of_its_own_text_fields_is_refused,
    "seams, EmbedBackend, and TypeDecl::embedding_problem: an embed field is a derived vector with dimensions, made from distinct text fields of its type other than itself, sensitive where a source is; anything else is refused as invalid, whatever backend is bound",
    embedding
);

async fn an_embedding_that_is_not_a_derived_vector_of_its_own_text_fields_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let good = || vec_field(&["title"], EMBED_MODEL, 2);
    let number = field("pages", StorageKind::Number);
    let cases: Vec<(&str, FieldDecl, Vec<FieldDecl>)> = vec![
        (
            "a text field",
            FieldDecl {
                ty: FieldType::kind(StorageKind::Text),
                ..good()
            },
            vec![],
        ),
        (
            "a vector without dimensions",
            FieldDecl {
                ty: FieldType::kind(StorageKind::Vector),
                ..good()
            },
            vec![],
        ),
        (
            "an authored field",
            FieldDecl {
                authority: Authority::Authored,
                ..good()
            },
            vec![],
        ),
        (
            "an embedding with a derivation too",
            FieldDecl {
                derived: Some("title".into()),
                ..good()
            },
            vec![],
        ),
        (
            "an embedding with its own depends_on",
            FieldDecl {
                depends_on: vec!["title".into()],
                ..good()
            },
            vec![],
        ),
        ("made from nothing", vec_field(&[], EMBED_MODEL, 2), vec![]),
        (
            "made from itself",
            vec_field(&["title", "vec"], EMBED_MODEL, 2),
            vec![],
        ),
        (
            "made from a field twice",
            vec_field(&["title", "title"], EMBED_MODEL, 2),
            vec![],
        ),
        (
            "made from a field it lacks",
            vec_field(&["summary"], EMBED_MODEL, 2),
            vec![],
        ),
        (
            "made from a number",
            vec_field(&["pages"], EMBED_MODEL, 2),
            vec![number.clone()],
        ),
        (
            "made from a sensitive field and not sensitive itself",
            vec_field(&["secret"], EMBED_MODEL, 2),
            vec![FieldDecl {
                sensitive: true,
                ..field("secret", StorageKind::Text)
            }],
        ),
    ];
    let before = store.head().await.must("reading the head")?;
    for (what, vec, extra) in cases {
        let got = cat
            .declare(&own, Declaration::Type(notes_with(vec, extra)))
            .await;
        ensure!(
            matches!(
                &got,
                Err(Error::Refused {
                    field: Some(f),
                    reason: lakelands_api::Refusal::Invalid { .. },
                }) if f.as_str() == "vec"
            ),
            "declaring `vec` as {what} must be refused as invalid, naming `vec`; it answered {}",
            answered(&got)
        );
    }
    let after = store.head().await.must("reading the head")?;
    ensure!(
        after == before,
        "every refused declaration appends nothing; the head moved from {before} to {after}"
    );
    // The same field, sensitive like its source, is an embedding.
    declare(
        cat.as_ref(),
        &own,
        notes_with(
            FieldDecl {
                sensitive: true,
                ..vec_field(&["secret"], EMBED_MODEL, 2)
            },
            vec![FieldDecl {
                sensitive: true,
                ..field("secret", StorageKind::Text)
            }],
        ),
    )
    .await?;
    Ok(())
}

check!(
    /// At most 64 texts go in one request.
    EMBED_BATCH = no_request_carries_more_than_sixty_four_texts,
    "seams, EmbedBackend: the store sends the due texts of one model in batches of at most 64, however many records one declaration makes due, so its memory per drain stays bounded",
    reopens
);

async fn no_request_carries_more_than_sixty_four_texts(
    store: Arc<dyn LakeStore>,
    reopen: Reopen,
) -> Result<(), Failure> {
    // A model of its own, so the sizes counted are this check's alone.
    let model = "conformance-batch";
    let store = reopen(store, with_embedder(Some(backing::embedder_as(model, 2))))
        .await
        .map_err(|e| reopen_failed(&format!("with a backend of `{model}`"), e))?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    // Ingest writes source fields: the text comes from a source here. The
    // records come first and the embedding after, so that one declaration
    // makes all 150 due at once; an ingest commits in smaller windows.
    let title = || FieldDecl {
        authority: Authority::Source,
        ..field("title", StorageKind::Text)
    };
    declare(cat.as_ref(), &own, type_decl("batch", vec![title()])).await?;
    let records: Vec<_> = (0..150)
        .map(|n| {
            source_record(
                "batch",
                &format!("b{n}"),
                json!({"title": format!("Batch {n}")}),
            )
        })
        .collect();
    ingest(cat.as_ref(), &own, "batches", records)
        .await
        .must("ingesting 150 batches")?;
    declare(
        cat.as_ref(),
        &own,
        type_decl("batch", vec![title(), vec_field(&["title"], model, 2)]),
    )
    .await?;
    let sizes = backing::embed_call_sizes(model);
    ensure!(
        sizes.iter().all(|n| *n <= 64) && sizes.iter().sum::<usize>() == 150,
        "declaring the embedding made 150 distinct texts due at once; every request carries at most 64 of them and \
         all 150 are sent once; the requests carried {sizes:?}"
    );
    Ok(())
}

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
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::Declaration;
use lakelands_api::{Error, LakeStore, Position};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::backing::{EMBED_MODEL, embedded, plug_text, unplug_text};
use crate::conformance::fixture::{
    Must, declare, doors, field, owner, read, type_decl, upsert, value,
};

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

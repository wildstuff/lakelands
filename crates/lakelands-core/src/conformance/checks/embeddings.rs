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
use std::sync::atomic::{AtomicBool, Ordering};

use lakelands_api::error::Seam;
use lakelands_api::event::EventBody;
use lakelands_api::ontology::{Authority, FieldDecl, FieldType, StorageKind};
use lakelands_api::principal::TypeGrants;
use lakelands_api::query::{Search, SearchMode};
use lakelands_api::seam::Seams;
use lakelands_api::{Access, Error, Grants, LakeStore, Outcome, Principal, PrincipalKind, Refusal};
use serde_json::{Value, json};

use super::search::doc_search;
use crate::conformance::backing::{
    EMBED_MODEL, UNREACHABLE, embedded, plug_text, release, unplug_text,
};
use crate::conformance::fixture::{
    Caller, Lake, Must, derived, doors, embedding, field, field_access, get_at, head_unmoved,
    join_all, migration, outputs_from, owner, principal, read, recorded_outputs, refused_on,
    source_record, sourced, type_decl, value,
};
use crate::conformance::{Failure, Given, Held, Reopen, Store, backing};

check!(
    VECTOR_COVERAGE = vector_coverage_counts_embedded_and_due_records_at_the_head,
    "022, extended, and lakelands#147: vector_coverage answers, per embedding field at the head, the live records holding a vector and those whose text is not embedded (never embedded, failed or waiting for a retry), not counting a record without text; a redeclaration's retry and a retire move the counts; an unknown type is not found and a type without embeddings answers none",
    embedding
);

check!(
    EMBED_WRITE = a_write_embeds_its_text_and_records_the_vector,
    "022, extended, and the log format: the store embeds the joined `from` text after the write, records the vector as output_recorded under embed:<model> naming the field, and the field holds it",
    embedding
);

check!(
    EMBED_ONCE = the_same_text_is_embedded_once_whichever_record_asks,
    "002 and 022, extended: a vector recorded for a text under a model is read back for every later record with that text, and after a record's text changed and came back; the log holds one output per distinct text",
    embedding
);

check!(
    EMBED_REBUILD = a_rebuild_reads_the_vectors_back_without_embedding,
    "001 and 002: a rebuild folds the recorded vectors; the fields read as before and nothing is embedded",
    embedding
);

check!(
    EMBED_OTHER_MODEL = an_embedding_whose_model_the_backend_does_not_make_is_refused_at_declare,
    "022, extended: an embedding is declared only where the bound backend names its model and makes vectors of its dimensions; otherwise the declaration is refused as unbound",
    embedding
);

check!(
    EMBED_NO_BACKEND = an_embedding_without_a_backend_is_refused_at_declare,
    "022, extended: with no embedding backend bound, declaring an embedding is refused as unbound, naming the model"
);

check!(
    EMBED_REDECLARED =
        a_changed_embedding_is_due_on_every_live_record_and_an_unchanged_one_embeds_nothing_new,
    "022, extended: a change of `from` or `model` is a redeclaration that makes the field due on every live record, one call per distinct text; declaring it again unchanged reads the recorded vectors back",
    embedding
);

check!(
    EMBED_NO_TEXT = a_record_with_nothing_to_embed_holds_no_vector_and_embeds_nothing,
    "022, extended: with no `from` field holding text, the field holds no value and keeps metadata without an error, and nothing is called; the text written later makes it due",
    embedding
);

check!(
    EMBED_FAILURE_RETRIED = a_failed_embedding_clears_the_field_and_a_redeclaration_retries_it,
    "022, extended: a failure is recorded, clears the field and puts its reason in the metadata; `recompute` calls again for that text once, the later output holds and the vector fills every record holding it",
    embedding
);

check!(
    EMBED_NAMED = a_vector_the_log_holds_is_named_by_its_position_not_carried_again,
    "053 and log-format §2.5: an embedding declared over ten records sharing two texts leaves two `output_recorded` that carry a vector and eight that name their call's by `output_at`, which the feed answers at that position, carrying the vector, for the same seam and input; every record holds the vector, after a rebuild, a reopen without a backend and an export's import too, and nothing is embedded again",
    given,
    embedding
);

check!(
    EMBED_NAMES_LATEST = a_fill_after_a_retry_names_the_new_output_and_a_failure_is_carried,
    "053, and 022, extended: a failure is carried, and so is the answer of the retry `recompute` makes; a second record's fill of the same call then names the new answer, not the failure, and holds its vector",
    embedding
);

/// The `output_recorded` events from `from` on: those that carry their
/// output, and those that name one by `output_at`.
async fn carried_and_named(
    store: &dyn LakeStore,
    from: lakelands_api::Position,
) -> Result<
    (
        Vec<lakelands_api::event::Event>,
        Vec<lakelands_api::event::Event>,
    ),
    Failure,
> {
    let outputs = outputs_from(store, from).await?;
    let named = |e: &lakelands_api::event::Event| {
        matches!(
            e.body,
            EventBody::OutputRecorded {
                output_at: Some(_),
                ..
            }
        )
    };
    Ok(outputs.into_iter().partition(|e| !named(e)))
}

/// The seam, input and `output_at` of an `output_recorded`.
fn call_of(
    e: &lakelands_api::event::Event,
) -> Option<(&str, &str, Option<lakelands_api::Position>)> {
    match &e.body {
        EventBody::OutputRecorded {
            seam,
            input,
            output_at,
            ..
        } => Some((seam, &input.0, *output_at)),
        _ => None,
    }
}

async fn a_vector_the_log_holds_is_named_by_its_position_not_carried_again(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(notes_by(&["title"], EMBED_MODEL, 2)).await?;
    let texts = ["Named harbour", "Named crane"];
    let keys: Vec<String> = (0..10).map(|i| format!("n{i}")).collect();
    for (i, key) in keys.iter().enumerate() {
        l.upsert("note", key, json!({"body": texts[i % 2]})).await?;
    }
    let from = store.head().await.must("reading the head")?;
    l.declare(notes_by(&["body"], EMBED_MODEL, 2)).await?;
    let (carried, named) = carried_and_named(store.as_ref(), from).await?;
    ensure!(
        carried.len() == 2 && named.len() == 8,
        "ten records share two texts: the log must hold two `output_recorded` carrying a \
         vector and eight naming one by `output_at`; it holds {} and {}",
        carried.len(),
        named.len()
    );
    for e in &named {
        let (seam, input, Some(at)) = call_of(e).expect("an output") else {
            unreachable!("a named output")
        };
        let held = outputs_from(store.as_ref(), at).await?;
        let first = held.first();
        ensure!(
            first.is_some_and(|h| h.position == at
                && call_of(h) == Some((seam, input, None))
                && matches!(&h.body, EventBody::OutputRecorded { output: Some(v), .. }
                    if is_east(&Some(v.clone())))),
            "the fill at {} names the output at {at}; the feed from there must answer first \
             the event carrying that vector for the same seam and input; it answers {first:?}",
            e.position
        );
    }
    let all_east = async |store: &dyn LakeStore, what: &str| {
        for key in &keys {
            let r = read(doors(store).as_ref(), &owner(store), "note", key).await?;
            let v = value(&r, "vec").cloned();
            ensure!(
                is_east(&v),
                "{what}, `{key}` must hold (1, 0); it holds {v:?}"
            );
        }
        Ok::<_, Failure>(())
    };
    let calls = texts.map(embedded);
    all_east(store.as_ref(), "after the declaration").await?;
    store.rebuild().await.must("rebuilding the lake")?;
    all_east(store.as_ref(), "after a rebuild").await?;
    let store = (given.reopen)(store, with_embedder(None))
        .await
        .map_err(|e| reopen_failed("without an embedding backend", e))?;
    all_east(store.as_ref(), "opened again without a backend").await?;
    let scratch = super::export::Scratch::new(given.scratch.clone());
    let dir = scratch.dir("named");
    crate::export::write(store.as_ref(), &dir)
        .await
        .must("exporting the lake")?;
    let lake = store.lake().clone();
    let imported = (given.import)(dir, lake, None, with_embedder(None))
        .await
        .map_err(|e| Failure::new(format!("importing the export into a fresh lake: {e}")))?;
    all_east(imported.store.as_ref(), "imported from an export").await?;
    ensure!(
        texts.map(embedded) == calls && calls == [1, 1],
        "each text is embedded once and never again by a rebuild, a reopen or an import; \
         they were embedded {:?} times",
        texts.map(embedded)
    );
    Ok(())
}

async fn a_fill_after_a_retry_names_the_new_output_and_a_failure_is_carried(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(notes_by(&["title"], EMBED_MODEL, 2)).await?;
    let text = "Named retry";
    unplug_text(text);
    for key in ["n1", "n2"] {
        l.upsert("note", key, json!({"body": text})).await?;
    }
    let from = store.head().await.must("reading the head")?;
    l.declare(notes_by(&["body"], EMBED_MODEL, 2)).await?;
    let (failed, named) = carried_and_named(store.as_ref(), from).await?;
    ensure!(
        failed.len() == 2 && named.is_empty(),
        "the backend failed on {text:?} for both records: each failure is carried, so the \
         log holds two carried outputs and none named; it holds {} and {}",
        failed.len(),
        named.len()
    );
    let head = store.head().await.must("reading the head")?;
    let retried = lakelands_api::Position(head.0 + 1);
    plug_text(text);
    recomputed(&l).await?;
    let (answered, named) = carried_and_named(store.as_ref(), retried).await?;
    let new = answered.first().map(|e| e.position);
    let names = named.first().and_then(call_of).and_then(|(_, _, at)| at);
    let (n1, n2) = (
        vec_of(store.as_ref(), "n1").await?,
        vec_of(store.as_ref(), "n2").await?,
    );
    ensure!(
        answered.len() == 1 && named.len() == 1 && names.is_some() && names == new,
        "`recompute` retries the failure once: the new answer is carried and the second \
         fill names it, the call's latest, not a failure; the log holds the carried \
         {answered:?} and the named {named:?}"
    );
    ensure!(
        is_east(&n1) && is_east(&n2),
        "both records hold the new answer's vector; they hold {n1:?} and {n2:?}"
    );
    Ok(())
}

check!(
    EMBED_FILL_REACTS = an_embedded_vector_recomputes_the_derived_fields_that_read_it,
    "012 and 022, extended: the field a vector fills is changed through the fold's ordinary path, so a derived field reading it is recomputed",
    embedding
);

fn notes_with(vec: FieldDecl, extra: Vec<FieldDecl>) -> lakelands_api::ontology::TypeDecl {
    let mut fields = vec![
        field("title", StorageKind::Text),
        field("body", StorageKind::Text),
        vec,
    ];
    fields.extend(extra);
    type_decl("note", fields)
}

/// `note` whose `vec` of `dimensions` embeds `from` with `model`.
fn notes_by(from: &[&str], model: &str, dimensions: u32) -> lakelands_api::ontology::TypeDecl {
    notes_with(embedding("vec", dimensions, from, model), vec![])
}

/// `note` computed again by the door: declared again it would settle nothing.
async fn recomputed(l: &Lake) -> Held {
    let what = lakelands_api::write::Recompute::Type("note".into());
    let done = l.cat.recompute(&l.own, what).await;
    done.must("computing `note` again").map(drop)
}

/// Declare `note`, whose `vec` embeds `title` and `body`.
async fn notes(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(notes_by(&["title", "body"], EMBED_MODEL, 2))
        .await?;
    Ok(l)
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

async fn a_write_embeds_its_text_and_records_the_vector(store: Store) -> Held {
    let l = notes(store.as_ref()).await?;
    l.upsert(
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
    let recorded = recorded_outputs(store.as_ref()).await?;
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

async fn the_same_text_is_embedded_once_whichever_record_asks(store: Store) -> Held {
    let l = notes(store.as_ref()).await?;
    let text = "Once harbour";
    for key in ["n1", "n2"] {
        l.upsert("note", key, json!({"title": text})).await?;
    }
    l.upsert("note", "n1", json!({"title": "Once elsewhere"}))
        .await?;
    l.upsert("note", "n1", json!({"title": text})).await?;
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
    let recorded = recorded_outputs(store.as_ref()).await?.len();
    ensure!(
        recorded == 2,
        "two distinct texts were embedded, so the log holds two `output_recorded`; a \
         record written with a text whose vector is recorded is filled in its own write. \
         It holds {recorded}"
    );
    Ok(())
}

async fn a_rebuild_reads_the_vectors_back_without_embedding(store: Store) -> Held {
    let l = notes(store.as_ref()).await?;
    let text = "Rebuilt harbour";
    l.upsert("note", "n1", json!({"title": text})).await?;
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
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let other_model = l.try_declare(notes_by(&["title"], "another-model", 2));
    let other_model = other_model.await;
    let other_dimensions = l.try_declare(notes_by(&["title"], EMBED_MODEL, 3)).await;
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

async fn an_embedding_without_a_backend_is_refused_at_declare(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let got = l.try_declare(notes_by(&["title"], EMBED_MODEL, 2)).await;
    ensure!(
        matches!(got.as_ref().err().and_then(Error::seam), Some(Seam::Embedding { model, .. }) if model == EMBED_MODEL),
        "with no embedding backend bound, declaring an embedding must be refused as \
         unbound, naming the model `{EMBED_MODEL}`; it answered {}",
        answered(&got)
    );
    Ok(())
}

async fn a_changed_embedding_is_due_on_every_live_record_and_an_unchanged_one_embeds_nothing_new(
    store: Store,
) -> Held {
    let l = notes(store.as_ref()).await?;
    let records = [
        ("n1", "Redeclared north", "Redeclared shared body"),
        ("n2", "Redeclared south", "Redeclared shared body"),
        ("n3", "Redeclared west", "Redeclared own body"),
    ];
    for (key, title, body) in records {
        l.upsert("note", key, json!({"title": title, "body": body}))
            .await?;
    }
    // Now made from `body` alone: two distinct texts for three records.
    l.declare(notes_by(&["body"], EMBED_MODEL, 2)).await?;
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
    l.declare(notes_by(&["body"], EMBED_MODEL, 2)).await?;
    ensure!(
        embedded("Redeclared shared body") == 1 && embedded("Redeclared own body") == 1,
        "declaring `vec` again unchanged reads the recorded vectors back; the bodies were \
         embedded {} and {} times",
        embedded("Redeclared shared body"),
        embedded("Redeclared own body")
    );
    Ok(())
}

async fn a_record_with_nothing_to_embed_holds_no_vector_and_embeds_nothing(store: Store) -> Held {
    let l = notes(store.as_ref()).await?;
    l.upsert("note", "n1", json!({"title": ""})).await?;
    let r = l.read("note", "n1").await?;
    let meta = r.meta.get("vec");
    ensure!(
        value(&r, "vec").is_none()
            && meta.is_some_and(|m| m.error.is_none())
            && recorded_outputs(store.as_ref()).await?.is_empty(),
        "`n1` has no text to embed: `vec` holds no value, keeps metadata without an error, \
         and nothing is recorded; it reads {:?} with metadata {meta:?}",
        value(&r, "vec")
    );
    let text = "Later harbour";
    l.upsert("note", "n1", json!({"title": text})).await?;
    let got = vec_of(store.as_ref(), "n1").await?;
    ensure!(
        is_east(&got) && embedded(text) == 1,
        "writing a title later makes `vec` due: {text:?} must be embedded once ({} times) \
         and `vec` read (1, 0); it reads {got:?}",
        embedded(text)
    );
    Ok(())
}

async fn a_failed_embedding_clears_the_field_and_a_redeclaration_retries_it(store: Store) -> Held {
    let l = notes(store.as_ref()).await?;
    let text = "Retried harbour";
    unplug_text(text);
    l.upsert("note", "n1", json!({"title": text})).await?;
    // A second record with the same text reads the recorded failure.
    l.upsert("note", "n2", json!({"title": text})).await?;
    let r = l.read("note", "n1").await?;
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
    l.upsert("note", "n1", json!({"body": ""})).await?;
    let before = embedded(text);
    plug_text(text);
    recomputed(&l).await?;
    let (n1, n2) = (
        vec_of(store.as_ref(), "n1").await?,
        vec_of(store.as_ref(), "n2").await?,
    );
    ensure!(
        before == 1 && embedded(text) == 2 && is_east(&n1) && is_east(&n2),
        "`recompute` of `note` retries the recorded failure: {text:?} must be sent once \
         more for both records that hold it (twice in all, not {}), and the later output \
         holds, so `vec` reads (1, 0) on both; they read {n1:?} and {n2:?}",
        embedded(text)
    );
    Ok(())
}

async fn an_embedded_vector_recomputes_the_derived_fields_that_read_it(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let has = derived(
        "embedded",
        StorageKind::Boolean,
        "vec is not null",
        &["vec"],
    );
    l.declare(notes_with(
        embedding("vec", 2, &["title"], EMBED_MODEL),
        vec![has],
    ))
    .await?;
    l.upsert("note", "n1", json!({"title": "Reacting harbour"}))
        .await?;
    let r = l.read("note", "n1").await?;
    ensure!(
        value(&r, "embedded") == Some(&json!(true)),
        "`embedded` is `vec is not null`; the vector's fill is a change of `vec`, so it \
         must read true, but reads {:?}",
        value(&r, "embedded")
    );
    Ok(())
}

check!(
    EMBED_BESIDE_BACKLOG = a_write_beside_a_backlog_waits_for_the_batch_in_flight_and_its_own,
    "022, extended, and 008: while the embeddings a declaration made due over a type are being made, a write that owes nothing answers at once, and one that owes a call answers after the batch in flight and its own, its field visible from its answer, while the rest is still being made; the declaration answers once all of it is made, each text once",
    embedding
);

check!(
    EMBED_ORPHAN_SWEEP = an_abandoned_sweep_advances_one_batch_with_each_write_that_owes,
    "022, extended: the embeddings a declaration whose caller went away left due wait for no one; a later write that owes a call answers after its own batch and one batch of them, and each such write advances them by a batch",
    embedding
);

check!(
    EMBED_SWEEP_REKEYED = a_rekey_while_a_type_is_embedded_leaves_every_record_embedded_once,
    "022, extended, and 027: a re-key that lands while the embeddings a declaration made due over its type are being made answers at once, and every record is then embedded under its new key, each text once",
    embedding
);

/// `doc`, keyed by the writer, with `code` and `body`, holding `N` records
/// fed before it has an embedding: `m000` on, `code` `a000` on. Answers
/// their texts; the text of each record in `held` starts `held-`, and the
/// suite's backend holds the request carrying it until it is released.
async fn backlog(l: &Lake, tag: &str, held: &[usize]) -> Result<Vec<String>, Failure> {
    l.declare(coded()).await?;
    let text = |i: usize| match held.contains(&i) {
        true => format!("held-{tag}-{i}"),
        false => format!("{tag} {i}"),
    };
    let texts: Vec<String> = (0..N).map(text).collect();
    let record = |(i, t): (usize, &String)| {
        source_record(
            "doc",
            &format!("m{i:03}"),
            json!({"code": format!("a{i:03}"), "body": t}),
        )
    };
    let fut = l.ingest("feed", texts.iter().enumerate().map(record).collect());
    fut.await.must("feeding the records")?;
    Ok(texts)
}

/// `doc` with `code`, which every record holds, and `body`.
fn coded() -> lakelands_api::ontology::TypeDecl {
    let code = FieldDecl {
        required: true,
        ..sourced("code", StorageKind::Text)
    };
    type_decl("doc", vec![code, sourced("body", StorageKind::Text)])
}

/// Records [`backlog`] holds: four batches of the store's.
const N: usize = 200;

/// Whether `done` comes to hold within five seconds.
async fn came<F: std::future::Future<Output = bool>>(done: impl Fn() -> F) -> bool {
    for _ in 0..1_000 {
        if done().await {
            return true;
        }
        crate::conformance::fixture::pause(std::time::Duration::from_millis(5)).await;
    }
    false
}

/// The embedding of `body` added to `doc`, setting `done` once answered.
fn embedded_over<'a>(l: &'a Lake, done: &'a AtomicBool) -> Caller<'a, Held> {
    Box::pin(async move {
        let vec = embedding("vec", 2, &["body"], EMBED_MODEL);
        let added = l.cat.add_field(&l.own, &"doc".into(), vec).await;
        done.store(true, Ordering::Release);
        added.must("adding `vec` over `doc`").map(drop)
    })
}

/// Every record of `doc` holds a vector, and each text was sent once.
async fn each_once(store: &dyn LakeStore, texts: &[String]) -> Held {
    let c = store.vector_coverage(&"doc".into()).await;
    let c = c.must("asking vector_coverage of `doc`")?;
    let twice: Vec<&String> = texts.iter().filter(|t| embedded(t) != 1).collect();
    ensure!(
        matches!(c.as_slice(), [one] if one.embedded == N as u64 && one.due == 0)
            && twice.is_empty(),
        "every one of the {N} records holds its vector and each text was sent once; \
         vector_coverage answered {c:?}, and these texts were not sent once: {twice:?}"
    );
    Ok(())
}

async fn a_write_beside_a_backlog_waits_for_the_batch_in_flight_and_its_own(store: Store) -> Held {
    let token = "beside_a_backlog";
    let l =
        super::backings::places(store.as_ref(), json!({"token": token, "echo": "code"})).await?;
    l.declare(type_decl("memo", vec![field("text", StorageKind::Text)]))
        .await?;
    let texts = backlog(&l, "beside", &[0, 150]).await?;
    let done = AtomicBool::new(false);
    let beside: Caller<'_, Held> = Box::pin(async {
        ensure!(
            came(|| async { embedded(&texts[0]) == 1 }).await,
            "the first request of `vec`'s backlog never reached the backend"
        );
        l.upsert("memo", "m", json!({"text": "aside"})).await?;
        ensure!(
            !done.load(Ordering::Acquire) && embedded(&texts[N - 1]) == 0,
            "a write that owes nothing answers while the backlog's first request is held: \
             the backlog's last text must not be sent yet, and was sent {} times",
            embedded(&texts[N - 1])
        );
        let before = store.head().await.must("reading the head")?;
        let own: Caller<'_, Result<Option<Outcome>, Failure>> = Box::pin(async {
            l.upsert("place", "p1", json!({"code": "AT"}))
                .await
                .map(Some)
        });
        let freed: Caller<'_, Result<Option<Outcome>, Failure>> = Box::pin(async {
            let moved = || async { store.head().await.is_ok_and(|h| h > before) };
            ensure!(came(moved).await, "the write of `p1` was never appended");
            release(&texts[0]);
            Ok(None)
        });
        let [own, released] = <[_; 2]>::try_from(join_all(vec![own, freed]).await)
            .map_err(|_| Failure::new("two callers answer two outcomes"))?;
        let (at, _) = (own?.map(|o| o.position()), released?);
        let got = l.cat.get(&l.own, &"place".into(), get_at("p1", at)).await;
        let name = got
            .must("reading `p1` at its answer")?
            .data
            .fields
            .get("name")
            .cloned();
        ensure!(
            name == Some(json!("AT")) && !done.load(Ordering::Acquire),
            "a write whose own call is due answers after the batch in flight and its own, \
             while the backlog waits at its held request: as of its answer {at:?} `name` \
             reads {name:?}, and the declaration had answered: {}",
            done.load(Ordering::Acquire)
        );
        release(&texts[150]);
        Ok(())
    });
    for r in join_all(vec![embedded_over(&l, &done), beside]).await {
        r?;
    }
    each_once(store.as_ref(), &texts).await
}

async fn an_abandoned_sweep_advances_one_batch_with_each_write_that_owes(store: Store) -> Held {
    let token = "abandoned_sweep";
    let config = json!({"token": token, "echo": "code"});
    let l = super::backings::places(store.as_ref(), config).await?;
    let texts = backlog(&l, "abandoned", &[0]).await?;
    // The declaration's caller goes away while its first request is held.
    let done = AtomicBool::new(false);
    let mut declared = embedded_over(&l, &done);
    std::future::poll_fn(|cx| match declared.as_mut().poll(cx) {
        _ if embedded(&texts[0]) == 1 => std::task::Poll::Ready(()),
        std::task::Poll::Ready(_) => std::task::Poll::Ready(()),
        std::task::Poll::Pending => std::task::Poll::Pending,
    })
    .await;
    drop(declared);
    release(&texts[0]);
    let mut covered = Vec::new();
    for code in ["AT", "BE"] {
        l.upsert("place", code, json!({"code": code})).await?;
        let c = store.vector_coverage(&"doc".into()).await;
        covered.push(c.must("asking vector_coverage of `doc`")?[0].embedded);
    }
    ensure!(
        !done.load(Ordering::Acquire) && covered == [64, 128],
        "the declaration's caller went away; each later write that owes a call makes its own \
         and one batch of the 64 records the declaration left, so `doc` holds 64 vectors and \
         then 128; it held {covered:?}"
    );
    Ok(())
}

async fn a_rekey_while_a_type_is_embedded_leaves_every_record_embedded_once(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let texts = backlog(&l, "rekeyed", &[0]).await?;
    let done = AtomicBool::new(false);
    let rekey: Caller<'_, Held> = Box::pin(async {
        ensure!(
            came(|| async { embedded(&texts[0]) == 1 }).await,
            "the first request of `vec`'s backlog never reached the backend"
        );
        let mut to = coded();
        to.fields.push(embedding("vec", 2, &["body"], EMBED_MODEL));
        to.key = vec!["code".into()];
        l.migrate(to).await?;
        ensure!(
            !done.load(Ordering::Acquire),
            "the re-key answers while the backlog's first request is held"
        );
        release(&texts[0]);
        Ok(())
    });
    for r in join_all(vec![embedded_over(&l, &done), rekey]).await {
        r?;
    }
    each_once(store.as_ref(), &texts).await
}

pub(super) fn with_embedder(embed: Option<Arc<dyn lakelands_api::seam::EmbedBackend>>) -> Seams {
    Seams {
        backings: backing::backings(),
        embed,
        ..Seams::default()
    }
}

pub(super) fn reopen_failed(what: &str, e: Error) -> Failure {
    Failure::new(format!("opening the lake again {what} failed: {e} ({e:?})"))
}

check!(
    EMBED_SEARCH_OTHER_BACKEND =
        a_hybrid_search_over_an_embedding_the_bound_backend_does_not_make_is_unbound,
    "seams, EmbedBackend: a hybrid search over an embed field whose model or dimensions are not the bound backend's is refused as unbound when it is made, not ranked with vectors of another space; a search as of an earlier position is held to the head's declaration, whose vectors the engines index",
    reopens
);

async fn a_hybrid_search_over_an_embedding_the_bound_backend_does_not_make_is_unbound(
    store: Store,
    reopen: Reopen,
) -> Held {
    let mut store = reopen(store, with_embedder(Some(backing::embedder())))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend", e))?;
    let l = notes(store.as_ref()).await?;
    l.upsert("note", "n1", json!({"title": "Quayside crane"}))
        .await?;
    let before = store.head().await.must("reading the head")?;
    for (model, dimensions) in [("another-model", 2), (EMBED_MODEL, 3)] {
        store = reopen(
            store,
            with_embedder(Some(backing::embedder_as(model, dimensions))),
        )
        .await
        .map_err(|e| reopen_failed(&format!("with a backend of `{model}`/{dimensions}"), e))?;
        let got = quay(store.as_ref(), None).await;
        // The refusal names both: what the field was declared with, and
        // what is bound.
        let names_both = |seam: &Seam| {
            let model = model.to_owned();
            let bound = Some(lakelands_api::error::Bound { model, dimensions });
            matches!(seam, Seam::Embedding { model: m, dimensions: 2, bound: b, .. }
                if m == EMBED_MODEL && *b == bound)
        };
        ensure!(
            matches!(&got, Err(Error::Unbound { seam }) if names_both(seam)),
            "`vec` holds `{EMBED_MODEL}` vectors of 2 dimensions; with a backend of \
             `{model}` and {dimensions} dimensions bound, a hybrid search over it must be \
             refused as unbound, naming both; it answered {}",
            answered(&got)
        );
    }
    // The engines index the head's vectors only: `vec` declared again with
    // `another-model`, a search as of before is embedded by the head's
    // declaration, not by the one it reads.
    let other = || with_embedder(Some(backing::embedder_as("another-model", 2)));
    store = reopen(store, other())
        .await
        .map_err(|e| reopen_failed("with a backend of `another-model`", e))?;
    let l = Lake::of(store.as_ref());
    l.declare(notes_by(&["title", "body"], "another-model", 2))
        .await?;
    recomputed(&l).await?;
    let ranked = quay(store.as_ref(), Some(before)).await;
    store = reopen(store, with_embedder(Some(backing::embedder())))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend", e))?;
    let old = quay(store.as_ref(), Some(before)).await;
    ensure!(
        ranked.as_ref().is_ok_and(|h| h.ranked_by == "hybrid")
            && matches!(&old, Err(Error::Unbound { seam })
                if matches!(&**seam, Seam::Embedding { model, dimensions: 2, .. } if model == "another-model")),
        "`vec`, declared again with `another-model`, holds its vectors at the head: a search \
         as of before ranks `hybrid` with that backend bound and is unbound, naming it, with \
         `{EMBED_MODEL}` bound; they answered {} and {}",
        format!("{:?}", ranked.map(|h| h.ranked_by)),
        answered(&old)
    );
    Ok(())
}

check!(
    EMBED_SEARCH_JUDGED_FIRST =
        a_hybrid_search_is_judged_for_its_caller_before_its_model_or_its_text,
    "024 and seams, EmbedBackend: a hybrid search without a vector is judged for its caller before its field's model is compared with the bound backend's and before its text is embedded: whatever backend is bound, a type not shown answers as one not declared, a field the caller may not know of, or one its vector is embedded from, as one not declared, and a masked one as not granted; a refused search calls nothing, the owner's over a type or field not declared or with a `where` on one included",
    reopens
);

/// A hybrid search for `text` over `ty.field` by `who`, with `filter` as
/// its `where`: how it ranked, or its error's words, and how often `text`
/// was embedded.
async fn judged(
    store: &dyn LakeStore,
    who: &Principal,
    (ty, field, filter): (&str, &str, Option<&str>),
    text: &str,
) -> (Result<String, String>, usize) {
    let mode = SearchMode::Hybrid {
        text: text.into(),
        field: field.into(),
        vector: None,
        fields: None,
        weight: None,
        text_weight: None,
    };
    let search = Search {
        ty: ty.into(),
        filter: filter.map(Into::into),
        ..doc_search(mode)
    };
    let got = doors(store).search(who, search).await;
    let got = got.map(|r| r.data.ranked_by).map_err(|e| format!("{e:?}"));
    (got, embedded(text))
}

async fn a_hybrid_search_is_judged_for_its_caller_before_its_model_or_its_text(
    store: Store,
    reopen: Reopen,
) -> Held {
    let mut store = reopen(store, with_embedder(Some(backing::embedder())))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend", e))?;
    let l = notes(store.as_ref()).await?;
    l.upsert("note", "n1", json!({"title": "Judged quay"}))
        .await?;
    let (s, own) = (store.as_ref(), l.own.clone());
    let mut grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let none = TypeGrants {
        default: Some(Access::None),
        ..TypeGrants::default()
    };
    grants.types.insert("note".into(), none);
    let held = |id: &str, f: &str, a| field_access(s, id, "note", &[(f, a)]);
    // Each caller, and what its answer names: the type, or the field.
    let cast = [
        (
            principal(s, PrincipalKind::Agent, "unshown", grants),
            "note",
        ),
        (held("vec-none", "vec", Access::None), "vec"),
        (held("body-none", "body", Access::None), "body"),
        (held("vec-masked", "vec", Access::Masked), "masked"),
    ];
    let masked = Error::Refused {
        field: Some("vec".into()),
        reason: Refusal::NotGranted,
    };
    let backends = [None, Some(("another-model", 2)), Some((EMBED_MODEL, 3))];
    for (round, backend) in backends.into_iter().enumerate() {
        if let Some((model, dimensions)) = backend {
            let embed = backing::embedder_as(model, dimensions);
            store = reopen(store, with_embedder(Some(embed)))
                .await
                .map_err(|e| reopen_failed(&format!("with a backend of `{model}`"), e))?;
        }
        let (s, text) = (store.as_ref(), |of: &str| format!("Judged {round} {of}"));
        let (ghost, ghost_calls) = judged(s, &own, ("zz", "vec", None), &text("zz")).await;
        let (nope, nope_calls) = judged(s, &own, ("note", "zz", None), &text("note.zz")).await;
        let filter = Some("zz = 1");
        let (bad, bad_calls) = judged(s, &own, ("note", "vec", filter), &text("where")).await;
        for (who, named) in &cast {
            let (got, calls) = judged(s, who, ("note", "vec", None), &text(&who.id)).await;
            let want = match *named {
                "masked" => Err(format!("{masked:?}")),
                "note" => ghost.clone().map_err(|e| e.replace("zz", "note")),
                field => nope.clone().map_err(|e| e.replace("zz", field)),
            };
            ensure!(
                got == want && calls == 0,
                "with {backend:?} bound, `{}`'s search over `note.vec` answers {want:?} \
                 and calls nothing; it answered {got:?} after {calls} calls",
                who.id
            );
        }
        ensure!(
            ghost.is_err()
                && nope.is_err()
                && bad.is_err()
                && ghost_calls + nope_calls + bad_calls == 0,
            "with {backend:?} bound, the owner's search over `zz`, over `note.zz` and \
             `where zz = 1` is refused and calls nothing; they answered {ghost:?}, {nope:?} \
             and {bad:?} after {ghost_calls}, {nope_calls} and {bad_calls} calls"
        );
        let (got, calls) = judged(s, &own, ("note", "vec", None), &text("quay")).await;
        let fits = match backend {
            None => got.as_deref() == Ok("hybrid") && calls == 1,
            Some(_) => got.as_ref().is_err_and(|e| e.starts_with("Unbound")) && calls == 0,
        };
        ensure!(
            fits,
            "the owner's hybrid search over `note.vec` ranks `hybrid` after one call with \
             the suite's backend bound and is unbound, calling nothing, with another; with \
             {backend:?} it answered {got:?} after {calls} calls"
        );
    }
    Ok(())
}

check!(
    EMBED_WAITS = a_due_embedding_waits_while_no_backend_makes_it,
    "seams, EmbedBackend: a due embedding whose backend is gone or makes another model or other dimensions is not made; the field waits, with neither a value nor metadata, until its record is written again or `recompute` names its type, with the backend bound",
    reopens
);

async fn a_due_embedding_waits_while_no_backend_makes_it(store: Store, reopen: Reopen) -> Held {
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
        let l = Lake::of(store.as_ref());
        l.upsert("note", key, json!({"title": text})).await?;
        let r = l.read("note", key).await?;
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
    // record is written again or the door names its type.
    store = reopen(store, with_embedder(Some(backing::embedder())))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend again", e))?;
    let l = Lake::of(store.as_ref());
    l.upsert("note", "later", json!({"title": "Later quay"}))
        .await?;
    let still = vec_of(store.as_ref(), "gone").await?;
    ensure!(
        still.is_none(),
        "opening the lake and writing another record call nothing for `gone`, whose \
         `vec` waits; it reads {still:?}"
    );
    l.upsert("note", "gone", json!({"body": "touched"})).await?;
    let touched = vec_of(store.as_ref(), "gone").await?;
    ensure!(
        is_east(&touched),
        "`gone` was written again with its backend bound, so its waiting `vec` is made; \
         it reads {touched:?}"
    );
    recomputed(&l).await?;
    let (other, wider) = (
        vec_of(store.as_ref(), "other").await?,
        vec_of(store.as_ref(), "wider").await?,
    );
    ensure!(
        is_east(&other) && is_east(&wider),
        "`recompute` of `note`, its backend bound, makes every waiting `vec`; `other` \
         and `wider` read {other:?} and {wider:?}"
    );
    Ok(())
}

check!(
    EMBED_BOUND_LATE = a_backend_bound_after_open_is_read_from_then_on,
    "lakelands#641 and seams, EmbedBackend: `bind_embed` on an open lake appends nothing, and from then on a declaration of its model is admitted, a write's embedding is made and a hybrid search embeds its text; a vector that fell due while none was bound waits until `recompute` names its type; a later bind replaces the backend, whatever its model",
    reopens
);

check!(
    EMBED_UNBOUND_KEPT = a_declaration_that_keeps_an_embedding_needs_no_backend,
    "022, extended, and lakelands#641: with no backend bound, a declaration that keeps a type's embedding as in force is admitted (restated, alone or in a set, with a relation whose link type embeds; a field added beside it; a migration), while one that adds an embedding or changes its model, dimensions or `from` is refused as unbound, naming the field, and so is `recompute` of the type",
    reopens
);

/// A hybrid search over `note.vec` for "quay", as the owner, as of `as_of`.
async fn quay(
    store: &dyn LakeStore,
    as_of: Option<lakelands_api::Position>,
) -> lakelands_api::Result<lakelands_api::query::Hits> {
    let mode = SearchMode::Hybrid {
        text: "quay".into(),
        field: "vec".into(),
        vector: None,
        fields: None,
        weight: None,
        text_weight: None,
    };
    let search = Search {
        ty: "note".into(),
        at: lakelands_api::ReadAt {
            as_of,
            ..Default::default()
        },
        ..doc_search(mode)
    };
    doors(store)
        .search(&owner(store), search)
        .await
        .map(|r| r.data)
}

/// `berth`, whose `moors` to `note` carries `near`, the embedding of its
/// link's `memo`.
fn berths() -> lakelands_api::ontology::TypeDecl {
    let mut ty = type_decl("berth", vec![]);
    let fields = vec![
        field("memo", StorageKind::Text),
        embedding("near", 2, &["memo"], EMBED_MODEL),
    ];
    ty.relations = vec![lakelands_api::ontology::RelationDecl {
        fields,
        ..lakelands_api::ontology::RelationDecl::to("moors", "note")
    }];
    ty
}

/// `note` and `berths` declared with the suite's backend, and the lake
/// opened again with none bound.
async fn unbound_notes(store: Store, reopen: &Reopen) -> Result<Store, Failure> {
    let store = reopen(store, with_embedder(Some(backing::embedder())))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend", e))?;
    notes(store.as_ref()).await?.declare(berths()).await?;
    reopen(store, with_embedder(None))
        .await
        .map_err(|e| reopen_failed("with no embedding backend", e))
}

async fn a_backend_bound_after_open_is_read_from_then_on(store: Store, reopen: Reopen) -> Held {
    let store = unbound_notes(store, &reopen).await?;
    let l = Lake::of(store.as_ref());
    let early = "Bound late early quay";
    l.upsert("note", "early", json!({"title": early})).await?;
    let before = quay(store.as_ref(), None);
    let before = before.await.must("a hybrid search with none bound")?;
    let head = store.head().await.must("reading the head")?;
    store
        .bind_embed(backing::embedder())
        .must("binding the suite's backend")?;
    let after = store.head().await.must("reading the head")?;
    ensure!(
        after == head,
        "a bind appends nothing; the head moved from {head} to {after}"
    );
    l.upsert("note", "late", json!({"title": "Bound late later quay"}))
        .await?;
    let searched = quay(store.as_ref(), None);
    let searched = searched.await.must("a hybrid search once bound")?;
    let (late, waits) = (
        vec_of(store.as_ref(), "late").await?,
        vec_of(store.as_ref(), "early").await?,
    );
    ensure!(
        before.ranked_by.starts_with("lexical")
            && searched.ranked_by.starts_with("hybrid")
            && is_east(&late)
            && waits.is_none(),
        "a hybrid search ranks lexically with none bound and embeds its text once bound \
         (they said {:?} and {:?}), and a write's `vec` is made then while `early`, due \
         before the bind, waits: `late` reads {late:?}, `early` {waits:?}",
        before.ranked_by,
        searched.ranked_by
    );
    recomputed(&l).await?;
    let made = vec_of(store.as_ref(), "early").await?;
    ensure!(
        is_east(&made) && embedded(early) == 1,
        "`recompute` of `note` after the bind makes `early`'s `vec` once; it reads \
         {made:?}, its text sent {} times",
        embedded(early)
    );
    store
        .bind_embed(backing::embedder_as("another-model", 2))
        .must("binding a backend of another model")?;
    let search = quay(store.as_ref(), None).await;
    let other = l.try_declare(notes_by(&["title", "body"], "another-model", 2));
    let other = other.await;
    ensure!(
        other.is_ok() && is_unbound(&search),
        "a second bind replaces the first: a search over `note.vec`, of `{EMBED_MODEL}`, \
         is unbound and `vec` declared with `another-model` is admitted; they answered {} \
         and {}",
        answered(&search),
        answered(&other)
    );
    Ok(())
}

async fn a_declaration_that_keeps_an_embedding_needs_no_backend(
    store: Store,
    reopen: Reopen,
) -> Held {
    let store = unbound_notes(store, &reopen).await?;
    let l = Lake::of(store.as_ref());
    let kept = notes_by(&["title", "body"], EMBED_MODEL, 2);
    l.declare(kept.clone()).await?;
    l.declare(berths()).await?;
    let set = vec![lakelands_api::write::Declaration::Type(kept.clone())];
    let fut = l.cat.apply(&l.own, set);
    fut.await.must("applying `note` as in force")?;
    // Other dimensions reshape the field: a migration, checked the same way.
    let wider = migration(notes_by(&["title", "body"], EMBED_MODEL, 3));
    let report = l.cat.migrate(&l.own, wider.clone()).await;
    let id = report
        .must("the preflight of `vec` with 3 dimensions")?
        .data
        .report_id;
    let added = notes_with(
        kept.fields[2].clone(),
        vec![embedding("vec2", 2, &["body"], EMBED_MODEL)],
    );
    let recompute = lakelands_api::write::Recompute::Type("note".into());
    for (got, named, what) in [
        (
            l.try_declare(notes_by(&["title"], EMBED_MODEL, 2)).await,
            "vec",
            "`note` with its `from` changed",
        ),
        (
            l.try_declare(notes_by(&["title", "body"], "another-model", 2))
                .await,
            "vec",
            "`note` with its model changed",
        ),
        (
            l.try_declare(added).await,
            "vec2",
            "`note` with an embedding added",
        ),
        (
            l.cat.migrate_apply(&l.own, wider, &id).await,
            "vec",
            "`vec` migrated to 3 dimensions",
        ),
        (
            l.cat.recompute(&l.own, recompute).await,
            "vec",
            "`recompute` of `note`",
        ),
    ] {
        ensure!(
            matches!(got.as_ref().err().and_then(Error::seam), Some(Seam::Embedding { ty, field, .. })
                if ty.as_str() == "note" && field == named),
            "with none bound, {what} must be refused as unbound, naming `note.{named}`; it \
             answered {}",
            answered(&got)
        );
    }
    let tagged = notes_with(
        kept.fields[2].clone(),
        vec![field("tag", StorageKind::Text)],
    );
    l.declare(tagged).await?;
    l.migrate(kept).await?;
    Ok(())
}

check!(
    EMBED_RULES = an_embedding_that_is_not_a_derived_vector_of_its_own_text_fields_is_refused,
    "seams, EmbedBackend, and TypeDecl::embedding_problem: an embed field is a derived vector with dimensions, made from distinct text fields of its type other than itself, sensitive where a source is; anything else is refused as invalid, whatever backend is bound",
    embedding
);

async fn an_embedding_that_is_not_a_derived_vector_of_its_own_text_fields_is_refused(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let good = || embedding("vec", 2, &["title"], EMBED_MODEL);
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
        (
            "made from nothing",
            embedding("vec", 2, &[], EMBED_MODEL),
            vec![],
        ),
        (
            "made from itself",
            embedding("vec", 2, &["title", "vec"], EMBED_MODEL),
            vec![],
        ),
        (
            "made from a field twice",
            embedding("vec", 2, &["title", "title"], EMBED_MODEL),
            vec![],
        ),
        (
            "made from a field it lacks",
            embedding("vec", 2, &["summary"], EMBED_MODEL),
            vec![],
        ),
        (
            "made from a number",
            embedding("vec", 2, &["pages"], EMBED_MODEL),
            vec![number.clone()],
        ),
        (
            "made from a sensitive field and not sensitive itself",
            embedding("vec", 2, &["secret"], EMBED_MODEL),
            vec![FieldDecl {
                sensitive: true,
                ..field("secret", StorageKind::Text)
            }],
        ),
    ];
    let before = store.head().await.must("reading the head")?;
    for (what, vec, extra) in cases {
        let got = l.try_declare(notes_with(vec, extra)).await;
        ensure!(
            matches!(
                refused_on(&got),
                Some(("vec", lakelands_api::Refusal::Invalid(..)))
            ),
            "declaring `vec` as {what} must be refused as invalid, naming `vec`; it answered {}",
            answered(&got)
        );
    }
    head_unmoved(store.as_ref(), before, "every refused declaration").await?;
    // The same field, sensitive like its source, is an embedding.
    l.declare(notes_with(
        FieldDecl {
            sensitive: true,
            ..embedding("vec", 2, &["secret"], EMBED_MODEL)
        },
        vec![FieldDecl {
            sensitive: true,
            ..field("secret", StorageKind::Text)
        }],
    ))
    .await?;
    Ok(())
}

check!(
    EMBED_BATCH = no_request_carries_more_than_sixty_four_texts,
    "seams, EmbedBackend: the store sends the due texts of one model in batches of at most 64, however many records one declaration makes due, so its memory per drain stays bounded",
    reopens
);

async fn no_request_carries_more_than_sixty_four_texts(store: Store, reopen: Reopen) -> Held {
    // A model of its own, so the sizes counted are this check's alone.
    let model = "conformance-batch";
    let store = reopen(store, with_embedder(Some(backing::embedder_as(model, 2))))
        .await
        .map_err(|e| reopen_failed(&format!("with a backend of `{model}`"), e))?;
    let l = Lake::of(store.as_ref());
    // Ingest writes source fields: the text comes from a source here. The
    // records come first and the embedding after, so that one declaration
    // makes all 150 due at once; an ingest commits in smaller windows.
    let title = || sourced("title", StorageKind::Text);
    l.declare(type_decl("batch", vec![title()])).await?;
    let records: Vec<_> = (0..150)
        .map(|n| {
            source_record(
                "batch",
                &format!("b{n}"),
                json!({"title": format!("Batch {n}")}),
            )
        })
        .collect();
    let fut = l.ingest("batches", records);
    fut.await.must("ingesting 150 batches")?;
    l.declare(type_decl(
        "batch",
        vec![title(), embedding("vec", 2, &["title"], model)],
    ))
    .await?;
    let sizes = backing::embed_call_sizes(model);
    ensure!(
        sizes.iter().all(|n| *n <= 64) && sizes.iter().sum::<usize>() == 150,
        "declaring the embedding made 150 distinct texts due at once; every request carries at most 64 of them and \
         all 150 are sent once; the requests carried {sizes:?}"
    );
    Ok(())
}

check!(
    EMBED_REFUSED_ALONE = a_text_the_backend_refuses_fails_alone_in_its_batch,
    "lakelands#658 and seams, EmbedBackend: a request the backend refuses for what it carries is asked again in halves, so only the text refused alone records the refusal and the others in its batch are embedded; a later record with one of their texts reads its vector, and `recompute` sends the refused text alone",
    embedding
);

check!(
    EMBED_WEATHER = weather_fails_every_text_its_request_had_not_answered,
    "lakelands#658 and seams, EmbedBackend: a failure that is not a refusal ends the asking and is recorded for every text not answered yet, while the texts answered before it keep their vectors and a text refused alone its refusal",
    embedding
);

/// Declare `pier` with records of `titles` (`p0`, `p1`, …), then its
/// embedding, which makes every title due in one batch: the requests made.
async fn piers(l: &Lake, titles: &[String]) -> Result<usize, Failure> {
    let title = || sourced("title", StorageKind::Text);
    l.declare(type_decl("pier", vec![title()])).await?;
    let records = titles.iter().enumerate();
    let records =
        records.map(|(n, t)| source_record("pier", &format!("p{n}"), json!({"title": t})));
    let fut = l.ingest("piers", records.collect());
    fut.await.must("ingesting piers")?;
    let before = backing::embed_calls();
    let vec = embedding("vec", 2, &["title"], EMBED_MODEL);
    l.declare(type_decl("pier", vec![title(), vec])).await?;
    Ok(backing::embed_calls() - before)
}

/// What each pier's `vec` reads: `east`, or the error its metadata carries.
async fn pier_vecs(store: &dyn LakeStore, n: usize) -> Result<Vec<String>, Failure> {
    let mut out = Vec::new();
    for key in (0..n).map(|n| format!("p{n}")) {
        let r = read(doors(store).as_ref(), &owner(store), "pier", &key).await?;
        let east = is_east(&value(&r, "vec").cloned()).then(|| "east".to_owned());
        out.push(
            r.meta
                .get("vec")
                .and_then(|m| m.error.clone())
                .or(east)
                .unwrap_or_default(),
        );
    }
    Ok(out)
}

async fn a_text_the_backend_refuses_fails_alone_in_its_batch(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let mut titles: Vec<String> = (0..8).map(|n| format!("Refusal pier {n}")).collect();
    titles[5] = format!("{UNREACHABLE} pier");
    let calls = piers(&l, &titles).await?;
    let got = pier_vecs(store.as_ref(), 8).await?;
    let failed: Vec<_> = got.iter().map(|g| g != "east").collect();
    ensure!(
        calls == 7 && failed == [false, false, false, false, false, true, false, false],
        "of eight texts in one batch the backend refuses `p5`'s: halving takes 1 + 2 × 3 = 7 \
         requests, `p5` holds the refusal and the other seven a vector; {calls} requests, \
         and the piers read {got:?}"
    );
    let before = backing::embed_calls();
    let again = source_record("pier", "p8", json!({"title": titles[0]}));
    let fut = l.ingest("piers", vec![again]);
    fut.await.must("ingesting `p8`")?;
    let p8 = pier_vecs(store.as_ref(), 9).await?.pop();
    let sent = embedded(&titles[5]);
    let what = lakelands_api::write::Recompute::Type("pier".into());
    let fut = l.cat.recompute(&l.own, what);
    fut.await.must("computing `pier` again")?;
    let calls = backing::embed_calls() - before;
    ensure!(
        p8.as_deref() == Some("east") && calls == 1 && embedded(&titles[5]) == sent + 1,
        "a later record with `p0`'s text reads its vector without a call, and `recompute` \
         sends the refused text alone: one request in all, not {calls}; `p8` reads {p8:?}"
    );
    Ok(())
}

async fn weather_fails_every_text_its_request_had_not_answered(store: Store) -> Held {
    use crate::conformance::backing::WEATHER;
    let l = Lake::of(store.as_ref());
    let titles = ["Weather pier 0", UNREACHABLE, WEATHER, "Weather pier 3"];
    let titles = titles.map(|t| format!("{t} quay"));
    let calls = piers(&l, &titles).await?;
    let got = pier_vecs(store.as_ref(), 4).await?;
    // The weather as its case says it: the backend answered 503.
    let remote = lakelands_api::error::Remote::Embedding {
        model: EMBED_MODEL.to_owned(),
    };
    let exchange = lakelands_api::error::Exchange::Status { status: 503 };
    let down = lakelands_api::error::Situation::Remote { remote, exchange }.to_string();
    ensure!(
        calls == 5
            && got[0] == "east"
            && got[1].contains("refused the texts")
            && got[2..].iter().all(|g| *g == down),
        "`p1`'s text is refused and `p2`'s meets weather: the halves are asked, `p0` is \
         embedded, `p1` holds its refusal, and the weather of `p2`'s request is recorded for \
         `p2` and `p3` without narrowing it, in 5 requests; {calls} requests, and the piers \
         read {got:?}"
    );
    Ok(())
}

async fn coverage(store: &dyn LakeStore, what: &str) -> Result<(u64, u64), Failure> {
    let c = store
        .vector_coverage(&lakelands_api::TypeName::from("note"))
        .await
        .must(&format!("asking vector_coverage of `note` {what}"))?;
    match c.as_slice() {
        [one] if one.field.as_str() == "vec" => Ok((one.embedded, one.due)),
        other => Err(Failure::new(format!(
            "`note` has one embedding field, `vec`; vector_coverage {what} answered {other:?}"
        ))),
    }
}

async fn vector_coverage_counts_embedded_and_due_records_at_the_head(store: Store) -> Held {
    let l = notes(store.as_ref()).await?;
    for (key, title) in [
        ("n1", "Coverage harbour one"),
        ("n2", "Coverage harbour two"),
        ("n3", "Coverage harbour three"),
        ("n0", ""),
    ] {
        l.upsert("note", key, json!({"title": title})).await?;
    }
    let got = coverage(store.as_ref(), "once the backend settled").await?;
    ensure!(
        got == (3, 0),
        "three notes with text are embedded and one has none: (embedded, due) is (3, 0), and \
         vector_coverage answered {got:?}"
    );

    let failing = ["Coverage harbour four", "Coverage harbour five"];
    for (key, title) in [("n4", failing[0]), ("n5", failing[1])] {
        unplug_text(title);
        l.upsert("note", key, json!({"title": title})).await?;
    }
    let got = coverage(store.as_ref(), "with two embeddings failed").await?;
    ensure!(
        got == (3, 2),
        "two notes whose embedding failed are due: (3, 2) is expected, and vector_coverage \
         answered {got:?}"
    );

    for title in failing {
        plug_text(title);
    }
    recomputed(&l).await?;
    let got = coverage(store.as_ref(), "after the redeclaration's retry").await?;
    ensure!(
        got == (5, 0),
        "`recompute` of `note` retries the two failures: (5, 0) is expected, and \
         vector_coverage answered {got:?}"
    );

    l.retire("note", "n1").await?;
    let got = coverage(store.as_ref(), "after a retire").await?;
    ensure!(
        got == (4, 0),
        "a retired note holds no vector the head counts: (4, 0) is expected, and \
         vector_coverage answered {got:?}"
    );

    let missing = store
        .vector_coverage(&lakelands_api::TypeName::from("nowhere"))
        .await;
    ensure!(
        matches!(missing, Err(lakelands_api::Error::NotFound { .. })),
        "vector_coverage of a type the lake does not hold is not found; it answered {missing:?}"
    );
    l.declare(type_decl("plain", vec![field("text", StorageKind::Text)]))
        .await?;
    let none = store
        .vector_coverage(&lakelands_api::TypeName::from("plain"))
        .await
        .must("asking vector_coverage of a type without embeddings")?;
    ensure!(
        none.is_empty(),
        "a type without embedding fields covers none; vector_coverage answered {none:?}"
    );
    Ok(())
}

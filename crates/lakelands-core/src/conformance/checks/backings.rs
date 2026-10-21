//! Fields computed by a function whose backing reaches outside the lake
//! (`docs/decisions/022-functions-are-declarations.md` and
//! `docs/decisions/002-determinism-is-the-boundary.md`, extended;
//! `docs/log-format.md` §2.5).
//!
//! The store calls such a backing after the write that made the call due,
//! records the answer as `output_recorded`, and fills the field from the
//! record, never from a second call. A replay that called the backing
//! again could answer differently, and a backing called twice for one
//! input costs twice and may act twice; both are why these checks count
//! the calls the backing received. The checks use the suite's scripted
//! backing ([`crate::conformance::backing`]), which every harness binds.

use lakelands_api::error::{Case, Seam};
use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind;
use lakelands_api::write::{Declaration, Recompute};
use lakelands_api::{
    Access, Error, Grants, LakeStore, Origin, Position, Principal, Refusal, Version,
};
use serde_json::{Value, json};

use crate::conformance::backing::{KIND, made, plug, unplug};
use crate::conformance::fixture::{
    Lake, Must, computed_by, derived, events_from, field, field_access, head_unmoved, invalid_case,
    reason, recorded_outputs, trigger_on, type_decl, value,
};
use crate::conformance::{Failure, Held, Store};

check!(
    UNBOUND_KIND_REFUSED = a_function_of_a_backing_kind_nobody_bound_is_refused_at_declare,
    "022, extended: a function computed outside the store is declared only where the host bound a backing of its kind; otherwise the declaration is refused as unbound, naming the kind, after its own checks (028, extended)"
);

check!(
    FILLED_AND_RECORDED = a_backed_field_is_filled_from_the_output_the_store_records,
    "022, extended, and the log format: the store calls the backing after the write, records the answer as output_recorded naming the field, and the write answers the position the filled field is visible from"
);

check!(
    ONE_CALL_PER_INPUT = the_same_input_is_never_sent_to_a_backing_twice,
    "002 and 022, extended: an output recorded for a call is read back for every later call with the same input, across records and after the input changed and came back"
);

check!(
    FAILURE_CLEARS = a_failing_backing_clears_the_field_and_records_the_failure,
    "022 and 010, extended: a failure is recorded as {\"error\": …}, clears the field it fills, puts the reason in the field's metadata, and is not retried for the same input"
);

check!(
    REBUILD_READS_OUTPUTS = a_rebuild_reads_recorded_outputs_back_without_calling_the_backing,
    "001 and 002: a rebuild folds the recorded outputs; with the backing refusing every call, the rebuilt field reads as before and no call is made"
);

check!(
    FILL_REACTS = a_filled_field_recomputes_what_reads_it_and_fires_its_triggers,
    "012 and 022, extended: the field an output fills is changed through the fold's ordinary path; a derived field reading it is recomputed and a trigger on it fires, caused by the output"
);

check!(
    COMPUTED_CYCLE_REFUSED = computed_fields_that_read_each_other_in_a_cycle_are_refused,
    "022, extended: a type's computed fields, derived and function-computed alike, read one another without a cycle, so a chain of fills ends; a cycle is refused at declare, naming its fields"
);

check!(
    RECORDED_FILL_REACTS = a_second_record_with_the_same_input_has_its_derived_fields_right,
    "022, extended: a field filled from an output the lake already holds is a change of that field; the derived fields reading it are recomputed in the same event"
);

check!(
    CHAIN_WAITS = a_backed_field_reading_another_waits_for_its_output,
    "022, extended: computed fields settle in the order of what they read; a backed field whose input is not computed yet is not called, so no output is recorded under an input that was never meant"
);

check!(
    OUTPUT_NOT_A_VALUE = an_output_its_fields_type_would_not_take_clears_the_field_naming_it,
    "022 and 011, extended: an output is held to its field's type as a write is, so a vector output with a component that is not finite as an `f32` (`1e300`) is recorded, fills nothing, clears the field, and puts in its metadata's `error` why, naming the field"
);

/// Declares `lookup`, a function of the suite's backing that echoes its
/// input `input`.
async fn lookup(store: &dyn LakeStore, input: &str, config: Value) -> Result<Lake, Failure> {
    echoing(store, "lookup", input, config).await
}

/// Declares `lookup`, a function of the suite's backing that echoes its
/// input `code`, and the type `place`, whose `name` it computes.
pub(super) async fn places(store: &dyn LakeStore, config: Value) -> Result<Lake, Failure> {
    let l = lookup(store, "code", config).await?;
    let name = computed_by("name", StorageKind::Text, "lookup", &["code"]);
    l.declare(type_decl(
        "place",
        vec![field("code", StorageKind::Text), name],
    ))
    .await?;
    Ok(Lake::of(store))
}

async fn a_function_of_a_backing_kind_nobody_bound_is_refused_at_declare(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    let function: Declaration = serde_json::from_value(json!({
        "declare": "function", "name": "geocode", "backing": "nowhere",
        "inputs": [{"name": "address", "type": {"kind": "text"}}],
        "config": {"url": "https://geo.example/lookup"}
    }))
    .map_err(|e| Failure::new(format!("the check's function does not read: {e}")))?;
    let answer = cat.declare(own, function.clone()).await;
    // The declaration's own checks before its backing (028, extended).
    let Declaration::Function(mut misnamed) = function else {
        unreachable!("a function declaration")
    };
    misnamed.name = "not a name".into();
    let first = cat.declare(own, Declaration::Function(misnamed)).await;
    ensure!(
        matches!(answer.as_ref().err().and_then(Error::seam), Some(Seam::FunctionBacking { kind }) if kind == "nowhere")
            && matches!(reason(&first), Some(Refusal::Invalid(..))),
        "no backing of the kind `nowhere` is bound, so declaring a function of that kind \
         must be refused as unbound, naming the kind, and one whose name is no name as \
         invalid first; it answered {answer:?} and {first:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused function declaration").await?;
    Ok(())
}

async fn a_backed_field_is_filled_from_the_output_the_store_records(store: Store) -> Held {
    let token = "filled_and_recorded";
    let l = places(store.as_ref(), json!({"token": token, "echo": "code"})).await?;
    let written = l.upsert("place", "p1", json!({"code": "AT"})).await?;

    let r = l.read("place", "p1").await?;
    ensure!(
        value(&r, "name") == Some(&json!("AT")),
        "`name` is computed by `lookup`, which answers its input; after writing code AT \
         it must read \"AT\", but reads {:?}",
        value(&r, "name")
    );
    let meta = r.meta.get("name");
    ensure!(
        meta.is_some_and(|m| m.origin == Origin::Derived
            && m.procedure.as_deref() == Some("lookup")
            && m.procedure_version.as_deref() == Some("1")
            && m.error.is_none()),
        "a filled field carries origin `derived`, the function `lookup` at its version (1) \
         and no error; \
         its metadata is {meta:?}"
    );
    ensure!(
        r.version == Version(1),
        "filling a computed field is not a write: `p1` was written once and is at version \
         1, not {:?}",
        r.version
    );

    let recorded = recorded_outputs(store.as_ref()).await?;
    let [event] = recorded.as_slice() else {
        return Err(Failure::new(format!(
            "one call was made, so the log holds one `output_recorded`; it holds {}",
            recorded.len()
        )));
    };
    let EventBody::OutputRecorded {
        seam,
        output,
        field: filled,
        ..
    } = &event.body
    else {
        return Err(Failure::new(
            "the feed answered another kind for `output_recorded`",
        ));
    };
    ensure!(
        seam == &format!("function:{KIND}:lookup")
            && output == &Some(json!("AT"))
            && filled
                .as_ref()
                .is_some_and(|(t, k, f)| t.as_str() == "place"
                    && k.as_str() == "p1"
                    && f.as_str() == "name"),
        "the output is recorded under the seam `function:{KIND}:lookup`, as \"AT\", naming \
         the field `place` `p1` `name`; the log holds seam {seam:?}, output {output:?}, field \
         {filled:?}"
    );
    ensure!(
        event.cause.is_some_and(|c| c < event.position)
            && event.procedure.as_ref().map(|p| p.name.as_str()) == Some("lookup"),
        "the recorded output names the write that made the call due as its cause and \
         `lookup` as its procedure; it has cause {:?} and procedure {:?}",
        event.cause,
        event.procedure
    );
    ensure!(
        written.position() == event.position,
        "the write answers the position its filled field is visible from, the recorded \
         output's ({}); it answered {}",
        event.position,
        written.position()
    );
    Ok(())
}

async fn the_same_input_is_never_sent_to_a_backing_twice(store: Store) -> Held {
    let token = "one_call_per_input";
    let l = places(store.as_ref(), json!({"token": token, "echo": "code"})).await?;
    for (key, code) in [("p1", "AT"), ("p2", "AT"), ("p1", "DE"), ("p1", "AT")] {
        l.upsert("place", key, json!({"code": code})).await?;
    }
    let calls = made(token);
    ensure!(
        calls == 2,
        "four writes asked for two inputs (AT, DE); the backing must have been called \
         once for each, but was called {calls} times"
    );
    for key in ["p1", "p2"] {
        let r = l.read("place", key).await?;
        ensure!(
            value(&r, "name") == Some(&json!("AT")),
            "`{key}` has code AT, so its `name` is the output recorded for AT, \"AT\"; it \
             reads {:?}",
            value(&r, "name")
        );
    }
    Ok(())
}

async fn a_failing_backing_clears_the_field_and_records_the_failure(store: Store) -> Held {
    let token = "failure_clears";
    places(
        store.as_ref(),
        json!({"token": token, "echo": "code", "fail_on": "XX"}),
    )
    .await?;
    let l = Lake::of(store.as_ref());
    l.upsert("place", "p1", json!({"code": "AT"})).await?;
    l.upsert("place", "p1", json!({"code": "XX"})).await?;

    let r = l.read("place", "p1").await?;
    ensure!(
        value(&r, "name").is_none(),
        "`lookup` fails for XX, so `name` must hold no value, not the value AT computed \
         before; it reads {:?}",
        value(&r, "name")
    );
    let meta = r.meta.get("name");
    // The backing's failure as its case says it: `lookup` answered 503.
    let remote = lakelands_api::error::Remote::Function {
        function: "lookup".into(),
    };
    let exchange = lakelands_api::error::Exchange::Status { status: 503 };
    let why = lakelands_api::error::Situation::Remote { remote, exchange }.to_string();
    ensure!(
        meta.is_some_and(|m| m.origin == Origin::Derived && m.error.as_deref() == Some(&why)),
        "a field cleared by a failure keeps metadata that says why (its `error`) though it \
         holds no value; the read with provenance answered {meta:?}"
    );
    let recorded = recorded_outputs(store.as_ref()).await?;
    let last = recorded.last().map(|e| &e.body);
    ensure!(
        matches!(last, Some(EventBody::OutputRecorded { output: Some(output), .. })
            if output.as_object().is_some_and(|o| o.len() == 1 && o["error"].is_string())),
        "the failure is recorded as its output, {{\"error\": \"<why>\"}}; the last \
         `output_recorded` is {last:?}"
    );

    l.upsert("place", "p2", json!({"code": "XX"})).await?;
    let calls = made(token);
    ensure!(
        calls == 2,
        "a failure is final for its input: `p2` with code XX is cleared from the recorded \
         failure, without a third call; the backing was called {calls} times"
    );
    let r = l.read("place", "p2").await?;
    ensure!(
        value(&r, "name").is_none() && r.meta.get("name").is_some_and(|m| m.error.is_some()),
        "`p2` reads the recorded failure: no `name`, and an error in its metadata; it \
         reads {r:?}"
    );
    Ok(())
}

async fn an_output_its_fields_type_would_not_take_clears_the_field_naming_it(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(json!({
        "declare": "function", "name": "lookup", "backing": KIND,
        "inputs": [{"name": "raw", "type": {"kind": "json"}}],
        "config": {"token": "output_not_a_value", "echo": "raw"}
    }))
    .await?;
    let mut at = computed_by("at", StorageKind::Vector, "lookup", &["raw"]);
    at.ty.dimensions = Some(2);
    let fields = vec![field("raw", StorageKind::Json), at];
    l.declare(type_decl("point", fields)).await?;
    l.upsert("point", "p1", json!({"raw": [0.6, 0.8]})).await?;
    l.upsert("point", "p1", json!({"raw": [1e300, 0]})).await?;
    let r = l.read("point", "p1").await?;
    let error = r.meta.get("at").and_then(|m| m.error.as_deref());
    ensure!(
        value(&r, "at").is_none() && error.is_some_and(|e| e.contains("`at`")),
        "`lookup` answers `[1e300, 0]`, which no `f32` holds, so `at` must hold no value, \
         not the `[0.6, 0.8]` before, and its metadata's `error` must name `at`; it reads \
         {:?} with the error {error:?}",
        value(&r, "at")
    );
    Ok(())
}

async fn a_rebuild_reads_recorded_outputs_back_without_calling_the_backing(store: Store) -> Held {
    let token = "rebuild_reads_outputs";
    let l = places(store.as_ref(), json!({"token": token, "echo": "code"})).await?;
    l.upsert("place", "p1", json!({"code": "AT"})).await?;
    let before = l.read("place", "p1").await?;
    let calls = made(token);

    unplug(token);
    store.rebuild().await.must("rebuilding the projection")?;
    let after = l.read("place", "p1").await?;
    ensure!(
        after == before,
        "a rebuild reads the recorded output back, so `p1` reads as before it.\n  before: \
         {before:?}\n  after:  {after:?}"
    );
    l.upsert("place", "p2", json!({"code": "AT"})).await?;
    let p2 = l.read("place", "p2").await?;
    ensure!(
        made(token) == calls && value(&p2, "name") == Some(&json!("AT")),
        "with the backing refusing every call, neither the rebuild nor a later write of the \
         same input may call it: {calls} calls before, {} after; `p2` reads {:?}",
        made(token),
        value(&p2, "name")
    );
    Ok(())
}

async fn a_filled_field_recomputes_what_reads_it_and_fires_its_triggers(store: Store) -> Held {
    let token = "fill_reacts";
    lookup(
        store.as_ref(),
        "code",
        json!({"token": token, "echo": "code"}),
    )
    .await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let name = computed_by("name", StorageKind::Text, "lookup", &["code"]);
    let greeting = greeting();
    l.declare(type_decl(
        "place",
        vec![field("code", StorageKind::Text), name, greeting],
    ))
    .await?;
    let fut = cat.declare(own, trigger_on("named", "place", Some("name = 'AT'")));
    fut.await.must("declaring the trigger `named` on `place`")?;
    l.upsert("place", "p1", json!({"code": "AT"})).await?;

    let r = l.read("place", "p1").await?;
    ensure!(
        value(&r, "greeting") == Some(&json!("AT!")),
        "`greeting` is concat(name, '!') and `name` was filled with \"AT\" by the output; \
         the fill is a change of `name`, so `greeting` must read \"AT!\", but reads {:?}",
        value(&r, "greeting")
    );
    let recorded = recorded_outputs(store.as_ref()).await?;
    let Some(output) = recorded.first() else {
        return Err(Failure::new(
            "no `output_recorded` in the log after the write",
        ));
    };
    let events = events_from(store.as_ref(), output.position).await?;
    let fired = events.iter().any(|e| {
        e.cause == Some(output.position)
            && matches!(&e.body, EventBody::TriggerFired { trigger, .. } if trigger.as_str() == "named")
    });
    ensure!(
        fired,
        "the fill made `name = 'AT'` hold, so the trigger `named` fires, caused by the \
         `output_recorded` at {}; the feed from there holds {:?}",
        output.position,
        events
            .iter()
            .map(|e| (e.position, e.body.kind(), e.cause))
            .collect::<Vec<_>>()
    );
    Ok(())
}

/// `greeting`, derived from `name` as concat(name, '!').
fn greeting() -> lakelands_api::ontology::FieldDecl {
    derived(
        "greeting",
        StorageKind::Text,
        "concat(name, '!')",
        &["name"],
    )
}

async fn computed_fields_that_read_each_other_in_a_cycle_are_refused(store: Store) -> Held {
    let token = "computed_cycle_refused";
    lookup(
        store.as_ref(),
        "label",
        json!({"token": token, "echo": "label"}),
    )
    .await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    let name = computed_by("name", StorageKind::Text, "lookup", &["label"]);
    let label = derived("label", StorageKind::Text, "concat(name, '-')", &["name"]);
    let answer = cat.declare(
        own,
        Declaration::Type(type_decl("place", vec![name, label])),
    );
    let answer = answer.await;
    let named_both = matches!(invalid_case(&answer), Some(Case::ComputedCycle { fields })
        if ["name", "label"].iter().all(|f| fields.0.iter().any(|n| n == f)));
    ensure!(
        named_both,
        "`name` is computed by `lookup` from `label`, and `label` is derived from `name`: \
         a cycle, which must be refused as invalid naming both fields; it answered {answer:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused cycle of computed fields").await?;
    Ok(())
}

/// Declares a function of the suite's backing, `name`, echoing `input`.
async fn echoing(
    store: &dyn LakeStore,
    name: &str,
    input: &str,
    config: Value,
) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare_json(json!({
        "declare": "function", "name": name, "backing": KIND,
        "inputs": [{"name": input, "type": {"kind": "text"}}],
        "config": config
    }))
    .await?;
    Ok(l)
}

async fn a_second_record_with_the_same_input_has_its_derived_fields_right(store: Store) -> Held {
    let token = "recorded_fill_reacts";
    let l = echoing(
        store.as_ref(),
        "lookup",
        "code",
        json!({"token": token, "echo": "code"}),
    )
    .await?;
    let greeting = greeting();
    l.declare(type_decl(
        "place",
        vec![
            field("code", StorageKind::Text),
            computed_by("name", StorageKind::Text, "lookup", &["code"]),
            greeting,
        ],
    ))
    .await?;
    l.upsert("place", "p1", json!({"code": "AT"})).await?;
    l.upsert("place", "p2", json!({"code": "AT"})).await?;
    let r = l.read("place", "p2").await?;
    ensure!(
        value(&r, "name") == Some(&json!("AT")) && value(&r, "greeting") == Some(&json!("AT!")),
        "`p2` has the code AT, whose output the lake already holds: `name` is filled from it \
         without a call, and `greeting`, derived from `name`, must read \"AT!\" in the same \
         event; `p2` reads name {:?}, greeting {:?}",
        value(&r, "name"),
        value(&r, "greeting")
    );
    ensure!(
        made(token) == 1,
        "one input, one call: the backing was called {} times",
        made(token)
    );
    Ok(())
}

async fn a_backed_field_reading_another_waits_for_its_output(store: Store) -> Held {
    let (first, second) = ("chain_waits_first", "chain_waits_second");
    echoing(
        store.as_ref(),
        "first",
        "code",
        json!({"token": first, "echo": "code"}),
    )
    .await?;
    // `second` fails on a null input: a call made before `a` was computed
    // would leave `b` with an error instead of a value.
    let l = echoing(
        store.as_ref(),
        "second",
        "a",
        json!({"token": second, "echo": "a", "fail_on": null}),
    )
    .await?;
    // `b` is declared before the field it reads.
    l.declare(type_decl(
        "place",
        vec![
            field("code", StorageKind::Text),
            computed_by("b", StorageKind::Text, "second", &["a"]),
            computed_by("a", StorageKind::Text, "first", &["code"]),
        ],
    ))
    .await?;
    l.upsert("place", "p1", json!({"code": "AT"})).await?;
    let r = l.read("place", "p1").await?;
    ensure!(
        value(&r, "a") == Some(&json!("AT")) && value(&r, "b") == Some(&json!("AT")),
        "`a` is computed from `code` and `b` from `a`: both must read \"AT\", `b` called \
         only once `a` was filled; `p1` reads a {:?}, b {:?}, with metadata {:?}",
        value(&r, "a"),
        value(&r, "b"),
        r.meta.get("b")
    );
    ensure!(
        made(first) == 1 && made(second) == 1,
        "one call for `a` and one for `b` with `a`'s value, never one with `a` absent; the \
         backing was called {} times for `a` and {} for `b`",
        made(first),
        made(second)
    );
    Ok(())
}

check!(
    FAILURE_RETRIED = a_redeclaration_retries_a_failed_call_once_and_the_later_output_holds,
    "022, extended: a settle forced by `recompute` calls again, once, for an input whose recorded output is a failure, for a function backing as for an embedding; the later output holds for every record with that input; and so does one forced by the function declared again with a change its call does not read (lakelands#935)"
);

async fn a_redeclaration_retries_a_failed_call_once_and_the_later_output_holds(
    store: Store,
) -> Held {
    let token = "failure_retried";
    let l = places(store.as_ref(), json!({"token": token, "echo": "code"})).await?;
    unplug(token);
    l.upsert("place", "p1", json!({"code": "AT"})).await?;
    l.upsert("place", "p2", json!({"code": "AT"})).await?;
    let failed = made(token);
    let r = l.read("place", "p1").await?;
    ensure!(
        failed == 1 && value(&r, "name").is_none(),
        "the backing refused every call, so `p1` and `p2`, both with code AT, hold no \
         `name` after one call; the backing was called {failed} times and `p1` reads {r:?}"
    );

    plug(token);
    // The door's caller is shown the type and not `name`: it is answered
    // of the type what it was answered before (lakelands#576's rule).
    let shown = field_access(store.as_ref(), "author", "place", &[("name", Access::None)]);
    let author = Principal {
        grants: Grants {
            schema: true,
            restricted: true,
            ..shown.grants.clone()
        },
        ..shown
    };
    let place = lakelands_api::TypeName::from("place");
    let described = || async {
        let seen = l.cat.describe_type(&author, &place, Default::default());
        let fields = seen.await.must("describing `place`")?.data.fields;
        Ok::<_, Failure>(format!("{fields:?}"))
    };
    let before = described().await?;
    let what = Recompute::Function("lookup".into());
    let done = l.cat.recompute(&author, what).await.must("the door")?;
    let after = described().await?;
    ensure!(
        after == before && !after.contains("\"name\"") && done.position() > Position::GENESIS,
        "the door's caller, shown `place` without `name`, is described the type as before \
         the call; it was described {before} and then {after}"
    );
    let calls = made(token) - failed;
    let (p1, p2) = (l.read("place", "p1").await?, l.read("place", "p2").await?);
    ensure!(
        calls == 1
            && value(&p1, "name") == Some(&json!("AT"))
            && value(&p2, "name") == Some(&json!("AT")),
        "`recompute` of `lookup` retries the failed call for code AT: one more call, not \
         {calls}, and its output fills `name` on both records; they read {:?} and {:?}",
        value(&p1, "name"),
        value(&p2, "name")
    );
    // By a type, the door appends the type as it was declared: a computed
    // field that listed no input lists none, where the lake completes it.
    let bare = computed_by("name", StorageKind::Text, "lookup", &[]);
    l.declare(type_decl(
        "spot",
        vec![field("code", StorageKind::Text), bare],
    ))
    .await?;
    let before = store.head().await.must("reading the head")?;
    let spot = Recompute::Type("spot".into());
    let fut = l.cat.recompute(&l.own, spot);
    fut.await.must("the door, by a type")?;
    let appended = events_from(store.as_ref(), before.next()).await?;
    let listed = appended.iter().find_map(|e| match &e.body {
        EventBody::Declared {
            declaration: Declaration::Type(t),
            ..
        } => Some(t.fields.iter().map(|f| f.depends_on.len()).sum::<usize>()),
        _ => None,
    });
    ensure!(
        listed == Some(0),
        "the door appends `spot` as it was declared, its computed field listing no input; \
         the event's fields list {listed:?}"
    );
    // A declaration that changed what its call does not read forces a
    // settle that calls again for a recorded failure, as `recompute` does.
    unplug(token);
    l.upsert("place", "p3", json!({"code": "BE"})).await?;
    plug(token);
    let failed = made(token);
    l.declare_json(
        json!({"declare": "function", "name": "lookup", "backing": KIND,
        "inputs": [{"name": "code", "type": {"kind": "text"}}], "description": "a lookup",
        "config": {"token": token, "echo": "code"}}),
    )
    .await?;
    let p3 = l.read("place", "p3").await?;
    ensure!(
        made(token) - failed == 1 && value(&p3, "name") == Some(&json!("BE")),
        "`lookup` declared again with a description retries the failed call for code BE once \
         and fills `p3`; it called {} times and `p3` reads {:?}",
        made(token) - failed,
        value(&p3, "name")
    );
    Ok(())
}

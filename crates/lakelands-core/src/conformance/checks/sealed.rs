//! What the log's own reading would not pass is refused where it enters
//! (`docs/log-format.md` §1.2 and §1.3).
//!
//! A field's value is judged by its door's check
//! ([`super::values::WRITTEN_FORM_AT_EVERY_DOOR`]). These are the other
//! places a value is written as it was given: what a source states of a
//! record beside its fields, a function's `config`, an output of a seam
//! and an executor's answer. Each row asserts the place its door names.
//! The store's seal refuses such an event too, whatever door it came
//! through, and names no place: a row here is red when only the seal
//! holds.

use lakelands_api::record::Upstream;

use crate::canonical::integer_too_large;
use crate::conformance::backing::{EMBED_MODEL, EXECUTOR, KIND, VAST, executed};
use crate::conformance::fixture::{Answer, source_record};
use crate::conformance::prelude::*;

check!(
    SEEN_AT_IN_THE_YEARS = an_ingest_whose_seen_at_leaves_the_years_is_refused_naming_it,
    "the log format §1.3: a source's `seen_at` is written as it was given, so one out of the years 0000 to 9999, which an instant built in process can be, is refused as no date-time at `seen_at` with its window, appending nothing; the first and the last microsecond of those years are written and read back, and the log still verifies and rebuilds"
);

check!(
    UPSTREAM_POSITION_EXACT =
        an_ingest_whose_upstream_position_no_reader_reads_exactly_is_refused_naming_it,
    "the log format §1.2: an upstream position is a number of the envelope, so one beyond ±(2^53 − 1) is refused at `upstream.position` with its window, appending nothing; 2^53 − 1 is written and read back"
);

check!(
    DECLARATION_EXACT =
        a_declaration_that_holds_an_integer_no_reader_reads_exactly_is_refused_at_its_member,
    "the log format §1.2: a declaration is written as it was declared, so a number anywhere in it written as an integer beyond ±(2^53 − 1) is refused at the declaration and the member that holds it, by its path, declared or proposed, appending nothing: a function's `config` (`1e16`, `9007199254740993` in a list), the duration literal of an expression in an object set's `where` and in a field's `derived`, in typed form at the literal and in text form (`104250d`, which parses to one) at the member that holds the text, and a field's `initial`; the largest integer, `1e21` and `104249d` are declared"
);

check!(
    OUTPUT_EXACT =
        an_output_that_holds_an_integer_no_reader_reads_exactly_is_recorded_as_its_calls_failure,
    "the log format §1.2 and 022, extended: a seam's output is recorded as the seam gave it, so a backing's value and an embedding's vector that hold an integer beyond ±(2^53 − 1) are recorded as that call's failure: the field holds no value, its metadata's `error` names the number, and no event holds it",
    embedding
);

check!(
    ANSWER_EXACT =
        an_executors_answer_that_holds_an_integer_no_reader_reads_exactly_is_lost_and_said_so,
    "the log format §1.2 and 013, extended: an executor's answer is recorded as it was given, so one that holds an integer beyond ±(2^53 − 1) is not recorded: the executor ran once, the call is answered `unavailable` with the answer and the number that kept it out of the log, no `output_recorded` follows the call, and a retry under its key is `unanswered` and runs nothing"
);

/// The case of an answer refused as invalid at exactly `places`: what a
/// door says of a value it names. The seal names no place.
fn said_at<'a>(a: &'a impl Answer, places: &[Place]) -> Option<&'a Case> {
    invalid_said(a)
        .filter(|said| said.at == places)
        .map(|said| &said.case)
}

fn member(path: &str) -> Place {
    Place::Member { path: path.into() }
}

/// The largest integer every reader reads exactly.
const MAX: u64 = (1 << 53) - 1;

/// `reading`, whose `v` a source writes.
async fn readings(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let v = sourced("v", StorageKind::Number);
    l.declare(type_decl("reading", vec![v])).await?;
    Ok(l)
}

async fn an_ingest_whose_seen_at_leaves_the_years_is_refused_naming_it(store: Store) -> Held {
    let l = readings(store.as_ref()).await?;
    let edge = |text: &str| Timestamp::parse(text).expect("an instant within the years");
    let first = edge("0000-01-01T00:00:00.000000Z");
    let last = edge("9999-12-31T23:59:59.999999Z");
    for (key, at) in [("first", first), ("last", last)] {
        let mut r = source_record("reading", key, json!({"v": 1}));
        r.seen_at = Some(at);
        let fed = l.ingest("meter", vec![r]).await;
        fed.must(&format!("ingesting `{key}`, seen at {at}"))?;
        let seen = l
            .read("reading", key)
            .await?
            .meta
            .get("v")
            .map(|m| m.seen_at);
        ensure!(
            seen == Some(Some(at)),
            "`{key}` was seen at {at}, an instant of the years the one form writes: its `v` \
             must read that `seen_at`; it reads {seen:?}"
        );
    }
    let before = store.head().await.must("reading the head")?;
    for at in [Timestamp(first.0 - 1), Timestamp(last.0 + 1)] {
        let mut r = source_record("reading", "out", json!({"v": 2}));
        r.seen_at = Some(at);
        // A record the door would pass comes first: the window is refused whole.
        let window = vec![source_record("reading", "in", json!({"v": 3})), r];
        let fed = l.ingest("meter", window).await;
        ensure!(
            matches!(
                said_at(&fed, &[member("seen_at")]),
                Some(Case::NotDateTime { .. })
            ),
            "a record seen at {at}, out of the years 0000 to 9999, must be refused as no \
             date-time at `seen_at`; the ingest answered {fed:?}"
        );
    }
    head_unmoved(store.as_ref(), before, "the ingests out of the years").await?;
    log_events(store.as_ref()).await?;
    let verified = store.verify(None, None).await;
    verified.must("verifying the log after the refused ingests")?;
    let rebuilt = store.rebuild().await;
    rebuilt.must("rebuilding after the refused ingests")?;
    Ok(())
}

async fn an_ingest_whose_upstream_position_no_reader_reads_exactly_is_refused_naming_it(
    store: Store,
) -> Held {
    let l = readings(store.as_ref()).await?;
    let from = |position: u64| Upstream {
        lake: "other".into(),
        position: Position(position),
    };
    let mut r = source_record("reading", "exact", json!({"v": 1}));
    r.upstream = Some(from(MAX));
    let fed = l.ingest("mirror", vec![r]).await;
    fed.must("ingesting a record read at the upstream position 2^53 − 1")?;
    let read = l.read("reading", "exact").await?;
    let upstream = read.meta.get("v").and_then(|m| m.upstream.clone());
    ensure!(
        upstream == Some(from(MAX)),
        "`exact` was read at the upstream position 2^53 − 1, which every reader reads: its \
         `v` must read that upstream; it reads {upstream:?}"
    );
    let before = store.head().await.must("reading the head")?;
    let mut r = source_record("reading", "beyond", json!({"v": 2}));
    r.upstream = Some(from(MAX + 1));
    let window = vec![source_record("reading", "in", json!({"v": 3})), r];
    let fed = l.ingest("mirror", window).await;
    let at = [member("upstream.position")];
    ensure!(
        matches!(said_at(&fed, &at), Some(Case::IntegerTooLarge { .. })),
        "a record read at the upstream position 2^53 must be refused as an integer beyond \
         ±(2^53 − 1) at `upstream.position`; the ingest answered {fed:?}"
    );
    head_unmoved(store.as_ref(), before, "the ingest beyond the range").await
}

async fn a_declaration_that_holds_an_integer_no_reader_reads_exactly_is_refused_at_its_member(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let read = |d: Value| -> Declaration { serde_json::from_value(d).expect("a declaration") };
    let function = |name: &str, config: Value| {
        read(
            json!({"declare": "function", "name": name, "backing": KIND, "config": config,
                   "inputs": [{"name": "code", "type": {"kind": "text"}}]}),
        )
    };
    // An expression in typed form whose duration literal is `micros`.
    let typed = |text: &str, micros: u64| {
        let e = crate::expr::parse(text).expect("the check's expression");
        let mut e = serde_json::to_value(e).expect("an expression serialises");
        e["rhs"]["value"]["value"] = json!(micros);
        e
    };
    let set = |name: &str, filter: Value| {
        read(json!({"declare": "object_set", "name": name, "of": "parcel", "where": filter}))
    };
    let old = |micros: u64| typed("known_at(sent) - known_at(code) > 1d", micros);
    let late = |derived: Value, initial: f64| {
        read(json!({"declare": "type", "name": "parcel", "fields": [
            {"name": "code", "type": {"kind": "text"}},
            {"name": "sent", "type": {"kind": "date_time"}},
            {"name": "late", "type": {"kind": "date_time"}, "authority": "derived",
             "derived": derived, "depends_on": ["sent"]},
            {"name": "weight", "type": {"kind": "number"}, "initial": initial}]}))
    };
    // 104249d is the largest whole number of days a duration holds.
    for fits in [
        function("fits", json!({"most": MAX, "more": 1e21})),
        late(typed("sent + 1d", MAX), MAX as f64),
        set("old", old(MAX)),
        set("older", json!("known_at(sent) - known_at(code) > 104249d")),
    ] {
        l.declare_as(fits, "a declaration within the range").await?;
    }
    let before = store.head().await.must("reading the head")?;
    let (vast, ancient, parcel) = (
        Place::Function {
            name: "vast".into(),
        },
        Place::ObjectSet {
            name: "ancient".into(),
        },
        Place::Type {
            name: "parcel".into(),
        },
    );
    for (d, place, path) in [
        (function("vast", json!({"big": 1e16})), &vast, "config.big"),
        (
            function("vast", json!({"nested": [MAX + 2]})),
            &vast,
            "config.nested[0]",
        ),
        (
            set("ancient", old(MAX + 2)),
            &ancient,
            "where.rhs.value.value",
        ),
        (
            set(
                "ancient",
                json!("known_at(sent) - known_at(code) > 104250d"),
            ),
            &ancient,
            "where",
        ),
        (
            late(typed("sent + 1d", MAX + 2), 1.0),
            &parcel,
            "fields[2].derived.rhs.value.value",
        ),
        (
            late(json!("sent + 104250d"), 1.0),
            &parcel,
            "fields[2].derived",
        ),
        (late(json!("sent + 1d"), 1e16), &parcel, "fields[3].initial"),
    ] {
        let declared = l.cat.declare(&l.own, d.clone()).await;
        let write = ProposedWrite::Declare {
            declarations: vec![d],
            plan: None,
        };
        let proposed = l.cat.propose(&l.own, write, "later".into()).await;
        let at = [place.clone(), member(path)];
        for (door, got) in [("declared", declared), ("proposed", proposed)] {
            ensure!(
                matches!(said_at(&got, &at), Some(Case::IntegerTooLarge { .. })),
                "{place} {door} with an integer beyond ±(2^53 − 1) at `{path}` must be refused \
                 there, naming the declaration and that member; it answered {got:?}"
            );
        }
    }
    head_unmoved(store.as_ref(), before, "the declarations").await
}

/// Fail unless `key` of `ty` holds no `f` and says in its metadata that
/// its output held an integer beyond the range.
async fn failed_as_too_large(l: &Lake, ty: &str, key: &str, f: &str) -> Held {
    let r = l.read(ty, key).await?;
    let error = r.meta.get(f).and_then(|m| m.error.as_deref());
    ensure!(
        value(&r, f).is_none() && error.is_some_and(|e| e.contains("beyond ±(2^53 − 1)")),
        "the output for `{f}` of `{ty}` `{key}` holds an integer no reader reads exactly: \
         the field must hold no value and its metadata's `error` say so; it reads {:?} and \
         {error:?}",
        value(&r, f)
    );
    Ok(())
}

async fn an_output_that_holds_an_integer_no_reader_reads_exactly_is_recorded_as_its_calls_failure(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(json!({
        "declare": "function", "name": "weigh", "backing": KIND,
        "inputs": [{"name": "code", "type": {"kind": "text"}}],
        "config": {"token": "output_exact", "echo": "code", "vast_on": "XX"}
    }))
    .await?;
    let fields = vec![
        field("code", StorageKind::Text),
        computed_by("weight", StorageKind::Json, "weigh", &["code"]),
        embedding("near", 2, &["code"], EMBED_MODEL),
    ];
    l.declare(type_decl("parcel", fields)).await?;
    // The backing answers 1e16 for XX; the embedding of a text starting
    // with `vast` has 1e16 as its first component.
    let vast = format!("{VAST} parcel");
    l.upsert("parcel", "weighed", json!({"code": "XX"})).await?;
    l.upsert("parcel", "embedded", json!({ "code": vast }))
        .await?;
    failed_as_too_large(&l, "parcel", "weighed", "weight").await?;
    failed_as_too_large(&l, "parcel", "embedded", "near").await?;
    let events = log_events(store.as_ref()).await?;
    let failures = events.iter().filter(|e| {
        matches!(&e.body, EventBody::OutputRecorded { output: Some(o), .. }
            if o["error"].as_str().is_some_and(|e| e.contains("beyond ±(2^53 − 1)")))
    });
    let held = events.iter().find(|e| {
        let event = serde_json::to_value(e).expect("an event serialises");
        integer_too_large(&event).is_some()
    });
    ensure!(
        failures.count() == 2 && held.is_none(),
        "each of the two outputs is recorded as its call's failure, \
         {{\"error\": \"… beyond ±(2^53 − 1) …\"}}, and no event holds the integer; the log \
         holds {events:?}"
    );
    Ok(())
}

async fn an_executors_answer_that_holds_an_integer_no_reader_reads_exactly_is_lost_and_said_so(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let n = field("n", StorageKind::Number);
    l.declare(type_decl("bill", vec![n])).await?;
    l.declare_json(json!({"declare": "verb", "name": "settle", "on": "bill",
        "risk": "low", "effect": "external", "executor": EXECUTOR,
        "payload": [{"name": "vast", "type": {"kind": "boolean"}}]}))
        .await?;
    let key = "answer-exact";
    l.upsert("bill", key, json!({"n": 1})).await?;
    let before = store.head().await.must("reading the head")?;
    let call = || {
        let opts = WriteOpts {
            idempotency_key: Some("settle-once".into()),
            ..WriteOpts::default()
        };
        let (verb, key) = ("settle".into(), key.into());
        let (cat, own, payload) = (l.cat.clone(), l.own.clone(), patch(json!({"vast": true})));
        async move { cat.call(&own, &verb, Some(&key), payload, opts).await }
    };
    let first = call().await;
    let lost = match &first {
        Err(Error::Unavailable { situation }) => match situation.as_ref() {
            Situation::AnswerLost { answered, cause } => {
                *answered == json!({"paid": 1e16})
                    && matches!(
                        said_at(cause.as_ref(), &[]),
                        Some(Case::IntegerTooLarge { .. })
                    )
            }
            _ => false,
        },
        _ => false,
    };
    ensure!(
        lost && executed(key) == 1,
        "the executor answered {{\"paid\": 1e16}}, which the log does not write: the call \
         must be answered `unavailable`, its answer lost, carrying the answer and the number \
         as its cause, after one run; it answered {first:?} after {} runs",
        executed(key)
    );
    let recorded = outputs_from(store.as_ref(), before).await?;
    let again = call().await;
    let unanswered = matches!(
        &again,
        Err(Error::Refused { reason: Refusal::Unanswered(said), .. })
            if matches!(said.case, Case::Unanswered { .. })
    );
    ensure!(
        recorded.is_empty() && unanswered && executed(key) == 1,
        "an answer that was not recorded is an answer never given: no `output_recorded` \
         follows the call, and a retry under its key is `unanswered` and runs nothing; the \
         log holds {recorded:?}, the retry answered {again:?} after {} runs",
        executed(key)
    );
    Ok(())
}

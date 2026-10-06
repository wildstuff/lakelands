//! What the fold of a `declared` event that computes derived fields again
//! leaves (lakelands#523): the fold keeps the event, not the records it
//! changed. Its records are walked once for every such event that is
//! pending, before the next event that reads a record and before the
//! commit; a record the transaction did not write is read later from the
//! view and taken through every step kept so far.
//!
//! As in `fold_steps.rs`, each test appends hand-built drafts, in one
//! transaction, over a projection that already holds records, on the
//! memory engine behind the wrapper of `probed`, and states the fold's
//! result literally. Where a row needs a later reader that shows the step,
//! it is `record_restricted`, which keeps the fields as it read them: a
//! write computes every derived field again and would hide the step. The
//! rows are numbered as the design numbers them.

mod probed;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::event::{EventBody, Procedure};
use lakelands_api::ontology::StorageKind::{Boolean, Decimal, Number, Text};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::record::{FieldMeta, Origin, StoredRecord, Upstream};
use lakelands_api::seam::{FunctionBacking, FunctionCall, Seams};
use lakelands_api::write::{Declaration, Migration, Recompute, Rekey};
use lakelands_api::{
    Error, FieldName, LakeStore, Location, Position, Timestamp, Version, WriteContext, WriteOrigin,
};
use lakelands_core::conformance::fixture::{
    self, computed_by, derived, embedding, field, patch, type_decl,
};
use lakelands_engine_mem::MemoryOpener;
use lakelands_store::backed::{self, Call, Of, Owed};
use lakelands_store::lake::Lake;
use lakelands_store::log::Draft;
use probed::{Asked, Probe, ProbedOpener, calls};
use serde_json::{Value, json};

/// A lake on the probed memory engine, written by hand-built events.
struct Folded {
    lake: Lake,
    probe: Arc<Probe>,
    /// The engine's opener, for a second handle on the same lake.
    opener: MemoryOpener,
    name: String,
}

impl Folded {
    fn new(name: &str) -> Folded {
        let (probe, opener) = (Arc::new(Probe::default()), MemoryOpener::new());
        let seams = Seams {
            engines: vec![Arc::new(ProbedOpener(opener.clone(), probe.clone()))],
            ..Seams::default()
        };
        let lake = Lake::open(&Location::memory(name), seams, OpenMode::Create).expect("a lake");
        Folded {
            lake,
            probe,
            opener,
            name: name.to_owned(),
        }
    }

    /// One append, folded in one transaction; the events' positions.
    fn try_append(&self, drafts: Vec<Draft>) -> lakelands_api::Result<Vec<Position>> {
        let mut w = self.lake.for_write()?;
        let events = self.lake.append(&mut w, drafts)?;
        Ok(events.iter().map(|e| e.position).collect())
    }

    fn append(&self, drafts: Vec<Draft>) -> Vec<Position> {
        let appended = self.try_append(drafts);
        appended.unwrap_or_else(|e| panic!("{}: the append must fold: {e}", self.name))
    }

    /// One append of events the store writes for itself.
    fn bodies(&self, bodies: Vec<EventBody>) -> Vec<Position> {
        let now = self.lake.clock(&mut self.lake.writer());
        self.append(bodies.into_iter().map(|b| Draft::system(now, b)).collect())
    }

    /// A record as the projection holds it at `at` (`None`: the head).
    fn record(&self, at: Option<Position>, ty: &str, key: &str) -> Option<StoredRecord> {
        let view = self.lake.engine.view(at).expect("a view");
        view.get(&ty.into(), &key.into()).expect("a read")
    }

    /// A record's fields, version and position at `at`; `ended` for an
    /// ended key, `absent` for a key never written.
    fn state(&self, at: Option<Position>, ty: &str, key: &str) -> Value {
        match self.record(at, ty, key) {
            None => json!("absent"),
            Some(r) if r.is_ended() => json!("ended"),
            Some(r) => json!([r.fields, r.version.0, r.position.0]),
        }
    }

    /// A field's provenance at `at`.
    fn meta(&self, at: Option<Position>, ty: &str, key: &str, name: &str) -> Option<FieldMeta> {
        let r = self.record(at, ty, key).expect("the record");
        r.meta.get(&FieldName::from(name)).cloned()
    }

    /// The calls that are due, taken: cause, type, key, field, the call's
    /// inputs, and whether a recorded failure is called again.
    fn due(&self) -> Vec<(u64, String, String, String, Value, bool)> {
        let due = calls(&self.lake);
        let one = |d: backed::Due| {
            let inputs = match &d.call {
                Call::Function { call, .. } => call.inputs.clone(),
                Call::Embed { text, .. } => json!(text),
            };
            let (ty, key, field) = (d.ty.to_string(), d.key.to_string(), d.field.to_string());
            (d.cause.0, ty, key, field, inputs, d.retry)
        };
        due.into_iter().map(one).collect()
    }

    /// The state of every record of `keys`, as of every position up to the
    /// head.
    fn history(&self, keys: &[(&str, &str)]) -> Vec<Value> {
        let head = self.lake.head().0;
        let at = |p| {
            keys.iter()
                .map(move |(ty, key)| (Some(Position(p)), *ty, *key))
        };
        let states = (0..=head).flat_map(at);
        states
            .map(|(at, ty, key)| self.state(at, ty, key))
            .collect()
    }
}

/// A time far past the wall clock, so that a draft's own `known_at` never
/// lies before the event before it.
const LATER: i64 = 4_000_000_000_000_000;

/// An event of `who` with a time, a source, an upstream, a procedure and a
/// context of its own, each told by `n`: nothing of it is what another
/// event carries.
fn own(n: u32, who: &str, body: EventBody) -> Draft {
    Draft {
        writer: who.into(),
        source: Some(format!("source-{n}").as_str().into()),
        seen_at: Some(Timestamp(i64::from(n))),
        upstream: Some(upstream(n)),
        procedure: Some(Procedure {
            name: "by hand".into(),
            version: format!("p{n}"),
        }),
        context: context(n),
        ..Draft::system(Timestamp(LATER + i64::from(n)), body)
    }
}

fn upstream(n: u32) -> Upstream {
    Upstream {
        lake: "elsewhere".into(),
        position: Position(u64::from(n)),
    }
}

fn context(n: u32) -> WriteContext {
    WriteContext {
        origin: WriteOrigin::Scheduled,
        depth: n,
        cause: None,
    }
}

/// The provenance `own(n, who, ..)` gives a field its fold computes, at
/// `position`: by a function at `version`, or else by the event's own
/// procedure.
fn stamped(n: u32, who: &str, position: Position, version: Option<&str>) -> FieldMeta {
    FieldMeta {
        origin: Origin::Derived,
        known_at: Timestamp(LATER + i64::from(n)),
        seen_at: Some(Timestamp(i64::from(n))),
        position,
        writer: who.into(),
        source: Some(format!("source-{n}").as_str().into()),
        procedure_version: Some(version.map_or(format!("p{n}"), str::to_owned)),
        upstream: Some(upstream(n)),
        error: None,
    }
}

fn system(body: EventBody) -> Draft {
    Draft::system(Timestamp(LATER + 1_000), body)
}

fn declared(decl: TypeDecl, version: u32) -> EventBody {
    EventBody::Declared {
        schema_hash: Some(lakelands_core::shape::schema_hash(&decl).expect("a schema hash")),
        declaration: Declaration::Type(decl),
        version,
        with_the_set: false,
    }
}

fn declaration(d: Value, version: u32) -> EventBody {
    EventBody::Declared {
        declaration: serde_json::from_value(d).expect("a declaration"),
        schema_hash: None,
        version,
        with_the_set: false,
    }
}

/// The function `name`, computed in the lake as `expr` over the number `n`.
fn expr(name: &str, version: u32, expr: &str) -> EventBody {
    let f = json!({"declare": "function", "name": name, "backing": "expr", "expr": expr,
                   "inputs": [{"name": "n", "type": {"kind": "number"}}]});
    declaration(f, version)
}

/// The function `f`, computed outside the lake from `title`, as a
/// declaration: with `config`, which is part of the call it makes.
fn outside(config: Value) -> Value {
    json!({"declare": "function", "name": "f", "backing": "http", "config": config,
           "inputs": [{"name": "title", "type": {"kind": "text"}}]})
}

fn http(version: u32, config: Value) -> EventBody {
    declaration(outside(config), version)
}

/// The seam and the digest of the call `f` makes of `title` under `config`.
fn call_of(config: Value, title: &str) -> (String, lakelands_api::event::Digest) {
    let Declaration::Function(f) = serde_json::from_value(outside(config)).expect("a function")
    else {
        panic!("a function");
    };
    let fields = patch(json!({"title": title}));
    (
        backed::seam(&f),
        backed::digest(&backed::call_of(&f, &fields)),
    )
}

/// An `output_recorded` of `f`'s call of `title` under `config`.
fn output(config: Value, title: &str, output: Value, fills: Option<(&str, &str)>) -> EventBody {
    let (seam, input) = call_of(config, title);
    EventBody::OutputRecorded {
        seam,
        input,
        output,
        field: fills.map(|(ty, key)| (ty.into(), key.into(), "sum".into())),
    }
}

fn wrote(ty: &str, key: &str, version: u64, fields: Value) -> EventBody {
    EventBody::RecordUpserted {
        ty: ty.into(),
        key: key.into(),
        version: Version(version),
        patch: patch(fields),
        made: false,
    }
}

fn restricted(ty: &str, key: &str, version: u64) -> EventBody {
    EventBody::RecordRestricted {
        ty: ty.into(),
        key: key.into(),
        version: Version(version),
        reason: "held".into(),
    }
}

fn retired(ty: &str, key: &str, version: u64) -> EventBody {
    EventBody::RecordRetired {
        ty: ty.into(),
        key: key.into(),
        version: Version(version),
    }
}

/// `doc` with the number `n` and `twice`, which `f` computes from it.
fn doubled() -> TypeDecl {
    let twice = computed_by("twice", Number, "f", &["n"]);
    type_decl("doc", vec![field("n", Number), twice])
}

/// `name` with `title` and `sum`, which `f` computes from it outside the
/// lake.
fn summed(name: &str) -> TypeDecl {
    let sum = computed_by("sum", Text, "f", &["title"]);
    type_decl(name, vec![field("title", Text), sum])
}

/// A lake that holds `f` as `n * 2`, `doc` and the records `docs`, each
/// with its `n`.
fn doubling(name: &str, docs: &[(&str, i64)]) -> (Folded, Vec<Position>) {
    let l = Folded::new(name);
    let mut load = vec![expr("f", 1, "n * 2"), declared(doubled(), 1)];
    load.extend(
        docs.iter()
            .map(|(k, n)| wrote("doc", k, 1, json!({"n": n}))),
    );
    let p = l.bodies(load);
    (l, p)
}

/// A lake that holds `f`, computed outside the lake, `doc` and `memo`, and
/// the records `held` (type, key, title), none with an output recorded.
fn summing(name: &str, held: &[(&str, &str, &str)]) -> Folded {
    let l = Folded::new(name);
    let mut load = vec![
        http(1, json!(1)),
        declared(summed("doc"), 1),
        declared(summed("memo"), 1),
    ];
    let records = held.iter();
    load.extend(records.map(|(ty, k, title)| wrote(ty, k, 1, json!({"title": title}))));
    l.bodies(load);
    l.due();
    l
}

#[test]
fn row_1_a_later_read_finds_the_record_as_the_step_left_it() {
    let (l, _) = doubling("recompute-1", &[("a", 3)]);
    let q = l.append(vec![
        own(1, "user:ada", expr("f", 2, "n * 3")),
        own(2, "user:bob", restricted("doc", "a", 2)),
    ]);
    let a = l.record(None, "doc", "a").expect("the record");
    assert_eq!(
        (json!(a.fields), a.version.0, a.position, a.restricted),
        (json!({"n": 3, "twice": 9}), 2, q[1], true),
        "row 1: the restriction lands on the record as the function's new version computed it"
    );
    assert_eq!(
        l.meta(None, "doc", "a", "twice"),
        Some(stamped(1, "user:ada", q[0], Some("2"))),
        "row 1: the field's provenance is the `declared`'s, member by member, not the reader's"
    );
    let then = l.record(Some(q[0]), "doc", "a").expect("the record");
    assert_eq!(
        (
            json!(then.fields),
            then.version.0,
            then.position,
            then.restricted
        ),
        (json!({"n": 3, "twice": 9}), 1, q[0], false),
        "row 1: as of the `declared` the record is computed again and not restricted"
    );
    assert_eq!(
        then.meta.get(&FieldName::from("twice")),
        Some(&stamped(1, "user:ada", q[0], Some("2"))),
        "row 1: as of the `declared` the provenance is its own, member by member"
    );
}

#[test]
fn row_2_a_record_the_transaction_wrote_is_due_from_what_was_written() {
    let l = summing("recompute-2", &[("doc", "a", "A"), ("doc", "c", "C")]);
    let q = l.bodies(vec![
        wrote("doc", "a", 2, json!({"title": "fresh"})),
        http(2, json!(1)),
    ]);
    let title = |t: &str| json!({"title": t});
    let due = |at: Position, key: &str, t: &str| {
        let (ty, key, field) = ("doc".to_owned(), key.to_owned(), "sum".to_owned());
        (at.0, ty, key, field, title(t), false)
    };
    assert_eq!(
        l.due(),
        [
            due(q[0], "a", "fresh"),
            due(q[1], "a", "fresh"),
            due(q[1], "c", "C")
        ],
        "row 2: the write makes its call due, the declaration then every record's: the written \
         one from what was written, never the view's `A`"
    );
}

#[test]
fn row_2a_a_written_record_that_makes_nothing_due_hides_the_views_row() {
    let l = summing("recompute-2a", &[("doc", "a", "A"), ("doc", "c", "C")]);
    l.probe.take();
    let q = l.bodies(vec![retired("doc", "a", 2), http(2, json!(1))]);
    let due: Vec<_> = l.due().into_iter().map(|d| (d.0, d.2, d.4)).collect();
    assert_eq!(
        due,
        [(q[1].0, "c".to_owned(), json!({"title": "C"}))],
        "row 2a: `a` was retired by the transaction: the view's live row at its key makes no \
         call due"
    );
    let puts = |asked: Vec<Asked>| -> Vec<Asked> {
        let put = |a: &Asked| matches!(a, Asked::Put(..));
        asked.into_iter().filter(put).collect()
    };
    assert_eq!(
        puts(l.probe.take()),
        [Asked::Put("doc".into(), "a".into(), q[0])],
        "row 2a: the retired record is put once, by its own event"
    );
}

#[test]
fn row_3_an_output_recorded_later_fills_the_record_the_step_cleared() {
    let l = summing("recompute-3", &[("doc", "a", "A")]);
    l.bodies(vec![output(json!(1), "A", json!("S1"), Some(("doc", "a")))]);
    // The new version's configuration makes another call: no output is
    // recorded for it yet.
    let q = l.append(vec![
        own(1, "user:ada", http(2, json!(2))),
        own(
            2,
            "user:bob",
            output(json!(2), "A", json!("S2"), Some(("doc", "a"))),
        ),
    ]);
    assert_eq!(
        l.state(None, "doc", "a"),
        json!([{"title": "A", "sum": "S2"}, 1, q[1].0]),
        "row 3: the output fills the field the step cleared"
    );
    let filled = FieldMeta {
        upstream: None,
        ..stamped(2, "user:bob", q[1], Some("2"))
    };
    assert_eq!(l.meta(None, "doc", "a", "sum"), Some(filled), "row 3");
    assert_eq!(
        (
            l.state(Some(q[0]), "doc", "a"),
            l.meta(Some(q[0]), "doc", "a", "sum")
        ),
        (json!([{"title": "A"}, 1, q[0].0]), None),
        "row 3: as of the `declared` the field is cleared, at the `declared`'s position"
    );
    let owed: Vec<_> = (l.lake.writer().owed.iter())
        .map(|o| {
            (
                o.cause,
                o.ty.to_string(),
                o.context,
                matches!(o.of, Of::Sweep { .. }),
            )
        })
        .collect();
    assert_eq!(
        owed,
        [(q[0], "doc".to_owned(), context(1), true)],
        "row 3: the step leaves its type owed, with the `declared`'s context; the later read \
         through the step owes nothing"
    );
}

#[test]
fn row_3_an_output_for_the_call_the_step_made_finds_its_record() {
    // `m` is computed in the lake by `g`, and `f`, outside the lake, reads
    // `m`: `g` declared again changes the call the record makes of `f`.
    // The output of that call fills the record only as the step left it;
    // the view's row still makes the call before.
    let f = json!({"declare": "function", "name": "f", "backing": "http",
                   "inputs": [{"name": "m", "type": {"kind": "number"}}]});
    let recorded = |m: i64, output: &str| {
        let Declaration::Function(f) = serde_json::from_value(f.clone()).expect("a function")
        else {
            panic!("a function");
        };
        let call = backed::call_of(&f, &patch(json!({"m": m})));
        EventBody::OutputRecorded {
            seam: backed::seam(&f),
            input: backed::digest(&call),
            output: json!(output),
            field: Some(("doc".into(), "a".into(), "sum".into())),
        }
    };
    let m = computed_by("m", Number, "g", &["n"]);
    let sum = computed_by("sum", Text, "f", &["m"]);
    let l = Folded::new("recompute-3-call");
    l.bodies(vec![
        expr("g", 1, "n * 2"),
        declaration(f.clone(), 1),
        declared(type_decl("doc", vec![field("n", Number), m, sum]), 1),
        wrote("doc", "a", 1, json!({"n": 3})),
    ]);
    l.bodies(vec![recorded(6, "S6")]);
    l.due();
    let q = l.bodies(vec![expr("g", 2, "n * 3"), recorded(9, "S9")]);
    assert_eq!(
        (l.state(Some(q[0]), "doc", "a"), l.state(None, "doc", "a")),
        (
            json!([{"n": 3, "m": 9}, 1, q[0].0]),
            json!([{"n": 3, "m": 9, "sum": "S9"}, 1, q[1].0])
        ),
        "row 3: the output of the call the step made fills the record, which makes that call \
         only as the step left it"
    );
}

#[test]
fn row_4_a_step_reads_the_outputs_recorded_before_it_whenever_it_is_asked() {
    let l = summing("recompute-4", &[("doc", "a", "A"), ("doc", "b", "A")]);
    let q = l.bodies(vec![
        output(json!(1), "A", json!("S"), None),
        http(2, json!(1)),
        output(json!(1), "A", json!("T"), Some(("doc", "b"))),
        restricted("doc", "a", 2),
    ]);
    assert_eq!(
        (l.state(None, "doc", "a"), l.state(None, "doc", "b")),
        (
            json!([{"title": "A", "sum": "S"}, 2, q[3].0]),
            json!([{"title": "A", "sum": "T"}, 1, q[2].0])
        ),
        "row 4: the output recorded before the `declared` fills `a` at the step; the one \
         recorded after it fills `b`, which it names, and leaves `a` as the step computed it"
    );
    let at = |key| l.meta(None, "doc", key, "sum").map(|m| m.position);
    assert_eq!((at("a"), at("b")), (Some(q[1]), Some(q[2])), "row 4");
    assert_eq!(
        l.state(Some(q[1]), "doc", "b"),
        json!([{"title": "A", "sum": "S"}, 1, q[1].0]),
        "row 4: as of the `declared` both hold the output recorded before it"
    );
}

#[test]
fn row_4a_every_recording_of_one_call_is_kept_with_its_position() {
    let l = summing(
        "recompute-4a",
        &[("doc", "a", "X"), ("doc", "b", "X"), ("memo", "m", "X")],
    );
    let q = l.bodies(vec![
        output(json!(1), "X", json!("S"), Some(("doc", "a"))),
        declared(summed("doc"), 2),
        output(json!(1), "X", json!("S"), Some(("memo", "m"))),
        restricted("doc", "b", 2),
    ]);
    assert_eq!(
        l.state(None, "doc", "b"),
        json!([{"title": "X", "sum": "S"}, 2, q[3].0]),
        "row 4a: the step's later read finds the first recording, which stands before it, \
         though the same call was recorded again after it"
    );
    let at = |ty, key| l.meta(None, ty, key, "sum").map(|m| m.position);
    assert_eq!(
        (at("doc", "a"), at("doc", "b"), at("memo", "m")),
        (Some(q[0]), Some(q[1]), Some(q[2])),
        "row 4a: `a` by its fill, `b` by the `declared`, `m` by its fill"
    );
}

#[test]
fn row_4b_an_output_recorded_between_a_declared_and_its_flush_is_not_read() {
    let l = summing("recompute-4b", &[("doc", "a", "A")]);
    let q = l.bodies(vec![
        output(json!(1), "A", json!("S"), None),
        http(2, json!(1)),
        output(json!(1), "A", json!("T"), None),
    ]);
    assert_eq!(
        l.state(None, "doc", "a"),
        json!([{"title": "A", "sum": "S"}, 1, q[1].0]),
        "row 4b: an output that names no field reads no record, so the step is walked after \
         it; the step reads what was recorded before its own event"
    );
}

/// `doc` with `twice`, by `f`, and `plus`, derived as `plus`.
fn doubled_plus(plus: &str) -> TypeDecl {
    let mut t = doubled();
    t.fields.push(derived("plus", Number, plus, &["n"]));
    t
}

#[test]
fn row_5_each_step_computes_with_what_stood_at_its_own_event() {
    let l = Folded::new("recompute-5");
    l.bodies(vec![
        expr("f", 1, "n * 2"),
        declared(doubled_plus("n + 1"), 1),
        wrote("doc", "a", 1, json!({"n": 3})),
    ]);
    let q = l.append(vec![
        own(1, "user:ada", expr("f", 2, "n * 3")),
        own(2, "user:bob", declared(doubled_plus("n + 10"), 2)),
        own(3, "user:cy", restricted("doc", "a", 2)),
    ]);
    assert_eq!(
        l.state(None, "doc", "a"),
        json!([{"n": 3, "twice": 9, "plus": 13}, 2, q[2].0]),
        "row 5"
    );
    assert_eq!(
        (
            l.meta(None, "doc", "a", "twice"),
            l.meta(None, "doc", "a", "plus")
        ),
        (
            Some(stamped(1, "user:ada", q[0], Some("2"))),
            Some(stamped(2, "user:bob", q[1], None))
        ),
        "row 5: each field is stamped by the step that computed it"
    );
    assert_eq!(
        (
            l.state(Some(q[0]), "doc", "a"),
            l.state(Some(q[1]), "doc", "a")
        ),
        (
            json!([{"n": 3, "twice": 9, "plus": 4}, 1, q[0].0]),
            json!([{"n": 3, "twice": 9, "plus": 13}, 1, q[1].0])
        ),
        "row 5: as of the function's `declared` the type's new expression is not in force yet"
    );
}

fn migrated(to: TypeDecl, moves: &[(&str, &str)]) -> EventBody {
    let moved = |(from, to): &(&str, &str)| Rekey {
        ty: "doc".into(),
        from: (*from).into(),
        to: (*to).into(),
    };
    EventBody::Migrated {
        schema_hash: lakelands_core::shape::schema_hash(&to).expect("a schema hash"),
        migration: Migration {
            to,
            renames: Vec::new(),
            relation_renames: Vec::new(),
        },
        version: 2,
        report_id: "by hand".into(),
        rekeyed: moves.iter().map(moved).collect(),
    }
}

/// A migration whose fold fails carries what is owed once: the fold that
/// catches up carries it from where the last commit left it.
#[test]
fn a_migration_whose_fold_fails_carries_what_is_owed_once() {
    let l = summing("recompute-carried", &[("doc", "b", "B")]);
    l.bodies(vec![wrote("doc", "a", 1, json!({"title": "A"}))]);
    *l.probe.refuse_commit.lock().unwrap() = Some(1);
    let chained = migrated(summed("doc"), &[("a", "b"), ("b", "c")]);
    l.try_append(vec![system(chained)])
        .expect_err("the fold fails");
    l.bodies(vec![wrote("doc", "z", 1, json!({"title": "Z"}))]);
    let keys: Vec<String> = (l.lake.writer().owed.iter())
        .map(|o| match &o.of {
            Of::Record(key) => key.to_string(),
            Of::Sweep { .. } => "*".to_owned(),
        })
        .collect();
    assert_eq!(keys, ["b", "z"], "`a`'s entry moves to `b` once");
}

#[test]
fn row_6_a_recomputation_after_a_rekey_lands_at_the_new_key() {
    let (l, _) = doubling("recompute-6", &[("a", 3)]);
    let q = l.bodies(vec![
        migrated(doubled(), &[("a", "b")]),
        expr("f", 2, "n * 3"),
        restricted("doc", "b", 2),
    ]);
    assert_eq!(
        (l.state(None, "doc", "b"), l.state(None, "doc", "a")),
        (json!([{"n": 3, "twice": 9}, 2, q[2].0]), json!("ended")),
        "row 6: the record at its new key holds the recomputed field"
    );
    assert_eq!(
        (
            l.state(Some(q[0]), "doc", "b"),
            l.state(Some(q[1]), "doc", "b")
        ),
        (
            json!([{"n": 3, "twice": 6}, 1, q[0].0]),
            json!([{"n": 3, "twice": 9}, 1, q[1].0])
        ),
        "row 6: moved by the migration, then computed again where it arrived"
    );
}

#[test]
fn row_7_a_migration_converts_the_record_as_the_recomputation_left_it() {
    let (l, _) = doubling("recompute-7", &[("a", 12)]);
    let mut texted = doubled();
    texted.fields[0] = field("n", Text);
    let q = l.bodies(vec![expr("f", 2, "n * 3"), migrated(texted, &[])]);
    assert_eq!(
        (l.state(Some(q[0]), "doc", "a"), l.state(None, "doc", "a")),
        (
            json!([{"n": 12, "twice": 36}, 1, q[0].0]),
            json!([{"n": "12", "twice": 36}, 1, q[1].0])
        ),
        "row 7: computed again over the number, as of the `declared`; then converted"
    );
}

const MANY: i64 = 2_500;

fn key(i: i64) -> String {
    format!("k{i:04}")
}

/// A lake that holds `f` as `n * 2`, `doc` and `MANY` records, each with
/// the `n` that `n` gives its number.
fn many(name: &str, n: impl Fn(i64) -> i64) -> Folded {
    let l = Folded::new(name);
    let mut load = vec![expr("f", 1, "n * 2"), declared(doubled(), 1)];
    load.extend((1..=MANY).map(|i| wrote("doc", &key(i), 1, json!({"n": n(i)}))));
    l.bodies(load);
    l.probe.take();
    l
}

/// The scans of `doc` among what was asked.
fn scans(asked: &[Asked]) -> usize {
    let scan = |a: &&Asked| matches!(a, Asked::Scan(ty) if ty.as_str() == "doc");
    asked.iter().filter(scan).count()
}

#[test]
fn row_8_a_page_is_put_before_the_next_is_read() {
    let l = many("recompute-8", |i| i);
    let q = l.bodies(vec![expr("f", 2, "n * 3")]);
    let asked = l.probe.take();
    let first = |wanted: &dyn Fn(&Asked) -> bool, nth: usize| {
        let found = asked.iter().enumerate().filter(|(_, a)| wanted(a));
        found.map(|(i, _)| i).nth(nth)
    };
    let put = first(
        &|a| matches!(a, Asked::Put(ty, ..) if ty.as_str() == "doc"),
        0,
    );
    let second = first(&|a| matches!(a, Asked::Scan(ty) if ty.as_str() == "doc"), 1);
    assert!(
        put.is_some() && put < second,
        "row 8: the first record of the type is put (call {put:?}) before its second page is \
         read (call {second:?}): nothing of the walk is kept"
    );
    for i in [1, 1_024, 1_025, MANY] {
        assert_eq!(
            l.state(None, "doc", &key(i)),
            json!([{"n": i, "twice": i * 3}, 1, q[0].0]),
            "row 8: {}",
            key(i)
        );
    }
}

#[test]
fn row_9_the_flush_indexes_the_vectors_it_fills() {
    let l = Folded::new("recompute-9");
    let embedded = || {
        let vec = embedding("vec", 3, &["title"], "m");
        type_decl("doc", vec![field("title", Text), vec])
    };
    let mut load = vec![declared(embedded(), 1)];
    load.extend(["a", "b", "c"].map(|k| wrote("doc", k, 1, json!({"title": k}))));
    l.bodies(load);
    let vector = |text: &str, v: Value| EventBody::OutputRecorded {
        seam: backed::embed_seam("m"),
        input: backed::digest(&backed::embed_call(text)),
        output: v,
        field: None,
    };
    let q = l.bodies(vec![
        vector("a", json!([1.0, 0.0, 0.0])),
        vector("b", json!([0.0, 1.0, 0.0])),
        vector("c", json!([0.0, 0.0, 1.0])),
        declared(embedded(), 2),
        restricted("doc", "b", 2),
        retired("doc", "c", 2),
    ]);
    assert_eq!(
        l.state(None, "doc", "a"),
        json!([{"title": "a", "vec": [1, 0, 0]}, 1, q[3].0]),
        "row 9: the type declared again fills the vector from the output recorded before it"
    );
    let (b, c) = (
        l.record(None, "doc", "b").expect("the record"),
        l.record(None, "doc", "c").expect("the record"),
    );
    assert_eq!(
        (json!(b.fields), b.restricted, b.position),
        (json!({"title": "b", "vec": [0, 1, 0]}), true, q[4]),
        "row 9: `b` is restricted as the step left it, with its vector"
    );
    assert_eq!(
        (json!(c.fields), c.retired, c.position),
        (json!({"title": "c", "vec": [0, 0, 1]}), true, q[5]),
        "row 9: `c` is retired as the step left it, with its vector"
    );
    let index = l.lake.engine.vectors().expect("the memory engine's index");
    let found = index.nearest(&"doc".into(), "vec", &[0.0, 0.0, 1.0], 3);
    let mut found: Vec<String> = (found.expect("a search").iter())
        .map(|(k, _)| k.to_string())
        .collect();
    found.sort();
    assert_eq!(
        found,
        ["a", "b"],
        "row 9: `a`, which only the flush put, is indexed by it; `b` and `c` were read later \
         through the step, and the event that wrote each indexed what it wrote: `b` \
         restricted is found, `c` retired is not, though the flush had indexed it"
    );
}

#[test]
fn row_10_a_record_the_recomputation_leaves_as_it_is_keeps_its_position() {
    let (l, p) = doubling("recompute-10", &[("a", 0), ("b", 3)]);
    l.probe.take();
    let q = l.bodies(vec![
        wrote("doc", "w", 1, json!({"n": 0})),
        expr("f", 2, "n * 3"),
    ]);
    assert_eq!(
        (
            l.state(None, "doc", "a"),
            l.state(None, "doc", "w"),
            l.state(None, "doc", "b")
        ),
        (
            json!([{"n": 0, "twice": 0}, 1, p[2].0]),
            json!([{"n": 0, "twice": 0}, 1, q[0].0]),
            json!([{"n": 3, "twice": 9}, 1, q[1].0])
        ),
        "row 10: a record whose value stays keeps its position, in the view and in what the \
         transaction wrote"
    );
    let put = |a: Asked| match a {
        Asked::Put(_, key, at) => Some((key.to_string(), at)),
        _ => None,
    };
    let puts: Vec<_> = l.probe.take().into_iter().filter_map(put).collect();
    assert_eq!(
        puts,
        [("w".to_owned(), q[0]), ("b".to_owned(), q[1])],
        "row 10: only the write and the record that changed are put"
    );
}

#[test]
fn row_11_a_retired_record_read_through_the_step_is_as_it_was() {
    let (l, p) = doubling("recompute-11", &[("b", 3), ("r", 3)]);
    l.bodies(vec![retired("doc", "r", 2)]);
    let q = l.bodies(vec![expr("f", 2, "n * 3"), restricted("doc", "r", 3)]);
    let r = l.record(None, "doc", "r").expect("the record");
    let twice = r.meta.get(&FieldName::from("twice")).map(|m| m.position);
    assert_eq!(
        (json!(r.fields), r.version.0, r.position, r.retired, twice),
        (json!({"n": 3, "twice": 6}), 3, q[1], true, Some(p[3])),
        "row 11: a retired record is not computed again, by the walk or by a later read"
    );
}

#[test]
fn row_12_a_restricted_record_is_computed_again_with_its_flags() {
    let (l, _) = doubling("recompute-12", &[("h", 3)]);
    l.bodies(vec![restricted("doc", "h", 2)]);
    let q = l.bodies(vec![expr("f", 2, "n * 3")]);
    let h = l.record(None, "doc", "h").expect("the record");
    assert_eq!(
        (
            json!(h.fields),
            h.version.0,
            h.position,
            h.retired,
            h.restricted
        ),
        (json!({"n": 3, "twice": 9}), 2, q[0], false, true),
        "row 12: restricted, and computed again; its version and its flags stay"
    );
}

#[test]
fn row_14_a_step_computes_with_the_value_types_and_settings_of_its_event() {
    let l = Folded::new("recompute-14");
    let money = |scale: u32, version: u32| {
        let d = json!({"declare": "value_type", "name": "money", "kind": "decimal",
                       "scale": scale});
        declaration(d, version)
    };
    let priced = |version: u32, expr: &str| {
        let f = json!({"declare": "function", "name": "f", "backing": "expr", "expr": expr,
                       "inputs": [{"name": "net", "type": {"kind": "decimal"}}]});
        declaration(f, version)
    };
    let mut gross = computed_by("gross", Decimal, "f", &["net"]);
    gross.ty.value_type = Some("money".into());
    let rate = |version: u64, value: i64| {
        let fields = json!({"key": "rate", "value": value});
        wrote("sys.setting", "rate", version, fields)
    };
    l.bodies(vec![
        declared(lakelands_api::seam::setting_type(), 1),
        money(2, 1),
        priced(1, "net * setting('rate')"),
        declared(type_decl("doc", vec![field("net", Decimal), gross]), 1),
        rate(1, 3),
        wrote("doc", "a", 1, json!({"net": "1.005"})),
    ]);
    assert_eq!(
        l.state(None, "doc", "a")[0],
        json!({"net": "1.005", "gross": "3.02"}),
        "row 14: the fixture rounds to the value type's two digits"
    );
    let q = l.bodies(vec![
        priced(2, "net * setting('rate') + 1"),
        money(3, 2),
        rate(2, 5),
        restricted("doc", "a", 2),
    ]);
    assert_eq!(
        (l.state(Some(q[0]), "doc", "a"), l.state(None, "doc", "a")),
        (
            json!([{"net": "1.005", "gross": "4.02"}, 1, q[0].0]),
            json!([{"net": "1.005", "gross": "4.02"}, 2, q[3].0])
        ),
        "row 14: computed with the rate of 3 and the two digits that stood at the `declared`, \
         by the walk and by the later read, though the value type has three digits and the \
         rate is 5 by then"
    );
}

#[test]
fn row_15_the_due_calls_come_by_step_type_and_key() {
    // `zz` settles before `aa`: both read `title` alone, and `zz` is
    // declared first.
    let both = |name: &str| {
        let by_f = |field: &str| computed_by(field, Text, "f", &["title"]);
        type_decl(name, vec![field("title", Text), by_f("zz"), by_f("aa")])
    };
    let l = Folded::new("recompute-15");
    l.bodies(vec![
        http(1, json!(1)),
        declared(both("doc"), 1),
        declared(both("memo"), 1),
        wrote("doc", "a", 1, json!({"title": "A"})),
        wrote("memo", "b", 1, json!({"title": "B"})),
        wrote("doc", "c", 1, json!({"title": "C"})),
    ]);
    l.due();
    let q = l.bodies(vec![
        wrote("doc", "b", 1, json!({"title": "W"})),
        http(2, json!(1)),
        http(3, json!(1)),
    ]);
    let due: Vec<_> = l.due().into_iter().map(|d| (d.0, d.1, d.2, d.3)).collect();
    let step = |at: Position| {
        let record = move |ty: &str, key: &str| {
            let one = |field: &str| (at.0, ty.to_owned(), key.to_owned(), field.to_owned());
            [one("zz"), one("aa")]
        };
        [
            record("doc", "a"),
            record("doc", "b"),
            record("doc", "c"),
            record("memo", "b"),
        ]
        .into_iter()
        .flatten()
    };
    let write =
        [("zz", q[0]), ("aa", q[0])].map(|(f, at)| (at.0, "doc".into(), "b".into(), f.into()));
    let expected: Vec<(u64, String, String, String)> =
        (write.into_iter().chain(step(q[1])).chain(step(q[2]))).collect();
    assert_eq!(
        due, expected,
        "row 15: the write's calls, then each step's: its types in the ontology's order, the \
         records of each by key, a record's fields in the order they settle"
    );
}

#[test]
fn row_16_a_forced_settle_calls_again_for_a_recorded_failure() {
    let failure = || json!({"error": "boom"});
    let check = |l: &Folded, at: Position, placement: &str| {
        assert_eq!(
            l.state(None, "doc", "a"),
            json!([{"title": "A"}, 1, at.0]),
            "row 16, {placement}: no value, the record at the `declared`'s position"
        );
        let stamp = l.meta(None, "doc", "a", "sum").expect("the provenance");
        assert_eq!(
            (
                stamp.error.as_deref(),
                stamp.position,
                stamp.writer.as_str()
            ),
            (Some("boom"), at, "user:ada"),
            "row 16, {placement}: the recorded reason, stamped by the `declared`"
        );
        assert_eq!(
            l.due(),
            [(
                at.0,
                "doc".into(),
                "a".into(),
                "sum".into(),
                json!({"title": "A"}),
                true
            )],
            "row 16, {placement}: one call is due, and it is called again"
        );
    };
    // The record written by the output's fill, in the same transaction.
    let l = summing("recompute-16-written", &[("doc", "a", "A")]);
    let q = l.append(vec![
        system(output(json!(1), "A", failure(), Some(("doc", "a")))),
        own(1, "user:ada", http(2, json!(1))),
    ]);
    check(&l, q[1], "the record written by the fill");
    // The record in the view only.
    let l = summing("recompute-16-view", &[("doc", "a", "A")]);
    l.bodies(vec![output(json!(1), "A", failure(), Some(("doc", "a")))]);
    let q = l.append(vec![own(1, "user:ada", http(2, json!(1)))]);
    check(&l, q[0], "the record in the view only");
}

/// A backing that fails its first call and answers `again` from then on.
struct Flaky(AtomicUsize);

#[async_trait]
impl FunctionBacking for Flaky {
    fn kind(&self) -> &str {
        "flaky"
    }

    async fn call(&self, _: &FunctionCall) -> lakelands_api::Result<Value> {
        match self.0.fetch_add(1, Ordering::SeqCst) {
            0 => Err(Error::Unavailable {
                message: "boom".into(),
            }),
            _ => Ok(json!("again")),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn row_16_the_recompute_door_calls_again_for_a_recorded_failure() {
    let flaky = Arc::new(Flaky(AtomicUsize::new(0)));
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        backings: vec![flaky.clone()],
        ..Seams::default()
    };
    let location = Location::memory("recompute-16-door");
    let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Create)
        .await
        .expect("a lake");
    let l = fixture::Lake::of(store.as_ref());
    let f = json!({"declare": "function", "name": "f", "backing": "flaky",
                   "inputs": [{"name": "title", "type": {"kind": "text"}}]});
    l.declare_json(f).await.unwrap();
    l.declare(summed("doc")).await.unwrap();
    l.upsert("doc", "a", json!({"title": "A"})).await.unwrap();
    let failed = l.read("doc", "a").await.unwrap();
    assert_eq!(
        (
            fixture::value(&failed, "sum"),
            failed.meta["sum"].error.as_deref()
        ),
        (None, Some("boom")),
        "row 16, the door: the first call fails and its reason is recorded"
    );
    let head = store.head().await.unwrap();
    let again = (l.cat).recompute(&l.own, Recompute::Function("f".into()));
    again.await.unwrap();
    let declared = head.next();
    let (doc, at) = ("doc".into(), fixture::get_at("a", Some(declared)));
    let then = l.cat.get(&l.own, &doc, at).await.unwrap().data;
    let stamp = &then.meta["sum"];
    assert_eq!(
        (
            fixture::value(&then, "sum"),
            stamp.error.as_deref(),
            stamp.position
        ),
        (None, Some("boom"), declared),
        "row 16, the door: as of the `declared` the door appends, no value and the recorded \
         reason, stamped by it"
    );
    let now = l.read("doc", "a").await.unwrap();
    assert_eq!(
        (fixture::value(&now, "sum"), flaky.0.load(Ordering::SeqCst)),
        (Some(&json!("again")), 2),
        "row 16, the door: the call is made once more, and its answer fills the field"
    );
}

#[test]
fn row_17_a_step_that_changed_one_record_of_the_first_page_is_kept() {
    let l = many("recompute-17", |i| if i == 3 { 3 } else { 0 });
    let q = l.bodies(vec![expr("f", 2, "n * 3"), restricted("doc", &key(3), 2)]);
    assert_eq!(
        l.state(None, "doc", &key(3)),
        json!([{"n": 3, "twice": 9}, 2, q[1].0]),
        "row 17: the one record the step changed is read through it, though the type's \
         later pages changed nothing"
    );
}

#[test]
fn row_18_a_written_record_the_flush_leaves_as_it_is_stays_kept() {
    let l = Folded::new("recompute-18");
    let mut noted = doubled();
    noted.fields.push(field("note", Text));
    l.bodies(vec![
        expr("f", 1, "n * 2"),
        declared(noted, 1),
        wrote("doc", "a", 1, json!({"n": 0, "note": "view"})),
        wrote("doc", "b", 1, json!({"n": 3})),
    ]);
    let q = l.bodies(vec![
        wrote("doc", "a", 2, json!({"note": "written"})),
        expr("f", 2, "n * 3"),
        restricted("doc", "a", 3),
    ]);
    let a = l.record(None, "doc", "a").expect("the record");
    assert_eq!(
        (json!(a.fields), a.version.0, a.position, a.restricted),
        (
            json!({"n": 0, "twice": 0, "note": "written"}),
            3,
            q[2],
            true
        ),
        "row 18: the restriction lands on what the transaction wrote, with the upsert's \
         value, though the `declared` did not change it; never on the view's row"
    );
}

#[test]
fn row_19_a_record_of_another_type_is_as_it_was() {
    let l = Folded::new("recompute-19");
    let p = l.bodies(vec![
        expr("f", 1, "n * 2"),
        declared(doubled(), 1),
        declared(type_decl("memo", vec![field("n", Number)]), 1),
        wrote("doc", "a", 1, json!({"n": 3})),
        wrote("memo", "m", 1, json!({"n": 3})),
    ]);
    let folded = l.try_append(vec![
        system(expr("f", 2, "n * 3")),
        system(restricted("memo", "m", 2)),
    ]);
    let q = folded.unwrap_or_else(|e| {
        panic!("row 19: a read of another type's record through the step must not fail: {e}")
    });
    let m = l.record(None, "memo", "m").expect("the record");
    let n = m.meta.get(&FieldName::from("n")).map(|m| m.position);
    assert_eq!(
        (json!(m.fields), m.version.0, m.position, n),
        (json!({"n": 3}), 2, q[1], Some(p[4])),
        "row 19: a step answers a record of a type it does not compute as it was"
    );
}

/// `doc` with `x`, `y` and `z`, by `f`, `g` and `h`.
fn tripled() -> TypeDecl {
    let by = |name: &str, f: &str| computed_by(name, Number, f, &["n"]);
    let fields = vec![field("n", Number), by("x", "f"), by("y", "g"), by("z", "h")];
    type_decl("doc", fields)
}

#[test]
fn row_20_a_bundle_of_changed_functions_walks_its_type_once() {
    let l = Folded::new("recompute-20");
    let mut load = vec![
        expr("f", 1, "n * 2"),
        expr("g", 1, "n + 1"),
        expr("h", 1, "n + 5"),
        declared(tripled(), 1),
    ];
    load.extend((1..=MANY).map(|i| wrote("doc", &key(i), 1, json!({"n": i}))));
    l.bodies(load);
    l.probe.take();
    let q = l.bodies(vec![
        expr("f", 2, "n * 3"),
        expr("g", 2, "n + 2"),
        expr("h", 2, "n + 6"),
    ]);
    let asked = l.probe.take();
    for i in 1..=MANY {
        let (k, x) = (key(i), i * 3);
        let at = |p: usize| l.state(Some(q[p]), "doc", &k);
        assert_eq!(
            [at(0), at(1), at(2)],
            [
                json!([{"n": i, "x": x, "y": i + 1, "z": i + 5}, 1, q[0].0]),
                json!([{"n": i, "x": x, "y": i + 2, "z": i + 5}, 1, q[1].0]),
                json!([{"n": i, "x": x, "y": i + 2, "z": i + 6}, 1, q[2].0])
            ],
            "row 20: `{k}` as of each `declared`"
        );
    }
    assert_eq!(
        scans(&asked),
        3,
        "row 20: three functions of one type of {MANY} records: its three pages are read \
         once, not once a function"
    );
}

/// The log of rows 21 and following: `f` and `doc` with three records,
/// then ten appends, each `f` declared again, with a write after the fifth.
/// Answers the lake, the write's position and every record's state at
/// every position, as the live lake holds them.
fn ten_appends(name: &str) -> (Folded, Position, Vec<Value>) {
    let (l, _) = doubling(name, &[("a", 1), ("b", 2), ("c", 3)]);
    let mut written = None;
    for v in 2..=11 {
        l.bodies(vec![expr("f", v, &format!("n * {}", v + 1))]);
        if v == 6 {
            written = l.bodies(vec![wrote("doc", "w", 1, json!({"n": 4}))]).pop();
        }
    }
    let live = l.history(KEYS);
    l.probe.take();
    (l, written.expect("the write"), live)
}

const KEYS: &[(&str, &str)] = &[("doc", "a"), ("doc", "b"), ("doc", "c"), ("doc", "w")];

fn commits(asked: &[Asked]) -> Vec<Position> {
    let commit = |a: &Asked| match a {
        Asked::Commit(at) => Some(*at),
        _ => None,
    };
    asked.iter().filter_map(commit).collect()
}

#[test]
fn row_21_a_rebuild_ends_its_transaction_after_a_flush_that_kept_a_step() {
    let (l, written, live) = ten_appends("recompute-21");
    let head = l.lake.head();
    assert_eq!(l.lake.rebuild().expect("the rebuild"), head);
    assert_eq!(
        l.history(KEYS),
        live,
        "row 21: every position answers as the live lake did"
    );
    assert_eq!(
        commits(&l.probe.take()),
        [Position(written.0 - 1), head],
        "row 21: the page's five recomputations before the write are flushed and committed \
         before the write is folded; the five after it with the page"
    );
}

#[test]
fn a_commit_refused_inside_a_page_leaves_what_was_committed_before_it() {
    // The commit after the flush is refused: nothing of the page is
    // committed, and the next fold starts the page again.
    let (l, written, live) = ten_appends("recompute-refused-first");
    let folded = || {
        l.lake
            .engine
            .projection()
            .folded_through()
            .expect("the mark")
    };
    *l.probe.refuse_commit.lock().unwrap() = Some(1);
    let refused = l.lake.rebuild().expect_err("the rebuild fails");
    assert!(refused.to_string().contains("planted"), "{refused}");
    assert_eq!(
        (folded(), l.lake.head()),
        (None, Position(0)),
        "the commit after the flush was refused: the page's earlier events are not committed"
    );
    assert_eq!(l.state(None, "doc", "a"), json!("absent"));
    l.lake.rebuild().expect("the next fold");
    assert_eq!(
        l.history(KEYS),
        live,
        "after the refused commit, the next fold answers as the live lake did"
    );

    // The page's last commit is refused: the page stands at the commit
    // after the flush, whole, and the next fold continues from there.
    let (l, written_2, live) = ten_appends("recompute-refused-last");
    assert_eq!(written, written_2);
    let folded = || {
        l.lake
            .engine
            .projection()
            .folded_through()
            .expect("the mark")
    };
    let before = Position(written.0 - 1);
    *l.probe.refuse_commit.lock().unwrap() = Some(2);
    l.lake.rebuild().expect_err("the rebuild fails");
    assert_eq!(
        (folded(), l.lake.head()),
        (Some(before), before),
        "the page's last commit was refused: the projection stands at the commit after the \
         flush, and the head names it"
    );
    let through = |all: &[Value]| all[..KEYS.len() * (before.0 as usize + 1)].to_vec();
    let view = l.lake.engine.view(None).expect("a view");
    assert_eq!(view.position(), Some(before));
    drop(view);
    let held: Vec<Value> = (0..=before.0)
        .flat_map(|p| KEYS.iter().map(move |(ty, key)| (Position(p), *ty, *key)))
        .map(|(at, ty, key)| l.state(Some(at), ty, key))
        .collect();
    assert_eq!(
        held,
        through(&live),
        "what the first commit holds is whole: every position through it answers as the \
         live lake did, the five recomputations put"
    );
    // The next fold: an append finds the projection behind and folds the
    // rest of the log first.
    l.bodies(vec![wrote("doc", "z", 1, json!({"n": 0}))]);
    let all = l.history(KEYS);
    assert_eq!(
        all[..live.len()],
        live[..],
        "the next fold continues after the first commit and answers as the live lake did"
    );
}

#[test]
fn another_handle_reads_the_first_commit_whole_between_the_two_commits_of_a_page() {
    let (l, written, live) = ten_appends("recompute-between");
    let before = Position(written.0 - 1);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (opener, location, into) = (
        l.opener.clone(),
        Location::memory(l.name.as_str()),
        seen.clone(),
    );
    // Before each commit of the rebuild, while its transaction is open and
    // holds every put, a second handle on the lake reads the head.
    *l.probe.before_commit.lock().unwrap() = Some(Box::new(move |through| {
        let other = opener
            .open(&location, OpenMode::Read)
            .expect("a second handle");
        let folded = other.projection().folded_through().expect("the mark");
        let view = other.view(None).expect("a view");
        let get = |(ty, key): &(&str, &str)| {
            let r = view.get(&(*ty).into(), &(*key).into()).expect("a read");
            r.map(|r| json!([r.fields, r.version.0, r.position.0]))
        };
        let read: Vec<_> = KEYS.iter().map(get).collect();
        into.lock()
            .unwrap()
            .push((through, folded, view.position(), read));
    }));
    l.lake.rebuild().expect("the rebuild");
    *l.probe.before_commit.lock().unwrap() = None;
    let seen = seen.lock().unwrap().clone();
    let head = l.lake.head();
    let at = |p: Position| -> Vec<Option<Value>> {
        let states = &live[KEYS.len() * p.0 as usize..][..KEYS.len()];
        let held = |s: &Value| (s != &json!("absent")).then(|| s.clone());
        states.iter().map(held).collect()
    };
    assert_eq!(seen.len(), 2, "the page commits twice: {seen:?}");
    assert_eq!(
        seen[0],
        (before, None, None, vec![None; KEYS.len()]),
        "before the first commit of the rebuilt page another handle reads an empty projection"
    );
    assert_eq!(
        seen[1],
        (head, Some(before), Some(before), at(before)),
        "between the two commits another handle reads the first commit's position, whole: \
         each record as the live lake held it there, and nothing of the open transaction"
    );
}

#[test]
fn a_live_append_commits_once_though_its_flush_keeps_a_step() {
    let (l, _) = doubling("recompute-live-once", &[("a", 3)]);
    l.probe.take();
    let q = l.bodies(vec![expr("f", 2, "n * 3"), restricted("doc", "a", 2)]);
    assert_eq!(
        commits(&l.probe.take()),
        [q[1]],
        "a live append is one transaction: the flush before the restriction keeps a step, \
         and the append still commits once, through its last event"
    );
    assert_eq!(
        l.state(None, "doc", "a"),
        json!([{"n": 3, "twice": 9}, 2, q[1].0])
    );
}

#[test]
fn the_dues_of_a_commit_inside_a_page_stay_due_when_the_page_fails_after_it() {
    for (failing, plant) in [
        ("the page's last commit", 0),
        ("the `begin` after the commit", 1),
    ] {
        // `a` holds a filled `sum`, `c` waits. `f` is declared again with
        // another call, a record is written, and `f` is declared again, in
        // one append whose fold fails at its commit: the lake is behind its
        // log, and the fold that catches up hands out that append's calls.
        let l = summing(
            &format!("recompute-dues-{plant}"),
            &[("doc", "a", "A"), ("doc", "c", "C")],
        );
        l.bodies(vec![output(json!(1), "A", json!("S"), Some(("doc", "a")))]);
        l.due();
        *l.probe.refuse_commit.lock().unwrap() = Some(1);
        let three = [
            http(2, json!(2)),
            wrote("doc", "w", 1, json!({"title": "W"})),
            http(3, json!(3)),
        ];
        let failed = l.try_append(three.into_iter().map(system).collect());
        failed.expect_err("the append's fold fails");
        let d2 = l.lake.engine.tip().expect("the tip").expect("a tip").0.0;
        let (d1, w1) = (d2 - 2, d2 - 1);
        // That catch-up commits inside the page, before the write, and then
        // fails: at the page's last commit, or at the `begin` after the
        // commit inside it.
        match plant {
            0 => *l.probe.refuse_commit.lock().unwrap() = Some(2),
            _ => *l.probe.refuse_begin.lock().unwrap() = Some(2),
        }
        let z = || vec![system(wrote("doc", "z", 1, json!({"title": "Z"})))];
        let failed = l.try_append(z()).expect_err("the fold fails");
        assert!(failed.to_string().contains("planted"), "{failed}");
        let folded = l
            .lake
            .engine
            .projection()
            .folded_through()
            .expect("the mark");
        assert_eq!(folded, Some(Position(w1 - 1)), "{failing} is refused");
        let due = |l: &Folded| -> Vec<(u64, String)> {
            let owed = std::mem::take(&mut l.lake.writer().owed);
            let of = |o: &Owed| match &o.of {
                Of::Record(key) => key.to_string(),
                Of::Sweep { .. } => "*".to_owned(),
            };
            owed.iter().map(|o| (o.cause.0, of(o))).collect()
        };
        let key = |at: u64, key: &str| (at, key.to_owned());
        assert_eq!(
            due(&l),
            [key(d1, "*")],
            "{failing} is refused: what the commit inside the page holds stays owed, the \
             flush's sweep; what was rolled back does not"
        );
        // The next fold makes the calls of the rest, once.
        let appended = l.try_append(z()).expect("the next fold, and the append");
        assert_eq!(
            due(&l),
            [key(w1, "w"), key(d2, "*"), key(appended[0].0, "z")],
            "after {failing} was refused, the next fold leaves owed what the rest of the page \
             owes"
        );
    }
}

#[test]
fn a_type_declared_again_with_one_more_plain_field_is_not_walked() {
    let l = many("recompute-plain-field", |i| i);
    let mut wider = doubled();
    wider.fields.push(field("note", Text));
    l.bodies(vec![declared(wider, 2)]);
    assert_eq!(
        scans(&l.probe.take()),
        0,
        "what computes `doc`'s fields is as it was: a plain field more scans no record"
    );
    let mut tripling = doubled();
    tripling
        .fields
        .push(derived("plus", Number, "n + 1", &["n"]));
    let q = l.bodies(vec![declared(tripling, 3)]);
    assert_eq!(
        (scans(&l.probe.take()), l.state(None, "doc", &key(7))),
        (3, json!([{"n": 7, "twice": 14, "plus": 8}, 1, q[0].0])),
        "a derived field more computes the type again"
    );
}

/// `body`, a `declared`, as one of a set that goes on in its append.
fn going_on(body: EventBody) -> EventBody {
    match body {
        EventBody::Declared {
            declaration,
            schema_hash,
            version,
            ..
        } => EventBody::Declared {
            declaration,
            schema_hash,
            version,
            with_the_set: true,
        },
        other => other,
    }
}

/// `tripled` with `moved`, which says whether the fold's last computation
/// changed `y`.
fn tripled_moved() -> TypeDecl {
    let mut t = tripled();
    let moved = derived("moved", Boolean, "changed(y)", &["y"]);
    t.fields.push(moved);
    t
}

/// The lake of rows 22 and 23: `f`, `g`, `h`, `doc` as `tripled_moved`,
/// and `MANY` records; then `f`, `g` and `h` declared again in one append,
/// as a set when `set`. Answers the lake, the positions of the records'
/// writes and of the three declarations.
fn three_again(name: &str, set: bool) -> (Folded, Vec<Position>, Vec<Position>) {
    let l = Folded::new(name);
    let mut load = vec![
        expr("f", 1, "n * 2"),
        expr("g", 1, "n + 1"),
        expr("h", 1, "n + 5"),
        declared(tripled_moved(), 1),
    ];
    load.extend((1..=MANY).map(|i| wrote("doc", &key(i), 1, json!({"n": i}))));
    let written = l.bodies(load).split_off(4);
    l.probe.take();
    let mark = |b: EventBody| if set { going_on(b) } else { b };
    let q = l.bodies(vec![
        mark(expr("f", 2, "n * 3")),
        mark(expr("g", 2, "n + 2")),
        expr("h", 2, "n + 6"),
    ]);
    (l, written, q)
}

#[test]
fn row_22_a_set_computes_its_records_again_once_at_its_last_declaration() {
    let (l, written, q) = three_again("recompute-22", true);
    let asked = l.probe.take();
    for i in 1..=MANY {
        let k = key(i);
        let w = written[usize::try_from(i - 1).expect("an index")].0;
        let at = |p: usize| l.state(Some(q[p]), "doc", &k);
        let before = json!({"n": i, "x": i * 2, "y": i + 1, "z": i + 5, "moved": true});
        assert_eq!(
            [at(0), at(1), at(2)],
            [
                json!([before, 1, w]),
                json!([before, 1, w]),
                json!([{"n": i, "x": i * 3, "y": i + 2, "z": i + 6, "moved": true}, 1, q[2].0])
            ],
            "row 22: `{k}` is as before the set at its first two declarations, and takes all \
             three functions at its last"
        );
    }
    let meta = |name: &str| l.meta(None, "doc", &key(1), name).expect("provenance");
    for name in ["x", "y", "z"] {
        let m = meta(name);
        assert_eq!(
            (m.position, m.procedure_version.as_deref()),
            (q[2], Some("2")),
            "row 22: `{name}` takes the set's last declaration and its function's version"
        );
    }
    assert_eq!(
        scans(&asked),
        3,
        "row 22: a set of three functions of one type of {MANY} records reads its three pages \
         once"
    );
}

#[test]
fn row_23_a_field_that_reads_its_change_is_computed_once_over_the_set() {
    let (each, _, _) = three_again("recompute-23-each", false);
    let (set, _, _) = three_again("recompute-23-set", true);
    let moved =
        |l: &Folded| l.record(None, "doc", &key(1)).expect("the record").fields["moved"].clone();
    assert_eq!(
        (moved(&each), moved(&set)),
        (json!(false), json!(true)),
        "row 23: computed at each declaration, `changed(y)` reads `h`'s, which leaves `y`; \
         computed once over the set, it reads the set's, which changed `y`"
    );
}

#[test]
fn row_24_a_page_of_a_rebuild_never_ends_inside_a_set() {
    let (l, _) = doubling("recompute-24", &[("a", 1), ("b", 2), ("c", 3)]);
    let n = 1_100u32;
    let mut set: Vec<EventBody> = (2..=n).map(|v| going_on(expr("f", v, "n * 3"))).collect();
    set.push(expr("f", n + 1, "n * 4"));
    let q = l.bodies(set);
    let live = l.history(&KEYS[..3]);
    l.probe.take();
    let head = l.lake.head();
    assert_eq!(l.lake.rebuild().expect("the rebuild"), head);
    assert_eq!(
        l.history(&KEYS[..3]),
        live,
        "row 24: every position answers as the live lake did"
    );
    assert_eq!(
        commits(&l.probe.take()),
        [head],
        "row 24: the first page of the log ends inside a set of {} declarations, so it reads \
         on to the set's end, and commits once",
        q.len()
    );
}

/// The lake of rows 29 and 30: two sets back to back. The first is of
/// functions that carry the mark, then an object set, its last `declared`,
/// at `close`, and the object set's three memberships; the second of
/// `second` such functions and one more. Answers the lake and the first
/// set's last position.
fn two_sets(name: &str, close: u64, second: u64) -> (Folded, Position) {
    let (l, _) = doubling(name, &[("a", 1), ("b", 2), ("c", 3)]);
    let going = close - (l.lake.head().0 + 1);
    let v = |v: u64| u32::try_from(v).expect("a version");
    let functions =
        |from: u64, n: u64| (from..from + n).map(move |i| going_on(expr("f", v(i), "n * 3")));
    let pos = json!({"declare": "object_set", "name": "pos", "of": "doc", "where": "n > 0"});
    let mut first: Vec<EventBody> = functions(2, going).collect();
    first.push(declaration(pos, 1));
    first.extend(["a", "b", "c"].map(|k| EventBody::ObjectSetChanged {
        set: "pos".into(),
        ty: "doc".into(),
        key: k.into(),
        joined: true,
    }));
    let end = *l.bodies(first).last().expect("the first set");
    assert_eq!(end, Position(close + 3), "the set closes at {close}");
    let mut set: Vec<EventBody> = functions(going + 2, second).collect();
    set.push(expr("f", v(going + second + 2), "n * 4"));
    l.bodies(set);
    (l, end)
}

/// A rebuild of `l`, checked against the live lake: its commits, and the
/// events the log's reads handed out by each.
fn rebuilt(l: &Folded) -> (Vec<Position>, Vec<(Position, u64)>) {
    let live = l.history(&KEYS[..3]);
    l.probe.take();
    let (probe, read) = (l.probe.clone(), Arc::new(Mutex::new(Vec::new())));
    let into = read.clone();
    let was = probe.events_read.load(std::sync::atomic::Ordering::SeqCst);
    *l.probe.before_commit.lock().unwrap() = Some(Box::new(move |through| {
        let n = probe.events_read.load(std::sync::atomic::Ordering::SeqCst) - was;
        into.lock().unwrap().push((through, n as u64));
    }));
    let head = l.lake.head();
    assert_eq!(l.lake.rebuild().expect("the rebuild"), head);
    *l.probe.before_commit.lock().unwrap() = None;
    assert_eq!(
        l.history(&KEYS[..3]),
        live,
        "every position answers as the live lake did"
    );
    let read = read.lock().unwrap().clone();
    (commits(&l.probe.take()), read)
}

#[test]
fn row_29_a_page_of_a_rebuild_reads_on_only_to_the_end_of_its_set() {
    // The chunk the first page reads on closes the first set, at 1 200,
    // and goes on into the second, three pages long.
    let (l, first) = two_sets("recompute-29", 1_200, 3 * 1_024);
    let (commits, read) = rebuilt(&l);
    // Each page reads what it holds and at most a chunk more, read again
    // by the next page.
    let mut at = (0, 0);
    for &(through, n) in &read {
        let (held, reads) = (through.0 + 1 - at.0, n - at.1);
        assert!(
            reads <= held + 1_024,
            "row 29: the page through {through} holds {held} events and read {reads}: it reads \
             on only to the end of its set, and at most a chunk beyond it"
        );
        at = (through.0 + 1, n);
    }
    assert_eq!(
        commits,
        [first, l.lake.head()],
        "row 29: the first set is the first page, the second set the second"
    );
}

#[test]
fn row_30_a_page_whose_set_closes_at_a_chunks_end_reads_on_for_its_memberships() {
    // The first set's last `declared` is the last event of the chunk the
    // first page reads on; its memberships begin the next chunk.
    let (l, first) = two_sets("recompute-30", 2 * 1_024 - 1, 10);
    let (commits, _) = rebuilt(&l);
    assert_eq!(
        commits,
        [first, l.lake.head()],
        "row 30: the first page reads on past the chunk for the set's memberships, so no \
         commit falls between a declaration and the memberships it changed"
    );
}

#[test]
fn row_25_a_log_that_ends_inside_a_set_is_not_folded() {
    let (l, _) = doubling("recompute-25", &[("a", 1)]);
    let now = l.lake.clock(&mut l.lake.writer());
    let refused = l.try_append(vec![Draft::system(now, going_on(expr("f", 2, "n * 3")))]);
    let why = refused
        .expect_err("the fold refuses a log that ends inside a set")
        .to_string();
    assert!(
        why.contains("the log ends inside a set of declarations"),
        "row 25: the refusal says why: {why}"
    );
}

#[test]
fn row_26_a_set_makes_one_call_due_for_a_record_at_its_last_declaration() {
    let l = summing("recompute-26", &[("doc", "a", "A")]);
    let failure = json!({"error": "boom"});
    l.bodies(vec![output(json!(1), "A", failure, Some(("doc", "a")))]);
    l.due();
    let q = l.bodies(vec![
        going_on(http(2, json!(1))),
        declared(summed("doc"), 2),
    ]);
    assert_eq!(
        l.due(),
        [(
            q[1].0,
            "doc".into(),
            "a".into(),
            "sum".into(),
            json!({"title": "A"}),
            true
        )],
        "row 26: a set that declares `f` and `doc` again makes `doc` `a`'s call due once, \
         caused by the set's last declaration, and calls its recorded failure again once"
    );
}

#[test]
fn row_27_a_kept_view_over_a_set_is_made_once_after_it() {
    let (l, _) = doubling("recompute-27", &[("a", 1), ("b", 2), ("c", 3)]);
    let total = json!({"declare": "type", "name": "total", "key": ["n"],
        "fields": [{"name": "n", "type": {"kind": "number"}},
                   {"name": "t", "type": {"kind": "number"}}],
        "view": {"source": "doc", "group_by": ["n"],
                 "measures": [{"name": "t", "expr": "sum(twice)"}]}});
    let asking = "view_kept.total";
    let setting = json!({"key": asking, "value": true});
    l.bodies(vec![
        declaration(total, 1),
        wrote(lakelands_api::seam::SETTING_TYPE, asking, 1, setting),
    ]);
    l.probe.take();
    let q = l.bodies(vec![
        going_on(declared(doubled_plus("n + 1"), 2)),
        expr("f", 2, "n * 3"),
    ]);
    let kept = lakelands_api::seam::kept_name(&"total".into());
    let puts = |at: Position| {
        let asked = l.probe.take();
        let of = |a: &&Asked| matches!(a, Asked::Put(ty, _, p) if *ty == kept && *p == at);
        let n = asked.iter().filter(of).count();
        l.probe.asked.lock().expect("the probe").extend(asked);
        n
    };
    assert_eq!(
        (puts(q[0]), puts(q[1])),
        (3, 3),
        "row 27: the set's first declaration ends the view's three records, and the view is \
         made once, after the set's records are computed again, not again from the records \
         before the set"
    );
    let t = |n: i64| {
        l.record(None, kept.as_str(), &n.to_string())
            .map(|r| r.fields["t"].clone())
    };
    assert_eq!(
        [t(1), t(2), t(3)],
        [Some(json!(3)), Some(json!(6)), Some(json!(9))],
        "row 27: the view's records sum the set's `twice`"
    );
}

#[test]
fn row_28_a_page_never_ends_inside_a_set_among_its_object_sets_memberships() {
    let l = many("recompute-28", |i| i);
    let joined = |i: i64| EventBody::ObjectSetChanged {
        set: "pos".into(),
        ty: "doc".into(),
        key: key(i).as_str().into(),
        joined: true,
    };
    let pos = json!({"declare": "object_set", "name": "pos", "of": "doc", "where": "n > 0"});
    // As `apply` drafts it: the set's memberships after its declaration,
    // more than a page of them, none marked; `g` the set's last.
    let mut set = vec![
        going_on(expr("f", 2, "n * 3")),
        going_on(declaration(pos, 1)),
    ];
    set.extend((1..=MANY).map(joined));
    set.push(expr("g", 1, "n + 1"));
    l.bodies(set);
    let keys = [("doc", "k0005"), ("doc", "k2500")];
    let live = l.history(&keys);
    let head = l.lake.head();
    assert_eq!(l.lake.rebuild().expect("the rebuild"), head);
    assert_eq!(
        l.history(&keys),
        live,
        "row 28: a page of the rebuild that would end among the set's {MANY} memberships reads \
         on to the set's last declaration, so every position answers as the live lake did"
    );
    assert_eq!(
        l.record(None, "doc", "k0005").expect("the record").fields["twice"],
        json!(15),
        "row 28: `twice` takes the set's `f` once, at its last declaration"
    );
}

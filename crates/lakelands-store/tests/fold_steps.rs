//! What the fold of a `migrated` event leaves, where a later read goes
//! through the step: the fold keeps the migration, not the records it
//! changed, so a record the transaction did not write is read from the
//! view and taken through every step folded so far (lakelands#480).
//!
//! A rebuild of a short log folds over an empty view, and on a live lake no
//! append holds a record event beside a `migrated`. So each test here
//! appends hand-built drafts, in one transaction, over a projection that
//! already holds records, on the memory engine, and states the fold's
//! result literally: read by a later event of the transaction, from the
//! committed head, and as of a step's position.

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::Seams;
use lakelands_api::write::{Declaration, Migration, Rekey};
use lakelands_api::{FieldName, Location, Position, Result, Version};
use lakelands_core::conformance::fixture::{
    computed_by, field, object_set_on, patch, reference, type_decl,
};
use lakelands_engine_mem::MemoryOpener;
use lakelands_store::lake::Lake;
use lakelands_store::log::Draft;
use serde_json::{Value, json};

/// A lake on the memory engine, written by hand-built events.
struct Folded(Lake);

impl Folded {
    fn new(name: &str) -> Folded {
        let seams = Seams {
            engines: vec![Arc::new(MemoryOpener::new())],
            ..Seams::default()
        };
        Folded(Lake::open(&Location::memory(name), seams, OpenMode::Create).expect("a new lake"))
    }

    /// One append, folded in one transaction; the events' positions.
    fn try_append(&self, bodies: Vec<EventBody>) -> Result<Vec<Position>> {
        let mut w = self.0.writer();
        let now = self.0.clock(&mut w);
        let drafts = bodies.into_iter().map(|b| Draft::system(now, b)).collect();
        let events = self.0.append(&mut w, drafts)?;
        Ok(events.iter().map(|e| e.position).collect())
    }

    fn append(&self, bodies: Vec<EventBody>) -> Vec<Position> {
        self.try_append(bodies).expect("the append folds")
    }

    /// A record as the projection holds it at `at` (`None`: the head).
    fn record(&self, at: Option<Position>, ty: &str, key: &str) -> Option<StoredRecord> {
        let view = self.0.engine.view(at).expect("a view");
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

    fn members(&self, at: Option<Position>, set: &str, ty: &str) -> Vec<String> {
        let view = self.0.engine.view(at).expect("a view");
        let held = view.members(set, &ty.into()).expect("the members");
        held.iter().map(|k| k.to_string()).collect()
    }
}

fn declared(decl: TypeDecl) -> EventBody {
    EventBody::Declared {
        schema_hash: Some(lakelands_core::shape::schema_hash(&decl).expect("a schema hash")),
        declaration: Declaration::Type(decl),
        version: 1,
    }
}

fn wrote(ty: &str, key: &str, version: u64, fields: Value) -> EventBody {
    EventBody::RecordUpserted {
        ty: ty.into(),
        key: key.into(),
        version: Version(version),
        patch: patch(fields),
    }
}

/// A `migrated` event to `to`, renaming `renames` and moving `moves`
/// (`type`, old key, new key).
fn migrated(to: TypeDecl, renames: &[(&str, &str)], moves: &[(&str, &str, &str)]) -> EventBody {
    EventBody::Migrated {
        schema_hash: lakelands_core::shape::schema_hash(&to).expect("a schema hash"),
        migration: Migration {
            to,
            renames: (renames.iter())
                .map(|(from, to)| ((*from).into(), (*to).into()))
                .collect(),
            relation_renames: Vec::new(),
        },
        version: 2,
        report_id: "by hand".into(),
        rekeyed: (moves.iter())
            .map(|(ty, from, to)| Rekey {
                ty: (*ty).into(),
                from: (*from).into(),
                to: (*to).into(),
            })
            .collect(),
    }
}

/// `doc`, with `n` of `kind`.
fn doc(n: lakelands_api::ontology::StorageKind) -> TypeDecl {
    type_decl("doc", vec![field("title", Text), field("n", n)])
}

/// `note`, which refers to a `doc`, with `n` of `kind`.
fn note(n: lakelands_api::ontology::StorageKind) -> TypeDecl {
    type_decl("note", vec![reference("about", "doc"), field("n", n)])
}

#[test]
fn a_later_write_lands_on_the_record_the_step_converted() {
    let l = Folded::new("fold-steps-1");
    let p = l.append(vec![
        declared(doc(Text)),
        wrote("doc", "a", 1, json!({"title": "T", "n": "12"})),
    ]);
    let renamed = type_decl("doc", vec![field("name", Text), field("n", Number)]);
    let q = l.append(vec![
        migrated(renamed, &[("title", "name")], &[]),
        wrote("doc", "a", 2, json!({"n": 13})),
    ]);
    assert_eq!(
        l.state(None, "doc", "a"),
        json!([{"name": "T", "n": 13}, 2, q[1].0]),
        "a sparse patch after the step lands on the converted fields"
    );
    assert_eq!(
        l.state(Some(q[0]), "doc", "a"),
        json!([{"name": "T", "n": 12}, 1, q[0].0]),
        "as of the step the record is converted, at the step's position"
    );
    let a = l.record(None, "doc", "a").expect("the record");
    let written_at = |f: &str| a.meta.get(&FieldName::from(f)).map(|m| m.position);
    assert_eq!(
        (written_at("name"), written_at("n"), written_at("title")),
        (Some(p[1]), Some(q[1]), None),
        "a renamed field keeps its provenance; a field written later takes the write's"
    );
}

#[test]
fn a_record_written_before_the_step_is_not_converted_from_the_view() {
    let l = Folded::new("fold-steps-2");
    l.append(vec![
        declared(doc(Text)),
        wrote("doc", "a", 1, json!({"n": "abc"})),
        wrote("doc", "v", 1, json!({"n": "7"})),
    ]);
    let again = type_decl("doc", vec![field("title", Text), field("m", Number)]);
    let q = l.append(vec![
        wrote("doc", "a", 2, json!({"n": "12"})),
        wrote("doc", "new", 1, json!({"n": "3"})),
        migrated(doc(Number), &[], &[]),
        migrated(again, &[("n", "m")], &[]),
        wrote("doc", "new", 2, json!({"title": "N"})),
    ]);
    assert_eq!(
        l.state(None, "doc", "a"),
        json!([{"m": 12}, 2, q[3].0]),
        "the view's `abc` does not convert; the record is what the transaction wrote"
    );
    assert_eq!(l.state(None, "doc", "v"), json!([{"m": 7}, 1, q[3].0]));
    assert_eq!(
        l.state(None, "doc", "new"),
        json!([{"m": 3, "title": "N"}, 2, q[4].0]),
        "a record created in the transaction is read through both steps"
    );
}

#[test]
fn a_rekey_moves_records_of_the_view_and_a_later_write_finds_them() {
    let l = Folded::new("fold-steps-4");
    l.append(vec![declared(doc(Text))]);
    let p = l.append(
        ["k1", "s1", "s2", "c1", "c2"]
            .map(|k| wrote("doc", k, 1, json!({"title": k})))
            .to_vec(),
    );
    let moves = [
        ("doc", "k1", "n1"),
        ("doc", "s1", "s2"),
        ("doc", "s2", "s1"),
        ("doc", "c1", "c2"),
        ("doc", "c2", "c3"),
    ];
    let q = l.append(vec![
        migrated(doc(Text), &[], &moves),
        wrote("doc", "n1", 2, json!({"n": "new"})),
        wrote("doc", "k1", 1, json!({"n": "fresh"})),
    ]);
    let at = |key| l.state(None, "doc", key);
    assert_eq!(
        at("n1"),
        json!([{"title": "k1", "n": "new"}, 2, q[1].0]),
        "a write at the new key lands on the moved record's fields"
    );
    assert_eq!(
        at("k1"),
        json!([{"n": "fresh"}, 1, q[2].0]),
        "a write at the old key holds the patch alone"
    );
    assert_eq!(
        (at("s1"), at("s2")),
        (
            json!([{"title": "s2"}, 1, q[0].0]),
            json!([{"title": "s1"}, 1, q[0].0])
        ),
        "two records swap keys"
    );
    assert_eq!(
        (at("c1"), at("c2"), at("c3")),
        (
            json!("ended"),
            json!([{"title": "c1"}, 1, q[0].0]),
            json!([{"title": "c2"}, 1, q[0].0])
        ),
        "a chain moves each record one key on and ends the first"
    );
    assert_eq!(
        l.state(Some(p[4]), "doc", "c2"),
        json!([{"title": "c2"}, 1, p[4].0]),
        "before the step the key held its own record"
    );
}

#[test]
fn a_second_step_finds_what_the_transaction_wrote_at_a_moved_key() {
    // 4a: the view's record does not convert; what was written at its new
    // key does.
    let l = Folded::new("fold-steps-4a");
    l.append(vec![
        declared(doc(Text)),
        wrote("doc", "a", 1, json!({"n": "abc"})),
    ]);
    let q = l.append(vec![
        migrated(doc(Text), &[], &[("doc", "a", "b")]),
        wrote("doc", "b", 2, json!({"n": "12"})),
        migrated(doc(Number), &[], &[]),
    ]);
    assert_eq!(l.state(None, "doc", "b"), json!([{"n": 12}, 2, q[2].0]));
    assert_eq!(l.state(None, "doc", "a"), json!("ended"));

    // 4b: a new record at the old key does not stand in for the moved one.
    let l = Folded::new("fold-steps-4b");
    l.append(vec![
        declared(doc(Text)),
        wrote("doc", "a", 1, json!({"n": "5"})),
    ]);
    let q = l.append(vec![
        migrated(doc(Text), &[], &[("doc", "a", "b")]),
        wrote("doc", "a", 1, json!({"n": "9"})),
        migrated(doc(Number), &[], &[]),
    ]);
    assert_eq!(
        (l.state(None, "doc", "b"), l.state(None, "doc", "a")),
        (json!([{"n": 5}, 1, q[2].0]), json!([{"n": 9}, 1, q[2].0])),
        "at the head the moved record holds both steps' state, the old key the new record"
    );
    assert_eq!(
        (
            l.state(Some(q[0]), "doc", "b"),
            l.state(Some(q[0]), "doc", "a")
        ),
        (json!([{"n": "5"}, 1, q[0].0]), json!("ended")),
        "as of the first step"
    );
    assert_eq!(
        l.state(Some(q[1]), "doc", "a"),
        json!([{"n": "9"}, 1, q[1].0]),
        "as of the write between the steps"
    );
}

#[test]
fn a_key_a_written_record_left_holds_the_record_that_arrives() {
    // 4c: one step moves a record of the transaction off `x` and a record
    // of the view onto it.
    let l = Folded::new("fold-steps-4c");
    l.append(vec![
        declared(doc(Text)),
        wrote("doc", "v", 1, json!({"title": "V"})),
    ]);
    let q = l.append(vec![
        wrote("doc", "x", 1, json!({"title": "W"})),
        migrated(doc(Text), &[], &[("doc", "x", "y"), ("doc", "v", "x")]),
    ]);
    let at = |key| l.state(None, "doc", key);
    assert_eq!(
        (at("x"), at("y"), at("v")),
        (
            json!([{"title": "V"}, 1, q[1].0]),
            json!([{"title": "W"}, 1, q[1].0]),
            json!("ended")
        )
    );
}

#[test]
fn a_record_arrives_at_a_key_a_written_one_left_a_step_earlier() {
    // 5: two steps; the view's record arrives at a key a created one left
    // a step earlier, and a later write there lands on it.
    let l = Folded::new("fold-steps-5");
    l.append(vec![
        declared(doc(Text)),
        wrote("doc", "v", 1, json!({"title": "V"})),
    ]);
    let q = l.append(vec![
        wrote("doc", "k", 1, json!({"title": "W"})),
        migrated(doc(Text), &[], &[("doc", "k", "m")]),
        migrated(doc(Text), &[], &[("doc", "v", "k")]),
        wrote("doc", "k", 2, json!({"n": "later"})),
    ]);
    assert_eq!(
        (at_head(&l, "k"), at_head(&l, "m"), at_head(&l, "v")),
        (
            json!([{"title": "V", "n": "later"}, 2, q[3].0]),
            json!([{"title": "W"}, 1, q[2].0]),
            json!("ended")
        )
    );
}

#[test]
fn a_written_record_that_moves_leaves_its_old_key_ended() {
    // 6: a record created in the transaction and moved by a later step
    // leaves its old key ended in the projection.
    let l = Folded::new("fold-steps-6");
    l.append(vec![declared(doc(Text))]);
    let q = l.append(vec![
        wrote("doc", "x", 1, json!({"title": "W"})),
        migrated(doc(Text), &[], &[("doc", "x", "y")]),
    ]);
    assert_eq!(
        (at_head(&l, "x"), at_head(&l, "y")),
        (json!("ended"), json!([{"title": "W"}, 1, q[1].0]))
    );
    assert_eq!(
        l.state(Some(q[0]), "doc", "x"),
        json!([{"title": "W"}, 1, q[0].0])
    );
}

fn at_head(l: &Folded, key: &str) -> Value {
    l.state(None, "doc", key)
}

#[test]
fn a_reference_follows_a_move_unless_the_transaction_wrote_it() {
    // 7: a referencing record the transaction rewrote keeps what it wrote.
    let l = Folded::new("fold-steps-7");
    l.append(vec![
        declared(doc(Text)),
        declared(note(Text)),
        wrote("doc", "a", 1, json!({})),
        wrote("doc", "u", 1, json!({})),
        wrote("note", "n1", 1, json!({"about": "a"})),
        wrote("note", "n2", 1, json!({"about": "a", "n": "3"})),
        wrote("note", "n3", 1, json!({"about": "u"})),
    ]);
    let p3 = l.record(None, "note", "n3").expect("n3").position;
    let q = l.append(vec![
        wrote("note", "n1", 2, json!({"about": "u"})),
        migrated(doc(Text), &[], &[("doc", "a", "b")]),
        // 8: the referencing type itself migrated by a later step.
        migrated(note(Number), &[], &[]),
    ]);
    let at = |at, key| l.state(at, "note", key);
    assert_eq!(
        at(Some(q[1]), "n1"),
        json!([{"about": "u"}, 2, q[0].0]),
        "what the transaction wrote stays, at its position"
    );
    assert_eq!(
        (at(Some(q[1]), "n2"), at(Some(q[1]), "n3")),
        (
            json!([{"about": "b", "n": "3"}, 1, q[1].0]),
            json!([{"about": "u"}, 1, p3.0])
        ),
        "a reference to a moved key is rewritten at the step; another record is left alone"
    );
    assert_eq!(
        (at(None, "n1"), at(None, "n2")),
        (
            json!([{"about": "u"}, 2, q[2].0]),
            json!([{"about": "b", "n": 3}, 1, q[2].0])
        ),
        "the later step converts the rewritten record"
    );
}

#[test]
fn an_ended_key_a_flagged_record_and_a_missing_move() {
    // 9: a key ended in an earlier transaction is no record to convert.
    let l = Folded::new("fold-steps-9");
    l.append(vec![
        declared(doc(Text)),
        wrote("doc", "a", 1, json!({"title": "x"})),
        wrote("doc", "r", 1, json!({"title": "r", "n": "4"})),
        wrote("doc", "h", 1, json!({"title": "h", "n": "5"})),
        EventBody::RecordRetired {
            ty: "doc".into(),
            key: "r".into(),
            version: Version(2),
        },
        EventBody::RecordRestricted {
            ty: "doc".into(),
            key: "h".into(),
            version: Version(2),
            reason: "held".into(),
        },
    ]);
    l.append(vec![migrated(doc(Text), &[], &[("doc", "a", "b")])]);
    let mut required = doc(Number);
    required.fields[0].required = true;
    let q = l.append(vec![
        migrated(required.clone(), &[], &[]),
        wrote("doc", "a", 1, json!({"title": "again"})),
    ]);
    assert_eq!(
        l.state(None, "doc", "a"),
        json!([{"title": "again"}, 1, q[1].0]),
        "a write at a key ended before holds the patch alone"
    );
    // 10: retired and restricted records are converted, flags and version kept.
    let flags = |key| {
        let r = l.record(None, "doc", key).expect("the record");
        (
            json!(r.fields),
            r.version.0,
            r.position,
            r.retired,
            r.restricted,
        )
    };
    assert_eq!(
        flags("r"),
        (json!({"title": "r", "n": 4}), 2, q[0], true, false)
    );
    assert_eq!(
        flags("h"),
        (json!({"title": "h", "n": 5}), 2, q[0], false, true)
    );

    // 12: a listed move whose record does not exist fails the fold.
    let refused = l.try_append(vec![migrated(required, &[], &[("doc", "gone", "z")])]);
    let message = refused.expect_err("the fold fails").to_string();
    assert!(
        message.contains("moves `doc` `gone`, which the lake does not hold"),
        "{message}"
    );
}

#[test]
fn memberships_move_with_the_keys_through_two_steps() {
    // 12a: a chain in one step, an `object_set_changed` between two steps.
    let l = Folded::new("fold-steps-12a");
    let changed = |key: &str, joined| EventBody::ObjectSetChanged {
        set: "kept".into(),
        ty: "doc".into(),
        key: key.into(),
        joined,
    };
    l.append(vec![
        declared(doc(Text)),
        EventBody::Declared {
            declaration: object_set_on("kept", "doc", "title = 'k'"),
            schema_hash: None,
            version: 1,
        },
        wrote("doc", "a", 1, json!({"title": "k"})),
        wrote("doc", "b", 1, json!({"title": "k"})),
        wrote("doc", "o", 1, json!({"title": "o"})),
        changed("a", true),
        changed("b", true),
    ]);
    let q = l.append(vec![
        migrated(doc(Text), &[], &[("doc", "a", "b"), ("doc", "b", "c")]),
        changed("c", false),
        migrated(doc(Text), &[], &[("doc", "b", "d"), ("doc", "o", "b")]),
    ]);
    let members = |at| l.members(at, "kept", "doc");
    assert_eq!(members(Some(q[0])), ["b", "c"], "after the chain");
    assert_eq!(members(Some(q[1])), ["b"], "after `c` left");
    assert_eq!(
        members(None),
        ["d"],
        "after the second step: `b` moved on, and what arrives at `b` was no member"
    );
}

#[test]
fn a_type_of_several_pages_is_converted_whole() {
    // 13: an ended key inside the view's first page, and a record written
    // before the step that sorts into the second.
    const N: u64 = 2_500;
    let l = Folded::new("fold-steps-13");
    let key = |i: u64| format!("k{i:04}");
    let mut load = vec![declared(doc(Text))];
    load.extend((0..N).map(|i| wrote("doc", &key(i), 1, json!({"n": i.to_string()}))));
    l.append(load);
    l.append(vec![migrated(doc(Text), &[], &[("doc", "k0005", "z")])]);
    let q = l.append(vec![
        wrote("doc", "k1500", 2, json!({"n": "77"})),
        migrated(doc(Number), &[], &[]),
    ]);
    for i in 0..N {
        let expected = match i {
            5 => json!("ended"),
            1500 => json!([{"n": 77}, 2, q[1].0]),
            _ => json!([{"n": i}, 1, q[1].0]),
        };
        assert_eq!(l.state(None, "doc", &key(i)), expected, "{}", key(i));
    }
    assert_eq!(l.state(None, "doc", "z"), json!([{"n": 5}, 1, q[1].0]));
}

/// The function `f`, computed outside the lake, at `version`.
fn function(version: u32) -> EventBody {
    let f = json!({"declare": "function", "name": "f", "backing": "http",
                   "inputs": [{"name": "title", "type": {"kind": "text"}}]});
    EventBody::Declared {
        declaration: serde_json::from_value(f).expect("a function"),
        schema_hash: None,
        version,
    }
}

/// `doc` with `sum`, which `f` computes from `title`.
fn summed() -> TypeDecl {
    let sum = computed_by("sum", Text, "f", &["title"]);
    type_decl("doc", vec![field("title", Text), sum])
}

#[test]
fn an_output_fills_the_record_at_its_moved_key() {
    // 6a: an `output_recorded` after the step names the record's new key.
    let l = Folded::new("fold-steps-6a");
    l.append(vec![
        function(1),
        declared(summed()),
        wrote("doc", "a", 1, json!({"title": "A"})),
    ]);
    let due = l.0.writer().due.pop().expect("the call `a` makes is due");
    let q = l.append(vec![
        migrated(summed(), &[], &[("doc", "a", "b")]),
        EventBody::OutputRecorded {
            seam: due.seam,
            input: due.input,
            output: json!("S"),
            field: Some(("doc".into(), "b".into(), "sum".into())),
        },
    ]);
    assert_eq!(
        l.state(None, "doc", "b"),
        json!([{"title": "A", "sum": "S"}, 1, q[1].0])
    );
}

#[test]
fn derived_fields_are_computed_again_over_the_records_as_the_step_left_them() {
    // 14: a re-key, a new record at the old key, then the function declared
    // again: every record of the type is computed again once, in the order
    // of the keys they have now.
    let l = Folded::new("fold-steps-14");
    l.append(vec![
        function(1),
        declared(summed()),
        wrote("doc", "a", 1, json!({"title": "A"})),
        wrote("doc", "c", 1, json!({"title": "C"})),
    ]);
    l.0.writer().due.clear();
    let q = l.append(vec![
        migrated(summed(), &[], &[("doc", "a", "z")]),
        wrote("doc", "a", 1, json!({"title": "fresh"})),
        function(2),
    ]);
    let w = l.0.writer();
    let due: Vec<_> = (w.due.iter())
        .map(|d| {
            let lakelands_store::backed::Call::Function { call, .. } = &d.call else {
                panic!("a function's call is due");
            };
            (d.cause, d.key.to_string(), call.inputs.clone())
        })
        .collect();
    let title = |t: &str| json!({"title": t});
    assert_eq!(
        due,
        [
            (q[1], "a".to_owned(), title("fresh")),
            (q[2], "a".to_owned(), title("fresh")),
            (q[2], "c".to_owned(), title("C")),
            (q[2], "z".to_owned(), title("A")),
        ],
        "the write makes its own call due; the declaration then every record's, the moved \
         one under its new key and the written one from what was written"
    );
}

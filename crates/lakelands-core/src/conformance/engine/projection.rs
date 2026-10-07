//! The projection through the seam: kept history, the fold's
//! transactions, and edges (`docs/seams.md` §Engine,
//! `docs/decisions/026-the-store-walks-the-graph.md`).
//!
//! Every put adds a state valid from a position and keeps the one it
//! supersedes, so a view at an earlier position reads the states then
//! without replaying the log; a position the projection has not folded
//! is refused. The fold's writes land whole when it commits and not at
//! all when it rolls back. Edges are derived from records and the
//! relations in force, the same on every engine, because the store's
//! walks are built on them.

use std::collections::BTreeMap;

use lakelands_api::Refusal;
use lakelands_api::engine::{LakeEngine, ProjectionStore, ProjectionView};
use lakelands_api::event::Digest;
use lakelands_api::id::{Position, RecordKey, RelationName, TypeName, Version};
use lakelands_api::query::Walk;
use lakelands_api::record::StoredRecord;
use lakelands_api::write::Declaration;
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::fixture::{Must, is_invalid, reason};

engine_check!(
    /// A view at a position reads the states valid then.
    VIEW_AT_A_POSITION = a_view_at_a_position_sees_the_states_then,
    "seams §Engine: the projection keeps every superseded state, so a view at an earlier position reads the lake as it was right after folding it"
);

engine_check!(
    /// A view past the folded position is refused.
    PAST_THE_FOLD = a_view_past_the_folded_position_is_refused,
    "seams §Engine: a view at a position the projection has not folded is refused as out of range, naming the head"
);

engine_check!(
    /// A rolled-back transaction leaves nothing.
    ROLLBACK = a_rolled_back_transaction_leaves_nothing,
    "seams §Engine: the fold writes one transaction per append, so a crash or an abandoned append leaves the projection at a whole append"
);

engine_check!(
    /// Edges walk in, out and both.
    EDGES_IN_OUT_BOTH = edges_walk_in_out_and_both,
    "026: out gives the links a record declares, in the links that point at it, both their union with each link once; the store's walks are built on them"
);

engine_check!(
    /// Links to a retired record are returned.
    EDGES_TO_RETIRED = edges_to_a_retired_record_are_returned,
    "026: an engine returns a link to a retired or missing record as it is; hiding that end is the store's"
);

engine_check!(
    /// Edges over many keys answer each key as it is answered alone.
    EDGES_OF_MANY_KEYS = edges_over_many_keys_answer_each_key_as_it_answers_alone,
    "seams §ProjectionView (lakelands#227): edges over many records of one type answer one list per key, in the keys' order, each what the key alone answers, at the head and at an earlier position; a link between two of the keys is in both lists"
);

engine_check!(
    /// `get_many` answers what `get` answers, key by key.
    GET_MANY = get_many_answers_what_get_answers_key_by_key,
    "seams §ProjectionView (lakelands#227): get_many answers each record of the keys that exists at the view's position, retired and restricted ones included, once, in key order; a key with no record is left out"
);

engine_check!(
    /// Edges follow a record's latest state.
    EDGES_FOLLOW_THE_LATEST_STATE = edges_follow_a_records_latest_state,
    "026: a record put again links where its new state says, and no longer where its earlier state did, in both directions"
);

engine_check!(
    /// A commit backwards, or with a put past `through`, is invalid.
    COMMIT_INVALID = a_commit_backwards_or_with_a_put_past_through_is_invalid,
    "seams §ProjectionStore: a commit never moves the projection backwards, and never commits a state valid from after the position it says it folded through"
);

engine_check!(
    /// A view does not see an open transaction.
    OPEN_TRANSACTION = a_view_does_not_see_an_open_transaction,
    "seams §ProjectionStore: nothing put in a transaction is visible to any view until it commits, and then all of it at once"
);

engine_check!(
    /// A view keeps its position after later commits.
    VIEW_KEEPS_POSITION = a_view_keeps_its_position_after_later_commits,
    "seams §LakeEngine: a view keeps answering its own position while the fold moves on"
);

engine_check!(
    /// A put at the same position replaces the state there.
    SAME_POSITION = a_put_at_the_same_position_replaces_the_state_there,
    "seams §ProjectionStore: a put of the same thing at the same position replaces the state there; the fold writing a state again never adds a second one"
);

engine_check!(
    /// A put before a later state ends where that state begins.
    PUT_BEFORE_A_LATER_STATE = a_put_before_a_later_state_ends_where_that_state_begins,
    "seams §ProjectionStore: a state put at a position before a later state of the same thing is valid until that later state begins, which stays the latest"
);

engine_check!(
    /// Several puts in one transaction leave what they leave one by one.
    PUTS_IN_ONE_TRANSACTION = several_puts_of_one_thing_in_one_transaction_leave_what_they_leave_one_by_one,
    "seams §ProjectionStore: puts of the same things in one transaction, at later positions, at the same one or before a later one, leave the history and the head's indexes a commit after each put would leave"
);

engine_check!(
    /// A key written often is put and read as a key written twice.
    MUCH_WRITTEN_KEY = a_much_written_key_is_put_and_read_as_one_written_twice,
    "seams §Engine (lakelands#596): a key put forty thousand times answers at the head, at an earlier position, in a page and in a count as a key put twice; what its puts and reads cost is counted in each disk engine's own tests"
);

engine_check!(
    /// A head-only name keeps one state for each key.
    HEAD_ONLY_NAME = a_head_only_name_keeps_one_state_for_each_key,
    "seams §ProjectionStore (lakelands#629): under a head-only name, `sys:order.<view>`, a put replaces its key's state and an end deletes it, a rollback leaves what the key held, and a page in key order after a key reads the next key's one row, an ended key between them read as none; it is read at the head only. `end_head_only` deletes every key's state at once, whether put before or earlier in the transaction, and keeps a key put after it in the same transaction"
);

engine_check!(
    /// `clear` empties the projection and the vector index.
    CLEAR = clear_empties_the_projection_and_the_vector_index,
    "seams §ProjectionStore: clear deletes every state, kept history and the vector index included, and leaves nothing folded, for a rebuild from the log"
);

/// A record state of `ty` at `position`, with version 1 and no flags.
pub(super) fn record(ty: &str, key: &str, fields: Value, position: u64) -> StoredRecord {
    StoredRecord {
        ty: TypeName::from(ty),
        key: RecordKey::from(key),
        version: Version(1),
        fields: serde_json::from_value(fields).unwrap_or_default(),
        meta: BTreeMap::new(),
        position: Position(position),
        retired: false,
        restricted: false,
    }
}

/// A view at `as_of`, or a failure saying which view could not open.
pub(super) fn view(
    engine: &dyn LakeEngine,
    as_of: Option<Position>,
) -> Result<Box<dyn ProjectionView + '_>, Failure> {
    engine
        .view(as_of)
        .map_err(|e| Failure::new(format!("opening a view at {as_of:?} failed: {e} ({e:?})")))
}

/// One fold transaction through `through`.
pub(super) fn fold(
    engine: &dyn LakeEngine,
    through: u64,
    puts: impl FnOnce(&dyn ProjectionStore) -> lakelands_api::Result<()>,
) -> Result<(), Failure> {
    let p = engine.projection();
    p.begin().must("beginning a transaction")?;
    puts(p).must("writing the projection")?;
    p.commit(Position(through))
        .must(&format!("committing through position {through}"))
}

/// One record folded in a transaction of its own, committed through
/// `through`: the same begin, put and commit as `fold`.
pub(super) fn fold_one(
    engine: &dyn LakeEngine,
    through: u64,
    state: StoredRecord,
) -> Result<(), Failure> {
    fold(engine, through, |p| p.put_record(&state))
}

fn a_view_at_a_position_sees_the_states_then(engine: &dyn LakeEngine) -> Result<(), Failure> {
    for (n, at) in [(1, 1), (2, 2), (3, 4)] {
        fold_one(engine, at, record("counter", "k", json!({"n": n}), at))?;
    }
    let ty = TypeName::from("counter");
    let key = RecordKey::from("k");
    for (as_of, expected) in [
        (Some(0), None),
        (Some(1), Some(1)),
        (Some(2), Some(2)),
        (Some(3), Some(2)),
        (Some(4), Some(3)),
        (None, Some(3)),
    ] {
        let at = view(engine, as_of.map(Position))?;
        let got = at
            .get(&ty, &key)
            .must("reading the record")?
            .and_then(|r| r.fields.get("n").and_then(Value::as_i64));
        ensure!(
            got == expected,
            "the record was put with n = 1 at position 1, 2 at 2 and 3 at 4; a view at \
             {as_of:?} read n = {got:?}, but the state valid there is {expected:?}"
        );
    }
    Ok(())
}

fn a_view_past_the_folded_position_is_refused(engine: &dyn LakeEngine) -> Result<(), Failure> {
    let refused = engine.view(Some(Position(0))).map(|v| v.position());
    ensure!(
        matches!(reason(&refused), Some(Refusal::OutOfRange { .. })),
        "a view at position 0 of a projection that has folded nothing answered {refused:?}; \
         it must be refused as out of range"
    );
    fold_one(engine, 2, record("counter", "k", json!({}), 2))?;
    view(engine, Some(Position(2)))?;
    let past = engine.view(Some(Position(3))).map(|v| v.position());
    ensure!(
        matches!(
            reason(&past),
            Some(Refusal::OutOfRange { head: Position(2) })
        ),
        "a view at position 3 of a projection folded through 2 answered {past:?}; it must be \
         refused as out of range with head 2"
    );
    Ok(())
}

fn a_rolled_back_transaction_leaves_nothing(engine: &dyn LakeEngine) -> Result<(), Failure> {
    fold_one(engine, 0, record("counter", "a", json!({}), 0))?;
    let p = engine.projection();
    p.begin().must("beginning a transaction")?;
    p.put_record(&record("counter", "b", json!({}), 1))
        .must("putting a record")?;
    p.rollback();
    let folded = p.folded_through().must("reading the folded position")?;
    ensure!(
        folded == Some(Position(0)),
        "after a rolled-back transaction the projection reports it is folded through \
         {folded:?}; it must still be 0"
    );
    // The next append folds position 1 again; the abandoned put must not
    // ride along with it.
    fold_one(engine, 1, record("counter", "c", json!({}), 1))?;
    let at = view(engine, None)?;
    let counter = TypeName::from("counter");
    let b = at
        .get(&counter, &RecordKey::from("b"))
        .must("reading the rolled-back record")?;
    ensure!(
        b.is_none(),
        "a record put in a rolled-back transaction is visible after the next commit: {b:?}"
    );
    let c = at
        .get(&counter, &RecordKey::from("c"))
        .must("reading the committed record")?;
    ensure!(
        c.is_some(),
        "a record put in the transaction after a rollback is not visible once it committed"
    );
    Ok(())
}

/// Invoices billed to customers and replacing other invoices, both
/// relations carried by reference fields.
fn billing(engine: &dyn LakeEngine) -> Result<(), Failure> {
    let invoice: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "invoice", "fields": [],
        "relations": [
            {"name": "billed_to", "target": "customer", "via": "customer"},
            {"name": "replaces", "target": "invoice", "via": "replaces"}
        ]
    }))
    .map_err(|e| Failure::new(format!("the check's declaration does not read: {e}")))?;
    let customer: Declaration =
        serde_json::from_value(json!({"declare": "type", "name": "customer", "fields": []}))
            .map_err(|e| Failure::new(format!("the check's declaration does not read: {e}")))?;
    fold(engine, 0, |p| {
        p.put_declaration(&invoice, 1, None, Position(0))?;
        p.put_declaration(&customer, 1, None, Position(0))?;
        p.put_record(&record("customer", "c1", json!({}), 0))?;
        p.put_record(&record(
            "invoice",
            "i1",
            json!({"customer": "c1", "replaces": "i1"}),
            0,
        ))?;
        p.put_record(&record(
            "invoice",
            "i2",
            json!({"customer": "c1", "replaces": "i1"}),
            0,
        ))
    })
}

/// A record's links, as sorted `from>relation>to` strings; the seam
/// promises no order.
fn links(
    engine: &dyn LakeEngine,
    ty: &str,
    key: &str,
    relation: Option<&str>,
    walk: Walk,
) -> Result<Vec<String>, Failure> {
    let relation = relation.map(RelationName::from);
    let at = view(engine, None)?;
    let mut out: Vec<String> = at
        .edges(
            &TypeName::from(ty),
            &[RecordKey::from(key)],
            relation.as_ref(),
            walk,
        )
        .must(&format!("reading the {walk:?} edges of {ty} {key}"))?
        .remove(0)
        .into_iter()
        .map(|e| format!("{}>{}>{}", e.from.1, e.relation, e.to.1))
        .collect();
    out.sort();
    Ok(out)
}

/// A walk's start (type, key), relation, direction and expected links.
type EdgeCase = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Walk,
    &'static [&'static str],
);

fn edges_walk_in_out_and_both(engine: &dyn LakeEngine) -> Result<(), Failure> {
    billing(engine)?;
    let cases: [EdgeCase; 5] = [
        (
            "invoice",
            "i2",
            None,
            Walk::Out,
            &["i2>billed_to>c1", "i2>replaces>i1"],
        ),
        (
            "invoice",
            "i2",
            Some("replaces"),
            Walk::Out,
            &["i2>replaces>i1"],
        ),
        (
            "customer",
            "c1",
            None,
            Walk::In,
            &["i1>billed_to>c1", "i2>billed_to>c1"],
        ),
        (
            "invoice",
            "i1",
            Some("replaces"),
            Walk::In,
            &["i1>replaces>i1", "i2>replaces>i1"],
        ),
        (
            "invoice",
            "i1",
            None,
            Walk::Both,
            &["i1>billed_to>c1", "i1>replaces>i1", "i2>replaces>i1"],
        ),
    ];
    for (ty, key, relation, walk, expected) in cases {
        let got = links(engine, ty, key, relation, walk)?;
        ensure!(
            got == expected,
            "the {walk:?} edges of {ty} {key} over {relation:?} are {got:?}; they must be \
             {expected:?} (a link from a record to itself counts once)"
        );
    }
    Ok(())
}

fn edges_follow_a_records_latest_state(engine: &dyn LakeEngine) -> Result<(), Failure> {
    billing(engine)?;
    let mut moved = record("invoice", "i2", json!({"customer": "c2"}), 1);
    moved.version = Version(2);
    fold(engine, 1, |p| {
        p.put_record(&record("customer", "c2", json!({}), 1))?;
        p.put_record(&moved)
    })?;
    let out = links(engine, "invoice", "i2", None, Walk::Out)?;
    let old = links(engine, "customer", "c1", None, Walk::In)?;
    let new = links(engine, "customer", "c2", None, Walk::In)?;
    ensure!(
        out == ["i2>billed_to>c2"] && old == ["i1>billed_to>c1"] && new == ["i2>billed_to>c2"],
        "invoice i2 was billed to c1 and replaced i1, then put again billed to c2 alone; its \
         out edges are {out:?}, c1's in edges {old:?} and c2's {new:?}, but i2 links only to \
         c2 now"
    );
    Ok(())
}

fn edges_to_a_retired_record_are_returned(engine: &dyn LakeEngine) -> Result<(), Failure> {
    billing(engine)?;
    let mut retired = record("customer", "c1", json!({}), 1);
    retired.retired = true;
    retired.version = Version(2);
    fold(engine, 1, |p| p.put_record(&retired))?;
    let out = links(engine, "invoice", "i1", Some("billed_to"), Walk::Out)?;
    ensure!(
        out == ["i1>billed_to>c1"],
        "invoice i1 is billed to customer c1, which is retired; its out edges are {out:?}, \
         but the link must be returned as it is"
    );
    let into = links(engine, "customer", "c1", None, Walk::In)?;
    ensure!(
        into == ["i1>billed_to>c1", "i2>billed_to>c1"],
        "the retired customer c1 has in edges {into:?}; the links pointing at it must be \
         returned as they are"
    );
    Ok(())
}

fn a_commit_backwards_or_with_a_put_past_through_is_invalid(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    fold_one(engine, 4, record("t", "a", json!({}), 4))?;
    let p = engine.projection();
    p.begin().must("beginning a transaction")?;
    let backwards = p.commit(Position(3));
    ensure!(
        is_invalid(&backwards),
        "committing through 3 a projection folded through 4 answered {backwards:?}; it must be \
         refused as invalid"
    );
    p.begin().must("beginning a transaction")?;
    p.put_record(&record("t", "b", json!({}), 9))
        .must("putting a record")?;
    let late = p.commit(Position(5));
    ensure!(
        is_invalid(&late),
        "committing through 5 a state valid from 9 answered {late:?}; it must be refused as \
         invalid"
    );
    let folded = p.folded_through().must("reading the folded position")?;
    let b = view(engine, None)?
        .get(&TypeName::from("t"), &RecordKey::from("b"))
        .must("reading the refused state")?;
    ensure!(
        folded == Some(Position(4)) && b.is_none(),
        "after two refused commits the projection is folded through {folded:?} and shows \
         {b:?}; it must be as before, through 4, without `b`"
    );
    Ok(())
}

fn a_view_does_not_see_an_open_transaction(engine: &dyn LakeEngine) -> Result<(), Failure> {
    let t = TypeName::from("t");
    let declared = |fields: Value| {
        serde_json::from_value::<Declaration>(
            json!({"declare": "type", "name": "t", "fields": fields}),
        )
        .map_err(|e| Failure::new(format!("the check's declaration does not read: {e}")))
    };
    let (one, two) = (
        declared(json!([]))?,
        declared(json!([{"name": "n", "type": {"kind": "number"}}]))?,
    );
    let (held, new) = (Digest("1".repeat(64)), Digest("2".repeat(64)));
    fold(engine, 1, |p| {
        p.put_declaration(&one, 1, None, Position(1))?;
        p.put_record(&record("t", "a", json!({"n": 1}), 1))?;
        p.put_record(&record("t", "c", json!({"n": 1}), 1))?;
        p.put_output("seam", &held, &json!("one"), Position(1))?;
        p.put_membership("s", &t, &RecordKey::from("a"), true, Position(1))
    })?;
    let p = engine.projection();
    p.begin().must("beginning a transaction")?;
    // The view a fold reads through: opened before the transaction's puts
    // and read between them, a page at a time.
    let before = view(engine, None)?;
    let page = |after: Option<&str>| -> Result<Vec<(String, Value)>, Failure> {
        let scan = lakelands_api::engine::Scan {
            limit: 1,
            after: after.map(|k| lakelands_api::engine::After {
                values: Vec::new(),
                key: RecordKey::from(k),
            }),
            ..Default::default()
        };
        let rows = before.scan(&t, &scan).must("scanning between the puts")?;
        Ok(rows
            .iter()
            .map(|r| (r.key.to_string(), json!(r.fields)))
            .collect())
    };
    p.put_declaration(&two, 2, None, Position(2))
        .must("putting a declaration")?;
    p.put_record(&record("t", "a", json!({"n": 2}), 2))
        .must("putting a record")?;
    let first = page(None)?;
    // A fold reads the outputs recorded before its transaction through its
    // view, and what the transaction recorded from what it staged.
    for (input, output) in [(&held, "two"), (&new, "new")] {
        p.put_output("seam", input, &json!(output), Position(2))
            .must("putting an output")?;
    }
    let outputs = (
        before.output("seam", &held).must("reading an output")?,
        before.output("seam", &new).must("reading an output")?,
    );
    p.put_record(&record("t", "b", json!({}), 2))
        .must("putting a record")?;
    for (key, member) in [("b", true), ("a", false)] {
        p.put_membership("s", &t, &RecordKey::from(key), member, Position(2))
            .must("putting a membership")?;
    }
    let next = page(Some("a"))?;
    let members = before.members("s", &t).must("reading the members")?;
    let versions: Vec<u32> = (before
        .declarations()
        .must("reading the declarations")?
        .iter())
    .filter(|d| matches!(&d.declaration, Declaration::Type(ty) if ty.name == t))
    .map(|d| d.version)
    .collect();
    drop(before);
    ensure!(
        first == [("a".to_owned(), json!({"n": 1}))]
            && next == [("c".to_owned(), json!({"n": 1}))]
            && members.iter().map(RecordKey::as_str).eq(["a"])
            && versions == [1]
            && outputs == (Some(json!("one")), None),
        "a view opened before a transaction's puts and read between them answered the page \
         {first:?}, after `a` the page {next:?}, the members {members:?}, `t` at versions \
         {versions:?} and the outputs {outputs:?}; it must answer what was committed: `a` \
         with n = 1, then `c`, the member `a`, version 1, and the output recorded before \
         the transaction, none for the input it recorded first"
    );
    let during = view(engine, None)?;
    let (a, b, at) = (
        during
            .get(&TypeName::from("t"), &RecordKey::from("a"))
            .must("reading during the transaction")?
            .and_then(|r| r.fields.get("n").cloned()),
        during
            .get(&TypeName::from("t"), &RecordKey::from("b"))
            .must("reading during the transaction")?,
        during.position(),
    );
    drop(during);
    p.commit(Position(2)).must("committing through 2")?;
    ensure!(
        a == Some(json!(1)) && b.is_none() && at == Some(Position(1)),
        "a view opened during an open transaction read n = {a:?}, `b` = {b:?} at {at:?}; it \
         must see only what was committed: n = 1, no `b`, at position 1"
    );
    let after = view(engine, None)?;
    let n = after
        .get(&TypeName::from("t"), &RecordKey::from("a"))
        .must("reading after the commit")?
        .and_then(|r| r.fields.get("n").cloned());
    let output = after.output("seam", &held).must("reading an output")?;
    ensure!(
        n == Some(json!(2)) && output == Some(json!("two")),
        "after the commit `a` reads n = {n:?} and the output {output:?}; they must be 2 and \
         the output the transaction recorded"
    );
    Ok(())
}

fn a_view_keeps_its_position_after_later_commits(engine: &dyn LakeEngine) -> Result<(), Failure> {
    fold_one(engine, 1, record("t", "a", json!({"n": 1}), 1))?;
    let early = view(engine, None)?;
    fold(engine, 2, |p| {
        p.put_record(&record("t", "a", json!({"n": 2}), 2))?;
        p.put_record(&record("t", "b", json!({}), 2))
    })?;
    let ty = TypeName::from("t");
    let n = early
        .get(&ty, &RecordKey::from("a"))
        .must("reading through the earlier view")?
        .and_then(|r| r.fields.get("n").cloned());
    let rows = early
        .scan(
            &ty,
            &lakelands_api::engine::Scan {
                limit: u32::MAX,
                ..Default::default()
            },
        )
        .must("scanning through the earlier view")?
        .len();
    ensure!(
        early.position() == Some(Position(1)) && n == Some(json!(1)) && rows == 1,
        "a view opened at 1 answered position {:?}, n = {n:?} and {rows} record(s) after a \
         later commit; it must keep answering position 1: n = 1, one record",
        early.position()
    );
    Ok(())
}

fn a_put_at_the_same_position_replaces_the_state_there(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    fold_one(engine, 1, record("t", "a", json!({"n": 1}), 1))?;
    fold(engine, 2, |p| {
        p.put_record(&record("t", "a", json!({"n": 2}), 2))?;
        p.put_record(&record("t", "a", json!({"n": 3}), 2))
    })?;
    let ty = TypeName::from("t");
    let read = |as_of: Option<u64>| -> Result<Option<Value>, Failure> {
        Ok(view(engine, as_of.map(Position))?
            .get(&ty, &RecordKey::from("a"))
            .must("reading the record")?
            .and_then(|r| r.fields.get("n").cloned()))
    };
    let (one, two) = (read(Some(1))?, read(Some(2))?);
    ensure!(
        one == Some(json!(1)) && two == Some(json!(3)),
        "`a` was put with n = 1 at 1, then twice at 2 (n = 2, then n = 3); it read {one:?} at 1 \
         and {two:?} at 2, where the second put replaced the first"
    );
    Ok(())
}

fn clear_empties_the_projection_and_the_vector_index(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    let ty = TypeName::from("t");
    fold_one(engine, 1, record("t", "a", json!({}), 1))?;
    if let Some(index) = engine.vectors() {
        index
            .put(&ty, &RecordKey::from("a"), "emb", &[1.0, 0.0])
            .must("putting a vector")?;
    }
    engine.projection().clear().must("clearing")?;
    let folded = engine
        .projection()
        .folded_through()
        .must("reading the folded position")?;
    ensure!(
        folded.is_none(),
        "after clear the projection is folded through {folded:?}"
    );
    let vectors = match engine.vectors() {
        Some(index) => index.count(&ty, "emb").must("counting vectors")?,
        None => 0,
    };
    ensure!(
        vectors == 0,
        "after clear the vector index holds {vectors} vector(s)"
    );
    fold(engine, 1, |_| Ok(()))?;
    let a = view(engine, None)?
        .get(&ty, &RecordKey::from("a"))
        .must("reading after clear")?;
    ensure!(
        a.is_none(),
        "after clear the record put before it reads {a:?}"
    );
    Ok(())
}

fn a_put_before_a_later_state_ends_where_that_state_begins(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    let words: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "t", "language": "en",
        "fields": [{"name": "word", "type": {"kind": "text"}, "searchable": true}]
    }))
    .expect("a declaration in the check is well-formed");
    fold(engine, 1, |p| {
        p.put_declaration(&words, 1, None, Position(0))?;
        p.put_record(&record("t", "a", json!({"word": "one"}), 1))
    })?;
    fold_one(engine, 5, record("t", "a", json!({"word": "five"}), 5))?;
    fold_one(engine, 5, record("t", "a", json!({"word": "three"}), 3))?;
    let (ty, key) = (TypeName::from("t"), RecordKey::from("a"));
    for (as_of, expected) in [
        (Some(1), "one"),
        (Some(2), "one"),
        (Some(3), "three"),
        (Some(4), "three"),
        (Some(5), "five"),
        (None, "five"),
    ] {
        let got = view(engine, as_of.map(Position))?
            .get(&ty, &key)
            .must("reading the record")?
            .and_then(|r| r.fields.get("word").cloned());
        ensure!(
            got == Some(json!(expected)),
            "`a` was put with `one` at 1 and `five` at 5, then `three` at 3; a view at \
             {as_of:?} read {got:?}, but the state valid there is {expected:?}"
        );
    }
    let head = view(engine, None)?;
    let five = head.lexical(&ty, "five", None, 10).must("searching")?;
    let three = head.lexical(&ty, "three", None, 10).must("searching")?;
    ensure!(
        five.len() == 1 && three.is_empty(),
        "at the head, `five` answered {five:?} and `three` {three:?}; the state put at 3 is \
         not the latest, so the head still reads `five`"
    );
    Ok(())
}

fn several_puts_of_one_thing_in_one_transaction_leave_what_they_leave_one_by_one(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    let words: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "t", "language": "en",
        "fields": [{"name": "word", "type": {"kind": "text"}, "searchable": true}],
        "relations": [{"name": "next", "target": "t", "via": "next"}]
    }))
    .expect("a declaration in the check is well-formed");
    fold(engine, 1, |p| {
        p.put_declaration(&words, 1, None, Position(0))?;
        p.put_record(&record("t", "a", json!({"word": "one", "next": "b"}), 1))?;
        p.put_record(&record("t", "b", json!({"word": "zero"}), 1))
    })?;
    // `a`, which had a state, twice at later positions and once again at
    // the last; `b` at 5, then at 3, before the state it just put.
    fold(engine, 6, |p| {
        p.put_record(&record("t", "a", json!({"word": "four", "next": "b"}), 4))?;
        p.put_record(&record("t", "a", json!({"word": "ten", "next": "a"}), 6))?;
        p.put_record(&record("t", "a", json!({"word": "six"}), 6))?;
        p.put_record(&record("t", "b", json!({"word": "five"}), 5))?;
        p.put_record(&record("t", "b", json!({"word": "three"}), 3))
    })?;
    let ty = TypeName::from("t");
    for (key, states) in [
        ("a", ["one", "one", "one", "four", "four", "six"]),
        ("b", ["zero", "zero", "three", "three", "five", "five"]),
    ] {
        for (at, expected) in (1..).zip(states) {
            let got = view(engine, Some(Position(at)))?
                .get(&ty, &RecordKey::from(key))
                .must("reading the record")?
                .and_then(|r| r.fields.get("word").cloned());
            ensure!(
                got == Some(json!(expected)),
                "in one transaction `a` was put at 4, at 6 and at 6 again, and `b` at 5 and then \
                 at 3; a view at {at} read {got:?} for `{key}`, but the state valid there is \
                 {expected:?}"
            );
        }
    }
    let head = view(engine, None)?;
    let mut hits = BTreeMap::new();
    for word in ["one", "four", "ten", "six", "zero", "three", "five"] {
        let found: Vec<String> = head
            .lexical(&ty, word, None, 10)
            .must("searching")?
            .into_iter()
            .map(|(k, _)| k.to_string())
            .collect();
        hits.insert(word, found);
    }
    let expected: BTreeMap<&str, Vec<String>> = [
        ("one", vec![]),
        ("four", vec![]),
        ("ten", vec![]),
        ("six", vec!["a".to_owned()]),
        ("zero", vec![]),
        ("three", vec![]),
        ("five", vec!["b".to_owned()]),
    ]
    .into_iter()
    .collect();
    ensure!(
        hits == expected,
        "at the head, full-text search found {hits:?}; only the latest states are the head's, \
         `a` reading `six` and `b` reading `five`, so it must find {expected:?}"
    );
    let out = links(engine, "t", "a", None, Walk::Out)?;
    ensure!(
        out.is_empty(),
        "`a` linked to `b` at 1 and 4 and to itself in a put replaced at 6, and its latest state \
         links nowhere; its out edges at the head are {out:?}"
    );
    Ok(())
}

/// [`billing`], and a relation `approved_by` from invoices to customers
/// carried by link records: through position 1, i1 and i2 approved by
/// c1; at position 2, i2's link removed and a record i3 retired.
fn billing_with_links(engine: &dyn LakeEngine) -> Result<(), Failure> {
    billing(engine)?;
    let read = |v: Value| {
        serde_json::from_value::<Declaration>(v)
            .map_err(|e| Failure::new(format!("the check's declaration does not read: {e}")))
    };
    let invoice = read(json!({
        "declare": "type", "name": "invoice", "fields": [],
        "relations": [
            {"name": "billed_to", "target": "customer", "via": "customer"},
            {"name": "replaces", "target": "invoice", "via": "replaces"},
            {"name": "approved_by", "target": "customer"}
        ]
    }))?;
    let Declaration::Type(t) = &invoice else {
        unreachable!("declared as a type")
    };
    let lt = crate::links::link_types_of(t).remove(0);
    let link = |from: &str, to: &str, at: u64| {
        let key = crate::links::link_key(&from.into(), &to.into());
        record(
            lt.name.as_str(),
            key.as_str(),
            json!({"from": from, "to": to}),
            at,
        )
    };
    fold(engine, 1, |p| {
        p.put_declaration(&invoice, 2, None, Position(1))?;
        p.put_declaration(&Declaration::Type(lt.clone()), 1, None, Position(1))?;
        p.put_record(&record("invoice", "i3", json!({"customer": "c1"}), 1))?;
        p.put_record(&link("i1", "c1", 1))?;
        p.put_record(&link("i2", "c1", 1))
    })?;
    fold(engine, 2, |p| {
        let mut gone = link("i2", "c1", 2);
        gone.retired = true;
        gone.version = Version(2);
        p.put_record(&gone)?;
        let mut retired = record("invoice", "i3", json!({"customer": "c1"}), 2);
        retired.retired = true;
        retired.version = Version(2);
        p.put_record(&retired)
    })
}

fn edges_over_many_keys_answer_each_key_as_it_answers_alone(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    billing_with_links(engine)?;
    let sorted = |edges: Vec<lakelands_api::query::Edge>| {
        let mut out: Vec<String> = edges
            .into_iter()
            .map(|e| format!("{}>{}>{}", e.from.1, e.relation, e.to.1))
            .collect();
        out.sort();
        out
    };
    // Out of key order, with a key that names no record.
    let keys: [(&str, &[&str]); 2] = [
        ("invoice", &["i9", "i3", "i2", "i1"]),
        ("customer", &["c1", "c9"]),
    ];
    let mut shared = false;
    for at in [Some(Position(1)), None] {
        let v = view(engine, at)?;
        for (ty, ks) in keys {
            let ty = TypeName::from(ty);
            let ks: Vec<RecordKey> = ks.iter().map(|k| RecordKey::from(*k)).collect();
            for relation in [None, Some("replaces"), Some("approved_by")] {
                let relation = relation.map(RelationName::from);
                for walk in [Walk::Out, Walk::In, Walk::Both] {
                    let what = format!("the {walk:?} edges over {relation:?} at {at:?}");
                    let many = v
                        .edges(&ty, &ks, relation.as_ref(), walk)
                        .must(&format!("reading {what} of {ks:?}"))?;
                    ensure!(
                        many.len() == ks.len(),
                        "{what} of {} keys answered {} lists",
                        ks.len(),
                        many.len()
                    );
                    for (key, got) in ks.iter().zip(many) {
                        let alone = v
                            .edges(&ty, std::slice::from_ref(key), relation.as_ref(), walk)
                            .must(&format!("reading {what} of {key}"))?
                            .remove(0);
                        let (got, alone) = (sorted(got), sorted(alone));
                        ensure!(
                            got == alone,
                            "{what} of {ty} {key} among {ks:?} are {got:?}; alone they are \
                             {alone:?}"
                        );
                        shared |= walk == Walk::Both
                            && key.as_str() == "i2"
                            && got.contains(&"i2>replaces>i1".to_owned());
                    }
                }
            }
        }
    }
    ensure!(
        shared,
        "i2 replaces i1 and both were asked about; the link must be in i2's list as in i1's"
    );
    Ok(())
}

fn get_many_answers_what_get_answers_key_by_key(engine: &dyn LakeEngine) -> Result<(), Failure> {
    billing_with_links(engine)?;
    let mut restricted = record("invoice", "i1", json!({"customer": "c1"}), 3);
    restricted.restricted = true;
    restricted.version = Version(2);
    fold(engine, 3, |p| p.put_record(&restricted))?;
    // Out of key order, one key twice, one that names no record.
    let keys: Vec<RecordKey> = ["i3", "i9", "i2", "i1", "i2"]
        .into_iter()
        .map(RecordKey::from)
        .collect();
    let ty = TypeName::from("invoice");
    for at in [Some(Position(0)), Some(Position(2)), None] {
        let v = view(engine, at)?;
        let got = v
            .get_many(&ty, &keys)
            .must(&format!("reading {keys:?} at {at:?}"))?;
        let mut want = Vec::new();
        for key in ["i1", "i2", "i3", "i9"] {
            if let Some(r) = v
                .get(&ty, &RecordKey::from(key))
                .must(&format!("reading {key} at {at:?}"))?
            {
                want.push(r);
            }
        }
        ensure!(
            got == want,
            "get_many of {keys:?} at {at:?} answered {got:?}; key by key, in key order and each \
             once, get answers {want:?}"
        );
    }
    Ok(())
}

fn a_much_written_key_is_put_and_read_as_one_written_twice(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    // `b` is put twice and `a` forty thousand times.
    const STATES: u64 = 40_000;
    let put = |key: &str, at: u64| record("t", key, json!({"n": at}), at);
    fold_one(engine, 1, put("b", 1))?;
    fold_one(engine, 2, put("b", 2))?;
    for first in (3..3 + STATES).step_by(200) {
        fold(engine, first + 199, |p| {
            (first..first + 200).try_for_each(|at| p.put_record(&put("a", at)))
        })?;
    }
    let (ty, head, then) = (TypeName::from("t"), 2 + STATES, 1_000);
    let n = |r: Option<StoredRecord>| r.and_then(|r| r.fields.get("n").cloned());
    let mut answers = Vec::new();
    for as_of in [None, Some(then)] {
        let v = view(engine, as_of.map(Position))?;
        for key in ["a", "b"] {
            let read = v.get(&ty, &RecordKey::from(key));
            answers.push(n(read.must("reading one record")?));
        }
        let scan = lakelands_api::engine::Scan {
            limit: 10,
            ..Default::default()
        };
        let page = v.scan(&ty, &scan).must("reading a page")?;
        let page: Vec<Value> = page.into_iter().filter_map(|r| n(Some(r))).collect();
        answers.push(Some(json!([page, v.count(&ty, &scan).must("counting")?])));
    }
    let want = |a: u64| [Some(json!(a)), Some(json!(2)), Some(json!([[a, 2], 2]))];
    ensure!(
        answers == [want(head), want(then)].concat(),
        "`a` was put at every position from 3 to {head} and `b` at 1 and 2; a `get` of each, a \
         page and a count answered {answers:?} at the head and as of {then}"
    );
    Ok(())
}

fn a_head_only_name_keeps_one_state_for_each_key(engine: &dyn LakeEngine) -> Result<(), Failure> {
    let ty = TypeName::from(format!("{}v", lakelands_api::seam::ORDER_PREFIX));
    let entry =
        |key: &str, n: u64, at: u64| record(ty.as_str(), key, json!({"sys:entry": {"n": n}}), at);
    let ended =
        |key: &str, at: u64| StoredRecord::ended(ty.clone(), RecordKey::from(key), Position(at));
    fold(engine, 1, |p| {
        (0..100).try_for_each(|i| p.put_record(&entry(&format!("k{i:03}"), 1, 1)))
    })?;
    fold(engine, 2, |p| {
        p.put_record(&entry("k001", 2, 2))?;
        p.put_record(&ended("k002", 2))
    })?;
    let p = engine.projection();
    p.begin().must("beginning a transaction")?;
    p.end_head_only(&ty, Position(3)).must("ending the name")?;
    p.put_record(&entry("k001", 3, 3)).must("putting k001")?;
    p.put_record(&ended("k003", 3)).must("ending k003")?;
    p.rollback();
    let v = view(engine, None)?;
    let n = |r: Option<StoredRecord>| r.and_then(|r| r.fields.get("sys:entry")?.get("n")?.as_u64());
    let mut got = Vec::new();
    for key in ["k001", "k002", "k003"] {
        got.push(n(v
            .get(&ty, &RecordKey::from(key))
            .must("reading an entry")?));
    }
    let all = v.count(&ty, &lakelands_api::engine::Scan::default());
    let all = all.must("counting the entries")?;
    ensure!(
        (got.as_slice(), all) == ([Some(2), None, Some(1)].as_slice(), 99),
        "100 entries were put at 1, then k001 again and k002 ended at 2, and a rolled-back \
         transaction put k001 and ended k003: k001, k002 and k003 read {got:?} and the name \
         counts {all}, where one state for each key leaves [Some(2), None, Some(1)] and 99"
    );
    let after = lakelands_api::engine::After {
        values: Vec::new(),
        key: RecordKey::from("k001"),
    };
    let scan = lakelands_api::engine::Scan {
        after: Some(after),
        limit: 1,
        ..Default::default()
    };
    let before = crate::engines::rows_read();
    let page = v.scan(&ty, &scan).must("reading a page after k001")?;
    let read = crate::engines::rows_read() - before;
    let keys: Vec<&str> = page.iter().map(|r| r.key.as_str()).collect();
    ensure!(
        (keys.as_slice(), read) == (["k003"].as_slice(), 1),
        "a page of one after k001, with k002 ended, answered {keys:?} reading {read} rows: the \
         next key's one row, k003"
    );
    fold(engine, 4, |p| {
        p.put_record(&entry("k060", 4, 4))?;
        p.end_head_only(&ty, Position(4))?;
        p.put_record(&entry("k050", 4, 4))
    })?;
    let v = view(engine, None)?;
    let all = v.count(&ty, &lakelands_api::engine::Scan::default());
    let all = all.must("counting the entries")?;
    let page = v.scan(&ty, &scan).must("reading a page after k001")?;
    let keys: Vec<&str> = page.iter().map(|r| r.key.as_str()).collect();
    ensure!(
        (keys.as_slice(), all) == (["k050"].as_slice(), 1),
        "one transaction put k060, ended the name and put k050: a page after k001 answered \
         {keys:?} and the name counts {all}, where it holds k050 alone"
    );
    Ok(())
}

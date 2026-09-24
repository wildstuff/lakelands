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

use lakelands_api::engine::{LakeEngine, ProjectionStore, ProjectionView};
use lakelands_api::id::{Position, RecordKey, RelationName, TypeName, Version};
use lakelands_api::query::Walk;
use lakelands_api::record::StoredRecord;
use lakelands_api::write::Declaration;
use lakelands_api::{Error, Refusal};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::fixture::Must;

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

fn a_view_at_a_position_sees_the_states_then(engine: &dyn LakeEngine) -> Result<(), Failure> {
    for (n, at) in [(1, 1), (2, 2), (3, 4)] {
        fold(engine, at, |p| {
            p.put_record(&record("counter", "k", json!({"n": n}), at))
        })?;
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
        matches!(
            refused,
            Err(Error::Refused {
                reason: Refusal::OutOfRange { .. },
                ..
            })
        ),
        "a view at position 0 of a projection that has folded nothing answered {refused:?}; \
         it must be refused as out of range"
    );
    fold(engine, 2, |p| {
        p.put_record(&record("counter", "k", json!({}), 2))
    })?;
    view(engine, Some(Position(2)))?;
    let past = engine.view(Some(Position(3))).map(|v| v.position());
    ensure!(
        matches!(
            past,
            Err(Error::Refused {
                reason: Refusal::OutOfRange { head: Position(2) },
                ..
            })
        ),
        "a view at position 3 of a projection folded through 2 answered {past:?}; it must be \
         refused as out of range with head 2"
    );
    Ok(())
}

fn a_rolled_back_transaction_leaves_nothing(engine: &dyn LakeEngine) -> Result<(), Failure> {
    fold(engine, 0, |p| {
        p.put_record(&record("counter", "a", json!({}), 0))
    })?;
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
    fold(engine, 1, |p| {
        p.put_record(&record("counter", "c", json!({}), 1))
    })?;
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
            &RecordKey::from(key),
            relation.as_ref(),
            walk,
        )
        .must(&format!("reading the {walk:?} edges of {ty} {key}"))?
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
    fold(engine, 4, |p| p.put_record(&record("t", "a", json!({}), 4)))?;
    let p = engine.projection();
    let invalid = |r: &lakelands_api::Result<()>| {
        matches!(
            r,
            Err(Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            })
        )
    };
    p.begin().must("beginning a transaction")?;
    let backwards = p.commit(Position(3));
    ensure!(
        invalid(&backwards),
        "committing through 3 a projection folded through 4 answered {backwards:?}; it must be \
         refused as invalid"
    );
    p.begin().must("beginning a transaction")?;
    p.put_record(&record("t", "b", json!({}), 9))
        .must("putting a record")?;
    let late = p.commit(Position(5));
    ensure!(
        invalid(&late),
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
    fold(engine, 1, |p| {
        p.put_record(&record("t", "a", json!({"n": 1}), 1))
    })?;
    let p = engine.projection();
    p.begin().must("beginning a transaction")?;
    p.put_record(&record("t", "a", json!({"n": 2}), 2))
        .must("putting a record")?;
    p.put_record(&record("t", "b", json!({}), 2))
        .must("putting a record")?;
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
    ensure!(
        n == Some(json!(2)),
        "after the commit `a` reads n = {n:?}; it must be 2"
    );
    Ok(())
}

fn a_view_keeps_its_position_after_later_commits(engine: &dyn LakeEngine) -> Result<(), Failure> {
    fold(engine, 1, |p| {
        p.put_record(&record("t", "a", json!({"n": 1}), 1))
    })?;
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
    fold(engine, 1, |p| {
        p.put_record(&record("t", "a", json!({"n": 1}), 1))
    })?;
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
    fold(engine, 1, |p| p.put_record(&record("t", "a", json!({}), 1)))?;
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

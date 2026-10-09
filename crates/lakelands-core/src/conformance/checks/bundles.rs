//! A bundle reaches a lake whole
//! (`docs/decisions/028-authoring-is-strict-and-whole.md`).
//!
//! `apply` validates a set of declarations as one: a declaration may name
//! a type the set declares after it, and the set is appended in one
//! append. A set with a problem appends nothing, and its refusal names
//! every problem.

use lakelands_api::Position;
use lakelands_api::error::Case;
use lakelands_api::ontology::{FieldType, StorageKind};
use lakelands_api::write::{Declaration, Outcome, ProposedWrite};
use serde_json::{Value, json};

use crate::conformance::fixture::{Lake, Must, field, get_at, invalid_case, said_like, type_decl};
use crate::conformance::{Failure, Held, Store};

check!(
    BUNDLE_WHOLE = a_bundle_with_forward_references_is_applied_whole_or_not_at_all,
    "028: apply validates a set of declarations together, so a reference may point forward within it, and appends all of it in one append or none of it"
);

check!(
    SET_COMPUTES_ONCE = a_set_computes_its_records_again_once_at_its_last_declaration,
    "028, extended (lakelands#605): a set of declarations, applied or an accepted proposal, computes the records it changes once, as its last declaration leaves the lake: a read inside the set finds them as before it, a field reading `changed()` or `previous()` reads the whole set's change, and a type that adds a relation first does not end the set early"
);

/// A type whose one field refers to `target`.
fn referring(name: &str, target: &str) -> Declaration {
    let mut to = field("to", StorageKind::Reference);
    to.ty = FieldType {
        target: Some(target.into()),
        ..FieldType::kind(StorageKind::Reference)
    };
    Declaration::Type(type_decl(name, vec![to]))
}

async fn a_bundle_with_forward_references_is_applied_whole_or_not_at_all(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    let applied = cat
        .apply(
            own,
            vec![
                referring("invoice", "partner"),
                Declaration::Type(type_decl("partner", vec![field("name", StorageKind::Text)])),
            ],
        )
        .await
        .must("applying `invoice`, which refers forward to `partner`, with `partner`")?;
    let after = store.head().await.must("reading the head")?;
    ensure!(
        matches!(applied, Outcome::Applied { position, .. } if position == after)
            && after.0 == before.0 + 2,
        "applying two declarations answered {applied:?} and moved the head from {before} to \
         {after}; it must append both in one append and answer the last position"
    );
    let described = l.describe().await?;
    let names: Vec<String> = described.types.iter().map(|t| t.name.to_string()).collect();
    ensure!(
        names.iter().any(|n| n == "invoice") && names.iter().any(|n| n == "partner"),
        "after the bundle the lake describes {names:?}; it must hold `invoice` and `partner`"
    );

    let refused = cat
        .apply(
            own,
            vec![
                Declaration::Type(type_decl("fine", vec![field("n", StorageKind::Number)])),
                referring("orphan", "ghost"),
                referring("stray", "nowhere"),
            ],
        )
        .await;
    let unmoved = store.head().await.must("reading the head")?;
    // The set refused whole, each problem said at the declaration it is in.
    let at = |name: &str| json!({"at": [{"name": name}]});
    let named_all = match invalid_case(&refused) {
        Some(Case::SetRefused { count: 2, problems }) => ["orphan", "stray"]
            .iter()
            .all(|n| (problems.iter()).any(|p| p.case().is_some_and(|s| said_like(s, &at(n))))),
        _ => false,
    };
    ensure!(
        named_all && unmoved == after,
        "a bundle with two broken references answered {refused:?} and moved the head from \
         {after} to {unmoved}; it must be refused whole, naming both problems, appending nothing"
    );
    let described = l.describe().await?;
    ensure!(
        described.types.iter().all(|t| t.name.as_str() != "fine"),
        "the refused bundle's valid declaration `fine` reached the lake"
    );
    Ok(())
}

/// The function `name` as `expr` over the number `n`.
fn over_n(name: &str, expr: &str) -> Declaration {
    let f = json!({"declare": "function", "name": name, "backing": "expr", "expr": expr,
                   "inputs": [{"name": "n", "type": {"kind": "number"}}]});
    serde_json::from_value(f).expect("a function")
}

/// `doc`: `n`, `x` by `f`, `y` by `g`, `moved`, whether the last
/// computation changed `y`, and `was`, `y` before it; when `to`, with a
/// relation `to` another `doc` and `plus`, so that its declaration
/// computes the records again too.
fn doc(to: bool) -> Declaration {
    let by = |name: &str, f: &str| {
        json!({"name": name, "type": {"kind": "number"}, "authority": "derived",
               "function": f, "depends_on": ["n"]})
    };
    let moved = json!({"name": "moved", "type": {"kind": "boolean"}, "authority": "derived",
                       "derived": "changed(y)", "depends_on": ["y"]});
    let was = json!({"name": "was", "type": {"kind": "number"}, "authority": "derived",
                     "derived": "previous(y)", "depends_on": ["y"]});
    let mut d = json!({"declare": "type", "name": "doc", "fields": [
        {"name": "n", "type": {"kind": "number"}}, by("x", "f"), by("y", "g"), moved, was]});
    if to {
        d["relations"] = json!([{"name": "to", "target": "doc"}]);
        let plus = json!({"name": "plus", "type": {"kind": "number"}, "authority": "derived",
                          "derived": "n + 1", "depends_on": ["n"]});
        d["fields"].as_array_mut().expect("the fields").push(plus);
    }
    serde_json::from_value(d).expect("a type")
}

/// `doc` `d1`'s fields `x`, `y`, `moved` and `was`, then `x`'s position,
/// as of `at`.
async fn d1(l: &Lake, at: Option<Position>) -> Result<Value, Failure> {
    let r = l.cat.get(&l.own, &"doc".into(), get_at("d1", at)).await;
    let r = r.must("reading `doc` `d1`")?.data;
    let fields = ["x", "y", "moved", "was"].map(|f| r.fields.get(f).cloned().unwrap_or_default());
    Ok(json!([fields, r.meta.get("x").map(|m| m.position.0)]))
}

async fn a_set_computes_its_records_again_once_at_its_last_declaration(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let first = vec![over_n("f", "n * 2"), over_n("g", "n + 1"), doc(false)];
    let applied = cat.apply(own, first).await;
    applied.must("applying `f`, `g` and `doc`")?;
    let written = l.upsert("doc", "d1", json!({"n": 1})).await?;
    let w = written.position().0;
    let before = json!([[2, 2, true, null], w]);
    // Applied: `doc` adds a relation, so the system declares its link type
    // after it, then `g` and `f` follow. Computed at each declaration,
    // `f`'s would come last and leave `y` as `g`'s found it.
    let set = vec![doc(true), over_n("g", "n + 2"), over_n("f", "n * 3")];
    let applied = cat.apply(own, set).await;
    let last = applied.must("applying the set")?.position();
    let mut inside = Vec::new();
    for back in 1..=3 {
        inside.push(d1(&l, Some(Position(last.0 - back))).await?);
    }
    let (after, once) = (d1(&l, None).await?, json!([[3, 3, true, 2], last.0]));
    ensure!(
        inside.iter().all(|s| *s == before) && after == once,
        "a set of `doc` with a relation, `g` and `f` reads `d1` inside it as before it, \
         {before}, and after it as computed once at its last declaration, {once}; it reads \
         {inside:?} inside and {after} after"
    );
    // Accepted: a proposal of `g` and `f` is the same set.
    let set = vec![over_n("g", "n + 3"), over_n("f", "n * 4")];
    let proposed = cat.propose(
        own,
        ProposedWrite::Declare { declarations: set },
        "again".into(),
    );
    let Outcome::Proposed { proposal, .. } = proposed.await.must("proposing `g` and `f`")? else {
        return Err(Failure::new("a proposal answers `proposed`"));
    };
    cat.accept(own, &proposal).await.must("accepting")?;
    let last = store.head().await.must("reading the head")?;
    let inside = d1(&l, Some(Position(last.0 - 1))).await?;
    let (after, once) = (d1(&l, None).await?, json!([[4, 4, true, 3], last.0]));
    let was = json!([[3, 3, true, 2], inside[1]]);
    ensure!(
        inside == was && after == once,
        "an accepted proposal of `g` and `f` reads `d1` at `g` as before it, {was}, and after \
         it as computed once at `f`, {once}; it reads {inside} and {after}"
    );
    Ok(())
}

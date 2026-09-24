//! The contract's JSON forms, as a transport or a remote client sees
//! them. These are the shapes the doors put on the wire; a change here is
//! a change every client notices.

use lakelands_api::expr::{CmpOp, Condition, Expr, Literal};
use lakelands_api::query::{Pattern, Projection, Walk};
use lakelands_api::write::ProposedWrite;
use lakelands_api::{Error, Find, Outcome, Position, ReadAt, Refusal, Timestamp, Version};
use serde_json::json;

#[test]
fn find_takes_where_as_text_and_the_time_axes_flat() {
    let find: Find = serde_json::from_value(json!({
        "where": "status = 'open'",
        "order_by": [{"field": "due", "dir": "desc"}],
        "limit": 10,
        "as_of": 42,
        "valid_at": "2026-09-01",
        "with_meta": true,
    }))
    .unwrap();
    assert_eq!(find.filter, Some(Condition::Text("status = 'open'".into())));
    assert_eq!(
        find.at,
        ReadAt {
            as_of: Some(Position(42)),
            valid_at: Some(Timestamp::parse("2026-09-01").unwrap()),
        }
    );
    assert!(find.with_meta);
}

#[test]
fn a_condition_is_text_or_the_typed_form_and_both_round_trip() {
    let typed = Condition::Expr(Expr::Cmp {
        cmp: CmpOp::Gt,
        lhs: Box::new(Expr::Path {
            path: vec!["amount".into()],
        }),
        rhs: Box::new(Expr::Lit {
            value: Literal::Number("100".into()),
        }),
    });
    for c in [Condition::Text("amount > 100".into()), typed] {
        let back: Condition = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
}

#[test]
fn a_write_answers_applied_or_proposed_and_both_carry_a_position() {
    let applied = Outcome::Applied {
        position: Position(7),
        version: Some(Version(3)),
    };
    assert_eq!(
        serde_json::to_value(&applied).unwrap(),
        json!({"outcome": "applied", "position": 7, "version": 3})
    );
    let proposed: Outcome = serde_json::from_value(json!({
        "outcome": "proposed", "position": 8, "proposal": "p-1", "why": "risk high",
    }))
    .unwrap();
    assert_eq!(proposed.position(), Position(8));
}

#[test]
fn errors_are_a_closed_tagged_set_with_a_sentence_for_people() {
    let e = Error::Refused {
        field: Some("amount".into()),
        reason: Refusal::WrongDoor,
    };
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        json!({"error": "refused", "field": "amount", "reason": {"refusal": "wrong_door"}})
    );
    assert_eq!(
        e.to_string(),
        "refused on `amount`: another authority writes this field"
    );
}

#[test]
fn the_named_graph_reads_are_patterns() {
    let n = Pattern::neighbors("invoice".into(), "INV-1".into(), "debtor".into(), Walk::Out);
    assert_eq!(n.steps.len(), 1);
    let s = Pattern::subgraph("invoice".into(), "INV-1".into(), vec!["debtor".into()], 3);
    assert_eq!(s.project, Projection::Graph);
    let p = Pattern::path(
        ("invoice".into(), "INV-1".into()),
        ("partner".into(), "P-9".into()),
        vec!["debtor".into()],
        4,
    );
    assert_eq!(p.to, Some(("partner".into(), "P-9".into())));
}

#[test]
fn a_patch_is_sparse_and_null_clears() {
    let w: ProposedWrite = serde_json::from_value(json!({
        "write": "upsert", "type": "invoice", "key": "INV-1",
        "patch": {"note": null, "verdict": "disputed"},
    }))
    .unwrap();
    let ProposedWrite::Upsert { patch, .. } = w else {
        panic!("not an upsert")
    };
    assert_eq!(patch.len(), 2);
    assert!(patch["note"].is_null());
}

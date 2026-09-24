//! The engine through its seam: the opener's registry, the lease across
//! handles, transactions, and every read a view answers.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use lakelands_api::engine::CanonicalEvent;
use lakelands_api::engine::{After, EngineOpener, LakeEngine, OpenMode, ProjectionStore, Scan};
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::{Position, ProposalId, RecordKey, TypeName, Version};
use lakelands_api::location::Location;
use lakelands_api::query::Walk;
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::record::{FieldMeta, Origin, StoredRecord};
use lakelands_api::time::Timestamp;
use lakelands_api::write::{Declaration, Proposal, ProposalFilter, ProposalState, ProposedWrite};
use lakelands_api::{Error, Refusal};
use serde_json::{Value, json};

use super::*;

/// A lake name no other test uses.
fn unique(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("{prefix}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// A chain of `n` events from `start`, under `fence`, following `prev`.
pub(crate) fn chain(start: u64, n: u64, fence: u64, prev: Digest) -> Vec<CanonicalEvent> {
    let mut prev = prev;
    (start..start + n)
        .map(|p| {
            let hash = Digest(format!("{p:064x}"));
            let e = CanonicalEvent {
                position: Position(p),
                id: EventId(format!("{p:032x}")),
                prev: prev.clone(),
                hash: hash.clone(),
                // Position 0 is written before any lease exists.
                fence: if p == 0 { 0 } else { fence },
                bytes: format!("{{\"p\":{p}}}").into_bytes(),
            };
            prev = hash;
            e
        })
        .collect()
}

fn open(name: &str, mode: OpenMode) -> Result<Box<dyn LakeEngine>> {
    MemoryOpener.open(&Location::memory(name), mode)
}

fn fresh() -> Box<dyn LakeEngine> {
    open(&unique("lake"), OpenMode::Create).unwrap()
}

fn decl(v: Value) -> Declaration {
    serde_json::from_value(v).unwrap()
}

fn record(ty: &str, key: &str, fields: Value, position: u64) -> StoredRecord {
    StoredRecord {
        ty: ty.into(),
        key: key.into(),
        version: Version(1),
        fields: serde_json::from_value(fields).unwrap(),
        meta: BTreeMap::new(),
        position: Position(position),
        retired: false,
        restricted: false,
    }
}

/// Folds `puts` in one transaction through `through`.
fn fold(e: &dyn LakeEngine, through: u64, puts: impl FnOnce(&dyn ProjectionStore)) {
    let p = e.projection();
    p.begin().unwrap();
    puts(p);
    p.commit(Position(through)).unwrap();
}

fn keys(records: &[StoredRecord]) -> Vec<&str> {
    records.iter().map(|r| r.key.as_str()).collect()
}

fn scan(limit: u32) -> Scan {
    Scan {
        limit,
        ..Scan::default()
    }
}

// ---- the opener ----------------------------------------------------------

#[test]
fn the_opener_answers_to_memory() {
    assert_eq!(MemoryOpener.name(), "memory");
    assert_eq!(fresh().name(), "memory");
}

#[test]
fn a_second_open_of_a_lake_sees_the_same_log_and_lease() {
    let name = unique("shared");
    let writer = open(&name, OpenMode::Create).unwrap();
    let lease = writer.take_lease("the writer").unwrap();
    let e = chain(0, 1, lease.fence, Digest::zero());
    writer.append(&lease, &e).unwrap();

    let reader = open(&name, OpenMode::Read).unwrap();
    assert_eq!(reader.read(Position(0), 10).unwrap(), e);
    let second = open(&name, OpenMode::Write).unwrap();
    let Err(Error::Unavailable { message }) = second.take_lease("another writer") else {
        panic!("a second writer took a held lease");
    };
    assert!(message.contains("the writer"), "{message}");
}

#[test]
fn a_lake_that_was_never_created_is_unavailable_to_read_and_write() {
    let name = unique("missing");
    for mode in [OpenMode::Read, OpenMode::Write] {
        assert!(matches!(open(&name, mode), Err(Error::Unavailable { .. })));
    }
    assert!(open(&name, OpenMode::Create).is_ok());
    assert!(open(&name, OpenMode::Write).is_ok());
}

#[test]
fn a_location_for_another_engine_is_refused() {
    let at = Location::sqlite("elsewhere", "/nowhere");
    assert!(matches!(
        MemoryOpener.open(&at, OpenMode::Create),
        Err(Error::Refused { .. })
    ));
}

#[test]
fn a_read_only_handle_refuses_the_lease_append_and_seal() {
    let name = unique("ro");
    let writer = open(&name, OpenMode::Create).unwrap();
    let lease = writer.take_lease("w").unwrap();
    let reader = open(&name, OpenMode::Read).unwrap();
    assert!(matches!(
        reader.take_lease("r"),
        Err(Error::Unavailable { .. })
    ));
    let e = chain(0, 1, lease.fence, Digest::zero());
    assert!(matches!(
        reader.append(&lease, &e),
        Err(Error::Unavailable { .. })
    ));
    assert!(matches!(
        reader.seal_segment(&lease),
        Err(Error::Unavailable { .. })
    ));
    assert!(reader.tip().unwrap().is_none());
}

#[test]
fn a_released_lease_can_be_taken_again_with_a_greater_fence() {
    let e = fresh();
    let a = e.take_lease("a").unwrap();
    e.release_lease(&a).unwrap();
    let b = e.take_lease("b").unwrap();
    assert!(b.fence > a.fence);
}

// ---- transactions --------------------------------------------------------

#[test]
fn puts_are_seen_only_after_commit_and_never_after_rollback() {
    let e = fresh();
    let p = e.projection();
    assert!(
        p.put_record(&record("t", "a", json!({}), 0)).is_err(),
        "no transaction"
    );
    p.begin().unwrap();
    p.put_record(&record("t", "a", json!({}), 0)).unwrap();
    assert_eq!(p.folded_through().unwrap(), None);
    assert_eq!(e.view(None).unwrap().position(), None);
    p.rollback();
    assert!(p.commit(Position(0)).is_err(), "nothing to commit");

    fold(e.as_ref(), 0, |p| {
        p.put_record(&record("t", "b", json!({}), 0)).unwrap()
    });
    let v = e.view(None).unwrap();
    assert_eq!(v.position(), Some(Position(0)));
    assert!(v.get(&"t".into(), &"a".into()).unwrap().is_none());
    assert!(v.get(&"t".into(), &"b".into()).unwrap().is_some());
}

#[test]
fn clear_drops_the_projection_but_keeps_the_log_and_the_lease() {
    let e = fresh();
    let lease = e.take_lease("w").unwrap();
    let events = chain(0, 1, lease.fence, Digest::zero());
    e.append(&lease, &events).unwrap();
    fold(e.as_ref(), 0, |p| {
        p.put_record(&record("t", "a", json!({}), 0)).unwrap()
    });
    e.vectors()
        .unwrap()
        .put(&"t".into(), &"a".into(), "v", &[1.0])
        .unwrap();

    e.projection().clear().unwrap();
    assert_eq!(e.projection().folded_through().unwrap(), None);
    assert!(
        e.view(None)
            .unwrap()
            .get(&"t".into(), &"a".into())
            .unwrap()
            .is_none()
    );
    assert_eq!(e.vectors().unwrap().count(&"t".into(), "v").unwrap(), 0);
    assert_eq!(e.read(Position(0), 10).unwrap(), events);
    assert!(matches!(e.take_lease("x"), Err(Error::Unavailable { .. })));
}

// ---- views at positions --------------------------------------------------

#[test]
fn a_view_sees_the_states_valid_at_its_position() {
    let e = fresh();
    fold(e.as_ref(), 2, |p| {
        p.put_record(&record("t", "a", json!({"n": 1}), 1)).unwrap();
        p.put_record(&record("t", "a", json!({"n": 2}), 2)).unwrap();
    });
    fold(e.as_ref(), 5, |p| {
        p.put_record(&record("t", "a", json!({"n": 3}), 5)).unwrap()
    });
    let n = |at: Option<u64>| {
        e.view(at.map(Position))
            .unwrap()
            .get(&"t".into(), &"a".into())
            .unwrap()
            .map(|r| r.fields["n"].clone())
    };
    assert_eq!(n(Some(0)), None);
    assert_eq!(n(Some(1)), Some(json!(1)));
    assert_eq!(n(Some(2)), Some(json!(2)));
    assert_eq!(n(Some(4)), Some(json!(2)));
    assert_eq!(n(Some(5)), Some(json!(3)));
    assert_eq!(n(None), Some(json!(3)));
}

#[test]
fn a_view_past_the_folded_position_is_refused_naming_the_head() {
    let e = fresh();
    assert!(matches!(
        e.view(Some(Position(0))),
        Err(Error::Refused {
            reason: Refusal::OutOfRange { .. },
            ..
        })
    ));
    fold(e.as_ref(), 3, |_| {});
    assert!(e.view(Some(Position(3))).is_ok());
    let Err(Error::Refused {
        reason: Refusal::OutOfRange { head },
        ..
    }) = e.view(Some(Position(4)))
    else {
        panic!("a view past the fold was served");
    };
    assert_eq!(head, Position(3));
}

#[test]
fn a_view_keeps_its_position_while_the_fold_moves_on() {
    let e = fresh();
    fold(e.as_ref(), 1, |p| {
        p.put_record(&record("t", "a", json!({"n": 1}), 1)).unwrap()
    });
    let v = e.view(None).unwrap();
    fold(e.as_ref(), 2, |p| {
        p.put_record(&record("t", "a", json!({"n": 2}), 2)).unwrap()
    });
    let got = v.get(&"t".into(), &"a".into()).unwrap().unwrap();
    assert_eq!(got.fields["n"], json!(1));
}

#[test]
fn declarations_memberships_proposals_outputs_and_applied_ids_keep_history() {
    let e = fresh();
    let ty = decl(json!({"declare": "type", "name": "t", "fields": []}));
    let set = TypeName::from("t");
    let proposal = |state| Proposal {
        id: ProposalId::from("p1"),
        position: Position(1),
        proposer: "human:a".into(),
        proposed_at: Timestamp(0),
        write: ProposedWrite::Retire {
            ty: "t".into(),
            key: "a".into(),
            opts: Default::default(),
        },
        why: "review".into(),
        state,
        decided_by: None,
        reason: None,
    };
    fold(e.as_ref(), 1, |p| {
        p.put_declaration(&ty, 1, Some("h1"), Position(0)).unwrap();
        p.put_membership("big", &set, &"a".into(), true, Position(1))
            .unwrap();
        p.put_proposal(&proposal(ProposalState::Pending), Position(1))
            .unwrap();
        p.mark_applied(&lakelands_api::event::EventId("e1".into()), Position(1))
            .unwrap();
    });
    fold(e.as_ref(), 3, |p| {
        p.put_declaration(&ty, 2, Some("h2"), Position(3)).unwrap();
        p.put_membership("big", &set, &"a".into(), false, Position(3))
            .unwrap();
        p.put_proposal(&proposal(ProposalState::Rejected), Position(3))
            .unwrap();
        p.put_output("fn", &Digest::zero(), &json!(42), Position(3))
            .unwrap();
    });
    let id = lakelands_api::event::EventId("e1".into());

    let then = e.view(Some(Position(1))).unwrap();
    let d = then.declarations().unwrap();
    assert_eq!((d.len(), d[0].version, d[0].since), (1, 1, Position(0)));
    assert_eq!(d[0].schema_hash.as_deref(), Some("h1"));
    assert!(then.is_member("big", &set, &"a".into()).unwrap());
    let ps = then.proposals(&ProposalFilter::default()).unwrap();
    assert_eq!(ps[0].state, ProposalState::Pending);
    assert_eq!(then.output("fn", &Digest::zero()).unwrap(), None);
    assert_eq!(then.applied(&id).unwrap(), Some(Position(1)));
    assert_eq!(
        e.view(Some(Position(0))).unwrap().applied(&id).unwrap(),
        None
    );

    let now = e.view(None).unwrap();
    assert_eq!(now.declarations().unwrap()[0].version, 2);
    assert!(!now.is_member("big", &set, &"a".into()).unwrap());
    assert!(!now.is_member("small", &set, &"a".into()).unwrap());
    assert_eq!(
        now.proposals(&ProposalFilter::default()).unwrap()[0].state,
        ProposalState::Rejected
    );
    assert_eq!(now.output("fn", &Digest::zero()).unwrap(), Some(json!(42)));
}

// ---- scans ---------------------------------------------------------------

fn numbered(e: &dyn LakeEngine) {
    fold(e, 0, |p| {
        p.put_record(&record("t", "a", json!({"n": 3, "s": "x"}), 0))
            .unwrap();
        p.put_record(&record("t", "b", json!({"n": "10"}), 0))
            .unwrap();
        p.put_record(&record("t", "c", json!({"s": "y"}), 0))
            .unwrap();
        p.put_record(&record("t", "d", json!({"n": 3}), 0)).unwrap();
        p.put_record(&record("t", "e", json!({"n": 9}), 0)).unwrap();
        let mut gone = record("t", "r", json!({"n": 1}), 0);
        gone.retired = true;
        p.put_record(&gone).unwrap();
        let mut held = record("t", "h", json!({"n": 2}), 0);
        held.restricted = true;
        p.put_record(&held).unwrap();
        p.put_record(&record("u", "a", json!({"n": 0}), 0)).unwrap();
    });
}

fn by_n(dir: Direction) -> Vec<Order> {
    vec![Order {
        field: "n".into(),
        by: OrderBy::Value,
        dir,
    }]
}

#[test]
fn a_scan_orders_numbers_numerically_with_missing_values_last_both_ways() {
    let e = fresh();
    numbered(e.as_ref());
    let v = e.view(None).unwrap();
    let asc = v
        .scan(
            &"t".into(),
            &Scan {
                order_by: by_n(Direction::Asc),
                ..scan(100)
            },
        )
        .unwrap();
    assert_eq!(keys(&asc), ["a", "d", "e", "b", "c"]);
    let desc = v
        .scan(
            &"t".into(),
            &Scan {
                order_by: by_n(Direction::Desc),
                ..scan(100)
            },
        )
        .unwrap();
    assert_eq!(
        keys(&desc),
        ["b", "e", "a", "d", "c"],
        "ties stay in key order"
    );
    assert_eq!(
        keys(&v.scan(&"t".into(), &scan(100)).unwrap()),
        ["a", "b", "c", "d", "e"]
    );
}

#[test]
fn a_scan_includes_retired_and_restricted_records_only_when_asked() {
    let e = fresh();
    numbered(e.as_ref());
    let v = e.view(None).unwrap();
    let t = TypeName::from("t");
    let with = |retired, restricted| {
        let s = Scan {
            retired,
            restricted,
            ..scan(100)
        };
        keys(&v.scan(&t, &s).unwrap()).join("")
    };
    assert_eq!(with(false, false), "abcde");
    assert_eq!(with(true, false), "abcder");
    assert_eq!(with(false, true), "abcdeh");
    assert!(
        v.get(&t, &"r".into()).unwrap().unwrap().retired,
        "get sees it"
    );
}

#[test]
fn a_scan_filters_exactly_and_an_unevaluable_filter_matches_nothing() {
    let e = fresh();
    numbered(e.as_ref());
    let v = e.view(None).unwrap();
    let filter = |text: &str| Scan {
        filter: Some(lakelands_core::expr::parse(text).unwrap()),
        ..scan(100)
    };
    let t = TypeName::from("t");
    assert_eq!(keys(&v.scan(&t, &filter("n > 5")).unwrap()), ["b", "e"]);
    assert_eq!(keys(&v.scan(&t, &filter("n is null")).unwrap()), ["c"]);
    assert!(v.scan(&t, &filter("n / 0 > 1")).unwrap().is_empty());
}

#[test]
fn a_scan_continues_after_a_keyset_position_and_stops_at_its_limit() {
    let e = fresh();
    numbered(e.as_ref());
    let v = e.view(None).unwrap();
    let t = TypeName::from("t");
    let order_by = by_n(Direction::Asc);
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = v
            .scan(
                &t,
                &Scan {
                    order_by: order_by.clone(),
                    after: after.clone(),
                    ..scan(2)
                },
            )
            .unwrap();
        let Some(last) = page.last() else { break };
        after = Some(After {
            values: vec![last.fields.get("n").cloned().unwrap_or(Value::Null)],
            key: last.key.clone(),
        });
        seen.extend(page.iter().map(|r| r.key.to_string()));
    }
    assert_eq!(seen, ["a", "d", "e", "b", "c"]);
    assert!(v.scan(&t, &scan(0)).unwrap().is_empty());
}

#[test]
fn a_scan_orders_by_when_a_field_was_learned() {
    let e = fresh();
    let meta = |t: i64| FieldMeta {
        origin: Origin::Authored,
        known_at: Timestamp(t),
        seen_at: None,
        position: Position(0),
        writer: "human:a".into(),
        source: None,
        procedure_version: None,
    };
    fold(e.as_ref(), 0, |p| {
        for (key, t) in [("a", 300), ("b", 100), ("c", 200)] {
            let mut r = record("t", key, json!({"n": 1}), 0);
            r.meta.insert("n".into(), meta(t));
            p.put_record(&r).unwrap();
        }
    });
    let order_by = vec![Order {
        field: "n".into(),
        by: OrderBy::KnownAt,
        dir: Direction::Asc,
    }];
    let got = e
        .view(None)
        .unwrap()
        .scan(
            &"t".into(),
            &Scan {
                order_by,
                ..scan(10)
            },
        )
        .unwrap();
    assert_eq!(keys(&got), ["b", "c", "a"]);
}

#[test]
fn a_scan_at_a_valid_instant_uses_the_types_valid_time() {
    let e = fresh();
    let ty = decl(json!({
        "declare": "type", "name": "price", "fields": [],
        "valid_from": "from", "valid_to": "to"
    }));
    fold(e.as_ref(), 0, |p| {
        p.put_declaration(&ty, 1, None, Position(0)).unwrap();
        p.put_record(&record(
            "price",
            "old",
            json!({"from": "2026-01-01", "to": "2026-06-01"}),
            0,
        ))
        .unwrap();
        p.put_record(&record("price", "new", json!({"from": "2026-06-01"}), 0))
            .unwrap();
        p.put_record(&record("price", "undated", json!({}), 0))
            .unwrap();
    });
    let v = e.view(None).unwrap();
    let at = |s: &str| {
        let s = Scan {
            valid_at: Some(Timestamp::parse(s).unwrap()),
            ..scan(10)
        };
        keys(&v.scan(&"price".into(), &s).unwrap()).join(",")
    };
    assert_eq!(at("2026-03-01"), "old");
    assert_eq!(at("2026-06-01"), "new");
    assert_eq!(keys(&v.scan(&"price".into(), &scan(10)).unwrap()).len(), 3);
}

// ---- edges ---------------------------------------------------------------

fn graph(e: &dyn LakeEngine) {
    let invoice = decl(json!({
        "declare": "type", "name": "invoice", "fields": [],
        "relations": [
            {"name": "billed_to", "target": "customer", "via": "customer"},
            {"name": "replaces", "target": "invoice", "via": "replaces"},
            {"name": "mentions", "target": "customer"}
        ]
    }));
    let customer = decl(json!({"declare": "type", "name": "customer", "fields": []}));
    fold(e, 0, |p| {
        p.put_declaration(&invoice, 1, None, Position(0)).unwrap();
        p.put_declaration(&customer, 1, None, Position(0)).unwrap();
        p.put_record(&record("customer", "c1", json!({}), 0))
            .unwrap();
        p.put_record(&record(
            "invoice",
            "i1",
            json!({"customer": "c1", "replaces": "i1"}),
            0,
        ))
        .unwrap();
        p.put_record(&record(
            "invoice",
            "i2",
            json!({"customer": "c1", "replaces": "i1"}),
            0,
        ))
        .unwrap();
        p.put_record(&record(
            "invoice",
            "i3",
            json!({"customer": "gone", "replaces": 7}),
            0,
        ))
        .unwrap();
    });
}

fn links(e: &dyn LakeEngine, ty: &str, key: &str, rel: Option<&str>, walk: Walk) -> Vec<String> {
    let rel = rel.map(lakelands_api::id::RelationName::from);
    let mut out: Vec<String> = e
        .view(None)
        .unwrap()
        .edges(&ty.into(), &key.into(), rel.as_ref(), walk)
        .unwrap()
        .into_iter()
        .map(|e| format!("{}>{}>{}", e.from.1, e.relation, e.to.1))
        .collect();
    out.sort();
    out
}

#[test]
fn edges_out_follow_via_fields_and_skip_relations_without_one() {
    let e = fresh();
    graph(e.as_ref());
    assert_eq!(
        links(e.as_ref(), "invoice", "i2", None, Walk::Out),
        ["i2>billed_to>c1", "i2>replaces>i1"]
    );
    assert_eq!(
        links(e.as_ref(), "invoice", "i2", Some("billed_to"), Walk::Out),
        ["i2>billed_to>c1"]
    );
    assert_eq!(
        links(e.as_ref(), "invoice", "i3", None, Walk::Out),
        ["i3>billed_to>gone"],
        "a missing target is returned as it is; a non-text value is no link"
    );
}

#[test]
fn edges_in_find_the_records_pointing_here() {
    let e = fresh();
    graph(e.as_ref());
    assert_eq!(
        links(e.as_ref(), "customer", "c1", None, Walk::In),
        ["i1>billed_to>c1", "i2>billed_to>c1"]
    );
    assert_eq!(
        links(e.as_ref(), "customer", "gone", None, Walk::In),
        ["i3>billed_to>gone"]
    );
}

#[test]
fn edges_both_list_each_link_once_a_self_link_included() {
    let e = fresh();
    graph(e.as_ref());
    assert_eq!(
        links(e.as_ref(), "invoice", "i1", None, Walk::Both),
        ["i1>billed_to>c1", "i1>replaces>i1", "i2>replaces>i1"]
    );
}

#[test]
fn edges_to_a_retired_record_are_returned() {
    let e = fresh();
    graph(e.as_ref());
    let mut gone = record("customer", "c1", json!({}), 1);
    gone.retired = true;
    fold(e.as_ref(), 1, |p| p.put_record(&gone).unwrap());
    assert_eq!(
        links(e.as_ref(), "invoice", "i1", Some("billed_to"), Walk::Out),
        ["i1>billed_to>c1"]
    );
    assert_eq!(links(e.as_ref(), "customer", "c1", None, Walk::In).len(), 2);
}

// ---- full text -----------------------------------------------------------

#[test]
fn lexical_search_scores_matched_tokens_over_searchable_text() {
    let e = fresh();
    let doc = decl(json!({
        "declare": "type", "name": "doc", "language": "en",
        "fields": [
            {"name": "title", "type": {"kind": "text"}, "searchable": true},
            {"name": "secret", "type": {"kind": "text"}}
        ]
    }));
    fold(e.as_ref(), 0, |p| {
        p.put_declaration(&doc, 1, None, Position(0)).unwrap();
        p.put_record(&record("doc", "a", json!({"title": "Running invoices"}), 0))
            .unwrap();
        p.put_record(&record("doc", "b", json!({"title": "an invoice"}), 0))
            .unwrap();
        p.put_record(&record(
            "doc",
            "c",
            json!({"title": "cats", "secret": "invoice"}),
            0,
        ))
        .unwrap();
        p.put_record(&record("doc", "d", json!({"title": "runs"}), 0))
            .unwrap();
        let mut gone = record("doc", "r", json!({"title": "invoice run"}), 0);
        gone.retired = true;
        p.put_record(&gone).unwrap();
        let mut held = record("doc", "h", json!({"title": "invoice run"}), 0);
        held.restricted = true;
        p.put_record(&held).unwrap();
    });
    let v = e.view(None).unwrap();
    let hits = v.lexical(&"doc".into(), "invoice runs", 10).unwrap();
    assert_eq!(
        hits,
        vec![
            ("a".into(), 2.0),
            ("h".into(), 2.0),
            ("b".into(), 1.0),
            ("d".into(), 1.0)
        ],
        "a restricted record is a candidate; a retired one is not"
    );
    assert_eq!(
        v.lexical(&"doc".into(), "invoice runs", 1).unwrap().len(),
        1
    );
    assert!(v.lexical(&"nope".into(), "invoice", 10).unwrap().is_empty());
}

// ---- proposals -----------------------------------------------------------

#[test]
fn proposals_filter_by_state_and_type_newest_first() {
    let e = fresh();
    let verb = decl(json!({
        "declare": "verb", "name": "pay", "on": "invoice", "risk": "high",
        "effect": "write", "set": []
    }));
    let proposal = |id: &str, position: u64, write: ProposedWrite, state| Proposal {
        id: id.into(),
        position: Position(position),
        proposer: "human:a".into(),
        proposed_at: Timestamp(0),
        write,
        why: "review".into(),
        state,
        decided_by: None,
        reason: None,
    };
    let retire = |ty: &str| ProposedWrite::Retire {
        ty: ty.into(),
        key: "k".into(),
        opts: Default::default(),
    };
    let call = ProposedWrite::Call {
        verb: "pay".into(),
        key: "k".into(),
        payload: Default::default(),
        opts: Default::default(),
    };
    fold(e.as_ref(), 3, |p| {
        p.put_declaration(&verb, 1, None, Position(0)).unwrap();
        p.put_proposal(
            &proposal("p1", 1, retire("invoice"), ProposalState::Pending),
            Position(1),
        )
        .unwrap();
        p.put_proposal(
            &proposal("p2", 2, retire("customer"), ProposalState::Pending),
            Position(2),
        )
        .unwrap();
        p.put_proposal(
            &proposal("p3", 3, call, ProposalState::Accepted),
            Position(3),
        )
        .unwrap();
    });
    let v = e.view(None).unwrap();
    let ids = |f: ProposalFilter| -> Vec<String> {
        v.proposals(&f)
            .unwrap()
            .into_iter()
            .map(|p| p.id.to_string())
            .collect()
    };
    assert_eq!(ids(ProposalFilter::default()), ["p3", "p2", "p1"]);
    assert_eq!(
        ids(ProposalFilter {
            state: Some(ProposalState::Pending),
            ..Default::default()
        }),
        ["p2", "p1"]
    );
    assert_eq!(
        ids(ProposalFilter {
            ty: Some("invoice".into()),
            ..Default::default()
        }),
        ["p3", "p1"]
    );
    assert_eq!(
        ids(ProposalFilter {
            limit: Some(1),
            ..Default::default()
        }),
        ["p3"]
    );
}

// ---- vectors -------------------------------------------------------------

#[test]
fn the_vector_index_is_shared_by_every_handle_to_the_lake() {
    let name = unique("vectors");
    let a = open(&name, OpenMode::Create).unwrap();
    let b = open(&name, OpenMode::Read).unwrap();
    let ty = TypeName::from("doc");
    let key = RecordKey::from("x");
    a.vectors()
        .unwrap()
        .put(&ty, &key, "emb", &[1.0, 0.0])
        .unwrap();
    let hits = b
        .vectors()
        .unwrap()
        .nearest(&ty, "emb", &[1.0, 0.0], 5)
        .unwrap();
    assert_eq!(hits, vec![(key.clone(), 1.0)]);
    a.vectors().unwrap().remove(&ty, &key, "emb").unwrap();
    assert_eq!(b.vectors().unwrap().count(&ty, "emb").unwrap(), 0);
}

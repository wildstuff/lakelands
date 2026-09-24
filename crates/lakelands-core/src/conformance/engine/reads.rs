//! What a view answers through the seam: scans, full text, the vector
//! index, proposals and the other kept states (`docs/seams.md`
//! §`ProjectionView`, §`VectorIndex`,
//! `docs/decisions/029-every-answer-has-one-order.md`).
//!
//! The store sorts and pages itself in this release, but a scan's order,
//! continuation and limit are contract all the same, so an engine that
//! later takes paging over cannot change an answer. Full text ranks by the
//! head and keeps what the view's position could see. The vector index is
//! exact: the true top `k` by cosine, ties by key.

use lakelands_api::engine::{After, LakeEngine, Scan};
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::{Declaration, Proposal, ProposalFilter, ProposalState, ProposedWrite};
use lakelands_api::{Error, Refusal, Timestamp};
use serde_json::{Value, json};

use super::projection::{fold, record, view};
use crate::conformance::Failure;
use crate::conformance::fixture::Must;

engine_check!(
    /// Mixed kinds sort the same on every engine.
    MIXED_KINDS = mixed_kinds_sort_the_same_on_every_engine,
    "029: a scan orders numbers, then instants, then text, then booleans, then JSON, a missing value last in either direction, ties by key, so every engine puts the same records in the same places"
);

engine_check!(
    /// A scan continues strictly after its keyset position.
    AFTER_AND_LIMIT = a_scan_continues_strictly_after_its_keyset_position,
    "seams §scan: a scan answers only what sorts strictly after `after`, and at most `limit` of it, 0 answering nothing, so an engine that pages natively gives the store's pages"
);

engine_check!(
    /// A scan's filter selects exactly what the evaluator selects.
    FILTER_AS_EVALUATOR = scan_filters_exactly_as_the_evaluator_including_mixed_kinds_and_errors,
    "seams §scan: an engine selects exactly the records the one evaluator selects, comparisons across kinds included, and a filter that errors for a record does not select it"
);

engine_check!(
    /// A scan honours the flags and `valid_at`.
    FLAGS_AND_VALID_AT = scan_honours_retired_restricted_and_valid_at,
    "seams §scan: retired and restricted records come only when asked for, and a valid-timed type's records only when valid at `valid_at`"
);

engine_check!(
    /// Lexical hits come from the pipeline over searchable, live records.
    LEXICAL_HITS = lexical_hits_are_the_pipelines_over_searchable_live_records,
    "025 and seams §lexical: a hit holds a token the one pipeline makes of the query in a searchable field of a live record, restricted ones included (the store applies grants); the hit set is the same on every engine, ties by key, at most k"
);

engine_check!(
    /// Lexical search finds tokens outside ASCII.
    LEXICAL_BEYOND_ASCII = lexical_finds_every_token_the_pipeline_makes_beyond_ascii,
    "025: every token the one pipeline makes is a term an engine finds, in any script, combining marks included, so no engine's own tokenizer narrows the hit set"
);

engine_check!(
    /// Lexical search at an earlier position ranks by the head.
    LEXICAL_EARLIER = lexical_at_an_earlier_position_ranks_by_the_head_and_keeps_what_was_visible_then,
    "seams §lexical: a view answers from the full-text index at the head and keeps only the records live at its own position"
);

engine_check!(
    /// The vector index ranks by cosine, ties by key.
    NEAREST_BY_COSINE = nearest_ranks_by_cosine_with_ties_by_key,
    "seams §VectorIndex: nearest answers the exact top k by the cosine of the angle, best first, equal scores by key"
);

engine_check!(
    /// A zero vector scores 0; a wrong length is invalid.
    ZERO_AND_WRONG_LENGTH = a_zero_vector_scores_zero_and_a_wrong_length_is_invalid,
    "seams §VectorIndex: a vector of zeros scores 0 against everything, and a query whose length differs from a stored vector's is refused as invalid"
);

engine_check!(
    /// `put` replaces a vector; `remove` forgets it.
    PUT_AND_REMOVE = put_replaces_a_vector_and_remove_forgets_it,
    "seams §VectorIndex: put replaces a record's vector for the field, remove forgets it, forgetting one that is absent does nothing, and count counts"
);

engine_check!(
    /// Edges over a relation carried by link records.
    LINK_EDGES = edges_over_a_link_relation_come_from_the_live_link_records_at_the_views_position,
    "035 and seams §edges: an engine answers a relation without via from its live link records at the view's position, out and in, each edge carrying the link's own fields"
);

engine_check!(
    /// Kept states read at an earlier position.
    EARLIER_STATES = declarations_membership_outputs_and_applied_ids_read_at_an_earlier_position,
    "seams §ProjectionView: declarations, membership, recorded outputs and applied ids are kept states too, read at a view's own position"
);

engine_check!(
    /// A later output replaces an earlier one.
    LATER_OUTPUT_HOLDS = a_later_output_for_the_same_call_replaces_the_earlier_from_its_position,
    "022, extended, and seams §ProjectionView: the log may hold two outputs for one seam and input (a failure, then the answer a redeclaration asked for); the later one holds from its position, the earlier one before it"
);

engine_check!(
    /// The first applied position is kept.
    FIRST_APPLIED = the_first_applied_position_is_kept,
    "seams §ProjectionStore: marking an id applied twice keeps the first position, so a redelivery is recognised where the event first took effect"
);

engine_check!(
    /// Proposals filter by state and type, newest first.
    PROPOSALS = proposals_filter_by_state_and_type_newest_first,
    "seams §ProjectionView: proposals match the filter's state and type, a call's type being its verb's `on`, newest first by their latest state, then by id"
);

fn keys(records: &[StoredRecord]) -> Vec<String> {
    records.iter().map(|r| r.key.to_string()).collect()
}

fn scan_all() -> Scan {
    Scan {
        limit: u32::MAX,
        ..Scan::default()
    }
}

fn by(field: &str, dir: Direction) -> Order {
    Order {
        field: field.into(),
        by: OrderBy::Value,
        dir,
    }
}

fn declare(json: Value) -> Declaration {
    serde_json::from_value(json).expect("a declaration in the check is well-formed")
}

fn mixed_kinds_sort_the_same_on_every_engine(engine: &dyn LakeEngine) -> Result<(), Failure> {
    let values = [
        ("a", json!("abc")),
        ("b", json!(true)),
        ("c", json!("10")),
        ("d", json!({"x": 1})),
        ("e", json!("2026-01-01T00:00:00Z")),
        ("f", json!(9)),
        ("g", json!(false)),
        ("h", Value::Null),
        ("i", json!(9.0)),
    ];
    fold(engine, 1, |p| {
        for (key, v) in &values {
            let fields = if v.is_null() {
                json!({})
            } else {
                json!({"v": v})
            };
            p.put_record(&record("t", key, fields, 1))?;
        }
        Ok(())
    })?;
    let at = view(engine, None)?;
    let ty = TypeName::from("t");
    for (dir, expected) in [
        (
            Direction::Asc,
            ["f", "i", "c", "e", "a", "g", "b", "d", "h"],
        ),
        (
            Direction::Desc,
            ["d", "b", "g", "a", "e", "c", "f", "i", "h"],
        ),
    ] {
        let got = keys(
            &at.scan(
                &ty,
                &Scan {
                    order_by: vec![by("v", dir)],
                    ..scan_all()
                },
            )
            .must("scanning in order")?,
        );
        ensure!(
            got == expected,
            "ordered by `v` {dir:?}, the scan answered {got:?}; the one sort order puts the \
             numbers first (9 and 9.0 equal, so by key), then instants, text, booleans (false \
             first) and JSON, and the record without `v` last: {expected:?}"
        );
    }
    Ok(())
}

fn a_scan_continues_strictly_after_its_keyset_position(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    fold(engine, 1, |p| {
        for (key, n) in [("a", 1), ("b", 2), ("c", 2), ("d", 3), ("e", 4)] {
            p.put_record(&record("t", key, json!({"n": n}), 1))?;
        }
        Ok(())
    })?;
    let at = view(engine, None)?;
    let ty = TypeName::from("t");
    let page = |after: Option<After>, limit: u32| {
        at.scan(
            &ty,
            &Scan {
                order_by: vec![by("n", Direction::Asc)],
                after,
                limit,
                ..Scan::default()
            },
        )
        .map(|r| keys(&r))
    };
    let first = page(None, 2).must("scanning the first page")?;
    ensure!(
        first == ["a", "b"],
        "the first page of two answered {first:?}; it must be [a, b]"
    );
    let next = page(
        Some(After {
            values: vec![json!(2)],
            key: "b".into(),
        }),
        2,
    )
    .must("scanning after (2, b)")?;
    ensure!(
        next == ["c", "d"],
        "after (n 2, key b) the scan answered {next:?}; strictly after that place come c (n 2, \
         a greater key) and d"
    );
    let none = page(None, 0).must("scanning with limit 0")?;
    ensure!(
        none.is_empty(),
        "a scan with limit 0 answered {none:?}; 0 answers nothing"
    );
    let past = page(
        Some(After {
            values: vec![json!(4)],
            key: "e".into(),
        }),
        10,
    )
    .must("scanning after the last row")?;
    ensure!(
        past.is_empty(),
        "a scan after the last row answered {past:?}"
    );
    Ok(())
}

fn scan_filters_exactly_as_the_evaluator_including_mixed_kinds_and_errors(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    fold(engine, 1, |p| {
        for (key, v) in [
            ("a", json!(10)),
            ("b", json!("10")),
            ("c", json!("10.0")),
            ("d", json!("ten")),
            ("e", json!(true)),
            ("f", json!(9)),
            ("g", json!({"n": 10})),
        ] {
            p.put_record(&record("t", key, json!({"v": v}), 1))?;
        }
        p.put_record(&record("t", "h", json!({}), 1))
    })?;
    let at = view(engine, None)?;
    let ty = TypeName::from("t");
    for (filter, expected) in [
        ("v = 10", vec!["a", "b", "c"]),
        ("v = '10'", vec!["a", "b", "c"]),
        ("v != 10", vec!["f"]),
        ("v > 9", vec!["a", "b", "c"]),
        ("v = 'ten'", vec!["d"]),
        ("v = true", vec!["e"]),
        ("v is null", vec!["h"]),
        ("v + 1 > 10", vec!["a", "b", "c"]),
    ] {
        let expr = crate::expr::parse(filter).expect("the check's filters parse");
        let got = keys(
            &at.scan(
                &ty,
                &Scan {
                    filter: Some(expr.clone()),
                    ..scan_all()
                },
            )
            .must(&format!("scanning with `{filter}`"))?,
        );
        let evaluator: Vec<String> = at
            .scan(&ty, &scan_all())
            .must("scanning everything")?
            .into_iter()
            .filter(|r| {
                crate::expr::holds(
                    &expr,
                    &crate::expr::RecordScope {
                        fields: &r.fields,
                        settings: None,
                        now: Timestamp(0),
                    },
                )
                .unwrap_or(false)
            })
            .map(|r| r.key.to_string())
            .collect();
        ensure!(
            got == expected && got == evaluator,
            "`{filter}` selected {got:?}; the evaluator selects {evaluator:?} and the check \
             expects {expected:?}"
        );
    }
    Ok(())
}

fn scan_honours_retired_restricted_and_valid_at(engine: &dyn LakeEngine) -> Result<(), Failure> {
    let timed = declare(json!({
        "declare": "type", "name": "t", "valid_from": "from", "valid_to": "to",
        "fields": [
            {"name": "from", "type": {"kind": "date_time"}},
            {"name": "to", "type": {"kind": "date_time"}}
        ]
    }));
    fold(engine, 1, |p| {
        p.put_declaration(&timed, 1, None, Position(0))?;
        let mut retired = record("t", "retired", json!({"from": "2026-01-01T00:00:00Z"}), 1);
        retired.retired = true;
        let mut held = record("t", "held", json!({"from": "2026-01-01T00:00:00Z"}), 1);
        held.restricted = true;
        p.put_record(&retired)?;
        p.put_record(&held)?;
        p.put_record(&record(
            "t",
            "ended",
            json!({"from": "2026-01-01T00:00:00Z", "to": "2026-06-01T00:00:00Z"}),
            1,
        ))?;
        p.put_record(&record(
            "t",
            "open",
            json!({"from": "2026-03-01T00:00:00Z"}),
            1,
        ))?;
        p.put_record(&record("t", "undated", json!({}), 1))
    })?;
    let at = view(engine, None)?;
    let ty = TypeName::from("t");
    let run = |scan: Scan| at.scan(&ty, &scan).map(|r| keys(&r));
    let live = run(scan_all()).must("scanning live records")?;
    ensure!(
        live == ["ended", "open", "undated"],
        "a scan without flags answered {live:?}; retired and restricted records come only when \
         asked for"
    );
    let all = run(Scan {
        retired: true,
        restricted: true,
        ..scan_all()
    })
    .must("scanning with both flags")?;
    ensure!(
        all.len() == 5,
        "a scan with both flags answered {all:?}; it must answer all five records"
    );
    let july = Timestamp::parse("2026-07-01T00:00:00Z").expect("an instant");
    let valid = run(Scan {
        valid_at: Some(july),
        ..scan_all()
    })
    .must("scanning at a valid time")?;
    ensure!(
        valid == ["open"],
        "a scan valid at 2026-07-01 answered {valid:?}; `ended` stopped in June and `undated` \
         cannot say when it began, so only `open` holds"
    );
    Ok(())
}

/// A searchable type: an English `body`, a searchable `title`, and an
/// unsearchable `note`.
fn documents() -> Declaration {
    declare(json!({
        "declare": "type", "name": "doc", "language": "en",
        "fields": [
            {"name": "title", "type": {"kind": "text"}, "searchable": true},
            {"name": "body", "type": {"kind": "text"}, "searchable": true},
            {"name": "note", "type": {"kind": "text"}}
        ]
    }))
}

fn hit_keys(hits: &[(RecordKey, f64)]) -> Vec<String> {
    hits.iter().map(|(k, _)| k.to_string()).collect()
}

fn lexical_hits_are_the_pipelines_over_searchable_live_records(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    fold(engine, 1, |p| {
        p.put_declaration(&documents(), 1, None, Position(0))?;
        p.put_record(&record(
            "doc",
            "two",
            json!({"title": "Running", "body": "the rivers"}),
            1,
        ))?;
        p.put_record(&record("doc", "one-b", json!({"body": "runs"}), 1))?;
        p.put_record(&record("doc", "one-a", json!({"body": "A RIVER"}), 1))?;
        p.put_record(&record("doc", "note", json!({"note": "run river"}), 1))?;
        let mut retired = record("doc", "retired", json!({"body": "run"}), 1);
        retired.retired = true;
        let mut held = record("doc", "held", json!({"body": "run"}), 1);
        held.restricted = true;
        p.put_record(&retired)?;
        p.put_record(&held)
    })?;
    let at = view(engine, None)?;
    let ty = TypeName::from("doc");
    let hits = at.lexical(&ty, "run rivers", 10).must("searching")?;
    ensure!(
        hit_keys(&hits) == ["two", "held", "one-a", "one-b"],
        "searching `run rivers` answered {:?}; the English pipeline makes `run` and `river` of \
         it, `two` holds both, `held` (restricted), `one-a` and `one-b` one each (ties by key), \
         `note` holds them only in an unsearchable field, and a retired record is no hit",
        hit_keys(&hits)
    );
    ensure!(
        hits[0].1 > hits[1].1 && hits[1].1 == hits[2].1 && hits[2].1 == hits[3].1,
        "the scores {hits:?} must rank the better match higher and tie the equal ones"
    );
    let one = at.lexical(&ty, "run rivers", 1).must("searching for one")?;
    ensure!(
        hit_keys(&one) == ["two"],
        "searching with k = 1 answered {:?}",
        hit_keys(&one)
    );
    let none = at.lexical(&ty, "ocean", 10).must("searching for nothing")?;
    ensure!(none.is_empty(), "searching `ocean` answered {none:?}");
    Ok(())
}

fn lexical_at_an_earlier_position_ranks_by_the_head_and_keeps_what_was_visible_then(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    fold(engine, 1, |p| {
        p.put_declaration(&documents(), 1, None, Position(0))?;
        p.put_record(&record("doc", "kept", json!({"body": "ocean"}), 1))?;
        p.put_record(&record("doc", "later-retired", json!({"body": "ocean"}), 1))
    })?;
    fold(engine, 2, |p| {
        p.put_record(&record("doc", "kept", json!({"body": "river"}), 2))?;
        let mut gone = record("doc", "later-retired", json!({"body": "river"}), 2);
        gone.retired = true;
        p.put_record(&gone)?;
        p.put_record(&record("doc", "new", json!({"body": "river"}), 2))
    })?;
    let ty = TypeName::from("doc");
    let then = view(engine, Some(Position(1)))?;
    let river = then.lexical(&ty, "river", 10).must("searching at 1")?;
    ensure!(
        hit_keys(&river) == ["kept", "later-retired"],
        "at position 1, `river` answered {:?}; the head's text holds `river` in `kept`, \
         `later-retired` and `new`, and at 1 only the first two were live",
        hit_keys(&river)
    );
    let ocean = then.lexical(&ty, "ocean", 10).must("searching at 1")?;
    ensure!(
        ocean.is_empty(),
        "at position 1, `ocean` answered {:?}; ranking is by the text at the head, which no \
         longer holds `ocean`",
        hit_keys(&ocean)
    );
    let now = view(engine, None)?;
    let head = now
        .lexical(&ty, "river", 10)
        .must("searching at the head")?;
    ensure!(
        hit_keys(&head) == ["kept", "new"],
        "at the head, `river` answered {:?}; `later-retired` is retired there",
        hit_keys(&head)
    );
    Ok(())
}

fn vectors(engine: &dyn LakeEngine) -> Result<&dyn lakelands_api::engine::VectorIndex, Failure> {
    engine
        .vectors()
        .ok_or_else(|| Failure::new("the engine keeps no vector index"))
}

fn nearest_ranks_by_cosine_with_ties_by_key(engine: &dyn LakeEngine) -> Result<(), Failure> {
    let index = vectors(engine)?;
    let ty = TypeName::from("doc");
    for (key, v) in [
        ("east", [1.0, 0.0]),
        ("north", [0.0, 1.0]),
        ("north-east", [1.0, 1.0]),
        ("also-east", [3.0, 0.0]),
        ("west", [-1.0, 0.0]),
    ] {
        index
            .put(&ty, &key.into(), "emb", &v)
            .must("putting a vector")?;
    }
    let hits = index
        .nearest(&ty, "emb", &[2.0, 0.0], 4)
        .must("searching the index")?;
    ensure!(
        hit_keys(&hits) == ["also-east", "east", "north-east", "north"],
        "the nearest four to (2, 0) came as {:?}; by cosine the two eastward vectors tie at 1 \
         (by key), then north-east, then north",
        hit_keys(&hits)
    );
    let expected = [1.0, 1.0, std::f64::consts::FRAC_1_SQRT_2, 0.0];
    ensure!(
        hits.iter()
            .zip(expected)
            .all(|((_, s), e)| (s - e).abs() < 1e-9),
        "the scores {hits:?} are not the cosines {expected:?}"
    );
    let none = index
        .nearest(&ty, "other", &[1.0, 0.0], 4)
        .must("searching an empty field")?;
    ensure!(
        none.is_empty(),
        "a field holding no vectors answered {none:?}"
    );
    Ok(())
}

fn a_zero_vector_scores_zero_and_a_wrong_length_is_invalid(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    let index = vectors(engine)?;
    let ty = TypeName::from("doc");
    index
        .put(&ty, &"a".into(), "emb", &[1.0, 2.0])
        .must("putting a vector")?;
    index
        .put(&ty, &"zero".into(), "emb", &[0.0, 0.0])
        .must("putting a zero vector")?;
    let hits = index
        .nearest(&ty, "emb", &[0.0, 0.0], 5)
        .must("searching with a zero vector")?;
    ensure!(
        hits.len() == 2 && hits.iter().all(|(_, s)| *s == 0.0),
        "a zero query answered {hits:?}; it scores 0 against everything"
    );
    let wrong = index.nearest(&ty, "emb", &[1.0, 0.0, 0.0], 5);
    ensure!(
        matches!(
            wrong,
            Err(Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            })
        ),
        "a query of three dimensions against vectors of two answered {wrong:?}; it must be \
         refused as invalid"
    );
    Ok(())
}

fn put_replaces_a_vector_and_remove_forgets_it(engine: &dyn LakeEngine) -> Result<(), Failure> {
    let index = vectors(engine)?;
    let ty = TypeName::from("doc");
    index
        .put(&ty, &"a".into(), "emb", &[1.0, 0.0])
        .must("putting a vector")?;
    index
        .put(&ty, &"b".into(), "emb", &[0.0, 1.0])
        .must("putting a vector")?;
    index
        .put(&ty, &"a".into(), "emb", &[0.0, 2.0])
        .must("replacing a vector")?;
    let count = index.count(&ty, "emb").must("counting")?;
    ensure!(
        count == 2,
        "after a replacing put the field counts {count}; it holds two"
    );
    let best = index
        .nearest(&ty, "emb", &[0.0, 1.0], 2)
        .must("searching")?;
    ensure!(
        best.iter().all(|(_, s)| (s - 1.0).abs() < 1e-9),
        "after `a` was replaced by (0, 2) both vectors point north, but the index answered \
         {best:?}"
    );
    index
        .remove(&ty, &"b".into(), "emb")
        .must("removing a vector")?;
    index
        .remove(&ty, &"absent".into(), "emb")
        .must("removing an absent vector")?;
    let count = index.count(&ty, "emb").must("counting")?;
    ensure!(
        count == 1,
        "after removing `b` the field counts {count}; it holds one"
    );
    Ok(())
}

fn declarations_membership_outputs_and_applied_ids_read_at_an_earlier_position(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    let v1 = declare(json!({"declare": "type", "name": "t", "fields": []}));
    let v2 = declare(json!({
        "declare": "type", "name": "t", "fields": [{"name": "n", "type": {"kind": "number"}}]
    }));
    let key = RecordKey::from("k");
    let ty = TypeName::from("t");
    let seen = EventId("seen".into());
    fold(engine, 1, |p| {
        p.put_declaration(&v1, 1, None, Position(1))?;
        p.put_membership("big", &ty, &key, true, Position(1))
    })?;
    fold(engine, 3, |p| {
        p.put_declaration(&v2, 2, None, Position(3))?;
        p.put_membership("big", &ty, &key, false, Position(3))?;
        p.put_output("fn", &Digest::zero(), &json!(42), Position(3))?;
        p.mark_applied(&seen, Position(3))
    })?;
    let then = view(engine, Some(Position(2)))?;
    let now = view(engine, None)?;
    let version = |v: &dyn lakelands_api::engine::ProjectionView| {
        v.declarations().map(|d| {
            d.iter()
                .find(|d| matches!(&d.declaration, Declaration::Type(t) if t.name == ty))
                .map(|d| (d.version, d.since))
        })
    };
    let (a, b) = (
        version(then.as_ref()).must("reading declarations at 2")?,
        version(now.as_ref()).must("reading declarations at the head")?,
    );
    ensure!(
        a == Some((1, Position(1))) && b == Some((2, Position(3))),
        "the type was declared at 1 and again at 3; at 2 the view answered {a:?}, at the head \
         {b:?}"
    );
    let (was, is) = (
        then.is_member("big", &ty, &key)
            .must("reading membership at 2")?,
        now.is_member("big", &ty, &key)
            .must("reading membership at the head")?,
    );
    ensure!(
        was && !is,
        "the record joined `big` at 1 and left at 3; at 2 it is a member ({was}), at the head \
         not ({is})"
    );
    let never = now
        .is_member("other", &ty, &key)
        .must("reading an unknown set")?;
    ensure!(!never, "a set never put answered membership");
    let (out_then, out_now) = (
        then.output("fn", &Digest::zero())
            .must("reading an output at 2")?,
        now.output("fn", &Digest::zero())
            .must("reading an output at the head")?,
    );
    ensure!(
        out_then.is_none() && out_now == Some(json!(42)),
        "an output recorded at 3 read {out_then:?} at 2 and {out_now:?} at the head"
    );
    let (id_then, id_now) = (
        then.applied(&seen).must("reading an applied id at 2")?,
        now.applied(&seen)
            .must("reading an applied id at the head")?,
    );
    ensure!(
        id_then.is_none() && id_now == Some(Position(3)),
        "an id applied at 3 read {id_then:?} at 2 and {id_now:?} at the head"
    );
    Ok(())
}

fn the_first_applied_position_is_kept(engine: &dyn LakeEngine) -> Result<(), Failure> {
    let id = EventId("once".into());
    fold(engine, 2, |p| p.mark_applied(&id, Position(2)))?;
    fold(engine, 5, |p| p.mark_applied(&id, Position(5)))?;
    let at = view(engine, None)?
        .applied(&id)
        .must("reading an applied id")?;
    ensure!(
        at == Some(Position(2)),
        "an id marked applied at 2 and again at 5 reads as applied at {at:?}; the first \
         position is kept"
    );
    Ok(())
}

fn proposal(id: &str, position: u64, write: ProposedWrite, state: ProposalState) -> Proposal {
    Proposal {
        id: id.into(),
        position: Position(position),
        proposer: "human:a".into(),
        proposed_at: Timestamp(0),
        write,
        why: "review".into(),
        state,
        decided_by: None,
        reason: None,
    }
}

fn proposals_filter_by_state_and_type_newest_first(engine: &dyn LakeEngine) -> Result<(), Failure> {
    let verb = declare(json!({
        "declare": "verb", "name": "pay", "on": "invoice", "risk": "high",
        "effect": "write", "set": []
    }));
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
    fold(engine, 3, |p| {
        p.put_declaration(&verb, 1, None, Position(0))?;
        p.put_proposal(
            &proposal("p1", 1, retire("invoice"), ProposalState::Pending),
            Position(1),
        )?;
        p.put_proposal(
            &proposal("p2", 2, retire("customer"), ProposalState::Pending),
            Position(2),
        )?;
        p.put_proposal(
            &proposal("p3", 3, call.clone(), ProposalState::Pending),
            Position(3),
        )
    })?;
    fold(engine, 4, |p| {
        p.put_proposal(
            &proposal("p3", 4, call, ProposalState::Accepted),
            Position(4),
        )
    })?;
    let at = view(engine, None)?;
    let ids = |f: ProposalFilter| {
        at.proposals(&f)
            .map(|ps| ps.into_iter().map(|p| p.id.to_string()).collect::<Vec<_>>())
    };
    let all = ids(ProposalFilter::default()).must("listing proposals")?;
    ensure!(
        all == ["p3", "p2", "p1"],
        "all proposals came as {all:?}; newest first"
    );
    let pending = ids(ProposalFilter {
        state: Some(ProposalState::Pending),
        ..Default::default()
    })
    .must("listing pending proposals")?;
    ensure!(
        pending == ["p2", "p1"],
        "pending proposals came as {pending:?}; p3 was accepted at 4"
    );
    let invoices = ids(ProposalFilter {
        ty: Some("invoice".into()),
        ..Default::default()
    })
    .must("listing proposals of a type")?;
    ensure!(
        invoices == ["p3", "p1"],
        "proposals touching `invoice` came as {invoices:?}; a call's type is its verb's `on`"
    );
    let one = ids(ProposalFilter {
        limit: Some(1),
        ..Default::default()
    })
    .must("listing one proposal")?;
    ensure!(one == ["p3"], "a limit of one answered {one:?}");
    let before = view(engine, Some(Position(3)))?
        .proposals(&ProposalFilter {
            state: Some(ProposalState::Pending),
            ..Default::default()
        })
        .must("listing proposals at 3")?;
    ensure!(
        before.len() == 3,
        "at position 3 all three proposals were pending; the view answered {}",
        before.len()
    );
    Ok(())
}

fn lexical_finds_every_token_the_pipeline_makes_beyond_ascii(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    let plain = declare(json!({
        "declare": "type", "name": "note",
        "fields": [{"name": "body", "type": {"kind": "text"}, "searchable": true}]
    }));
    fold(engine, 1, |p| {
        p.put_declaration(&plain, 1, None, Position(0))?;
        p.put_record(&record(
            "note",
            "greek",
            json!({"body": "Ωμέγα ★★★ café"}),
            1,
        ))?;
        p.put_record(&record(
            "note",
            "cyrillic",
            json!({"body": "Привет, мир"}),
            1,
        ))?;
        p.put_record(&record(
            "note",
            "devanagari",
            json!({"body": "नमस्ते दुनिया"}),
            1,
        ))?;
        p.put_record(&record("note", "ascii", json!({"body": "hello world"}), 1))
    })?;
    let at = view(engine, None)?;
    let ty = TypeName::from("note");
    for (query, expected) in [
        ("ωμέγα", vec!["greek"]),
        ("CAFÉ", vec!["greek"]),
        ("мир", vec!["cyrillic"]),
        ("नमस्ते", vec!["devanagari"]),
        ("★★★", vec![]),
        ("hello мир ωμέγα", vec!["ascii", "cyrillic", "greek"]),
    ] {
        let hits = at
            .lexical(&ty, query, 10)
            .must(&format!("searching `{query}`"))?;
        ensure!(
            hit_keys(&hits) == expected,
            "searching `{query}` answered {:?}; the pipeline's tokens are {:?}, and every record \
             holding one of them is a hit: {expected:?}",
            hit_keys(&hits),
            crate::engines::tokens(query, "")
        );
    }
    Ok(())
}

fn a_later_output_for_the_same_call_replaces_the_earlier_from_its_position(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    let failed = json!({"error": "timed out"});
    fold(engine, 2, |p| {
        p.put_output("embed:m", &Digest::zero(), &failed, Position(2))
    })?;
    fold(engine, 4, |p| {
        p.put_output("embed:m", &Digest::zero(), &json!([1.0, 0.0]), Position(4))
    })?;
    let at = |position: Option<Position>| -> Result<Option<serde_json::Value>, Failure> {
        view(engine, position)?
            .output("embed:m", &Digest::zero())
            .must("reading an output")
    };
    let (before, between, after) = (at(Some(Position(1)))?, at(Some(Position(3)))?, at(None)?);
    ensure!(
        before.is_none() && between == Some(failed.clone()) && after == Some(json!([1.0, 0.0])),
        "a failure recorded at 2 and an answer for the same call at 4: at 1 the view must \
         answer nothing ({before:?}), at 3 the failure ({between:?}), at the head the answer \
         ({after:?})"
    );
    Ok(())
}

fn edges_over_a_link_relation_come_from_the_live_link_records_at_the_views_position(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    use lakelands_api::query::Walk;
    let partner = declare(json!({"declare": "type", "name": "partner", "fields": []}));
    let invoice = declare(json!({
        "declare": "type", "name": "invoice", "fields": [],
        "relations": [{"name": "approved_by", "target": "partner",
                       "fields": [{"name": "since", "type": {"kind": "date_time"}}]}]
    }));
    let Declaration::Type(t) = &invoice else {
        unreachable!("declared as a type")
    };
    let lt = crate::links::link_types_of(t).remove(0);
    let link = |from: &str, to: &str, since: &str, at: u64| {
        let key = crate::links::link_key(&from.into(), &to.into());
        record(
            lt.name.as_str(),
            key.as_str(),
            json!({"from": from, "to": to, "since": since}),
            at,
        )
    };
    fold(engine, 1, |p| {
        p.put_declaration(&partner, 1, None, Position(0))?;
        p.put_declaration(&invoice, 1, None, Position(1))?;
        p.put_declaration(&Declaration::Type(lt.clone()), 1, None, Position(1))
    })?;
    fold(engine, 2, |p| {
        p.put_record(&link("i1", "p1", "2026-09-01T00:00:00.000000Z", 2))
    })?;
    fold(engine, 3, |p| {
        p.put_record(&link("i2", "p1", "2026-09-02T00:00:00.000000Z", 3))
    })?;
    fold(engine, 4, |p| {
        let mut gone = link("i1", "p1", "2026-09-01T00:00:00.000000Z", 4);
        gone.retired = true;
        gone.version = lakelands_api::Version(2);
        p.put_record(&gone)
    })?;
    let edges = |at: Option<Position>, ty: &str, key: &str, walk: Walk| {
        view(engine, at)?
            .edges(&TypeName::from(ty), &RecordKey::from(key), None, walk)
            .must("reading edges")
            .map(|e| {
                e.into_iter()
                    .map(|e| {
                        format!(
                            "{}>{}@{}",
                            e.from.1,
                            e.to.1,
                            e.fields.get("since").and_then(Value::as_str).unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
            })
    };
    let then = edges(Some(Position(3)), "invoice", "i1", Walk::Out)?;
    ensure!(
        then == ["i1>p1@2026-09-01T00:00:00.000000Z"],
        "at 3 the link i1 → p1 is live: out of i1 the engine answers it with its `since`; \
         it answered {then:?}"
    );
    let now = edges(None, "invoice", "i1", Walk::Out)?;
    ensure!(
        now.is_empty(),
        "at the head the link i1 → p1 is retired: out of i1 the engine answers nothing; it \
         answered {now:?}"
    );
    let into = edges(None, "partner", "p1", Walk::In)?;
    ensure!(
        into == ["i2>p1@2026-09-02T00:00:00.000000Z"],
        "at the head only i2's link points into p1; the engine answered {into:?}"
    );
    let both = edges(Some(Position(3)), "partner", "p1", Walk::In)?;
    ensure!(
        both.len() == 2,
        "at 3 both links point into p1; the engine answered {both:?}"
    );
    Ok(())
}

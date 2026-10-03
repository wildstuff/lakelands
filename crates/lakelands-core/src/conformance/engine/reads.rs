//! What a view answers through the seam: scans, full text, the vector
//! index, proposals and the other kept states (`docs/seams.md`
//! §`ProjectionView`, §`VectorIndex`,
//! `docs/decisions/029-every-answer-has-one-order.md`).
//!
//! The store asks the engine for its pages: a scan's order, continuation
//! and limit are what `find` answers from. A scan ordered by the key alone
//! is read in batches from `after` (`lakelands_core::engines::
//! scan_in_key_order`), and answers as one scan over every state would. Full text ranks by the
//! head and keeps what the view's position could see. The vector index is
//! exact: the true top `k` by cosine, ties by key.

use lakelands_api::engine::{After, LakeEngine, Scan};
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::{Declaration, Proposal, ProposalFilter, ProposalState, ProposedWrite};
use lakelands_api::{Refusal, Timestamp};
use serde_json::{Value, json};

use super::projection::{fold, fold_one, record, view};
use crate::conformance::Failure;
use crate::conformance::fixture::{Must, order, reason};

engine_check!(
    /// A count is its scan's length.
    COUNT_IS_THE_SCAN = a_count_is_the_number_of_records_its_scan_selects,
    "seams §ProjectionView and 029: `count` answers the number of records `scan` selects, its flags, `valid_at`, `filter` and `equal` applied and its order, keyset position and limit ignored, at the head and at an earlier position, across more than one batch, a superseded state and a retired record not counted"
);

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
    /// A key-ordered scan answers across its batches as one scan.
    KEY_BATCHES = a_key_ordered_scan_answers_across_its_batches_as_one_scan,
    "seams §scan and lakelands#134: a scan ordered by the key alone, read in batches from `after`, answers what one scan over every state answers, whatever its limit, its `after`, its flags and its filter, at the head and at an earlier position, however many rows each batch passes over"
);

engine_check!(
    /// A field-ordered scan walks page by page as one scan.
    FIELD_ORDER_WALK = a_field_ordered_scan_walks_page_by_page_as_one_scan,
    "seams §scan and lakelands#137: a scan ordered by fields, walked page by page from `after`, answers what one scan over every state answers, whatever its directions, its filter and its flags, after later puts, a redeclaration and a change of the `sort_index.<type>` setting, at the head and at an earlier position"
);

engine_check!(
    /// A page ordered by an indexed field reads its page.
    SORTED_PAGE = a_page_ordered_by_an_indexed_field_reads_its_page_and_the_setting_switches_it,
    "041 and lakelands#137: a page of 10 ordered by a field the `sort_index.<type>` setting names reads its page and at most three rows more, not the type, the first page and one continued after a value longer than the index keeps alike; the setting's write switches the same page from sorting the type to the index and back, and the answer stays what one scan over every state answers"
);

engine_check!(
    /// A scan's `equal` selects identical values only.
    EQUAL_IS_IDENTITY = a_scan_with_equal_answers_exactly_the_identical_values,
    "044 and seams §scan: a scan's `equal` selects the records whose field holds exactly the JSON value, never the expression's `=` (`'010'` is not `'10'`, text is not a number, an instant in another written form is not the same), never a null, at the head and at an earlier position alike"
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
    /// Lexical search limited to named fields matches in those only.
    LEXICAL_NAMED_FIELDS = lexical_over_named_fields_is_the_pipelines_over_those_fields,
    "025 and seams §lexical: a search limited to named fields matches, scores and ties over the head text of the named searchable fields only, and no engine cuts its candidates at k before it narrows them: a record matching in a named field is a hit however many others match only in fields not named"
);

engine_check!(
    /// A record put after its type is declared again in the same
    /// transaction is indexed under the new declaration.
    LEXICAL_REDECLARED_MID_FOLD = a_record_put_after_a_redeclaration_in_the_same_transaction_is_indexed_under_it,
    "025 and seams §ProjectionStore: a put reads the declarations as the transaction holds them, so a type declared again inside one fold's transaction governs every record put after it, and the records it already held"
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
    /// A record's edges read what they reach at the head.
    EDGES_READ_WHAT_THEY_REACH = a_records_edges_at_the_head_read_its_links_not_the_type,
    "seams §edges and lakelands#190: at the head, the edges into or out of a record over a relation carried by link records, and the edges into it over one carried by a reference field, are found without reading the link type or the declaring type: the records an engine hands the rulebook to find them are about the edges it returns"
);

engine_check!(
    /// Kept states read at an earlier position.
    EARLIER_STATES = declarations_membership_outputs_and_applied_ids_read_at_an_earlier_position,
    "seams §ProjectionView: declarations, membership (a record's, and a set's members of a type), recorded outputs and applied ids are kept states too, read at a view's own position"
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
    /// A page at an earlier position reads the head index and the changes.
    SORTED_PAGE_PINNED = a_page_at_an_earlier_position_reads_the_head_index_and_the_keys_changed_since,
    "041, extended: a page ordered by an indexed field at an earlier position reads the head's order index and the keys of the type changed since, with their states at the position, so it answers what one scan over the states there answers, a key changed, retired or written after the position in its place there, and reads its page, at most three rows more and two per changed key, not the type; a commit between two pages of one view changes neither"
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

/// `scan_all()` with `edit` applied: the same value as the struct literal.
fn scan_with(edit: impl FnOnce(&mut Scan)) -> Scan {
    let mut scan = scan_all();
    edit(&mut scan);
    scan
}

fn by(field: &str, dir: Direction) -> Order {
    order(field, OrderBy::Value, dir)
}

fn declare(json: Value) -> Declaration {
    serde_json::from_value(json).expect("a declaration in the check is well-formed")
}

/// The `sort_index.t` setting asking for `fields`, as folded at `at`.
fn sort_setting(fields: Value, at: u64) -> StoredRecord {
    record(
        lakelands_api::seam::SETTING_TYPE,
        "sort_index.t",
        json!({"key": "sort_index.t", "value": fields}),
        at,
    )
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
            &at.scan(&ty, &scan_with(|s| s.order_by = vec![by("v", dir)]))
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

/// Records in the key-batch check: more than one batch of 1 024.
const BATCHED: u64 = 2100;

fn a_key_ordered_scan_answers_across_its_batches_as_one_scan(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    // Every third record retired and every fifth restricted at position
    // 1; every fourth moved to n = -1 at position 2.
    let state = |i: u64, at: u64| {
        let n = if at >= 2 && i.is_multiple_of(4) {
            -1
        } else {
            i as i64
        };
        let mut r = record("t", &format!("k{i:04}"), json!({"n": n}), at);
        r.retired = i.is_multiple_of(3);
        r.restricted = i.is_multiple_of(5);
        r
    };
    fold(engine, 1, |p| {
        (0..BATCHED).try_for_each(|i| p.put_record(&state(i, 1)))
    })?;
    fold(engine, 2, |p| {
        (0..BATCHED)
            .filter(|i| i.is_multiple_of(4))
            .try_for_each(|i| p.put_record(&state(i, 2)))
    })?;
    let ty = TypeName::from("t");
    let filter = crate::expr::parse("n >= 1000").expect("the check's filter parses");
    for at in [Some(Position(1)), None] {
        let view_at = view(engine, at)?;
        let pos = if at.is_some() { 1 } else { 2 };
        for flags in [false, true] {
            for filtered in [false, true] {
                for after in [None, Some("k1023"), Some("k2099"), Some("zzz")] {
                    for limit in [0, 1, 7, 1025, u32::MAX] {
                        let scan = Scan {
                            filter: filtered.then(|| filter.clone()),
                            after: after.map(|k| After {
                                values: vec![],
                                key: RecordKey::from(k),
                            }),
                            limit,
                            retired: flags,
                            restricted: flags,
                            ..Scan::default()
                        };
                        let expected: Vec<String> = (0..BATCHED)
                            .map(|i| state(i, pos))
                            .filter(|r| (flags || !r.retired) && (flags || !r.restricted))
                            .filter(|r| {
                                !filtered || r.fields.get("n").and_then(Value::as_i64) >= Some(1000)
                            })
                            .map(|r| r.key.to_string())
                            .filter(|k| after.is_none_or(|a| k.as_str() > a))
                            .take(limit as usize)
                            .collect();
                        let got = keys(&view_at.scan(&ty, &scan).must("scanning in key order")?);
                        ensure!(
                            got == expected,
                            "a key-ordered scan at position {pos} (flags {flags}, filter \
                             {filtered}, after {after:?}, limit {limit}) answered {} rows, \
                             first {:?}, last {:?}; one scan over every state answers {}, \
                             first {:?}, last {:?}",
                            got.len(),
                            got.first(),
                            got.last(),
                            expected.len(),
                            expected.first(),
                            expected.last()
                        );
                    }
                }
            }
        }
    }
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
            &at.scan(&ty, &scan_with(|s| s.filter = Some(expr.clone())))
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
    let all = run(scan_with(|s| (s.retired, s.restricted) = (true, true)))
        .must("scanning with both flags")?;
    ensure!(
        all.len() == 5,
        "a scan with both flags answered {all:?}; it must answer all five records"
    );
    let july = Timestamp::parse("2026-07-01T00:00:00Z").expect("an instant");
    let valid = run(scan_with(|s| s.valid_at = Some(july))).must("scanning at a valid time")?;
    ensure!(
        valid == ["open"],
        "a scan valid at 2026-07-01 answered {valid:?}; `ended` stopped in June and `undated` \
         cannot say when it began, so only `open` holds"
    );
    Ok(())
}

fn a_count_is_the_number_of_records_its_scan_selects(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    let timed = declare(json!({
        "declare": "type", "name": "t", "valid_from": "from", "valid_to": "to",
        "fields": [
            {"name": "from", "type": {"kind": "date_time"}},
            {"name": "to", "type": {"kind": "date_time"}},
            {"name": "tag", "type": {"kind": "text"}}
        ]
    }));
    let jan = json!({"from": "2026-01-01T00:00:00Z", "tag": "x"});
    fold(engine, 1, |p| {
        p.put_declaration(&timed, 1, None, Position(0))?;
        // More records than one batch, so a count reads on past the first.
        for i in 0..1100 {
            let tag = if i % 2 == 0 { "x" } else { "y" };
            let fields = json!({"from": "2026-01-01T00:00:00Z", "tag": tag});
            p.put_record(&record("t", &format!("b{i:04}"), fields, 1))?;
        }
        for (key, retired, restricted) in [("retired", true, false), ("held", false, true)] {
            let mut r = record("t", key, jan.clone(), 1);
            (r.retired, r.restricted) = (retired, restricted);
            p.put_record(&r)?;
        }
        p.put_record(&record(
            "t",
            "open",
            json!({"from": "2026-03-01T00:00:00Z"}),
            1,
        ))
    })?;
    // A later state of `open` supersedes its first, and `b0000` is retired.
    fold(engine, 2, |p| {
        let mut open = record("t", "open", json!({"from": "2026-08-01T00:00:00Z"}), 2);
        open.version = lakelands_api::Version(2);
        p.put_record(&open)?;
        let mut gone = record("t", "b0000", jan.clone(), 2);
        (gone.version, gone.retired) = (lakelands_api::Version(2), true);
        p.put_record(&gone)
    })?;
    let ty = TypeName::from("t");
    let july = Timestamp::parse("2026-07-01T00:00:00Z").expect("an instant");
    let tagged = crate::expr::parse("tag = 'x'").expect("the check's filter parses");
    let scans = [
        ("no flags", scan_all()),
        ("retired", scan_with(|s| s.retired = true)),
        ("restricted", scan_with(|s| s.restricted = true)),
        (
            "both flags",
            scan_with(|s| (s.retired, s.restricted) = (true, true)),
        ),
        (
            "valid at 2026-07-01",
            scan_with(|s| s.valid_at = Some(july)),
        ),
        ("tag = 'x'", scan_with(|s| s.filter = Some(tagged))),
        (
            "tag exactly x",
            scan_with(|s| s.equal = vec![("tag".into(), json!("x"))]),
        ),
        (
            "a page of 5 after b0500",
            Scan {
                limit: 5,
                after: Some(After {
                    values: Vec::new(),
                    key: "b0500".into(),
                }),
                ..scan_all()
            },
        ),
    ];
    for at in [None, Some(Position(1))] {
        let v = view(engine, at)?;
        for (what, scan) in &scans {
            let all = Scan {
                limit: u32::MAX,
                after: None,
                ..scan.clone()
            };
            let want = v
                .scan(&ty, &all)
                .must(&format!("scanning ({what}) at {at:?}"))?
                .len();
            let got = v
                .count(&ty, scan)
                .must(&format!("counting ({what}) at {at:?}"))?;
            ensure!(
                got == want as u64,
                "a count ({what}) at {at:?} must be the {want} records its scan selects, \
                 ignoring order, keyset position and limit; it answered {got}"
            );
        }
    }
    let head = view(engine, None)?
        .count(&ty, &scan_all())
        .must("counting at the head")?;
    ensure!(
        head == 1100,
        "at the head, 1 099 of the batch records are live (b0000 was retired) and `open` is \
         one record in its later state; the count answered {head}"
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
    let hits = at.lexical(&ty, "run rivers", None, 10).must("searching")?;
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
    let one = at
        .lexical(&ty, "run rivers", None, 1)
        .must("searching for one")?;
    ensure!(
        hit_keys(&one) == ["two"],
        "searching with k = 1 answered {:?}",
        hit_keys(&one)
    );
    let none = at
        .lexical(&ty, "ocean", None, 10)
        .must("searching for nothing")?;
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
    let river = then
        .lexical(&ty, "river", None, 10)
        .must("searching at 1")?;
    ensure!(
        hit_keys(&river) == ["kept", "later-retired"],
        "at position 1, `river` answered {:?}; the head's text holds `river` in `kept`, \
         `later-retired` and `new`, and at 1 only the first two were live",
        hit_keys(&river)
    );
    let ocean = then
        .lexical(&ty, "ocean", None, 10)
        .must("searching at 1")?;
    ensure!(
        ocean.is_empty(),
        "at position 1, `ocean` answered {:?}; ranking is by the text at the head, which no \
         longer holds `ocean`",
        hit_keys(&ocean)
    );
    let now = view(engine, None)?;
    let head = now
        .lexical(&ty, "river", None, 10)
        .must("searching at the head")?;
    ensure!(
        hit_keys(&head) == ["kept", "new"],
        "at the head, `river` answered {:?}; `later-retired` is retired there",
        hit_keys(&head)
    );
    Ok(())
}

fn lexical_over_named_fields_is_the_pipelines_over_those_fields(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    fold(engine, 1, |p| {
        p.put_declaration(&documents(), 1, None, Position(0))?;
        p.put_record(&record(
            "doc",
            "both",
            json!({"title": "rivers running", "body": "run"}),
            1,
        ))?;
        p.put_record(&record("doc", "t-only", json!({"title": "river"}), 1))?;
        // More records match only in `body` than a search limited to
        // `title` asks for, and every one sorts before `t-only`.
        for key in ["b-1", "b-2", "b-3"] {
            p.put_record(&record("doc", key, json!({"body": "river"}), 1))?;
        }
        p.put_record(&record("doc", "note", json!({"note": "river"}), 1))
    })?;
    fold_one(
        engine,
        2,
        record("doc", "late", json!({"title": "river"}), 2),
    )?;
    let ty = TypeName::from("doc");
    let title = [lakelands_api::id::FieldName::from("title")];
    let body = [lakelands_api::id::FieldName::from("body")];
    let now = view(engine, None)?;
    let all = now
        .lexical(&ty, "river run", None, 10)
        .must("searching every field")?;
    ensure!(
        hit_keys(&all) == ["both", "b-1", "b-2", "b-3", "late", "t-only"],
        "searching `river run` in every searchable field answered {:?}",
        hit_keys(&all)
    );
    let titled = now
        .lexical(&ty, "river run", Some(&title), 3)
        .must("searching `title`")?;
    ensure!(
        titled
            == [
                ("both".into(), 2.0),
                ("late".into(), 1.0),
                ("t-only".into(), 1.0)
            ],
        "searching `river run` in `title` with k = 3 answered {titled:?}; `both` holds both \
         tokens in its title, `late` and `t-only` one each, and `b-1` to `b-3` match only in \
         `body`, which was not named: a hit set cut at k before it was narrowed loses them"
    );
    let bodied = now
        .lexical(&ty, "river run", Some(&body), 10)
        .must("searching `body`")?;
    ensure!(
        bodied
            == [
                ("b-1".into(), 1.0),
                ("b-2".into(), 1.0),
                ("b-3".into(), 1.0),
                ("both".into(), 1.0)
            ],
        "searching `river run` in `body` answered {bodied:?}; `both` holds only `run` there"
    );
    let noted = now
        .lexical(
            &ty,
            "river",
            Some(&[lakelands_api::id::FieldName::from("note")]),
            10,
        )
        .must("searching `note`")?;
    ensure!(
        noted.is_empty(),
        "searching `river` in `note`, which is not searchable, answered {:?}",
        hit_keys(&noted)
    );
    let then = view(engine, Some(Position(1)))?
        .lexical(&ty, "river run", Some(&title), 10)
        .must("searching `title` at 1")?;
    ensure!(
        hit_keys(&then) == ["both", "t-only"],
        "at position 1, searching `river run` in `title` answered {:?}; `late` was not yet \
         live",
        hit_keys(&then)
    );
    Ok(())
}

fn a_record_put_after_a_redeclaration_in_the_same_transaction_is_indexed_under_it(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    let narrow = declare(json!({
        "declare": "type", "name": "doc", "language": "en",
        "fields": [
            {"name": "title", "type": {"kind": "text"}, "searchable": true},
            {"name": "body", "type": {"kind": "text"}}
        ]
    }));
    fold(engine, 4, |p| {
        p.put_declaration(&narrow, 1, None, Position(1))?;
        p.put_record(&record("doc", "early", json!({"body": "river"}), 2))?;
        p.put_declaration(&documents(), 2, None, Position(3))?;
        p.put_record(&record("doc", "late", json!({"body": "river"}), 4))
    })?;
    let hits = view(engine, None)?
        .lexical(&TypeName::from("doc"), "river", None, 10)
        .must("searching")?;
    ensure!(
        hit_keys(&hits) == ["early", "late"],
        "one transaction declared `doc` with an unsearchable `body`, put `early`, declared it \
         again with `body` searchable and put `late`, both with `river` in `body`; searching \
         `river` answered {:?}, but under the declaration in force both are hits",
        hit_keys(&hits)
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
        matches!(reason(&wrong), Some(Refusal::Invalid { .. })),
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
        p.put_membership("big", &ty, &"stay".into(), true, Position(3))?;
        p.put_membership("big", &"u".into(), &"x".into(), true, Position(3))?;
        p.put_membership("small", &ty, &"y".into(), true, Position(3))?;
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
    let members = |v: &dyn lakelands_api::engine::ProjectionView, set: &str| {
        let held = v.members(set, &ty).must("reading a set's members")?;
        Ok::<_, Failure>(held.iter().map(ToString::to_string).collect::<Vec<_>>())
    };
    let (was, is, never) = (
        members(then.as_ref(), "big")?,
        members(now.as_ref(), "big")?,
        members(now.as_ref(), "other")?,
    );
    ensure!(
        was == ["k"] && is == ["stay"] && never.is_empty(),
        "`big` holds `k` of `t` at 2 and `stay` at the head, whatever another set or type \
         holds; its members read {was:?} and {is:?}, an unknown set's {never:?}"
    );
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
    use ProposalState::{Accepted, Pending};
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
        key: Some("k".into()),
        payload: Default::default(),
        opts: Default::default(),
    };
    fold(engine, 3, |p| {
        p.put_declaration(&verb, 1, None, Position(0))?;
        p.put_proposal(&proposal("p1", 1, retire("invoice"), Pending), Position(1))?;
        p.put_proposal(&proposal("p2", 2, retire("customer"), Pending), Position(2))?;
        p.put_proposal(&proposal("p3", 3, call.clone(), Pending), Position(3))
    })?;
    fold(engine, 4, |p| {
        p.put_proposal(&proposal("p3", 4, call, Accepted), Position(4))
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
        state: Some(Pending),
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
            state: Some(Pending),
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
            .lexical(&ty, query, None, 10)
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
            .edges(&TypeName::from(ty), &[RecordKey::from(key)], None, walk)
            .must("reading edges")
            .map(|mut e| {
                e.remove(0)
                    .into_iter()
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

/// The values a field-ordered walk sorts: every kind the rulebook ranks,
/// equal values written differently, and ties.
fn order_value(i: u64) -> Value {
    let values = [
        json!(3),
        json!("3.0"),
        json!(-2.5),
        json!("10"),
        json!("2026-01-01T00:00:00Z"),
        json!("2026-01-01T01:00:00+01:00"),
        json!("2025-06-30"),
        json!("apple"),
        json!("Apple"),
        json!("apple"),
        json!(""),
        json!(false),
        json!(true),
        json!({"a": 1}),
        json!([1, 2]),
        json!(0),
        json!("-0.00"),
        // Four texts that part only after the order index's cut.
        json!("a text long enough to pass the cut, then z"),
        json!("a text long enough to pass the cut, then b"),
        json!("a text long enough to pass the cut, then m"),
        json!("a text long enough to pass the cut, then a"),
    ];
    values[(i as usize * 7) % values.len()].clone()
}

fn a_field_ordered_scan_walks_page_by_page_as_one_scan(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    const N: u64 = 160;
    let declare = |fields: &[&str]| -> Result<Declaration, Failure> {
        let fields: Vec<Value> = fields
            .iter()
            .map(|f| json!({"name": f, "type": {"kind": "text"}}))
            .collect();
        serde_json::from_value(json!({"declare": "type", "name": "t", "fields": fields}))
            .map_err(|e| Failure::new(format!("the check's declaration does not read: {e}")))
    };
    // `v` is missing from every fifth record, `w` has three values, and
    // `x` is carried before any declaration names it.
    let state = |i: u64, at: u64| {
        let mut fields = serde_json::Map::new();
        let v = if at >= 2 && i.is_multiple_of(4) {
            order_value(i + 5)
        } else {
            order_value(i)
        };
        if !(i.is_multiple_of(5) ^ (at >= 2 && i.is_multiple_of(10))) {
            fields.insert("v".into(), v);
        }
        fields.insert("w".into(), json!(["a", "b", "c"][(i % 3) as usize]));
        if !i.is_multiple_of(6) {
            fields.insert("x".into(), json!((i * 13) % 17));
        }
        let mut r = record(
            "t",
            &format!("k{:03}", (i * 37) % 997),
            Value::Object(fields),
            at,
        );
        r.retired = i.is_multiple_of(9);
        r.restricted = i.is_multiple_of(11);
        r
    };
    let two = declare(&["v", "w"])?;
    let three = declare(&["v", "w", "x"])?;
    // The lake asks for the order index on `v`, `w` and `x` after the
    // records are in, before any declaration names `x`; later it drops
    // `v`, so an order by `v` sorts again.
    fold(engine, 1, |p| {
        p.put_declaration(&two, 1, None, Position(0))?;
        (0..N).try_for_each(|i| p.put_record(&state(i, 1)))
    })?;
    // Every fourth moves, and every tenth loses or gains `v`, some twice
    // in the transaction; then the setting.
    fold(engine, 2, |p| {
        (0..N)
            .filter(|i| i.is_multiple_of(4) || i.is_multiple_of(10))
            .try_for_each(|i| {
                let mut again = state(i, 1);
                again.position = Position(2);
                p.put_record(&again)?;
                p.put_record(&state(i, 2))
            })?;
        p.put_record(&sort_setting(json!(["v", "w", "x"]), 2))
    })?;
    fold(engine, 3, |p| {
        p.put_declaration(&three, 2, None, Position(3))
    })?;
    walk_field_orders(
        engine,
        Some(Position(1)),
        &two,
        1,
        N,
        &state,
        "at position 1",
    )?;
    walk_field_orders(engine, None, &three, 2, N, &state, "at the head")?;
    fold(engine, 4, |p| {
        p.put_record(&sort_setting(json!(["w", "x"]), 4))
    })?;
    walk_field_orders(
        engine,
        None,
        &three,
        2,
        N,
        &state,
        "at the head, the setting without `v`",
    )
}

/// Walk every field order of the check's type `t` page by page at `at`,
/// against one scan over the states `state(i, pos)` under `decl`.
fn walk_field_orders(
    engine: &dyn LakeEngine,
    at: Option<Position>,
    decl: &Declaration,
    pos: u64,
    n: u64,
    state: &dyn Fn(u64, u64) -> StoredRecord,
    when: &str,
) -> Result<(), Failure> {
    let ty = TypeName::from("t");
    let by = |field: &str, dir| order(field, OrderBy::Value, dir);
    let filter = crate::expr::parse("w != 'b'").expect("the check's filter parses");
    let Declaration::Type(decl) = decl else {
        unreachable!("the check declares types")
    };
    let states: Vec<StoredRecord> = (0..n).map(|i| state(i, pos)).collect();
    let view_at = view(engine, at)?;
    for order_by in [
        vec![by("v", Direction::Asc)],
        vec![by("v", Direction::Desc)],
        vec![by("w", Direction::Asc), by("v", Direction::Desc)],
        vec![by("x", Direction::Desc), by("w", Direction::Asc)],
        vec![by("x", Direction::Asc)],
    ] {
        for filtered in [false, true] {
            for flags in [false, true] {
                for limit in [1, 7, 50] {
                    let scan = Scan {
                        filter: filtered.then(|| filter.clone()),
                        order_by: order_by.clone(),
                        limit,
                        retired: flags,
                        restricted: flags,
                        ..Scan::default()
                    };
                    let expected = keys(&crate::engines::scan(
                        Some(decl),
                        &states,
                        &Scan {
                            limit: u32::MAX,
                            ..scan.clone()
                        },
                    ));
                    let mut got = Vec::new();
                    let mut page = scan.clone();
                    for _ in 0..=n {
                        let rows = view_at.scan(&ty, &page).must("scanning a page")?;
                        let Some(last) = rows.last() else { break };
                        page.after = Some(After {
                            values: order_by
                                .iter()
                                .map(|o| last.fields.get(&o.field).cloned().unwrap_or(Value::Null))
                                .collect(),
                            key: last.key.clone(),
                        });
                        got.extend(keys(&rows));
                    }
                    let first_wrong = got
                        .iter()
                        .zip(&expected)
                        .position(|(g, e)| g != e)
                        .unwrap_or(got.len().min(expected.len()));
                    ensure!(
                        got == expected,
                        "walking {order_by:?} {when} (filter {filtered}, flags {flags}) in \
                         pages of {limit} answered {} rows, one scan over every state {}; they \
                         part at row {first_wrong}: {:?} against {:?}",
                        got.len(),
                        expected.len(),
                        got.get(first_wrong),
                        expected.get(first_wrong)
                    );
                }
            }
        }
    }
    Ok(())
}

fn a_page_ordered_by_an_indexed_field_reads_its_page_and_the_setting_switches_it(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    const N: u64 = 1000;
    const LIMIT: u32 = 10;
    let decl: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "t",
        "fields": [{"name": "v", "type": {"kind": "text"}}]
    }))
    .map_err(|e| Failure::new(format!("the check's declaration does not read: {e}")))?;
    let Declaration::Type(t) = &decl else {
        unreachable!("the check declares a type")
    };
    // Every record holds its own `v`, told apart in its first bytes but
    // longer than the index keeps, so each is a run of one that a page
    // continued after it starts from.
    let states: Vec<StoredRecord> = (0..N)
        .map(|i| {
            record(
                "t",
                &format!("k{i:04}"),
                json!({"v": format!("v{:04} {}", (i * 7919) % N, "past the cut ".repeat(3))}),
                1,
            )
        })
        .collect();
    fold(engine, 1, |p| {
        p.put_declaration(&decl, 1, None, Position(0))?;
        states.iter().try_for_each(|r| p.put_record(r))
    })?;
    let ty = TypeName::from("t");
    // The rows the engine handed the rulebook for one page, and the page:
    // the first page, or the one after the first's last row.
    let page = |dir, continued: bool| -> Result<(u64, bool), Failure> {
        let mut scan = Scan {
            order_by: vec![order("v", OrderBy::Value, dir)],
            limit: LIMIT,
            ..Scan::default()
        };
        if continued {
            let first = crate::engines::scan(Some(t), &states, &scan);
            let last = first.last().expect("a first page");
            scan.after = Some(After {
                values: vec![last.fields.get("v").cloned().unwrap_or(Value::Null)],
                key: last.key.clone(),
            });
        }
        let view = view(engine, None)?;
        let before = crate::engines::rows_read();
        let rows = view
            .scan(&ty, &scan)
            .must("scanning a page ordered by `v`")?;
        let read = crate::engines::rows_read() - before;
        Ok((
            read,
            keys(&rows) == keys(&crate::engines::scan(Some(t), &states, &scan)),
        ))
    };
    // No setting; the setting on `v`; retired; on `v` again; on nothing.
    let mut at = 1;
    for (asked, index) in [
        (None, false),
        (Some(json!(["v"])), true),
        (Some(Value::Null), false),
        (Some(json!(["v"])), true),
        (Some(json!([])), false),
    ] {
        if let Some(fields) = &asked {
            at += 1;
            let mut state = sort_setting(fields.clone(), at);
            if fields.is_null() {
                state.fields = sort_setting(json!(["v"]), at).fields;
                state.retired = true;
            }
            fold(engine, at, |p| p.put_record(&state))?;
        }
        for (dir, continued) in [
            (Direction::Asc, false),
            (Direction::Desc, false),
            (Direction::Asc, true),
            (Direction::Desc, true),
        ] {
            let (read, right) = page(dir, continued)?;
            let asked = match &asked {
                None => "no setting".to_owned(),
                Some(Value::Null) => "the setting retired".to_owned(),
                Some(f) => format!("the setting {f}"),
            };
            let which = if continued {
                "the second page"
            } else {
                "a page"
            };
            ensure!(
                right,
                "with {asked}, {which} of {LIMIT} ordered by `v` {dir:?} is not what one scan \
                 over every state answers"
            );
            if index {
                // The row `after` names, read again as the start of its
                // run, and the rows that close the page's last run.
                ensure!(
                    read <= u64::from(LIMIT) + 3,
                    "with {asked}, {which} of {LIMIT} ordered by `v` {dir:?} read {read} of the \
                     type's {N} records; the order index reads its page and at most three more"
                );
            } else {
                ensure!(
                    read >= N,
                    "with {asked}, {which} of {LIMIT} ordered by `v` {dir:?} read {read} rows; \
                     with no index on `v` it sorts all {N} records, so the index answered \
                     what the lake did not ask it for"
                );
            }
        }
    }
    Ok(())
}

fn a_page_at_an_earlier_position_reads_the_head_index_and_the_keys_changed_since(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    const N: u64 = 1000;
    const LIMIT: u32 = 10;
    let decl = declare(json!({
        "declare": "type", "name": "t",
        "fields": [{"name": "v", "type": {"kind": "text"}}]
    }));
    let Declaration::Type(t) = &decl else {
        unreachable!("the check declares a type")
    };
    let v = |n: u64| json!({"v": format!("v{n:04} {}", "past the cut ".repeat(3))});
    let states: Vec<StoredRecord> = (0..N)
        .map(|i| record("t", &format!("k{i:04}"), v((i * 7919) % N), 1))
        .collect();
    fold(engine, 1, |p| {
        p.put_declaration(&decl, 1, None, Position(0))?;
        states.iter().try_for_each(|r| p.put_record(r))
    })?;
    fold_one(engine, 2, sort_setting(json!(["v"]), 2))?;
    let ty = TypeName::from("t");
    let asc = |dir| scan_with(|s| s.order_by = vec![by("v", dir)]);
    let first = keys(&crate::engines::scan(
        Some(t),
        &states,
        &asc(Direction::Asc),
    ));
    // After the position: the first in order moves last, the second is
    // retired, a new key sorts first, and another type is written.
    let mut moved = record("t", &first[0], json!({"v": "zz"}), 3);
    moved.version = lakelands_api::Version(2);
    let mut retired = record("t", &first[1], v(1), 3);
    retired.retired = true;
    fold(engine, 3, |p| {
        p.put_record(&moved)?;
        p.put_record(&retired)?;
        p.put_record(&record("t", "a-new", json!({"v": "a"}), 3))?;
        p.put_record(&record("u", "u1", json!({"v": "a"}), 3))
    })?;
    let pinned = view(engine, Some(Position(2)))?;
    let page = |scan: &Scan, changed: u64, what: &str| -> Result<Vec<StoredRecord>, Failure> {
        let before = crate::engines::rows_read();
        let rows = pinned.scan(&ty, scan).must(what)?;
        let read = crate::engines::rows_read() - before;
        let whole = crate::engines::scan(Some(t), &states, scan);
        ensure!(
            keys(&rows) == keys(&whole),
            "{what} at position 2 answers {:?}, not {:?}, what one scan over the states there \
             answers",
            keys(&rows),
            keys(&whole)
        );
        ensure!(
            read <= u64::from(LIMIT) + 3 + 2 * changed,
            "{what} at position 2 read {read} rows of the type's {N}, with {changed} key(s) \
             changed since; the head's order index reads its page, at most three more and two \
             per changed key"
        );
        Ok(rows)
    };
    for dir in [Direction::Asc, Direction::Desc] {
        let scan = scan_with(|s| {
            s.order_by = vec![by("v", dir)];
            s.limit = LIMIT;
        });
        // The second direction's first page sees the first's commit too.
        let changed = 3 + u64::from(dir == Direction::Desc);
        let rows = page(&scan, changed, &format!("a page ordered by `v` {dir:?}"))?;
        let last = rows.last().expect("a first page");
        // A commit between the two pages of one view: the view's position
        // stays where it was.
        let after = 4 + u64::from(dir == Direction::Desc);
        fold_one(
            engine,
            after,
            record("t", &first[15], json!({"v": "b"}), after),
        )?;
        let next = Scan {
            after: Some(After {
                values: vec![last.fields.get("v").cloned().unwrap_or(Value::Null)],
                key: last.key.clone(),
            }),
            ..scan
        };
        page(
            &next,
            4,
            &format!("the next page ordered by `v` {dir:?}, after a commit"),
        )?;
    }
    Ok(())
}

fn a_records_edges_at_the_head_read_its_links_not_the_type(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    use lakelands_api::query::Walk;
    const N: u64 = 1000;
    let partner = declare(json!({"declare": "type", "name": "partner", "fields": []}));
    // `approved_by` is carried by link records, `billed_by` by the
    // reference field `billed`.
    let invoice = declare(json!({
        "declare": "type", "name": "invoice",
        "fields": [{"name": "billed", "type": {"kind": "reference", "target": "partner"}}],
        "relations": [
            {"name": "approved_by", "target": "partner"},
            {"name": "billed_by", "target": "partner", "via": "billed"}
        ]
    }));
    let Declaration::Type(t) = &invoice else {
        unreachable!("declared as a type")
    };
    let lt = crate::links::link_types_of(t).remove(0);
    let partner_of = |i: u64| format!("p{:03}", i % 250);
    fold(engine, 1, |p| {
        p.put_declaration(&partner, 1, None, Position(0))?;
        p.put_declaration(&invoice, 1, None, Position(1))?;
        p.put_declaration(&Declaration::Type(lt.clone()), 1, None, Position(1))?;
        (0..N).try_for_each(|i| {
            let (inv, to) = (format!("i{i:04}"), partner_of(i));
            p.put_record(&record("invoice", &inv, json!({"billed": to}), 1))?;
            let key = crate::links::link_key(&inv.as_str().into(), &to.as_str().into());
            p.put_record(&record(
                lt.name.as_str(),
                key.as_str(),
                json!({"from": inv, "to": to}),
                1,
            ))
        })
    })?;
    let view = view(engine, None)?;
    for (ty, key, relation, walk, want) in [
        ("partner", "p007", "approved_by", Walk::In, 4),
        ("invoice", "i0007", "approved_by", Walk::Out, 1),
        ("partner", "p007", "billed_by", Walk::In, 4),
    ] {
        let before = crate::engines::links_read();
        let edges = view
            .edges(
                &TypeName::from(ty),
                &[RecordKey::from(key)],
                Some(&relation.into()),
                walk,
            )
            .must("reading edges")?
            .remove(0);
        let read = crate::engines::links_read() - before;
        ensure!(
            edges.len() == want,
            "{walk:?} over `{relation}` at `{ty}` `{key}` answered {} edges, not {want}",
            edges.len()
        );
        ensure!(
            read <= want as u64 + 2,
            "{walk:?} over `{relation}` at `{ty}` `{key}` returned {want} edges and handed the \
             rulebook {read} records of the {N} in the type: at the head a record's edges are \
             found from the record, not by reading the type"
        );
    }
    Ok(())
}

fn a_scan_with_equal_answers_exactly_the_identical_values(
    engine: &dyn LakeEngine,
) -> Result<(), Failure> {
    fold(engine, 1, |p| {
        for (key, v) in [
            ("a", json!("10")),
            ("b", json!("010")),
            ("c", json!(10)),
            ("d", json!("10.0")),
            ("e", json!("2026-01-01T00:00:00Z")),
            ("f", json!("2026-01-01T00:00:00.000000Z")),
            ("g", json!("10")),
        ] {
            p.put_record(&record("t", key, json!({"v": v}), 1))?;
        }
        p.put_record(&record("t", "h", json!({}), 1))
    })?;
    // Later, `g` moves away: the head and position 1 differ.
    fold_one(engine, 2, record("t", "g", json!({"v": "11"}), 2))?;
    let ty = TypeName::from("t");
    for (at, head_g) in [(None, false), (Some(Position(1)), true)] {
        let v = view(engine, at)?;
        for (value, expected) in [
            (json!("10"), if head_g { vec!["a", "g"] } else { vec!["a"] }),
            (json!("010"), vec!["b"]),
            (json!(10), vec!["c"]),
            (json!("2026-01-01T00:00:00Z"), vec!["e"]),
            (json!(null), vec![]),
        ] {
            let got = keys(
                &v.scan(
                    &ty,
                    &scan_with(|s| s.equal = vec![("v".into(), value.clone())]),
                )
                .must(&format!("scanning for exactly {value} at {at:?}"))?,
            );
            ensure!(
                got == expected,
                "a scan for exactly {value} at {at:?} selected {got:?}; identity selects \
                 {expected:?}"
            );
        }
    }
    Ok(())
}

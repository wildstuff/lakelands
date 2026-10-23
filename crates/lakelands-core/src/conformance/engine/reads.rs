//! What a view answers through the seam: scans, proposals and the other
//! kept states, and what `ranked` answers of the head's indexes
//! (`docs/seams.md` §`ProjectionView`, §`ranked`, §`VectorIndex`,
//! `docs/decisions/029-every-answer-has-one-order.md`).
//!
//! The store asks the engine for its pages: a scan's order, continuation
//! and limit are what `find` answers from. A scan ordered by the key alone
//! is read in batches from `after` (`lakelands_core::engines::
//! scan_in_key_order`), and answers as one scan over every state would. Full text and the
//! vectors rank the head's live records, all lists of one call from one
//! state, with its position. A vector's list is exact: the true top `k`
//! by cosine, ties by key.

use std::collections::BTreeMap;

use lakelands_api::engine::{
    After, Declared, LakeEngine, ProjectionStore, ProjectionView, Rankings, Scan,
};
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::{FieldName, Position, RecordKey, TypeName};
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::{Declaration, Proposal, ProposalFilter, ProposalState, ProposedWrite};
use lakelands_api::{Refusal, Timestamp};
use serde_json::{Value, json};

use super::EngineHarness;
use super::projection::{fold, fold_one, record, view};
use crate::conformance::fixture::{Must, order, reason};
use crate::conformance::{Failure, Held};

engine_check!(
    COUNT_IS_THE_SCAN = a_count_is_the_number_of_records_its_scan_selects,
    "seams §ProjectionView and 029: `count` answers the number of records `scan` selects, its flags, `valid_at`, `filter` and `equal` applied and its order, keyset position and limit ignored, at the head and at an earlier position, across more than one batch, a superseded state and a retired record not counted"
);

engine_check!(
    KEPT_NAME = a_kept_name_answers_as_a_stored_type_holding_the_same_states,
    "seams §ProjectionStore and lakelands#575: the same states put under a view's kept name, `sys:kept.<view>`, and under an ordinary one answer alike through `get`, `get_many`, a walk by key from `after` across batches, a page under a filter, in a field's order and under `equal`, and a count under each flag and under a filter, at the head and at an earlier position, over a key put thousands of times, keys first put after that position between older ones, and an ended, a retired and a restricted key; an output recorded as often is read at both"
);

engine_check!(
    MIXED_KINDS = mixed_kinds_sort_the_same_on_every_engine,
    "029: a scan orders numbers, then instants, then text, then booleans, then JSON, a missing value last in either direction, ties by key, so every engine puts the same records in the same places"
);

engine_check!(
    AFTER_AND_LIMIT = a_scan_continues_strictly_after_its_keyset_position,
    "seams §scan: a scan answers only what sorts strictly after `after`, and at most `limit` of it, 0 answering nothing, so an engine that pages natively gives the store's pages"
);

engine_check!(
    KEY_BATCHES = a_key_ordered_scan_answers_across_its_batches_as_one_scan,
    "seams §scan and lakelands#134: a scan ordered by the key alone, read in batches from `after`, answers what one scan over every state answers, whatever its limit, its `after`, its flags and its filter, at the head and at an earlier position, however many rows each batch passes over"
);

engine_check!(
    TEXT_PAGE = a_page_by_key_under_an_exact_text_reads_from_after_not_every_match,
    "seams §scan and lakelands#574: a scan ordered by the key alone that must hold a text exactly, by a term of its filter or by `equal`, walked page by page from `after` and read whole, answers what one scan over every state answers, past records that hold the text and do not qualify (another term, retired, restricted) and a key that ended on both sides of a batch's end, over keys a database's collation orders otherwise than their bytes; a walk is handed at most twice the type's rows and a batch, not every record holding the text for each page, and a count no more rows than hold the text and a batch"
);

engine_check!(
    FIELD_ORDER_WALK = a_field_ordered_scan_walks_page_by_page_as_one_scan,
    "seams §scan and lakelands#137: a scan ordered by fields, walked page by page from `after`, answers what one scan over every state answers, whatever its directions, its filter (an exact text among them, which many or few records hold, lakelands#587) and its flags, after later puts (of keys first put after the position among them), a redeclaration and a change of the `sort_index.<type>` setting, at the head and at an earlier position"
);

engine_check!(
    SORTED_PAGE = a_page_ordered_by_an_indexed_field_reads_its_page_and_the_setting_switches_it,
    "041 and lakelands#137: a page of 10 ordered by a field the `sort_index.<type>` setting names reads its page and at most three rows more, not the type, the first page and one continued after a value longer than the index keeps alike; the setting's write switches the same page from sorting the type to the index and back, and the answer stays what one scan over every state answers; and so for a view's kept records, whose index its `sort_index.<view>` setting asks for of the fields the view declares (lakelands#668)"
);

engine_check!(
    CHOSEN_ORDER = a_chosen_order_index_is_built_beside_the_fold_and_read_only_once_it_is_kept,
    "041 and lakelands#1131: a field the lake chose an order index for beside the log is started, filled in chunks of the head's records while puts land between the chunks, and kept; a page ordered by it sorts the type until it is kept, answers what one scan over every state answers all the while, and reads its page and at most three rows more once kept, the records put during the build among those it holds; `chosen` names the kept and the building fields; a `sort_index.<type>` setting naming a field takes the choice away and a step on such a type does nothing, and retiring the setting leaves the type with no index; a field dropped is sorted whole again; a `Start` of a field the type does not declare does nothing"
);

engine_check!(
    CHOSEN_LEFT_BEHIND = reopening a_chosen_field_a_writer_without_it_folded_past_is_read_by_no_view,
    "041 and lakelands#1131: a writer that keeps no chosen field (a binary before them) folds past a chosen field's rows and moves every `sorted.<type>` mark all the same; a page ordered by the field then answers what one scan over every state answers, read-only, after commits and a step of choosing through the handle that chose (its lease taken back with no open) and after a writer's open, and `chosen` names none, and the open drops the choice, so a later commit does not make the rows it left read again, and the lake chooses again; and so after such a writer's rebuild through the same position, which empties the order index and keeps every mark, the lake choosing again after a writer's open"
);

engine_check!(
    EQUAL_IS_IDENTITY = a_scan_with_equal_answers_exactly_the_identical_values,
    "044 and seams §scan: a scan's `equal` selects the records whose field holds exactly the JSON value, never the expression's `=` (`'010'` is not `'10'`, text is not a number, an instant in another written form is not the same), never a null, at the head and at an earlier position alike"
);

engine_check!(
    EQUAL_ANY_IS_EACH = a_scan_with_equal_any_answers_what_one_equal_a_tuple_answers,
    "044, seams §scan and lakelands#971: a scan's `equal_any` selects the records that hold one of its tuples exactly, what one scan with `equal` a tuple selects, together and in key order: one field and two, text and a number, `'010'` beside `'10'`, a null that selects nothing, at the head and at an earlier position, read whole and walked one record at a time from `after`"
);

engine_check!(
    FILTER_AS_EVALUATOR = scan_filters_exactly_as_the_evaluator_including_mixed_kinds_and_errors,
    "seams §scan: an engine selects exactly the records the one evaluator selects, comparisons across kinds included, and a filter that errors for a record does not select it"
);

engine_check!(
    FLAGS_AND_VALID_AT = scan_honours_retired_restricted_and_valid_at,
    "seams §scan: retired and restricted records come only when asked for, and a valid-timed type's records only when valid at `valid_at`"
);

engine_check!(
    LEXICAL_HITS = lexical_hits_are_the_pipelines_over_searchable_live_records,
    "025 and seams §ranked: a hit holds a token the one pipeline makes of the query in a searchable field of a live record, restricted ones included (the store applies grants); the hit set is the same on every engine, ties by key, at most k"
);

engine_check!(
    LEXICAL_BEYOND_ASCII = lexical_finds_every_token_the_pipeline_makes_beyond_ascii,
    "025: every token the one pipeline makes is a term an engine finds, in any script, combining marks included, so no engine's own tokenizer narrows the hit set"
);

engine_check!(
    RANKED_ONE_STATE = ranked_answers_every_list_from_one_state_and_says_its_position,
    "025 and seams §ranked: one call answers its text's list and each vector's, in that order, from one state of the head's indexes, with the position the projection is folded through there; an open transaction has moved neither a list nor the position, and its commit moves both; a record retired at the head is no candidate, whatever it held before"
);

engine_check!(
    LEXICAL_NAMED_FIELDS = lexical_over_named_fields_is_the_pipelines_over_those_fields,
    "025 and seams §ranked: a search limited to named fields matches, scores and ties over the head text of the named searchable fields only, and no engine cuts its candidates at k before it narrows them: a record matching in a named field is a hit however many others match only in fields not named"
);

engine_check!(
    LEXICAL_WEIGHTED = a_weighed_text_scores_each_token_at_its_heaviest_field_before_the_cut,
    "025 and seams §ranked, extended (lakelands#1230): a text's list weighs each distinct query token at the heaviest searched field holding it, a field not weighed at 1, and sums them in the tokens' order, so a token in two fields counts once and an unweighed list scores the tokens' count; the list is cut at k by that score, so a candidate holding fewer tokens in a heavier field outranks one holding more in a lighter one; a weight for a field the search does not name weighs nothing"
);

engine_check!(
    LEXICAL_REDECLARED_MID_FOLD =
        a_record_put_after_a_redeclaration_in_the_same_transaction_is_indexed_under_it,
    "025 and seams §ProjectionStore: a put reads the declarations as the transaction holds them, so a type declared again inside one fold's transaction governs every record put after it, and the records it already held"
);

engine_check!(
    NEAREST_BY_COSINE = nearest_ranks_by_cosine_with_ties_by_key,
    "seams §ranked: a vector's list is the exact top k by the cosine of the angle, best first, equal scores by key"
);

engine_check!(
    ZERO_AND_WRONG_LENGTH = a_zero_vector_scores_zero_and_a_wrong_length_is_invalid,
    "seams §ranked: a vector of zeros scores 0 against everything, and a query whose length differs from a stored vector's is refused as invalid"
);

engine_check!(
    PUT_AND_REMOVE = put_replaces_a_vector_and_remove_forgets_it,
    "seams §VectorIndex: put replaces a record's vector for the field, remove forgets it, forgetting one that is absent does nothing, and count counts"
);

engine_check!(
    PUT_OVER_A_STATE = put_and_remove_answer_alike_over_a_vector_a_state_holds,
    "072, seams §VectorIndex: put and remove answer over a record whose state holds the field as they answer alone, in the state's transaction and after it: a put of a field the type does not keep apart is counted, a put of other elements replaces the state's, and a remove forgets them"
);

engine_check!(
    LINK_EDGES = edges_over_a_link_relation_come_from_the_live_link_records_at_the_views_position,
    "035 and seams §edges: an engine answers a relation with neither `via` nor `derived` from its live link records at the view's position, out and in, each edge carrying the link's own fields"
);

engine_check!(
    EDGES_READ_WHAT_THEY_REACH = a_records_edges_at_the_head_read_its_links_not_the_type,
    "seams §edges and lakelands#190: at the head, the edges into or out of a record over a relation carried by link records, and the edges into it over one carried by a reference field, are found without reading the link type or the declaring type: the records an engine hands the rulebook to find them are about the edges it returns"
);

engine_check!(
    EARLIER_STATES = declarations_membership_outputs_and_applied_ids_read_at_an_earlier_position,
    "seams §ProjectionView: declarations, membership (a record's, and a set's members of a type), recorded outputs and applied ids are kept states too, read at a view's own position, applied ids one or many in one call"
);

engine_check!(
    LATER_OUTPUT_HOLDS = a_later_output_for_the_same_call_replaces_the_earlier_from_its_position,
    "022, extended, and seams §ProjectionView: the log may hold two outputs for one seam and input (a failure, then the answer a redeclaration asked for); the later one holds from its position, the earlier one before it; an output under the store's own seam name, `sys:renames`, is kept and answered as of a position like any other (lakelands#527)"
);

engine_check!(
    SOURCE_ROUTES = the_index_answers_a_sources_positions_in_order_to_its_through,
    "058 and seams §ProjectionView (lakelands#1151): `of_source` answers the positions of the applied events of a source, each once however many types it is about, in order, at most `limit` with `through` the last of them, else through the range asked, nothing past a view's position, and nothing decided where a rebuild's refold has not reached the range"
);

engine_check!(
    FIRST_APPLIED = the_first_applied_position_is_kept,
    "seams §ProjectionStore: marking an id applied twice keeps the first position, so a redelivery is recognised where the event first took effect"
);

engine_check!(
    ROUTES = the_index_answers_a_filters_positions_in_order_to_its_through,
    "seams §ProjectionView and lakelands#678: `matching` answers the positions whose applied events a filter admits, by kind, by type, by key and by a type with its key, an event about several types to each of them and one about no type to no filter naming types, in order and each once however many of its values match or types it is about, at most `limit` with `through` the last of them, else through the range asked, and nothing past a view's position, through which it answers; nor past what the projection has folded where the index is read, so a view taken before a rebuild's `clear` answers through the refold, and decides nothing where the refold has not reached the range"
);

engine_check!(
    SORTED_PAGE_PINNED =
        a_page_at_an_earlier_position_reads_the_head_index_and_the_keys_changed_since,
    "041, extended: a page ordered by an indexed field at an earlier position reads the head's order index and the keys of the type changed since, with their states at the position, so it answers what one scan over the states there answers, a key changed, retired or written after the position in its place there, and reads its page, at most three rows more and two per changed key, not the type; a commit between two pages of one view changes neither"
);

engine_check!(
    PROPOSALS_PAGED = proposals_come_newest_first_a_page_at_a_time,
    "seams §ProjectionView (lakelands#1063): `proposals_before` answers, at the head and at earlier positions, for every state filter and pages of one, two and four, the proposals `proposals` answers whole, newest first, each page those before the last one read: a state is judged as it stood at the view, and a page holds no more than its limit"
);

engine_check!(
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

fn mixed_kinds_sort_the_same_on_every_engine(engine: &dyn LakeEngine) -> Held {
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

fn a_scan_continues_strictly_after_its_keyset_position(engine: &dyn LakeEngine) -> Held {
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

fn a_key_ordered_scan_answers_across_its_batches_as_one_scan(engine: &dyn LakeEngine) -> Held {
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

fn a_page_by_key_under_an_exact_text_reads_from_after_not_every_match(
    engine: &dyn LakeEngine,
) -> Held {
    const N: u64 = 4100;
    const BATCH: u64 = 1024;
    // Keys in three runs, which the bytes order `B`, `_`, `a` and a
    // database's collation otherwise. All but two hold `v = 'a'`. On each
    // side of the first batch's end, be it a batch of the type (after
    // 1 023) or of the records holding the text (after 1 024): one that
    // fails the filter's other term, a retired one, a restricted one, and
    // a key that ended after it held the text.
    let first: Vec<(StoredRecord, bool)> = (0..N)
        .map(|i| {
            let odd = (1020..=1028).contains(&i).then(|| (i - 1020) % 5);
            let key = format!("{}{i:04}", ["B", "_", "a"][(i / 1367) as usize]);
            let v = if i % 1500 == 1499 { "b" } else { "a" };
            let n = if odd == Some(0) { -1 } else { 1 };
            let mut r = record("t", &key, json!({"v": v, "n": n}), 1);
            (r.retired, r.restricted) = (odd == Some(1), odd == Some(2));
            (r, odd == Some(3))
        })
        .collect();
    fold(engine, 1, |p| {
        first.iter().try_for_each(|(r, _)| p.put_record(r))
    })?;
    let ended = |r: StoredRecord| StoredRecord::ended(r.ty, r.key, Position(2));
    let states: Vec<StoredRecord> = (first.into_iter())
        .map(|(r, ends)| if ends { ended(r) } else { r })
        .collect();
    fold(engine, 2, |p| {
        (states.iter().filter(|r| r.is_ended())).try_for_each(|r| p.put_record(r))
    })?;
    let (ty, head) = (TypeName::from("t"), view(engine, None)?);
    // The pages of a scan, each after the last key of the one before, and
    // the rows the engine handed the rulebook for them all.
    let walk = |mut scan: Scan, what: &str| -> Held {
        let mut read = 0;
        loop {
            let before = crate::engines::rows_read();
            let page = head.scan(&ty, &scan).must(what)?;
            read += crate::engines::rows_read() - before;
            let whole = crate::engines::scan(None, &states, &scan);
            ensure!(
                keys(&page) == keys(&whole),
                "{what}, after {:?}, answered {} rows, first {:?}, last {:?}; one scan over \
                 every state answers {}, first {:?}, last {:?}",
                scan.after.as_ref().map(|a| a.key.as_str()),
                page.len(),
                page.first().map(|r| r.key.as_str()),
                page.last().map(|r| r.key.as_str()),
                whole.len(),
                whole.first().map(|r| r.key.as_str()),
                whole.last().map(|r| r.key.as_str())
            );
            let Some(last) = page.last().filter(|_| page.len() as u64 == BATCH) else {
                break;
            };
            scan.after = Some(After {
                values: vec![],
                key: last.key.clone(),
            });
        }
        ensure!(
            read <= 2 * N + BATCH,
            "{what}, walked to its end, was handed {read} rows of a type of {N}; a page reads \
             from `after` what fills it, at most a batch over, not every record holding the \
             text for each page"
        );
        Ok(())
    };
    let filter = crate::expr::parse("v = 'a' and n >= 0").expect("the check's filter parses");
    let equal = vec![("v".into(), json!("a"))];
    let scan = scan_with(|s| s.filter = Some(filter));
    let paged = |scan: Scan| Scan {
        limit: BATCH as u32,
        ..scan
    };
    walk(
        paged(scan.clone()),
        "pages by key under `v = 'a' and n >= 0`",
    )?;
    let by_equal = scan_with(|s| s.equal = equal);
    walk(paged(by_equal.clone()), "pages by key with `equal`")?;
    walk(by_equal, "a scan with `equal` and no limit")?;
    let holding = (states.iter())
        .filter(|r| r.fields.get("v") == Some(&json!("a")))
        .count() as u64;
    let selected = crate::engines::scan(None, &states, &scan).len() as u64;
    let before = crate::engines::rows_read();
    let counted = head.count(&ty, &scan).must("counting under the filter")?;
    let read = crate::engines::rows_read() - before;
    ensure!(
        counted == selected && read <= holding + BATCH,
        "a count under `v = 'a' and n >= 0` answered {counted} from {read} rows handed to the \
         rulebook; its scan selects {selected}, and {holding} records hold the text: a count \
         reads them once, at most a batch over"
    );
    Ok(())
}

fn scan_filters_exactly_as_the_evaluator_including_mixed_kinds_and_errors(
    engine: &dyn LakeEngine,
) -> Held {
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
                        meta: None,
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

fn scan_honours_retired_restricted_and_valid_at(engine: &dyn LakeEngine) -> Held {
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

fn a_count_is_the_number_of_records_its_scan_selects(engine: &dyn LakeEngine) -> Held {
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

/// The one list `ranked` answers for `asked`.
fn one_list(
    engine: &dyn LakeEngine,
    ty: &TypeName,
    asked: Rankings,
    k: u32,
) -> lakelands_api::Result<Vec<(RecordKey, f64)>> {
    Ok(engine.ranked(ty, &asked, k)?.lists.swap_remove(0))
}

/// The head's full-text hits of `text` over `ty`, in the fields named.
pub fn lexical(
    engine: &dyn LakeEngine,
    ty: &TypeName,
    text: &str,
    fields: Option<&[FieldName]>,
    k: u32,
) -> lakelands_api::Result<Vec<(RecordKey, f64)>> {
    lexical_weighed(engine, ty, text, fields, BTreeMap::new(), k)
}

/// [`lexical`], a match in each field weighed by `text_weights`.
pub fn lexical_weighed(
    engine: &dyn LakeEngine,
    ty: &TypeName,
    text: &str,
    fields: Option<&[FieldName]>,
    text_weights: BTreeMap<FieldName, f64>,
    k: u32,
) -> lakelands_api::Result<Vec<(RecordKey, f64)>> {
    let text = Some((text.to_owned(), fields.map(<[FieldName]>::to_vec)));
    let asked = Rankings {
        text,
        text_weights,
        ..Rankings::default()
    };
    one_list(engine, ty, asked, k)
}

/// The head's nearest vectors of `ty.field` to `query`.
pub fn nearest(
    engine: &dyn LakeEngine,
    ty: &TypeName,
    field: &str,
    query: &[f32],
    k: u32,
) -> lakelands_api::Result<Vec<(RecordKey, f64)>> {
    let vectors = vec![(field.to_owned(), query.to_vec())];
    one_list(
        engine,
        ty,
        Rankings {
            vectors,
            ..Rankings::default()
        },
        k,
    )
}

fn lexical_hits_are_the_pipelines_over_searchable_live_records(engine: &dyn LakeEngine) -> Held {
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
    let ty = TypeName::from("doc");
    let hits = lexical(engine, &ty, "run rivers", None, 10).must("searching")?;
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
    let one = lexical(engine, &ty, "run rivers", None, 1).must("searching for one")?;
    ensure!(
        hit_keys(&one) == ["two"],
        "searching with k = 1 answered {:?}",
        hit_keys(&one)
    );
    let none = lexical(engine, &ty, "ocean", None, 10).must("searching for nothing")?;
    ensure!(none.is_empty(), "searching `ocean` answered {none:?}");
    Ok(())
}

fn ranked_answers_every_list_from_one_state_and_says_its_position(engine: &dyn LakeEngine) -> Held {
    let (index, ty) = (vectors(engine)?, TypeName::from("doc"));
    fold(engine, 1, |p| {
        p.put_declaration(&documents(), 1, None, Position(0))?;
        p.put_record(&record("doc", "kept", json!({"body": "ocean"}), 1))?;
        p.put_record(&record("doc", "gone", json!({"body": "ocean"}), 1))?;
        index.put(&ty, &"kept".into(), "emb", &[1.0, 0.0])?;
        index.put(&ty, &"gone".into(), "emb", &[0.0, 1.0])
    })?;
    let asked = Rankings {
        text: Some(("river ocean".into(), None)),
        vectors: vec![("emb".into(), vec![0.0, 1.0]), ("none".into(), vec![1.0])],
        ..Rankings::default()
    };
    let read = || -> Result<(Option<Position>, Vec<Vec<String>>), Failure> {
        let ranked = engine.ranked(&ty, &asked, 10).must("ranking")?;
        Ok((
            ranked.position,
            ranked.lists.iter().map(|l| hit_keys(l)).collect(),
        ))
    };
    let state = |at: u64, text: [&str; 2], near: [&str; 2]| {
        let list = |keys: [&str; 2]| keys.map(str::to_owned).to_vec();
        (Some(Position(at)), vec![list(text), list(near), Vec::new()])
    };
    let before = read()?;
    ensure!(
        before == state(1, ["gone", "kept"], ["gone", "kept"]),
        "folded through 1, `river ocean`, the vectors nearest (0, 1) and a field that holds none          answered {before:?}; both records hold `ocean` (ties by key), `gone` points north, and          the position is 1"
    );
    let p = engine.projection();
    p.begin().must("beginning a transaction")?;
    let puts = || -> lakelands_api::Result<()> {
        p.put_record(&record("doc", "kept", json!({"body": "river"}), 2))?;
        let mut gone = record("doc", "gone", json!({"body": "river"}), 2);
        gone.retired = true;
        p.put_record(&gone)?;
        p.put_record(&record("doc", "new", json!({"body": "river"}), 2))?;
        index.remove(&ty, &"gone".into(), "emb")?;
        index.put(&ty, &"new".into(), "emb", &[0.0, 1.0])
    };
    puts().must("writing the projection")?;
    let open = read()?;
    ensure!(
        open == before,
        "while the transaction through 2 was open the same call answered {open:?}, not {before:?};          lists that hold a put of it beside the position 1 are of two states"
    );
    p.commit(Position(2)).must("committing through 2")?;
    let after = read()?;
    ensure!(
        after == state(2, ["kept", "new"], ["new", "kept"]),
        "folded through 2, the same call answered {after:?}; `kept` and `new` hold `river`,          `gone` is retired there and no candidate of either list, `new` points north, and the          position is 2"
    );
    let folded = p.folded_through().must("reading the folded position")?;
    ensure!(
        after.0 == folded,
        "`ranked` said the position {:?}; the projection is folded through {folded:?}",
        after.0
    );
    Ok(())
}

fn lexical_over_named_fields_is_the_pipelines_over_those_fields(engine: &dyn LakeEngine) -> Held {
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
    let title = [FieldName::from("title")];
    let body = [FieldName::from("body")];
    let all = lexical(engine, &ty, "river run", None, 10).must("searching every field")?;
    ensure!(
        hit_keys(&all) == ["both", "b-1", "b-2", "b-3", "late", "t-only"],
        "searching `river run` in every searchable field answered {:?}",
        hit_keys(&all)
    );
    let titled = lexical(engine, &ty, "river run", Some(&title), 3).must("searching `title`")?;
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
    let bodied = lexical(engine, &ty, "river run", Some(&body), 10).must("searching `body`")?;
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
    let noted = lexical(engine, &ty, "river", Some(&[FieldName::from("note")]), 10)
        .must("searching `note`")?;
    ensure!(
        noted.is_empty(),
        "searching `river` in `note`, which is not searchable, answered {:?}",
        hit_keys(&noted)
    );
    Ok(())
}

fn a_weighed_text_scores_each_token_at_its_heaviest_field_before_the_cut(
    engine: &dyn LakeEngine,
) -> Held {
    fold(engine, 1, |p| {
        p.put_declaration(&documents(), 1, None, Position(0))?;
        let rows = [
            ("both", json!({"title": "river", "body": "river"})),
            ("heavy", json!({"title": "river"})),
            ("light-a", json!({"body": "river run"})),
            ("light-b", json!({"body": "rivers running"})),
            ("noted", json!({"note": "river run"})),
        ];
        for (key, fields) in rows {
            p.put_record(&record("doc", key, fields, 1))?;
        }
        Ok(())
    })?;
    let ty = TypeName::from("doc");
    let weighed = |text: &str, fields: Option<&[FieldName]>, weights: &[(&str, f64)], k| {
        let weights = weights.iter().map(|(f, w)| (FieldName::from(*f), *w));
        lexical_weighed(engine, &ty, text, fields, weights.collect(), k)
    };
    let keyed = |hits: &[(&str, f64)]| -> Vec<(RecordKey, f64)> {
        hits.iter()
            .map(|(k, s)| (RecordKey::from(*k), *s))
            .collect()
    };
    let none = weighed("river run", None, &[], 3).must("ranking unweighed")?;
    ensure!(
        none == keyed(&[("light-a", 2.0), ("light-b", 2.0), ("both", 1.0)]),
        "unweighed, `river run` answered {none:?}: the count of the distinct tokens held, \
         `both` holding `river` in two fields counted once"
    );
    let title = weighed("river run", None, &[("title", 4.0)], 3).must("ranking by title")?;
    ensure!(
        title == keyed(&[("both", 4.0), ("heavy", 4.0), ("light-a", 2.0)]),
        "with `title` at 4, `river run` with k = 3 answered {title:?}: `both` and `heavy` hold \
         `river` in the title (4, not 4 + 1 for `both`), `light-a` and `light-b` hold both \
         tokens in `body`, which weighs 1; a list cut at k before it was weighed loses `heavy`"
    );
    let both = [("title", 4.0), ("body", 0.25)];
    let halved = weighed("river run", None, &both, 10).must("ranking by both fields")?;
    let light = 0.25 + 0.25;
    ensure!(
        halved
            == keyed(&[
                ("both", 4.0),
                ("heavy", 4.0),
                ("light-a", light),
                ("light-b", light)
            ]),
        "with `title` at 4 and `body` at 0.25, `river run` answered {halved:?}"
    );
    let bodied = [FieldName::from("body")];
    let named = weighed("river", Some(&bodied), &[("title", 4.0)], 10).must("ranking `body`")?;
    ensure!(
        named == keyed(&[("both", 1.0), ("light-a", 1.0), ("light-b", 1.0)]),
        "`river` in `body`, `title` at 4, answered {named:?}: a field not searched weighs nothing"
    );
    Ok(())
}

fn a_record_put_after_a_redeclaration_in_the_same_transaction_is_indexed_under_it(
    engine: &dyn LakeEngine,
) -> Held {
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
    let hits = lexical(engine, &TypeName::from("doc"), "river", None, 10).must("searching")?;
    ensure!(
        hit_keys(&hits) == ["early", "late"],
        "one transaction declared `doc` with an unsearchable `body`, put `early`, declared it \
         again with `body` searchable and put `late`, both with `river` in `body`; searching \
         `river` answered {:?}, but under the declaration in force both are hits",
        hit_keys(&hits)
    );
    Ok(())
}

engine_check!(
    REPLACED = what_was_replaced_since_a_position_and_what_is_in_force_hold_what_was_then,
    "seams §ProjectionView (lakelands#1064): `replaced(from, to)` answers each declaration's state that a later one replaced after `from` and by `to`, as it stood: at every position, the latest state of each name at or before it, among those in force at the head and those replaced since, is what `declarations` answers there, two ranges that meet answer what the one over both does, and a view at an earlier position answers no further than its position"
);

fn what_was_replaced_since_a_position_and_what_is_in_force_hold_what_was_then(
    engine: &dyn LakeEngine,
) -> Held {
    let ty = |name: &str, fields: &[&str]| {
        let fields: Vec<Value> = (fields.iter())
            .map(|f| json!({"name": f, "type": {"kind": "text"}}))
            .collect();
        declare(json!({"declare": "type", "name": name, "fields": fields}))
    };
    fold(engine, 7, |p| {
        p.put_declaration(&ty("a", &["x"]), 1, None, Position(1))?;
        p.put_declaration(&ty("b", &["x"]), 1, None, Position(1))?;
        p.put_declaration(&ty("a", &["x", "y"]), 2, None, Position(3))?;
        p.put_declaration(&ty("c", &["x"]), 1, None, Position(4))?;
        p.put_declaration(&ty("a", &["x", "y", "z"]), 3, None, Position(6))?;
        p.put_declaration(&ty("b", &["x", "w"]), 2, None, Position(6))
    })?;
    let shown = |d: &[Declared]| {
        let mut all: Vec<String> = d
            .iter()
            .map(|d| serde_json::to_string(d).unwrap_or_default())
            .collect();
        all.sort();
        all
    };
    let head = view(engine, None)?;
    let now = head
        .declarations()
        .must("reading the declarations at the head")?;
    let replaced = |from: u64, to: u64| -> Result<Vec<Declared>, Failure> {
        let got = head.replaced(Position(from), Position(to));
        got.must(&format!("reading what was replaced in ({from}, {to}]"))?
            .ok_or_else(|| Failure::new("the engine keeps no read of what was replaced".to_owned()))
    };
    for at in 1..=7 {
        let then = view(engine, Some(Position(at)))?.declarations();
        let then = then.must(&format!("reading the declarations at {at}"))?;
        let since = replaced(at, 7)?;
        let mut by: std::collections::BTreeMap<String, &Declared> = Default::default();
        for d in now.iter().chain(&since).filter(|d| d.since.0 <= at) {
            let k = crate::engines::declaration_key(&d.declaration);
            if by.get(&k).is_none_or(|e| e.since < d.since) {
                by.insert(k, d);
            }
        }
        let built: Vec<Declared> = by.into_values().cloned().collect();
        ensure!(
            shown(&built) == shown(&then),
            "at {at}, the head's declarations with those replaced since answer {:?}; `declarations` there answers {:?}",
            shown(&built),
            shown(&then)
        );
        // A view at `at` answers to its own position, whatever `to` asks.
        let then = view(engine, Some(Position(at)))?;
        let early = then.replaced(Position(1), Position(7));
        let early = early.must(&format!("reading what was replaced as of {at}"))?;
        ensure!(
            early.as_ref().map(|e| shown(e)) == Some(shown(&replaced(1, at)?)),
            "as of {at}, what was replaced in (1, 7] is {early:?}; it is what was replaced in (1, {at}]"
        );
        for mid in at..=7 {
            let mut parts = replaced(at, mid)?;
            parts.extend(replaced(mid, 7)?);
            ensure!(
                shown(&parts) == shown(&since),
                "what was replaced in ({at}, {mid}] and ({mid}, 7] is {:?}; in ({at}, 7] it is {:?}",
                shown(&parts),
                shown(&since)
            );
        }
    }
    Ok(())
}

engine_check!(
    CHANGED_ROWS = a_changed_record_is_found_by_what_it_holds_now_and_read_no_more,
    "seams §ProjectionStore, 025 and 041: after a record changes, the head indexes hold the rows of its latest state and no others: an exact text lookup, a search and a page in an indexed field's order answer by what it holds now, a field it cleared included, and an exact lookup hands the rulebook no record that matched only before, unless the engine keeps no index of exact text and reads the type for every lookup"
);

fn a_changed_record_is_found_by_what_it_holds_now_and_read_no_more(
    engine: &dyn LakeEngine,
) -> Held {
    let t = declare(json!({
        "declare": "type", "name": "t", "language": "en",
        "fields": [
            {"name": "a", "type": {"kind": "text"}},
            {"name": "b", "type": {"kind": "text"}},
            {"name": "same", "type": {"kind": "text"}},
            {"name": "body", "type": {"kind": "text"}, "searchable": true}
        ]
    }));
    let k1 = json!({"a": "x", "b": "y", "same": "s", "body": "river bank"});
    fold(engine, 1, |p| {
        p.put_declaration(&t, 1, None, Position(0))?;
        p.put_record(&sort_setting(json!(["a"]), 1))?;
        p.put_record(&record("t", "k1", k1, 1))?;
        p.put_record(&record(
            "t",
            "k2",
            json!({"a": "m", "b": "y", "body": "hill"}),
            1,
        ))
    })?;
    // `k1` changes `a` and a word of `body`, clears `b` and keeps `same`.
    let k1 = json!({"a": "c", "same": "s", "body": "meadow bank"});
    fold_one(engine, 2, record("t", "k1", k1, 2))?;
    let (ty, head) = (TypeName::from("t"), view(engine, None)?);
    // What an exact lookup answers, with the rows it handed the rulebook. A
    // search may hand in any superset of its hits, so its rows are not
    // counted.
    let lookup = |filter: &str| -> Result<(Vec<String>, u64), Failure> {
        let filter = crate::expr::parse(filter).expect("the check's filter parses");
        let before = crate::engines::rows_read();
        let scan = scan_with(|s| s.filter = Some(filter));
        let rows = head.scan(&ty, &scan).must("an exact lookup")?;
        Ok((keys(&rows), crate::engines::rows_read() - before))
    };
    let search = |text: &str| -> Result<(Vec<String>, u64), Failure> {
        let hits = lexical(engine, &ty, text, None, 10).must("searching")?;
        Ok((hit_keys(&hits), 0))
    };
    let answers = [
        ("a = 'x'", lookup("a = 'x'")?, vec![]),
        ("a = 'c'", lookup("a = 'c'")?, vec!["k1"]),
        ("b = 'y'", lookup("b = 'y'")?, vec!["k2"]),
        ("river", search("river")?, vec![]),
        ("meadow", search("meadow")?, vec!["k1"]),
        ("bank", search("bank")?, vec!["k1"]),
    ];
    // An engine that keeps no index of exact text reads the type, both
    // records, for every lookup.
    let whole = answers[..3].iter().all(|(_, (_, rows), _)| *rows == 2);
    for (asked, (keys, rows), must) in &answers {
        ensure!(
            keys == must && (whole || *rows <= must.len() as u64),
            "`k1` changed `a` from `x` to `c` and `river bank` to `meadow bank`, cleared its \
             `b`, which `k2` still holds as `y`, and kept `same`; `{asked}` answered {keys:?} \
             from {rows} row(s) handed to the rulebook; it must answer {must:?} from no more \
             rows than that, unless every lookup reads the type"
        );
    }
    let by_a = scan_with(|s| s.order_by = vec![order("a", OrderBy::Value, Direction::Asc)]);
    let page = keys(&head.scan(&ty, &by_a).must("a page ordered by `a`")?);
    ensure!(
        page == ["k1", "k2"],
        "`k1` changed its `a` from `x` to `c`, before `k2`'s `m`; the page ordered by `a` \
         answered {page:?}; it must be `k1` then `k2`"
    );
    Ok(())
}

engine_check!(
    MANY_PUTS = a_transaction_of_many_puts_leaves_what_putting_them_one_by_one_leaves,
    "seams §ProjectionStore: an engine may write a transaction's puts before its commit, so a type's declaration is put before any record of the type at or after its position; a transaction of more than a page of puts (records below a declaration's position, the declaration, records after it, a record put again, a state put again at its position, a membership put twice, a `sort_index.<type>` setting after its records, a `vector_index.<type>` setting in its first page and one in its last, each with vectors put before and after the page between), over keys a database's collation orders otherwise than their bytes, leaves the head indexes, the earlier states, the memberships and the nearest vectors that putting them one by one leaves; a view opened in it after more than a page of its puts answers what was committed before it; one refused at its commit leaves nothing; and a type that holds an ended key is indexed whole when it is declared again; and the states of two records put record by record, below a declaration's position and at it, after the declaration, leave what position order leaves"
);

fn a_transaction_of_many_puts_leaves_what_putting_them_one_by_one_leaves(
    engine: &dyn LakeEngine,
) -> Held {
    const N: u64 = 2_100;
    let t = |searchable: bool| {
        let under = json!([{"name": "under", "target": "t", "via": "up"}]);
        declare(json!({
            "declare": "type", "name": "t", "language": "en",
            "fields": [{"name": "body", "type": {"kind": "text"}, "searchable": searchable},
                       {"name": "v", "type": {"kind": "text"}},
                       {"name": "up", "type": {"kind": "reference", "target": "t"}}],
            "relations": if searchable { under } else { json!([]) }
        }))
    };
    // Keys in three runs, which the bytes order `B`, `_`, `a` and a
    // database's collation otherwise; the first and the last two are each
    // more than a page, so a pass over the type by pages ends a page inside
    // a run in either order.
    let run = |i: u64| ["B", "_", "a"][usize::from(i >= 1_040) + usize::from(i >= 1_070)];
    let (ty, key) = (TypeName::from("t"), |i: u64| format!("{}{i:04}", run(i)));
    let state = |i: u64, at: u64| {
        let fields = json!({"body": "river", "v": format!("{i:04}"), "up": key(0)});
        record("t", &key(i), fields, at)
    };
    let member = |p: &dyn ProjectionStore, is: bool, at: u64| {
        p.put_membership("s", &ty, &RecordKey::from(key(2)), is, Position(at))
    };
    // Two types with a vector field: `d`'s `vector_index` setting goes into
    // the transaction's first page and `e`'s into its last, each with
    // vectors put before and after the page between them.
    let index = vectors(engine)?;
    let vector = |p: &dyn ProjectionStore, ty: &str, key: &str, v: [f32; 2], at: u64| {
        p.put_record(&record(ty, key, json!({"emb": v}), at))?;
        index.put(&ty.into(), &key.into(), "emb", &v)
    };
    let asks = |ty: &str, at: u64| {
        let key = format!("vector_index.{ty}");
        let fields = json!({"key": key, "value": ["emb"]});
        record(lakelands_api::seam::SETTING_TYPE, &key, fields, at)
    };
    // Folded through 1 with nothing put, so a view opened inside the next
    // transaction has a position and reads; what it reads is asked there.
    fold(engine, 1, |_| Ok(()))?;
    let mut open = None;
    fold(engine, 3, |p| {
        for ty in ["d", "e"] {
            let emb = json!([{"name": "emb", "type": {"kind": "vector", "dimensions": 2}}]);
            let decl = declare(json!({"declare": "type", "name": ty, "fields": emb}));
            p.put_declaration(&decl, 1, None, Position(1))?;
            vector(p, ty, "east", [1.0, 0.0], 1)?;
            vector(p, ty, "north", [0.0, 1.0], 1)?;
        }
        p.put_record(&asks("d", 1))?;
        p.put_declaration(&t(false), 1, None, Position(1))?;
        member(p, true, 1)?;
        (0..N).try_for_each(|i| p.put_record(&state(i, 1)))?;
        let (during, third) = (engine.view(None)?, RecordKey::from(key(3)));
        let holds = crate::expr::parse("v = '0003'").expect("the check's filter parses");
        open = Some((
            during.get(&ty, &third)?.map(|r| r.key),
            keys(&during.scan(&ty, &scan_with(|s| s.filter = Some(holds)))?),
            during.members("s", &ty)?,
        ));
        drop(during);
        // A state put again at its position, a page later, replaces the first.
        p.put_record(&record("t", &key(1), json!({"body": "again"}), 1))?;
        p.put_declaration(&t(true), 2, None, Position(2))?;
        p.put_record(&record(
            "t",
            &key(0),
            json!({"body": "hill", "v": "zzzz"}),
            3,
        ))?;
        (N..N + 10).try_for_each(|i| p.put_record(&state(i, 3)))?;
        member(p, false, 3)?;
        vector(p, "d", "north-east", [1.0, 1.0], 3)?;
        vector(p, "e", "north-east", [1.0, 1.0], 3)?;
        p.put_record(&asks("e", 3))?;
        p.put_record(&sort_setting(json!(["v"]), 3))
    })?;
    ensure!(
        open == Some((None, vec![], Default::default())),
        "a view opened in a transaction after more than a page of its puts read {open:?}: \
         `{}`, the records holding `v = '0003'` and the members of `s`; nothing of them is \
         committed, so it must answer none, none and none",
        key(3)
    );
    // What the head answers: the search, a relation's edges, an exact
    // lookup, a page in the indexed order.
    let answers = |when: &str, rivers: u64| -> Held {
        let head = view(engine, None)?;
        let found = |text: &str| lexical(engine, &ty, text, None, 5_000).map(|h| hit_keys(&h));
        let (river, hill) = (
            found("river").must("searching")?,
            found("hill").must("searching")?,
        );
        let from = [RecordKey::from(key(5))];
        let out = lakelands_api::query::Walk::Out;
        let edges = head.edges(&ty, &from, None, out).must("reading edges")?;
        let exact = crate::expr::parse("v = '1099'").expect("the check's filter parses");
        let exact = head.scan(&ty, &scan_with(|s| s.filter = Some(exact)));
        let largest = [key(0), key(N + 9), key(N + 8)];
        let by_v = scan_with(|s| {
            s.order_by = vec![order("v", OrderBy::Value, Direction::Desc)];
            s.limit = 3;
        });
        let page = keys(&head.scan(&ty, &by_v).must("a page ordered by `v`")?);
        ensure!(
            river.len() as u64 == rivers
                && hill == [key(0)]
                && edges.first().is_some_and(|e| e.len() == 1)
                && keys(&exact.must("an exact lookup")?) == [key(1099)]
                && page == largest,
            "{when}: `river` is found in {} records ({rivers} hold it), `hill` in {hill:?} \
             (`{}`); `{}` has {:?} edge(s) (one, to `{}`); the three largest `v` are {page:?} \
             ({largest:?})",
            river.len(),
            key(0),
            key(5),
            edges.first().map(Vec::len),
            key(0)
        );
        for vectors in ["d", "e"] {
            let near = nearest(engine, &vectors.into(), "emb", &[2.0, 0.0], 3);
            let near = hit_keys(&near.must("searching the vectors")?);
            ensure!(
                near == ["east", "north-east", "north"],
                "{when}: the nearest three of `{vectors}` to (2, 0) are {near:?} (`east`, \
                 `north-east`, `north`)"
            );
        }
        Ok(())
    };
    answers("after one transaction of more than a page of puts", N + 8)?;
    let (at_one, head) = (view(engine, Some(Position(1)))?, view(engine, None)?);
    let body = |v: &dyn ProjectionView, k: u64| -> Result<Option<Value>, Failure> {
        let r = v
            .get(&ty, &RecordKey::from(key(k)))
            .must("reading a record")?;
        Ok(r.and_then(|r| r.fields.get("body").cloned()))
    };
    let members = |v: &dyn ProjectionView| v.members("s", &ty).map(|m| m.len());
    ensure!(
        body(at_one.as_ref(), 0)? == Some(json!("river"))
            && body(at_one.as_ref(), 1)? == Some(json!("again"))
            && body(head.as_ref(), 0)? == Some(json!("hill"))
            && members(at_one.as_ref()).must("reading the members")? == 1
            && members(head.as_ref()).must("reading the members")? == 0,
        "as of 1 `{0}` holds `river` and `{1}` the state put again there; at the head `{0}` \
         holds `hill`; `{2}` is a member as of 1 and none at the head",
        key(0),
        key(1),
        key(2)
    );
    drop((at_one, head));
    // An ended key, then the type declared again: every record is indexed
    // again, the ended key passed over.
    fold_one(
        engine,
        4,
        StoredRecord::ended(ty.clone(), key(7).as_str().into(), Position(4)),
    )?;
    fold(engine, 5, |p| {
        p.put_declaration(&t(true), 3, None, Position(5))
    })?;
    answers("after an ended key and a redeclaration", N + 7)?;
    // A transaction refused at its commit leaves nothing of its pages, seen
    // once the projection is folded past the position they were put at.
    let p = engine.projection();
    p.begin().must("beginning a transaction")?;
    (0..N)
        .try_for_each(|i| p.put_record(&record("t", &format!("r{i:04}"), json!({}), 9)))
        .must("putting more than a page of records")?;
    let late = p.commit(Position(6));
    fold(engine, 9, |_| Ok(()))?;
    let left = view(engine, None)?.get(&ty, &RecordKey::from("r0000"));
    ensure!(
        crate::conformance::fixture::is_invalid(&late) && left.must("reading")?.is_none(),
        "a commit through 6 of states valid from 9 answered {late:?}; it must be refused, \
         and none of its {N} records stay"
    );
    // A declaration given first, then the states of two records, record by
    // record, below its position and at it: what position order leaves.
    let o = |searchable: bool| {
        let body = json!({"name": "body", "type": {"kind": "text"}, "searchable": searchable});
        declare(json!({"declare": "type", "name": "o", "language": "en", "fields": [body]}))
    };
    let state = |key: &str, body: &str, at: u64| record("o", key, json!({"body": body}), at);
    fold(engine, 10, |p| {
        p.put_declaration(&o(false), 1, None, Position(10))?;
        p.put_record(&state("a", "river", 10))?;
        p.put_record(&state("b", "river", 10))
    })?;
    fold(engine, 12, |p| {
        p.put_declaration(&o(true), 2, None, Position(12))?;
        p.put_record(&state("a", "hill", 11))?;
        p.put_record(&state("a", "meadow", 12))?;
        p.put_record(&state("b", "hill", 11))
    })?;
    let o = TypeName::from("o");
    let found = |text: &str| lexical(engine, &o, text, None, 10).map(|h| hit_keys(&h));
    let (meadow, hill, river) = (
        found("meadow").must("searching")?,
        found("hill").must("searching")?,
        found("river").must("searching")?,
    );
    let then = |at: u64, key: &str| -> Result<Option<Value>, Failure> {
        let v = view(engine, Some(Position(at)))?;
        let r = v.get(&o, &RecordKey::from(key)).must("reading a record")?;
        Ok(r.and_then(|r| r.fields.get("body").cloned()))
    };
    ensure!(
        meadow == ["a"]
            && hill == ["b"]
            && river.is_empty()
            && then(10, "a")? == Some(json!("river"))
            && then(11, "a")? == Some(json!("hill"))
            && then(12, "b")? == Some(json!("hill")),
        "`o` was declared searchable at 12 before `a` was put at 11 and 12 and `b` at 11, in \
         that order; `meadow` is found in {meadow:?} (`a`), `hill` in {hill:?} (`b`, whose \
         head state lies below the declaration), `river` in {river:?} (none); as of 10, 11 \
         and 12 a record holds the state put there"
    );
    Ok(())
}

fn vectors(engine: &dyn LakeEngine) -> Result<&dyn lakelands_api::engine::VectorIndex, Failure> {
    engine
        .vectors()
        .ok_or_else(|| Failure::new("the engine keeps no vector index"))
}

fn nearest_ranks_by_cosine_with_ties_by_key(engine: &dyn LakeEngine) -> Held {
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
    let hits = nearest(engine, &ty, "emb", &[2.0, 0.0], 4).must("searching the index")?;
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
    let none = nearest(engine, &ty, "other", &[1.0, 0.0], 4).must("searching an empty field")?;
    ensure!(
        none.is_empty(),
        "a field holding no vectors answered {none:?}"
    );
    Ok(())
}

fn a_zero_vector_scores_zero_and_a_wrong_length_is_invalid(engine: &dyn LakeEngine) -> Held {
    let index = vectors(engine)?;
    let ty = TypeName::from("doc");
    index
        .put(&ty, &"a".into(), "emb", &[1.0, 2.0])
        .must("putting a vector")?;
    index
        .put(&ty, &"zero".into(), "emb", &[0.0, 0.0])
        .must("putting a zero vector")?;
    let hits = nearest(engine, &ty, "emb", &[0.0, 0.0], 5).must("searching with a zero vector")?;
    ensure!(
        hits.len() == 2 && hits.iter().all(|(_, s)| *s == 0.0),
        "a zero query answered {hits:?}; it scores 0 against everything"
    );
    let wrong = nearest(engine, &ty, "emb", &[1.0, 0.0, 0.0], 5);
    ensure!(
        matches!(reason(&wrong), Some(Refusal::Invalid(..))),
        "a query of three dimensions against vectors of two answered {wrong:?}; it must be \
         refused as invalid"
    );
    Ok(())
}

fn put_replaces_a_vector_and_remove_forgets_it(engine: &dyn LakeEngine) -> Held {
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
    let best = nearest(engine, &ty, "emb", &[0.0, 1.0], 2).must("searching")?;
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

fn put_and_remove_answer_alike_over_a_vector_a_state_holds(engine: &dyn LakeEngine) -> Held {
    let index = vectors(engine)?;
    let decl = declare(json!({
        "declare": "type", "name": "t", "fields": [
            {"name": "v", "type": {"kind": "vector", "dimensions": 2}},
            {"name": "j", "type": {"kind": "json"}}
        ]
    }));
    let ty = TypeName::from("t");
    let (east, north) = ([0.5, 0.25], [0.25, 0.5]);
    // Each state is followed by the puts the store's fold makes for it.
    fold(engine, 1, |p| {
        p.put_declaration(&decl, 1, None, Position(0))?;
        p.put_record(&record("t", "a", json!({"v": east, "j": east}), 1))?;
        index.put(&ty, &"a".into(), "v", &east)?;
        index.put(&ty, &"a".into(), "j", &east)?;
        p.put_record(&record("t", "b", json!({"v": east}), 1))?;
        index.put(&ty, &"b".into(), "v", &east)?;
        index.put(&ty, &"b".into(), "v", &north)?;
        p.put_record(&record("t", "c", json!({"v": east}), 1))?;
        index.put(&ty, &"c".into(), "v", &east)?;
        index.remove(&ty, &"c".into(), "v")
    })?;
    let counts = (index.count(&ty, "v"), index.count(&ty, "j"));
    let counts = (counts.0.must("counting")?, counts.1.must("counting")?);
    ensure!(
        counts == (2, 1),
        "in one transaction `a`, `b` and `c` were put with v, `a` with j too, each followed by \
         its puts, then `b` was put with other elements and `c` removed; v and j count \
         {counts:?}, but they hold (2, 1)"
    );
    index
        .put(&ty, &"a".into(), "v", &north)
        .must("replacing a vector")?;
    let hits = nearest(engine, &ty, "v", &north, 3).must("searching")?;
    ensure!(
        hits.iter().all(|(_, s)| (s - 1.0).abs() < 1e-9) && hit_keys(&hits) == ["a", "b"],
        "`a`'s v was replaced by {north:?} after its state and `b`'s in the state's \
         transaction; the nearest to it answered {hits:?}, but both score 1"
    );
    index
        .remove(&ty, &"a".into(), "v")
        .must("removing a vector")?;
    let count = index.count(&ty, "v").must("counting")?;
    ensure!(
        count == 1,
        "after removing `a`'s v, which its state held, the field counts {count}; it holds one"
    );
    Ok(())
}

fn declarations_membership_outputs_and_applied_ids_read_at_an_earlier_position(
    engine: &dyn LakeEngine,
) -> Held {
    let v1 = declare(json!({"declare": "type", "name": "t", "fields": []}));
    let v2 = declare(json!({
        "declare": "type", "name": "t", "fields": [{"name": "n", "type": {"kind": "number"}}]
    }));
    let key = RecordKey::from("k");
    let ty = TypeName::from("t");
    let (seen, early) = (EventId("seen".into()), EventId("early".into()));
    fold(engine, 1, |p| {
        p.put_declaration(&v1, 1, None, Position(1))?;
        p.put_membership("big", &ty, &key, true, Position(1))?;
        p.mark_applied(&unrouted(&early, 1))
    })?;
    fold(engine, 3, |p| {
        p.put_declaration(&v2, 2, None, Position(3))?;
        p.put_membership("big", &ty, &key, false, Position(3))?;
        p.put_membership("big", &ty, &"stay".into(), true, Position(3))?;
        p.put_membership("big", &"u".into(), &"x".into(), true, Position(3))?;
        p.put_membership("small", &ty, &"y".into(), true, Position(3))?;
        p.put_output("fn", &Digest::zero(), &json!(42), Position(3))?;
        p.mark_applied(&unrouted(&seen, 3))
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
        out_then.is_none() && out_now == Some((Position(3), json!(42))),
        "an output recorded at 3 read {out_then:?} at 2 and {out_now:?} at the head, which \
         answers it with the position 3"
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
    // Many ids in one call: an answer an id, in their order, twice for an
    // id asked twice, none for an id never applied.
    let ids = [seen.clone(), EventId("never".into()), early, seen];
    let (many_then, many_now) = (
        then.applied_many(&ids).must("reading many ids at 2")?,
        now.applied_many(&ids)
            .must("reading many ids at the head")?,
    );
    let (p1, p3) = (Some(Position(1)), Some(Position(3)));
    ensure!(
        many_then == [None, None, p1, None] && many_now == [p3, None, p1, p3],
        "ids applied at 3, never and at 1, the first asked again, read {many_then:?} at 2 and \
         {many_now:?} at the head"
    );
    let none = now.applied_many(&[]).must("reading no id")?;
    ensure!(none.is_empty(), "no id asked answered {none:?}");
    Ok(())
}

/// The event `id` applied at `at`, a checkpoint about nothing.
fn unrouted(id: &EventId, at: u64) -> lakelands_api::engine::Applied {
    lakelands_api::engine::Applied {
        id: id.clone(),
        at: Position(at),
        kind: "checkpoint",
        about: vec![lakelands_api::engine::About {
            ty: None,
            key: None,
        }],
        source: None,
    }
}

fn the_index_answers_a_filters_positions_in_order_to_its_through(engine: &dyn LakeEngine) -> Held {
    use lakelands_api::engine::{About, Applied};
    use lakelands_api::seam::FeedFilter;
    let about = |ty: Option<&str>, key: Option<&str>| About {
        ty: ty.map(Into::into),
        key: key.map(Into::into),
    };
    let events: [(u64, &'static str, Vec<About>); 9] = [
        (
            1,
            "record_upserted",
            vec![about(Some("invoice"), Some("A"))],
        ),
        (
            2,
            "record_upserted",
            vec![about(Some("invoice"), Some("B"))],
        ),
        (3, "trigger_fired", vec![about(Some("invoice"), Some("A"))]),
        (4, "verb_called", vec![about(None, Some("A"))]),
        (
            5,
            "migrated",
            ["invoice", "sys.link.invoice.x", "sys.link.invoice.y"]
                .map(|t| about(Some(t), None))
                .to_vec(),
        ),
        (6, "checkpoint", vec![about(None, None)]),
        (7, "record_upserted", vec![about(Some("memo"), Some("A"))]),
        (
            8,
            "record_upserted",
            vec![about(Some("invoice"), Some("A"))],
        ),
        (
            9,
            "migrated",
            ["invoice", "memo", "note"]
                .map(|t| about(Some(t), None))
                .to_vec(),
        ),
    ];
    let applied = |(at, kind, about): &(u64, &'static str, Vec<About>)| Applied {
        id: EventId(format!("e{at}")),
        at: Position(*at),
        kind,
        about: about.clone(),
        source: None,
    };
    let (first, rest) = events.split_at(5);
    fold(engine, 5, |p| {
        first.iter().try_for_each(|e| p.mark_applied(&applied(e)))
    })?;
    fold(engine, 9, |p| {
        rest.iter().try_for_each(|e| p.mark_applied(&applied(e)))
    })?;
    let f = |kinds: &[&str], types: &[&str], keys: &[&str]| FeedFilter {
        kinds: kinds.iter().map(|k| k.to_string()).collect(),
        types: types.iter().map(|&t| t.into()).collect(),
        keys: keys.iter().map(|&k| k.into()).collect(),
    };
    let (head, then) = (view(engine, None)?, view(engine, Some(Position(5)))?);
    let ask = |v: &dyn lakelands_api::engine::ProjectionView, f: &FeedFilter, from, to, n| {
        let m = v.matching(f, Position(from), Position(to), n);
        let m = m.must(&format!(
            "asking the index for {f:?} from {from} through {to}"
        ))?;
        Ok::<_, Failure>((
            m.positions.iter().map(|p| p.0).collect::<Vec<_>>(),
            m.through.map(|p| p.0),
        ))
    };
    let invoice = f(&[], &["invoice"], &[]);
    let rows: [(FeedFilter, Vec<u64>); 9] = [
        (invoice.clone(), vec![1, 2, 3, 5, 8]),
        (f(&[], &[], &["A"]), vec![1, 3, 4, 7, 8]),
        (f(&[], &["invoice"], &["A"]), vec![1, 3, 8]),
        (f(&["record_upserted"], &["invoice"], &[]), vec![1, 2, 8]),
        (f(&[], &["sys.link.invoice.y"], &[]), vec![5]),
        (f(&["checkpoint"], &[], &[]), vec![6]),
        (f(&["checkpoint"], &["memo"], &[]), vec![]),
        (f(&[], &["invoice", "memo"], &[]), vec![1, 2, 3, 5, 7, 8]),
        (
            f(&["migrated", "record_upserted"], &[], &[]),
            vec![1, 2, 5, 7, 8],
        ),
    ];
    for (filter, want) in rows {
        let got = ask(head.as_ref(), &filter, 0, 8, 100)?;
        ensure!(
            got == (want.clone(), Some(8)),
            "the index answered {got:?} for {filter:?} from 0 through 8; it answers {want:?} \
             through 8"
        );
    }
    let paged = [
        ask(head.as_ref(), &invoice, 0, 8, 2)?,
        ask(head.as_ref(), &invoice, 3, 8, 2)?,
        ask(head.as_ref(), &invoice, 6, 8, 2)?,
        ask(head.as_ref(), &invoice, 2, 4, 9)?,
    ];
    let want = [
        (vec![1, 2], Some(2)),
        (vec![3, 5], Some(5)),
        (vec![8], Some(8)),
        (vec![2, 3], Some(4)),
    ];
    ensure!(
        paged == want,
        "the index answered {paged:?} for `invoice` with limits of 2 from 0, 3 and 6, and from \
         2 through 4; it answers {want:?}: at most the limit, through the last when it is \
         reached and through the range asked when it is not"
    );
    let earlier = ask(then.as_ref(), &invoice, 0, 8, 100)?;
    ensure!(
        earlier == (vec![1, 2, 3, 5], Some(5)),
        "a view at 5 answered {earlier:?} for `invoice` through 8; nothing past its position, \
         and through it"
    );
    let migrated = ask(head.as_ref(), &f(&["migrated"], &[], &[]), 0, 9, 2)?;
    ensure!(
        migrated == (vec![5, 9], Some(9)),
        "the index answered {migrated:?} for `migrated` with a limit of 2; it answers [5, 9] \
         through 9: an event about three types is one position"
    );
    // A view taken before a rebuild's `clear` decides only what the refold
    // has reached, and nothing where it has not reached the range.
    let before = view(engine, None)?;
    engine
        .projection()
        .clear()
        .must("clearing the projection")?;
    fold(engine, 3, |p| {
        first[..3]
            .iter()
            .try_for_each(|e| p.mark_applied(&applied(e)))
    })?;
    let rebuilt = [
        ask(before.as_ref(), &invoice, 0, 9, 100)?,
        ask(before.as_ref(), &invoice, 4, 9, 100)?,
    ];
    ensure!(
        rebuilt == [(vec![1, 2, 3], Some(3)), (vec![], None)],
        "a view at 9 taken before a clear and a refold through 3 answered {rebuilt:?} for \
         `invoice` from 0 and from 4; it answers [1, 2, 3] through 3, and decides nothing"
    );
    Ok(())
}

fn the_index_answers_a_sources_positions_in_order_to_its_through(engine: &dyn LakeEngine) -> Held {
    use lakelands_api::engine::{About, Applied};
    let about = |ty: &str| About {
        ty: Some(ty.into()),
        key: Some("A".into()),
    };
    let events: [(u64, Option<&str>, Vec<About>); 6] = [
        (1, Some("s1"), vec![about("invoice")]),
        (2, Some("s2"), vec![about("invoice")]),
        (3, None, vec![about("invoice")]),
        (4, Some("s1"), vec![about("invoice"), about("memo")]),
        (5, Some("s2"), vec![about("memo")]),
        (6, Some("s1"), vec![about("memo")]),
    ];
    let applied = |(at, source, about): &(u64, Option<&str>, Vec<About>)| Applied {
        id: EventId(format!("e{at}")),
        at: Position(*at),
        kind: "record_ingested",
        about: about.clone(),
        source: source.map(Into::into),
    };
    let marked = |p: &dyn lakelands_api::engine::ProjectionStore, from: usize, to: usize| {
        events[from..to]
            .iter()
            .try_for_each(|e| p.mark_applied(&applied(e)))
    };
    fold(engine, 4, |p| marked(p, 0, 4))?;
    fold(engine, 6, |p| marked(p, 4, 6))?;
    let ask = |v: &dyn lakelands_api::engine::ProjectionView, source: &str, from, to, n| {
        let m = v.of_source(&source.into(), Position(from), Position(to), n);
        let m = m.must(&format!(
            "asking the index for {source} from {from} through {to}"
        ))?;
        Ok::<_, Failure>((
            m.positions.iter().map(|p| p.0).collect::<Vec<_>>(),
            m.through.map(|p| p.0),
        ))
    };
    let (head, then) = (view(engine, None)?, view(engine, Some(Position(4)))?);
    let got = [
        ask(head.as_ref(), "s1", 0, 6, 100)?,
        ask(head.as_ref(), "s2", 0, 6, 100)?,
        ask(head.as_ref(), "s3", 0, 6, 100)?,
        ask(head.as_ref(), "s1", 0, 6, 2)?,
        ask(head.as_ref(), "s1", 5, 6, 2)?,
        ask(head.as_ref(), "s1", 2, 3, 9)?,
        ask(then.as_ref(), "s1", 0, 6, 100)?,
    ];
    let want = [
        (vec![1, 4, 6], Some(6)),
        (vec![2, 5], Some(6)),
        (vec![], Some(6)),
        (vec![1, 4], Some(4)),
        (vec![6], Some(6)),
        (vec![], Some(3)),
        (vec![1, 4], Some(4)),
    ];
    ensure!(
        got == want,
        "the index answered {got:?} for s1, s2 and s3 through 6, s1 with a limit of 2 from 0 \
         and from 5, s1 from 2 through 3, and s1 at a view at 4; it answers {want:?}"
    );
    // A view taken before a rebuild's `clear` decides only what the refold
    // has reached.
    let before = view(engine, None)?;
    (engine.projection().clear()).must("clearing the projection")?;
    fold(engine, 2, |p| marked(p, 0, 2))?;
    let rebuilt = [
        ask(before.as_ref(), "s1", 0, 6, 100)?,
        ask(before.as_ref(), "s1", 3, 6, 100)?,
    ];
    ensure!(
        rebuilt == [(vec![1], Some(2)), (vec![], None)],
        "a view at 6 taken before a clear and a refold through 2 answered {rebuilt:?} for s1 \
         from 0 and from 3; it answers [1] through 2, and decides nothing"
    );
    Ok(())
}

fn the_first_applied_position_is_kept(engine: &dyn LakeEngine) -> Held {
    let id = EventId("once".into());
    fold(engine, 2, |p| p.mark_applied(&unrouted(&id, 2)))?;
    fold(engine, 5, |p| p.mark_applied(&unrouted(&id, 5)))?;
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
        write: Some(write),
        why: "review".into(),
        state,
        decided_by: None,
        reason: None,
        more: false,
    }
}

fn proposals_come_newest_first_a_page_at_a_time(engine: &dyn LakeEngine) -> Held {
    use ProposalState::{Accepted, Pending, Rejected};
    let retire = |ty: &str| ProposedWrite::Retire {
        ty: ty.into(),
        key: "k".into(),
        opts: Default::default(),
    };
    let write = |n: u64| {
        retire(if n.is_multiple_of(2) {
            "invoice"
        } else {
            "customer"
        })
    };
    fold(engine, 6, |p| {
        for n in 1..=6 {
            p.put_proposal(
                &proposal(&format!("p{n}"), n, write(n), Pending),
                Position(n),
            )?;
        }
        Ok(())
    })?;
    fold(engine, 8, |p| {
        p.put_proposal(&proposal("p2", 2, write(2), Accepted), Position(7))?;
        p.put_proposal(&proposal("p5", 5, write(5), Rejected), Position(8))
    })?;
    for at in [None, Some(Position(6)), Some(Position(7))] {
        let v = view(engine, at)?;
        for state in [None, Some(Pending), Some(Accepted), Some(Rejected)] {
            let filter = ProposalFilter {
                state,
                ..Default::default()
            };
            let ids =
                |ps: Vec<Proposal>| ps.into_iter().map(|p| p.id.to_string()).collect::<Vec<_>>();
            let whole = ids(v.proposals(&filter).must("listing proposals")?);
            for size in [1, 2, 4] {
                // At most a page more than the proposals: an engine that never
                // ends its pages fails here, not by hanging.
                let (mut paged, mut before, mut ended) = (Vec::new(), None, false);
                for _ in 0..=whole.len() {
                    let asked = ProposalFilter {
                        limit: Some(size),
                        ..filter.clone()
                    };
                    let page = v.proposals_before(&asked, before).must("reading a page")?;
                    let Some(page) = page else {
                        return Err(Failure::new(
                            "the engine keeps no page of proposals".to_owned(),
                        ));
                    };
                    ensure!(
                        page.len() <= size as usize,
                        "a page of {size} held {}",
                        page.len()
                    );
                    let last = page.last().map(|p| p.position);
                    let full = page.len() == size as usize;
                    paged.extend(ids(page));
                    match last {
                        Some(last) if full => before = Some(last),
                        _ => {
                            ended = true;
                            break;
                        }
                    }
                }
                ensure!(
                    ended && paged == whole,
                    "at {at:?}, state {state:?}, pages of {size} answered {paged:?}; the whole listing is {whole:?}"
                );
            }
        }
    }
    Ok(())
}

fn proposals_filter_by_state_and_type_newest_first(engine: &dyn LakeEngine) -> Held {
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

fn lexical_finds_every_token_the_pipeline_makes_beyond_ascii(engine: &dyn LakeEngine) -> Held {
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
    let ty = TypeName::from("note");
    for (query, expected) in [
        ("ωμέγα", vec!["greek"]),
        ("CAFÉ", vec!["greek"]),
        ("мир", vec!["cyrillic"]),
        ("नमस्ते", vec!["devanagari"]),
        ("★★★", vec![]),
        ("hello мир ωμέγα", vec!["ascii", "cyrillic", "greek"]),
    ] {
        let hits = lexical(engine, &ty, query, None, 10).must(&format!("searching `{query}`"))?;
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
) -> Held {
    // A seam's name and the store's own, under which the fold keeps the
    // lake's renames: an engine keeps both alike.
    let (failed, answer) = (json!({"error": "timed out"}), json!([1.0, 0.0]));
    for (through, output) in [(2, &failed), (4, &answer)] {
        fold(engine, through, |p| {
            p.put_output("embed:m", &Digest::zero(), output, Position(through))?;
            p.put_output("sys:renames", &Digest::zero(), output, Position(through))
        })?;
    }
    for seam in ["embed:m", "sys:renames"] {
        type Read = Option<(Position, serde_json::Value)>;
        let at = |position: Option<Position>| -> Result<Read, Failure> {
            view(engine, position)?
                .output(seam, &Digest::zero())
                .must("reading an output")
        };
        let (before, between, after) = (at(Some(Position(1)))?, at(Some(Position(3)))?, at(None)?);
        ensure!(
            before.is_none()
                && between == Some((Position(2), failed.clone()))
                && after == Some((Position(4), answer.clone())),
            "under `{seam}`, a failure recorded at 2 and an answer for the same call at 4: at 1 \
             the view must answer nothing ({before:?}), at 3 the failure ({between:?}), at the \
             head the answer ({after:?})"
        );
    }
    Ok(())
}

fn edges_over_a_link_relation_come_from_the_live_link_records_at_the_views_position(
    engine: &dyn LakeEngine,
) -> Held {
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

fn a_field_ordered_scan_walks_page_by_page_as_one_scan(engine: &dyn LakeEngine) -> Held {
    const N: u64 = 160;
    // Twenty-one more keys are first put at position 2, among the rows of
    // every order: a page at position 1 passes them over.
    const NEW: u64 = 21;
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
        let at = if i >= N { at.max(2) } else { at };
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
    // in the transaction; the setting comes between them, so the records
    // put after it in the same transaction are indexed under it.
    fold(engine, 2, |p| {
        (0..N)
            .filter(|i| i.is_multiple_of(4) || i.is_multiple_of(10))
            .try_for_each(|i| {
                if i == N / 2 {
                    p.put_record(&sort_setting(json!(["v", "w", "x"]), 2))?;
                }
                let mut again = state(i, 1);
                again.position = Position(2);
                p.put_record(&again)?;
                p.put_record(&state(i, 2))
            })?;
        (N..N + NEW).try_for_each(|i| p.put_record(&state(i, 2)))
    })?;
    let after_the_setting = "at the head, before the type is declared again";
    walk_field_orders(engine, None, &two, 2, N + NEW, &state, after_the_setting)?;
    fold(engine, 3, |p| {
        p.put_declaration(&three, 2, None, Position(3))
    })?;
    walk_field_orders(
        engine,
        Some(Position(1)),
        &two,
        1,
        N + NEW,
        &state,
        "at position 1",
    )?;
    walk_field_orders(engine, None, &three, 2, N + NEW, &state, "at the head")?;
    fold(engine, 4, |p| {
        p.put_record(&sort_setting(json!(["w", "x"]), 4))
    })?;
    walk_field_orders(
        engine,
        None,
        &three,
        2,
        N + NEW,
        &state,
        "at the head, the setting without `v`",
    )
}

engine_check!(
    SETTINGS_ORDERED = a_sort_setting_for_the_settings_type_orders_the_settings_put_after_it,
    "041 and seams §ProjectionStore: a `sort_index.<type>` setting takes effect for the records put after it in the same transaction, the settings type's own included: a setting put before it and one put after it are both in the order index"
);

fn a_sort_setting_for_the_settings_type_orders_the_settings_put_after_it(
    engine: &dyn LakeEngine,
) -> Held {
    use lakelands_api::seam::SETTING_TYPE;
    let settings = declare(json!({
        "declare": "type", "name": SETTING_TYPE,
        "fields": [{"name": "key", "type": {"kind": "text"}},
                   {"name": "value", "type": {"kind": "json"}}]
    }));
    let setting =
        |key: &str, value: Value| record(SETTING_TYPE, key, json!({"key": key, "value": value}), 1);
    fold(engine, 1, |p| {
        p.put_declaration(&settings, 1, None, Position(0))?;
        p.put_record(&setting("a.one", json!(1)))?;
        p.put_record(&setting("sort_index.sys.setting", json!(["key"])))?;
        p.put_record(&setting("a.two", json!(2)))
    })?;
    let by_key = scan_with(|s| s.order_by = vec![order("key", OrderBy::Value, Direction::Desc)]);
    let page = view(engine, None)?
        .scan(&TypeName::from(SETTING_TYPE), &by_key)
        .must("a page of the settings ordered by `key`")?;
    ensure!(
        keys(&page) == ["sort_index.sys.setting", "a.two", "a.one"],
        "one transaction put the setting `a.one`, then `sort_index.sys.setting` asking for \
         `key`, then `a.two`; the settings by `key`, descending, came as {:?}",
        keys(&page)
    );
    Ok(())
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
) -> Held {
    let ty = TypeName::from("t");
    let by = |field: &str, dir| order(field, OrderBy::Value, dir);
    let filters =
        ["w != 'b'", "w = 'a'"].map(|f| crate::expr::parse(f).expect("the check's filter parses"));
    let Declaration::Type(decl) = decl else {
        unreachable!("the check declares types")
    };
    // A record first put after `pos` is not among its states there.
    let states: Vec<StoredRecord> = (0..n)
        .map(|i| state(i, pos))
        .filter(|r| r.position.0 <= pos)
        .collect();
    let view_at = view(engine, at)?;
    for order_by in [
        vec![by("v", Direction::Asc)],
        vec![by("v", Direction::Desc)],
        vec![by("w", Direction::Asc), by("v", Direction::Desc)],
        vec![by("x", Direction::Desc), by("w", Direction::Asc)],
        vec![by("x", Direction::Asc)],
    ] {
        for filtered in [None, Some(&filters[0]), Some(&filters[1])] {
            for flags in [false, true] {
                for limit in [1, 7, 50] {
                    let scan = Scan {
                        filter: filtered.cloned(),
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
                        "walking {order_by:?} {when} (filter {filtered:?}, flags {flags}) in \
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
) -> Held {
    // A stored type, then the records a view's fold keeps, put under its
    // kept name, at positions after the type's.
    sorted_page_reads(engine, "t", false, 0)?;
    sorted_page_reads(engine, "w", true, 10)
}

/// [`a_page_ordered_by_an_indexed_field_reads_its_page_and_the_setting_switches_it`]
/// for the type `name`, declared from `from` on, a view of `t` if `kept`,
/// whose records are put under its kept name.
fn sorted_page_reads(engine: &dyn LakeEngine, name: &str, kept: bool, from: u64) -> Held {
    const N: u64 = 1000;
    const LIMIT: u32 = 10;
    let mut declared = json!({
        "declare": "type", "name": name,
        "fields": [{"name": "v", "type": {"kind": "text"}}]
    });
    let mut ty = TypeName::from(name);
    if kept {
        declared["view"] = json!({"source": "t", "group_by": ["v"], "measures": []});
        ty = lakelands_api::seam::kept_name(&ty);
    }
    let decl: Declaration = serde_json::from_value(declared)
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
                ty.as_str(),
                &format!("k{i:04}"),
                json!({"v": format!("v{:04} {}", (i * 7919) % N, "past the cut ".repeat(3))}),
                from + 1,
            )
        })
        .collect();
    fold(engine, from + 1, |p| {
        p.put_declaration(&decl, 1, None, Position(from))?;
        states.iter().try_for_each(|r| p.put_record(r))
    })?;
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
    let mut at = from + 1;
    for (asked, index) in [
        (None, false),
        (Some(json!(["v"])), true),
        (Some(Value::Null), false),
        (Some(json!(["v"])), true),
        (Some(json!([])), false),
    ] {
        if let Some(fields) = &asked {
            at += 1;
            let key = format!("sort_index.{name}");
            let setting = |fields| json!({"key": key, "value": fields});
            let mut state = record(
                lakelands_api::seam::SETTING_TYPE,
                &key,
                setting(fields.clone()),
                at,
            );
            if fields.is_null() {
                state.fields = record(
                    lakelands_api::seam::SETTING_TYPE,
                    &key,
                    setting(json!(["v"])),
                    at,
                )
                .fields;
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
                "with {asked}, {which} of {LIMIT} of `{ty}` ordered by `v` {dir:?} is not what one scan \
                 over every state answers"
            );
            if index {
                // The row `after` names, read again as the start of its
                // run, and the rows that close the page's last run.
                ensure!(
                    read <= u64::from(LIMIT) + 3,
                    "with {asked}, {which} of {LIMIT} of `{ty}` ordered by `v` {dir:?} read {read} of the \
                     {N} records; the order index reads its page and at most three more"
                );
            } else {
                ensure!(
                    read >= N,
                    "with {asked}, {which} of {LIMIT} of `{ty}` ordered by `v` {dir:?} read {read} rows; \
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
) -> Held {
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

fn a_records_edges_at_the_head_read_its_links_not_the_type(engine: &dyn LakeEngine) -> Held {
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

fn a_scan_with_equal_answers_exactly_the_identical_values(engine: &dyn LakeEngine) -> Held {
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

fn a_scan_with_equal_any_answers_what_one_equal_a_tuple_answers(engine: &dyn LakeEngine) -> Held {
    fold(engine, 1, |p| {
        for (key, v, w) in [
            ("a", json!("10"), json!("x")),
            ("b", json!("010"), json!("x")),
            ("c", json!(10), json!("y")),
            ("d", json!("10.0"), json!("y")),
            ("e", json!("20"), json!("x")),
            ("f", json!("20"), json!("y")),
            ("g", json!("10"), json!("y")),
        ] {
            p.put_record(&record("t", key, json!({"v": v, "w": w}), 1))?;
        }
        // More holders of two texts than one batch of a key-ordered read.
        for i in 0..1100 {
            let v = if i % 2 == 0 { "30" } else { "31" };
            p.put_record(&record("t", &format!("m{i:04}"), json!({"v": v}), 1))?;
        }
        p.put_record(&record("t", "h", json!({"w": "x"}), 1))
    })?;
    // Later, `g` moves away: the head and position 1 differ.
    fold_one(engine, 2, record("t", "g", json!({"v": "11", "w": "y"}), 2))?;
    let ty = TypeName::from("t");
    let one = |pairs: &[(&str, Value)]| -> Vec<(FieldName, Value)> {
        pairs
            .iter()
            .map(|(f, v)| (FieldName::from(*f), v.clone()))
            .collect()
    };
    let asks: Vec<Vec<Vec<(FieldName, Value)>>> = vec![
        vec![one(&[("v", json!("10"))]), one(&[("v", json!("010"))])],
        vec![one(&[("v", json!(10))]), one(&[("v", json!("20"))])],
        vec![
            one(&[("v", json!("10")), ("w", json!("x"))]),
            one(&[("v", json!("20")), ("w", json!("y"))]),
        ],
        vec![one(&[("v", json!(null))]), one(&[("v", json!("11"))])],
        vec![one(&[("v", json!("10"))])],
        vec![one(&[("v", json!("10"))]), one(&[("v", json!(10))])],
        vec![one(&[("v", json!("30"))]), one(&[("v", json!("31"))])],
    ];
    for at in [None, Some(Position(1))] {
        let v = view(engine, at)?;
        for any in &asks {
            let mut each = Vec::new();
            for t in any {
                let scan = scan_with(|s| s.equal = t.clone());
                each.extend(keys(&v.scan(&ty, &scan).must("scanning for one tuple")?));
            }
            each.sort();
            each.dedup();
            let whole = keys(
                &v.scan(&ty, &scan_with(|s| s.equal_any = any.clone()))
                    .must("scanning for any")?,
            );
            let counted = v.count(&ty, &scan_with(|s| s.equal_any = any.clone()));
            let counted = counted.must("counting any")?;
            let mut walked: Vec<String> = Vec::new();
            loop {
                let after = walked.last().map(|k| After {
                    values: Vec::new(),
                    key: RecordKey::from(k.as_str()),
                });
                let page = scan_with(|s| {
                    (s.equal_any, s.after, s.limit) = (any.clone(), after.clone(), 1)
                });
                let got = keys(&v.scan(&ty, &page).must("walking any")?);
                if got.is_empty() {
                    break;
                }
                walked.extend(got.into_iter().map(|k| k.to_string()));
            }
            ensure!(
                whole == each && walked == each && counted == each.len() as u64,
                "at {at:?} a scan for any of {any:?} selected {} whole, {} walked and counted \
                 {counted}; one scan a tuple selects {}",
                whole.len(),
                walked.len(),
                each.len()
            );
        }
    }
    Ok(())
}

fn a_kept_name_answers_as_a_stored_type_holding_the_same_states(engine: &dyn LakeEngine) -> Held {
    // Under each name `a` is put at every position, the even keys of
    // 2 600 at position 1 and the odd ones, which lie between them, after
    // `then`; one key ends before `then` and one after it; some keys are
    // retired and some restricted. The output is recorded at every
    // position.
    const STATES: u64 = 3_000;
    const KEYS: u64 = 2_600;
    let (then, seam, input) = (STATES / 2, "sys:kept", Digest::zero());
    let names = [
        lakelands_api::seam::kept_name(&TypeName::from("v")),
        TypeName::from("t"),
    ];
    let key = |i: u64| format!("k{i:04}");
    let puts = |ty: &TypeName, at: u64| {
        let mut all = vec![record(ty.as_str(), "a", json!({"n": at, "g": "x"}), at)];
        let born = |i: &u64| at == if i.is_multiple_of(2) { 1 } else { then + 1 };
        all.extend((0..KEYS).filter(born).map(|i| {
            let g = if i.is_multiple_of(3) { "x" } else { "y" };
            let mut r = record(ty.as_str(), &key(i), json!({"n": i, "g": g}), at);
            (r.retired, r.restricted) = (i % 11 == 5, i % 13 == 6);
            r
        }));
        let ended = |i| StoredRecord::ended(ty.clone(), RecordKey::from(key(i)), Position(at));
        all.extend((at == 2).then(|| ended(200)));
        all.extend((at == then + 2).then(|| ended(100)));
        all
    };
    for first in (1..=STATES).step_by(200) {
        fold(engine, first + 199, |p| {
            for at in first..first + 200 {
                for r in names.iter().flat_map(|ty| puts(ty, at)) {
                    p.put_record(&r)?;
                }
                p.put_output(seam, &input, &json!(at), Position(at))?;
            }
            Ok(())
        })?;
    }
    let some: Vec<RecordKey> = ["a", "k0000", "k0001", "k0100", "k0200", "none"]
        .map(RecordKey::from)
        .to_vec();
    let filter = crate::expr::parse("n >= 1000").expect("the check's filter parses");
    for (as_of, at, held) in [(None, STATES, KEYS + 1), (Some(then), then, KEYS / 2 + 1)] {
        let v = view(engine, as_of.map(Position))?;
        let recorded = v.output(seam, &input).must("reading the output")?;
        ensure!(
            recorded == Some((Position(at), json!(at))),
            "an output recorded at every position through {STATES} read {recorded:?} as of \
             {as_of:?}"
        );
        // Every answer under a name, each record as the other name's.
        let read = |ty: &TypeName| -> Result<(Vec<Vec<StoredRecord>>, Vec<u64>), Failure> {
            let mut one = Vec::new();
            for key in &some {
                one.extend(v.get(ty, key).must("reading one record")?);
            }
            let mut pages = vec![one, v.get_many(ty, &some).must("reading by keys")?];
            // The walk ends at a page that is not full, and after ten whatever
            // an engine answers.
            let mut walk = scan_with(|s| (s.limit, s.retired, s.restricted) = (1024, true, true));
            for _ in 0..10 {
                let page = v.scan(ty, &walk).must("walking by key")?;
                let last = page.last().filter(|_| page.len() == 1024);
                walk.after = last.map(|r| After {
                    values: vec![],
                    key: r.key.clone(),
                });
                pages.push(page);
                if walk.after.is_none() {
                    break;
                }
            }
            let mut counts = vec![pages[2..].iter().map(Vec::len).sum::<usize>() as u64];
            for scan in [
                scan_all(),
                scan_with(|s| s.retired = true),
                scan_with(|s| s.restricted = true),
                scan_with(|s| s.filter = Some(filter.clone())),
                scan_with(|s| s.equal = vec![("g".into(), json!("x"))]),
                scan_with(|s| (s.order_by, s.limit) = (vec![by("n", Direction::Desc)], 50)),
            ] {
                counts.push(v.count(ty, &scan).must("counting")?);
                pages.push(v.scan(ty, &scan).must("scanning")?);
            }
            for r in pages.iter_mut().flatten() {
                r.ty = names[1].clone();
            }
            Ok((pages, counts))
        };
        let (kept, stored) = (read(&names[0])?, read(&names[1])?);
        let sizes = |(pages, counts): &(Vec<Vec<StoredRecord>>, Vec<u64>)| {
            let rows: Vec<usize> = pages.iter().map(Vec::len).collect();
            format!("{rows:?} records and the counts {counts:?}")
        };
        ensure!(
            kept == stored && kept.1[0] == held,
            "as of {as_of:?}, a `get` of six keys, a `get_many` of them, a walk by key of \
             every state, {held} of them, 1 024 a page, and a scan under no flag, each flag, \
             a filter, an exact text and a field's order answered, under the kept name, \
             {}, the walk's states first; under an ordinary name, {}",
            sizes(&kept),
            sizes(&stored)
        );
    }
    Ok(())
}

fn a_chosen_field_a_writer_without_it_folded_past_is_read_by_no_view(
    engine: &dyn LakeEngine,
    harness: &dyn EngineHarness,
) -> Held {
    use lakelands_api::engine::{Choose, Chosen, OpenMode};
    const N: u64 = 300;
    let decl = declare(json!({
        "declare": "type", "name": "t",
        "fields": [
            {"name": "v", "type": {"kind": "text"}},
            {"name": "w", "type": {"kind": "text"}}
        ]
    }));
    let Declaration::Type(t) = &decl else {
        unreachable!("the check declares a type")
    };
    let mut states: Vec<StoredRecord> = (0..N)
        .map(|i| {
            record(
                "t",
                &format!("k{i:04}"),
                json!({"v": format!("v{:04}", (i * 7919) % N)}),
                1,
            )
        })
        .collect();
    fold(engine, 1, |p| {
        p.put_declaration(&decl, 1, None, Position(0))?;
        states.iter().try_for_each(|r| p.put_record(r))
    })?;
    let (ty, v) = (TypeName::from("t"), FieldName::from("v"));
    // Start, fill in one chunk and keep `field`.
    let choose = |opened: &dyn LakeEngine, field: &FieldName| -> Held {
        let fill = Choose::Fill {
            field: field.clone(),
            after: None,
            limit: N as u32,
        };
        for step in [
            Choose::Start(field.clone()),
            fill,
            Choose::Keep(field.clone()),
        ] {
            let chosen = opened.projection().choose_order(&ty, &step);
            chosen.must("a step of choosing")?;
        }
        Ok(())
    };
    choose(engine, &v)?;
    if !harness.fold_as_before(engine.lake()) {
        return Ok(());
    }
    let scan = Scan {
        order_by: vec![order("v", OrderBy::Value, Direction::Asc)],
        limit: 10,
        ..Scan::default()
    };
    let answers = |opened: &dyn LakeEngine, states: &[StoredRecord], when: &str| -> Held {
        let at = view(opened, None)?;
        let page = at.scan(&ty, &scan).must("scanning a page ordered by `v`")?;
        ensure!(
            keys(&page) == keys(&crate::engines::scan(Some(t), states, &scan)),
            "{when}, a page ordered by `v` is not what one scan over every state answers: it read the rows a writer without the chosen field left"
        );
        let chosen = at.chosen(&ty).must("reading the chosen fields")?;
        ensure!(
            chosen == Chosen::default(),
            "{when}, `chosen` names {chosen:?}; a writer without it folded past the choice"
        );
        Ok(())
    };
    let open = |mode: OpenMode| {
        let opened = harness.reopen(engine.lake(), mode);
        opened.map_err(|e| Failure::new(format!("opening the lake {mode:?}: {e}")))
    };
    let reader = open(OpenMode::Read)?;
    answers(reader.as_ref(), &states, "read-only")?;
    // The handle that chose, its lease taken back with no open: neither its
    // commit nor a step of choosing makes the rows left read again.
    let put = |opened: &dyn LakeEngine, at: u64, states: &mut Vec<StoredRecord>| {
        let new = record("t", &format!("n{at:04}"), json!({"v": "v0000"}), at);
        states.push(new.clone());
        fold_one(opened, at, new)
    };
    put(engine, 3, &mut states)?;
    let start = Choose::Start(FieldName::from("w"));
    engine
        .projection()
        .choose_order(&ty, &start)
        .must("a step of choosing")?;
    put(engine, 4, &mut states)?;
    answers(
        engine,
        &states,
        "after a commit and a step of choosing through the handle that chose",
    )?;
    let writer = open(OpenMode::Write)?;
    answers(writer.as_ref(), &states, "after a writer's open")?;
    put(writer.as_ref(), 5, &mut states)?;
    answers(writer.as_ref(), &states, "after a commit of the writer")?;
    answers(
        reader.as_ref(),
        &states,
        "read-only, after a commit of the writer",
    )?;
    // The open dropped what was left: the lake chooses again.
    choose(writer.as_ref(), &v)?;
    let chosen = view(writer.as_ref(), None)?
        .chosen(&ty)
        .must("reading the chosen fields")?;
    ensure!(
        chosen.kept == [v.clone()],
        "after a writer's open dropped the choice left behind, the lake must choose again; `chosen` names {chosen:?}"
    );
    // Such a writer's rebuild through the same position empties the order
    // index and keeps every mark.
    if harness.rebuild_as_before(engine.lake()) {
        let when = "after a rebuild by a writer without the chosen fields";
        answers(
            open(OpenMode::Read)?.as_ref(),
            &states,
            &format!("read-only, {when}"),
        )?;
        let writer = open(OpenMode::Write)?;
        answers(
            writer.as_ref(),
            &states,
            &format!("{when} and a writer's open"),
        )?;
        choose(writer.as_ref(), &v)?;
        let chosen = view(writer.as_ref(), None)?
            .chosen(&ty)
            .must("reading the chosen fields")?;
        ensure!(
            chosen.kept == [v],
            "{when}, a writer's open must leave the lake choosing again; `chosen` names {chosen:?}"
        );
    }
    Ok(())
}

fn a_chosen_order_index_is_built_beside_the_fold_and_read_only_once_it_is_kept(
    engine: &dyn LakeEngine,
) -> Held {
    use lakelands_api::engine::Choose;
    const N: u64 = 600;
    const LIMIT: u32 = 10;
    const CHUNK: u32 = 100;
    let decl = declare(json!({
        "declare": "type", "name": "t",
        "fields": [{"name": "v", "type": {"kind": "text"}}]
    }));
    let Declaration::Type(t) = &decl else {
        unreachable!("the check declares a type")
    };
    let v = |n: u64| json!({"v": format!("v{n:04}")});
    let mut states: Vec<StoredRecord> = (0..N)
        .map(|i| record("t", &format!("k{i:04}"), v((i * 7919) % N), 1))
        .collect();
    fold(engine, 1, |p| {
        p.put_declaration(&decl, 1, None, Position(0))?;
        states.iter().try_for_each(|r| p.put_record(r))
    })?;
    let ty = TypeName::from("t");
    let field = FieldName::from("v");
    let step = |s: Choose| {
        engine
            .projection()
            .choose_order(&ty, &s)
            .must("a step of choosing")
    };
    let chosen = || -> Result<Option<lakelands_api::engine::Chosen>, Failure> {
        let held = view(engine, None)?
            .chosen(&ty)
            .must("reading the chosen fields")?;
        Ok(Some(held).filter(|h| *h != lakelands_api::engine::Chosen::default()))
    };
    // A page ordered by `v`: whether it answers what one scan over every
    // state answers, and how many rows it read.
    let page = |states: &[StoredRecord]| -> Result<(u64, bool), Failure> {
        let scan = Scan {
            order_by: vec![order("v", OrderBy::Value, Direction::Asc)],
            limit: LIMIT,
            ..Scan::default()
        };
        let at = view(engine, None)?;
        let before = crate::engines::rows_read();
        let rows = at.scan(&ty, &scan).must("scanning a page ordered by `v`")?;
        let read = crate::engines::rows_read() - before;
        Ok((
            read,
            keys(&rows) == keys(&crate::engines::scan(Some(t), states, &scan)),
        ))
    };
    let sorts = |what: &str, states: &[StoredRecord]| -> Held {
        let (read, right) = page(states)?;
        ensure!(
            right,
            "{what}, a page of {LIMIT} ordered by `v` is not what one scan over every state answers"
        );
        ensure!(
            read >= N,
            "{what}, a page of {LIMIT} ordered by `v` read {read} rows; no index is kept for `v` yet, so it sorts all {N}"
        );
        Ok(())
    };
    let reads_its_page = |what: &str, states: &[StoredRecord]| -> Held {
        let (read, right) = page(states)?;
        ensure!(
            right,
            "{what}, a page of {LIMIT} ordered by `v` is not what one scan over every state answers"
        );
        ensure!(
            read <= u64::from(LIMIT) + 3,
            "{what}, a page of {LIMIT} ordered by `v` read {read} of the {N} records; an index reads its page and at most three more"
        );
        Ok(())
    };
    ensure!(
        chosen()?.is_none(),
        "a lake that chose nothing must name no chosen field"
    );
    sorts("before anything is chosen", &states)?;
    // A field the type does not hold is no step's, on every engine.
    step(Choose::Start(FieldName::from("absent")))?;
    ensure!(
        chosen()?.is_none(),
        "`Start` of a field the type does not declare must do nothing"
    );
    step(Choose::Start(field.clone()))?;
    ensure!(
        chosen()?.is_some_and(|c| c.building == [field.clone()] && c.kept.is_empty()),
        "after `Start`, `v` must be building and none kept"
    );
    sorts("while `v` is being built", &states)?;
    // Chunks, with a put between each: a record changed, and a new one.
    let (mut after, mut at) = (None, 1);
    for round in 0.. {
        let next = step(Choose::Fill {
            field: field.clone(),
            after: after.clone(),
            limit: CHUNK,
        })?;
        if round > 20 {
            return Err(Failure::new(
                "a fill of 600 records in chunks of 100 never ended",
            ));
        }
        at += 1;
        let mut moved = record("t", &format!("k{:04}", round * 7), v(1000 + round), at);
        moved.version = lakelands_api::Version(at);
        let new = record("t", &format!("n{round:04}"), v(2000 + round), at);
        fold(engine, at, |p| {
            p.put_record(&moved)?;
            p.put_record(&new)
        })?;
        states.retain(|r| r.key != moved.key);
        states.extend([moved, new]);
        sorts(&format!("after chunk {round} of the build"), &states)?;
        match next {
            Some(last) => after = Some(last),
            None => break,
        }
    }
    step(Choose::Keep(field.clone()))?;
    ensure!(
        chosen()?.is_some_and(|c| c.kept == [field.clone()] && c.building.is_empty()),
        "after `Keep`, `v` must be kept and none building"
    );
    reads_its_page(
        "once `v` is kept, with the records put during its build",
        &states,
    )?;
    // A host's setting wins: the choice goes, the setting's index stays.
    at += 1;
    fold_one(engine, at, sort_setting(json!(["v"]), at))?;
    ensure!(
        chosen()?.is_none(),
        "a `sort_index.t` setting naming `v` must take the lake's choice away"
    );
    reads_its_page("with the setting", &states)?;
    ensure!(
        step(Choose::Start(field.clone()))?.is_none() && chosen()?.is_none(),
        "a step on a type whose setting names a field must do nothing"
    );
    at += 1;
    let mut gone = sort_setting(json!(["v"]), at);
    gone.retired = true;
    fold_one(engine, at, gone)?;
    sorts("with the setting retired", &states)?;
    // Built again, then dropped.
    step(Choose::Start(field.clone()))?;
    let mut after = None;
    while let Some(last) = step(Choose::Fill {
        field: field.clone(),
        after,
        limit: CHUNK,
    })? {
        after = Some(last);
    }
    step(Choose::Keep(field.clone()))?;
    reads_its_page("once `v` is chosen again", &states)?;
    step(Choose::Drop(field.clone()))?;
    ensure!(
        chosen()?.is_none(),
        "a dropped field must no longer be chosen"
    );
    sorts("once `v` is dropped", &states)
}

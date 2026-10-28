//! At the head, a count and a page by key whose filter holds an exact
//! text are handed the records holding that text, not the type
//! (lakelands#574), and so is a page in the order of a field the lake
//! indexes, whether many records hold the text or few (lakelands#587).
//! The suite's check runs on the memory engine too, which keeps no index
//! of exact text and reads the type, so its bound cannot tell the index
//! from a walk of the type; this one can. Here the statements of a page
//! under a text many hold are planned, the tables analysed, without a sort,
//! and the cursor of a long page under a text few hold from the text's
//! rows, a seek for each holder.

mod common;

use lakelands_api::engine::{After, EngineOpener, LakeEngine, OpenMode, Scan};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::Declaration;
use lakelands_core::engines;
use lakelands_engine_pg::{
    PgOpener, few_batch_statements, few_holders_statements, holding_statements,
};
use postgres::types::ToSql;
use serde_json::json;

/// More than two batches of 1 024.
const N: u64 = 3000;

/// The records of the walked type: `v` is `past` in one more than
/// `FEW_HOLDERS` and `at` in exactly that many, a tenth each; `rare` in
/// five that hold `w`, and `bare` in five that hold none; three in a
/// hundred of the others hold no `w` either. Those holding `w` hold one of seven `x`, and
/// one in thirteen is restricted.
fn walked() -> Vec<StoredRecord> {
    let (mut past, mut at) = (0, 0);
    let states: Vec<StoredRecord> = (0..41_000_u64)
        .map(|i| {
            let v = match i {
                _ if i % 10_000 == 505 => "rare",
                _ if i % 10_000 == 700 => "bare",
                _ if i % 10 == 1 && past <= engines::FEW_HOLDERS => {
                    past += 1;
                    "past"
                }
                _ if i % 10 == 2 && at < engines::FEW_HOLDERS => {
                    at += 1;
                    "at"
                }
                _ => "other",
            };
            let mut fields = json!({"v": v});
            if v != "bare" && i % 100 >= 3 {
                fields["w"] = json!((i * 7919) % 41_000);
                fields["x"] = json!(i % 7);
            }
            StoredRecord {
                ty: "t".into(),
                key: format!("k{i:05}").into(),
                version: Version(1),
                fields: serde_json::from_value(fields).unwrap(),
                meta: Default::default(),
                position: Position(1),
                made: Position(1),
                retired: false,
                restricted: i % 13 == 4,
            }
        })
        .collect();
    assert_eq!((past, at), (engines::FEW_HOLDERS + 1, engines::FEW_HOLDERS));
    states
}

#[test]
fn a_count_and_a_page_under_an_exact_text_are_handed_the_records_holding_it() {
    let Some(url) = common::url_or_skip("the pg text-lookup test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "texts", &schema), OpenMode::Create)
        .unwrap();
    // Three records hold `v = 'b'`, one in each batch of the type.
    let p = engine.projection();
    p.begin().unwrap();
    for i in 0..N {
        let v = if i % 1000 == 500 { "b" } else { "a" };
        p.put_record(&StoredRecord {
            ty: "t".into(),
            key: format!("k{i:04}").into(),
            version: Version(1),
            fields: serde_json::from_value(json!({"v": v})).unwrap(),
            meta: Default::default(),
            position: Position(1),
            made: Position(1),
            retired: false,
            restricted: false,
        })
        .unwrap();
    }
    p.commit(Position(1)).unwrap();
    let (ty, view) = (TypeName::from("t"), engine.view(None).unwrap());
    let scan = Scan {
        filter: Some(lakelands_core::expr::parse("v = 'b'").unwrap()),
        limit: u32::MAX,
        ..Scan::default()
    };
    let before = engines::rows_read();
    let counted = view.count(&ty, &scan).unwrap();
    let read = engines::rows_read() - before;
    assert!(
        counted == 3 && read <= 3,
        "a count under `v = 'b'` at the head answered {counted} from {read} rows handed to the \
         rulebook, of a type of {N}; three records hold the text, and it reads those"
    );
    let before = engines::rows_read();
    let page = view.scan(&ty, &Scan { limit: 10, ..scan }).unwrap();
    let read = engines::rows_read() - before;
    assert!(
        page.len() == 3 && read <= 3,
        "a page of 10 by key under `v = 'b'` at the head answered {} rows from {read} handed to \
         the rulebook, of a type of {N}; three records hold the text, and it reads those",
        page.len()
    );
    drop(view);
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// Walks `w` in pages of 10 in either direction under each text: the
/// order index's rows read in its order where more than `FEW_HOLDERS`
/// hold the text (`past`), the text index's rows where at most that many
/// do (`at`, `rare`, `bare`), and the records holding no `w` after them.
/// Each page is the rulebook's, and is handed at most two batches: the
/// page that passes from the indexed records to those without `w` asks
/// each for one. Restricted records are passed over, ties in `x` are
/// walked alone and under `w`, `only` pairs are kept, a page of one whose
/// filter rejects most holders asks for a handful of batches, and under a
/// text few hold reads the first three as any page does and the holders
/// left once. `planned` runs while `at` is held by exactly `FEW_HOLDERS`,
/// and then a walk goes on across writes that move a text from one way to
/// the other.
fn walk_under_each_text(engine: &dyn LakeEngine, planned: impl FnOnce()) {
    let decl: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "t",
        "fields": [
            {"name": "v", "type": {"kind": "text"}}, {"name": "w", "type": {"kind": "number"}},
            {"name": "x", "type": {"kind": "number"}}
        ]
    }))
    .unwrap();
    let Declaration::Type(t) = &decl else {
        unreachable!()
    };
    let mut states = walked();
    let p = engine.projection();
    p.begin().unwrap();
    p.put_declaration(&decl, 1, None, Position(0)).unwrap();
    states.iter().for_each(|r| p.put_record(r).unwrap());
    // The order index on `w` and `x`, and the exact-text index on `v`,
    // which no declaration looks records up by (057).
    for (key, value) in [
        ("sort_index.t", json!(["w", "x"])),
        ("lookup_index.t", json!(["v"])),
    ] {
        p.put_record(&StoredRecord {
            ty: lakelands_api::seam::SETTING_TYPE.into(),
            key: key.into(),
            fields: serde_json::from_value(json!({"key": key, "value": value})).unwrap(),
            restricted: false,
            ..states[0].clone()
        })
        .unwrap();
    }
    p.commit(Position(1)).unwrap();
    let scan = |filter: &str, order: &[(&str, Direction)]| Scan {
        filter: Some(lakelands_core::expr::parse(filter).unwrap()),
        order_by: order
            .iter()
            .map(|&(field, dir)| Order {
                field: field.into(),
                by: OrderBy::Value,
                dir,
            })
            .collect(),
        limit: 10,
        ..Scan::default()
    };
    for text in ["past", "at", "rare", "bare"] {
        for dir in [Direction::Asc, Direction::Desc] {
            let walk = scan(&format!("v = '{text}'"), &[("w", dir)]);
            walk_pages(engine, t, &states, walk, Some(2 * 10 + 2), |_| {});
        }
    }
    // Ties: `x` takes seven values, each held by hundreds under a text.
    for text in ["past", "at"] {
        for dir in [Direction::Asc, Direction::Desc] {
            let filter = format!("v = '{text}'");
            walk_pages(
                engine,
                t,
                &states,
                scan(&filter, &[("x", dir)]),
                Some(22),
                |_| {},
            );
            let by_both = Scan {
                limit: 100,
                ..scan(&filter, &[("x", dir), ("w", Direction::Asc)])
            };
            walk_pages(engine, t, &states, by_both, None, |_| {});
            let only = Scan {
                equal: vec![("x".into(), json!(3))],
                limit: 100,
                ..scan(&filter, &[("w", dir)])
            };
            walk_pages(engine, t, &states, only, None, |_| {});
        }
    }
    // A page of one under a filter that passes over all but a fortieth of
    // the holders: each batch the engine answers may read every holder.
    // And pages of one under `at` that pass over its first holders in
    // `w`'s order, none to eight of them.
    let w = |r: &StoredRecord| r.fields.get("w").and_then(Value::as_u64);
    let held = |r: &&StoredRecord| r.fields.get("v") == Some(&json!("at"));
    let mut first: Vec<u64> = states.iter().filter(held).filter_map(w).collect();
    first.sort_unstable();
    let passing = first[..9].iter().map(|w| format!("v = 'at' and w >= {w}"));
    let rejecting = ["at", "past"].map(|text| format!("v = '{text}' and w > 40000"));
    let view = engine.view(None).unwrap();
    let mut asked_by_few = std::collections::BTreeSet::new();
    for filter in passing.chain(rejecting) {
        let page = Scan {
            limit: 1,
            ..scan(&filter, &[("w", Direction::Asc)])
        };
        let expected = engines::scan(Some(t), &states, &page);
        let (before, read) = (engines::batches_read(), engines::holder_reads());
        let answered = view.scan(&TypeName::from("t"), &page).unwrap();
        let asked = engines::batches_read() - before;
        // Few hold `at`: a page reads up to three batches as any page
        // does, and declares its cursor when it asks for a fourth
        // (lakelands#695). Many hold `past`, and the order index is walked.
        let few = !filter.contains("past");
        let (reads, once) = (engines::holder_reads() - read, few && asked > 3);
        asked_by_few.extend(few.then_some(asked));
        assert!(
            (1..=16).contains(&asked)
                && reads == u64::from(once)
                && keys(&answered) == keys(&expected),
            "a page of one in `w`'s order under `{filter}` asked for {asked} batches of the \
             index and read the text's holders whole {reads} times; a page doubles its batch as \
             its filter rejects rows, and under a text few hold reads the holders left once when \
             it asks for a fourth batch, not before"
        );
    }
    let asked = |n: std::ops::RangeInclusive<u64>| asked_by_few.range(n).next().is_some();
    assert!(
        asked(1..=2) && asked(3..=3) && asked(4..=4) && asked(5..=16),
        "the pages under `at` asked for {asked_by_few:?} batches; they are to cover a page of \
         fewer than three batches, of three, of four and a longer one"
    );
    drop(view);
    planned();
    // Between two pages of a walk, `at` gains a holder and goes to the
    // order index's way; between two pages of the next, `past` loses one
    // and goes to the text index's.
    let gained = StoredRecord {
        key: "k99999".into(),
        fields: serde_json::from_value(json!({"v": "at", "w": 50_000, "x": 1})).unwrap(),
        position: Position(2),
        restricted: false,
        ..states[0].clone()
    };
    let past = |r: &&StoredRecord| r.fields.get("v") == Some(&json!("past"));
    let mut lost = states
        .iter()
        .filter(past)
        .max_by_key(|r| w(r))
        .unwrap()
        .clone();
    lost.fields = serde_json::from_value(json!({"v": "other", "w": w(&lost)})).unwrap();
    lost.position = Position(3);
    for (text, write) in [("at", &gained), ("past", &lost)] {
        states.retain(|r| r.key != write.key);
        states.push(write.clone());
        let walk = scan(&format!("v = '{text}'"), &[("w", Direction::Asc)]);
        walk_pages(engine, t, &states, walk, None, |page| {
            if page == 3 {
                p.begin().unwrap();
                p.put_record(write).unwrap();
                p.commit(write.position).unwrap();
            }
        });
    }
}

/// Walks `scan` to its end, a view of the head taken for each page and
/// `between(page)` called after each, and checks that it answers what the
/// rulebook answers over `states`, each page handed at most `bound` rows.
fn walk_pages(
    engine: &dyn LakeEngine,
    t: &TypeDecl,
    states: &[StoredRecord],
    mut scan: Scan,
    bound: Option<u64>,
    mut between: impl FnMut(usize),
) {
    let expected = engines::scan(
        Some(t),
        states,
        &Scan {
            limit: u32::MAX,
            ..scan.clone()
        },
    );
    let order: Vec<_> = scan
        .order_by
        .iter()
        .map(|o| format!("{} {:?}", o.field, o.dir))
        .collect();
    let what = format!("{order:?} under {:?} and {:?}", scan.filter, scan.equal);
    let mut got = Vec::new();
    for pages in 1.. {
        assert!(pages <= states.len(), "a walk in {what} never ends");
        let view = engine.view(None).unwrap();
        let before = engines::rows_read();
        let page = view.scan(&TypeName::from("t"), &scan).unwrap();
        let read = engines::rows_read() - before;
        drop(view);
        assert!(
            bound.is_none_or(|b| read <= b),
            "a page of {} in {what} at the head was handed {read} rows of a type of {}; it \
             reads its page",
            scan.limit,
            states.len()
        );
        let Some(last) = page.last() else { break };
        scan.after = Some(After {
            values: scan
                .order_by
                .iter()
                .map(|o| last.fields.get(&o.field).cloned().unwrap_or(Value::Null))
                .collect(),
            key: last.key.clone(),
        });
        got.extend(keys(&page));
        between(pages);
    }
    assert!(
        got == keys(&expected),
        "a walk in {what} answered {} rows, the rulebook {}",
        got.len(),
        expected.len()
    );
}

fn keys(page: &[StoredRecord]) -> Vec<RecordKey> {
    page.iter().map(|r| r.key.clone()).collect()
}

/// The plan of each statement of a page under a text many records hold,
/// the tables analysed first, so that the plan does not hang on whether
/// the server's own analysis has run (lakelands#674): the order index's
/// rows in its order, or the text index's in key order, and no sort.
///
/// The cursor a long page under a text few records hold declares
/// (lakelands#695), and each batch before it (lakelands#840), have one plan, at five holders (`rare`) and at the
/// ceiling (`at`, read while exactly `FEW_HOLDERS` hold it), in either
/// direction: from the text's rows, each holder's order row sought by its
/// key, sorted once, and each record sought by its key. It never reads a
/// table whole, whatever the type's size, and every step of it ran.
fn planned_without_a_sort(url: &str, schema: &str) {
    let (url, schema) = (url.to_owned(), schema.to_owned());
    let plans: Vec<Vec<String>> = std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        client
            .batch_execute(&format!(
                "ANALYZE {schema}.records; ANALYZE {schema}.text_index; \
                 ANALYZE {schema}.sort_index; SET plan_cache_mode = force_custom_plan"
            ))
            .unwrap();
        let few = engines::FEW_HOLDERS as i64 + 1;
        // A cursor continues after a row in the middle of `w`'s order.
        let (after, key) = (engines::order_key(&json!(20_000)), "k");
        let many: Vec<&(dyn ToSql + Sync)> = vec![&"t", &"w", &10_i64, &"v", &"past"];
        let held =
            |text| -> Vec<&(dyn ToSql + Sync)> { vec![&"t", &"w", &few, &"v", text, &after, &key] };
        let holding = holding_statements(&schema).map(|s| (s, many.clone()));
        let [up, down] = few_holders_statements(&schema);
        let [batch_up, batch_down] = few_batch_statements(&schema);
        let cursors = [&"at", &"rare"].map(|t| {
            [
                up.clone(),
                down.clone(),
                batch_up.clone(),
                batch_down.clone(),
            ]
            .map(|s| (s, held(t)))
        });
        (holding.iter().chain(cursors.as_flattened()))
            .map(|(statement, args)| {
                let explain =
                    format!("EXPLAIN (ANALYZE, COSTS OFF, TIMING OFF, SUMMARY OFF) {statement}");
                let rows = client.query(&explain, args).unwrap();
                rows.iter().map(|row| row.get(0)).collect()
            })
            .collect()
    })
    .join()
    .unwrap();
    let (holding, cursors) = plans.split_at(3);
    for (plan, index) in
        holding
            .iter()
            .zip(["sort_index_asc", "sort_index_desc", "text_index_value"])
    {
        let has = |step: &str| plan.iter().any(|line| line.contains(step));
        assert!(
            has(index) && !has("Sort") && !has("Seq Scan"),
            "a page under a text many records hold is not read from `{index}` in its order, \
             with no sort:\n{plan:#?}"
        );
    }
    for plan in cursors {
        let has = |step: &str| plan.iter().any(|line| line.contains(step));
        let sorts = plan.iter().filter(|line| line.contains("Sort Key")).count();
        assert!(
            ["text_index_value", "sort_index_pkey", "records_head_pkey"]
                .iter()
                .all(|index| has(index))
                && sorts == 1
                && !has("Seq Scan")
                && !has("never executed"),
            "the cursor or a batch of a page under a text few records hold is not read from \
             the text's rows, each holder's order row sought by its key, those sorted once, and each \
             record sought by its key:\n{plan:#?}"
        );
    }
}

#[test]
fn a_page_in_a_fields_order_under_an_exact_text_is_handed_its_page() {
    let Some(url) = common::url_or_skip("the pg text-order test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "texts", &schema), OpenMode::Create)
        .unwrap();
    walk_under_each_text(engine.as_ref(), || planned_without_a_sort(&url, &schema));
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// A find and a count whose `where` holds the type's key of one text
/// field equal to a text read the state at the key the text makes, at the
/// head and as of an earlier position, not the type (057, lakelands#1318).
/// The exact-text index keeps no rows for that field, so a walk would
/// hand the rulebook every record.
fn read_by_key(engine: &dyn LakeEngine) {
    let decl: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "t", "key": ["id"],
        "fields": [{"name": "id", "type": {"kind": "text"}}, {"name": "n", "type": {"kind": "number"}}]
    }))
    .unwrap();
    let state = |id: &str, n: u64| StoredRecord {
        ty: "t".into(),
        key: lakelands_api::key::key_component(&json!(id))
            .unwrap()
            .into(),
        version: Version(n),
        fields: serde_json::from_value(json!({"id": id, "n": n})).unwrap(),
        meta: Default::default(),
        position: Position(n),
        made: Position(1),
        retired: false,
        restricted: false,
    };
    let ids = (0..N).map(|i| format!("k{i:04}"));
    let p = engine.projection();
    p.begin().unwrap();
    p.put_declaration(&decl, 1, None, Position(0)).unwrap();
    for id in ids.chain(["a:b".into(), "50%".into()]) {
        p.put_record(&state(&id, 1)).unwrap();
    }
    p.commit(Position(1)).unwrap();
    p.begin().unwrap();
    p.put_record(&state("a:b", 2)).unwrap();
    p.commit(Position(2)).unwrap();
    let ty = TypeName::from("t");
    let by_n = || Order {
        field: "n".into(),
        by: OrderBy::Value,
        dir: Direction::Asc,
    };
    for (at, n) in [(None, 2), (Some(Position(1)), 1)] {
        let view = engine.view(at).unwrap();
        let ask = |what: &str, scan: Scan, want: &[&str]| {
            let before = engines::rows_read();
            let counted = view.count(&ty, &scan).unwrap();
            let page = view.scan(&ty, &scan).unwrap();
            let sorted = view
                .scan(
                    &ty,
                    &Scan {
                        order_by: vec![by_n()],
                        ..scan
                    },
                )
                .unwrap();
            let read = engines::rows_read() - before;
            let found = |page: &[StoredRecord]| -> Vec<String> {
                let mut found: Vec<String> = page.iter().map(|r| r.key.to_string()).collect();
                found.sort();
                found
            };
            let (listed, ordered) = (found(&page), found(&sorted));
            // The count is not printed: CodeQL follows the engine's
            // connection into it and calls that logging a certificate.
            let count = match counted == want.len() as u64 {
                true => "right",
                false => "wrong",
            };
            assert!(
                count == "right"
                    && listed == want
                    && ordered == want
                    && read <= 3 * want.len().max(1) as u64,
                "as of {at:?}, {what} counted {count}, found {listed:?}, and {ordered:?} in \
                 `n`'s order, from {read} rows handed to the rulebook, of a type of {N}; the \
                 records at the keys the texts make are read"
            );
        };
        let held = format!("id = 'a:b' and n = {n}");
        for (filter, want) in [
            (held.as_str(), &["a%3Ab"][..]),
            ("id = 'k0500'", &["k0500"]),
            ("n = 1 and id = '50%'", &["50%25"]),
            ("id = 'a:b' and n = 3", &[]),
            ("id = 'absent'", &[]),
        ] {
            let scan = Scan {
                filter: Some(lakelands_core::expr::parse(filter).unwrap()),
                limit: 10,
                ..Scan::default()
            };
            ask(&format!("`{filter}`"), scan, want);
        }
        // A derived relation's match asks for several texts at once.
        let scan = Scan {
            equal_any: ["k0500", "a:b", "absent"]
                .map(|t| vec![("id".into(), json!(t))])
                .into(),
            limit: 10,
            ..Scan::default()
        };
        ask("a match of three texts", scan, &["a%3Ab", "k0500"]);
    }
}

#[test]
fn a_find_and_a_count_by_a_text_key_read_its_record_by_key() {
    let Some(url) = common::url_or_skip("the pg text-key test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "texts", &schema), OpenMode::Create)
        .unwrap();
    read_by_key(engine.as_ref());
    drop(engine);
    common::drop_schema(&url, &schema);
}

//! At the head, a count and a page by key whose filter holds an exact
//! text are handed the records holding that text, not the type
//! (lakelands#574), and so is a page in the order of a field the lake
//! indexes, whether many records hold the text or few (lakelands#587).
//! The suite's check runs on the memory engine too, which keeps no index
//! of exact text and reads the type, so its bound cannot tell the index
//! from a walk of the type; this one can.

use std::path::PathBuf;

use lakelands_api::Location;
use lakelands_api::engine::{After, EngineOpener, LakeEngine, OpenMode, Scan};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::Declaration;
use lakelands_core::engines;
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

/// More than two batches of 1 024.
const N: u64 = 3000;

/// The records of the walked type: `v` is `past` in one more than
/// `FEW_HOLDERS` and `at` in exactly that many, a tenth each; `rare` in
/// five, and `bare` in five that hold no `w`; three in a hundred of the
/// others hold no `w` either. Those holding `w` hold one of seven `x`, and
/// one in thirteen is restricted.
fn walked() -> Vec<StoredRecord> {
    let (mut past, mut at) = (0, 0);
    let states: Vec<StoredRecord> = (0..41_000_u64)
        .map(|i| {
            let v = match i {
                _ if i % 10_000 == 500 => "rare",
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
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("text-lookup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let engine = SqliteOpener
        .open(&Location::sqlite("texts", &dir), OpenMode::Create)
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
    let _ = std::fs::remove_dir_all(&dir);
}

/// Walks `w` in pages of 10 in either direction under each text: the
/// order index's rows read in its order where more than `FEW_HOLDERS`
/// hold the text (`past`), the text index's rows where at most that many
/// do (`at`, `rare`, `bare`), and the records holding no `w` after them.
/// Each page is the rulebook's, and is handed at most two batches: the
/// page that passes from the indexed records to those without `w` asks
/// each for one. Restricted records are passed over, ties in `x` are
/// walked alone and under `w`, `only` pairs are kept, a page of one whose
/// filter rejects most holders asks for a handful of batches, and a walk
/// goes on across writes that move a text from one way to the other.
fn walk_under_each_text(engine: &dyn LakeEngine) {
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
    p.put_record(&StoredRecord {
        ty: lakelands_api::seam::SETTING_TYPE.into(),
        key: "sort_index.t".into(),
        fields: serde_json::from_value(json!({"key": "sort_index.t", "value": ["w", "x"]}))
            .unwrap(),
        restricted: false,
        ..states[0].clone()
    })
    .unwrap();
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
    let view = engine.view(None).unwrap();
    for text in ["at", "past"] {
        let rejecting = Scan {
            limit: 1,
            ..scan(
                &format!("v = '{text}' and w > 40000"),
                &[("w", Direction::Asc)],
            )
        };
        let expected = engines::scan(Some(t), &states, &rejecting);
        let before = engines::batches_read();
        let page = view.scan(&TypeName::from("t"), &rejecting).unwrap();
        let asked = engines::batches_read() - before;
        assert!(
            asked <= 16 && keys(&page) == keys(&expected),
            "a page of one in `w`'s order under `v = '{text}' and w > 40000` asked for {asked} \
             batches of the index; a page doubles its batch as its filter rejects rows"
        );
    }
    drop(view);
    // Between two pages of a walk, `at` gains a holder and goes to the
    // order index's way; between two pages of the next, `past` loses one
    // and goes to the text index's.
    let w = |r: &StoredRecord| r.fields.get("w").and_then(Value::as_u64);
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

#[test]
fn a_page_in_a_fields_order_under_an_exact_text_is_handed_its_page() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("text-order-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let engine = SqliteOpener
        .open(&Location::sqlite("texts", &dir), OpenMode::Create)
        .unwrap();
    walk_under_each_text(engine.as_ref());
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}

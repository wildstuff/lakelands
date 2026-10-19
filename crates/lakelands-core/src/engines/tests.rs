use super::*;
use crate::conformance::fixture::order;
use serde_json::json;

#[test]
fn numbers_compare_numerically_even_as_text() {
    assert_eq!(cmp_values(&json!(9), &json!("10")), Ordering::Less);
    assert_eq!(cmp_values(&json!("1.0"), &json!(1)), Ordering::Equal);
    assert_eq!(cmp_values(&json!(2.5), &json!(-3)), Ordering::Greater);
}

/// Values of every kind the rulebook ranks, with equal values written
/// differently: scales, numbers as text, one instant in two offsets,
/// JSON a float and an integer print the same in.
fn sort_corpus() -> Vec<serde_json::Value> {
    let mut values = vec![
        json!(0),
        json!(0.0),
        json!("0"),
        json!("-0.00"),
        json!(1),
        json!(1.0),
        json!("1.00"),
        json!("1"),
        json!(-1),
        json!(-1.5),
        json!("-1.55"),
        json!(-10),
        json!(10),
        json!(100),
        json!("100.5"),
        json!(0.001),
        json!("0.0010"),
        json!(1e20),
        json!(-1e-20),
        json!(1e300),
        json!(2.5),
        json!("-3"),
        json!("79228162514264337593543950335"),
        json!("-0.0000000000000000000000000001"),
        json!("2026-01-01"),
        json!("2026-01-01T00:00:00Z"),
        json!("2026-01-01T01:00:00+01:00"),
        json!("2025-12-31T23:59:59.999999Z"),
        json!("1969-07-20T20:17:00Z"),
        json!("abc"),
        json!("Abc"),
        json!("ab"),
        json!("ä"),
        json!(""),
        json!(" "),
        json!("1e3"),
        json!(format!("{}b", "long prefix ".repeat(4))),
        json!(format!("{}a", "long prefix ".repeat(4))),
        json!(format!("{}c", "long prefix ".repeat(4))),
        json!("long prefix ".repeat(4)),
        json!(false),
        json!(true),
        json!({"a": 1}),
        json!({"a": 1.0}),
        json!([1, 2]),
        json!([]),
        json!(null),
    ];
    // Decimals of many scales and both signs, as numbers and as text.
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    for _ in 0..300 {
        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let mantissa = (x >> 20) % 2_000_001;
        let scale = (x >> 8) % 7;
        let sign = if x & 1 == 0 { "" } else { "-" };
        let text = format!(
            "{sign}{}.{:0>width$}",
            mantissa / 10u64.pow(scale as u32),
            mantissa % 10u64.pow(scale as u32),
            width = scale as usize
        );
        let text = text.trim_end_matches('.').to_owned();
        values.push(json!(text));
        if let Ok(n) = text.parse::<f64>() {
            values.push(json!(n));
        }
    }
    values
}

#[test]
fn sort_keys_order_as_cmp_values_and_are_equal_where_it_is() {
    let values = sort_corpus();
    for a in &values {
        for b in &values {
            assert_eq!(
                sort_key(a).cmp(&sort_key(b)),
                cmp_values(a, b),
                "the sort keys of {a} and {b} order them otherwise than cmp_values"
            );
        }
    }
}

#[test]
fn a_scan_sorted_by_keys_answers_what_cmp_places_orders() {
    let records: Vec<StoredRecord> = sort_corpus()
        .into_iter()
        .enumerate()
        .map(|(i, v)| StoredRecord {
            ty: TypeName::from("t"),
            key: RecordKey::from(format!("k{:04}", (i * 37) % 1000)),
            version: lakelands_api::Version(1),
            fields: [(FieldName::from("v"), v)].into_iter().collect(),
            meta: BTreeMap::new(),
            position: lakelands_api::Position(1),
            made: lakelands_api::Position(1),
            retired: false,
            restricted: false,
        })
        .collect();
    for dir in [Direction::Asc, Direction::Desc] {
        let order_by = vec![order("v", OrderBy::Value, dir)];
        let mut expected: Vec<&StoredRecord> = records.iter().collect();
        expected.sort_by(|a, b| {
            cmp_places(
                &Place::of(a, &order_by),
                &Place::of(b, &order_by),
                &order_by,
            )
        });
        let got = scan(
            None,
            &records,
            &Scan {
                order_by: order_by.clone(),
                limit: u32::MAX,
                ..Default::default()
            },
        );
        let keys = |rows: Vec<&StoredRecord>| -> Vec<String> {
            rows.into_iter().map(|r| r.key.to_string()).collect()
        };
        assert_eq!(keys(got.iter().collect()), keys(expected), "{dir:?}");
    }
}

/// Walks `scan` page by page through [`scan_in_field_order`] over an
/// index of `records` kept in a vector, continuing after each page's last
/// row; with `changed`, through [`scan_in_field_order_at`], `records`
/// being the head.
fn walk_by_index(
    decl: &TypeDecl,
    records: &[StoredRecord],
    changed: Option<&Changed>,
    scan: &Scan,
) -> Vec<String> {
    let first = &scan.order_by[0];
    let mut index: Vec<(Vec<u8>, &StoredRecord)> = records
        .iter()
        .filter_map(|r| {
            order_rows(
                &sorted_fields(&decl.name, decl, Some(&json!(["v", "w"]))),
                r,
            )
            .into_iter()
            .find(|(f, _)| **f == first.field)
            .map(|(_, k)| (k, r))
        })
        .collect();
    index.sort_by(|a, b| match first.dir {
        Direction::Asc => (&a.0, &a.1.key).cmp(&(&b.0, &b.1.key)),
        Direction::Desc => b.0.cmp(&a.0).then(a.1.key.cmp(&b.1.key)),
    });
    let position = |k: &[u8], key: &str| -> (Vec<u8>, String) { (k.to_vec(), key.to_owned()) };
    let past = |row: &(Vec<u8>, &StoredRecord), from: &(Vec<u8>, String)| match first.dir {
        Direction::Asc => position(&row.0, row.1.key.as_str()) > *from,
        Direction::Desc => {
            row.0 < from.0 || (row.0 == from.0 && row.1.key.as_str() > from.1.as_str())
        }
    };
    let mut missing: Vec<&StoredRecord> = records
        .iter()
        .filter(|r| r.fields.get(&first.field).is_none_or(|v| v.is_null()))
        .collect();
    missing.sort_by(|a, b| a.key.cmp(&b.key));
    let mut keys = Vec::new();
    let mut page = scan.clone();
    loop {
        let sorted = |from: Option<(&[u8], &str)>, n: u32| {
            let from = from.map(|(k, r)| position(k, r));
            Ok(index
                .iter()
                .filter(|row| from.as_ref().is_none_or(|f| past(row, f)))
                .take(n as usize)
                .map(|(k, r)| (k.clone(), (*r).clone()))
                .collect())
        };
        let unsorted = |after: Option<&str>, n: u32| {
            Ok(missing
                .iter()
                .filter(|r| after.is_none_or(|a| r.key.as_str() > a))
                .take(n as usize)
                .map(|r| (*r).clone())
                .collect())
        };
        let rows: Vec<StoredRecord> = match changed {
            None => scan_in_field_order::<()>(Some(decl), &page, sorted, unsorted),
            Some(c) => scan_in_field_order_at::<()>(Some(decl), &page, c, sorted, unsorted),
        }
        .unwrap();
        let Some(last) = rows.last() else { break };
        page.after = Some(After {
            values: page
                .order_by
                .iter()
                .map(|o| sort_value(last, o).unwrap_or(Value::Null))
                .collect(),
            key: last.key.clone(),
        });
        keys.extend(rows.iter().map(|r| r.key.to_string()));
    }
    keys
}

#[test]
fn a_walk_in_index_order_answers_what_a_whole_sort_answers() {
    let decl: TypeDecl = serde_json::from_value(json!({
        "name": "t", "fields": [
            {"name": "v", "type": {"kind": "text"}},
            {"name": "w", "type": {"kind": "text"}}
        ]
    }))
    .unwrap();
    let corpus = sort_corpus();
    let records: Vec<StoredRecord> = (0..corpus.len() + 40)
        .map(|i| {
            let mut fields = BTreeMap::new();
            // Every fifth record misses `v`; `w` has few values, so
            // two fields make runs.
            if i % 5 != 0 {
                fields.insert(FieldName::from("v"), corpus[i % corpus.len()].clone());
            }
            if i % 7 != 0 {
                fields.insert(FieldName::from("w"), json!(format!("w{}", i % 3)));
            }
            StoredRecord {
                ty: TypeName::from("t"),
                key: RecordKey::from(format!("k{:04}", (i * 37) % 1009)),
                version: lakelands_api::Version(1),
                fields,
                meta: BTreeMap::new(),
                position: lakelands_api::Position(1),
                made: lakelands_api::Position(1),
                retired: i % 11 == 0,
                restricted: false,
            }
        })
        .collect();
    let order = |field: &str, dir| order(field, OrderBy::Value, dir);
    for dir in [Direction::Asc, Direction::Desc] {
        for order_by in [
            vec![order("v", dir)],
            vec![order("w", dir), order("v", Direction::Asc)],
            vec![order("v", dir), order("w", Direction::Desc)],
        ] {
            for filter in [None, Some(crate::expr::parse("w != 'w1'").unwrap())] {
                let scan = Scan {
                    order_by: order_by.clone(),
                    filter: filter.clone(),
                    limit: 3,
                    ..Default::default()
                };
                let whole: Vec<String> = super::scan(
                    Some(&decl),
                    &records,
                    &Scan {
                        limit: u32::MAX,
                        ..scan.clone()
                    },
                )
                .into_iter()
                .map(|r| r.key.to_string())
                .collect();
                assert_eq!(
                    walk_by_index(&decl, &records, None, &scan),
                    whole,
                    "{order_by:?} with {filter:?}"
                );
            }
        }
    }
}

#[test]
fn a_walk_at_an_earlier_position_answers_what_a_whole_sort_there_answers() {
    let decl: TypeDecl = serde_json::from_value(json!({
        "name": "t", "fields": [
            {"name": "v", "type": {"kind": "text"}},
            {"name": "w", "type": {"kind": "text"}}
        ]
    }))
    .unwrap();
    let corpus = sort_corpus();
    let record = |i: usize, v: Option<serde_json::Value>, retired: bool| {
        let mut fields = BTreeMap::new();
        if let Some(v) = v {
            fields.insert(FieldName::from("v"), v);
        }
        fields.insert(FieldName::from("w"), json!(format!("w{}", i % 3)));
        StoredRecord {
            ty: TypeName::from("t"),
            key: RecordKey::from(format!("k{:04}", (i * 37) % 1009)),
            version: lakelands_api::Version(1),
            fields,
            meta: BTreeMap::new(),
            position: lakelands_api::Position(1),
            made: lakelands_api::Position(1),
            retired,
            restricted: false,
        }
    };
    let n = corpus.len() + 40;
    let value = |i: usize| (!i.is_multiple_of(5)).then(|| corpus[i % corpus.len()].clone());
    // At the position: every record but the last ten.
    let at: Vec<StoredRecord> = (0..n - 10).map(|i| record(i, value(i), false)).collect();
    // At the head: every third changed its `v` (some lose it, some gain
    // one), every eleventh retired, and ten records written after.
    let head: Vec<StoredRecord> = (0..n)
        .map(|i| match i {
            _ if i >= n - 10 => record(i, value(i), false),
            _ if i.is_multiple_of(3) => record(i, value(i + 7), false),
            _ if i.is_multiple_of(11) => record(i, value(i), true),
            _ => record(i, value(i), false),
        })
        .collect();
    let keys: BTreeSet<RecordKey> = (0..n)
        .filter(|i| *i >= n - 10 || i.is_multiple_of(3) || i.is_multiple_of(11))
        .map(|i| record(i, None, false).key)
        .collect();
    let changed = Changed {
        states: at
            .iter()
            .filter(|r| keys.contains(&r.key))
            .cloned()
            .collect(),
        keys,
    };
    let order = |field: &str, dir| order(field, OrderBy::Value, dir);
    for dir in [Direction::Asc, Direction::Desc] {
        for order_by in [
            vec![order("v", dir)],
            vec![order("v", dir), order("w", Direction::Desc)],
        ] {
            for retired in [false, true] {
                let scan = Scan {
                    order_by: order_by.clone(),
                    retired,
                    limit: 3,
                    ..Default::default()
                };
                let whole: Vec<String> = super::scan(
                    Some(&decl),
                    &at,
                    &Scan {
                        limit: u32::MAX,
                        ..scan.clone()
                    },
                )
                .into_iter()
                .map(|r| r.key.to_string())
                .collect();
                assert_eq!(
                    walk_by_index(&decl, &head, Some(&changed), &scan),
                    whole,
                    "{order_by:?}, retired {retired}"
                );
            }
        }
    }
}

#[test]
fn only_text_that_compares_as_text_narrows() {
    let term = |s: &str| exact_text_term(&crate::expr::parse(s).unwrap());
    assert_eq!(
        term("status = 'open'"),
        Some(("status".into(), "open".into()))
    );
    assert_eq!(
        term("n > 1 and 'open' = status"),
        Some(("status".into(), "open".into()))
    );
    assert_eq!(term("status = '10'"), None, "compares as a number");
    assert_eq!(
        term("at = '2026-01-01T00:00:00Z'"),
        None,
        "compares as an instant"
    );
    assert_eq!(term("status = 'a' or status = 'b'"), None);
}

/// `TopK` answers what `rank` answers over the whole stream, to the
/// bit: random corpora with few distinct scores (so ties straddle the
/// cut), both zeros, keys offered in any order, every `k` from none
/// to more than the corpus, and `u32::MAX`.
#[test]
fn top_k_answers_what_rank_answers() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        // splitmix64
        seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    for _ in 0..500 {
        let n = (next() % 60) as usize;
        let distinct = 1 + next() % 6;
        let scores = [-1.0, -0.0, 0.0, 0.25, 0.5, 1.0 / 3.0, 1.0];
        let corpus: Vec<(RecordKey, f64)> = (0..n)
            .map(|i| {
                let score = if next().is_multiple_of(4) {
                    f64::from_bits(next() >> 2)
                } else {
                    scores[(next() % distinct) as usize]
                };
                (RecordKey::from(format!("{:02x}-{i}", next() % 16)), score)
            })
            .collect();
        let ks = [0, 1, 2, (n / 2) as u32, n as u32, n as u32 + 3, u32::MAX];
        for k in ks.into_iter().chain([(next() % 64) as u32]) {
            let mut reference = corpus.clone();
            rank(&mut reference, k);
            let mut top = TopK::new(k);
            for (key, score) in &corpus {
                top.push(key, *score);
            }
            let got = top.into_ranked();
            let bits = |v: &[(RecordKey, f64)]| {
                v.iter()
                    .map(|(key, s)| (key.clone(), s.to_bits()))
                    .collect::<Vec<_>>()
            };
            assert_eq!(bits(&got), bits(&reference), "k = {k} over {corpus:?}");
        }
    }
}

/// A record's state keeps one block for the fields that share it, and
/// reads back every field's: all one block, a tie, a block with no value,
/// a value with no block beside a base, an authored field among source
/// ones, no meta at all; a state of the form before it reads the same.
#[test]
fn a_state_keeps_shared_provenance_once_and_reads_every_fields_back() {
    let record = |fields: serde_json::Value, meta: serde_json::Value| -> StoredRecord {
        let r = json!({"type": "t", "key": "k", "version": 1, "fields": fields, "meta": meta, "position": 9, "made": 3});
        serde_json::from_value(r).unwrap()
    };
    let block = |at: u64, origin: &str| json!({"origin": origin, "known_at": format!("2026-01-01T00:00:0{at}Z"), "position": at, "writer": "w"});
    let (a, b) = (block(1, "source"), block(2, "authored"));
    let failed = json!({"error": "down", "origin": "derived", "known_at": "2026-01-01T00:00:03Z", "position": 3, "writer": "w"});
    let (xy, xyz) = (json!({"x": 1, "y": 1}), json!({"x": 1, "y": 1, "z": 1}));
    let cases = [
        record(xyz.clone(), json!({"x": a, "y": a, "z": a})),
        record(xy.clone(), json!({"x": a, "y": b})),
        record(xy.clone(), json!({"x": a, "y": a, "c": failed})),
        record(xy.clone(), json!({"x": a, "y": a, "c": a})),
        record(json!({"x": 1, "y": 1, "bare": 1}), json!({"x": a, "y": a})),
        record(xyz.clone(), json!({"x": a, "y": b, "z": a})),
        record(json!({"x": 1}), json!({})),
    ];
    for r in &cases {
        let state = record_state(r);
        assert_eq!(&decode_record(&state).unwrap(), r, "{state}");
        assert_eq!(
            &decode_record(&serde_json::to_string(r).unwrap()).unwrap(),
            r
        );
    }
    let shared = record_state(&cases[0]);
    assert_eq!(
        shared.matches("\"origin\"").count(),
        1,
        "one block for three fields: {shared}"
    );
}

#[test]
fn a_commit_decides_again_only_the_types_its_puts_move_and_as_the_whole_decides() {
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    let decl = |v: serde_json::Value| -> Declaration { serde_json::from_value(v).unwrap() };
    let ty = |n: &str, fields: &[&str]| {
        let fields: Vec<_> = fields.iter().map(|f| text(f)).collect();
        decl(json!({"declare": "type", "name": n, "fields": fields}))
    };
    let lake = [
        ty("t", &["a", "b"]),
        ty("u", &["c", "d"]),
        ty("w", &["e", "f"]),
    ];
    let set = |of: &str, filter: &str| {
        decl(json!({"declare": "object_set", "name": "s", "of": of, "where": filter}))
    };
    let setting = StoredRecord {
        ty: lakelands_api::seam::SETTING_TYPE.into(),
        key: "lookup_index.w".into(),
        version: lakelands_api::id::Version(1),
        fields: serde_json::from_value(json!({"key": "lookup_index.w", "value": ["f"]})).unwrap(),
        meta: Default::default(),
        position: Position(1),
        made: Position(1),
        retired: false,
        restricted: false,
    };
    // Each moves the index of a type other than the one it declares, or,
    // declared again, of the one its last declaration named.
    let steps: Vec<(&str, Vec<Declaration>, bool)> = vec![
        ("a set's type", vec![set("t", "a = 'x'")], false),
        (
            "the set declared again over another type",
            vec![set("w", "e = 'x'")],
            false,
        ),
        (
            "a derived relation's target",
            vec![decl(json!({
            "declare": "type", "name": "p", "fields": [text("k")],
            "relations": [{"name": "hits", "target": "u", "cardinality": "many_to_many",
                           "derived": {"match": [{"from": "k", "to": "c"}]}}]}))],
            false,
        ),
        (
            "a view's source and its merge's",
            vec![decl(json!({
            "declare": "type", "name": "v", "key": ["d"], "fields": [text("d")],
            "view": {"source": "u", "where": "d = 'y'", "group_by": ["d"], "measures": [],
                     "merge": [{"source": "t", "where": "b = 'z'", "group_by": ["b"],
                                "measures": []}]}}))],
            false,
        ),
        ("a setting", vec![], true),
        (
            "a value type",
            vec![decl(
                json!({"declare": "value_type", "name": "x", "kind": "text"}),
            )],
            false,
        ),
    ];
    let mut decls: Vec<Declaration> = lake.to_vec();
    let mut heads: Heads = (decls.iter())
        .map(|d| (declaration_key(d), d.clone()))
        .collect();
    let looked = |decls: &[Declaration], asked: bool| -> BTreeSet<(String, String)> {
        let types = decls.iter().filter_map(|d| match d {
            Declaration::Type(t) => Some(t.name.clone()),
            _ => None,
        });
        let ours = |t: &TypeName| asked && t.as_str() == "w";
        let value = setting_value(&setting);
        (types.flat_map(|t| {
            let fields = looked_up(&t, decls, value.filter(|_| ours(&t)));
            fields
                .into_iter()
                .flatten()
                .map(move |f| (t.to_string(), f.to_string()))
        }))
        .collect()
    };
    let mut asked = false;
    for (what, put, ask) in steps {
        // What the engine keeps: the whole decision before this commit.
        let was = looked(&decls, asked);
        asked |= ask;
        let answers = |key: &str| {
            let ours = asked && key == "lookup_index.w";
            Ok(ours.then(|| setting.clone()))
        };
        let mut relook = Relook::default();
        put.iter().for_each(|d| relook.declared(d));
        if ask {
            relook.record(&setting);
        }
        for d in &put {
            decls.retain(|x| declaration_key(x) != declaration_key(d));
            decls.push(d.clone());
        }
        let (after, moved) = relook.heads(Some(heads), || unreachable!()).unwrap();
        // The engine reads what it keeps of the moved types alone.
        let of_moved = |(t, _): &&(String, String)| moved.iter().flatten().any(|m| m.as_str() == t);
        let kept = was.iter().filter(of_moved).cloned().collect();
        let narrow = relooked(&after, moved.as_ref(), &kept, answers).unwrap();
        let whole = relooked(&after, None, &was, answers).unwrap();
        assert_eq!(narrow, whole, "{what}: the moved types {moved:?}");
        // Each but the value type moves something, so a type left out of
        // what it moves would show here.
        let moves = put.iter().all(|d| !matches!(d, Declaration::ValueType(_)));
        assert_eq!(
            !whole.is_empty(),
            moves,
            "{what} moves the index: {whole:?}"
        );
        heads = after;
    }
}

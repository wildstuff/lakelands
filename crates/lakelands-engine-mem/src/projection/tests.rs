use super::*;
use lakelands_api::id::Version;
use lakelands_core::conformance::fixture::stored_record;
use serde_json::json;

fn record(key: &str, n: i64, position: u64) -> StoredRecord {
    stored_record(
        "t",
        key,
        [("n".into(), json!(n))].into_iter().collect(),
        position,
    )
}

#[test]
fn a_history_answers_the_latest_state_at_or_before_a_position() {
    let mut h = History::default();
    h.put(Position(5), "five");
    h.put(Position(2), "two");
    h.put(Position(9), "nine");
    assert_eq!(h.at(Some(Position(1))), None);
    assert_eq!(h.at(Some(Position(2))), Some(&"two"));
    assert_eq!(h.at(Some(Position(8))), Some(&"five"));
    assert_eq!(h.at(Some(Position(100))), Some(&"nine"));
    assert_eq!(h.at(None), None, "an empty projection shows nothing");
    h.put(Position(5), "five again");
    assert_eq!(h.at(Some(Position(5))), Some(&"five again"));
}

#[test]
fn the_setting_keeps_the_rows_of_the_fields_it_names_and_no_others() {
    let mut p = Projection::default();
    let decl: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "t",
        "fields": [
            {"name": "n", "type": {"kind": "number"}},
            {"name": "m", "type": {"kind": "number"}}
        ]
    }))
    .unwrap();
    let setting = |fields, at, retired| StoredRecord {
        version: Version(at),
        retired,
        ..stored_record(
            lakelands_api::seam::SETTING_TYPE,
            "sort_index.t",
            [
                ("key".into(), json!("sort_index.t")),
                ("value".into(), fields),
            ]
            .into_iter()
            .collect(),
            at,
        )
    };
    let mut puts = vec![Put::Declaration(Declared {
        declaration: decl,
        version: 1,
        schema_hash: None,
        since: Position(1),
    })];
    puts.extend((0..10).map(|i| {
        let mut r = record(&format!("k{i}"), i, 1);
        r.fields.insert("m".into(), json!(-i));
        Put::Record(r)
    }));
    puts.push(Put::Record(setting(json!(["n", "m"]), 1, false)));
    p.commit(puts, Position(1)).unwrap();
    let rows = |p: &Projection, field: &str| {
        p.sorted
            .get(&(TypeName::from("t"), FieldName::from(field)))
            .map_or(0, BTreeSet::len)
    };
    assert_eq!((rows(&p, "n"), rows(&p, "m")), (10, 10));
    p.commit(
        vec![Put::Record(setting(json!(["n"]), 2, false))],
        Position(2),
    )
    .unwrap();
    assert_eq!(
        (rows(&p, "n"), rows(&p, "m")),
        (10, 0),
        "the setting's write without `m` drops the rows of `m`"
    );
    p.commit(
        vec![Put::Record(setting(json!(["n"]), 3, true))],
        Position(3),
    )
    .unwrap();
    assert_eq!(
        (rows(&p, "n"), rows(&p, "m")),
        (0, 0),
        "the setting retired drops every row"
    );
}

#[test]
fn a_commit_keeps_the_superseded_record_state() {
    let mut p = Projection::default();
    p.commit(vec![Put::Record(record("a", 1, 1))], Position(1))
        .unwrap();
    p.commit(vec![Put::Record(record("a", 2, 3))], Position(3))
        .unwrap();
    let ty = TypeName::from("t");
    let at = |n| {
        p.records_at(&ty, Some(Position(n)))
            .next()
            .map(|r| r.fields["n"].clone())
    };
    assert_eq!(at(0), None);
    assert_eq!(at(2), Some(json!(1)));
    assert_eq!(at(3), Some(json!(2)));
    assert_eq!(p.folded, Some(Position(3)));
}

#[test]
fn a_range_bounds_the_key_walk_and_a_crossed_one_answers_none() {
    let mut p = Projection::default();
    let puts = ["k1", "k2", "k3", "k4", "k5"]
        .into_iter()
        .map(|k| Put::Record(record(k, 1, 1)))
        .collect();
    p.commit(puts, Position(1)).unwrap();
    let ty = TypeName::from("t");
    let range = |from: Option<&str>, until: Option<&str>| KeyRange {
        from: from.map(RecordKey::from),
        until: until.map(RecordKey::from),
    };
    let keys = |range: &KeyRange| -> Vec<String> {
        p.records_after(&ty, Some(Position(1)), None, Some(range), 100)
            .into_iter()
            .map(|r| r.key.to_string())
            .collect()
    };
    assert_eq!(keys(&range(Some("k2"), Some("k4"))), ["k2", "k3"]);
    assert_eq!(
        keys(&range(None, Some("k3"))),
        ["k1", "k2"],
        "from is inclusive, until exclusive"
    );
    assert_eq!(keys(&range(Some("k2"), None)), ["k2", "k3", "k4", "k5"]);
    assert_eq!(keys(&range(None, None)).len(), 5, "either end open");
    assert!(
        keys(&range(Some("k4"), Some("k3"))).is_empty(),
        "a from past the until answers none"
    );
    assert!(
        keys(&range(Some("k3"), Some("k3"))).is_empty(),
        "the until itself is outside"
    );
    let after: Vec<String> = p
        .records_after(
            &ty,
            Some(Position(1)),
            Some("k3"),
            Some(&range(Some("k2"), None)),
            100,
        )
        .into_iter()
        .map(|r| r.key.to_string())
        .collect();
    assert_eq!(after, ["k4", "k5"], "the later of `after` and the from");
}

#[test]
fn a_commit_behind_the_projection_or_with_a_later_state_changes_nothing() {
    let mut p = Projection::default();
    p.commit(vec![], Position(4)).unwrap();
    assert!(
        p.commit(vec![Put::Record(record("a", 1, 2))], Position(3))
            .is_err()
    );
    assert!(
        p.commit(vec![Put::Record(record("a", 1, 9))], Position(5))
            .is_err()
    );
    assert!(p.records.is_empty());
    assert_eq!(p.folded, Some(Position(4)));
}

#[test]
fn an_output_is_valid_from_its_own_event_not_the_commit() {
    let mut p = Projection::default();
    p.commit(
        vec![Put::Output {
            seam: "fn".into(),
            input: Digest::zero(),
            output: json!(7),
            at: Position(4),
        }],
        Position(6),
    )
    .unwrap();
    let h = &p.outputs[&("fn".to_owned(), Digest::zero())];
    assert_eq!(h.at(Some(Position(3))), None);
    assert_eq!(h.at(Some(Position(4))), Some(&json!(7)));
}

fn meta(writer: &str, at: u64) -> lakelands_api::record::FieldMeta {
    lakelands_api::record::FieldMeta {
        upstream: None,
        origin: lakelands_api::record::Origin::Authored,
        known_at: lakelands_api::time::Timestamp(7),
        seen_at: None,
        position: Position(at),
        writer: writer.into(),
        source: None,
        procedure: None,
        procedure_version: None,
        error: None,
        erased: false,
        on_behalf_of: None,
    }
}

/// Eight fields written by one event, a ninth by another, a vector whose
/// elements are not `f32`s, one that are, and arrays that are no vector.
fn wide() -> StoredRecord {
    let mut r = record("a", 1, 4);
    let vectors = [
        ("double", json!([0.1, -2.5e-8, 3, 0])),
        ("single", json!([0.5, 0.25, 1.5])),
        ("mixed", json!([1, "x", 2])),
        ("empty", json!([])),
    ];
    for (f, v) in vectors {
        r.fields.insert(f.into(), v);
    }
    for i in 0..8 {
        r.fields
            .insert(format!("f{i}").into(), json!(format!("value {i}")));
    }
    for f in r.fields.keys() {
        r.meta.insert(f.clone(), meta("human:long-writer-name", 4));
    }
    r.meta.insert("f3".into(), meta("human:other", 2));
    r.meta.insert("gone".into(), meta("human:gone", 1));
    r.restricted = true;
    r
}

#[test]
fn a_held_record_reads_back_whole() {
    let (r, mut bases) = (wide(), Bases::default());
    assert_eq!(Held::of(&r, &mut bases).record(&bases), r, "inline");
    let base = engines::base_of(&r).unwrap();
    bases.2.insert(base);
    let held = Held::of(&r, &mut bases);
    assert!(held.base.is_some());
    assert_eq!(held.record(&bases), r, "its base a row");
    let ended = StoredRecord::ended("t".into(), "k".into(), Position(9));
    assert_eq!(Held::of(&ended, &mut bases).record(&bases), ended);
}

#[test]
fn a_held_record_shares_its_provenance_and_keeps_vectors_as_elements() {
    let mut bases = Bases::default();
    let held = Held::of(&wide(), &mut bases);
    let blocks = bases.0.values().map(String::as_str);
    let kept = blocks.chain([&*held.state]).collect::<String>();
    assert_eq!(kept.matches("long-writer-name").count(), 1);
    assert_eq!(
        held.state.matches("0.1").count(),
        0,
        "a vector is not in the state"
    );
    let apart: Vec<&str> = held.apart.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(apart, ["double", "single"]);
}

/// `key`'s record at `at`, its one field written by `source` then.
fn sourced(key: &str, source: &str, at: u64) -> StoredRecord {
    let mut r = record(key, 1, at);
    let mut m = meta("human:owner", at);
    (m.source, m.known_at) = (
        Some(source.into()),
        lakelands_api::time::Timestamp(at as i64),
    );
    r.meta.insert("n".into(), m);
    r
}

#[test]
fn a_base_is_a_row_where_a_write_shares_it_and_goes_once_no_state_names_it() {
    use lakelands_core::engines::SHARED;
    let write = |n: usize, source: &str, at: u64| -> Vec<Put> {
        (0..n)
            .map(|i| Put::Record(sourced(&format!("{source}{at}.{i}"), source, at)))
            .collect()
    };
    let mut p = Projection::default();
    p.commit(write(SHARED, "crm", 1), Position(1)).unwrap();
    p.commit(write(SHARED - 1, "erp", 2), Position(2)).unwrap();
    p.commit(write(1, "crm", 3), Position(3)).unwrap();
    let named = |p: &Projection| {
        let states = p.records.values().flat_map(|h| &h.0);
        states.filter(|(_, held)| held.base.is_some()).count()
    };
    assert_eq!(p.bases.0.len(), 1, "a row for the shared write's base");
    assert_eq!(named(&p), SHARED, "the others keep theirs inline");
    let ty = TypeName::from("t");
    let read = |p: &Projection| {
        let records = p.records_at(&ty, Some(Position(3)));
        let sources = records.map(|r| r.meta["n"].source.clone().unwrap().to_string());
        sources.collect::<Vec<_>>()
    };
    let crm = |n| std::iter::repeat_n("crm", n);
    let mut sources: Vec<_> = (crm(SHARED).chain(std::iter::repeat_n("erp", SHARED - 1)))
        .chain(crm(1))
        .collect();
    sources.sort();
    let mut all = read(&p);
    all.sort();
    assert_eq!(all, sources, "every record reads its base");
    let dropped = |keys: std::ops::Range<usize>| -> Vec<Put> {
        let mut puts: Vec<_> =
            (keys.map(|i| Put::DroppedRecord(ty.clone(), format!("crm1.{i}").into()))).collect();
        puts.push(Put::DroppedBases);
        puts
    };
    p.commit(dropped(0..SHARED - 1), Position(3)).unwrap();
    assert_eq!(p.bases.0.len(), 1, "a base a state names stays");
    p.commit(dropped(SHARED - 1..SHARED), Position(3)).unwrap();
    assert!(p.bases.0.is_empty(), "a base no state names goes");
    assert_eq!(read(&p).len(), SHARED, "the inline ones read theirs");
}

/// A book's series state and the rows of its derived field.
fn series_projection() -> (Projection, TypeName, Declaration) {
    let decl: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "book",
        "fields": [
            {"name": "series", "type": {"kind": "text"}},
            {"name": "at", "type": {"kind": "number"}}
        ]
    }))
    .unwrap();
    let ty = TypeName::from("book");
    let mut p = Projection::default();
    let books = [
        ("k1", json!({"series": "red", "at": 2})),
        ("k2", json!({"series": "red", "at": 1})),
        ("k3", json!({"series": "blue", "at": 3})),
        ("k4", json!({"series": "red"})),
    ];
    let mut puts = vec![Put::Declaration(Declared {
        declaration: decl.clone(),
        version: 1,
        schema_hash: None,
        since: Position(1),
    })];
    puts.extend(books.into_iter().map(|(key, fields)| {
        Put::Record(stored_record(
            "book",
            key,
            serde_json::from_value(fields).unwrap(),
            1,
        ))
    }));
    puts.push(Put::Record(series_setting(json!([["series", "at"]]), 1)));
    p.commit(puts, Position(1)).unwrap();
    (p, ty, decl)
}

fn series_setting(value: Value, at: u64) -> StoredRecord {
    StoredRecord {
        version: Version(at),
        ..stored_record(
            lakelands_api::seam::SETTING_TYPE,
            "series_index.book",
            [
                ("key".into(), json!("series_index.book")),
                ("value".into(), value),
            ]
            .into_iter()
            .collect(),
            at,
        )
    }
}

fn series_row(field: &FieldName) -> (TypeName, FieldName) {
    (TypeName::from("book"), field.clone())
}

#[test]
fn a_series_pair_is_filled_from_the_log_kept_and_put_back_to_start() {
    let (mut p, ty, decl) = series_projection();
    let field = FieldName::from(lakelands_api::seam::series_field("at", "series"));
    let rows = |p: &Projection| p.sorted.get(&series_row(&field)).map_or(0, BTreeSet::len);
    let pair = SeriesPair {
        series: FieldName::from("series"),
        order: FieldName::from("at"),
    };
    assert_eq!(rows(&p), 4, "a row a book, the one with no order value too");
    assert_eq!(
        p.series_state(&ty),
        SeriesState {
            kept: vec![pair.clone()],
            building: Vec::new(),
        }
    );
    let key_of = |p: &Projection, key: &str| -> Vec<u8> {
        p.sorted_rows
            .get(&(ty.clone(), RecordKey::from(key)))
            .and_then(|rows| rows.iter().find(|(f, _)| f == &field))
            .map(|(_, k)| k.clone())
            .expect("the row of the key")
    };
    assert_eq!(key_of(&p, "k1")[..8], key_of(&p, "k2")[..8]);
    assert_ne!(key_of(&p, "k1")[..8], key_of(&p, "k3")[..8]);
    assert_eq!(key_of(&p, "k4")[8..], [0xFF]);

    // Taken back a chunk a call, the pair leaves with its last chunk.
    let mut after = None;
    loop {
        let step = Choose::DropChunk {
            field: field.clone(),
            after,
            limit: 2,
        };
        match p.choose(&ty, &step) {
            Some(last) => after = Some(last),
            None => break,
        }
    }
    assert_eq!(rows(&p), 0);
    assert_eq!(p.series_state(&ty), SeriesState::default());

    // Folded from the log again, it is whole again; a redeclaration puts
    // it back to `Start`.
    p.commit(
        vec![Put::Record(series_setting(json!([["series", "at"]]), 2))],
        Position(2),
    )
    .unwrap();
    assert_eq!(rows(&p), 4);
    assert_eq!(p.series_state(&ty).kept, vec![pair.clone()]);
    p.commit(
        vec![Put::Declaration(Declared {
            declaration: decl,
            version: 2,
            schema_hash: None,
            since: Position(3),
        })],
        Position(3),
    )
    .unwrap();
    assert!(p.series_state(&ty).kept.is_empty());
    assert_eq!(p.series_state(&ty).building, vec![pair.clone()]);

    // A build beside the writer fills it and keeps it.
    p.choose(&ty, &Choose::Start(field.clone()));
    let mut after = None;
    loop {
        let step = Choose::Fill {
            field: field.clone(),
            after,
            limit: 3,
        };
        match p.choose(&ty, &step) {
            Some(last) => after = Some(last),
            None => break,
        }
    }
    p.choose(&ty, &Choose::Keep(field.clone()));
    assert_eq!(p.series_state(&ty).kept, vec![pair]);
    assert_eq!(rows(&p), 4);

    // A setting put beside the writer takes its pair back in that write,
    // and the pair's rows go with it.
    p.commit(vec![Put::Live(series_setting(Value::Null, 4))], Position(4))
        .unwrap();
    assert_eq!(p.series_state(&ty), SeriesState::default());
    assert_eq!(rows(&p), 0, "a pair taken back leaves no row");
}

/// A setting committed before the declaration it names starts its pair at
/// the declaration, and the declaration's reindex fills its rows there.
#[test]
fn a_setting_put_before_the_declaration_starts_its_pair() {
    let decl: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "book",
        "fields": [
            {"name": "series", "type": {"kind": "text"}},
            {"name": "at", "type": {"kind": "number"}}
        ]
    }))
    .unwrap();
    let ty = TypeName::from("book");
    let field = FieldName::from(lakelands_api::seam::series_field("at", "series"));
    let mut p = Projection::default();
    p.commit(
        vec![Put::Record(series_setting(json!([["series", "at"]]), 1))],
        Position(1),
    )
    .unwrap();
    assert_eq!(p.series_state(&ty), SeriesState::default());
    let mut puts = vec![Put::Declaration(Declared {
        declaration: decl,
        version: 1,
        schema_hash: None,
        since: Position(2),
    })];
    puts.extend(
        [
            ("k1", json!({"series": "red", "at": 2})),
            ("k2", json!({"series": "red", "at": 1})),
            ("k3", json!({"series": "blue", "at": 3})),
            ("k4", json!({"series": "red"})),
        ]
        .into_iter()
        .map(|(key, fields)| {
            Put::Record(stored_record(
                "book",
                key,
                serde_json::from_value(fields).unwrap(),
                2,
            ))
        }),
    );
    p.commit(puts, Position(2)).unwrap();
    let rows = p.sorted.get(&series_row(&field)).map_or(0, BTreeSet::len);
    assert_eq!(rows, 4, "the declaration starts the pair and its rows");
    let state = p.series_state(&ty);
    assert!(state.kept.is_empty());
    assert_eq!(
        state.building,
        vec![SeriesPair {
            series: FieldName::from("series"),
            order: FieldName::from("at"),
        }],
        "the pair waits on its build"
    );
}

use super::*;
use lakelands_api::id::Version;
use serde_json::json;

fn record(key: &str, n: i64, position: u64) -> StoredRecord {
    StoredRecord {
        ty: TypeName::from("t"),
        key: RecordKey::from(key),
        version: Version(1),
        fields: [("n".into(), json!(n))].into_iter().collect(),
        meta: BTreeMap::new(),
        position: Position(position),
        made: Position(position),
        retired: false,
        restricted: false,
    }
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
        ty: TypeName::from(lakelands_api::seam::SETTING_TYPE),
        key: RecordKey::from("sort_index.t"),
        version: Version(at),
        fields: [
            ("key".into(), json!("sort_index.t")),
            ("value".into(), fields),
        ]
        .into_iter()
        .collect(),
        meta: BTreeMap::new(),
        position: Position(at),
        made: Position(at),
        retired,
        restricted: false,
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
    let r = wide();
    assert_eq!(Held::of(&r).record(), r);
    let ended = StoredRecord::ended("t".into(), "k".into(), Position(9));
    assert_eq!(Held::of(&ended).record(), ended);
}

#[test]
fn a_held_record_shares_its_provenance_and_keeps_vectors_as_elements() {
    let held = Held::of(&wide());
    assert_eq!(held.state.matches("long-writer-name").count(), 1);
    assert_eq!(
        held.state.matches("0.1").count(),
        0,
        "a vector is not in the state"
    );
    let apart: Vec<&str> = held.apart.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(apart, ["double", "single"]);
}

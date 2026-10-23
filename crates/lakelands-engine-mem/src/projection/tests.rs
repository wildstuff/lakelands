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

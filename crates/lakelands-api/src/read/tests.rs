use super::*;

#[test]
fn a_latest_key_carries_no_field_on_the_wire_and_every_other_names_one() {
    let latest: Order = serde_json::from_value(serde_json::json!({"by": "latest", "dir": "desc"}))
        .expect("a latest key reads without a field");
    assert!(latest.field.as_str().is_empty());
    assert_eq!(latest.misfit(), None);
    assert_eq!(
        serde_json::to_value(&latest).unwrap(),
        serde_json::json!({"by": "latest", "dir": "desc"}),
        "echoed back, a latest key names no field"
    );
    let named = Order {
        field: "amount".into(),
        ..latest.clone()
    };
    assert_eq!(named.misfit().map(|m| m.0), Some(Some("amount".into())));
    let unnamed = Order {
        by: OrderBy::KnownAt,
        ..latest
    };
    assert_eq!(unnamed.misfit().map(|m| m.0), Some(None));
}

/// Each request's JSON form with every member it takes, read and written
/// back the same; and a member it does not take, at the top and inside
/// every object at any depth, refused by name (lakelands#563). `params`,
/// `patch` and `payload` hold the names a type or a view declares, which
/// the store checks.
#[test]
fn a_request_reads_every_member_it_takes_and_refuses_any_other() {
    use crate::query::{Pattern, Search};
    use crate::seam::FeedFilter;
    use crate::write::{ProposalFilter, ProposedWrite, WriteOpts};
    use serde_json::{Value, json};
    let whole = json!([
        ["find", {"where": "n > 1", "order_by": [{"field": "n", "dir": "desc"}, {"by": "latest", "dir": "asc"}],
            "limit": 5, "cursor": "c", "offset": 2, "total": true, "with_meta": true, "params": {"p": 1},
            "as_of": 7, "valid_at": "2026-01-01T00:00:00.000000Z"}],
        ["find", {"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "n"}],
            "group_by": ["g", {"name": "m", "expr": "date_bucket(on, 'month')"}]},
            "after": {"values": [1], "key": "k"}, "with_meta": false}],
        ["get", {"key": "k", "with_meta": true, "params": {"p": 1}, "as_of": 7}],
        ["pattern", {"start": {"type": "t", "key": "k", "where": "n > 1", "params": {"p": 1},
            "search": {"mode": "lexical", "text": "a", "fields": ["t"]}},
            "steps": [{"relation": "r", "walk": "in", "type": "u", "where": "n > 1", "depth": {"min": 1, "max": 3}}],
            "to": ["u", "k2"], "project": {"as": "rows", "fields": ["n"]},
            "order_by": [{"field": "n", "dir": "asc"}], "limit": 5, "cursor": "c", "valid_at": "2026-01-01T00:00:00.000000Z"}],
        ["pattern", {"start": {"type": "t"}, "project": {"as": "aggregate", "measures": [], "group_by": ["g"]}}],
        ["pattern", {"start": {"type": "t"}, "project": {"as": "graph"}}],
        ["pattern", {"start": {"type": "t"}, "project": {"as": "degree"}}],
        ["pattern", {"start": {"type": "t"}, "project": {"as": "components"}}],
        ["pattern", {"start": {"type": "t"}, "project": {"as": "page_rank"}}],
        ["pattern", {"start": {"type": "t"}, "project": {"as": "cheapest", "weight": "w"}}],
        ["search", {"type": "t", "k": 5, "cursor": "c", "where": "n > 1", "with_meta": true, "as_of": 7,
            "mode": "lexical", "text": "a", "fields": ["t"]}],
        ["search", {"type": "t", "k": 5, "with_meta": false, "mode": "vector", "field": "v", "vector": [1.0],
            "weight": 0.5, "model": "m", "also": [{"field": "w", "vector": [2.0], "weight": 1.0, "model": "m"}]}],
        ["search", {"type": "t", "k": 5, "with_meta": false, "mode": "hybrid", "text": "a", "field": "v",
            "vector": [1.0], "fields": ["t"], "weight": 0.5, "text_weight": 0.5, "model": "m"}],
        ["write", {"expected_version": 3, "idempotency_key": "i"}],
        ["proposals", {"state": "pending", "type": "t", "kind": "upsert", "limit": 5,
            "order_by": [{"field": "position", "dir": "asc"}], "cursor": "c", "as_of": 7}],
        ["feed", {"kinds": ["record_upserted"], "types": ["t"], "keys": ["k"]}],
        ["proposed", {"write": "upsert", "type": "t", "key": "k", "patch": {"n": 1},
            "opts": {"expected_version": 3, "idempotency_key": "i"}}],
        ["proposed", {"write": "retire", "type": "t", "key": "k", "opts": {}}],
        ["proposed", {"write": "call", "verb": "v", "key": "k", "payload": {"n": 1}, "opts": {}}],
        ["proposed", {"write": "declare", "declarations": []}]
    ]);
    fn back<T: serde::Serialize + serde::de::DeserializeOwned>(v: &Value) -> Result<Value, String> {
        let read: T = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        Ok(serde_json::to_value(read).unwrap())
    }
    let read = |what: &Value, v: &Value| match what.as_str().unwrap() {
        "find" => back::<Find>(v),
        "get" => back::<Get>(v),
        "pattern" => back::<Pattern>(v),
        "search" => back::<Search>(v),
        "write" => back::<WriteOpts>(v),
        "proposals" => back::<ProposalFilter>(v),
        "proposed" => back::<ProposedWrite>(v),
        _ => back::<FeedFilter>(v),
    };
    fn objects(v: &Value, at: String, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                out.push(at.clone());
                let maps = ["params", "patch", "payload"];
                let members = o.iter().filter(|(k, _)| !maps.contains(&k.as_str()));
                members.for_each(|(k, x)| objects(x, format!("{at}/{k}"), out));
            }
            Value::Array(a) => {
                (a.iter().enumerate()).for_each(|(i, x)| objects(x, format!("{at}/{i}"), out))
            }
            _ => {}
        }
    }
    let named = |answer: Result<Value, String>, what: &Value, at: &str| {
        let said = matches!(&answer, Err(e) if e.contains("unknown field `zzz`"));
        assert!(said, "{what}: `zzz` at {at:?} is read: {answer:?}");
    };
    let mut refused = 0;
    for pair in whole.as_array().unwrap() {
        let (what, v) = (&pair[0], &pair[1]);
        assert_eq!(read(what, v).as_ref(), Ok(v), "{what}: written back");
        let mut places = Vec::new();
        objects(v, String::new(), &mut places);
        for place in places {
            let mut bad = v.clone();
            let inside = bad.pointer_mut(&place).and_then(Value::as_object_mut);
            inside.unwrap().insert("zzz".into(), json!(1));
            named(read(what, &bad), what, &place);
            refused += 1;
        }
    }
    assert_eq!(refused, 49, "the places a member was planted");
    // A migration's proposal writes its report back, so it is planted alone.
    for write in ["migrate", "migrate_value_type"] {
        let v = json!({"write": write, "zzz": 1});
        named(read(&json!("proposed"), &v), &v, "");
    }
}

/// A group key is a field's name or an object with `name` and `expr`. A
/// list is refused in the words a number is, though serde reads a struct
/// from one (lakelands#720).
#[test]
fn a_group_key_is_a_name_or_an_object() {
    use serde_json::json;
    let key = |v: Value| serde_json::from_value::<GroupKey>(v).map_err(|e| e.to_string());
    let named = json!({"name": "m", "expr": "date_bucket(on, 'month')"});
    assert!(matches!(key(json!("g")), Ok(GroupKey::Field(_))));
    assert!(matches!(key(named), Ok(GroupKey::Named { .. })));
    for v in [
        json!(["m", "date_bucket(on, 'month')"]),
        json!(3),
        json!(true),
    ] {
        let said = key(v.clone()).expect_err("a group key that is neither");
        assert!(
            said.contains("expected a field's name, or an object with `name` and `expr`"),
            "{v}: {said}"
        );
    }
}

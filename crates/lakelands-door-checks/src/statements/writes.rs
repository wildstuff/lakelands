//! A statement that writes, checked as the one call of a typed write door
//! it translates into: a node's `CREATE` or `MERGE` an `upsert`, a
//! relationship's a `link`, and a `MATCH … SET`, `REMOVE` or `DELETE` the
//! plan of a change, enacted only with that plan's id. Everything the
//! subset does not write is refused naming its construct, before a
//! record is read, and nothing is written.

use std::sync::Arc;

use lakelands_api::error::{Case, Construct, Error};
use lakelands_api::expr::Condition;
use lakelands_api::read::{Params, ReadAt};
use lakelands_api::write::{Change, ChangeTo, Declaration, WriteOpts};
use lakelands_api::{LakeStore, Principal, Timestamp, Version};
use serde_json::{Value, json};

use super::{Statements, at, billed, invalid};
use crate::checks::{Fixture, patch, setup};

/// `firm`, keyed by its `code`, with a size and whether it is big, which
/// the lake derives; `staff`, keyed by its writer, working at a firm
/// since a year the link carries.
async fn firms(f: &Fixture) {
    let text = json!({"kind": "text"});
    let number = json!({"kind": "number"});
    for d in [
        json!({"declare": "type", "name": "firm", "key": ["code"], "fields": [
            {"name": "code", "type": text},
            {"name": "size", "type": number},
            {"name": "big", "type": {"kind": "boolean"}, "derived": "size > 10"}]}),
        json!({"declare": "type", "name": "staff", "fields": [{"name": "name", "type": text}],
            "relations": [{"name": "works_at", "target": "firm", "cardinality": "many_to_one",
                "fields": [{"name": "since", "type": number}]}]}),
    ] {
        let d: Declaration = serde_json::from_value(d).expect("a declaration");
        let declared = f.catalog.declare(&f.cast.owner, d).await;
        declared.expect("the owner declares firms and staff");
    }
}

fn params(v: Value) -> Params {
    serde_json::from_value(v).expect("parameters")
}

fn create() -> WriteOpts {
    WriteOpts {
        expected_version: Some(Version(0)),
        idempotency_key: None,
        defer_computed_fields: false,
    }
}

/// `statement` with `params` as `who`, at the head.
async fn said(door: &dyn Statements, who: &Principal, s: &str, p: Value) -> Result<Value, Error> {
    door.cypher(who, s, &params(p), at(None), None).await
}

fn applied(answer: &Result<Value, Error>, s: &str) {
    let answer = answer.as_ref().unwrap_or_else(|e| panic!("{s}: {e:?}"));
    assert_eq!(answer["outcome"], json!("applied"), "{s}: {answer}");
}

/// The rows a read statement answers, as the owner.
async fn rows(f: &Fixture, door: &dyn Statements, s: &str) -> Value {
    let read = said(door, &f.cast.owner, s, json!({})).await;
    read.unwrap_or_else(|e| panic!("{s}: {e:?}"))["data"]["rows"].clone()
}

fn outside(construct: Construct, s: &str, at: &str) -> Error {
    let offset = s.find(at).unwrap_or_else(|| panic!("{at} in {s}"));
    invalid(Case::CypherOutside { construct, offset })
}

/// `MERGE` writes the record its type's declared key names, every key
/// field given and no other, and `CREATE` expects none under its key, as
/// an `upsert` that expects none; a writer-keyed type is keyed by the
/// map's `key`. A `MERGE` on another field, without a key field, or with
/// `ON CREATE` or `ON MATCH` is refused naming it, and writes nothing.
pub async fn a_merge_writes_on_the_declared_key_and_a_create_expects_none(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    firms(&f).await;
    let (owner, door) = (&f.cast.owner, door.as_ref());
    let merge = "MERGE (c:firm {code: $code}) SET c.size = $size";
    for size in [5, 7] {
        let p = json!({"code": "F-1", "size": size});
        applied(&said(door, owner, merge, p).await, merge);
    }
    let sizes = "MATCH (c:firm) RETURN c.code, c.size, c.big ORDER BY c.code";
    let want = json!([{"c.code": "F-1", "c.size": 7, "c.big": false}]);
    assert_eq!(
        rows(&f, door, sizes).await,
        want,
        "a MERGE writes one record"
    );

    let made = "CREATE (c:firm {code: $code, size: 3})";
    let head = f.store.head().await.expect("the head");
    let again = said(door, owner, made, json!({"code": "F-1"})).await;
    let (ty, key) = ("firm".into(), "F-1".into());
    let direct = f
        .catalog
        .upsert(owner, &ty, &key, patch(&[("code", json!("F-1"))]), create());
    assert_eq!(
        again.expect_err(made),
        direct.await.expect_err("F-1 is there")
    );
    assert_eq!(
        f.store.head().await.expect("the head"),
        head,
        "a CREATE over F-1 wrote"
    );
    applied(&said(door, owner, made, json!({"code": "F-2"})).await, made);

    let hired = "CREATE (s:staff {key: $key, name: $name})";
    let p = json!({"key": "S-1", "name": "Ada"});
    applied(&said(door, owner, hired, p).await, hired);
    let named = "MATCH (s:staff) RETURN s.key, s.name";
    let want = json!([{"s.key": "S-1", "s.name": "Ada"}]);
    assert_eq!(rows(&f, door, named).await, want, "a writer-keyed CREATE");

    let head = f.store.head().await.expect("the head");
    let (firm, staff) = (|| "firm".into(), || "staff".into());
    for (s, case) in [
        (
            "MERGE (c:firm {code: 'F-1', size: 5})",
            Case::CypherMergeOn {
                ty: firm(),
                field: "size".into(),
            },
        ),
        (
            "MERGE (c:firm {size: 5})",
            Case::CypherMergeOn {
                ty: firm(),
                field: "size".into(),
            },
        ),
        (
            "MERGE (c:firm) SET c.size = 1",
            Case::CypherKeyMissing {
                ty: firm(),
                field: "code".into(),
            },
        ),
        (
            "MERGE (s:staff {name: 'Ada'})",
            Case::CypherMergeOn {
                ty: staff(),
                field: "name".into(),
            },
        ),
        (
            "MERGE (s:staff) SET s.name = 'Ada'",
            Case::CypherKeyMissing {
                ty: staff(),
                field: "key".into(),
            },
        ),
    ] {
        let e = said(door, owner, s, json!({})).await;
        assert_eq!(e.expect_err(s), invalid(case), "{s}");
    }
    for (s, construct, at) in [
        (
            "MERGE (c:firm {code: 'F-3'}) ON CREATE SET c.size = 1",
            Construct::OnCreate,
            "ON CREATE",
        ),
        (
            "MERGE (c:firm {code: 'F-1'}) ON MATCH SET c.size = 1",
            Construct::OnMatch,
            "ON MATCH",
        ),
    ] {
        let e = said(door, owner, s, json!({})).await;
        assert_eq!(e.expect_err(s), outside(construct, s, at), "{s}");
    }
    assert_eq!(
        f.store.head().await.expect("the head"),
        head,
        "a refusal wrote"
    );
}

/// A relationship's `MERGE` or `CREATE` between two matched nodes is a
/// `link` of the pair, either way the arrow points: `MERGE` links it once
/// however often it is sent, `CREATE` expects no link of the pair, as a
/// `link` that expects none, and a `MERGE` on a field the link carries
/// is refused naming it, and an end labelled other than the type the relation
/// reaches is refused as a read's step is.
pub async fn a_relationship_is_merged_or_created_on_its_pair(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    firms(&f).await;
    let (owner, door) = (&f.cast.owner, door.as_ref());
    for s in [
        "CREATE (c:firm {code: 'F-1'})",
        "CREATE (s:staff {key: 'S-1', name: 'Ada'})",
        "CREATE (s:staff {key: 'S-2', name: 'Bo'})",
    ] {
        applied(&said(door, owner, s, json!({})).await, s);
    }
    let made = "MATCH (s:staff {key: $staff}), (c:firm {code: $firm}) \
        CREATE (s)-[w:works_at {since: $since}]->(c)";
    let p = json!({"staff": "S-1", "firm": "F-1", "since": 2020});
    applied(&said(door, owner, made, p.clone()).await, made);
    let head = f.store.head().await.expect("the head");
    let again = said(door, owner, made, p).await;
    let (ty, rel) = ("staff".into(), "works_at".into());
    let (from, to) = ("S-1".into(), "F-1".into());
    let direct = f
        .catalog
        .link(owner, &ty, &from, &rel, &to, Default::default(), create());
    assert_eq!(
        again.expect_err(made),
        direct.await.expect_err("S-1 works at F-1")
    );
    assert_eq!(
        f.store.head().await.expect("the head"),
        head,
        "a second CREATE wrote"
    );

    let merged = "MATCH (c:firm {code: 'F-1'}), (s:staff {key: 'S-2'}) MERGE (c)<-[:works_at]-(s)";
    for _ in 0..2 {
        applied(&said(door, owner, merged, json!({})).await, merged);
    }
    let pairs = "MATCH (s:staff)-[:works_at]->(c:firm) RETURN s.key, c.code ORDER BY s.key";
    let want = json!([
        {"s.key": "S-1", "c.code": "F-1"},
        {"s.key": "S-2", "c.code": "F-1"},
    ]);
    assert_eq!(rows(&f, door, pairs).await, want);

    let head = f.store.head().await.expect("the head");
    let on = "MATCH (s:staff {key: 'S-1'}), (c:firm {code: 'F-1'}) MERGE (s)-[:works_at {since: 1}]->(c)";
    let e = said(door, owner, on, json!({})).await;
    let case = Case::CypherMergeOn {
        ty: "sys.link.staff.works_at".into(),
        field: "since".into(),
    };
    assert_eq!(e.expect_err(on), invalid(case));
    let reaches = invalid(Case::StepReaches {
        reached: "firm".into(),
        ty: "staff".into(),
        also: None,
    });
    for s in [
        "MATCH (s:staff {key: 'S-1'}), (p:staff {key: 'S-2'}) MERGE (s)-[:works_at]->(p)",
        "MATCH (s:staff {key: 'S-1'}), (p:staff {key: 'S-2'}) CREATE (p)<-[:works_at]-(s)",
    ] {
        let e = said(door, owner, s, json!({})).await;
        assert_eq!(e.expect_err(s), reaches, "{s}");
    }
    assert_eq!(
        f.store.head().await.expect("the head"),
        head,
        "a refusal wrote"
    );
}

/// `MATCH … SET`, `REMOVE` or `DELETE` sent alone answers the plan of the
/// change and changes nothing; sent again with the plan's id it makes
/// that change, and where a selected record moved since, or the id is
/// another change's, it is refused `plan_stale` and writes nothing.
/// `REMOVE` sets the field to null and `DELETE` retires.
pub async fn a_change_is_answered_by_its_plan_and_enacted_with_its_id(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    let (owner, door) = (&f.cast.owner, door.as_ref());
    let late = "MATCH (i:invoice) WHERE i.status = $status SET i.status = $to";
    let p = params(json!({"status": "open", "to": "late"}));
    let head = f.store.head().await.expect("the head");
    let plan = door.cypher(owner, late, &p, at(None), None).await;
    let plan = plan.expect("a plan").clone();
    let records = &plan["data"]["records"];
    assert_eq!(records["selected"], json!(3), "{plan}");
    assert_eq!(
        records["fields"],
        json!([{"field": "status", "records": 3}]),
        "{plan}"
    );
    assert_eq!(
        f.store.head().await.expect("the head"),
        head,
        "a plan wrote"
    );
    let id = plan["data"]["report_id"].as_str().expect("its id");
    applied(&door.enact(owner, late, &p, id).await, late);
    let statuses = "MATCH (i:invoice) RETURN i.key, i.status, i.note ORDER BY i.key";
    let read = rows(&f, door, statuses).await;
    let status: Vec<&Value> = read
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| &r["i.status"])
        .collect();
    assert_eq!(
        status,
        [
            &json!("late"),
            &json!("paid"),
            &json!("late"),
            &json!("late")
        ]
    );

    let unnoted = "MATCH (i:invoice {status: 'late'}) REMOVE i.note";
    let none = Params::new();
    let plan = door
        .cypher(owner, unnoted, &none, at(None), None)
        .await
        .expect("a plan");
    let stale = plan["data"]["report_id"]
        .as_str()
        .expect("its id")
        .to_owned();
    let (ty, key) = ("invoice".into(), "INV-1".into());
    let moved = f.catalog.upsert(
        owner,
        &ty,
        &key,
        patch(&[("note", json!("moved"))]),
        Default::default(),
    );
    moved.await.expect("the owner writes INV-1");
    let head = f.store.head().await.expect("the head");
    let e = door.enact(owner, unnoted, &none, &stale).await;
    assert_eq!(e.expect_err("INV-1 moved"), invalid(Case::PlanStale));
    let retire = "MATCH (i:invoice {status: 'paid'}) DELETE i";
    let other = door
        .cypher(owner, retire, &none, at(None), None)
        .await
        .expect("a plan");
    let other = other["data"]["report_id"]
        .as_str()
        .expect("its id")
        .to_owned();
    let e = door.enact(owner, unnoted, &none, &other).await;
    assert_eq!(
        e.expect_err("another change's plan"),
        invalid(Case::PlanStale)
    );
    assert_eq!(
        f.store.head().await.expect("the head"),
        head,
        "a stale enact wrote"
    );

    let plan = door
        .cypher(owner, unnoted, &none, at(None), None)
        .await
        .expect("a plan");
    let id = plan["data"]["report_id"].as_str().expect("its id");
    applied(&door.enact(owner, unnoted, &none, id).await, unnoted);
    applied(&door.enact(owner, retire, &none, &other).await, retire);
    let read = rows(&f, door, statuses).await;
    let want = json!([
        {"i.key": "INV-1", "i.status": "late"},
        {"i.key": "INV-3", "i.status": "late"},
        {"i.key": "INV-4", "i.status": "late"},
    ]);
    assert_eq!(read, want, "the notes removed and INV-2 retired");
}

/// A relationship named in the `MATCH` is the link its `SET` or `DELETE`
/// changes: its ends picked by their keys, its fields by its map, and
/// `DELETE` unlinks, planned and enacted as every change is; an end
/// labelled other than the type the relation reaches is refused.
pub async fn a_link_is_changed_through_its_relationship(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    firms(&f).await;
    let (owner, door) = (&f.cast.owner, door.as_ref());
    for s in [
        "CREATE (c:firm {code: 'F-1'})",
        "CREATE (s:staff {key: 'S-1', name: 'Ada'})",
        "CREATE (s:staff {key: 'S-2', name: 'Bo'})",
        "MATCH (s:staff {key: 'S-1'}), (c:firm {code: 'F-1'}) CREATE (s)-[:works_at {since: 2020}]->(c)",
        "MATCH (s:staff {key: 'S-2'}), (c:firm {code: 'F-1'}) CREATE (s)-[:works_at {since: 2021}]->(c)",
    ] {
        applied(&said(door, owner, s, json!({})).await, s);
    }
    let enacted = async |s: &str, p: Value, selected: u64| {
        let p = params(p);
        let plan = door.cypher(owner, s, &p, at(None), None).await;
        let plan = plan.unwrap_or_else(|e| panic!("{s}: {e:?}"));
        assert_eq!(
            plan["data"]["records"]["selected"],
            json!(selected),
            "{s}: {plan}"
        );
        let id = plan["data"]["report_id"].as_str().expect("its id");
        applied(&door.enact(owner, s, &p, id).await, s);
    };
    let moved = "MATCH (s:staff {key: $staff})-[w:works_at]->(c:firm) SET w.since = $since";
    enacted(moved, json!({"staff": "S-1", "since": 2024}), 1).await;
    let left = "MATCH (c:firm)<-[w:works_at {since: $since}]-(s:staff) DELETE w";
    enacted(left, json!({"since": 2024}), 1).await;
    let head = f.store.head().await.expect("the head");
    let reaches = invalid(Case::StepReaches {
        reached: "firm".into(),
        ty: "staff".into(),
        also: None,
    });
    for s in [
        "MATCH (s:staff)-[w:works_at]->(p:staff) SET w.since = 1",
        "MATCH (p:staff)<-[w:works_at]-(s:staff) DELETE w",
    ] {
        let e = said(door, owner, s, json!({})).await;
        assert_eq!(e.expect_err(s), reaches, "{s}");
    }
    assert_eq!(
        f.store.head().await.expect("the head"),
        head,
        "a refusal wrote"
    );
    let pairs = "MATCH (s:staff)-[:works_at]->(c:firm) RETURN s.key, c.code";
    let want = json!([{"s.key": "S-2", "c.code": "F-1"}]);
    assert_eq!(rows(&f, door, pairs).await, want, "S-1 unlinked");
}

/// A statement that would take two door calls is refused naming its
/// second updating clause, where it stands, and writes nothing.
pub async fn a_statement_of_two_writes_is_refused_naming_the_second(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    firms(&f).await;
    let (owner, door) = (&f.cast.owner, door.as_ref());
    let head = f.store.head().await.expect("the head");
    for (s, construct, at) in [
        (
            "CREATE (a:firm {code: 'F-8'}) CREATE (b:firm {code: 'F-9'})",
            Construct::Create,
            "CREATE (b",
        ),
        (
            "CREATE (a:firm {code: 'F-8'}), (b:firm {code: 'F-9'})",
            Construct::Create,
            "(b",
        ),
        (
            "MERGE (a:firm {code: 'F-8'}) MERGE (b:firm {code: 'F-9'})",
            Construct::Merge,
            "MERGE (b",
        ),
        (
            "MATCH (i:invoice) SET i.status = 'x' DELETE i",
            Construct::Delete,
            "DELETE",
        ),
        (
            "MATCH (i:invoice) DELETE i SET i.status = 'x'",
            Construct::Set,
            "SET",
        ),
        (
            "MATCH (i:invoice) DELETE i REMOVE i.note",
            Construct::Remove,
            "REMOVE",
        ),
        (
            "MERGE (a:firm {code: 'F-8'}) DELETE a",
            Construct::Delete,
            "DELETE",
        ),
    ] {
        let offset = s.find(at).expect("the second write");
        let case = Case::CypherSecondWrite { construct, offset };
        let e = said(door, owner, s, json!({})).await;
        assert_eq!(e.expect_err(s), invalid(case), "{s}");
    }
    assert_eq!(
        f.store.head().await.expect("the head"),
        head,
        "a refusal wrote"
    );
}

/// What a write statement cannot say is refused naming the construct,
/// where it stands, before a record is read: to a principal who may read
/// nothing as to the owner. A field the lake computes is refused naming
/// it, by the type's description alone.
pub async fn a_write_the_subset_does_not_take_is_refused_before_a_record_is_read(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    firms(&f).await;
    let door = door.as_ref();
    let head = f.store.head().await.expect("the head");
    let some = [
        ("MATCH (i:invoice) SET i:paid", Construct::Label, ":paid"),
        (
            "CREATE INDEX FOR (i:invoice) ON (i.status)",
            Construct::Schema,
            "CREATE",
        ),
        ("DROP INDEX status", Construct::Schema, "DROP"),
        (
            "MATCH (i:invoice) DETACH DELETE i",
            Construct::DetachDelete,
            "DETACH",
        ),
        (
            "MATCH (i:invoice) SET i.status = i.verdict",
            Construct::ComputedValue,
            "i.verdict",
        ),
        (
            "MATCH (i:invoice) WITH count(*) AS n MERGE (c:firm {code: n})",
            Construct::With,
            "WITH",
        ),
        ("CALL db.labels()", Construct::Call, "CALL"),
        (
            "MATCH (i:invoice) FOREACH (x IN [1] | SET i.status = 'x')",
            Construct::Foreach,
            "FOREACH",
        ),
        (
            "MERGE (c:firm {code: 'F-1'}) RETURN c",
            Construct::ReturnBesideWrite,
            "RETURN",
        ),
        (
            "MATCH (i:invoice) SET i = {status: 'x'}",
            Construct::SetMap,
            "i =",
        ),
        (
            "EXPLAIN MATCH (i:invoice) SET i.status = 'x'",
            Construct::ExplainWrite,
            "EXPLAIN",
        ),
    ];
    for who in [&f.cast.stranger, &f.cast.owner] {
        for (s, construct, at) in some.clone() {
            let e = said(door, who, s, json!({})).await;
            assert_eq!(
                e.expect_err(s),
                outside(construct, s, at),
                "{}: {s}",
                who.id
            );
        }
    }
    let owner = &f.cast.owner;
    for s in [
        "CREATE (c:firm {code: 'F-1', big: true})",
        "MERGE (c:firm {code: 'F-1'}) SET c.big = true",
        "MATCH (c:firm) SET c.big = false",
    ] {
        let case = Case::CypherComputed {
            ty: "firm".into(),
            field: "big".into(),
        };
        let e = said(door, owner, s, json!({})).await;
        assert_eq!(e.expect_err(s), invalid(case), "{s}");
    }
    assert_eq!(
        f.store.head().await.expect("the head"),
        head,
        "a refusal wrote"
    );
}

/// The `MATCH` of a write reads as the plan of its change reads: a
/// condition on a field the principal may not read is refused as the
/// plan refuses it, a field it may not write as the plan answers it, and
/// a write it may not make as the door it calls refuses it.
pub async fn the_match_of_a_write_reads_as_its_plan_reads(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    let (clerk, door) = (&f.cast.clerk, door.as_ref());
    let head = f.store.head().await.expect("the head");
    let change = |filter: &str, field: &str| Change::Records {
        ty: "invoice".into(),
        filter: Box::new(Condition::Text(filter.into())),
        to: ChangeTo::Set(patch(&[(field, json!("x"))])),
        only: Default::default(),
        skip_restricted: false,
    };
    let hidden = "MATCH (i:invoice) WHERE i.secret = $s SET i.status = 'x'";
    let e = said(door, clerk, hidden, json!({"s": "k"})).await;
    let direct = f
        .catalog
        .plan(clerk, change("secret = 'k'", "status"))
        .await;
    assert_eq!(
        e.expect_err(hidden),
        direct.expect_err("the clerk reads no secret")
    );

    let verdict = "MATCH (i:invoice) WHERE i.status = 'open' SET i.verdict = 'x'";
    let at_door = said(door, clerk, verdict, json!({})).await;
    let direct = f
        .catalog
        .plan(clerk, change("status = 'open'", "verdict"))
        .await;
    match (at_door, direct) {
        (Ok(a), Ok(d)) => {
            let d = serde_json::to_value(d).expect("a plan is JSON");
            assert_eq!(a["data"]["records"], d["data"]["records"], "{verdict}");
        }
        (a, d) => assert_eq!(a.map(|_| ()), d.map(|_| ()), "{verdict}"),
    }

    let stranger = &f.cast.stranger;
    let s = "MERGE (i:invoice {key: 'INV-9'}) SET i.status = 'open'";
    let e = said(door, stranger, s, json!({})).await;
    let (ty, key) = ("invoice".into(), "INV-9".into());
    let direct = f.catalog.upsert(
        stranger,
        &ty,
        &key,
        patch(&[("status", json!("open"))]),
        Default::default(),
    );
    assert_eq!(
        e.expect_err(s),
        direct.await.expect_err("the stranger writes nothing")
    );
    assert_eq!(
        f.store.head().await.expect("the head"),
        head,
        "a refusal wrote"
    );
}

/// A value is written as its parameter holds it, never read as text of
/// the statement: a key that reads as Cypher is that key, and a
/// condition's parameter that reads as Cypher selects nothing.
pub async fn a_written_value_is_its_parameter_never_text(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    firms(&f).await;
    let (owner, door) = (&f.cast.owner, door.as_ref());
    let code = "F-1'}) DETACH DELETE c //";
    let merge = "MERGE (c:firm {code: $code}) SET c.size = $size";
    applied(
        &said(door, owner, merge, json!({"code": code, "size": 2})).await,
        merge,
    );
    let codes = "MATCH (c:firm) RETURN c.code, c.size";
    assert_eq!(
        rows(&f, door, codes).await,
        json!([{"c.code": code, "c.size": 2}])
    );
    let any = "MATCH (i:invoice) WHERE i.status = $status DELETE i";
    let plan = said(door, owner, any, json!({"status": "open' OR 'a' = 'a"})).await;
    let plan = plan.expect("a plan");
    assert_eq!(plan["data"]["records"]["selected"], json!(0), "{plan}");
}

/// A write is made at the head and pages nothing: an axis or a cursor
/// beside it is refused naming the member, and so is a plan's id beside
/// a write that has no plan, or beside a read.
pub async fn what_a_write_is_not_sent_with_is_refused_naming_it(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    let head = billed(&f).await;
    firms(&f).await;
    let (owner, none) = (&f.cast.owner, Params::new());
    let member = |m: &str| invalid(Case::CypherMember { member: m.into() });
    let merge = "MERGE (c:firm {code: 'F-1'})";
    let e = door.cypher(owner, merge, &none, at(Some(head)), None).await;
    assert_eq!(e.expect_err("as of"), member("as_of"));
    let valid = ReadAt {
        valid_at: Some(Timestamp::parse("2026-01-01T00:00:00Z").expect("an instant")),
        ..ReadAt::default()
    };
    let e = door.cypher(owner, merge, &none, valid, None).await;
    assert_eq!(e.expect_err("valid at"), member("valid_at"));
    let page = "MATCH (i:invoice) RETURN i.key ORDER BY i.key LIMIT 1";
    let page = door.cypher(owner, page, &none, at(None), None).await;
    let cursor = page.expect("a page")["data"]["next_cursor"].clone();
    let cursor = serde_json::from_value(cursor).expect("a cursor");
    let e = door
        .cypher(owner, merge, &none, at(None), Some(&cursor))
        .await;
    assert_eq!(e.expect_err("a cursor"), member("cursor"));
    let e = door.enact(owner, merge, &none, "a-plan").await;
    assert_eq!(e.expect_err("a MERGE has no plan"), member("plan_id"));
    let read = "MATCH (i:invoice) RETURN i.key";
    let e = door.enact(owner, read, &none, "a-plan").await;
    assert_eq!(e.expect_err("a read has no plan"), member("plan_id"));
    let after = f.store.head().await.expect("the head");
    let read = rows(&f, door.as_ref(), "MATCH (c:firm) RETURN c.code").await;
    assert_eq!(read, json!([]), "nothing is written at {after:?}");
}

//! A translated statement is a query the store reads, and its rows are
//! the ones the statement means.

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::write::Declaration;
use lakelands_api::{Grants, Location, Params, Principal, PrincipalKind, WriteContext};
use lakelands_cypher::{Statement, translate};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

macro_rules! from {
    ($($v:tt)+) => {
        serde_json::from_value(json!($($v)+)).expect("decodes")
    };
}

#[tokio::test]
async fn a_translated_statement_reads_the_rows_it_means() {
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory("c"), seams, OpenMode::Create)
        .await
        .expect("opens");
    let who = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "c".into(),
        grants: Grants::owner(),
    };
    let cat = store.catalog(WriteContext::default());
    for d in [
        json!({"declare": "type", "name": "company", "fields": [{"name": "size", "type": {"kind": "number"}}]}),
        json!({"declare": "type", "name": "person", "fields": [{"name": "name", "type": {"kind": "text"}}],
               "relations": [{"name": "works_at", "target": "company", "cardinality": "many_to_one"}]}),
        json!({"declare": "type", "name": "bill",
               "fields": [{"name": "amount", "type": {"kind": "number"}}, {"name": "note", "type": {"kind": "text"}}],
               "relations": [{"name": "entered_by", "target": "person", "cardinality": "many_to_one"},
                             {"name": "approved_by", "target": "person", "cardinality": "many_to_one"}]}),
    ] {
        cat.declare(
            &who,
            serde_json::from_value::<Declaration>(d).expect("decodes"),
        )
        .await
        .expect("declares");
    }
    let none = Default::default;
    let no = Default::default;
    for (k, n) in [("p1", "ann"), ("p2", "bob")] {
        cat.upsert(
            &who,
            &"person".into(),
            &k.into(),
            from!({"name": n}),
            none(),
        )
        .await
        .expect("upserts");
    }
    cat.upsert(
        &who,
        &"company".into(),
        &"c1".into(),
        from!({"size": 5}),
        none(),
    )
    .await
    .expect("upserts");
    cat.link(
        &who,
        &"person".into(),
        &"p1".into(),
        &"works_at".into(),
        &"c1".into(),
        no(),
        none(),
    )
    .await
    .expect("links");
    for (k, a, p) in [("b1", 5, "p1"), ("b2", 2, "p2"), ("b3", 9, "p1")] {
        cat.upsert(
            &who,
            &"bill".into(),
            &k.into(),
            from!({"amount": a, "note": "x"}),
            none(),
        )
        .await
        .expect("upserts");
        cat.link(
            &who,
            &"bill".into(),
            &k.into(),
            &"entered_by".into(),
            &p.into(),
            no(),
            none(),
        )
        .await
        .expect("links");
    }
    cat.link(
        &who,
        &"bill".into(),
        &"b1".into(),
        &"approved_by".into(),
        &"p1".into(),
        no(),
        none(),
    )
    .await
    .expect("links");

    let cases = [
        (
            "MATCH (b:bill {key: $k}) RETURN b.amount",
            json!([{"b.amount": 2}]),
        ),
        (
            "MATCH (b:bill)-[:entered_by]->(p:person) WHERE b.amount > $n OR p.name = 'bob' \
             RETURN b.amount, p.name ORDER BY b.amount DESC LIMIT 2",
            json!([{"b.amount": 9, "p.name": "ann"}, {"b.amount": 5, "p.name": "ann"}]),
        ),
        // The optional step's condition decides whether it matches, not
        // whether the row stands.
        (
            "MATCH (b:bill)-[:entered_by]->(p:person) OPTIONAL MATCH (p)-[:works_at]->(c:company) \
             WHERE c.size > 3 RETURN b.key, c.size ORDER BY b.key",
            json!([{"b.key": "b1", "c.size": 5}, {"b.key": "b2"}, {"b.key": "b3", "c.size": 5}]),
        ),
        // An absent relationship to a name bound before is a join.
        (
            "MATCH (b:bill)-[:entered_by]->(p:person) WHERE NOT (b)-[:approved_by]->(p) RETURN b.key ORDER BY b.key",
            json!([{"b.key": "b2"}, {"b.key": "b3"}]),
        ),
        (
            "MATCH (b:bill) WHERE NOT EXISTS { (b)-[:entered_by]->(p:person) WHERE p.name = 'ann' } RETURN b.key",
            json!([{"b.key": "b2"}]),
        ),
        (
            "MATCH (b:bill)-[:entered_by]->(p), (b)-[:approved_by]->(p) RETURN b.key",
            json!([{"b.key": "b1"}]),
        ),
        (
            "MATCH (b:bill)-[:entered_by]->(p:person) RETURN p.name, count(*) AS n, sum(b.amount) AS total",
            json!([{"by": {"p.name": "ann"}, "values": {"n": "2", "total": "14"}},
                   {"by": {"p.name": "bob"}, "values": {"n": "1", "total": "2"}}]),
        ),
        // A key condition on a step is placed where the store reads it.
        (
            "MATCH (b:bill)-[:entered_by]->(p:person)-[:works_at]->(c:company) WHERE p.key = 'p1' \
             RETURN b.key, c.key ORDER BY b.key",
            json!([{"b.key": "b1", "c.key": "c1"}, {"b.key": "b3", "c.key": "c1"}]),
        ),
    ];
    for (statement, rows) in cases {
        let given: Params = from!({"k": "b2", "n": 4});
        let given = given
            .into_iter()
            .filter(|(k, _)| statement.contains(&format!("${k}")))
            .collect();
        let Statement::Read(pattern) = translate(statement, &given).expect(statement).statement
        else {
            panic!("{statement} is a read");
        };
        let read = cat.query(&who, *pattern).await.expect(statement);
        let data = serde_json::to_value(&read.data).expect("json");
        assert_eq!(data["rows"], rows, "{statement}");
    }
}

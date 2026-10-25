//! Each statement of the subset and the one pattern it translates into.
//! A `where` and a named group key are written in the text form of the
//! expression grammar, and compared typed.

use lakelands_api::Params;
use lakelands_api::expr::{Condition, parse};
use lakelands_api::query::{Pattern, Projection};
use lakelands_api::read::GroupKey;
use lakelands_cypher::{Statement, Translated, translate};
use serde_json::{Value, json};

fn params(v: Value) -> Params {
    serde_json::from_value(v).expect("parameters")
}

fn typed(c: &mut Condition) {
    if let Condition::Text(t) = c {
        *c = Condition::Expr(parse(t).expect("an expression"));
    }
}

/// The pattern `v` describes, its conditions typed.
fn pattern(v: Value) -> Pattern {
    let mut p: Pattern = serde_json::from_value(v).expect("a pattern");
    (p.start.filter.iter_mut())
        .chain(p.steps.iter_mut().filter_map(|s| s.filter.as_mut()))
        .for_each(typed);
    if let Projection::Aggregate(a) = &mut p.project {
        for g in &mut a.group_by {
            if let GroupKey::Named { expr, .. } = g {
                typed(expr);
            }
        }
    }
    p
}

fn read(statement: &str, given: Value) -> Pattern {
    let t = translate(statement, &params(given)).expect(statement);
    let Statement::Read(p) = t.statement else {
        panic!("{statement} reads")
    };
    p
}

#[track_caller]
fn same(statement: &str, given: Value, expected: Value) {
    assert_eq!(read(statement, given), pattern(expected), "{statement}");
}

fn rows() -> Value {
    json!({"as": "rows", "fields": []})
}

#[test]
fn the_start_names_its_type_and_its_key() {
    same(
        "MATCH (b:bill) RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"}, "project": rows()}),
    );
    same(
        "MATCH (b:bill {key: 'b1'}) RETURN b.amount",
        json!({}),
        json!({"start": {"type": "bill", "key": "b1", "as": "b"},
               "project": {"as": "rows", "fields": ["b.amount"]}}),
    );
    same(
        "MATCH (b:bill {key: $k, amount: 5}) RETURN b",
        json!({"k": "b1"}),
        json!({"start": {"type": "bill", "key": "b1", "where": "amount = 5", "as": "b"},
               "project": rows()}),
    );
    same(
        "MATCH (b:bill) WHERE b.key = 'b1' AND b.amount > 2 RETURN b",
        json!({}),
        json!({"start": {"type": "bill", "key": "b1", "where": "amount > 2", "as": "b"},
               "project": rows()}),
    );
    same(
        "MATCH (:bill)-[:entered_by]->(p:person) RETURN p.name",
        json!({}),
        json!({"start": {"type": "bill"},
               "steps": [{"relation": "entered_by", "type": "person", "as": "p"}],
               "project": {"as": "rows", "fields": ["p.name"]}}),
    );
}

#[test]
fn a_relationship_is_a_step_walked_from_the_node_on_its_left() {
    same(
        "MATCH (b:bill)<-[:approves]-(p:person) RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "approves", "walk": "in", "type": "person", "as": "p"}],
               "project": rows()}),
    );
    same(
        "MATCH (b:bill)-[:related*1..3]-(c:bill) RETURN c.key",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "related", "walk": "both", "type": "bill",
                          "depth": {"min": 1, "max": 3}, "as": "c"}],
               "project": {"as": "rows", "fields": ["c.key"]}}),
    );
    same(
        "MATCH (b:bill)-[:related*..2]->(c) RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "related", "depth": {"min": 1, "max": 2}, "as": "c"}],
               "project": rows()}),
    );
    same(
        "MATCH (b:bill)-[:related*0..2]->(c) RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "related", "depth": {"min": 0, "max": 2}, "as": "c"}],
               "project": rows()}),
    );
}

#[test]
fn a_condition_goes_to_the_step_binding_the_last_name_it_reads() {
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person) WHERE p.name = 'ann' AND b.amount > 3 RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "where": "amount > 3", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "where": "name = 'ann'", "as": "p"}],
               "project": rows()}),
    );
    // A disjunction across names is one conjunct, on the later name.
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person) WHERE p.name = 'ann' OR b.amount > 3 RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person",
                          "where": "name = 'ann' or b.amount > 3", "as": "p"}],
               "project": rows()}),
    );
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person {name: 'ann'}) RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "where": "name = 'ann'", "as": "p"}],
               "project": rows()}),
    );
    // Conjuncts on one step keep the statement's order.
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person {name: 'ann'}) WHERE p.age > b.amount AND p.age < 9 RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person",
                          "where": "name = 'ann' and age > b.amount and age < 9", "as": "p"}],
               "project": rows()}),
    );
}

#[test]
fn a_names_key_type_and_depth_are_read_by_the_next_step_of_its_clause() {
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person)-[:works_at]->(c:company) WHERE p.key = 'p1' RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "as": "p"},
                         {"relation": "works_at", "type": "company", "where": "p.key = 'p1'", "as": "c"}],
               "project": rows()}),
    );
    same(
        "MATCH (b:bill)-[:entered_by]->(:person {key: 'p1'})-[:works_at]->(c) RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person"},
                         {"relation": "works_at", "where": "person.key = 'p1'", "as": "c"}],
               "project": rows()}),
    );
    // With no step after it, it stays where it is, for the store to judge.
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person) WHERE p.key = 'p1' AND p._depth = 1 RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person",
                          "where": "p.key = 'p1' and p._depth = 1", "as": "p"}],
               "project": rows()}),
    );
    same(
        "MATCH (b:bill)-[:entered_by]->(p) WHERE b.key > 'a' RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "where": "b.key > 'a'", "as": "p"}],
               "project": rows()}),
    );
}

#[test]
fn a_part_branches_from_a_bound_name_and_joins_one() {
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person), (b)-[:payee]->(q:person) WHERE q.name = p.name RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "as": "p"},
                         {"relation": "payee", "type": "person", "where": "name = p.name",
                          "as": "q", "from": "b"}],
               "project": rows()}),
    );
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person), (b)-[:approved_by]->(p) RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "as": "p"},
                         {"relation": "approved_by", "from": "b", "join": "p"}],
               "project": rows()}),
    );
    // A second clause continues the first; a label given there is the
    // binder's type.
    same(
        "MATCH (b:bill)-[:entered_by]->(p) MATCH (p:person {name: 'ann'})-[:works_at]->(c) RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "where": "name = 'ann'", "as": "p"},
                         {"relation": "works_at", "as": "c"}],
               "project": rows()}),
    );
}

#[test]
fn an_optional_match_is_one_optional_branch() {
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person) OPTIONAL MATCH (p)-[:works_at]->(c:company) RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "as": "p"},
                         {"relation": "works_at", "type": "company", "as": "c", "optional": true}],
               "project": rows()}),
    );
    // Its condition decides whether it matched: none goes before its
    // first step.
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person) \
         OPTIONAL MATCH (p)-[:works_at]->(c:company)-[:in]->(t:town) WHERE t.name = 'x' AND b.amount > 1 RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "as": "p"},
                         {"relation": "works_at", "type": "company", "where": "b.amount > 1",
                          "as": "c", "optional": true},
                         {"relation": "in", "type": "town", "where": "name = 'x'",
                          "as": "t", "optional": true}],
               "project": rows()}),
    );
    // A label on a name bound outside it is a condition of its own.
    same(
        "MATCH (b:bill)-[:entered_by]->(p) OPTIONAL MATCH (p:person)-[:works_at]->(c) RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "as": "p"},
                         {"relation": "works_at", "where": "p._type = 'person'", "as": "c", "optional": true}],
               "project": rows()}),
    );
    // One that binds no name keeps every row: it stands as nothing.
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person) OPTIONAL MATCH (p)<-[:approved_by]-(b) RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "as": "p"}],
               "project": rows()}),
    );
    // A step walking back to a name of the branch joins it.
    same(
        "MATCH (b:bill) OPTIONAL MATCH (b)-[:entered_by]->(p:person)<-[:payee]-(b) RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "as": "p", "optional": true},
                         {"relation": "payee", "walk": "in", "join": "b", "optional": true}],
               "project": rows()}),
    );
}

#[test]
fn not_exists_is_an_absent_step() {
    same(
        "MATCH (b:bill) WHERE NOT EXISTS { (b)-[:entered_by]->(p:person) WHERE p.name = $n } RETURN b",
        json!({"n": "ann"}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "where": "name = 'ann'", "absent": true}],
               "project": rows()}),
    );
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person) WHERE NOT (b)-[:approved_by]->(p) AND b.amount > 1 RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "where": "amount > 1", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "as": "p"},
                         {"relation": "approved_by", "from": "b", "join": "p", "absent": true}],
               "project": rows()}),
    );
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person) WHERE NOT EXISTS { MATCH (p)-[:works_at]->(:company {size: 5}) } RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "as": "p"},
                         {"relation": "works_at", "type": "company", "where": "size = 5", "absent": true}],
               "project": rows()}),
    );
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person) \
         OPTIONAL MATCH (p)-[:works_at]->(c:company) WHERE NOT EXISTS { (c)-[:in]->(:town) } RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "as": "p"},
                         {"relation": "works_at", "type": "company", "as": "c", "optional": true},
                         {"relation": "in", "type": "town", "optional": true, "absent": true}],
               "project": rows()}),
    );
}

#[test]
fn an_expression_is_one_of_the_grammar() {
    same(
        "MATCH (b:bill) WHERE b.name STARTS WITH 'a' AND b.name CONTAINS 'b' AND b.name =~ 'a.*' RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b",
                         "where": "starts_with(name, 'a') and contains(name, 'b') and matches(name, 'a.*')"},
               "project": rows()}),
    );
    same(
        "MATCH (b:bill) WHERE b.tag IN ['x', 'y'] AND b.tag IN $tags AND b.note IS NULL AND b.name IS NOT NULL RETURN *",
        json!({"tags": ["x", "z"]}),
        json!({"start": {"type": "bill", "as": "b",
                         "where": "tag in ('x', 'y') and tag in ('x', 'z') and note is null and not (name is null)"},
               "project": rows()}),
    );
    same(
        "MATCH (b:bill) WHERE -b.amount * 2 + 1 >= 3 / 2 - 1 RETURN *",
        json!({}),
        json!({"start": {"type": "bill", "as": "b", "where": "-amount * 2 + 1 >= 3 / 2 - 1"},
               "project": rows()}),
    );
    // A decimal given as text stays text: a parameter is never read
    // for what it might mean.
    same(
        "MATCH (b:bill) WHERE b.amount = $d RETURN *",
        json!({"d": "12.50"}),
        json!({"start": {"type": "bill", "as": "b", "where": "amount = '12.50'"}, "project": rows()}),
    );
}

#[test]
fn a_duration_is_a_fixed_length() {
    let p = read(
        "MATCH (b:bill) WHERE b.due < now() - duration('P1DT2H') RETURN *",
        json!({}),
    );
    let filter = serde_json::to_value(p.start.filter).expect("json");
    let micros = (26 * 3_600_i64) * 1_000_000;
    let expected = json!({"op": "cmp", "cmp": "lt",
        "lhs": {"op": "path", "path": ["due"]},
        "rhs": {"op": "arith", "arith": "sub",
                "lhs": {"op": "call", "func": "now", "args": []},
                "rhs": {"op": "lit", "value": {"type": "duration", "value": micros}}}});
    assert_eq!(filter, expected);
}

#[test]
fn return_answers_rows_or_an_aggregate() {
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person) RETURN b, p",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "as": "p"}],
               "project": rows()}),
    );
    same(
        "MATCH (b:bill) RETURN b.amount, b.key ORDER BY b.amount DESC, b.key ASC SKIP 0 LIMIT 10",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "project": {"as": "rows", "fields": ["b.amount", "b.key"]},
               "order_by": [{"field": "b.amount", "dir": "desc"}, {"field": "b.key", "dir": "asc"}],
               "limit": 10}),
    );
    same(
        "MATCH (b:bill) RETURN b LIMIT $n",
        json!({"n": 5}),
        json!({"start": {"type": "bill", "as": "b"}, "project": rows(), "limit": 5}),
    );
    same(
        "MATCH (b:bill)-[:entered_by]->(p:person) RETURN p.name, count(*) AS n, count(b) AS bills, \
         sum(b.amount) AS total, min(b.amount) AS lo, max(b.amount) AS hi, avg(b.amount) AS mean",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "steps": [{"relation": "entered_by", "type": "person", "as": "p"}],
               "project": {"as": "aggregate", "group_by": ["p.name"], "measures": [
                   {"name": "n", "func": "count"},
                   {"name": "bills", "func": "count"},
                   {"name": "total", "func": "sum", "field": "b.amount"},
                   {"name": "lo", "func": "min", "field": "b.amount"},
                   {"name": "hi", "func": "max", "field": "b.amount"},
                   {"name": "mean", "func": "avg", "field": "b.amount"}]}}),
    );
    same(
        "MATCH (b:bill) RETURN date_bucket(b.due, 'month') AS month, b.amount AS a, count(*) AS n",
        json!({}),
        json!({"start": {"type": "bill", "as": "b"},
               "project": {"as": "aggregate",
                           "group_by": [{"name": "month", "expr": "date_bucket(b.due, 'month')"},
                                        {"name": "a", "expr": "b.amount"}],
                           "measures": [{"name": "n", "func": "count"}]}}),
    );
}

#[test]
fn explain_is_said_beside_the_pattern() {
    let given = Params::new();
    let explained = translate("EXPLAIN MATCH (b:bill) RETURN b", &given).expect("explain");
    let run = translate("MATCH (b:bill) RETURN b", &given).expect("run");
    assert!(explained.explain && !run.explain);
    assert_eq!(explained.statement, run.statement);
}

#[test]
fn keywords_are_any_case_and_names_may_be_quoted() {
    same(
        "match (`the bill`:`sys.bill`) // a comment\n where `the bill`.amount <> 1 /* another */ return `the bill`.amount;",
        json!({}),
        json!({"start": {"type": "sys.bill", "as": "the bill", "where": "amount != 1"},
               "project": {"as": "rows", "fields": ["the bill.amount"]}}),
    );
}

#[test]
fn the_same_statement_gives_the_same_pattern() {
    let statement = "MATCH (b:bill)-[:entered_by]->(p:person), (b)-[:payee]->(q) \
                     WHERE q.name = p.name AND b.amount > $n AND NOT (b)-[:approved_by]->(p) \
                     RETURN p.name, sum(b.amount) AS total";
    let given = params(json!({"n": 3}));
    let first: Translated = translate(statement, &given).expect("translates");
    for _ in 0..8 {
        assert_eq!(translate(statement, &given).expect("translates"), first);
    }
}

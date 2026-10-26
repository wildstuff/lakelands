//! What the subset does not read is refused, each by its own case, at
//! the character it begins at, before anything after it is read.

use lakelands_api::Params;
use lakelands_api::error::Construct;
use lakelands_cypher::translate;
use serde_json::{Value, json};

/// The refusal's reason, as the wire says it.
fn refused(statement: &str, given: Value) -> Value {
    let given: Params = serde_json::from_value(given).expect("parameters");
    let error = translate(statement, &given).expect_err(statement);
    serde_json::to_value(&error).expect("json")["reason"].clone()
}

/// The character offset `marker` first stands at in `statement`.
fn offset(statement: &str, marker: &str) -> usize {
    let at = statement.find(marker).expect(marker);
    statement[..at].chars().count()
}

/// A statement each construct is refused in, and the text it is refused
/// at. The match is exhaustive, so a construct added to the set is given
/// one here before this compiles.
fn refused_at(construct: &Construct) -> (&'static str, &'static str) {
    match construct {
        Construct::Create => ("MATCH (b:bill) CREATE (c:bill) RETURN b", "CREATE"),
        Construct::Merge => ("MATCH (b:bill) MERGE (c:bill) RETURN b", "MERGE"),
        Construct::Set => ("MATCH (b:bill) SET b.amount = 1 RETURN b", "SET"),
        Construct::Delete => ("MATCH (b:bill) DELETE b", "DELETE"),
        Construct::DetachDelete => ("MATCH (b:bill) DETACH DELETE b", "DETACH"),
        Construct::Remove => ("MATCH (b:bill) REMOVE b.amount RETURN b", "REMOVE"),
        Construct::Foreach => (
            "MATCH (b:bill) FOREACH (x IN [1] | SET b.amount = x)",
            "FOREACH",
        ),
        Construct::LoadCsv => ("LOAD CSV FROM 'x' AS row RETURN row", "LOAD"),
        Construct::Call => (
            "MATCH (b:bill) CALL db.labels() YIELD label RETURN b",
            "CALL",
        ),
        Construct::With => ("MATCH (b:bill) WITH b RETURN b", "WITH"),
        Construct::Union => (
            "MATCH (b:bill) RETURN b UNION MATCH (b:bill) RETURN b",
            "UNION",
        ),
        Construct::Unwind => ("UNWIND [1, 2] AS x RETURN x", "UNWIND"),
        Construct::Use => ("USE other MATCH (b:bill) RETURN b", "USE"),
        Construct::Profile => ("PROFILE MATCH (b:bill) RETURN b", "PROFILE"),
        Construct::SeveralStatements => (
            "MATCH (b:bill) RETURN b; MATCH (c:bill) RETURN c",
            "MATCH (c",
        ),
        Construct::Distinct => ("MATCH (b:bill) RETURN DISTINCT b.amount", "DISTINCT"),
        Construct::ListComprehension => (
            "MATCH (b:bill) WHERE b.amount IN [x IN [1, 2] | x] RETURN b",
            "[x",
        ),
        Construct::PatternComprehension => (
            "MATCH (b:bill) RETURN [(b)-[:entered_by]->(p) | p.name] AS names",
            "[(",
        ),
        Construct::MapProjection => ("MATCH (b:bill) RETURN b {.amount} AS m", "{"),
        Construct::CountSubquery => (
            "MATCH (b:bill) WHERE COUNT { (b)-->() } > 1 RETURN b",
            "COUNT",
        ),
        Construct::Exists => (
            "MATCH (b:bill) WHERE EXISTS { (b)-[:entered_by]->() } RETURN b",
            "EXISTS",
        ),
        Construct::Case => (
            "MATCH (b:bill) RETURN CASE WHEN b.amount > 1 THEN 1 END AS c",
            "CASE",
        ),
        Construct::Xor => (
            "MATCH (b:bill) WHERE b.amount > 1 XOR b.amount < 5 RETURN b",
            "XOR",
        ),
        Construct::Modulo => ("MATCH (b:bill) WHERE b.amount % 2 = 0 RETURN b", "%"),
        Construct::Power => ("MATCH (b:bill) WHERE b.amount ^ 2 > 4 RETURN b", "^"),
        Construct::EndsWith => ("MATCH (b:bill) WHERE b.name ENDS WITH 'c' RETURN b", "ENDS"),
        Construct::ListLiteral => ("MATCH (b:bill) WHERE b.amount = [1, 2] RETURN b", "["),
        Construct::MapLiteral => ("MATCH (b:bill) WHERE b.amount = {a: 1} RETURN b", "{"),
        Construct::NamedPath => ("MATCH x = (b:bill)-[:entered_by]->(p) RETURN b", "x ="),
        Construct::ShortestPath => (
            "MATCH (b:bill), shortestPath((b)-[:entered_by*..3]->(p)) RETURN b",
            "shortestPath",
        ),
        Construct::RelationshipVariable => ("MATCH (b:bill)-[r:entered_by]->(p) RETURN b", "r:"),
        Construct::RelationshipProperties => {
            ("MATCH (b:bill)-[:entered_by {at: 1}]->(p) RETURN b", "{")
        }
        Construct::RelationAlternation => ("MATCH (b:bill)-[:entered_by|payee]->(p) RETURN b", "|"),
        Construct::SeveralLabels => ("MATCH (b:bill:person) RETURN b", ":person"),
        Construct::UnboundedLength => ("MATCH (b:bill)-[:entered_by*]->(p) RETURN b", "*"),
        Construct::CalendarDuration => (
            "MATCH (b:bill) WHERE b.at > duration('P1M') RETURN b",
            "duration",
        ),
        Construct::MatchFromOptional => (
            "MATCH (b:bill) OPTIONAL MATCH (b)-[:entered_by]->(p) MATCH (p)-[:works_at]->(c) RETURN b",
            "(p)-[:works_at]",
        ),
        Construct::OptionalFromOptional => (
            "MATCH (b:bill) OPTIONAL MATCH (b)-[:entered_by]->(p) OPTIONAL MATCH (p)-[:works_at]->(c) RETURN b",
            "(p)-[:works_at]",
        ),
        Construct::OptionalParts => (
            "MATCH (b:bill)-[:entered_by]->(p) OPTIONAL MATCH (b)-[:payee]->(q), (p)-[:works_at]->(c) RETURN b",
            "(c)",
        ),
        Construct::AggregateInExpression => (
            "MATCH (b:bill) RETURN sum(b.amount) / count(*) AS r",
            "count",
        ),
        Construct::Using => (
            "MATCH (b:bill) USING INDEX b:bill(amount) RETURN b",
            "USING",
        ),
        Construct::OrderByAggregate => (
            "MATCH (b:bill) RETURN b.payee, count(*) AS n ORDER BY b.payee",
            "ORDER",
        ),
        Construct::Unknown(name) => panic!("`{name}` is no construct this build knows"),
    }
}

#[test]
fn every_construct_outside_the_subset_is_refused_at_its_offset() {
    let all = [
        Construct::Create,
        Construct::Merge,
        Construct::Set,
        Construct::Delete,
        Construct::DetachDelete,
        Construct::Remove,
        Construct::Foreach,
        Construct::LoadCsv,
        Construct::Call,
        Construct::With,
        Construct::Union,
        Construct::Unwind,
        Construct::Use,
        Construct::Profile,
        Construct::SeveralStatements,
        Construct::Distinct,
        Construct::ListComprehension,
        Construct::PatternComprehension,
        Construct::MapProjection,
        Construct::CountSubquery,
        Construct::Exists,
        Construct::Case,
        Construct::Xor,
        Construct::Modulo,
        Construct::Power,
        Construct::EndsWith,
        Construct::ListLiteral,
        Construct::MapLiteral,
        Construct::NamedPath,
        Construct::ShortestPath,
        Construct::RelationshipVariable,
        Construct::RelationshipProperties,
        Construct::RelationAlternation,
        Construct::SeveralLabels,
        Construct::UnboundedLength,
        Construct::CalendarDuration,
        Construct::MatchFromOptional,
        Construct::OptionalFromOptional,
        Construct::OptionalParts,
        Construct::AggregateInExpression,
        Construct::Using,
        Construct::OrderByAggregate,
    ];
    for construct in &all {
        let (statement, marker) = refused_at(construct);
        let expected = json!({"code": "cypher_outside", "construct": construct.name(),
                              "offset": offset(statement, marker)});
        let reason = refused(statement, json!({}));
        let got = json!({"code": reason["code"], "construct": reason["construct"], "offset": reason["offset"]});
        assert_eq!(got, expected, "{statement}");
    }
}

#[test]
fn a_construct_is_refused_wherever_it_stands() {
    let cases = [
        // A clause after the first, in any case.
        ("MATCH (b:bill) with b RETURN b", "with", "with"),
        // Every other path function.
        (
            "MATCH (b:bill) RETURN allShortestPaths((b)-[:x*..2]->()) AS s, count(*) AS n",
            "shortest_path",
            "allShortestPaths",
        ),
        // A hint after a condition.
        (
            "MATCH (b:bill) WHERE b.amount > 1 USING SCAN b:bill RETURN b",
            "using",
            "USING",
        ),
        // `DISTINCT` inside an aggregate.
        (
            "MATCH (b:bill) RETURN count(DISTINCT b) AS n",
            "distinct",
            "DISTINCT",
        ),
        // A label a second clause gives otherwise.
        (
            "MATCH (b:bill)-[:entered_by]->(p:person) MATCH (p:company) RETURN b",
            "several_labels",
            "(p:company)",
        ),
        // `NOT EXISTS` that is no whole conjunct, or over more than one
        // relationship, at its `NOT`.
        (
            "MATCH (b:bill) WHERE b.amount > 1 OR NOT EXISTS { (b)-[:payee]->() } RETURN b",
            "exists",
            "NOT",
        ),
        (
            "MATCH (b:bill) WHERE NOT EXISTS { (b)-[:entered_by]->(p)-[:works_at]->(c) } RETURN b",
            "exists",
            "NOT",
        ),
        // A `MATCH` condition reading an optional name, at the name.
        (
            "MATCH (b:bill) OPTIONAL MATCH (b)-[:entered_by]->(p) MATCH (b)-[:payee]->(q) WHERE q.name = p.name RETURN b",
            "match_from_optional",
            "p.name",
        ),
        // A `MATCH` part beginning at an optional name.
        (
            "MATCH (b:bill) OPTIONAL MATCH (b)-[:entered_by]->(p) MATCH (p) RETURN b",
            "match_from_optional",
            "(p) RETURN",
        ),
        // An aggregate of no path.
        (
            "MATCH (b:bill) RETURN sum(b.amount * 2) AS s",
            "aggregate_in_expression",
            "sum",
        ),
        // A relationship's absence outside `WHERE`, at its `NOT`.
        (
            "MATCH (b:bill) RETURN NOT (b)-[:approved_by]->(:person) AS flag, count(*) AS n",
            "exists",
            "NOT",
        ),
        (
            "MATCH (b:bill) RETURN b.key ORDER BY NOT (b)-[:approved_by]->(:person)",
            "exists",
            "NOT",
        ),
    ];
    for (statement, construct, marker) in cases {
        let reason = refused(statement, json!({}));
        let got = json!({"code": reason["code"], "construct": reason["construct"], "offset": reason["offset"]});
        let expected = json!({"code": "cypher_outside", "construct": construct, "offset": offset(statement, marker)});
        assert_eq!(got, expected, "{statement}");
    }
}

#[test]
fn a_statement_that_does_not_parse_says_what_and_where() {
    let cases = [
        (
            "MATCH (b:bill RETURN b",
            json!({"code": "parse_expected", "token": ")"}),
            "RETURN",
        ),
        (
            "MATCH (b:bill) WHERE b.amount > RETURN b",
            json!({"code": "parse_keyword", "keyword": "RETURN"}),
            "RETURN",
        ),
        (
            "MATCH (b:bill) RETURN b.name = 'x",
            json!({"code": "parse_unclosed"}),
            "",
        ),
        (
            "MATCH (b:bill) WHERE b.amount > 1e3 RETURN b",
            json!({"code": "parse_number_into_name"}),
            "e3",
        ),
        (
            "MATCH (b:bill) WHERE b.amount ! 1 RETURN b",
            json!({"code": "parse_unexpected", "found": "!"}),
            "!",
        ),
        (
            "MATCH (b:bill) WHERE",
            json!({"code": "parse_ends_early"}),
            "",
        ),
        (
            "OPTIONAL MATCH (b:bill) RETURN b",
            json!({"code": "parse_expected", "token": "MATCH"}),
            "OPTIONAL",
        ),
        ("MATCH (b:bill)", json!({"code": "parse_ends_early"}), ""),
        (
            "MATCH (b:bill) WHERE b.amount = duration('soon') RETURN b",
            json!({"code": "parse_no_duration", "raw": "soon"}),
            "duration",
        ),
        (
            "MATCH (b:bill) WHERE b IS NULL RETURN b",
            json!({"code": "parse_expected", "token": "."}),
            "b IS",
        ),
        // A reserved word as a name, though the subset reads no `XOR`.
        (
            "MATCH (xor:bill) RETURN xor.key",
            json!({"code": "parse_keyword", "keyword": "XOR"}),
            "xor:",
        ),
    ];
    for (statement, expected, marker) in cases {
        let reason = refused(statement, json!({}));
        assert_eq!(
            reason["code"], "statement_not_parsed",
            "{statement}: {reason}"
        );
        let why = &reason["why"];
        for (k, v) in expected.as_object().expect("an object") {
            assert_eq!(&why[k], v, "{statement}: {why}");
        }
        let at = match marker {
            "" => statement.chars().count(),
            m => offset(statement, m),
        };
        assert_eq!(why["offset"], at, "{statement}: {why}");
    }
}

#[test]
fn an_offset_counts_characters_not_bytes() {
    let statement = "MATCH (b:bill) WHERE b.note = 'ü€' AND b.amount ! 1 RETURN b";
    let reason = refused(statement, json!({}));
    let why = &reason["why"];
    assert_eq!(why["code"], "parse_unexpected", "{why}");
    assert_eq!(why["offset"], 48, "{why}");
    let statement = "MATCH (b:bill) WHERE b.note = 'ü€' RETURN b.note SKIP 1";
    let reason = refused(statement, json!({}));
    assert_eq!(reason["code"], "cypher_skip", "{reason}");
    let statement = "MATCH (ä:bill) RETURN ä.key LIMIT 1.5";
    let reason = refused(statement, json!({}));
    assert_eq!(reason["offset"], 34, "{reason}");
}

#[test]
fn a_statement_says_what_it_reads_in_the_subsets_terms() {
    let cases = [
        (
            "MATCH (b) RETURN b",
            json!({"code": "cypher_start_type", "offset": 6}),
        ),
        (
            "MATCH (b:bill) MATCH (p:person) RETURN b",
            json!({"code": "cypher_unbound", "name": "p", "offset": 21}),
        ),
        (
            "MATCH (b:bill) RETURN q.name",
            json!({"code": "cypher_unbound", "name": "q", "offset": 22}),
        ),
        (
            "MATCH (b:bill) WHERE q.x = 1 RETURN b",
            json!({"code": "cypher_unbound", "name": "q", "offset": 21}),
        ),
        (
            "MATCH (b:bill) RETURN b SKIP 2",
            json!({"code": "cypher_skip", "skip": 2}),
        ),
        (
            "MATCH (b:bill) RETURN b.amount + 1",
            json!({"code": "cypher_row_item", "item": "b.amount + 1"}),
        ),
        (
            "MATCH (b:bill) RETURN b.amount AS a",
            json!({"code": "cypher_row_item", "item": "b.amount"}),
        ),
        (
            "MATCH (b:bill)-[:entered_by]->(p) RETURN b",
            json!({"code": "cypher_row_item", "item": "b"}),
        ),
        (
            "MATCH (b:bill) RETURN b, b.amount",
            json!({"code": "cypher_row_item", "item": "b"}),
        ),
        (
            "MATCH (b:bill) RETURN b ORDER BY b.amount + 1",
            json!({"code": "cypher_order_item", "item": "b.amount + 1"}),
        ),
        (
            "MATCH (b:bill) RETURN count(*)",
            json!({"code": "cypher_alias", "item": "count(*)"}),
        ),
        (
            "MATCH (b:bill) RETURN b.amount * 2, count(*) AS n",
            json!({"code": "cypher_alias", "item": "b.amount * 2"}),
        ),
        (
            "MATCH (b:bill) RETURN count(b.amount) AS n",
            json!({"code": "cypher_count", "item": "count(b.amount)"}),
        ),
        (
            "MATCH (b:bill) OPTIONAL MATCH (b)-[:payee]->(p) RETURN count(p) AS n",
            json!({"code": "cypher_count", "item": "count(p)"}),
        ),
        (
            "MATCH (b:bill) RETURN toUpper(b.name) AS n",
            json!({"code": "cypher_function", "name": "toUpper", "offset": 22}),
        ),
        (
            "MATCH (b:bill) RETURN apoc.x(b.name) AS n",
            json!({"code": "cypher_function", "name": "apoc.x", "offset": 22}),
        ),
        (
            "MATCH (b:bill) RETURN b.key SKIP 1.5",
            json!({"code": "cypher_whole", "clause": "SKIP", "offset": 33}),
        ),
        (
            "MATCH (b:bill) RETURN b.key LIMIT 1.5",
            json!({"code": "cypher_whole", "clause": "LIMIT", "offset": 34}),
        ),
        (
            "MATCH (b:bill) RETURN b.key LIMIT -1",
            json!({"code": "cypher_whole", "clause": "LIMIT", "offset": 34}),
        ),
    ];
    for (statement, expected) in cases {
        let reason = refused(statement, json!({}));
        for (k, v) in expected.as_object().expect("an object") {
            assert_eq!(&reason[k], v, "{statement}: {reason}");
        }
    }
}

#[test]
fn a_parameter_is_given_read_and_of_its_places_kind() {
    let cases = [
        (
            "MATCH (b:bill) WHERE b.amount = $z RETURN b",
            json!({}),
            json!({"code": "cypher_param_missing", "name": "z"}),
        ),
        (
            "MATCH (b:bill) RETURN b",
            json!({"z": 1}),
            json!({"code": "cypher_param_unused", "name": "z"}),
        ),
        // The first unused by name.
        (
            "MATCH (b:bill) WHERE b.amount = $a RETURN b",
            json!({"a": 1, "c": 2, "b": 3}),
            json!({"code": "cypher_param_unused", "name": "b"}),
        ),
        (
            "MATCH (b:bill {key: $k}) RETURN b",
            json!({"k": 7}),
            json!({"code": "cypher_param_value", "name": "k"}),
        ),
        (
            "MATCH (b:bill) WHERE b.amount = $o RETURN b",
            json!({"o": {"a": 1}}),
            json!({"code": "cypher_param_value", "name": "o"}),
        ),
        (
            "MATCH (b:bill) WHERE b.amount = $l RETURN b",
            json!({"l": [1]}),
            json!({"code": "cypher_param_value", "name": "l"}),
        ),
        (
            "MATCH (b:bill) WHERE b.tag IN $t RETURN b",
            json!({"t": "x"}),
            json!({"code": "cypher_param_value", "name": "t"}),
        ),
        (
            "MATCH (b:bill) WHERE b.tag IN $t RETURN b",
            json!({"t": [[1]]}),
            json!({"code": "cypher_param_value", "name": "t"}),
        ),
        (
            "MATCH (b:bill) RETURN b LIMIT $n",
            json!({"n": 1.5}),
            json!({"code": "cypher_param_value", "name": "n"}),
        ),
        (
            "MATCH (b:bill) RETURN b LIMIT $n",
            json!({"n": "5"}),
            json!({"code": "cypher_param_value", "name": "n"}),
        ),
        (
            "MATCH (b:bill) RETURN b SKIP $n",
            json!({"n": 3}),
            json!({"code": "cypher_skip", "skip": 3}),
        ),
    ];
    for (statement, given, expected) in cases {
        let reason = refused(statement, given);
        for (k, v) in expected.as_object().expect("an object") {
            assert_eq!(&reason[k], v, "{statement}: {reason}");
        }
    }
}

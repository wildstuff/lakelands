//! The door that takes a statement of the openCypher subset, checked as
//! the pattern query is: what it answers is what `query` answers for the
//! pattern the statement translates into, under the same rights, axes,
//! cursor and position; `EXPLAIN` answers that pattern and reads nothing;
//! and a refusal says its case, with the offset where it has one.
//!
//! The door is not a door of the catalog, so a check takes it beside the
//! lake as a [`Statements`]: the REST route over a network door, or
//! [`InProcess`] for a host that holds its lake and translates the
//! statement itself. [`statement_checks!`](crate::statement_checks)
//! expands to one `#[test]` per check.
//!
//! The MCP door serves the same statements as its `cypher` tool, listed
//! where `query` is. [`cypher_tool_checks!`](crate::cypher_tool_checks)
//! checks it against a [`Statements`] door over the same lake, through
//! [`Tools`]: a session of one principal, listing and calling.

use std::sync::Arc;

use async_trait::async_trait;
use lakelands_api::error::{Case, Construct, Error, Refusal, Said};
use lakelands_api::id::{Cursor, Position};
use lakelands_api::query::Pattern;
use lakelands_api::read::{Params, ReadAt};
use lakelands_api::tools::{Door, tools};
use lakelands_api::{Catalog, LakeStore, Principal};
use serde_json::{Value, json};

use crate::checks::{Fixture, patch, setup};

mod writes;

pub use writes::*;

/// Expands to one `#[test]` per statement check.
///
/// `$new` is called once per check and gives a fresh, empty lake and the
/// statement door over it, as `(Arc<dyn LakeStore>, Arc<dyn Statements>)`;
/// `$block_on` runs a future to completion.
///
/// ```ignore
/// lakelands_door_checks::statement_checks!(my_lake_and_door, my_runtime::block_on);
/// ```
#[macro_export]
macro_rules! statement_checks {
    ($new:expr, $block_on:path) => {
        $crate::statement_checks!(@each $new, $block_on;
            a_statement_is_answered_as_its_pattern,
            an_explained_statement_answers_its_pattern_and_reads_nothing,
            a_statement_outside_the_subset_is_refused_at_its_offset,
            a_parameter_the_statement_reads_is_given_beside_it,
            a_statement_nested_past_the_limit_is_refused_where_it_goes_deeper,
            a_merge_writes_on_the_declared_key_and_a_create_expects_none,
            a_relationship_is_merged_or_created_on_its_pair,
            a_change_is_answered_by_its_plan_and_enacted_with_its_id,
            a_link_is_changed_through_its_relationship,
            a_statement_of_two_writes_is_refused_naming_the_second,
            a_write_the_subset_does_not_take_is_refused_before_a_record_is_read,
            the_match_of_a_write_reads_as_its_plan_reads,
            a_written_value_is_its_parameter_never_text,
            what_a_write_is_not_sent_with_is_refused_naming_it);
    };
    (@each $new:expr, $block_on:path; $($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                let (lake, door) = ($new)();
                $block_on($crate::statements::$name(lake, door));
            }
        )*
    };
}

/// Expands to one `#[test]` per check of the `cypher` tool.
///
/// `$new` is called once per check and gives a fresh, empty lake, a
/// statement door over it and the MCP door's tools over it, as
/// `(Arc<dyn LakeStore>, Arc<dyn Statements>, Arc<dyn Tools>)`.
#[macro_export]
macro_rules! cypher_tool_checks {
    ($new:expr, $block_on:path) => {
        $crate::cypher_tool_checks!(@each $new, $block_on;
            the_cypher_tool_is_listed_where_query_is,
            a_statement_through_the_tool_answers_as_at_the_door,
            the_tool_pages_with_the_cursor_beside_the_statement,
            a_write_statement_is_refused_by_the_tool_naming_it,
            an_order_beside_an_aggregate_is_refused_in_the_statements_words,
            a_type_not_shown_is_answered_through_cypher_as_through_query,
            a_record_named_in_plain_words_is_found_through_describe_and_cypher,
            a_record_found_by_search_is_walked_from_by_its_key_through_cypher);
    };
    (@each $new:expr, $block_on:path; $($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                let (lake, door, tools) = ($new)();
                $block_on($crate::statements::$name(lake, door, tools));
            }
        )*
    };
}

/// A door that takes a statement of the openCypher subset. It answers as
/// the REST route does: the pattern's answer as `query` gives it, or, for
/// a statement that begins with `EXPLAIN`, `{"pattern": …}`.
#[async_trait]
pub trait Statements: Send + Sync {
    /// `statement` with `params`, as `who`, read at `at` and from `cursor`.
    async fn cypher(
        &self,
        who: &Principal,
        statement: &str,
        params: &Params,
        at: ReadAt,
        cursor: Option<&Cursor>,
    ) -> Result<Value, Error>;

    /// `statement` with `params`, as `who`, enacting the plan `plan_id`.
    async fn enact(
        &self,
        who: &Principal,
        statement: &str,
        params: &Params,
        plan_id: &str,
    ) -> Result<Value, Error>;
}

/// The statement door of a host that holds its lake in process: the
/// statement is run by [`lakelands_cypher::run`], as every door runs it.
pub struct InProcess(pub Arc<dyn Catalog>);

#[async_trait]
impl Statements for InProcess {
    async fn cypher(
        &self,
        who: &Principal,
        statement: &str,
        params: &Params,
        at: ReadAt,
        cursor: Option<&Cursor>,
    ) -> Result<Value, Error> {
        let c = self.0.as_ref();
        let cursor = cursor.cloned();
        let beside = lakelands_cypher::Beside {
            at,
            cursor,
            plan_id: None,
        };
        let answer = lakelands_cypher::run(c, who, statement, params, beside);
        Ok(serde_json::to_value(answer.await?).expect("an answer is JSON"))
    }

    async fn enact(
        &self,
        who: &Principal,
        statement: &str,
        params: &Params,
        plan_id: &str,
    ) -> Result<Value, Error> {
        let c = self.0.as_ref();
        let beside = lakelands_cypher::Beside {
            plan_id: Some(plan_id.to_owned()),
            ..Default::default()
        };
        let answer = lakelands_cypher::run(c, who, statement, params, beside);
        Ok(serde_json::to_value(answer.await?).expect("an answer is JSON"))
    }
}

/// The MCP door's tools, as a session of one principal lists and calls
/// them.
#[async_trait]
pub trait Tools: Send + Sync {
    /// What `tools/list` lists for `who`, each tool as listed.
    async fn list(&self, who: &Principal) -> Vec<Value>;

    /// `tools/call` of `name` with `arguments`, as `who`: the structured
    /// content of the answer, or the error a refused call carries.
    async fn call(&self, who: &Principal, name: &str, arguments: Value) -> Result<Value, Error>;
}

const STATEMENT: &str = "MATCH (i:invoice)-[:billed_by]->(p:partner) \
    WHERE i.status = $status RETURN i.key, i.note, p.name ORDER BY i.key LIMIT 2";

fn given(status: &str) -> Params {
    serde_json::from_value(json!({ "status": status })).expect("parameters")
}

fn at(as_of: Option<Position>) -> ReadAt {
    ReadAt {
        as_of,
        ..ReadAt::default()
    }
}

/// Two partners and the invoices they bill, three open and one paid;
/// each invoice carries a note the clerk is shown masked. The position
/// before the last open invoice is answered.
async fn billed(f: &Fixture) -> Position {
    let owner = &f.cast.owner;
    for (k, name) in [("P-1", "Acme"), ("P-2", "Brix")] {
        let fields = patch(&[("name", json!(name))]);
        let (ty, key) = ("partner".into(), k.into());
        let put = f
            .catalog
            .upsert(owner, &ty, &key, fields, Default::default());
        put.await.expect("the owner writes a partner");
    }
    let mut before = None;
    for (k, status, partner) in [
        ("INV-1", "open", "P-1"),
        ("INV-2", "paid", "P-1"),
        ("INV-3", "open", "P-2"),
        ("INV-4", "open", "P-1"),
    ] {
        if k == "INV-4" {
            before = Some(f.store.head().await.expect("the head"));
        }
        let fields = patch(&[
            ("status", json!(status)),
            ("partner", json!(partner)),
            ("note", json!("private")),
        ]);
        let (ty, key) = ("invoice".into(), k.into());
        let put = f
            .catalog
            .upsert(owner, &ty, &key, fields, Default::default());
        put.await.expect("the owner writes an invoice");
    }
    before.expect("the position before the last invoice")
}

/// An answer with its cursor said only as there or not: a cursor carries
/// the instant it was handed out at.
fn uncursored(mut answer: Value) -> Value {
    if let Some(c) = answer["data"].get_mut("next_cursor") {
        *c = json!(true);
    }
    answer
}

/// The pattern an `EXPLAIN` answers.
fn explained(answer: Value) -> Pattern {
    let p = answer.get("pattern").cloned().unwrap_or_else(|| {
        panic!("`EXPLAIN` answers a pattern, got {answer}");
    });
    serde_json::from_value(p).expect("the answer is a pattern")
}

/// A statement is answered as `query` answers the pattern it translates
/// into, as the same principal: a note the clerk is not shown is in neither,
/// a page continues by the cursor beside the statement, and a read at an
/// earlier position sees that position, both answering the position they
/// read at.
pub async fn a_statement_is_answered_as_its_pattern(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    let before = billed(&f).await;
    let clerk = &f.cast.clerk;
    let open = given("open");
    let query = |pattern: Pattern| {
        let c = f.catalog.clone();
        async move {
            let reply = c.query(clerk, pattern).await.expect("the pattern is read");
            serde_json::to_value(reply).expect("an answer is JSON")
        }
    };
    let explain = format!("EXPLAIN {STATEMENT}");

    let first = door.cypher(clerk, STATEMENT, &open, at(None), None).await;
    let first = first.expect("the statement is read");
    let pattern = door.cypher(clerk, &explain, &open, at(None), None).await;
    let pattern = explained(pattern.expect("the statement is explained"));
    assert_eq!(
        uncursored(first.clone()),
        uncursored(query(pattern).await),
        "the first page"
    );
    let rows = &first["data"]["rows"];
    let keys: Vec<&Value> = (0..3).map(|i| &rows[i]["i.key"]).collect();
    assert_eq!(
        keys,
        [&json!("INV-1"), &json!("INV-3"), &Value::Null],
        "{first}"
    );
    assert_eq!(rows[0]["p.name"], json!("Acme"), "{first}");
    assert_eq!(
        rows[0].get("i.note"),
        None,
        "the clerk is not shown the note: {first}"
    );
    let owner = door
        .cypher(&f.cast.owner, STATEMENT, &open, at(None), None)
        .await;
    let owner = owner.expect("the owner reads the statement");
    assert_eq!(
        owner["data"]["rows"][0]["i.note"],
        json!("private"),
        "{owner}"
    );

    let next = first["data"]["next_cursor"].clone();
    let next: Cursor = serde_json::from_value(next).expect("a second page");
    let second = door
        .cypher(clerk, STATEMENT, &open, at(None), Some(&next))
        .await;
    let second = second.expect("the second page is read");
    let pattern = door
        .cypher(clerk, &explain, &open, at(None), Some(&next))
        .await;
    let pattern = explained(pattern.expect("the second page is explained"));
    assert_eq!(
        pattern.cursor.as_ref(),
        Some(&next),
        "the cursor stands on the pattern"
    );
    assert_eq!(
        uncursored(second.clone()),
        uncursored(query(pattern).await),
        "the second page"
    );
    assert_eq!(
        second["data"]["rows"][0]["i.key"],
        json!("INV-4"),
        "{second}"
    );

    let earlier = door
        .cypher(clerk, STATEMENT, &open, at(Some(before)), None)
        .await;
    let earlier = earlier.expect("the statement is read as of before");
    let pattern = door
        .cypher(clerk, &explain, &open, at(Some(before)), None)
        .await;
    let pattern = explained(pattern.expect("the earlier read is explained"));
    assert_eq!(
        pattern.at.as_of,
        Some(before),
        "`as_of` stands on the pattern"
    );
    assert_eq!(
        uncursored(earlier.clone()),
        uncursored(query(pattern).await),
        "as of before"
    );
    assert_eq!(earlier["position"], json!(before.0), "{earlier}");
}

/// `EXPLAIN` answers the pattern and reads nothing: a statement over a
/// type the lake does not declare, or as of a position past the head, is
/// explained, where reading it is refused; and the pattern is the one the
/// statement is read as.
pub async fn an_explained_statement_answers_its_pattern_and_reads_nothing(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    let owner = &f.cast.owner;
    let open = given("open");
    let head = f.store.head().await.expect("the head");
    let past = at(Some(Position(head.0 + 100)));

    let pattern = door
        .cypher(owner, &format!("EXPLAIN {STATEMENT}"), &open, past, None)
        .await;
    let pattern = explained(pattern.expect("a position past the head is explained"));
    assert_eq!(pattern.at, past, "the axes stand on the pattern");
    assert_eq!(pattern.limit, Some(2));
    let read = door.cypher(owner, STATEMENT, &open, past, None).await;
    let refused = |e: &Error| {
        matches!(
            e,
            Error::Refused {
                reason: Refusal::OutOfRange { .. },
                ..
            }
        )
    };
    assert!(
        read.as_ref().is_err_and(refused),
        "read past the head: {read:?}"
    );

    let nowhere = "MATCH (x:ledger) RETURN x.key";
    let (explain, none) = (format!("EXPLAIN {nowhere}"), Params::new());
    let pattern = door.cypher(owner, &explain, &none, at(None), None).await;
    let pattern = explained(pattern.expect("a type not declared is explained"));
    assert_eq!(pattern.start.ty, Some("ledger".into()));
    let read = door.cypher(owner, nowhere, &none, at(None), None).await;
    assert!(read.is_err(), "a type not declared is not read: {read:?}");
    assert_eq!(
        f.store.head().await.expect("the head"),
        head,
        "nothing is appended"
    );
}

/// A construct outside the subset is refused naming it, at the character
/// it begins at, and a statement that does not parse is refused with
/// what went wrong and where, as any other refusal is.
pub async fn a_statement_outside_the_subset_is_refused_at_its_offset(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    let owner = &f.cast.owner;
    let none = Params::new();
    let union = "MATCH (i:invoice) RETURN i.key UNION MATCH (i:invoice) RETURN i.key";
    for s in [union.to_owned(), format!("EXPLAIN {union}")] {
        let e = door.cypher(owner, &s, &none, at(None), None).await;
        let offset = s.find("UNION").expect("a union");
        let case = Case::CypherOutside {
            construct: Construct::Union,
            offset,
        };
        assert_eq!(e.expect_err(&s), invalid(case), "{s}");
    }
    let open = "MATCH (é:invoice RETURN é.key";
    let e = door.cypher(owner, open, &none, at(None), None).await;
    let why = Case::ParseExpected {
        token: ")".into(),
        offset: 17,
    };
    let case = Case::StatementNotParsed { why: Box::new(why) };
    assert_eq!(
        e.expect_err(open),
        invalid(case),
        "an offset counts characters"
    );
}

/// The parameters a statement reads are given beside it, never in its
/// text: one the statement reads and is not given is refused naming it,
/// and so is one given and not read.
pub async fn a_parameter_the_statement_reads_is_given_beside_it(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    let owner = &f.cast.owner;
    let missing = door
        .cypher(owner, STATEMENT, &Params::new(), at(None), None)
        .await;
    let name = "status".into();
    let case = Case::CypherParamMissing { name };
    assert_eq!(missing.expect_err("no `$status`"), invalid(case));
    let mut more = given("open");
    more.extend(given_as("extra", "x"));
    let unused = door.cypher(owner, STATEMENT, &more, at(None), None).await;
    let name = "extra".into();
    let case = Case::CypherParamUnused { name };
    assert_eq!(unused.expect_err("`extra` is not read"), invalid(case));
    let object = serde_json::from_value(json!({ "status": { "is": "open" } }));
    let object = object.expect("parameters");
    let kind = door.cypher(owner, STATEMENT, &object, at(None), None).await;
    let name = "status".into();
    let case = Case::CypherParamValue { name };
    assert_eq!(kind.expect_err("an object is no status"), invalid(case));
    let quoted = "MATCH (i:invoice) WHERE i.status = '$status' RETURN i.key";
    let read = door
        .cypher(owner, quoted, &Params::new(), at(None), None)
        .await;
    let read = read.expect("a `$` inside a string is text");
    assert_eq!(
        read["data"]["rows"],
        json!([]),
        "no status is `$status`: {read}"
    );
}

/// A condition nests at most 32 levels deep: a parenthesis, a `NOT` and
/// a unary `-` each go one deeper. One level more is refused at the
/// character where it begins, however deep the statement goes on, and
/// the host that read it keeps answering.
pub async fn a_statement_nested_past_the_limit_is_refused_where_it_goes_deeper(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    let owner = &f.cast.owner;
    let none = Params::new();
    let at_limit = format!("{}true{}", "(".repeat(31), ")".repeat(31));
    for (open, inner, close, n, begins) in [
        ("(", "true", ")", 100_000, 56),
        ("(", "true", ")", 32, 56),
        ("NOT ", "false", "", 1_000, 152),
        ("- ", "1 = 1", "", 1_000, 88),
    ] {
        let (open, close) = (open.repeat(n), close.repeat(n));
        let s = format!("MATCH (i:invoice) WHERE {open}{inner}{close} RETURN i.key");
        let e = door.cypher(owner, &s, &none, at(None), None).await;
        let case = Case::CypherTooDeep {
            max: 32,
            offset: begins,
        };
        assert_eq!(e.expect_err("too deep"), invalid(case), "{}", &s[..80]);
    }
    let s = format!("MATCH (i:invoice) WHERE {at_limit} RETURN i.key ORDER BY i.key");
    let read = door.cypher(owner, &s, &none, at(None), None).await;
    let read = read.expect("32 levels are answered");
    assert_eq!(
        read["data"]["rows"].as_array().map(Vec::len),
        Some(4),
        "{read}"
    );
}

/// The names `tools/list` lists for `who`.
async fn listed(tools: &dyn Tools, who: &Principal) -> Vec<String> {
    let listed = tools.list(who).await;
    let name = |t: &Value| t["name"].as_str().expect("a tool is named").to_owned();
    listed.iter().map(name).collect()
}

/// The name the generator gives `door` for `who`, if it offers it.
async fn named(f: &Fixture, who: &Principal, door: &Door) -> Option<String> {
    let d = f.catalog.describe(who, ReadAt::default()).await;
    let d = d.expect("the principal is described").data;
    let offered = tools(&d).expect("the fixture's names are valid");
    offered
        .into_iter()
        .find(|t| t.door == *door)
        .map(|t| t.name)
}

/// The `cypher` tool through the MCP door, as `who`: `statement` with
/// `params`, and `cursor`.
async fn through_tool(
    (tools, f): (&dyn Tools, &Fixture),
    who: &Principal,
    statement: &str,
    params: &Params,
    cursor: Option<&Cursor>,
) -> Result<Value, Error> {
    let mut a = json!({ "statement": statement, "params": params });
    if let Some(c) = cursor {
        a["cursor"] = serde_json::to_value(c).expect("a cursor is JSON");
    }
    let name = named(f, &f.cast.owner, &Door::Cypher).await;
    tools
        .call(who, &name.expect("the owner is offered it"), a)
        .await
}

/// The `cypher` tool is listed exactly where `query` is, for every
/// principal of the cast, with its schema: the statement, its parameters,
/// the cursor and the two axes; and its description says what `ORDER BY`,
/// `LIMIT` and an aggregate may name, what the subset cannot spell, which
/// `query` can, and how the cursor is passed.
pub async fn the_cypher_tool_is_listed_where_query_is(
    lake: Arc<dyn LakeStore>,
    _door: Arc<dyn Statements>,
    tools: Arc<dyn Tools>,
) {
    let f = setup(lake).await;
    let c = &f.cast;
    let cast = [
        &c.owner,
        &c.clerk,
        &c.architect,
        &c.auditor,
        &c.reviewer,
        &c.importer,
        &c.stranger,
        &c.keeper,
        &c.tuner,
    ];
    for who in cast {
        let names = listed(tools.as_ref(), who).await;
        let cypher = named(&f, who, &Door::Cypher).await;
        let query = named(&f, who, &Door::Query).await;
        assert_eq!(cypher.is_some(), query.is_some(), "{}: {names:?}", who.id);
        for offered in [&cypher, &query].into_iter().flatten() {
            assert!(names.contains(offered), "{}: {names:?}", who.id);
        }
    }

    let owner = tools.list(&c.owner).await;
    let cypher = named(&f, &c.owner, &Door::Cypher).await;
    let tool = owner
        .iter()
        .find(|t| t["name"].as_str() == cypher.as_deref());
    let tool = tool.expect("the owner is listed the tool");
    let schema = &tool["inputSchema"];
    let members: Vec<&String> = schema["properties"]
        .as_object()
        .expect("the members")
        .keys()
        .collect();
    let want = ["as_of", "cursor", "params", "statement", "valid_at"];
    assert_eq!(members, want, "{schema}");
    assert_eq!(schema["required"], json!(["statement"]), "{schema}");
    let said = tool["description"].as_str().expect("a description");
    for words in [
        "ORDER BY",
        "LIMIT",
        "count(*)",
        "shortest",
        "degree",
        "`query`",
        "`next_cursor`",
        "the same statement and parameters",
        "An aggregate answers every group in one page with no cursor, ignoring `LIMIT`",
    ] {
        assert!(said.contains(words), "the description says {words}: {said}");
    }
}

/// A statement read through the tool answers what the statement door
/// answers, as the same principal: the owner the note, the clerk not.
pub async fn a_statement_through_the_tool_answers_as_at_the_door(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
    tools: Arc<dyn Tools>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    let open = given("open");
    for who in [&f.cast.owner, &f.cast.clerk] {
        let tool = through_tool((tools.as_ref(), &f), who, STATEMENT, &open, None).await;
        let tool = tool.expect("the tool reads the statement");
        let at_door = door.cypher(who, STATEMENT, &open, at(None), None).await;
        let at_door = at_door.expect("the door reads the statement");
        assert_eq!(uncursored(tool.clone()), uncursored(at_door), "{}", who.id);
        let rows = &tool["data"]["rows"];
        assert_eq!(rows[0]["i.key"], json!("INV-1"), "{tool}");
    }
}

/// A page past the first is read with the answer's `next_cursor` passed
/// beside the same statement and parameters, and is the page the
/// statement door reads from that cursor; the last page hands out none.
pub async fn the_tool_pages_with_the_cursor_beside_the_statement(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
    tools: Arc<dyn Tools>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    let (clerk, open) = (&f.cast.clerk, given("open"));
    let first = through_tool((tools.as_ref(), &f), clerk, STATEMENT, &open, None).await;
    let first = first.expect("the first page");
    let next = first["data"]["next_cursor"].clone();
    let next: Cursor = serde_json::from_value(next).expect("a second page");
    let second = through_tool((tools.as_ref(), &f), clerk, STATEMENT, &open, Some(&next)).await;
    let second = second.expect("the second page");
    let at_door = door
        .cypher(clerk, STATEMENT, &open, at(None), Some(&next))
        .await;
    let at_door = at_door.expect("the door reads the second page");
    assert_eq!(uncursored(second.clone()), uncursored(at_door));
    let rows = &second["data"]["rows"];
    assert_eq!(rows[0]["i.key"], json!("INV-4"), "{second}");
    assert_eq!(rows.as_array().map(Vec::len), Some(1), "{second}");
    assert_eq!(second["data"].get("next_cursor"), None, "{second}");
}

/// A statement that writes is refused by the tool naming its first
/// updating clause, at its offset, though the statement door takes it:
/// the tool writes nothing. A construct no door takes is refused by both
/// as outside the subset.
pub async fn a_write_statement_is_refused_by_the_tool_naming_it(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
    tools: Arc<dyn Tools>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    let (owner, none) = (&f.cast.owner, Params::new());
    let head = f.store.head().await.expect("the head");
    for (s, construct) in [
        ("MATCH (i:invoice) SET i.status = 'paid'", Construct::Set),
        ("MATCH (i:invoice) REMOVE i.note", Construct::Remove),
        ("MATCH (i:invoice) DELETE i", Construct::Delete),
        ("MERGE (i:invoice {key: 'INV-9'})", Construct::Merge),
        (
            "CREATE (i:invoice {key: 'INV-9'}) RETURN i",
            Construct::Create,
        ),
    ] {
        let word = construct.to_string();
        let clause = word.trim_start_matches('`').split('`').next();
        let offset = s.find(clause.expect("a clause")).expect("the clause");
        let case = Case::CypherToolWrites { construct, offset };
        let tool = through_tool((tools.as_ref(), &f), owner, s, &none, None).await;
        assert_eq!(tool.expect_err(s), invalid(case), "{s}");
    }
    let s = "MATCH (i:invoice) DETACH DELETE i";
    let offset = s.find("DETACH").expect("the clause");
    let construct = Construct::DetachDelete;
    let case = Case::CypherOutside { construct, offset };
    let tool = through_tool((tools.as_ref(), &f), owner, s, &none, None).await;
    assert_eq!(tool.expect_err(s), invalid(case.clone()), "{s}");
    let at_door = door.cypher(owner, s, &none, at(None), None).await;
    assert_eq!(at_door.expect_err(s), invalid(case), "{s}");
    let after = f.store.head().await.expect("the head");
    assert_eq!(after, head, "nothing is written");
    let s = "MERGE (i:invoice {key: 'INV-9'}) SET i.status = 'open'";
    let at_door = door.cypher(owner, s, &none, at(None), None).await;
    let at_door = at_door.expect("the statement door writes");
    assert_eq!(at_door["outcome"], json!("applied"), "{at_door}");
}

/// `ORDER BY` beside an aggregate is refused in the statement's words,
/// at the tool as at the statement door: the clause, at its offset, and
/// what to write instead. The statement written so is answered, its
/// groups in the order of the field returned beside them.
pub async fn an_order_beside_an_aggregate_is_refused_in_the_statements_words(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
    tools: Arc<dyn Tools>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    let (owner, none) = (&f.cast.owner, Params::new());
    let grouped = "MATCH (i:invoice)-[:billed_by]->(p:partner) \
        RETURN p.name, count(*) AS invoices";
    let s = format!("{grouped} ORDER BY p.name LIMIT 10");
    let offset = s.find("ORDER BY").expect("an order");
    let tool = through_tool((tools.as_ref(), &f), owner, &s, &none, None).await;
    let at_door = door.cypher(owner, &s, &none, at(None), None).await;
    for e in [tool.expect_err(&s), at_door.expect_err(&s)] {
        let wire = serde_json::to_value(&e).expect("an error is JSON");
        let reason = &wire["reason"];
        let got = json!([reason["code"], reason["construct"], reason["offset"]]);
        let want = json!(["cypher_outside", "order_by_aggregate", offset]);
        assert_eq!(got, want, "{wire}");
        let said = e.to_string();
        assert!(said.contains("leave `ORDER BY` out"), "{said}");
        assert!(!said.contains("order_by"), "{said}");
    }
    // A partner written last whose name sorts first: the groups come in
    // the order of the field beside the aggregate, not of the writes.
    for (ty, key, fields) in [
        ("partner", "P-3", patch(&[("name", json!("Abel"))])),
        (
            "invoice",
            "INV-5",
            patch(&[("status", json!("open")), ("partner", json!("P-3"))]),
        ),
    ] {
        let (ty, key) = (ty.into(), key.into());
        let put = f
            .catalog
            .upsert(owner, &ty, &key, fields, Default::default());
        put.await.expect("the owner writes it");
    }
    // `LIMIT` beside an aggregate caps nothing: every group, one page.
    for s in [grouped.to_string(), format!("{grouped} LIMIT 1")] {
        let read = through_tool((tools.as_ref(), &f), owner, &s, &none, None).await;
        let read = read.expect("the groups are read");
        let groups: Vec<Value> = read["data"]["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|r| json!([r["by"]["p.name"], r["values"]["invoices"]]))
            .collect();
        // A count comes as a decimal string.
        let want = [
            json!(["Abel", "1"]),
            json!(["Acme", "3"]),
            json!(["Brix", "1"]),
        ];
        assert_eq!(groups, want, "{s}: {read}");
        assert!(read["data"].get("next_cursor").is_none(), "{s}: {read}");
    }
}

/// A principal not shown a type is answered through `cypher` as through
/// `query` given the pattern the statement translates into: the stranger,
/// shown an invoice's status alone, reads it by either tool, and neither
/// reads a partner nor an invoice's amount.
pub async fn a_type_not_shown_is_answered_through_cypher_as_through_query(
    lake: Arc<dyn LakeStore>,
    door: Arc<dyn Statements>,
    tools: Arc<dyn Tools>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    let (stranger, none) = (&f.cast.stranger, Params::new());
    let mut answered = 0;
    for s in [
        "MATCH (i:invoice) RETURN i.status ORDER BY i.key",
        "MATCH (p:partner) RETURN p.name",
        "MATCH (i:invoice)-[:billed_by]->(p:partner) RETURN i.key, p.name",
        "MATCH (i:invoice) RETURN i.status, i.amount ORDER BY i.key",
    ] {
        let explain = format!("EXPLAIN {s}");
        let pattern = door.cypher(&f.cast.owner, &explain, &none, at(None), None);
        let pattern = pattern.await.expect("the owner explains it")["pattern"].take();
        let query = named(&f, stranger, &Door::Query).await;
        let query = query.expect("the stranger is offered `query`");
        let by_query = tools.call(stranger, &query, pattern).await;
        let by_cypher = through_tool((tools.as_ref(), &f), stranger, s, &none, None).await;
        match (by_cypher, by_query) {
            (Ok(c), Ok(q)) => {
                answered += 1;
                assert_eq!(uncursored(c.clone()), uncursored(q), "{s}");
                let statuses: Vec<&Value> = c["data"]["rows"]
                    .as_array()
                    .expect("rows")
                    .iter()
                    .map(|r| &r["i.status"])
                    .collect();
                let want = ["open", "paid", "open", "open"].map(|v| json!(v));
                assert_eq!(statuses, want.iter().collect::<Vec<_>>(), "{s}: {c}");
            }
            (c, q) => assert_eq!(c, q, "{s}"),
        }
    }
    assert_eq!(answered, 1, "the status alone is answered");
}

/// A record named in plain words is found through the MCP door alone, as
/// an agent finds it: `describe` names the relation that reaches the
/// type holding the name, a statement walks it against its direction
/// from the record of that name, and its every page is read through the
/// cursor beside the statement: the partner Acme's invoices, two a page.
pub async fn a_record_named_in_plain_words_is_found_through_describe_and_cypher(
    lake: Arc<dyn LakeStore>,
    _door: Arc<dyn Statements>,
    tools: Arc<dyn Tools>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    let clerk = &f.cast.clerk;
    let describe = named(&f, clerk, &Door::Describe).await;
    let describe = describe.expect("the clerk is offered `describe`");
    let d = tools.call(clerk, &describe, json!({})).await;
    let d = d.expect("the clerk is described the lake");
    let mut reaching = Vec::new();
    for t in d["data"]["types"].as_array().expect("the types") {
        for r in t["relations"].as_array().into_iter().flatten() {
            if r["target"] == json!("partner") {
                reaching.push((t["name"].clone(), r["name"].clone()));
            }
        }
    }
    assert_eq!(reaching, [(json!("invoice"), json!("billed_by"))], "{d}");
    let (ty, relation) = (&reaching[0].0, &reaching[0].1);
    let (ty, relation) = (
        ty.as_str().expect("a type"),
        relation.as_str().expect("a relation"),
    );
    let s = format!(
        "MATCH (p:partner {{name: $name}})<-[:{relation}]-(i:{ty}) \
         RETURN i.key, i.status ORDER BY i.key LIMIT 2"
    );
    let acme = given_as("name", "Acme");
    let (mut keys, mut cursor, mut pages) = (Vec::new(), None::<Cursor>, 0);
    loop {
        let page = through_tool((tools.as_ref(), &f), clerk, &s, &acme, cursor.as_ref()).await;
        let page = page.unwrap_or_else(|e| panic!("{s}: {e}"));
        pages += 1;
        for r in page["data"]["rows"].as_array().expect("rows") {
            keys.push(json!([r["i.key"], r["i.status"]]));
        }
        match page["data"].get("next_cursor") {
            Some(c) => cursor = Some(serde_json::from_value(c.clone()).expect("a cursor")),
            None => break,
        }
    }
    let want = [
        json!(["INV-1", "open"]),
        json!(["INV-2", "paid"]),
        json!(["INV-4", "open"]),
    ];
    assert_eq!(keys, want, "{s}");
    assert_eq!(pages, 2, "{s}");
}

/// A record found by a word of its text is walked from through the MCP
/// door alone, as an agent walks from it: `search` ranks the partner a
/// lower-cased word of its name matches first, and a statement walks the
/// relation against its direction from the hit's key: Acme's invoices.
pub async fn a_record_found_by_search_is_walked_from_by_its_key_through_cypher(
    lake: Arc<dyn LakeStore>,
    _door: Arc<dyn Statements>,
    tools: Arc<dyn Tools>,
) {
    let f = setup(lake).await;
    billed(&f).await;
    let clerk = &f.cast.clerk;
    let search = named(&f, clerk, &Door::Search).await;
    let search = search.expect("the clerk is offered `search`");
    let asked = json!({"type": "partner", "mode": "lexical", "text": "acme"});
    let hits = tools.call(clerk, &search, asked).await;
    let hits = hits.expect("the clerk's search is answered");
    let found: Vec<&Value> = hits["data"]["hits"]
        .as_array()
        .expect("the hits")
        .iter()
        .map(|h| &h["record"]["key"])
        .collect();
    assert_eq!(found, [&json!("P-1")], "{hits}");
    let key = found[0].as_str().expect("a key");
    let s = "MATCH (p:partner {key: $key})<-[:billed_by]-(i:invoice) \
             RETURN i.key, i.status ORDER BY i.key";
    let page = through_tool((tools.as_ref(), &f), clerk, s, &given_as("key", key), None).await;
    let page = page.unwrap_or_else(|e| panic!("{s}: {e}"));
    let rows: Vec<Value> = page["data"]["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| json!([r["i.key"], r["i.status"]]))
        .collect();
    let want = [
        json!(["INV-1", "open"]),
        json!(["INV-2", "paid"]),
        json!(["INV-4", "open"]),
    ];
    assert_eq!(rows, want, "{s}: {page}");
}

fn given_as(name: &str, value: &str) -> Params {
    serde_json::from_value(json!({ name: value })).expect("parameters")
}

fn invalid(case: Case) -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::Invalid(Box::new(Said::from(case))),
    }
}

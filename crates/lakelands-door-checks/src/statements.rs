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

use std::sync::Arc;

use async_trait::async_trait;
use lakelands_api::error::{Case, Construct, Error, Refusal, Said};
use lakelands_api::id::{Cursor, Position};
use lakelands_api::query::Pattern;
use lakelands_api::read::{Params, ReadAt};
use lakelands_api::{Catalog, LakeStore, Principal};
use lakelands_cypher::Statement;
use serde_json::{Value, json};

use crate::checks::{Fixture, patch, setup};

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
            a_parameter_the_statement_reads_is_given_beside_it);
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
}

/// The statement door of a host that holds its lake in process: the
/// statement is translated here and its pattern read through the catalog.
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
        let t = lakelands_cypher::translate(statement, params)?;
        let Statement::Read(mut p) = t.statement else {
            unreachable!("the subset translates a read alone")
        };
        (p.at, p.cursor) = (at, cursor.cloned());
        if t.explain {
            return Ok(json!({ "pattern": p }));
        }
        let reply = self.0.query(who, p).await?;
        Ok(serde_json::to_value(reply).expect("an answer is JSON"))
    }
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

fn given_as(name: &str, value: &str) -> Params {
    serde_json::from_value(json!({ name: value })).expect("parameters")
}

fn invalid(case: Case) -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::Invalid(Box::new(Said::from(case))),
    }
}

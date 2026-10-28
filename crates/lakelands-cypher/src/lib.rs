//! The openCypher subset Lakelands takes, translated into one
//! [`Pattern`] or one call of a typed write door: a door for a model
//! that already writes Cypher, onto the one read the store has and the
//! write doors it has.
//!
//! [`translate`] parses a statement of `MATCH`, `OPTIONAL MATCH`,
//! `WHERE` and `RETURN` and produces the pattern a host runs like any
//! other: [`run`] translates it and reads the pattern through
//! [`Catalog::query`], in process and behind a door alike. A statement
//! that writes, with `CREATE`, `MERGE`, `SET`, `REMOVE` or `DELETE`, is
//! one door call ([`Write`]), judged by the door as every other write
//! is. Nothing else here reads the store, and the store knows nothing of
//! Cypher. Every construct outside the subset is refused at its offset
//! with the construct named, never approximated, and the same statement
//! and parameters always give the same pattern or the same call.
//!
//! What differs from openCypher:
//!
//! - A row holds records, one per bound name, never paths or
//!   relationships; a relationship carries no variable and no map.
//! - A variable-length relationship matches a record once, at the
//!   fewest hops, and its upper bound is written.
//! - A part begins at the start's name or at one bound before it; the
//!   first node of the statement names its type, and the clauses are
//!   run in the order written.
//! - A condition goes to the step that binds the last name it reads.
//!   A step reads its own record's fields bare and every name bound
//!   before it as `<name>.<field>`; a condition on a name's `key`,
//!   `_type` or `_depth` goes to the next step of its clause, and is
//!   refused where there is none.
//! - The time axes and the cursor stand beside the statement, on the
//!   pattern, not in it.
//!
//! [`Catalog::query`]: lakelands_api::catalog::Catalog::query

use lakelands_api::error::{Case, Construct, Error, Result};
use lakelands_api::id::Cursor;
use lakelands_api::query::{Pattern, StatementAnswer};
use lakelands_api::{Catalog, Params, Principal, ReadAt};

use crate::parse::Kind;

mod expr;
mod lex;
mod parse;
mod read;
mod write;

pub use write::Write;

/// A statement translated.
#[derive(Clone, Debug, PartialEq)]
pub struct Translated {
    /// Whether the statement began with `EXPLAIN`: the host shows the
    /// pattern instead of running it.
    pub explain: bool,
    /// What the statement does.
    pub statement: Statement,
}

/// What a statement does.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Statement {
    /// A read: the pattern the host runs.
    Read(Pattern),
    /// A write: the one door it calls.
    Write(Write),
}

/// The statement `statement` with its parameters `params`, translated.
///
/// A statement that does not parse is refused as `statement_not_parsed`
/// with what went wrong and where, by character offset; a construct
/// outside the subset, or a parameter missing, unused or of a kind its
/// place does not take, is refused by its own case.
pub fn translate(statement: &str, params: &Params) -> Result<Translated> {
    let chars: Vec<char> = statement.chars().collect();
    let refuse = |case: Case| match case {
        Case::CypherAlias { .. }
        | Case::CypherCount { .. }
        | Case::CypherFunction { .. }
        | Case::CypherMergeOn { .. }
        | Case::CypherOrderItem { .. }
        | Case::CypherOutside { .. }
        | Case::CypherParamMissing { .. }
        | Case::CypherParamUnused { .. }
        | Case::CypherParamValue { .. }
        | Case::CypherRowItem { .. }
        | Case::CypherSecondWrite { .. }
        | Case::CypherSkip { .. }
        | Case::CypherStartType { .. }
        | Case::CypherTooDeep { .. }
        | Case::CypherUnbound { .. }
        | Case::CypherWhole { .. } => Error::invalid(case),
        why => Error::invalid(Case::StatementNotParsed { why: Box::new(why) }),
    };
    let toks = lex::lex(&chars).map_err(refuse)?;
    let mut parser = parse::Parser::new(toks, chars, params);
    let query = parser.query().map_err(refuse)?;
    let explain = query.explain;
    let writes =
        (query.clauses.iter()).any(|c| !matches!(c.kind, Kind::Match(_) | Kind::Return(_)));
    let statement = match writes {
        true => Statement::Write(Write(write::write(query).map_err(refuse)?)),
        false => Statement::Read(read::pattern(query).map_err(refuse)?),
    };
    if let Some(name) = params.keys().find(|k| !parser.used.contains(*k)) {
        let name = name.clone();
        return Err(Error::invalid(Case::CypherParamUnused { name }));
    }
    Ok(Translated { explain, statement })
}

/// The first updating clause of `statement` and where it stands, if it
/// has one before anything that does not parse: so a door that only
/// reads refuses a write by its clause, whatever else it holds.
pub fn writes(statement: &str) -> Option<(Construct, usize)> {
    let chars: Vec<char> = statement.chars().collect();
    let params = Params::new();
    let mut parser = parse::Parser::new(lex::lex(&chars).ok()?, chars, &params);
    let _ = parser.query();
    parser.wrote
}

/// What a statement is run with beside its parameters: a read's axes
/// and cursor, and a change's plan id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Beside {
    /// The axes a read is read at.
    pub at: ReadAt,
    /// Where a read continues.
    pub cursor: Option<Cursor>,
    /// The id of the plan a `MATCH … SET`, `REMOVE` or `DELETE` enacts;
    /// without one, the statement answers the plan.
    pub plan_id: Option<String>,
}

/// The statement `statement` with `params`, translated and run through
/// `catalog` as `who`: a read at `beside`'s axes and from its cursor, a
/// write as the one door call it is, a change planned or, with
/// `beside`'s plan id, enacted. Every door that takes a statement runs
/// it here.
pub async fn run(
    catalog: &dyn Catalog,
    who: &Principal,
    statement: &str,
    params: &Params,
    beside: Beside,
) -> Result<StatementAnswer> {
    let t = translate(statement, params)?;
    let Beside {
        at,
        cursor,
        plan_id,
    } = beside;
    let mut pattern = match t.statement {
        Statement::Write(Write(op)) => {
            let given = [
                (at.as_of.is_some(), "as_of"),
                (at.valid_at.is_some(), "valid_at"),
                (cursor.is_some(), "cursor"),
            ];
            if let Some((_, member)) = given.into_iter().find(|g| g.0) {
                let member = member.to_owned();
                return Err(Error::invalid(Case::CypherMember { member }));
            }
            return write::run(catalog, who, op, plan_id.as_deref()).await;
        }
        Statement::Read(pattern) => pattern,
    };
    if plan_id.is_some() {
        let member = "plan_id".to_owned();
        return Err(Error::invalid(Case::CypherMember { member }));
    }
    (pattern.at, pattern.cursor) = (at, cursor);
    if t.explain {
        return Ok(StatementAnswer::Explained {
            pattern: Box::new(pattern),
        });
    }
    Ok(StatementAnswer::Read(catalog.query(who, pattern).await?))
}

#[cfg(test)]
mod tests;

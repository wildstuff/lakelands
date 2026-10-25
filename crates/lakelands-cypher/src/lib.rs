//! The openCypher subset Lakelands reads, translated into one
//! [`Pattern`]: a door for a model that already writes Cypher, onto the
//! one read the store has.
//!
//! [`translate`] parses a statement of `MATCH`, `OPTIONAL MATCH`,
//! `WHERE` and `RETURN` and produces the pattern a host runs through
//! the store's `query` like any other; nothing here reads the store,
//! and the store knows nothing of Cypher. Every construct outside the
//! subset is refused at its offset with the construct named, never
//! approximated, and the same statement and parameters always give the
//! same pattern.
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

use lakelands_api::Params;
use lakelands_api::error::{Case, Error, Result};
use lakelands_api::query::Pattern;

mod expr;
mod lex;
mod parse;
mod read;

/// A statement translated.
#[derive(Clone, Debug, PartialEq)]
pub struct Translated {
    /// Whether the statement began with `EXPLAIN`: the host shows the
    /// pattern instead of running it.
    pub explain: bool,
    /// What the statement does.
    pub statement: Statement,
}

/// What a statement does. The subset reads; a write is refused.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Statement {
    /// A read: the pattern the host runs.
    Read(Pattern),
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
        | Case::CypherOrderItem { .. }
        | Case::CypherOutside { .. }
        | Case::CypherParamMissing { .. }
        | Case::CypherParamUnused { .. }
        | Case::CypherParamValue { .. }
        | Case::CypherRowItem { .. }
        | Case::CypherSkip { .. }
        | Case::CypherStartType { .. }
        | Case::CypherUnbound { .. } => Error::invalid(case),
        why => Error::invalid(Case::StatementNotParsed { why: Box::new(why) }),
    };
    let toks = lex::lex(&chars).map_err(refuse)?;
    let mut parser = parse::Parser::new(toks, chars, params);
    let query = parser.query().map_err(refuse)?;
    let explain = query.explain;
    let pattern = read::pattern(query).map_err(refuse)?;
    if let Some(name) = params.keys().find(|k| !parser.used.contains(*k)) {
        let name = name.clone();
        return Err(Error::invalid(Case::CypherParamUnused { name }));
    }
    Ok(Translated {
        explain,
        statement: Statement::Read(pattern),
    })
}

#[cfg(test)]
mod tests;

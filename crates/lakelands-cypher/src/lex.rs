//! The statement's tokens, each with the character offset it starts at
//! and the one after it ends.
//!
//! A word is an identifier or a keyword as written; the parser tells
//! them apart, case-insensitively, by [`RESERVED`]. A backtick-quoted
//! name is a word no keyword can be. Arrows are not tokens: `<`, `-` and
//! `>` are, and the pattern parser reads an arrow from them, so `a<-1`
//! in an expression stays a comparison.

use lakelands_api::error::Case;

/// Every reserved word of openCypher, the subset's and every other
/// clause's, so that a name that is one is backticked from the first
/// release, and the tokens do not change when the subset grows.
pub(crate) const RESERVED: [&str; 60] = [
    "ADD",
    "ALL",
    "AND",
    "AS",
    "ASC",
    "ASCENDING",
    "BY",
    "CALL",
    "CASE",
    "CONSTRAINT",
    "CONTAINS",
    "CREATE",
    "CSV",
    "DELETE",
    "DESC",
    "DESCENDING",
    "DETACH",
    "DISTINCT",
    "DO",
    "DROP",
    "ELSE",
    "END",
    "ENDS",
    "EXISTS",
    "EXPLAIN",
    "FALSE",
    "FOR",
    "FOREACH",
    "IN",
    "IS",
    "LIMIT",
    "LOAD",
    "MANDATORY",
    "MATCH",
    "MERGE",
    "NOT",
    "NULL",
    "OF",
    "ON",
    "OPTIONAL",
    "OR",
    "ORDER",
    "PROFILE",
    "REMOVE",
    "REQUIRE",
    "RETURN",
    "SCALAR",
    "SET",
    "SKIP",
    "STARTS",
    "THEN",
    "TRUE",
    "UNION",
    "UNIQUE",
    "UNWIND",
    "USE",
    "USING",
    "WHEN",
    "WHERE",
    "WITH",
];

/// What a token is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Tok {
    /// An identifier or a keyword, as written.
    Word(String),
    /// A backtick-quoted name, unquoted.
    Quoted(String),
    /// A string literal, unescaped.
    Str(String),
    /// A number, as written.
    Num(String),
    /// A parameter's name, without its `$`.
    Param(String),
    /// Punctuation or an operator.
    Punct(&'static str),
    /// The end of the statement.
    End,
}

/// A token and where it stands.
#[derive(Clone, Debug)]
pub(crate) struct Token {
    pub tok: Tok,
    pub at: usize,
    pub end: usize,
}

/// Longest first, so `<>` is read before `<`.
const PUNCT: [&str; 25] = [
    "<>", "<=", ">=", "=~", "..", "(", ")", "[", "]", "{", "}", ",", ".", ":", ";", "|", "=", "<",
    ">", "+", "-", "*", "/", "%", "^",
];

/// The statement's tokens, ending with [`Tok::End`].
pub(crate) fn lex(chars: &[char]) -> Result<Vec<Token>, Case> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        i = skip(chars, i)?;
        let at = i;
        let Some(&c) = chars.get(i) else {
            out.push(Token {
                tok: Tok::End,
                at,
                end: at,
            });
            return Ok(out);
        };
        let tok = if c.is_alphabetic() || c == '_' {
            i = word_end(chars, i);
            Tok::Word(chars[at..i].iter().collect())
        } else if c.is_ascii_digit() {
            i = number_end(chars, i)?;
            Tok::Num(chars[at..i].iter().collect())
        } else if c == '`' {
            let (name, next) = quoted(chars, i)?;
            i = next;
            Tok::Quoted(name)
        } else if c == '\'' || c == '"' {
            let (s, next) = string(chars, i)?;
            i = next;
            Tok::Str(s)
        } else if c == '$' {
            i += 1;
            let name = match chars.get(i) {
                Some('`') => {
                    let (name, next) = quoted(chars, i)?;
                    i = next;
                    name
                }
                Some(c) if c.is_alphanumeric() || *c == '_' => {
                    let start = i;
                    i = word_end(chars, i);
                    chars[start..i].iter().collect()
                }
                _ => {
                    let offset = at;
                    return Err(Case::ParseUnexpected { found: '$', offset });
                }
            };
            Tok::Param(name)
        } else {
            let rest = &chars[i..];
            let Some(p) = PUNCT
                .iter()
                .find(|p| rest.iter().take(p.len()).copied().eq(p.chars()))
            else {
                let offset = at;
                return Err(Case::ParseUnexpected { found: c, offset });
            };
            i += p.len();
            Tok::Punct(p)
        };
        out.push(Token { tok, at, end: i });
    }
}

/// Past whitespace and comments.
fn skip(chars: &[char], mut i: usize) -> Result<usize, Case> {
    loop {
        match (chars.get(i), chars.get(i + 1)) {
            (Some(c), _) if c.is_whitespace() => i += 1,
            (Some('/'), Some('/')) => {
                while chars.get(i).is_some_and(|c| *c != '\n') {
                    i += 1;
                }
            }
            (Some('/'), Some('*')) => {
                let close = (i + 2..chars.len().saturating_sub(1))
                    .find(|k| chars[*k] == '*' && chars[*k + 1] == '/');
                let Some(k) = close else {
                    let (token, offset) = ("*/".to_owned(), chars.len());
                    return Err(Case::ParseExpected { token, offset });
                };
                i = k + 2;
            }
            _ => return Ok(i),
        }
    }
}

fn word_end(chars: &[char], mut i: usize) -> usize {
    while chars
        .get(i)
        .is_some_and(|c| c.is_alphanumeric() || *c == '_')
    {
        i += 1;
    }
    i
}

/// A number: digits, and a fraction where a digit follows the point, so
/// `1..3` is a range. A letter run into it is refused, which keeps `1e3`
/// and `0x1f` from reading as something else.
fn number_end(chars: &[char], mut i: usize) -> Result<usize, Case> {
    let digits = |chars: &[char], mut i: usize| {
        while chars.get(i).is_some_and(char::is_ascii_digit) {
            i += 1;
        }
        i
    };
    i = digits(chars, i);
    if chars.get(i) == Some(&'.') && chars.get(i + 1).is_some_and(char::is_ascii_digit) {
        i = digits(chars, i + 1);
    }
    if chars.get(i).is_some_and(|c| c.is_alphabetic() || *c == '_') {
        return Err(Case::ParseNumberIntoName { offset: i });
    }
    Ok(i)
}

/// A backtick-quoted name; a backtick inside is written twice.
fn quoted(chars: &[char], mut i: usize) -> Result<(String, usize), Case> {
    let mut name = String::new();
    i += 1;
    loop {
        match (chars.get(i), chars.get(i + 1)) {
            (None, _) => {
                let (token, offset) = ("`".to_owned(), i);
                return Err(Case::ParseExpected { token, offset });
            }
            (Some('`'), Some('`')) => {
                name.push('`');
                i += 2;
            }
            (Some('`'), _) => return Ok((name, i + 1)),
            (Some(c), _) => {
                name.push(*c);
                i += 1;
            }
        }
    }
}

/// A string in single or double quotes, with openCypher's backslash
/// escapes.
fn string(chars: &[char], mut i: usize) -> Result<(String, usize), Case> {
    let quote = chars[i];
    let mut s = String::new();
    i += 1;
    loop {
        match chars.get(i) {
            None if quote == '\'' => return Err(Case::ParseUnclosed { offset: i }),
            None => {
                let (token, offset) = ("\"".to_owned(), i);
                return Err(Case::ParseExpected { token, offset });
            }
            Some(c) if *c == quote => return Ok((s, i + 1)),
            Some('\\') => {
                let (escaped, len) = match chars.get(i + 1) {
                    Some('u') => {
                        let hex: String = chars.iter().skip(i + 2).take(4).collect();
                        let c = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32);
                        (c.filter(|_| hex.len() == 4), 6)
                    }
                    Some(c) => match c {
                        '\\' | '\'' | '"' => (Some(*c), 2),
                        'n' => (Some('\n'), 2),
                        't' => (Some('\t'), 2),
                        'r' => (Some('\r'), 2),
                        'b' => (Some('\u{8}'), 2),
                        'f' => (Some('\u{c}'), 2),
                        _ => (None, 2),
                    },
                    None => (None, 1),
                };
                let Some(c) = escaped else {
                    let found = chars.get(i + 1).copied().unwrap_or('\\');
                    return Err(Case::ParseUnexpected { found, offset: i });
                };
                s.push(c);
                i += len;
            }
            Some(c) => {
                s.push(*c);
                i += 1;
            }
        }
    }
}

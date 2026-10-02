//! I-Regexp (RFC 9485), the interoperable regular-expression subset
//! `matches()` takes.
//!
//! A pattern is parsed by the RFC's grammar and refused if it steps
//! outside it — no `\d`, `\w`, `\b`, flags, lazy quantifiers, anchors or
//! back-references — and only then translated into the syntax of the
//! `regex` crate: every literal escaped, `.` as any character but a line
//! break (`\n`, `\r`), groups as non-capturing groups, and the whole
//! pattern anchored at both ends, because an I-Regexp always matches the
//! whole text. The translation is built from the parse, never spliced
//! from the text, so no pattern can escape its anchors.

use std::iter::Peekable;
use std::str::Chars;

/// Translate an I-Regexp into an anchored pattern for the `regex` crate,
/// or say why it is not an I-Regexp.
pub fn translate(pattern: &str) -> Result<String, String> {
    let mut p = Parser {
        chars: pattern.chars().peekable(),
        out: String::from("^(?:"),
    };
    p.regexp()?;
    if let Some(c) = p.chars.next() {
        return Err(format!("unexpected `{c}` (an unmatched `)`?)"));
    }
    p.out.push_str(")$");
    Ok(p.out)
}

struct Parser<'a> {
    chars: Peekable<Chars<'a>>,
    out: String,
}

/// Characters that stand for themselves outside a class.
fn is_normal(c: char) -> bool {
    !matches!(
        c,
        '(' | ')' | '*' | '+' | '.' | '?' | '[' | '\\' | ']' | '{' | '|' | '}'
    )
}

/// Characters a single-character escape may name.
fn is_single_escape(c: char) -> bool {
    matches!(
        c,
        '(' | ')'
            | '*'
            | '+'
            | '-'
            | '.'
            | '?'
            | '['
            | '\\'
            | ']'
            | '^'
            | 'n'
            | 'r'
            | 't'
            | '{'
            | '|'
            | '}'
    )
}

/// The Unicode general categories `\p{…}` may name.
fn is_category(name: &str) -> bool {
    let mut c = name.chars();
    let (Some(major), minor) = (c.next(), c.next()) else {
        return false;
    };
    if c.next().is_some() {
        return false;
    }
    let minors = match major {
        'L' => "lmotu",
        'M' => "cen",
        'N' => "dlo",
        'P' => "cdefios",
        'Z' => "lps",
        'S' => "ckmo",
        'C' => "cfno",
        _ => return false,
    };
    minor.is_none_or(|m| minors.contains(m))
}

fn escaped(c: char) -> String {
    match c {
        'n' => "\\n".into(),
        'r' => "\\r".into(),
        't' => "\\t".into(),
        c => regex::escape(&c.to_string()),
    }
}

/// A character inside a `regex` class, escaped where the crate would read
/// it as syntax.
fn in_class(c: char) -> String {
    match c {
        '\\' | ']' | '[' | '^' | '-' | '&' | '~' => format!("\\{c}"),
        '\n' => "\\n".into(),
        '\r' => "\\r".into(),
        '\t' => "\\t".into(),
        c => c.to_string(),
    }
}

impl Parser<'_> {
    fn regexp(&mut self) -> Result<(), String> {
        self.branch()?;
        while self.chars.peek() == Some(&'|') {
            self.chars.next();
            self.out.push('|');
            self.branch()?;
        }
        Ok(())
    }

    fn branch(&mut self) -> Result<(), String> {
        while let Some(&c) = self.chars.peek() {
            if c == '|' || c == ')' {
                return Ok(());
            }
            self.atom()?;
            self.quantifier()?;
        }
        Ok(())
    }

    fn atom(&mut self) -> Result<(), String> {
        let c = self.chars.next().expect("caller peeked");
        match c {
            '(' => {
                self.out.push_str("(?:");
                self.regexp()?;
                if self.chars.next() != Some(')') {
                    return Err("a group is not closed with `)`".into());
                }
                self.out.push(')');
            }
            '.' => self.out.push_str("[^\\n\\r]"),
            '[' => self.class()?,
            '\\' => {
                let e = self.escape()?;
                self.out.push_str(&e);
            }
            c if is_normal(c) => self.out.push_str(&regex::escape(&c.to_string())),
            c => return Err(format!("`{c}` must be escaped")),
        }
        Ok(())
    }

    /// After a `\`: a single-character escape or a category escape, as it
    /// is written outside a class.
    fn escape(&mut self) -> Result<String, String> {
        match self.chars.next() {
            Some(p @ ('p' | 'P')) => {
                if self.chars.next() != Some('{') {
                    return Err(format!("`\\{p}` must be followed by `{{`"));
                }
                let name: String = self.chars.by_ref().take_while(|c| *c != '}').collect();
                if !is_category(&name) {
                    return Err(format!("`{name}` is not a Unicode general category"));
                }
                Ok(format!("\\{p}{{{name}}}"))
            }
            Some(c) if is_single_escape(c) => Ok(escaped(c)),
            Some(c) => Err(format!("`\\{c}` is not an I-Regexp escape")),
            None => Err("the pattern ends with `\\`".into()),
        }
    }

    fn quantifier(&mut self) -> Result<(), String> {
        match self.chars.peek() {
            Some(&q @ ('*' | '+' | '?')) => {
                self.chars.next();
                self.out.push(q);
            }
            Some('{') => {
                self.chars.next();
                let body: String = self.chars.by_ref().take_while(|c| *c != '}').collect();
                let (lo, hi) = match body.split_once(',') {
                    Some((lo, hi)) => (lo, Some(hi)),
                    None => (body.as_str(), None),
                };
                let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
                if !digits(lo) || hi.is_some_and(|h| !h.is_empty() && !digits(h)) {
                    return Err(format!("`{{{body}}}` is not a repetition"));
                }
                if let Some(h) = hi.filter(|h| !h.is_empty()) {
                    let (a, b): (u64, u64) =
                        (lo.parse().unwrap_or(u64::MAX), h.parse().unwrap_or(0));
                    if a > b {
                        return Err(format!("`{{{body}}}` repeats at least more than at most"));
                    }
                }
                self.out.push('{');
                self.out.push_str(&body);
                self.out.push('}');
            }
            _ => return Ok(()),
        }
        if matches!(self.chars.peek(), Some('*' | '+' | '?' | '{')) {
            return Err("a quantifier cannot follow a quantifier".into());
        }
        Ok(())
    }

    /// One character of a class, or `None` for a category escape (written
    /// to `out` directly).
    fn class_char(&mut self, out: &mut String) -> Result<Option<char>, String> {
        match self.chars.next() {
            Some('\\') => match self.chars.peek() {
                Some('p' | 'P') => {
                    let e = self.escape()?;
                    out.push_str(&e);
                    Ok(None)
                }
                _ => match self.chars.next() {
                    Some(c) if is_single_escape(c) => Ok(Some(match c {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        c => c,
                    })),
                    Some(c) => Err(format!("`\\{c}` is not an I-Regexp escape")),
                    None => Err("the pattern ends with `\\`".into()),
                },
            },
            Some(c @ ('[' | ']')) => Err(format!("`{c}` must be escaped inside a class")),
            Some(c) => Ok(Some(c)),
            None => Err("a class is not closed with `]`".into()),
        }
    }

    fn class(&mut self) -> Result<(), String> {
        let mut out = String::from("[");
        if self.chars.peek() == Some(&'^') {
            self.chars.next();
            out.push('^');
        }
        let mut first = true;
        loop {
            match self.chars.peek() {
                None => return Err("a class is not closed with `]`".into()),
                Some(']') if !first => {
                    self.chars.next();
                    break;
                }
                Some('-') => {
                    // A `-` stands for itself only first or last.
                    self.chars.next();
                    if !first && self.chars.peek() != Some(&']') {
                        return Err("`-` inside a class must be first, last or escaped".into());
                    }
                    out.push_str("\\-");
                }
                _ => {
                    let Some(lo) = self.class_char(&mut out)? else {
                        first = false;
                        continue;
                    };
                    let mut look = self.chars.clone();
                    if look.next() == Some('-') && !matches!(look.peek(), Some(']') | None) {
                        self.chars.next();
                        let Some(hi) = self.class_char(&mut String::new())? else {
                            return Err("a range cannot end in a category".into());
                        };
                        if lo > hi {
                            return Err(format!("the range `{lo}-{hi}` runs backwards"));
                        }
                        out.push_str(&format!("{}-{}", in_class(lo), in_class(hi)));
                    } else {
                        out.push_str(&in_class(lo));
                    }
                }
            }
            first = false;
        }
        out.push(']');
        self.out.push_str(&out);
        Ok(())
    }
}

#[cfg(test)]
mod tests;

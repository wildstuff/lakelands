//! Per-language lexical stemming for full-text search.
//!
//! A type's full-text index may name the language its content is in, as a
//! BCP-47 tag (`de`, `en`, `de-CH`). Only the primary language subtag
//! selects a stemmer, so `de-CH` stems as `de`; a language without a
//! stemmer, or no tag at all, means no stemming: text is split into
//! tokens, lower-cased and folded only.
//!
//! ## Why this is the store's and not an engine's
//!
//! Every engine must return the same match set for the same query, and
//! the match set is a function of the token stream. So the token pipeline
//! is shared domain logic, not a mechanic an engine may own: each engine
//! pre-stems the text it indexes and the terms it queries with these
//! functions, on the store's side of the seam. Two copies would be two
//! chances for "Rechnung" to find "Rechnungen" on one engine and not on
//! another.
//!
//! ## Why pre-processing and not an in-database tokenizer
//!
//! A full-text match returns record keys and the store then reads the
//! records. An engine never reads the indexed text back, so it needs no
//! token offsets into the original text, the one thing a native tokenizer
//! adds. A pure pre-processing step gives the same recall on every engine
//! with no native extension to install. Because the same transformation is
//! applied to the indexed text and to the query, a query's stem meets the
//! stored stem.
//!
//! Where a hit answers which of its tokens matched, the store walks the
//! record it already holds with the same loop ([`walk`]), which keeps each
//! token's offsets beside its term: what is marked is what was counted.

mod snowball;

pub use snowball::{stem_text, stem_tokens};

/// The one token loop of the pipeline: each run of alphanumeric
/// characters (Unicode-aware) of `text`, its first char and the char
/// after its last counted in Unicode scalar values, with the term `stem`
/// makes of it lower-cased.
pub(crate) fn walk<'t>(
    text: &'t str,
    stem: impl Fn(&str) -> String + 't,
) -> impl Iterator<Item = (usize, usize, String)> + 't {
    let mut chars = text.char_indices().enumerate().peekable();
    std::iter::from_fn(move || {
        let (start, from) = loop {
            match chars.next()? {
                (n, (b, c)) if c.is_alphanumeric() => break (n, b),
                _ => {}
            }
        };
        let (mut end, mut to) = (start + 1, text.len());
        while let Some(&(n, (b, c))) = chars.peek() {
            if !c.is_alphanumeric() {
                to = b;
                break;
            }
            end = n + 1;
            chars.next();
        }
        Some((start, end, stem(&text[from..to].to_lowercase())))
    })
}

#[cfg(test)]
mod tests;

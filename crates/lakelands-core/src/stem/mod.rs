//! Per-language lexical stemming for full-text search.
//!
//! A type's full-text index may name the language its content is in, as a
//! BCP-47 tag (`de`, `en`, `de-CH`). Only the primary language subtag
//! selects a stemmer, so `de-CH` stems as `de`; a language without a
//! stemmer, or no tag at all, means no stemming: text is split into
//! tokens and lower-cased only.
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
//! records; it never reads the indexed text back, so token offsets into
//! the original text (the one thing a native tokenizer adds) are unused.
//! A pure pre-processing step gives the same recall on every engine with
//! no native extension to install. Because the same transformation is
//! applied to the indexed text and to the query, a query's stem meets the
//! stored stem.

mod snowball;

pub use snowball::stem_text;

/// The token loop every form of the pipeline shares: split on runs of
/// characters that are not alphanumeric (Unicode-aware), lower-case each
/// token, pass it through `stem`, and rejoin with single spaces.
fn normalize(text: &str, stem: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    for token in text.split(|c: char| !c.is_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&stem(&token.to_lowercase()));
    }
    out
}

/// The pipeline without a stemmer: tokens, lower-cased, joined by single
/// spaces. What text gets when its language has no stemmer.
pub fn tokenize_lowercase(text: &str) -> String {
    normalize(text, str::to_owned)
}

#[cfg(test)]
mod tests;

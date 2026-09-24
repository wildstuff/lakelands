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

/// A language with a stemmer: the closed set of Snowball algorithms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Language {
    /// `ar`.
    Arabic,
    /// `da`.
    Danish,
    /// `nl`.
    Dutch,
    /// `en`.
    English,
    /// `fi`.
    Finnish,
    /// `fr`.
    French,
    /// `de`.
    German,
    /// `el`.
    Greek,
    /// `hu`.
    Hungarian,
    /// `it`.
    Italian,
    /// `no`, `nb` or `nn`.
    Norwegian,
    /// `pt`.
    Portuguese,
    /// `ro`.
    Romanian,
    /// `ru`.
    Russian,
    /// `es`.
    Spanish,
    /// `sv`.
    Swedish,
    /// `ta`.
    Tamil,
    /// `tr`.
    Turkish,
}

impl Language {
    /// The language a BCP-47 tag selects, by its primary language subtag
    /// (case-insensitive). `None` when that language has no stemmer.
    pub(crate) fn for_tag(tag: &str) -> Option<Language> {
        let primary = tag.split('-').next()?.to_ascii_lowercase();
        Some(match primary.as_str() {
            "ar" => Language::Arabic,
            "da" => Language::Danish,
            "nl" => Language::Dutch,
            "en" => Language::English,
            "fi" => Language::Finnish,
            "fr" => Language::French,
            "de" => Language::German,
            "el" => Language::Greek,
            "hu" => Language::Hungarian,
            "it" => Language::Italian,
            "no" | "nb" | "nn" => Language::Norwegian,
            "pt" => Language::Portuguese,
            "ro" => Language::Romanian,
            "ru" => Language::Russian,
            "es" => Language::Spanish,
            "sv" => Language::Swedish,
            "ta" => Language::Tamil,
            "tr" => Language::Turkish,
            _ => return None,
        })
    }
}

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
mod tests {
    use super::*;

    #[test]
    fn the_primary_subtag_selects_the_language() {
        assert_eq!(Language::for_tag("de"), Some(Language::German));
        assert_eq!(Language::for_tag("de-CH"), Some(Language::German));
        assert_eq!(Language::for_tag("EN-gb"), Some(Language::English));
        assert_eq!(Language::for_tag("nb-NO"), Some(Language::Norwegian));
        assert_eq!(Language::for_tag("zh-Hant"), None);
        assert_eq!(Language::for_tag(""), None);
    }

    #[test]
    fn tokenizing_drops_punctuation_and_lower_cases() {
        assert_eq!(
            tokenize_lowercase("Die offenen Rechnungen!"),
            "die offenen rechnungen"
        );
        assert_eq!(tokenize_lowercase("  EK-2026-0102  "), "ek 2026 0102");
        assert_eq!(tokenize_lowercase("?!"), "");
    }
}

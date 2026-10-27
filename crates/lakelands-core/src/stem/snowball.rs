//! The stemmers and the diacritic fold behind [`stem_tokens`] and
//! [`fold_diacritics`].

use rust_stemmers::{Algorithm, Stemmer};
use unicode_normalization::UnicodeNormalization;

use super::walk;

/// The Snowball algorithm a BCP-47 tag selects, by its primary language
/// subtag (case-insensitive). `None` when that language has no stemmer.
pub(super) fn algorithm(tag: &str) -> Option<Algorithm> {
    let primary = tag.split('-').next()?.to_ascii_lowercase();
    Some(match primary.as_str() {
        "ar" => Algorithm::Arabic,
        "da" => Algorithm::Danish,
        "nl" => Algorithm::Dutch,
        "en" => Algorithm::English,
        "fi" => Algorithm::Finnish,
        "fr" => Algorithm::French,
        "de" => Algorithm::German,
        "el" => Algorithm::Greek,
        "hu" => Algorithm::Hungarian,
        "it" => Algorithm::Italian,
        "no" | "nb" | "nn" => Algorithm::Norwegian,
        "pt" => Algorithm::Portuguese,
        "ro" => Algorithm::Romanian,
        "ru" => Algorithm::Russian,
        "es" => Algorithm::Spanish,
        "sv" => Algorithm::Swedish,
        "ta" => Algorithm::Tamil,
        "tr" => Algorithm::Turkish,
        _ => return None,
    })
}

/// The whole token pipeline for the language of a BCP-47 tag: each token
/// of `text`, where it stands in chars, its end exclusive, and its term:
/// the token lower-cased, stemmed, then its diacritics folded. A tag
/// whose language has no stemmer only lower-cases and folds.
///
/// Stemming runs before the fold because the German rules need their
/// umlauts. An engine indexes exactly the terms this returns and searches
/// with exactly those it returns for the query; it folds nothing itself
/// (`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
///
/// It is applied to every indexed text, identifiers included. That is
/// safe: an identifier such as `EK-2026-0102` stems the same way on the
/// index and the query side, so an exact lookup still matches.
pub fn stem_tokens<'t>(
    text: &'t str,
    tag: &str,
) -> impl Iterator<Item = (usize, usize, String)> + 't {
    let stemmer = algorithm(tag).map(Stemmer::create);
    walk(text, move |t| match &stemmer {
        Some(stemmer) => fold_diacritics(&stemmer.stem(t)),
        None => fold_diacritics(t),
    })
}

/// Strips combining marks after canonical decomposition: `Grüße` becomes
/// `Gruße`, `café` becomes `cafe`; `ß` is a letter and stays.
///
/// The last step of [`stem_tokens`], so every engine matches accents the
/// same way. Decomposition covers what decomposes, which is every Latin
/// letter that matters here.
pub(crate) fn fold_diacritics(text: &str) -> String {
    // ASCII decomposes to itself and holds no combining mark.
    if text.is_ascii() {
        return text.to_owned();
    }
    text.nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect()
}

#[cfg(test)]
mod tests;

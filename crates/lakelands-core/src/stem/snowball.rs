//! The stemmers and the diacritic fold behind [`stem_text`] and
//! [`fold_diacritics`].

use rust_stemmers::{Algorithm, Stemmer};
use unicode_normalization::UnicodeNormalization;

use super::{normalize, tokenize_lowercase};

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

/// The whole token pipeline for the language of a BCP-47 tag: tokens,
/// lower-cased, each stemmed, then diacritics folded, joined by single
/// spaces. A tag whose language has no stemmer is only split,
/// lower-cased and folded.
///
/// Stemming runs before the fold because the German rules need their
/// umlauts. An engine indexes exactly what this returns and searches with
/// exactly what it returns for the query; it folds nothing itself
/// (`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
///
/// It is applied to every indexed text, identifiers included. That is
/// safe: an identifier such as `EK-2026-0102` stems the same way on the
/// index and the query side, so an exact lookup still matches.
pub fn stem_text(text: &str, tag: &str) -> String {
    let stemmed = match algorithm(tag) {
        Some(algorithm) => {
            let stemmer = Stemmer::create(algorithm);
            normalize(text, |t| stemmer.stem(t).into_owned())
        }
        None => tokenize_lowercase(text),
    };
    fold_diacritics(&stemmed)
}

/// Strips combining marks after canonical decomposition: `Grüße` becomes
/// `Gruße`, `café` becomes `cafe`; `ß` is a letter and stays.
///
/// The last step of [`stem_text`], so every engine matches accents the
/// same way. Decomposition covers what decomposes, which is every Latin
/// letter that matters here.
pub(crate) fn fold_diacritics(text: &str) -> String {
    text.nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect()
}

#[cfg(test)]
mod tests;

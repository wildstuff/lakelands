//! The stemmers and the diacritic fold behind [`stem_text`] and
//! [`fold_diacritics`].

use rust_stemmers::{Algorithm, Stemmer};
use unicode_normalization::UnicodeNormalization;

use super::{Language, normalize, tokenize_lowercase};

/// The Snowball algorithm for a language. The match is exhaustive, so a
/// language added to [`Language`] cannot silently fall through to the
/// wrong stemmer.
fn algorithm(lang: Language) -> Algorithm {
    match lang {
        Language::Arabic => Algorithm::Arabic,
        Language::Danish => Algorithm::Danish,
        Language::Dutch => Algorithm::Dutch,
        Language::English => Algorithm::English,
        Language::Finnish => Algorithm::Finnish,
        Language::French => Algorithm::French,
        Language::German => Algorithm::German,
        Language::Greek => Algorithm::Greek,
        Language::Hungarian => Algorithm::Hungarian,
        Language::Italian => Algorithm::Italian,
        Language::Norwegian => Algorithm::Norwegian,
        Language::Portuguese => Algorithm::Portuguese,
        Language::Romanian => Algorithm::Romanian,
        Language::Russian => Algorithm::Russian,
        Language::Spanish => Algorithm::Spanish,
        Language::Swedish => Algorithm::Swedish,
        Language::Tamil => Algorithm::Tamil,
        Language::Turkish => Algorithm::Turkish,
    }
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
    let stemmed = match Language::for_tag(tag) {
        Some(lang) => {
            let stemmer = Stemmer::create(algorithm(lang));
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

use rust_stemmers::Algorithm;

use super::snowball::algorithm;
use super::*;

#[test]
fn the_primary_subtag_selects_the_language() {
    assert_eq!(algorithm("de"), Some(Algorithm::German));
    assert_eq!(algorithm("de-CH"), Some(Algorithm::German));
    assert_eq!(algorithm("EN-gb"), Some(Algorithm::English));
    assert_eq!(algorithm("nb-NO"), Some(Algorithm::Norwegian));
    assert_eq!(algorithm("zh-Hant"), None);
    assert_eq!(algorithm(""), None);
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

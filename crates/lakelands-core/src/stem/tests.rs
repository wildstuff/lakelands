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

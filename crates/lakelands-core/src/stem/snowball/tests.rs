use super::*;

#[test]
fn the_pipeline_folds_accents_so_no_engine_has_to() {
    assert_eq!(stem_text("Café", "en"), stem_text("cafe", "en"));
    assert_eq!(stem_text("Grüße", "xx"), "gruße");
}

#[test]
fn german_plural_and_singular_share_a_stem() {
    // A stemmer, not a lemmatiser: the noun forms converge, which is
    // what search relies on.
    assert_eq!(stem_text("Rechnungen", "de"), stem_text("Rechnung", "de"));
    assert_eq!(stem_text("Kunden", "de"), stem_text("Kunde", "de"));
}

#[test]
fn a_regional_tag_stems_as_its_language() {
    assert_eq!(
        stem_text("Rechnungen", "de-CH"),
        stem_text("Rechnung", "de")
    );
}

#[test]
fn an_unknown_language_is_not_stemmed() {
    assert_eq!(stem_text("Rechnungen!", "zh"), "rechnungen");
    assert_eq!(stem_text("Rechnungen!", ""), "rechnungen");
}

#[test]
fn several_words_keep_their_count_and_each_is_stemmed() {
    let stemmed = stem_text("Die offenen Rechnungen!", "de");
    let tokens: Vec<&str> = stemmed.split(' ').collect();
    assert_eq!(tokens.len(), 3, "{stemmed:?}");
    assert_eq!(tokens[2], stem_text("Rechnung", "de"));
}

#[test]
fn punctuation_and_case_do_not_defeat_stemming() {
    assert_eq!(stem_text("Rechnungen!", "de"), stem_text("rechnung", "de"));
}

#[test]
fn an_identifier_stems_the_same_on_both_sides() {
    let id = "EK-2026-0102";
    assert_eq!(stem_text(id, "de"), stem_text(id, "de"));
}

#[test]
fn tokenizing_is_the_pipeline_without_the_stemmer() {
    assert_ne!(
        tokenize_lowercase("Rechnungen"),
        stem_text("Rechnungen", "de")
    );
}

#[test]
fn folding_strips_accents_after_stemming() {
    assert_eq!(fold_diacritics("Straße"), "Straße");
    assert_eq!(fold_diacritics("über café"), "uber cafe");
    let stem_then_fold = |s: &str| fold_diacritics(&stem_text(s, "de"));
    assert_eq!(stem_then_fold("Grüßen"), stem_then_fold("Grüße"));
}

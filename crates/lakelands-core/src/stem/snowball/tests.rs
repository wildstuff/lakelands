use super::*;
use crate::stem::tests::stem_text;

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
    assert_ne!(stem_text("Rechnungen", ""), stem_text("Rechnungen", "de"));
}

#[test]
fn folding_strips_accents_after_stemming() {
    assert_eq!(fold_diacritics("Straße"), "Straße");
    assert_eq!(fold_diacritics("über café"), "uber cafe");
    let stem_then_fold = |s: &str| fold_diacritics(&stem_text(s, "de"));
    assert_eq!(stem_then_fold("Grüßen"), stem_then_fold("Grüße"));
}

/// The pipeline as it stood before the walker: split, lower-cased,
/// stemmed and joined, then the whole folded.
fn before_the_walker(text: &str, tag: &str) -> String {
    let stemmer = algorithm(tag).map(Stemmer::create);
    let tokens = text.split(|c: char| !c.is_alphanumeric());
    let terms: Vec<String> = (tokens.filter(|t| !t.is_empty()))
        .map(|t| t.to_lowercase())
        .map(|t| {
            stemmer
                .as_ref()
                .map_or(t.clone(), |s| s.stem(&t).into_owned())
        })
        .collect();
    fold_diacritics(&terms.join(" "))
}

#[test]
fn the_pipeline_over_the_walker_answers_what_it_answered_before() {
    let fixtures = [
        "Café",
        "cafe",
        "Grüße",
        "Grüßen",
        "Rechnungen",
        "Rechnung!",
        "Die offenen Rechnungen!",
        "Kunden Kunde",
        "  EK-2026-0102  ",
        "über café Straße",
        "running runs ran",
        "İstanbul ΣΟΦΊΑ",
        "cafe\u{301} naïve",
        "?!",
        "",
    ];
    for tag in ["de", "de-CH", "en", "fr", "zh", ""] {
        for text in fixtures {
            assert_eq!(
                stem_text(text, tag),
                before_the_walker(text, tag),
                "{text:?} {tag}"
            );
        }
    }
}

#[test]
fn each_token_answers_its_span_and_the_term_the_pipeline_makes() {
    let tokens: Vec<_> = stem_tokens("Running cafés, run!", "en").collect();
    let spans: Vec<_> = tokens.iter().map(|(s, e, _)| (*s, *e)).collect();
    assert_eq!(spans, [(0, 7), (8, 13), (15, 18)]);
    assert_eq!(tokens[0].2, tokens[2].2, "running stems as run");
    assert_eq!(
        tokens[1].2,
        stem_text("cafe", "en"),
        "the fold is the term's"
    );
}

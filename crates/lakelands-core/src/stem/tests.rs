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
        stem_text("Die offenen Rechnungen!", ""),
        "die offenen rechnungen"
    );
    assert_eq!(stem_text("  EK-2026-0102  ", ""), "ek 2026 0102");
    assert_eq!(stem_text("?!", ""), "");
}

/// The walker's tokens of `text`, unstemmed: each one's char span and
/// its term.
fn walked(text: &str) -> Vec<(usize, usize, String)> {
    walk(text, str::to_owned).collect()
}

#[test]
fn the_walker_keeps_each_tokens_chars_on_every_split() {
    let spans = |text: &str| -> Vec<(usize, usize)> {
        walked(text).into_iter().map(|(s, e, _)| (s, e)).collect()
    };
    // Spaces, punctuation and a run of both, at the ends too.
    assert_eq!(spans("  EK-2026-0102  "), [(2, 4), (5, 9), (10, 14)]);
    assert_eq!(spans("a,b;; c"), [(0, 1), (2, 3), (6, 7)]);
    // A token that ends the text, and one char long.
    assert_eq!(spans("x"), [(0, 1)]);
    assert_eq!(spans("?!"), []);
    assert_eq!(spans(""), []);
    // Letters of several bytes count as one char each.
    assert_eq!(spans("Grüße café"), [(0, 5), (6, 10)]);
    // A separator of several bytes, and letters with no case.
    assert_eq!(spans("Rechnung→請求書 😀ok"), [(0, 8), (9, 12), (14, 16)]);
    // A combining mark is no letter: it splits, as the pipeline always did.
    assert_eq!(spans("cafe\u{301}s"), [(0, 4), (5, 6)]);
}

#[test]
fn the_walker_lower_cases_the_token_and_stems_it_whole() {
    assert_eq!(
        walked("Die OFFENEN"),
        [(0, 3, "die".into()), (4, 11, "offenen".into())]
    );
    // Lower-casing may lengthen a token; its span is the original's.
    let dotted: Vec<_> = walk("İx", |t| t.chars().count().to_string()).collect();
    assert_eq!(dotted, [(0, 2, "3".into())]);
}

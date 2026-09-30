use super::*;

fn whole(pattern: &str, text: &str) -> bool {
    regex::Regex::new(&translate(pattern).unwrap())
        .unwrap()
        .is_match(text)
}

#[test]
fn i_regexp_matches_the_whole_text() {
    assert!(whole("DE[0-9]{20}", "DE89370400440532013000"));
    assert!(!whole("DE[0-9]{2}", "DE89370400440532013000"));
    assert!(whole("a|b", "b"));
    assert!(whole("(ab)+", "abab"));
    assert!(whole("\\p{Lu}\\p{Ll}*", "Zürich"));
    assert!(whole("[^a-c]", "d"));
    assert!(whole("[a-]", "-"));
    assert!(whole("\\.\\*", ".*"));
}

#[test]
fn a_pattern_cannot_splice_out_of_its_anchors() {
    assert!(translate("a)|(b").is_err());
    assert!(translate("(a").is_err());
    assert!(translate("a]").is_err());
}

#[test]
fn syntax_beyond_i_regexp_is_refused() {
    for bad in [
        "\\d+",
        "\\w",
        "\\b",
        "(?i)a",
        "a*?",
        "a{2,1}",
        "a**",
        "\\p{Greek}",
        "[z-a]",
        "a{x}",
        "\\1",
    ] {
        assert!(translate(bad).is_err(), "{bad} was accepted");
    }
}

#[test]
fn what_other_dialects_read_as_syntax_is_literal_here() {
    assert!(
        whole("^a$", "^a$"),
        "`^` and `$` are ordinary characters in I-Regexp"
    );
    assert!(!whole("^a$", "a"));
    assert!(whole("a&&b", "a&&b"));
    assert!(whole("[&~]", "~"));
}

#[test]
fn a_dot_is_any_character_but_a_line_break() {
    assert!(whole(".", "é"));
    assert!(!whole(".", "\n"));
    assert!(!whole(".", "\r"));
}

use super::*;

#[test]
fn identifiers_are_ascii_letters_digits_and_underscores() {
    for ok in ["a", "_x", "amount_eur", "A1", "iban"] {
        assert!(is_identifier(ok), "{ok}");
    }
    for bad in ["", "1a", "a-b", "a.b", "a b", "grüße", "a\0b", "\u{1f600}"] {
        assert!(!is_identifier(bad), "{bad:?}");
    }
}

#[test]
fn lake_ids_are_short_lower_case_and_file_safe() {
    for ok in [
        "finance",
        "hr-2026",
        "a",
        "0lake",
        "ops_eu",
        &"a".repeat(63),
    ] {
        assert!(is_lake_id(ok), "{ok}");
    }
    let long = "a".repeat(64);
    for bad in [
        "",
        "Finance",
        "-lake",
        "_lake",
        "a.b",
        "a/b",
        "a b",
        "grüße",
        long.as_str(),
    ] {
        assert!(!is_lake_id(bad), "{bad:?}");
    }
}

#[test]
fn type_names_may_be_dotted() {
    assert!(is_type_name("sys.setting"));
    assert!(is_type_name("invoice"));
    for bad in ["", ".a", "a.", "a..b", "Invoice-Line"] {
        assert!(!is_type_name(bad), "{bad}");
    }
    assert!(fault("type", "sys.source").is_none());
    assert!(fault("field", "sys.source").is_some());
}

#[test]
fn on_ascii_names_code_point_and_utf16_order_agree() {
    let mut names = vec!["zeta", "Alpha", "_x", "a1", "a_b", "B"];
    let mut by_utf16 = names.clone();
    names.sort();
    by_utf16.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    assert_eq!(names, by_utf16);
}

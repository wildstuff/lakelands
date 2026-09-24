//! What a declared name may be.
//!
//! Every name a lake declares — a field, a relation, a verb, a value type,
//! a function, a trigger, an object set, a measure — is an ASCII
//! identifier: a letter or `_`, then letters, digits or `_`. A type name
//! is one or more such identifiers joined by `.` (`invoice`,
//! `sys.setting`). The bundle refuses anything else, and so does `declare`.
//!
//! The rule is narrow on purpose. Every name can be written in an
//! expression without quoting; it never holds the NUL byte that separates
//! names in the log's hash preimages (`docs/log-format.md` §3); and because
//! every name is ASCII, sorting by Unicode code point and sorting by UTF-16
//! code unit — the two orders the log format uses — agree on every name,
//! so a canonical form can never depend on which one an implementation
//! picked.

/// Whether a name can name a lake: 1 to 63 characters of lower-case
/// ASCII letters, digits, `-` and `_`, starting with a letter or a
/// digit. A lake id names files and directories, so it is narrower than
/// an identifier in case and wider in `-`.
pub fn is_lake_id(id: &str) -> bool {
    (1..=63).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Whether a name is an ASCII identifier.
pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether a name is a type name: identifiers joined by `.`.
pub fn is_type_name(name: &str) -> bool {
    !name.is_empty() && name.split('.').all(is_identifier)
}

/// Why a name of this kind cannot be declared, if it cannot. `what` is
/// the kind of thing named (`type`, `field`, …), for the message.
pub fn fault(what: &str, name: &str) -> Option<String> {
    let ok = if what == "type" {
        is_type_name(name)
    } else {
        is_identifier(name)
    };
    if ok {
        return None;
    }
    let rule = if what == "type" {
        "ASCII identifiers joined by `.`"
    } else {
        "an ASCII identifier: a letter or `_`, then letters, digits or `_`"
    };
    Some(if name.is_empty() {
        format!("a {what} name must not be empty")
    } else {
        format!("{what} name `{}` is not {rule}", name.escape_debug())
    })
}

#[cfg(test)]
mod tests {
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
}

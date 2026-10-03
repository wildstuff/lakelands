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
    let (ok, rule) = if what == "type" {
        (is_type_name(name), "ASCII identifiers joined by `.`")
    } else {
        let rule = "an ASCII identifier: a letter or `_`, then letters, digits or `_`";
        (is_identifier(name), rule)
    };
    if ok {
        return None;
    }
    Some(if name.is_empty() {
        format!("a {what} name must not be empty")
    } else {
        format!("{what} name `{}` is not {rule}", name.escape_debug())
    })
}

#[cfg(test)]
mod tests;

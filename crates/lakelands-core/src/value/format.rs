//! The known value formats: is this text well-formed?
//!
//! A value type may name a `format`, which must be one of the formats
//! here; every value of that type must pass its predicate. A name that is
//! none of them is refused when the value type is declared, and a value
//! checked against it is refused too, so a misspelt format never checks
//! nothing in silence.
//!
//! Every predicate is pure, total and structural. It answers "is this
//! well-formed", never "does this exist": whether a domain resolves or an
//! account behind an IBAN is open is a fact about the world, and a check
//! that reached for it would stop being deterministic (and could no
//! longer run wherever an input form wants to say "that is not an e-mail
//! address" before the write is sent).
//!
//! A predicate guards stored values, so tightening one later would start
//! refusing values that were legal when they were written. Changing a
//! predicate is therefore a value migration, not a tidy-up.

use serde::{Deserialize, Serialize};

/// A named built-in check on the text form of a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueFormat {
    /// An e-mail address: exactly one `@`, a non-empty local part, and a
    /// dotted domain with no leading, trailing or doubled dot.
    Email,
    /// An IBAN: 15 to 34 ASCII alphanumerics (whitespace ignored) whose
    /// ISO 7064 mod-97 checksum is 1.
    Iban,
    /// A DNS hostname: two or more dot-separated labels, each 1 to 63
    /// characters of `[A-Za-z0-9-]` not starting or ending in `-`, at most
    /// 253 characters in all, and an alphabetic final label of two or more
    /// characters.
    Domain,
    /// An absolute `http` or `https` URL: the scheme, `://`, a host that
    /// is a `domain` or `localhost`, an optional numeric port, and no
    /// whitespace anywhere.
    Url,
}

impl ValueFormat {
    /// Every known format, in declaration order.
    pub const ALL: [ValueFormat; 4] = [
        ValueFormat::Email,
        ValueFormat::Iban,
        ValueFormat::Domain,
        ValueFormat::Url,
    ];

    /// The name a value type's `format` uses for this format.
    pub fn name(self) -> &'static str {
        match self {
            ValueFormat::Email => "email",
            ValueFormat::Iban => "iban",
            ValueFormat::Domain => "domain",
            ValueFormat::Url => "url",
        }
    }

    /// The known format of that name, if any.
    pub fn parse(name: &str) -> Option<ValueFormat> {
        ValueFormat::ALL.into_iter().find(|f| f.name() == name)
    }

    /// Whether `s` is well-formed for this format.
    pub fn matches(self, s: &str) -> bool {
        match self {
            ValueFormat::Email => is_email(s),
            ValueFormat::Iban => is_iban(s),
            ValueFormat::Domain => is_domain(s),
            ValueFormat::Url => is_url(s),
        }
    }
}

/// The e-mail shape. Its domain part is deliberately looser than
/// [`is_domain`] (`a@under_score.com` passes): mail hosts accept names a
/// DNS hostname check refuses, and the two only align through a migration.
fn is_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
}

/// ISO 7064 mod-97: move the first four characters to the end, read
/// letters as `10..=35`, and require the remainder 1.
fn is_iban(s: &str) -> bool {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !(15..=34).contains(&t.len()) || !t.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let t = t.to_ascii_uppercase();
    let (head, tail) = t.split_at(4);
    let mut rem: u32 = 0;
    for c in tail.chars().chain(head.chars()) {
        let v = c.to_digit(36).expect("checked alphanumeric");
        rem = if v >= 10 {
            (rem * 100 + v) % 97
        } else {
            (rem * 10 + v) % 97
        };
    }
    rem == 1
}

/// The DNS hostname shape. The trailing-dot spelling (`example.com.`) is
/// refused: correct in a zone file, a typo everywhere a person types a
/// domain. A numeric final label is refused too, which keeps a dotted
/// IPv4 address out; an address would be its own format.
fn is_domain(s: &str) -> bool {
    if s.is_empty() || s.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = s.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let label_ok = |l: &&str| {
        !l.is_empty()
            && l.len() <= 63
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    if !labels.iter().all(label_ok) {
        return false;
    }
    let tld = labels[labels.len() - 1];
    tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
}

/// The web-address shape. Narrow on purpose: `mailto:` or `ftp:` values
/// are other concepts and would get formats of their own rather than
/// widen this one. The scheme and host are case-insensitive; the path is
/// not judged. Credentials before the host (`user:pw@`) are allowed.
fn is_url(s: &str) -> bool {
    if s.chars().any(char::is_whitespace) {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return false;
    }
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (host_port, None),
    };
    if port.is_some_and(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        return false;
    }
    host == "localhost" || is_domain(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_iban_passes_its_checksum_and_a_corrupt_one_does_not() {
        assert!(is_iban("DE89 3704 0044 0532 0130 00"));
        assert!(is_iban("DE89370400440532013000"));
        assert!(is_iban("GB82WEST12345698765432"));
        assert!(is_iban("gb82west12345698765432"));
        assert!(!is_iban("DE89370400440532013001"));
        assert!(!is_iban("not-an-iban"));
        assert!(!is_iban("DE"));
    }

    #[test]
    fn an_email_needs_one_at_a_local_part_and_a_dotted_domain() {
        assert!(is_email("a@b.co"));
        assert!(is_email("first.last@sub.example.com"));
        assert!(is_email("a@under_score.com"));
        assert!(!is_email("no-at-sign"));
        assert!(!is_email("two@@at.com"));
        assert!(!is_email("a@b@c.com"));
        assert!(!is_email("@no-local.com"));
        assert!(!is_email("no-dot@domain"));
        assert!(!is_email("trailing@dot."));
        assert!(!is_email("lead@.dot.com"));
        assert!(!is_email("double@dot..com"));
    }

    #[test]
    fn a_domain_is_a_hostname_and_not_its_lookalikes() {
        assert!(is_domain("example.com"));
        assert!(is_domain("sub.domain.example.co.uk"));
        assert!(is_domain("xn--bcher-kva.de"));
        assert!(is_domain("a-b.de"));
        assert!(!is_domain("localhost"));
        assert!(!is_domain("example.com."));
        assert!(!is_domain(".example.com"));
        assert!(!is_domain("double..dot.com"));
        assert!(!is_domain("-lead.example.com"));
        assert!(!is_domain("trail-.example.com"));
        assert!(!is_domain("under_score.com"));
        assert!(!is_domain("space in.com"));
        assert!(!is_domain("192.168.0.1"));
        assert!(!is_domain("example.c"));
        assert!(!is_domain(&format!("{}.com", "a".repeat(64))));
        assert!(is_domain(&format!("{}.com", "a".repeat(63))));
    }

    #[test]
    fn a_url_is_an_absolute_web_address_and_nothing_else() {
        assert!(is_url("https://example.com"));
        assert!(is_url("http://example.com/path?q=1#frag"));
        assert!(is_url("HTTPS://Example.COM"));
        assert!(is_url("https://localhost:8080/admin"));
        assert!(is_url("https://user:pw@example.com/x"));
        assert!(!is_url("example.com"));
        assert!(!is_url("www.example.com"));
        assert!(!is_url("ftp://example.com"));
        assert!(!is_url("mailto:a@b.com"));
        assert!(!is_url("https://"));
        assert!(!is_url("https://under_score.com"));
        assert!(!is_url("https://example.com:port"));
        assert!(!is_url("https://example.com:"));
        assert!(!is_url("https://exa mple.com"));
    }

    #[test]
    fn every_format_round_trips_through_its_name() {
        for f in ValueFormat::ALL {
            assert_eq!(ValueFormat::parse(f.name()), Some(f));
            let json = serde_json::to_string(&f).unwrap();
            assert_eq!(json, format!("\"{}\"", f.name()));
            assert_eq!(serde_json::from_str::<ValueFormat>(&json).unwrap(), f);
        }
        assert_eq!(ValueFormat::parse("0.00"), None);
    }
}

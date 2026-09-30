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

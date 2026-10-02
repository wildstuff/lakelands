use lakelands_api::Error;
use lakelands_api::location::{PgTls, PgTlsMode};

use super::tls_config_with;

/// `require` with no `ca_file` on a system with no bundle at any
/// well-known path refuses to connect as unavailable, and says what to
/// name: the location's `tls.ca_file`.
#[test]
fn require_without_a_ca_file_or_a_system_bundle_is_unavailable_naming_ca_file() {
    let tls = PgTls {
        mode: PgTlsMode::Require,
        ..PgTls::default()
    };
    match tls_config_with(&tls, &[]) {
        Err(Error::Unavailable { message }) => assert!(
            message.contains("tls.ca_file"),
            "the refusal names what to set: {message}"
        ),
        Err(other) => panic!("refused as {other:?}, not as unavailable"),
        Ok(_) => panic!("TLS was set up with no certificate authority at all"),
    }
}

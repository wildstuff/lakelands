use lakelands_api::error::{Situation, Tls};
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
        Err(e) if matches!(e.situation(), Some(Situation::Tls { .. })) => {
            let Some(Situation::Tls { problem, .. }) = e.situation() else {
                unreachable!()
            };
            assert_eq!(*problem, Tls::NoBundle, "the refusal says what to set")
        }
        Err(other) => panic!("refused as {other:?}, not as unavailable"),
        Ok(_) => panic!("TLS was set up with no certificate authority at all"),
    }
}

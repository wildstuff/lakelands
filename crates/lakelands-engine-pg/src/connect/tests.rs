use lakelands_api::error::{Situation, Tls};
use lakelands_api::location::{PgTls, PgTlsMode, PgTuning};

use super::{PREPARED_MOST, Target, reader, tls_config_with};

/// A reader that ran more statement texts than [`PREPARED_MOST`] keeps
/// that many prepared on the server, not one for each and not fewer, and
/// a statement it let go answers again when run. Against the server
/// `LAKELANDS_TEST_PG_URL` names, read here, in test code; without it the
/// test stands down.
#[test]
fn a_reader_keeps_at_most_its_most_statements_and_each_answers() {
    let Ok(url) = std::env::var("LAKELANDS_TEST_PG_URL") else {
        return;
    };
    let tls = PgTls {
        mode: PgTlsMode::Disable,
        ..PgTls::default()
    };
    let (schema, tuning) = (String::from("public"), PgTuning::default());
    let mut db = reader(&Target {
        url,
        schema,
        tls,
        tuning,
        tablespaces: Default::default(),
    })
    .unwrap();
    for _ in 0..2 {
        for i in 0..PREPARED_MOST as i32 + 100 {
            let row = db.query_one(&format!("SELECT $1::int + {i}"), &[&1]);
            assert_eq!(row.unwrap().get::<_, i32>(0), 1 + i, "statement {i}");
        }
    }
    let kept = "SELECT count(*) FROM pg_catalog.pg_prepared_statements";
    let kept: i64 = db.query_one(kept, &[]).unwrap().get(0);
    let most = PREPARED_MOST as i64;
    assert!(
        (most - 1..=most + 1).contains(&kept),
        "{kept} kept prepared"
    );
}

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

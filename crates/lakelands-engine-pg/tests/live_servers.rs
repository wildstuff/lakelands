//! The engine's refusals at connect, run against what they guard against:
//! a transaction-mode pooler, a session-mode pooler, and a server that
//! demands a client certificate (`docs/decisions/038-a-lake-in-postgresql.md`).
//!
//! `scripts/pg-live/up.sh` starts them and prints the variables below; CI's
//! `pg-live` job runs this file with `LAKELANDS_TEST_PG_LIVE=1`, which makes
//! a missing variable a failure. Without it the tests stand down, as the
//! other PostgreSQL tests do, so the conformance run, which runs every test
//! file of the crate, passes on none of them.

mod common;

use std::path::PathBuf;

use lakelands_api::Error;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::error::{Case, Situation};
use lakelands_api::location::{EngineConfig, Location, PgTls, PgTlsMode};
use lakelands_engine_pg::PgOpener;

/// A variable of the live servers, or a line saying why the test stands down.
fn live(test: &str, name: &str) -> Option<String> {
    let set = std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    if set.is_none() {
        assert!(
            std::env::var_os("LAKELANDS_TEST_PG_LIVE").is_none(),
            "{test}: LAKELANDS_TEST_PG_LIVE is set but {name} is not"
        );
        eprintln!("{test}: {name} is not set; no live server to run against");
    }
    set
}

fn at(url: &str, lake: &str, schema: &str, tls: PgTls) -> Location {
    let mut location = common::location(url, lake, schema);
    if let EngineConfig::Postgres { tls: t, .. } = &mut location.engine {
        *t = tls;
    }
    location
}

/// A hash of every row of every table of `schema`: the lake as it stands.
fn content(url: &str, schema: &str) -> String {
    let mut db = postgres::Client::connect(url, postgres::NoTls).unwrap();
    let tables = db
        .query(
            "SELECT tablename FROM pg_tables WHERE schemaname = $1 ORDER BY tablename",
            &[&schema],
        )
        .unwrap();
    let mut seen = String::new();
    for table in tables {
        let name: String = table.get(0);
        let row = db
            .query_one(
                &format!(
                    "SELECT coalesce(md5(string_agg(t::text, '|' ORDER BY t::text)), '') \
                     FROM \"{schema}\".\"{name}\" t"
                ),
                &[],
            )
            .unwrap();
        seen += &format!("{name}={};", row.get::<_, String>(0));
    }
    seen
}

fn situation(result: Result<Box<dyn lakelands_api::engine::LakeEngine>, Error>) -> Situation {
    match result {
        Err(e) => e.situation().cloned().unwrap_or_else(|| panic!("{e:?}")),
        Ok(_) => panic!("the open was served"),
    }
}

#[test]
fn a_transaction_mode_pooler_is_refused_and_the_lake_is_unchanged() {
    let (Some(direct), Some(pooled)) = (
        live("the pooler test", "LAKELANDS_TEST_PG_URL"),
        live("the pooler test", "LAKELANDS_TEST_PG_POOLER_TX_URL"),
    ) else {
        return;
    };
    let schema = common::fresh_schema();
    // The lake exists, made on the direct connection.
    drop(
        PgOpener
            .open(
                &common::location(&direct, "pooled", &schema),
                OpenMode::Create,
            )
            .unwrap(),
    );
    let before = content(&direct, &schema);
    for mode in [OpenMode::Write, OpenMode::Create] {
        let opened = PgOpener.open(&common::location(&pooled, "pooled", &schema), mode);
        assert_eq!(situation(opened), Situation::Pooler, "{mode:?}");
    }
    assert_eq!(content(&direct, &schema), before, "the lake is unchanged");
    // A lake that does not exist is not made through it either.
    let none = common::fresh_schema();
    let opened = PgOpener.open(&common::location(&pooled, "none", &none), OpenMode::Create);
    assert_eq!(situation(opened), Situation::Pooler);
    assert_eq!(content(&direct, &none), "", "no lake was made");
    common::drop_schema(&direct, &schema);
    common::drop_schema(&direct, &none);
}

#[test]
fn a_session_mode_pooler_serves_the_lake() {
    let Some(pooled) = live("the pooler test", "LAKELANDS_TEST_PG_POOLER_SESSION_URL") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(
            &common::location(&pooled, "pooled", &schema),
            OpenMode::Create,
        )
        .expect("a session-mode pooler serves the lake");
    let lease = engine.take_lease("live").expect("the lease is kept");
    engine.release_lease(&lease).unwrap();
    drop(engine);
    common::drop_schema(&pooled, &schema);
}

/// The TLS of the server that demands a certificate: its authority and,
/// when `client`, the certificate and key the test made for it.
fn mutual(client: bool) -> Option<(String, PgTls)> {
    let url = live("the mutual-TLS test", "LAKELANDS_TEST_PG_MTLS_URL")?;
    let dir = PathBuf::from(live("the mutual-TLS test", "LAKELANDS_TEST_PG_MTLS_DIR")?);
    let tls = PgTls {
        mode: PgTlsMode::Require,
        ca_file: Some(dir.join("ca.pem")),
        client_cert: client.then(|| dir.join("client.pem")),
        client_key: client.then(|| dir.join("client.key")),
    };
    Some((url, tls))
}

#[test]
fn a_server_that_demands_a_certificate_serves_the_one_who_has_it() {
    let Some((url, tls)) = mutual(true) else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&at(&url, "mutual", &schema, tls), OpenMode::Create)
        .expect("the certificate is accepted");
    drop(engine);
    common::drop_schema(&url, &schema);
}

#[test]
fn a_server_that_demands_a_certificate_refuses_the_one_without() {
    let (Some((url, tls)), Some((_, with))) = (mutual(false), mutual(true)) else {
        return;
    };
    let schema = common::fresh_schema();
    // Not the server's certificate, nor TLS, nor the user: only the missing
    // certificate tells the two locations apart.
    let opened = PgOpener.open(&at(&url, "mutual", &schema, tls), OpenMode::Create);
    assert!(
        matches!(situation(opened), Situation::Storage { .. }),
        "the server refuses the connection"
    );
    let served = PgOpener.open(&at(&url, "mutual", &schema, with), OpenMode::Create);
    assert!(
        served.is_ok(),
        "the same location with a certificate is served"
    );
    drop(served);
    common::drop_schema(&url, &schema);
}

#[test]
fn half_a_certificate_is_refused_before_any_connection() {
    let Some((_, tls)) = mutual(true) else {
        return;
    };
    // Nothing listens at the URL: a connection tried would be refused as
    // unavailable, not as a half certificate.
    let nowhere = "postgres://lakelands:lakelands@127.0.0.1:1/lakelands";
    for half in [
        PgTls {
            client_key: None,
            ..tls.clone()
        },
        PgTls {
            client_cert: None,
            ..tls.clone()
        },
    ] {
        let schema = common::fresh_schema();
        let opened = PgOpener.open(&at(nowhere, "half", &schema, half), OpenMode::Create);
        let Err(e @ Error::Refused { .. }) = opened else {
            panic!("half a certificate was not refused as invalid")
        };
        assert_eq!(e.case().map(|s| s.case.clone()), Some(Case::ClientCertHalf));
    }
}

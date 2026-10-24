//! The PostgreSQL the engine's tests run against.
//!
//! `LAKELANDS_TEST_PG_URL` names it. It is read here, in test code, and
//! nowhere in the engine: the engine takes its URL from the location. When
//! the variable is unset or empty the tests say so and pass, so a workspace
//! run on a machine without PostgreSQL stays green; the conformance job for
//! `pg` refuses to run without it (`scripts/conformance.sh`), so the suite
//! is never skipped there.
//!
//! Every test lake gets a schema of its own, `lakelands_test_<pid>_<n>`,
//! and the schemas a test binary made are dropped when it ends.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lakelands_api::location::{EngineConfig, Location, PgTls, PgTlsMode, PgTuning};

/// The test server's URL, if one is configured.
pub fn url() -> Option<String> {
    std::env::var("LAKELANDS_TEST_PG_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
}

/// The URL, or a line saying why the test stands down.
pub fn url_or_skip(test: &str) -> Option<String> {
    let url = url();
    if url.is_none() {
        eprintln!("{test}: LAKELANDS_TEST_PG_URL is not set; no PostgreSQL to run against");
    }
    url
}

/// The prefix of this process's schemas.
pub fn prefix() -> String {
    format!("lakelands_test_{}_", std::process::id())
}

/// A fresh schema name for this process.
pub fn fresh_schema() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("{}{}", prefix(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// The schema a rebuild of the lake in `schema` builds its scratch in, as
/// the engine names it.
pub fn scratch_of(schema: &str) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(schema.as_bytes());
    format!(
        "lakelands_rebuild_{}",
        lakelands_core::canonical::hex(&digest[..8])
    )
}

/// A lake on the test server, in `schema`, over plaintext (the test
/// server serves no TLS).
pub fn location(url: &str, lake: &str, schema: &str) -> Location {
    Location {
        engine: EngineConfig::Postgres {
            url: url.to_owned(),
            schema: Some(schema.to_owned()),
            tls: PgTls {
                mode: PgTlsMode::Disable,
                ..PgTls::default()
            },
            tuning: PgTuning::default(),
            tablespaces: Default::default(),
        },
        ..Location::memory(lake)
    }
}

/// Drop every schema this process made, when its one test is done. On a thread of its own: the
/// synchronous client blocks on a runtime of its own, which an async test
/// body cannot.
pub fn drop_schemas(url: &str) {
    let url = url.to_owned();
    let _ = std::thread::spawn(move || {
        let Ok(mut client) = postgres::Client::connect(&url, postgres::NoTls) else {
            return;
        };
        let Ok(rows) = client.query(
            "SELECT nspname FROM pg_catalog.pg_namespace WHERE starts_with(nspname, $1)",
            &[&prefix()],
        ) else {
            return;
        };
        for row in rows {
            let name: String = row.get(0);
            let scratch = scratch_of(&name);
            let _ = client.batch_execute(&format!(
                "DROP SCHEMA \"{name}\" CASCADE; DROP SCHEMA IF EXISTS \"{scratch}\" CASCADE"
            ));
        }
    })
    .join();
}

/// Drop one schema, when a test in a binary of several is done with it.
pub fn drop_schema(url: &str, schema: &str) {
    let (url, schema) = (url.to_owned(), schema.to_owned());
    let _ = std::thread::spawn(move || {
        if let Ok(mut client) = postgres::Client::connect(&url, postgres::NoTls) {
            let _ = client.batch_execute(&format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE"));
        }
    })
    .join();
}

/// A count the server keeps of the lake in `schema`, which `query`
/// selects with the schema as `$1`, once it stands still: a backend that
/// ended reports what it read at its end.
pub fn settled(url: &str, schema: &str, query: &str) -> i64 {
    let (url, schema, query) = (url.to_owned(), schema.to_owned(), query.to_owned());
    let read = move || {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        let row = client.query_opt(&query, &[&schema]).unwrap();
        row.map_or(0, |row| row.get::<_, i64>(0))
    };
    std::thread::spawn(move || {
        let (asked, mut last) = (Instant::now(), -1);
        loop {
            std::thread::sleep(Duration::from_millis(700));
            let now = read();
            if now == last || asked.elapsed() > Duration::from_secs(20) {
                return now;
            }
            last = now;
        }
    })
    .join()
    .unwrap()
}

/// The tuples of one of the lake's tables the server has fetched, by an
/// index or in sequence, once the count stands still.
pub fn fetched(url: &str, schema: &str, table: &str) -> i64 {
    let tuples = format!(
        "SELECT COALESCE(idx_tup_fetch, 0) + seq_tup_read FROM pg_stat_user_tables \
         WHERE schemaname = $1 AND relname = '{table}'"
    );
    settled(url, schema, &tuples)
}

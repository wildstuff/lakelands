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
//! and the schemas a test binary made are dropped when it ends. A drop that
//! fails says what the server said, where the end-of-run check reads it
//! (`leftovers.rs`): the test has ended by then, and nextest keeps no
//! passing test's output, so a schema left behind would name no reason.

#![allow(dead_code)]

use std::io::Write as _;
use std::path::PathBuf;
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

/// Where a drop that failed is written for the end-of-run check to show.
/// The tests share this directory with the check (they are the same
/// crate's integration tests), and it belongs to the worktree's target, so
/// a run reads its own records and not another's.
fn failed_drops_path() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pg-failed-drops.log")
}

/// Say a drop failed, with the schema's name and the server's message, in
/// the line the end-of-run check prints when a schema is left behind.
fn note_failed_drop(schema: &str, e: &postgres::Error) {
    let line = format!("{schema}: {}\n", said(e));
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(failed_drops_path())
        .and_then(|mut log| log.write_all(line.as_bytes()));
}

/// What the server said of a failed statement: its severity, SQLSTATE and
/// primary message. `DETAIL` and `HINT` stay out, as they do of the
/// engine's answers (024): they can quote a record's values.
fn said(e: &postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => format!("{} {}: {}", db.severity(), db.code().code(), db.message()),
        None => e.to_string(),
    }
}

/// Whether a drop failed for a reason a holder ending may clear: the
/// schema was locked, or the statement ran out of time.
fn held_or_timed_out(e: &postgres::Error) -> bool {
    use postgres::error::SqlState;
    let code = e.code();
    code == Some(&SqlState::LOCK_NOT_AVAILABLE)
        || code == Some(&SqlState::T_R_DEADLOCK_DETECTED)
        || code == Some(&SqlState::QUERY_CANCELED)
}

/// Drop a schema, trying once more if a lock or a timeout refused it, and
/// saying so where the end-of-run check reads it if it still refuses.
fn drop_and_note(client: &mut postgres::Client, schema: &str) {
    let statement = format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE");
    let mut outcome = client.batch_execute(&statement);
    if matches!(&outcome, Err(e) if held_or_timed_out(e)) {
        std::thread::sleep(Duration::from_millis(250));
        outcome = client.batch_execute(&statement);
    }
    if let Err(e) = outcome {
        note_failed_drop(schema, &e);
    }
}

/// The drops that failed, as the tests recorded them, taken away as they
/// are read so a run speaks of its own. Empty when every drop succeeded.
pub fn failed_drops() -> String {
    let Ok(record) = std::fs::read_to_string(failed_drops_path()) else {
        return String::new();
    };
    let _ = std::fs::remove_file(failed_drops_path());
    let record = record.trim_end();
    if record.is_empty() {
        String::new()
    } else {
        format!(
            "; and {} drop(s) failed first: {record}",
            record.lines().count()
        )
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
            drop_and_note(&mut client, &name);
            drop_and_note(&mut client, &scratch_of(&name));
        }
    })
    .join();
}

/// Drop one schema, when a test in a binary of several is done with it.
pub fn drop_schema(url: &str, schema: &str) {
    let (url, schema) = (url.to_owned(), schema.to_owned());
    let _ = std::thread::spawn(move || {
        if let Ok(mut client) = postgres::Client::connect(&url, postgres::NoTls) {
            drop_and_note(&mut client, &schema);
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

//! The teardown of the schemas a PostgreSQL test makes.
//!
//! A test's schema is dropped when the test ends, and the end-of-run check
//! (`leftovers.rs` in the engine's tests) refuses a run that left one
//! behind. The drop runs beside whatever the test's own runtime is still
//! ending: a reader of the schema can hold a relation while the drop waits
//! for another one, and the server then resolves the deadlock by aborting
//! one of the two. A drop that is the victim must be tried again, not
//! ignored, and a drop must not wait forever for a reader that has ended
//! at the server but not yet at the client (lakelands#1816).
//!
//! So a drop is one short attempt: `lock_timeout` is set below the
//! server's deadlock check, so the drop is refused rather than chosen as
//! the victim, and is reset when the attempt succeeds. An attempt refused
//! because a holder still locks the schema is repeated, backing off, until
//! [`WITHIN`] has passed; a drop that is still refused then is a failure
//! the caller shows (`leftovers.rs` reads it) or fails its test with.
//!
//! The synchronous client drives a runtime of its own, which a test's
//! runtime thread cannot start: [`drop_all_off_thread`] runs the drops on
//! a thread of its own, and the functions that take a client are for a
//! caller that already has one.

use std::time::{Duration, Instant};

/// How long one attempt waits for a lock before the server refuses it.
///
/// Below the server's `deadlock_timeout` (a second by default), so an
/// attempt that would complete a deadlock's cycle is refused first: the
/// drop is never the victim, and the reader it waits beside is never
/// aborted.
const ATTEMPT: Duration = Duration::from_millis(250);

/// How long the drops keep trying before one is said to have failed: long
/// enough for a reader that is ending to close its connection, short
/// enough that a stuck test ends rather than hangs.
pub const WITHIN: Duration = Duration::from_secs(30);

/// What the server said of a failed statement: its severity, SQLSTATE and
/// primary message. `DETAIL` and `HINT` stay out, as they do of the
/// engine's answers: they can quote a record's values.
pub fn said(e: &postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => format!("{} {}: {}", db.severity(), db.code().code(), db.message()),
        None => e.to_string(),
    }
}

/// Drop `schema` and everything in it, retrying while a live holder keeps
/// it locked.
///
/// # Errors
///
/// The last attempt's error, when the schema is still locked after
/// [`WITHIN`], or the connection's error when the server cannot be
/// reached.
pub fn drop_schema(url: &str, schema: &str) -> Result<(), postgres::Error> {
    let mut client = postgres::Client::connect(url, postgres::NoTls)?;
    drop_schema_on(&mut client, schema)
}

/// As [`drop_schema`], on a client already connected.
///
/// # Errors
///
/// As [`drop_schema`].
pub fn drop_schema_on(client: &mut postgres::Client, schema: &str) -> Result<(), postgres::Error> {
    // One implicit transaction: a refused attempt rolls the `SET` back, so
    // the next attempt starts from the session's own timeout; a successful
    // one resets it, so a client reused for other statements is unchanged.
    let statement = format!(
        "SET lock_timeout = '{}ms'; DROP SCHEMA IF EXISTS \"{schema}\" CASCADE; \
         SET lock_timeout = DEFAULT",
        ATTEMPT.as_millis()
    );
    let deadline = Instant::now() + WITHIN;
    let mut pause = Duration::from_millis(25);
    loop {
        match client.batch_execute(&statement) {
            Ok(()) => return Ok(()),
            Err(e) if refused_by_a_holder(&e) && Instant::now() < deadline => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_millis(500));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Drop each `(url, schema)`, on a thread of its own, and fail the test
/// that ended with a schema the server still refused: a panic, so the
/// failing line names the schema and what the server said. A test already
/// unwinding prints the same line instead (a panic there would end the
/// process), and the end-of-run check reports the schema beside it.
pub fn drop_all_off_thread(schemas: Vec<(String, String)>) {
    if schemas.is_empty() {
        return;
    }
    let dropped = std::thread::spawn(move || {
        let mut refused = Vec::new();
        for (url, schema) in schemas {
            if let Err(e) = drop_schema(&url, &schema) {
                refused.push(format!("{schema}: {}", said(&e)));
            }
        }
        refused
    });
    let refused = match dropped.join() {
        Ok(refused) => refused,
        Err(payload) => {
            if !std::thread::panicking() {
                std::panic::resume_unwind(payload);
            }
            eprintln!("a test schema's teardown panicked");
            return;
        }
    };
    if refused.is_empty() {
        return;
    }
    let refused = refused.join("; ");
    if std::thread::panicking() {
        eprintln!("a test schema was left behind: {refused}");
    } else {
        panic!("a test schema was left behind: {refused}");
    }
}

/// Whether a drop was refused for a reason a holder ending may clear: the
/// schema was locked, the server aborted the drop as a deadlock's victim,
/// or the statement ran out of time.
fn refused_by_a_holder(e: &postgres::Error) -> bool {
    use postgres::error::SqlState;
    let code = e.code();
    code == Some(&SqlState::LOCK_NOT_AVAILABLE)
        || code == Some(&SqlState::T_R_DEADLOCK_DETECTED)
        || code == Some(&SqlState::QUERY_CANCELED)
}

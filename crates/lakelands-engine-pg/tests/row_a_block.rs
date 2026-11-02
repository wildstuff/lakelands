//! The log keeps a row a block and an `ids` row an event (lakelands#1673):
//! an append of three events framed as one block is one row of `log`,
//! which keeps nothing beside its bytes, and three of `ids`. The active
//! segment's digest runs on its row of `segments` as the writer appends,
//! so a seal reads no row of the log: with `log` hidden behind a view that
//! shows none, the seal answers the digest of the events appended. By
//! hand, with `LAKELANDS_TEST_PG_URL` set.

mod common;

use lakelands_api::canonical;
use lakelands_api::engine::{CanonicalEvent, EngineOpener, OpenMode};
use lakelands_api::event::{Digest, EventId, LOG_FORMAT_VERSION};
use lakelands_api::id::Position;
use lakelands_core::conformance::engine::{chain, seal};
use lakelands_engine_pg::PgOpener;
use serde_json::json;

/// `n` chained events from `start` after `prev`, under `fence`, as one
/// append of the store's writer stores them: a block.
fn framed(start: u64, n: u64, prev: &Digest, fence: u64) -> Vec<CanonicalEvent> {
    let mut prev = prev.clone();
    let mut stored = Vec::new();
    for p in start..start + n {
        let fence = if p == 0 { 0 } else { fence };
        let mut event = json!({
            "body": {"kind": "note", "n": p},
            "fence": fence,
            "id": format!("{p:032x}"),
            "known_at": "2026-01-01T00:00:00.000000Z",
            "position": p,
            "prev": prev.0,
            "v": LOG_FORMAT_VERSION,
        });
        let hash = Digest(canonical::sha256_hex(&event));
        event["hash"] = json!(hash.0);
        stored.push(CanonicalEvent {
            position: Position(p),
            id: EventId(format!("{p:032x}")),
            prev: prev.clone(),
            hash: hash.clone(),
            fence,
            bytes: canonical::to_vec(&event),
            frame: None,
        });
        prev = hash;
    }
    lakelands_core::log::frame_append(&mut stored);
    stored
}

/// One number a query on the test server answers.
fn one(url: &str, query: &str) -> i64 {
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    client.query_one(query, &[]).unwrap().get(0)
}

/// Statements on the test server, on a connection of their own.
fn admin(url: &str, statements: &str) {
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    client.batch_execute(statements).unwrap();
}

/// The active segment's running state, as the server holds it.
fn running(url: &str, schema: &str) -> Option<Vec<u8>> {
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    client
        .query_one(
            &format!("SELECT running FROM \"{schema}\".segments ORDER BY first DESC LIMIT 1"),
            &[],
        )
        .unwrap()
        .get(0)
}

/// Hand the active segment a running state, as another release would.
fn set_running(url: &str, schema: &str, state: Option<&[u8]>) {
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    client
        .execute(
            &format!("UPDATE \"{schema}\".segments SET running = $1"),
            &[&state],
        )
        .unwrap();
}

/// The schema, dropped when the test ends, also when it fails.
struct Schema<'u>(&'u str, String);

impl Drop for Schema<'_> {
    fn drop(&mut self) {
        common::drop_schema(self.0, &self.1);
    }
}

#[test]
fn a_block_is_one_row_and_a_seal_reads_none() {
    let Some(url) = common::url_or_skip("a row a block") else {
        return;
    };
    let kept = Schema(&url, common::fresh_schema());
    let s = format!("\"{}\"", kept.1);
    let engine = PgOpener
        .open(&common::location(&url, "blocks", &kept.1), OpenMode::Create)
        .unwrap();
    let lease = engine.take_lease("writer").unwrap();
    let block = framed(0, 3, &Digest::zero(), lease.fence);
    assert!(block.iter().all(|e| e.frame.is_some()), "one block");
    engine.append(&lease, &block).unwrap();
    let alone = chain(3, 1, &block[2].hash, &lease);
    engine.append(&lease, &alone).unwrap();
    let rows = format!("SELECT count(*) FROM {s}.log");
    let bare = format!("SELECT count(*) FROM {s}.log WHERE kept IS NULL");
    assert_eq!((one(&url, &rows), one(&url, &bare)), (2, 2));
    assert_eq!(one(&url, &format!("SELECT count(*) FROM {s}.ids")), 4);
    let running = format!("SELECT count(running) FROM {s}.segments");
    assert_eq!(one(&url, &running), 1, "the active segment's digest runs");

    admin(
        &url,
        &format!(
            "ALTER TABLE {s}.log RENAME TO log_hidden; \
             CREATE VIEW {s}.log AS SELECT * FROM {s}.log_hidden WHERE false"
        ),
    );
    let next = chain(4, 1, &alone[0].hash, &lease);
    let sealed = seal(engine.as_ref(), &lease, &next);
    admin(
        &url,
        &format!("DROP VIEW {s}.log; ALTER TABLE {s}.log_hidden RENAME TO log"),
    );
    let all = [block.clone(), alone].concat();
    let digest = lakelands_core::log::digest_of(&all).unwrap();
    let sealed = sealed.unwrap().expect("the first segment sealed");
    assert_eq!(
        (sealed.first, sealed.last, &sealed.digest),
        (Position(0), Position(3), &digest)
    );
    assert_eq!(engine.segments().unwrap(), [sealed]);
    assert_eq!(engine.read(Position(0), 10).unwrap(), [all, next].concat());
    assert_eq!(one(&url, &running), 1, "only the active segment's runs");
}

#[test]
fn a_reopen_mid_segment_seals_to_the_same_digest() {
    let Some(url) = common::url_or_skip("a reopen mid segment") else {
        return;
    };
    let kept = Schema(&url, common::fresh_schema());
    let location = common::location(&url, "blocks", &kept.1);
    let open = |mode| PgOpener.open(&location, mode).unwrap();
    let (mut held, mut at, mut sealed): (Vec<CanonicalEvent>, u64, Vec<_>) =
        (Vec::new(), 0, Vec::new());
    for segment in 0..2 {
        // Half the segment by a writer that goes, half by the one that
        // reopens the lake and takes the lease over: the digest goes on
        // from the state the first left on the segment's row.
        let mode = if segment == 0 {
            OpenMode::Create
        } else {
            OpenMode::Write
        };
        let before = open(mode);
        let lease = before.take_lease("first writer").unwrap();
        let prev = held.last().map_or_else(Digest::zero, |e| e.hash.clone());
        let first = framed(at, 3, &prev, lease.fence);
        before.append(&lease, &first).unwrap();
        drop(before);
        held.extend(first);
        at += 3;
        let engine = open(OpenMode::Write);
        let lease = engine.take_lease("second writer").unwrap();
        let second = framed(at, 3, &held.last().unwrap().hash, lease.fence);
        engine.append(&lease, &second).unwrap();
        held.extend(second);
        at += 3;
        let mut whole = lakelands_core::log::Digesting::default();
        whole.add(&held);
        assert_eq!(
            running(&url, &kept.1),
            whole.state(),
            "segment {segment} keeps its state running"
        );
        // The seal goes on from the state the reopened writer read on the
        // segment's row: with the log hidden behind a view that shows no
        // row, a seal that restored the segment would digest nothing.
        let s = format!("\"{}\"", kept.1);
        admin(
            &url,
            &format!(
                "ALTER TABLE {s}.log RENAME TO log_hidden; \
                 CREATE VIEW {s}.log AS SELECT * FROM {s}.log_hidden WHERE false"
            ),
        );
        let next = chain(at, 1, &held.last().unwrap().hash, &lease);
        let got = seal(engine.as_ref(), &lease, &next);
        admin(
            &url,
            &format!("DROP VIEW {s}.log; ALTER TABLE {s}.log_hidden RENAME TO log"),
        );
        let digest = lakelands_core::log::digest_of(&held).unwrap();
        let got = got.unwrap().expect("the segment sealed");
        assert_eq!(
            (got.first, got.last, &got.digest),
            (held[0].position, held.last().unwrap().position, &digest),
            "segment {segment}"
        );
        sealed.push(got);
        held = next;
        at += 1;
    }
    let reader = open(OpenMode::Write);
    assert_eq!(reader.segments().unwrap(), sealed);
}

#[test]
fn a_state_this_release_does_not_read_is_cleared_and_the_seal_reads_rows() {
    let Some(url) = common::url_or_skip("a state not read") else {
        return;
    };
    let kept = Schema(&url, common::fresh_schema());
    let location = common::location(&url, "blocks", &kept.1);
    let engine = PgOpener.open(&location, OpenMode::Create).unwrap();
    let lease = engine.take_lease("first writer").unwrap();
    let first = framed(0, 3, &Digest::zero(), lease.fence);
    engine.append(&lease, &first).unwrap();
    drop(engine);
    // A state another release's format leaves: a writer that cannot read
    // it must clear it, or a later one would trust it.
    let mut state = running(&url, &kept.1).expect("the append keeps a state");
    assert_eq!(state[0], 1, "the tag this release writes");
    state[0] = 2;
    set_running(&url, &kept.1, Some(&state));
    let engine = PgOpener.open(&location, OpenMode::Write).unwrap();
    let lease = engine.take_lease("second writer").unwrap();
    let second = framed(3, 2, &first[2].hash, lease.fence);
    engine.append(&lease, &second).unwrap();
    assert_eq!(running(&url, &kept.1), None, "the state it could not read");
    // With no state to seal by, the segment seals over its rows.
    let all = [first, second].concat();
    let next = chain(5, 1, &all[4].hash, &lease);
    let got = seal(engine.as_ref(), &lease, &next).unwrap();
    let digest = lakelands_core::log::digest_of(&all).unwrap();
    let got = got.expect("the segment sealed");
    assert_eq!(
        (got.first, got.last, &got.digest),
        (Position(0), Position(4), &digest)
    );
}

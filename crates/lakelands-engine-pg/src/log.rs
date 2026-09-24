//! The log as the format's lines, one row per event.
//!
//! **Rows.** An event's canonical bytes are stored unchanged, with the
//! position, id, `prev`, `hash` and fence the store hands beside them, and
//! the first position of the segment the event belongs to. A read gives
//! them back unchanged without parsing the bytes.
//!
//! **Segments.** The last segment is the active one and takes appends;
//! every other is sealed and never changes. Sealing starts the next
//! segment, named by the position its first event will take, and keeps the
//! sealed one's digest: the SHA-256 of its events' bytes, each followed by
//! one line feed, which is the digest of the file `docs/log-format.md`
//! §4.2 describes. [`export`] writes the segments as those files.
//!
//! **All or none.** An append is one transaction on the lease connection:
//! it checks the fence against the lease row, checks that the events
//! continue the log, inserts them and commits. After a crash an append is
//! either all there or not at all; nothing needs cutting off.

use std::io::Write as _;
use std::path::Path;

use lakelands_api::engine::{CanonicalEvent, Lease};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::id::Position;
use sha2::{Digest as _, Sha256};

use crate::connect::Db;
use crate::projection::Names;
use crate::{invalid, unavailable};

/// Create the first, empty segment of a new lake.
pub(crate) fn create(db: &mut Db, names: &Names) -> Result<()> {
    db.execute(
        &format!(
            "INSERT INTO {} (first) VALUES (0) ON CONFLICT DO NOTHING",
            names.t("segments")
        ),
        &[],
    )?;
    Ok(())
}

/// The last event's position and hash.
pub(crate) fn tip(db: &mut Db, names: &Names) -> Result<Option<(Position, Digest)>> {
    Ok(db
        .query_opt(
            &format!(
                "SELECT position, hash FROM {} ORDER BY position DESC LIMIT 1",
                names.t("log")
            ),
            &[],
        )?
        .map(|r| {
            (
                Position(r.get::<_, i64>(0) as u64),
                Digest(r.get::<_, String>(1)),
            )
        }))
}

/// The active segment's first position.
fn active(db: &mut Db, names: &Names) -> Result<i64> {
    db.query_one(
        &format!("SELECT max(first) FROM {}", names.t("segments")),
        &[],
    )?
    .get::<_, Option<i64>>(0)
    .ok_or_else(|| unavailable("the lake's log has no segment; it was not created by this engine"))
}

/// Refuse, inside the caller's transaction, a lease whose fence the lease
/// row no longer records. The row is locked until the transaction ends, so
/// no takeover commits between this check and what the transaction writes.
fn fenced(db: &mut Db, names: &Names, lease: &Lease, what: &str) -> Result<()> {
    let row = db.query_opt(
        &format!(
            "SELECT fence, holder FROM {} WHERE one = 1 FOR UPDATE",
            names.t("lease")
        ),
        &[],
    )?;
    let recorded = row.map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)));
    match recorded {
        Some((fence, _)) if fence == lease.fence as i64 => Ok(()),
        Some((fence, holder)) if !holder.is_empty() => Err(unavailable(format!(
            "the lease with fence {} is stale: `{holder}` holds the lake's writer lease with \
             fence {fence}; nothing was {what}",
            lease.fence
        ))),
        Some((fence, _)) => Err(unavailable(format!(
            "the lease with fence {} is stale: the lease row records fence {fence} and no \
             holder; nothing was {what}",
            lease.fence
        ))),
        None => Err(unavailable(format!(
            "the lease with fence {} is stale: the lake's lease row is gone; nothing was {what}",
            lease.fence
        ))),
    }
}

/// Append `events` under `lease`, in one transaction that checks the
/// lease row holds its fence.
pub(crate) fn append(
    db: &mut Db,
    names: &Names,
    lease: &Lease,
    events: &[CanonicalEvent],
) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
        fenced(db, names, lease, "appended")?;
        let tip = tip(db, names)?;
        let first = tip.as_ref().map_or(0, |(p, _)| p.0 + 1);
        let mut prev = tip.map_or_else(Digest::zero, |(_, h)| h);
        for (expected, event) in (first..).zip(events) {
            // The event at position 0 is written before any lease exists in
            // the lake, under fence 0 (`docs/log-format.md` §4.1).
            let wanted = if event.position.0 == 0 {
                0
            } else {
                lease.fence
            };
            if event.fence != wanted {
                return Err(invalid(format!(
                    "the event at position {} carries fence {}, and it must carry fence \
                     {wanted}; nothing was appended",
                    event.position, event.fence
                )));
            }
            if event.position.0 != expected {
                return Err(invalid(format!(
                    "the event at position {} does not continue the log, whose next position \
                     is {expected}; nothing was appended",
                    event.position
                )));
            }
            if event.prev != prev {
                return Err(invalid(format!(
                    "the event at position {} names `{}` as the hash before it, but that hash \
                     is `{}`; nothing was appended",
                    event.position, event.prev.0, prev.0
                )));
            }
            if event.bytes.contains(&b'\n') {
                return Err(invalid(format!(
                    "the event at position {} holds a line feed in its bytes, which canonical \
                     bytes never do; nothing was appended",
                    event.position
                )));
            }
            prev = event.hash.clone();
        }
        let segment = active(db, names)?;
        let insert = format!(
            "INSERT INTO {} (position, segment, fence, id, prev, hash, bytes) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            names.t("log")
        );
        for e in events {
            db.execute(
                &insert,
                &[
                    &(e.position.0 as i64),
                    &segment,
                    &(e.fence as i64),
                    &e.id.0,
                    &e.prev.0,
                    &e.hash.0,
                    &e.bytes,
                ],
            )?;
        }
        Ok(())
    })
}

/// At most `limit` events from `from` on, as appended.
pub(crate) fn read(
    db: &mut Db,
    names: &Names,
    from: Position,
    limit: u32,
) -> Result<Vec<CanonicalEvent>> {
    // Positions are kept as `bigint`: nothing lies past its range.
    let Ok(from) = i64::try_from(from.0) else {
        return Ok(Vec::new());
    };
    if limit == 0 {
        return Ok(Vec::new());
    }
    let rows = db.query(
        &format!(
            "SELECT position, fence, id, prev, hash, bytes FROM {} WHERE position >= $1 \
             ORDER BY position LIMIT $2",
            names.t("log")
        ),
        &[&from, &i64::from(limit)],
    )?;
    Ok(rows
        .iter()
        .map(|r| CanonicalEvent {
            position: Position(r.get::<_, i64>(0) as u64),
            fence: r.get::<_, i64>(1) as u64,
            id: EventId(r.get(2)),
            prev: Digest(r.get(3)),
            hash: Digest(r.get(4)),
            bytes: r.get(5),
        })
        .collect())
}

/// A segment's lines: each event's bytes followed by one line feed.
fn lines(db: &mut Db, names: &Names, first: i64) -> Result<Vec<u8>> {
    let rows = db.query(
        &format!(
            "SELECT bytes FROM {} WHERE segment = $1 ORDER BY position",
            names.t("log")
        ),
        &[&first],
    )?;
    let mut out = Vec::new();
    for r in rows {
        out.extend_from_slice(&r.get::<_, Vec<u8>>(0));
        out.push(b'\n');
    }
    Ok(out)
}

fn digest_of(bytes: &[u8]) -> Digest {
    Digest(lakelands_core::canonical::hex(&Sha256::digest(bytes)))
}

/// The first and last positions a segment holds, if it holds any.
fn span(db: &mut Db, names: &Names, first: i64) -> Result<Option<(Position, Position)>> {
    let row = db.query_one(
        &format!(
            "SELECT min(position), max(position) FROM {} WHERE segment = $1",
            names.t("log")
        ),
        &[&first],
    )?;
    Ok(
        match (row.get::<_, Option<i64>>(0), row.get::<_, Option<i64>>(1)) {
            (Some(a), Some(b)) => Some((Position(a as u64), Position(b as u64))),
            _ => None,
        },
    )
}

/// Close the active segment, if it holds any event, and start the next,
/// in one transaction that checks the lease row holds the lease's fence.
pub(crate) fn seal(db: &mut Db, names: &Names, lease: &Lease) -> Result<Option<SegmentDigest>> {
    db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
        fenced(db, names, lease, "sealed")?;
        let first = active(db, names)?;
        let Some((from, to)) = span(db, names, first)? else {
            return Ok(None);
        };
        let digest = digest_of(&lines(db, names, first)?);
        db.execute(
            &format!(
                "UPDATE {} SET digest = $2 WHERE first = $1",
                names.t("segments")
            ),
            &[&first, &digest.0],
        )?;
        db.execute(
            &format!("INSERT INTO {} (first) VALUES ($1)", names.t("segments")),
            &[&((to.0 + 1) as i64)],
        )?;
        Ok(Some(SegmentDigest {
            first: from,
            last: to,
            digest,
        }))
    })
}

/// Every sealed segment, oldest first.
pub(crate) fn segments(db: &mut Db, names: &Names) -> Result<Vec<SegmentDigest>> {
    let rows = db.query(
        &format!(
            "SELECT first, digest FROM {} WHERE first < (SELECT max(first) FROM {}) ORDER BY first",
            names.t("segments"),
            names.t("segments")
        ),
        &[],
    )?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let first: i64 = r.get(0);
        let Some((from, to)) = span(db, names, first)? else {
            continue;
        };
        let digest = match r.get::<_, Option<String>>(1) {
            Some(d) => Digest(d),
            None => digest_of(&lines(db, names, first)?),
        };
        out.push(SegmentDigest {
            first: from,
            last: to,
            digest,
        });
    }
    Ok(out)
}

/// Write every segment that holds an event as
/// `<dir>/log/<first position in 20 digits>.jsonl`, the files
/// `docs/log-format.md` §4.2 describes; an active segment still empty
/// after a seal has no file.
pub(crate) fn export(db: &mut Db, names: &Names, dir: &Path) -> Result<()> {
    let log = dir.join("log");
    std::fs::create_dir_all(&log)
        .map_err(|e| unavailable(format!("creating {} failed: {e}", log.display())))?;
    let firsts: Vec<i64> = db
        .query(
            &format!("SELECT first FROM {} ORDER BY first", names.t("segments")),
            &[],
        )?
        .iter()
        .map(|r| r.get(0))
        .collect();
    for first in firsts {
        let bytes = lines(db, names, first)?;
        if bytes.is_empty() {
            continue;
        }
        let path = log.join(format!("{first:020}.jsonl"));
        std::fs::File::create(&path)
            .and_then(|mut f| f.write_all(&bytes).and_then(|()| f.sync_all()))
            .map_err(|e| unavailable(format!("writing {} failed: {e}", path.display())))?;
    }
    Ok(())
}

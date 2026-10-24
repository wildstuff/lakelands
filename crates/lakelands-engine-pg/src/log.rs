//! The log as the format's lines, one row per event.
//!
//! **Rows.** An event's canonical bytes are stored unchanged, with the
//! position, id, `prev`, `hash` and fence the store hands beside them, and
//! the first position of the segment the event belongs to. A read gives
//! them back unchanged without parsing the bytes.
//!
//! **Segments.** The last segment is the active one and takes appends;
//! every other is sealed and never changes. Sealing starts the next
//! segment, named by the position of its first event, and inserts that
//! segment's first events in the same transaction, so no later segment is
//! ever empty; it keeps the sealed one's digest: the SHA-256 of its
//! events' bytes, each followed by one line feed, which is the digest of
//! the file `docs/log-format.md` §4.2 describes. [`export`] writes the
//! segments as those files.
//!
//! **All or none.** An append is one transaction on the lease connection:
//! it checks the fence against the lease row, checks that the events
//! continue the log, inserts them and commits. After a crash an append is
//! either all there or not at all; nothing needs cutting off.
//!
//! **Carried.** The writer knows the log's end without asking: the take of
//! the lease reads the tip and the active segment once ([`Carried`]), and
//! every append and seal under that lease carries them on. No other writer
//! appends while the lease row holds this lease's fence, which each append
//! checks, so what the writer carries is what the server holds; were it
//! not, the log's key on the position would still refuse a second event
//! at one position. An append that fails drops what it carried, and the
//! next reads it again.
//!
//! **Synced.** An append's and a seal's commits wait for their flush
//! whatever the server, the database or the role sets, so an answered
//! append outlasts a crash, and it flushes the projection commits before
//! it (`docs/decisions/061-a-projection-commit-waits-for-no-flush.md`).

use std::io::Write as _;
use std::path::Path;

use lakelands_api::engine::{CanonicalEvent, Lease};
use lakelands_api::error::{Act, Case, Doing, Error, Part, Result, Situation};
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::id::Position;
use sha2::{Digest as _, Sha256};

use crate::connect::Db;
use crate::projection::Names;

/// How an append and a seal begin: synced, whatever the session's default.
const SYNCED: &str = "BEGIN ISOLATION LEVEL READ COMMITTED; SET LOCAL synchronous_commit = on";

/// Create the first, empty segment of a new lake.
pub(crate) fn create(db: &mut Db, names: &Names) -> Result<()> {
    db.execute(
        &format!(
            "INSERT INTO {} (first) VALUES (0) ON CONFLICT DO NOTHING",
            names.t("segments")
        ),
        &[],
    )
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

/// What the writer under one lease carries from append to append: the
/// log's tip and the active segment's first position.
#[derive(Clone, Debug)]
pub(crate) struct Carried {
    fence: u64,
    tip: Option<(Position, Digest)>,
    segment: i64,
}

/// What the writer under `fence` carries, read from the server: by the
/// take of the lease, in the transaction that wrote the fence, and by an
/// append after a failed one.
pub(crate) fn carried(db: &mut Db, names: &Names, fence: u64) -> Result<Carried> {
    Ok(Carried {
        fence,
        tip: tip(db, names)?,
        segment: active(db, names)?,
    })
}

/// What `carried` holds for `lease`, or what the server holds where it
/// holds nothing for it.
fn carried_for(
    db: &mut Db,
    names: &Names,
    lease: &Lease,
    carried: &Option<Carried>,
) -> Result<Carried> {
    match carried {
        Some(c) if c.fence == lease.fence => Ok(c.clone()),
        _ => self::carried(db, names, lease.fence),
    }
}

/// The active segment's first position.
fn active(db: &mut Db, names: &Names) -> Result<i64> {
    db.query_one(
        &format!("SELECT max(first) FROM {}", names.t("segments")),
        &[],
    )?
    .get::<_, Option<i64>>(0)
    .ok_or_else(|| {
        Error::unavailable(Situation::Unmade {
            part: Part::Segment,
        })
    })
}

/// Refuse, inside the caller's transaction, a lease whose fence the lease
/// row no longer records. The row is locked until the transaction ends, so
/// no takeover commits between this check and what the transaction writes.
fn fenced(db: &mut Db, names: &Names, lease: &Lease, undone: Act) -> Result<()> {
    let row = db.query_opt(
        &format!(
            "SELECT fence, holder FROM {} WHERE one = 1 FOR UPDATE",
            names.t("lease")
        ),
        &[],
    )?;
    let recorded = row.map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1)));
    let (holder, current) = match recorded {
        Some((fence, _)) if fence == lease.fence as i64 => return Ok(()),
        Some((fence, holder)) => ((!holder.is_empty()).then_some(holder), Some(fence as u64)),
        None => (None, None),
    };
    Err(Error::unavailable(Situation::LeaseStale {
        fence: lease.fence,
        holder,
        current,
        undone: Some(undone),
    }))
}

/// Append `events` under `lease`, in one transaction that checks the
/// lease row holds its fence. With `imported`, as an import does: the log
/// must hold nothing but imported events, each event keeps its own fence,
/// never below the one before it, and the `meta` rows `imported` (the last
/// position imported) and `import_fence` (the greatest fence imported,
/// which the next lease exceeds) follow in the same transaction. The tip
/// and the segment are `carried`'s, which the commit moves on.
pub(crate) fn append(
    db: &mut Db,
    names: &Names,
    lease: &Lease,
    carried: &mut Option<Carried>,
    events: &[CanonicalEvent],
    imported: bool,
) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let held = carried.take();
    let moved = db.transaction(SYNCED, |db| {
        fenced(db, names, lease, Act::Appended)?;
        let at = carried_for(db, names, lease, &held)?;
        add(db, names, lease, at, events, imported)
    })?;
    *carried = Some(moved);
    Ok(())
}

/// What an append does inside its transaction, the lease row checked:
/// `events` are checked to continue the log at `at` and inserted into its
/// segment. Answers what the writer carries once they are.
fn add(
    db: &mut Db,
    names: &Names,
    lease: &Lease,
    at: Carried,
    events: &[CanonicalEvent],
    imported: bool,
) -> Result<Carried> {
    let Carried {
        fence: under,
        tip,
        segment,
    } = at;
    let first = tip.as_ref().map_or(0, |(p, _)| p.0 + 1);
    let mut floor = 0;
    if imported && let Some((at, _)) = &tip {
        let row = db.query_one(
            &format!(
                "SELECT (SELECT n FROM {m} WHERE k = 'imported'), \
                 (SELECT fence FROM {l} WHERE position = $1)",
                m = names.t("meta"),
                l = names.t("log"),
            ),
            &[&(at.0 as i64)],
        )?;
        if row.get::<_, Option<i64>>(0) != Some(at.0 as i64) {
            return Err(Error::invalid(Case::ImportIntoHeld { tip: *at }));
        }
        floor = row.get::<_, Option<i64>>(1).unwrap_or(0) as u64;
    }
    let fence = (!imported).then_some(lease.fence);
    let prev = tip.as_ref().map_or_else(Digest::zero, |(_, h)| h.clone());
    lakelands_core::engines::append_fault(events, first, prev, fence, floor)?;
    // The window's events in one statement, from arrays.
    let (mut positions, mut fences) = (Vec::new(), Vec::new());
    let (mut ids, mut prevs, mut hashes, mut bytes) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for e in events {
        positions.push(e.position.0 as i64);
        fences.push(e.fence as i64);
        ids.push(e.id.0.as_str());
        prevs.push(e.prev.0.as_str());
        hashes.push(e.hash.0.as_str());
        bytes.push(e.bytes.as_slice());
    }
    db.execute(
        &format!(
            "INSERT INTO {} (position, segment, fence, id, prev, hash, bytes) \
             SELECT p, $2, f, i, v, h, b FROM unnest($1::int8[], $3::int8[], $4::text[], \
             $5::text[], $6::text[], $7::bytea[]) AS e(p, f, i, v, h, b)",
            names.t("log")
        ),
        &[&positions, &segment, &fences, &ids, &prevs, &hashes, &bytes],
    )?;
    if imported && let Some(last) = events.last() {
        db.execute(
            &format!(
                "INSERT INTO {m} (k, n) VALUES ('imported', $1), ('import_fence', $2) \
                 ON CONFLICT (k) DO UPDATE SET n = greatest({m}.n, EXCLUDED.n)",
                m = names.t("meta")
            ),
            &[&(last.position.0 as i64), &(last.fence as i64)],
        )?;
    }
    let last = events.last().map(|e| (e.position, e.hash.clone()));
    Ok(Carried {
        fence: under,
        tip: last.or(tip),
        segment,
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

/// Hand a segment's lines to `f` in position order, one at a time: each
/// event's bytes, without the line feed that ends it in the file. A
/// segment is never held whole.
fn each_line(
    db: &mut Db,
    names: &Names,
    first: i64,
    mut f: impl FnMut(&[u8]) -> Result<()> + Send,
) -> Result<()> {
    db.query_each(
        &format!(
            "SELECT bytes FROM {} WHERE segment = $1 ORDER BY position",
            names.t("log")
        ),
        &[&first],
        |r| f(r.get::<_, &[u8]>(0)),
    )
}

/// A segment's digest (`docs/log-format.md` §4.2): the SHA-256 of its
/// lines, each followed by a line feed, hashed as they are read.
fn digest_of(db: &mut Db, names: &Names, first: i64) -> Result<Digest> {
    let mut hash = Sha256::new();
    each_line(db, names, first, |line| {
        hash.update(line);
        hash.update(b"\n");
        Ok(())
    })?;
    Ok(Digest(lakelands_core::canonical::hex(&hash.finalize())))
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

/// Close the active segment, if it holds any event, and begin the next
/// with the events `first` answers for it, appended as [`append`] does
/// with `imported`: one transaction that checks the lease row holds the
/// lease's fence, so the seal and those events are both there or neither.
/// The segment it closes is `carried`'s, and the commit carries on the one
/// it begins.
pub(crate) fn seal(
    db: &mut Db,
    names: &Names,
    lease: &Lease,
    carried: &mut Option<Carried>,
    imported: bool,
    first: &mut dyn FnMut(&SegmentDigest) -> Result<Vec<CanonicalEvent>>,
) -> Result<Option<SegmentDigest>> {
    let held = carried.take();
    let (sealed, moved) = db.transaction(SYNCED, |db| {
        fenced(db, names, lease, Act::Sealed)?;
        let mut at = carried_for(db, names, lease, &held)?;
        let active = at.segment;
        let Some((from, to)) = span(db, names, active)? else {
            return Ok((None, at));
        };
        let digest = digest_of(db, names, active)?;
        let sealed = SegmentDigest {
            first: from,
            last: to,
            digest,
        };
        let events = lakelands_core::engines::first_of_next(first, &sealed)?;
        db.execute(
            &format!(
                "UPDATE {} SET digest = $2 WHERE first = $1",
                names.t("segments")
            ),
            &[&active, &sealed.digest.0],
        )?;
        db.execute(
            &format!("INSERT INTO {} (first) VALUES ($1)", names.t("segments")),
            &[&((to.0 + 1) as i64)],
        )?;
        at.segment = (to.0 + 1) as i64;
        let moved = add(db, names, lease, at, &events, imported)?;
        Ok((Some(sealed), moved))
    })?;
    *carried = Some(moved);
    Ok(sealed)
}

/// Every sealed segment, oldest first.
pub(crate) fn segments(db: &mut Db, names: &Names) -> Result<Vec<SegmentDigest>> {
    let rows = db.query(
        &format!(
            "SELECT first, digest FROM {s} WHERE first < (SELECT max(first) FROM {s}) ORDER BY first",
            s = names.t("segments")
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
            None => digest_of(db, names, first)?,
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
/// `docs/log-format.md` §4.2 describes; a new lake's first segment,
/// still empty, has no file.
pub(crate) fn export(db: &mut Db, names: &Names, dir: &Path) -> Result<()> {
    let log = dir.join("log");
    std::fs::create_dir_all(&log)
        .map_err(|e| lakelands_core::io_failed(Doing::Create, Part::Export, &e))?;
    let firsts: Vec<i64> = db.column(
        &format!("SELECT first FROM {} ORDER BY first", names.t("segments")),
        &[],
    )?;
    for first in firsts {
        // Written as the rows arrive, under a temporary name that becomes
        // the file's once it is whole and synced: a failed export leaves
        // no file short of its segment. A segment with no line has none.
        let path = log.join(format!("{first:020}.jsonl"));
        let partial = log.join(format!("{first:020}.jsonl.partial"));
        let failed = |e: std::io::Error| lakelands_core::io_failed(Doing::Write, Part::Export, &e);
        let mut file: Option<std::io::BufWriter<std::fs::File>> = None;
        each_line(db, names, first, |line| {
            if file.is_none() {
                file = Some(std::io::BufWriter::new(
                    std::fs::File::create(&partial).map_err(failed)?,
                ));
            }
            let w = file.as_mut().expect("opened above");
            w.write_all(line)
                .and_then(|()| w.write_all(b"\n"))
                .map_err(failed)
        })?;
        if let Some(w) = file {
            w.into_inner()
                .map_err(|e| failed(e.into_error()))
                .and_then(|f| f.sync_all().map_err(failed))
                .and_then(|()| std::fs::rename(&partial, &path).map_err(failed))?;
        }
    }
    Ok(())
}

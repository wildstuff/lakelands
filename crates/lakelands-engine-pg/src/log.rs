//! The log as the format's lines, one row per event.
//!
//! **Rows.** An event's line, its bytes in the stored form, is kept
//! unchanged, with the position, id, `prev`, `hash` and fence the store
//! hands beside it, and the first position of the segment the event
//! belongs to; a frame the events of an append share is one row of
//! `frames`, keyed by the first position it covers, beside them. A read
//! gives each event back unchanged with the frame that covers it, without
//! parsing the bytes. A lake made before frames has no `frames` table
//! until a writer opens it, and a read there finds none.
//!
//! **Segments.** The last segment is the active one and takes appends;
//! every other is sealed and never changes. Sealing starts the next
//! segment, named by the position of its first event, and inserts that
//! segment's first events in the same transaction, so no later segment is
//! ever empty; it keeps the sealed one's digest: the SHA-256 of its events
//! as they restore, each followed by one line feed, which is the digest
//! `docs/log-format.md` §4.2 names. [`export`] writes the segments as the
//! files §4.2 describes, each frame's line before its first event's.
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
//! checks, so what the writer carries is what the server holds. A carried
//! tip behind the server's would meet the log's key on the position,
//! which refuses a second event at one position; nothing refuses one
//! ahead of it, and nothing but this writer's commits moves it. An append
//! that fails drops what it carried, and the next reads it again.
//!
//! **Ids and hashes as bytes.** `id`, `prev` and `hash` are kept as the
//! bytes their hex digits spell (`bytea`: 16, 32 and 32), and handed back
//! as the same lower-case hex; every read takes them in the type the
//! catalogue gave them at the handle's open ([`Names::hex`]), so the ends
//! of a log a release before 0.21.0 kept as text, and a build ahead that
//! reads one, read as before. Such a log is converted in place, once, by
//! [`convert`], and gains the index on `id` that answers an applied id
//! (`docs/decisions/086-an-engine-keeps-an-events-id-once-and-its-hashes-as-bytes.md`).
//!
//! **Synced.** An append's and a seal's commits wait for their flush
//! whatever the server, the database or the role sets, so an answered
//! append outlasts a crash, and it flushes the projection commits before
//! it (`docs/decisions/061-a-projection-commit-waits-for-no-flush.md`).

use std::io::Write as _;
use std::path::Path;

use std::sync::Arc;

use lakelands_api::engine::{CanonicalEvent, Frame, Lease};
use lakelands_api::error::{Act, Case, Doing, Error, Flaw, Part, Result, Situation};
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::id::Position;
use sha2::{Digest as _, Sha256};

use crate::connect::Db;
use crate::projection::{HEX_COLUMNS, Names};

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

/// Convert a log a release before 0.21.0 kept, its `id`, `prev` and
/// `hash` as text, to this build's, in the caller's transaction, under
/// the lease and before any append: each column still of type `text`
/// becomes the bytes it spells, the index on `id` is made where missing,
/// and `applied` is dropped. It does only what the catalogue shows undone,
/// so a log converted is untouched; the tables of this build's schema are
/// made before it, by the writer's open or in the rebuild's transaction,
/// so a release before refuses the schema before it reads a converted
/// row. A log holding an id not of its form is refused, naming the first
/// position holding one, and nothing is converted. Answers whether it
/// changed anything.
pub(crate) fn convert(db: &mut Db, names: &Names) -> Result<bool> {
    let row = db.query_one(
        "SELECT (SELECT array_agg(attname::text) FROM pg_catalog.pg_attribute \
         WHERE attrelid = to_regclass($1) AND atttypid = 'text'::regtype \
         AND attname::text = ANY($2) AND NOT attisdropped), \
         to_regclass($3) IS NULL, to_regclass($4) IS NOT NULL",
        &[
            &names.t("log"),
            &HEX_COLUMNS.to_vec(),
            &names.t("log_id"),
            &names.t("applied"),
        ],
    )?;
    let text = row.get::<_, Option<Vec<String>>>(0).unwrap_or_default();
    let (unindexed, applied) = (row.get::<_, bool>(1), row.get::<_, bool>(2));
    if text.is_empty() && !unindexed && !applied {
        return Ok(false);
    }
    // The rewrite takes as long as the log is big: no statement timeout
    // a host set holds it, past the transaction it runs in.
    db.batch("SET LOCAL statement_timeout = 0")?;
    if text.iter().any(|c| c == "id") {
        let odd = format!(
            "SELECT min(position) FROM {} WHERE id !~ '^[0-9a-f]{{32}}$'",
            names.t("log")
        );
        if let Some(at) = db.query_one(&odd, &[])?.get::<_, Option<i64>>(0) {
            let at = Some(Position(at as u64));
            return Err(lakelands_core::damaged(Part::Log, at, Flaw::IdNotOfForm));
        }
    }
    let (log, space) = (names.t("log"), Names::space(&names.at.log));
    let mut sql = String::new();
    if !text.is_empty() {
        let altered: Vec<String> = (text.iter())
            .map(|c| format!("ALTER COLUMN {c} TYPE bytea USING decode({c}, 'hex')"))
            .collect();
        sql += &format!("ALTER TABLE {log} {};\n", altered.join(", "));
    }
    sql += &format!(
        "CREATE INDEX IF NOT EXISTS log_id ON {log} (id){space};\nDROP TABLE IF EXISTS {};",
        names.t("applied")
    );
    db.batch(&sql)?;
    Ok(true)
}

/// The last event's position and hash.
pub(crate) fn tip(db: &mut Db, names: &Names) -> Result<Option<(Position, Digest)>> {
    Ok(db
        .query_opt(
            &format!(
                "SELECT position, {} FROM {} ORDER BY position DESC LIMIT 1",
                names.hex("hash"),
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
#[derive(Debug)]
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
    carried: Option<Carried>,
) -> Result<Carried> {
    match carried {
        Some(c) if c.fence == lease.fence => Ok(c),
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
        let at = carried_for(db, names, lease, held)?;
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
    lakelands_core::log::frame_fault(events)?;
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
             SELECT p, $2, f, {}, {}, {}, b FROM unnest($1::int8[], $3::int8[], $4::text[], \
             $5::text[], $6::text[], $7::bytea[]) AS e(p, f, i, v, h, b)",
            names.t("log"),
            names.kept("id", "i"),
            names.kept("prev", "v"),
            names.kept("hash", "h"),
        ),
        &[&positions, &segment, &fences, &ids, &prevs, &hashes, &bytes],
    )?;
    let frames = events
        .iter()
        .filter_map(|e| e.frame.as_ref().filter(|f| f.first == e.position));
    let (mut firsts, mut counts, mut lines) = (Vec::new(), Vec::new(), Vec::new());
    for f in frames {
        firsts.push(f.first.0 as i64);
        counts.push(f.events as i32);
        lines.push(f.bytes.as_slice());
    }
    if !firsts.is_empty() {
        db.execute(
            &format!(
                "INSERT INTO {} (first, events, bytes) \
                 SELECT * FROM unnest($1::int8[], $2::int4[], $3::bytea[])",
                names.t("frames")
            ),
            &[&firsts, &counts, &lines],
        )?;
    }
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
            "SELECT position, fence, {}, {}, {}, bytes, to_regclass($3) IS NOT NULL \
             FROM {} WHERE position >= $1 ORDER BY position LIMIT $2",
            names.hex("id"),
            names.hex("prev"),
            names.hex("hash"),
            names.t("log")
        ),
        &[&from, &i64::from(limit), &names.t("frames")],
    )?;
    let mut events: Vec<CanonicalEvent> = rows
        .iter()
        .map(|r| CanonicalEvent {
            position: Position(r.get::<_, i64>(0) as u64),
            fence: r.get::<_, i64>(1) as u64,
            id: EventId(r.get(2)),
            prev: Digest(r.get(3)),
            hash: Digest(r.get(4)),
            bytes: r.get(5),
            frame: None,
        })
        .collect();
    let (Some(first), Some(last)) = (events.first(), events.last()) else {
        return Ok(events);
    };
    if !rows[0].get::<_, bool>(6) {
        return Ok(events);
    }
    let (first, last) = (first.position.0 as i64, last.position.0 as i64);
    // Frames never overlap, so the first that can cover `first` is the
    // last to begin at or before it: the key bounds the scan both ways.
    let frames = db.query(
        &format!(
            "SELECT first, events, bytes FROM {f} WHERE first <= $2 AND first >= \
             coalesce((SELECT max(first) FROM {f} WHERE first <= $1), $1) ORDER BY first",
            f = names.t("frames")
        ),
        &[&first, &last],
    )?;
    let frames: Vec<Arc<Frame>> = frames.iter().map(frame_of).collect();
    for event in &mut events {
        let after = frames.partition_point(|f| f.first <= event.position);
        let frame = after.checked_sub(1).map(|i| &frames[i]);
        event.frame = frame.filter(|f| covers(f, event.position)).cloned();
    }
    Ok(events)
}

/// A row of `frames`, as the frame it keeps.
fn frame_of(row: &postgres::Row) -> Arc<Frame> {
    Arc::new(Frame {
        first: Position(row.get::<_, i64>(0) as u64),
        events: row.get::<_, i32>(1) as u32,
        bytes: row.get(2),
    })
}

/// Whether `frame` covers `position`.
fn covers(frame: &Frame, position: Position) -> bool {
    position
        .0
        .checked_sub(frame.first.0)
        .is_some_and(|i| i < u64::from(frame.events))
}

/// Hand a segment's events to `f` in position order, one at a time, each
/// with the frame that covers it and, at the frame's first event, the
/// frame's line: what the file of the segment holds before the event's
/// own. A segment is never held whole.
fn each_event(
    db: &mut Db,
    names: &Names,
    first: i64,
    mut f: impl FnMut(Option<&[u8]>, &CanonicalEvent) -> Result<()> + Send,
) -> Result<()> {
    let mut held: Option<Arc<Frame>> = None;
    let framed = db
        .query_one("SELECT to_regclass($1) IS NOT NULL", &[&names.t("frames")])?
        .get::<_, bool>(0);
    // A lake no writer of frames has opened has no `frames` table.
    let (frame, join) = match framed {
        true => {
            let join = format!("LEFT JOIN {} f ON f.first = l.position", names.t("frames"));
            ("f.events, f.bytes", join)
        }
        false => ("NULL::int, NULL::bytea", String::new()),
    };
    db.query_each(
        &format!(
            "SELECT l.position, {prev}, l.bytes, {frame}, {id}, {hash}, l.fence FROM {} l {join} \
             WHERE l.segment = $1 ORDER BY l.position",
            names.t("log"),
            prev = names.hex("l.prev"),
            id = names.hex("l.id"),
            hash = names.hex("l.hash"),
        ),
        &[&first],
        |r| {
            let position = Position(r.get::<_, i64>(0) as u64);
            if let Some(events) = r.get::<_, Option<i32>>(3) {
                let (events, bytes) = (events as u32, r.get(4));
                held = Some(Arc::new(Frame {
                    first: position,
                    events,
                    bytes,
                }));
            }
            let frame = held.clone().filter(|f| covers(f, position));
            let event = CanonicalEvent {
                position,
                prev: Digest(r.get(1)),
                bytes: r.get(2),
                frame,
                id: EventId(r.get(5)),
                hash: Digest(r.get(6)),
                fence: r.get::<_, i64>(7) as u64,
            };
            let line = event.frame.as_ref().filter(|f| f.first == position);
            f(line.map(|f| f.bytes.as_slice()), &event)
        },
    )
}

/// A segment's digest (`docs/log-format.md` §4.2): the SHA-256 of its
/// events as they restore, each followed by a line feed, hashed as they
/// are read.
fn digest_of(db: &mut Db, names: &Names, first: i64) -> Result<Digest> {
    let mut hash = Sha256::new();
    each_event(db, names, first, |_, event| {
        let (restored, whole) = lakelands_core::log::restore_json(event)
            .map_err(|flaw| lakelands_core::damaged(Part::Log, Some(event.position), flaw))?;
        match whole {
            true => hash.update(&event.bytes),
            false => hash.update(lakelands_core::canonical::to_vec(&restored)),
        }
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
        let mut at = carried_for(db, names, lease, held)?;
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
        each_event(db, names, first, |frame, event| {
            if file.is_none() {
                file = Some(std::io::BufWriter::new(
                    std::fs::File::create(&partial).map_err(failed)?,
                ));
            }
            let w = file.as_mut().expect("opened above");
            for line in frame.into_iter().chain([event.bytes.as_slice()]) {
                w.write_all(line)
                    .and_then(|()| w.write_all(b"\n"))
                    .map_err(failed)?;
            }
            Ok(())
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

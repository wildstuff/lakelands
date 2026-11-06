//! The log as the format's units, one row per block or line.
//!
//! **Rows.** A row is a unit of the stored form (`docs/log-format.md`
//! §1.5): a block, its first line and its stored bytes as the frame its
//! events share holds them, or the line of an event appended alone. It is
//! keyed by its first position and keeps its last one, the `prev` of its
//! first event and the `hash` of its last, so that the tip, a segment's
//! end and a read's first row are found without its bytes. A block is
//! compressed already, so its bytes are kept as they are, never
//! compressed again (`STORAGE EXTERNAL`). A read restores each event from its row: its
//! line, and the position, id, `prev`, `hash` and fence the store handed
//! beside it, which a block's header and its lines decide. Where an append
//! hands events their row would not give back so, an engine's checks'
//! events and a block that does not restore among them, the row keeps
//! them as handed (`kept`), and they come back unchanged.
//!
//! **Ids.** Each event's id and position are a row of `ids`, keyed by
//! both, which answers an applied id. Ids and hashes are kept as the bytes
//! their hex digits spell (`bytea`: 16 and 32) and handed back as the same
//! lower-case hex ([`Names::hex`])
//! (`docs/decisions/086-an-engine-keeps-an-events-id-once-and-its-hashes-as-bytes.md`).
//!
//! **Segments.** A segment is the range of positions from its first to the
//! next one's. The last is the active one and takes appends; every other
//! is sealed and never changes. The active segment's digest runs as its
//! events are appended, over them as they restore, each followed by one
//! line feed, the digest `docs/log-format.md` §4.2 names: each append
//! keeps its state on the segment's row in its transaction, so a seal
//! restores nothing, and a writer that takes the lease goes on from it.
//! Where an event did not restore it keeps none, and the seal digests the
//! segment from its rows, which names the event. Sealing starts the next
//! segment, named by the position of its first event, and inserts that
//! segment's first events in the same transaction, so no later segment is
//! ever empty. [`export`] writes each segment's rows as the file §4.2
//! describes.
//!
//! **All or none.** An append is one transaction on the lease connection:
//! it checks the fence against the lease row, checks that the events
//! continue the log, inserts them and commits. After a crash an append is
//! either all there or not at all; nothing needs cutting off.
//!
//! **Carried.** The writer knows the log's end without asking: the take of
//! the lease reads the tip, the active segment and its digest once
//! ([`Carried`]), and every append and seal under that lease carries them
//! on. No other writer appends while the lease row holds this lease's
//! fence, which each append checks, so what the writer carries is what the
//! server holds. A carried tip behind the server's would meet the log's key
//! on the position, which refuses a second unit at one position; nothing
//! refuses one ahead of it, and nothing but this writer's commits moves it.
//! An append that fails drops what it carried, and the next reads it again.
//!
//! **Synced.** An append's and a seal's commits wait for their flush
//! whatever the server, the database or the role sets, so an answered
//! append outlasts a crash, and it flushes the projection commits before
//! it (`docs/decisions/061-a-projection-commit-waits-for-no-flush.md`).

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::sync::Arc;

use lakelands_api::engine::{CanonicalEvent, Frame, Lease};
use lakelands_api::error::{Act, Case, Doing, Error, Flaw, Part, Result, Situation};
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::id::Position;
use lakelands_core::log::{Digesting, Restoring};
use serde_json::Value;

use crate::connect::Db;
use crate::projection::Names;

/// How an append and a seal begin: synced, whatever the session's default.
const SYNCED: &str = "BEGIN ISOLATION LEVEL READ COMMITTED; SET LOCAL synchronous_commit = on";

/// Create the first, empty segment of a new lake, its digest begun.
pub(crate) fn create(db: &mut Db, names: &Names) -> Result<()> {
    db.execute(
        &format!(
            "INSERT INTO {} (first, running) VALUES (0, $1) ON CONFLICT DO NOTHING",
            names.t("segments")
        ),
        &[&Digesting::default().state()],
    )
}

/// The last event's position and hash.
pub(crate) fn tip(db: &mut Db, names: &Names) -> Result<Option<(Position, Digest)>> {
    Ok(db
        .query_opt(
            &format!(
                "SELECT last, {} FROM {} ORDER BY position DESC LIMIT 1",
                Names::hex("hash"),
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
/// log's tip, the active segment's first position and its digest so far,
/// none where it is not known.
#[derive(Debug)]
pub(crate) struct Carried {
    fence: u64,
    tip: Option<(Position, Digest)>,
    segment: i64,
    digesting: Option<Digesting>,
}

/// What the writer under `fence` carries, read from the server: by the
/// take of the lease, in the transaction that wrote the fence, and by an
/// append or a seal after a failed one.
pub(crate) fn carried(db: &mut Db, names: &Names, fence: u64) -> Result<Carried> {
    let active = db.query_opt(
        &format!(
            "SELECT first, running FROM {} ORDER BY first DESC LIMIT 1",
            names.t("segments")
        ),
        &[],
    )?;
    let Some(active) = active else {
        return Err(Error::unavailable(Situation::Unmade {
            part: Part::Segment,
        }));
    };
    let running = active.get::<_, Option<Vec<u8>>>(1);
    Ok(Carried {
        fence,
        tip: tip(db, names)?,
        segment: active.get(0),
        digesting: running.and_then(|r| Digesting::of_state(&r)),
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
/// which the next lease exceeds) follow in the same transaction. The tip,
/// the segment and its digest are `carried`'s, which the commit moves on.
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
/// `events` are checked to continue the log at `at`, inserted a row a
/// unit with their ids, and added to the active segment's digest. Answers
/// what the writer carries once they are.
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
        mut digesting,
    } = at;
    let first = tip.as_ref().map_or(0, |(p, _)| p.0 + 1);
    let mut floor = 0;
    if imported && let Some((at, _)) = &tip {
        let row = db.query_opt(
            &format!("SELECT n FROM {} WHERE k = 'imported'", names.t("meta")),
            &[],
        )?;
        if row.and_then(|r| r.get::<_, Option<i64>>(0)) != Some(at.0 as i64) {
            return Err(Error::invalid(Case::ImportIntoHeld { tip: *at }));
        }
        floor = read(db, names, *at, 1)?.pop().map_or(0, |e| e.fence);
    }
    let fence = (!imported).then_some(lease.fence);
    let prev = tip.as_ref().map_or_else(Digest::zero, |(_, h)| h.clone());
    lakelands_core::engines::append_fault(events, first, prev, fence, floor)?;
    lakelands_core::log::frame_fault(events)?;
    // The units' rows and the events' ids, each in one statement from arrays.
    let (mut positions, mut lasts, mut prevs, mut hashes) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut bytes, mut kept) = (Vec::new(), Vec::new());
    for unit in units(events) {
        let (first, last) = (&unit[0], &unit[unit.len() - 1]);
        let row = Row {
            position: first.position.0 as i64,
            last: last.position.0 as i64,
            prev: first.prev.0.clone(),
            hash: last.hash.0.clone(),
            bytes: first
                .frame
                .as_ref()
                .map_or(&first.bytes, |f| &f.bytes)
                .clone(),
            kept: None,
        };
        let restored = row.events().ok().filter(|(back, _)| back == unit);
        match (&mut digesting, &restored) {
            (Some(d), Some((_, canonical))) if first.frame.is_some() => {
                canonical.iter().for_each(|c| d.restored(c));
            }
            (Some(d), _) => d.add(unit),
            (None, _) => {}
        }
        kept.push(restored.is_none().then(|| keep(unit)));
        positions.push(row.position);
        lasts.push(row.last);
        prevs.push(row.prev);
        hashes.push(row.hash);
        bytes.push(row.bytes);
    }
    db.execute(
        &format!(
            "INSERT INTO {} (position, last, prev, hash, bytes, kept) \
             SELECT p, l, {}, {}, b, k FROM unnest($1::int8[], $2::int8[], $3::text[], \
             $4::text[], $5::bytea[], $6::bytea[]) AS u(p, l, v, h, b, k)",
            names.t("log"),
            Names::kept("v"),
            Names::kept("h"),
        ),
        &[&positions, &lasts, &prevs, &hashes, &bytes, &kept],
    )?;
    let ids: Vec<&str> = events.iter().map(|e| e.id.0.as_str()).collect();
    let ats: Vec<i64> = events.iter().map(|e| e.position.0 as i64).collect();
    db.execute(
        &format!(
            "INSERT INTO {} (id, at) SELECT {}, a FROM unnest($1::text[], $2::int8[]) AS e(i, a)",
            names.t("ids"),
            Names::kept("i"),
        ),
        &[&ids, &ats],
    )?;
    // The state on the segment's row follows this append, and is cleared
    // where the digest is not known: a state a writer could not read, or
    // could not carry, is one a later writer must not trust. A digest
    // whose row is not there to follow the rows is known no more: the
    // seal digests the segment from its rows.
    let state = digesting.as_ref().and_then(Digesting::state);
    let kept = db.query_opt(
        &format!(
            "UPDATE {} SET running = $2 WHERE first = $1 RETURNING first",
            names.t("segments")
        ),
        &[&segment, &state],
    )?;
    digesting = digesting.filter(|_| kept.is_some());
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
        digesting,
    })
}

/// `events` cut into the units their rows hold: a block's events, which
/// share its frame, or an event alone. [`lakelands_core::log::frame_fault`]
/// has checked each frame is held whole.
fn units(events: &[CanonicalEvent]) -> impl Iterator<Item = &[CanonicalEvent]> {
    let mut rest = events;
    std::iter::from_fn(move || {
        let n = rest
            .first()?
            .frame
            .as_ref()
            .map_or(1, |f| f.events as usize);
        let (unit, after) = rest.split_at(n.clamp(1, rest.len()));
        rest = after;
        Some(unit)
    })
}

/// A row of `log`, as read.
struct Row {
    position: i64,
    last: i64,
    prev: String,
    hash: String,
    bytes: Vec<u8>,
    kept: Option<Vec<u8>>,
}

impl Row {
    /// The row a unit is in a statement's answer: its first and last
    /// position, the `prev` and `hash` in hex, its bytes and what it keeps.
    fn of(r: &postgres::Row) -> Row {
        Row {
            position: r.get(0),
            last: r.get(1),
            prev: r.get(2),
            hash: r.get(3),
            bytes: r.get(4),
            kept: r.get(5),
        }
    }

    /// The events the row holds, as they were handed, and, of a block's,
    /// the canonical bytes each restores to. A row that keeps its events
    /// gives them as kept; one that keeps none restores them from its
    /// bytes, and none where they do not restore.
    fn events(&self) -> std::result::Result<(Vec<CanonicalEvent>, Vec<Vec<u8>>), Flaw> {
        // A kept unit says whether it was handed framed; else its bytes do.
        let framed = match &self.kept {
            Some(kept) => kept.first() == Some(&1),
            None => self.bytes.starts_with(b"{\"block\":"),
        };
        let frame = framed.then(|| {
            Arc::new(Frame {
                first: Position(self.position as u64),
                events: (self.last - self.position + 1) as u32,
                bytes: self.bytes.clone(),
            })
        });
        if let Some(kept) = &self.kept {
            let events = (kept.get(1..))
                .and_then(|k| unkeep(k, self.position, frame))
                .ok_or(Flaw::Disagrees)?;
            return match events.len() as i64 == self.last - self.position + 1 {
                true => Ok((events, Vec::new())),
                false => Err(Flaw::Disagrees),
            };
        }
        let Some(frame) = frame else {
            return self
                .line()
                .map(|e| (vec![e], Vec::new()))
                .ok_or(Flaw::Disagrees);
        };
        let block = lakelands_core::log::block_of(&frame).ok_or(Flaw::NotABlock)??;
        let feed = frame
            .bytes
            .iter()
            .position(|b| *b == b'\n')
            .ok_or(Flaw::NotABlock)?;
        let raw = lakelands_core::log::inflate(&block, &frame.bytes[feed + 1..])?;
        let mut lines: Vec<&[u8]> = raw[..raw.len() - 1].split(|b| *b == b'\n').collect();
        if lines.len() as u64 == block.events + 1 {
            lines.remove(0);
        }
        let input = std::io::BufReader::new(frame.bytes.as_slice().chain(&b"\n"[..]));
        let mut restoring = Restoring::new(input, false);
        let (mut events, mut canonical) = (Vec::new(), Vec::new());
        while let Some(r) = restoring.next_event().map_err(|f| match f {
            lakelands_core::log::Fault::Flaw(flaw) => flaw,
            _ => Flaw::BlockBytes,
        })? {
            let text = |name: &str| r.event.get(name).and_then(Value::as_str).map(str::to_owned);
            let line = lines.get(events.len()).ok_or(Flaw::BlockBytes)?;
            events.push(CanonicalEvent {
                position: Position(r.event.get("position").and_then(Value::as_u64).unwrap_or(0)),
                id: EventId(text("id").unwrap_or_default()),
                prev: Digest(text("prev").unwrap_or_default()),
                hash: Digest(text("hash").unwrap_or_default()),
                fence: r.event.get("fence").and_then(Value::as_u64).unwrap_or(0),
                bytes: line.to_vec(),
                frame: Some(frame.clone()),
            });
            canonical.push(r.bytes);
        }
        match events.len() as u64 == block.events {
            true => Ok((events, canonical)),
            false => Err(Flaw::BlockBytes),
        }
    }

    /// The event a line alone is: its id, `hash` and fence its own, its
    /// position and `prev` the row's.
    fn line(&self) -> Option<CanonicalEvent> {
        let line: Value = serde_json::from_slice(&self.bytes).ok()?;
        let text = |name: &str| line.get(name)?.as_str().map(str::to_owned);
        Some(CanonicalEvent {
            position: Position(self.position as u64),
            id: EventId(text("id")?),
            prev: Digest(self.prev.clone()),
            hash: Digest(text("hash")?),
            fence: line.get("fence")?.as_u64()?,
            bytes: self.bytes.clone(),
            frame: None,
        })
    }
}

/// A unit's events as a row keeps them where its bytes do not give them
/// back: a byte, 1 where they were handed framed, then each one's fence,
/// its id, `prev`, `hash` and line, each after its length.
fn keep(unit: &[CanonicalEvent]) -> Vec<u8> {
    let mut out = vec![u8::from(unit.iter().any(|e| e.frame.is_some()))];
    for e in unit {
        out.extend(e.fence.to_be_bytes());
        for part in [
            e.id.0.as_bytes(),
            e.prev.0.as_bytes(),
            e.hash.0.as_bytes(),
            &e.bytes,
        ] {
            out.extend((part.len() as u32).to_be_bytes());
            out.extend(part);
        }
    }
    out
}

/// The events [`keep`] kept, after its first byte, from `first` on, each
/// with `frame`.
fn unkeep(kept: &[u8], first: i64, frame: Option<Arc<Frame>>) -> Option<Vec<CanonicalEvent>> {
    fn take<'k>(rest: &mut &'k [u8], n: usize) -> Option<&'k [u8]> {
        let (part, after) = rest.split_at_checked(n)?;
        *rest = after;
        Some(part)
    }
    fn part(rest: &mut &[u8]) -> Option<Vec<u8>> {
        let n = u32::from_be_bytes(take(rest, 4)?.try_into().ok()?);
        take(rest, n as usize).map(<[u8]>::to_vec)
    }
    let text = |b: Vec<u8>| String::from_utf8(b).ok();
    let (mut rest, mut events) = (kept, Vec::new());
    while !rest.is_empty() {
        let fence = u64::from_be_bytes(take(&mut rest, 8)?.try_into().ok()?);
        let (id, prev) = (part(&mut rest)?, part(&mut rest)?);
        let (hash, bytes) = (part(&mut rest)?, part(&mut rest)?);
        events.push(CanonicalEvent {
            position: Position(first as u64 + events.len() as u64),
            id: EventId(text(id)?),
            prev: Digest(text(prev)?),
            hash: Digest(text(hash)?),
            fence,
            bytes,
            frame: frame.clone(),
        });
    }
    (!events.is_empty()).then_some(events)
}

/// Hand each row of `log` the condition `which` picks to `f`, in position
/// order, one at a time.
fn rows(
    db: &mut Db,
    names: &Names,
    which: &str,
    params: &[&(dyn postgres::types::ToSql + Sync)],
    mut f: impl FnMut(Row) -> Result<()> + Send,
) -> Result<()> {
    db.query_each(
        &format!(
            "SELECT position, last, {}, {}, bytes, kept FROM {l} WHERE {which} ORDER BY position",
            Names::hex("prev"),
            Names::hex("hash"),
            l = names.t("log"),
        ),
        params,
        |r| f(Row::of(r)),
    )
}

/// The events a row holds, or the damage of one that keeps none and does
/// not restore.
fn events_of(row: &Row) -> Result<Vec<CanonicalEvent>> {
    let at = Some(Position(row.position as u64));
    row.events()
        .map(|(events, _)| events)
        .map_err(|flaw| lakelands_core::damaged(Part::Log, at, flaw))
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
    let to = from.saturating_add(i64::from(limit) - 1);
    let mut events = Vec::new();
    // The row that holds `from` is the last to begin at or before it.
    let which = format!(
        "position >= coalesce((SELECT max(position) FROM {} WHERE position <= $1), $1) \
         AND position <= $2",
        names.t("log")
    );
    rows(db, names, &which, &[&from, &to], |row| {
        let within = |e: &CanonicalEvent| (from..=to).contains(&(e.position.0 as i64));
        events.extend(events_of(&row)?.into_iter().filter(within));
        Ok(())
    })?;
    Ok(events)
}

/// The rows of the segment that begins at `first`, which the one at
/// `next` follows, if any: the condition and its parameters.
fn of_segment(first: i64, next: Option<i64>) -> (&'static str, [i64; 2]) {
    match next {
        Some(next) => ("position >= $1 AND position < $2", [first, next]),
        None => ("position >= $1 AND position <= $2", [first, i64::MAX]),
    }
}

/// The active segment's digest (`docs/log-format.md` §4.2), which begins
/// at `first`, from its rows, its events restored as they are read: what
/// a seal takes where the running one is not known.
fn digest_of(db: &mut Db, names: &Names, first: i64) -> Result<Digest> {
    let mut digesting = Digesting::default();
    let (which, [a, b]) = of_segment(first, None);
    rows(db, names, which, &[&a, &b], |row| {
        digesting.add(&events_of(&row)?);
        Ok(())
    })?;
    digesting
        .digest()
        .map_err(|(at, flaw)| lakelands_core::damaged(Part::Log, Some(at), flaw))
}

/// Close the active segment, if it holds any event, and begin the next
/// with the events `first` answers for it, appended as [`append`] does
/// with `imported`: one transaction that checks the lease row holds the
/// lease's fence, so the seal and those events are both there or neither.
/// The segment it closes and its digest are `carried`'s, and the commit
/// carries on the one it begins.
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
        let Some((to, _)) = at.tip.clone().filter(|(p, _)| p.0 as i64 >= active) else {
            return Ok((None, at));
        };
        let digest = match at.digesting.as_ref().map(Digesting::digest) {
            Some(running) => {
                running.map_err(|(at, flaw)| lakelands_core::damaged(Part::Log, Some(at), flaw))?
            }
            None => digest_of(db, names, active)?,
        };
        let sealed = SegmentDigest {
            first: Position(active as u64),
            last: to,
            digest,
        };
        let events = lakelands_core::engines::first_of_next(first, &sealed)?;
        let next = (to.0 + 1) as i64;
        db.execute(
            &format!(
                "UPDATE {s} SET digest = $2, running = NULL WHERE first = $1",
                s = names.t("segments")
            ),
            &[&active, &sealed.digest.0],
        )?;
        db.execute(
            &format!(
                "INSERT INTO {} (first, running) VALUES ($1, $2)",
                names.t("segments")
            ),
            &[&next, &Digesting::default().state()],
        )?;
        (at.segment, at.digesting) = (next, Some(Digesting::default()));
        let moved = add(db, names, lease, at, &events, imported)?;
        Ok((Some(sealed), moved))
    })?;
    *carried = Some(moved);
    Ok(sealed)
}

/// A segment as `segments` holds it: its first position, the first of the
/// one after it, none for the active one, and its digest, none until it
/// is sealed.
type Kept = (i64, Option<i64>, Option<String>);

/// Every segment, oldest first.
fn firsts(db: &mut Db, names: &Names) -> Result<Vec<Kept>> {
    let rows = db.query(
        &format!(
            "SELECT first, lead(first) OVER (ORDER BY first), digest FROM {} ORDER BY first",
            names.t("segments")
        ),
        &[],
    )?;
    Ok(rows
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect())
}

/// Every sealed segment, oldest first.
pub(crate) fn segments(db: &mut Db, names: &Names) -> Result<Vec<SegmentDigest>> {
    let mut out = Vec::new();
    for (first, next, digest) in firsts(db, names)? {
        // A seal writes a segment's digest with the next segment's row.
        let (Some(next), Some(digest)) = (next, digest) else {
            continue;
        };
        out.push(SegmentDigest {
            first: Position(first as u64),
            last: Position(next as u64 - 1),
            digest: Digest(digest),
        });
    }
    Ok(out)
}

/// Write every segment that holds an event as
/// `<dir>/log/<first position in 20 digits>.jsonl`, the files
/// `docs/log-format.md` §4.2 describes, each row's bytes and a line feed;
/// a new lake's first segment, still empty, has no file.
pub(crate) fn export(db: &mut Db, names: &Names, dir: &Path) -> Result<()> {
    let log = dir.join("log");
    std::fs::create_dir_all(&log)
        .map_err(|e| lakelands_core::io_failed(Doing::Create, Part::Export, &e))?;
    for (first, next, _) in firsts(db, names)? {
        // Written as the rows arrive, under a temporary name that becomes
        // the file's once it is whole and synced: a failed export leaves
        // no file short of its segment. A segment with no row has none.
        let path = log.join(format!("{first:020}.jsonl"));
        let partial = log.join(format!("{first:020}.jsonl.partial"));
        let failed = |e: std::io::Error| lakelands_core::io_failed(Doing::Write, Part::Export, &e);
        let mut file: Option<std::io::BufWriter<std::fs::File>> = None;
        let (which, [a, b]) = of_segment(first, next);
        rows(db, names, which, &[&a, &b], |row| {
            if file.is_none() {
                file = Some(std::io::BufWriter::new(
                    std::fs::File::create(&partial).map_err(failed)?,
                ));
            }
            let w = file.as_mut().expect("opened above");
            w.write_all(&row.bytes)
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

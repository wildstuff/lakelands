//! A large lake goes through the store in memory that does not grow with
//! it (`docs/decisions/016-two-write-doors-by-origin.md`, the change feed
//! in `docs/seams.md`,
//! `docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`).
//!
//! The work that reads or writes a whole lake has to work in pages. That
//! is ingest from a source, the feed read from the start, an export and
//! its verification, a walk of a type through `find` page by page, a
//! rebuild of the projection, and a reopen. Each may hold a window, a
//! page or a segment's worth of memory, set by the
//! lake's limits or by constants, but not more as the lake grows. One
//! check builds a lake of [`Given::large_lake`] records, runs every
//! operation on it, grows it to four times that size and runs them all
//! again.
//!
//! **What is measured.** The harness's [`Meter`] counts the process's
//! heap. For each operation the check resets the peak, runs it, and
//! takes what it *held*. For an operation that only reads the lake, that
//! is the peak minus what was live before, so a leak counts as much as a
//! hoard. Ingest is different: on an engine that keeps the lake on the
//! heap, the lake grows with it by design, so for ingest it is the peak
//! minus what was live after, what it held beyond what it leaves behind.
//!
//! **What "bounded" means.** The claim is that the heap an operation
//! holds does not depend on the lake's size, not that it stays under some
//! number of bytes: a number would describe one machine and one
//! allocator. So what it holds at four times the size may exceed what it
//! held at the first size by at most a quarter plus 1 MiB. The quarter
//! absorbs allocator rounding and the vectors a page or a window grows
//! into. The 1 MiB absorbs a small first size, where fixed costs
//! dominate. On every engine the operations held here measure the same at
//! both sizes to within 0.2 MiB, so the slack is kept small: at 4 MiB,
//! an ingest that kept every window's records and a verification that
//! kept every file it read both passed. An operation that holds the whole range it reads grows about
//! fourfold and exceeds the tolerance by far, even at the suite's small
//! sizes.
//!
//! Ingest, the feed, the export and its verification are held to this.
//! Three more are measured and reported against the same tolerance but
//! not held to it yet, each until the fix named in its line turns the
//! claim on. The rebuild and the reopen fold a range read whole
//! (lakelands#130). The walk through `find` measures each page on its
//! own, as its largest page: a page may hold its rows, not the type, but
//! today every page reads the whole type (lakelands#134). A harness
//! without a meter runs everything else the check claims and reports the
//! heap as not measured.
//!
//! **What a green run does not show.** Two limits of the measurement:
//!
//! - The 1 MiB slack hides a hoard below about 175 bytes per record at
//!   the suite's default sizes: 1 MiB over the 6 000 records between
//!   2 000 and 8 000. A harness that asks for a smaller first size widens
//!   that (about 320 bytes per record at 1 100). A hoard of the log's raw
//!   bytes, about 400 per event, is caught at either. The long run
//!   (25 000 and 100 000 records) narrows it to about 14 bytes per record.
//! - The meter counts Rust's allocator. SQLite allocates its own heap in
//!   C, beside it, so a hoard inside the SQLite engine's C code is
//!   invisible to the check. Each row reports the process's resident
//!   high-water mark (`VmHWM`) where the system gives it, for a reader to
//!   see that.
//!
//! **Without the heap.** Ingest must not read its source ahead of the
//! windows it has committed: a consumer that stops reading the answers
//! stops the source. The check pauses before reading any answer. The
//! source may by then have been read by at most sixteen windows' worth of
//! records, whatever its length.
//!
//! Each operation prints one line to standard error. The same rows are
//! appended as JSON lines to a file beside the check's scratch
//! directory, whose path is printed once.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_core::Stream;
use lakelands_api::event::{Digest, EventBody};
use lakelands_api::ontology::{Authority, StorageKind, TypeDecl};
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::{
    Find, Found, LakeStore, Position, Principal, SourceId, SourceRecord, TypeName,
};
use serde_json::json;
use sha2::{Digest as _, Sha256};

use crate::conformance::fixture::{Must, declare, doors, field, limits, owner, pause, type_decl};
use crate::conformance::{Failure, Given, Meter, backing, next};
use crate::export;

check!(
    /// A large lake goes through ingest, the feed and export in bounded memory.
    STREAMS = a_large_lake_goes_through_ingest_the_feed_and_export_in_memory_that_does_not_grow_with_it,
    "016, the change feed and 030: ingest reads its source only as windows commit, and ingest, the feed read from the start and an export with its verification hold as much memory at four times a lake's size as at its first size, give or take a quarter and 1 MiB; a walk of a type through find, a rebuild and a reopen are measured against the same bound",
    given
);

/// How much larger the lake grows for the second measurement.
const GROWTH: u32 = 4;
/// What an operation holds at the larger size may exceed the first by this
/// many percent ...
const TOLERANCE_PERCENT: u64 = 25;
/// ... plus this many bytes.
const SLACK: u64 = 1 << 20;
/// How many windows' worth of records ingest may read ahead of the
/// answers a consumer has taken.
const LEAD_WINDOWS: u64 = 16;
/// How long the check leaves an ingest's answers unread.
const UNREAD: Duration = Duration::from_millis(200);

const TY: &str = "reading";
const SOURCE: &str = "meters";

/// `reading`: a sequence number, a site, a value and a note of 200
/// characters, all from the source.
fn reading() -> TypeDecl {
    let source = |name: &str, kind| {
        let mut f = field(name, kind);
        f.authority = Authority::Source;
        f
    };
    type_decl(
        TY,
        vec![
            source("seq", StorageKind::Number),
            source("site", StorageKind::Text),
            source("value", StorageKind::Number),
            source("note", StorageKind::Text),
        ],
    )
}

/// Record `i` of the source, as first delivered.
fn content(i: u64, value: f64) -> SourceRecord {
    let word = format!("{:08x}", i.wrapping_mul(2_654_435_761) as u32);
    SourceRecord {
        ty: TypeName::from(TY),
        key: format!("r{i:09}").as_str().into(),
        fields: crate::conformance::fixture::patch(json!({
            "seq": i,
            "site": format!("site-{}", i % 97),
            "value": value,
            "note": word.repeat(25),
        })),
        seen_at: None,
    }
}

fn first_value(i: u64) -> f64 {
    ((i * 7) % 1000) as f64 / 10.0
}

/// The source of a delivery of `n` records from `from` on. It computes
/// each record when it is polled and holds none. Every twentieth record,
/// at offset 7, delivers the one before it again, a duplicate; every
/// twentieth, at offset 13, changes the value of the record ten before
/// it, a second version.
struct Readings {
    from: u64,
    at: u64,
    end: u64,
    taken: Arc<AtomicU64>,
}

impl Readings {
    fn record(&self, i: u64) -> SourceRecord {
        match (i - self.from) % 20 {
            7 => content(i - 1, first_value(i - 1)),
            13 => content(i - 10, first_value(i - 10) + 1000.0),
            _ => content(i, first_value(i)),
        }
    }
}

impl Stream for Readings {
    type Item = SourceRecord;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
        if self.at == self.end {
            return Poll::Ready(None);
        }
        let i = self.at;
        self.at += 1;
        self.taken.fetch_add(1, Ordering::Relaxed);
        Poll::Ready(Some(self.record(i)))
    }
}

/// How many of `n` records from a [`Readings`] are duplicates.
fn duplicates_in(n: u64) -> u64 {
    n / 20 + u64::from(n % 20 > 7)
}

/// How many of `n` records from a [`Readings`] change an earlier one.
fn changes_in(n: u64) -> u64 {
    n / 20 + u64::from(n % 20 > 13)
}

/// What the heap did during one operation.
#[derive(Clone, Copy, Debug)]
struct Heap {
    before: u64,
    peak: u64,
    after: u64,
    /// Whether the operation grows the lake, which an engine may keep on
    /// the heap.
    grows: bool,
}

impl Heap {
    /// What the operation held: beyond what it leaves behind if it grows
    /// the lake, beyond what was live before if it only reads it.
    fn held(&self) -> u64 {
        let base = if self.grows {
            self.before.max(self.after)
        } else {
            self.before
        };
        self.peak.saturating_sub(base)
    }
}

/// One operation being measured.
struct Measure {
    meter: Option<Arc<dyn Meter>>,
    before: u64,
    grows: bool,
    started: Instant,
}

impl Measure {
    fn start(meter: &Option<Arc<dyn Meter>>, grows: bool) -> Measure {
        let before = meter.as_ref().map_or(0, |m| {
            m.reset_peak();
            m.live()
        });
        Measure {
            meter: meter.clone(),
            before,
            grows,
            started: Instant::now(),
        }
    }

    fn stop(self) -> (Option<Heap>, Duration) {
        let elapsed = self.started.elapsed();
        let heap = self.meter.map(|m| Heap {
            before: self.before,
            peak: m.peak(),
            after: m.live(),
            grows: self.grows,
        });
        (heap, elapsed)
    }
}

/// Whether the claim is held to, or measured and reported only.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Claim {
    Held,
    /// Measured and reported until the named issue's fix holds it.
    Reported(&'static str),
}

/// One operation at one size, as reported.
struct Row {
    phase: &'static str,
    claim: Claim,
    records: u64,
    events: u64,
    what: String,
    elapsed: Duration,
    heap: Option<Heap>,
    hwm: Option<u64>,
}

/// The process's resident high-water mark, where the system tells it.
fn hwm() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

fn mib(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0))
}

/// The most an operation may hold at the larger size.
fn bound(first: u64) -> u64 {
    first + first * TOLERANCE_PERCENT / 100 + SLACK
}

/// Everything the check reads back of the lake: how many records `find`
/// answers and a hash over them in order, page by page.
async fn answers(store: &dyn LakeStore, who: &Principal) -> Result<(u64, String), Failure> {
    let cat = doors(store);
    let mut find = Find {
        with_meta: true,
        limit: Some(limits().max_limit),
        ..Find::default()
    };
    let (mut count, mut hash) = (0u64, Sha256::new());
    loop {
        let reply = cat
            .find(who, &TypeName::from(TY), find.clone())
            .await
            .must("finding every `reading`")?;
        let Found::Records { rows, next_cursor } = reply.data else {
            return Err(Failure::new(
                "finding `reading` records without an aggregate answered groups",
            ));
        };
        for row in &rows {
            count += 1;
            hash.update(serde_json::to_vec(row).expect("a record serialises"));
        }
        match next_cursor {
            Some(c) => find.cursor = Some(c),
            None => return Ok((count, hex(&hash.finalize()))),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What reading the whole log through the feed found.
struct Log {
    events: u64,
    last: Digest,
    checkpoints: u64,
    pages: u64,
}

/// One size of the lake: every operation, measured.
struct Size<'a> {
    given: &'a Given,
    own: Principal,
    rows: Vec<Row>,
    /// How many `reading` records the lake holds.
    live: u64,
}

impl Size<'_> {
    fn row(
        &mut self,
        phase: &'static str,
        claim: Claim,
        records: u64,
        events: u64,
        what: String,
        (heap, elapsed): (Option<Heap>, Duration),
    ) {
        self.rows.push(Row {
            phase,
            claim,
            records,
            events,
            what,
            elapsed,
            heap,
            hwm: hwm(),
        });
    }

    /// Deliver `n` records from `from` on and check what the windows
    /// answer and how far the source was read ahead of them.
    async fn ingest(&mut self, store: &dyn LakeStore, from: u64, n: u64) -> Result<(), Failure> {
        let cat = doors(store);
        let window = u64::from(limits().commit_window_events.max(1));
        let taken = Arc::new(AtomicU64::new(0));
        let what = format!("ingesting {n} records from {from} on");
        let m = Measure::start(&self.given.meter, true);
        let mut commits = cat
            .ingest(
                &self.own,
                &SourceId::from(SOURCE),
                Box::pin(Readings {
                    from,
                    at: from,
                    end: from + n,
                    taken: taken.clone(),
                }),
            )
            .await
            .map_err(|e| Failure::new(format!("{what} was refused: {e}")))?;
        // A consumer that reads no answer: the store may read on only
        // until its answers back up.
        pause(UNREAD).await;
        let mut lead = taken.load(Ordering::Relaxed);
        let (mut answered, mut records, mut duplicates, mut windows) = (0u64, 0u64, 0u64, 0u64);
        let mut last: Option<Position> = None;
        while let Some(w) = next(&mut commits).await {
            let w = w.must(&what)?;
            let size = u64::from(w.records) + u64::from(w.duplicates);
            ensure!(
                size <= window,
                "{what}: a window answered {size} records; a window holds at most the lake's \
                 commit_window_events, {window}"
            );
            if let Some(before) = last {
                ensure!(
                    w.position > before || (w.records == 0 && w.position == before),
                    "{what}: a window answered position {} after one that answered {before}; \
                     windows answer in log order",
                    w.position
                );
            }
            last = Some(w.position);
            answered += size;
            records += u64::from(w.records);
            duplicates += u64::from(w.duplicates);
            windows += 1;
            lead = lead.max(taken.load(Ordering::Relaxed).saturating_sub(answered));
        }
        let measured = m.stop();
        ensure!(
            records + duplicates == n && duplicates == duplicates_in(n),
            "{what}: the windows answered {records} records and {duplicates} duplicates; the \
             source delivered {n}, {} of them duplicates",
            duplicates_in(n)
        );
        ensure!(
            lead <= LEAD_WINDOWS * window,
            "{what}: the source was read {lead} records ahead of the answers taken; ingest reads \
             a source only as its windows commit, at most {LEAD_WINDOWS} windows of {window} ahead"
        );
        let head = store.head().await.must("reading the head")?;
        ensure!(
            last == Some(head),
            "{what}: the last window answered {last:?}, the head is {head}; every window is \
             visible once it answers"
        );
        self.live += n - duplicates_in(n) - changes_in(n);
        let size = from + n;
        self.row(
            "ingest",
            Claim::Held,
            size,
            head.0 + 1,
            format!("{windows} windows, {duplicates} duplicates, read ahead {lead}"),
            measured,
        );
        Ok(())
    }

    /// Read the whole log through the feed, a page at a time, keeping
    /// only a count and the last hash.
    async fn feed(&mut self, store: &dyn LakeStore, records: u64) -> Result<Log, Failure> {
        let feed = store.feed();
        let limit = limits().max_limit;
        let head = store.head().await.must("reading the head")?;
        let m = Measure::start(&self.given.meter, false);
        let mut from = Position::GENESIS;
        let mut log = Log {
            events: 0,
            last: Digest::zero(),
            checkpoints: 0,
            pages: 0,
        };
        loop {
            let batch = feed
                .poll(from, &FeedFilter::default(), limit)
                .await
                .must(&format!("polling the feed from {from}"))?;
            if batch.events.is_empty() {
                break;
            }
            ensure!(
                batch.events.len() as u64 <= u64::from(limit),
                "polling the feed from {from} with a limit of {limit} answered {} events",
                batch.events.len()
            );
            for e in &batch.events {
                ensure!(
                    e.position.0 == log.events,
                    "reading the feed from the start answered position {} where {} was next; \
                     the feed answers every position once, in order",
                    e.position,
                    log.events
                );
                log.events += 1;
                if matches!(e.body, EventBody::Checkpoint { .. }) {
                    log.checkpoints += 1;
                }
                log.last = e.hash.clone();
            }
            log.pages += 1;
            from = batch.next;
        }
        let measured = m.stop();
        ensure!(
            log.events == head.0 + 1,
            "reading the feed from the start answered {} events; the head is {head}",
            log.events
        );
        self.row(
            "feed",
            Claim::Held,
            records,
            log.events,
            format!("{} pages of at most {limit}", log.pages),
            measured,
        );
        Ok(log)
    }

    /// Export the lake and verify the files.
    async fn export(
        &mut self,
        store: &dyn LakeStore,
        records: u64,
        log: &Log,
        dir: &Path,
    ) -> Result<(), Failure> {
        let m = Measure::start(&self.given.meter, false);
        let manifest = export::write(store, dir)
            .await
            .must(&format!("exporting into {}", dir.display()))?;
        let written = m.stop();
        let m = Measure::start(&self.given.meter, false);
        let verified = export::verify(dir).must(&format!("verifying {}", dir.display()))?;
        let checked = m.stop();
        let through = Position(log.events - 1);
        ensure!(
            manifest.through == through
                && manifest.hash == log.last
                && verified.through == through
                && verified.hash == log.last,
            "the export's manifest names {} {:?} and its files verify to {} {:?}; the feed read \
             the log through {through} {:?}",
            manifest.through,
            manifest.hash,
            verified.through,
            verified.hash,
            log.last
        );
        ensure!(
            manifest.segments.len() as u64 == log.checkpoints + 1,
            "the export wrote {} files for a log with {} checkpoints; it writes one per segment",
            manifest.segments.len(),
            log.checkpoints
        );
        let bytes: u64 = fs::read_dir(dir.join("log"))
            .map_err(|e| Failure::new(format!("listing the export: {e}")))?
            .filter_map(|e| e.ok()?.metadata().ok())
            .map(|m| m.len())
            .sum();
        let what = format!("{} files, {} MiB", manifest.segments.len(), mib(bytes));
        self.row(
            "export",
            Claim::Held,
            records,
            log.events,
            what.clone(),
            written,
        );
        self.row(
            "export-verify",
            Claim::Held,
            records,
            log.events,
            what,
            checked,
        );
        let _ = fs::remove_dir_all(dir);
        Ok(())
    }

    /// Walk every `reading` through `find`, a page of `max_limit` at a
    /// time with the cursor, measuring each page on its own: a page holds
    /// its rows, whatever the size of the type.
    async fn find_walk(&mut self, store: &dyn LakeStore, records: u64) -> Result<(), Failure> {
        let cat = doors(store);
        let head = store.head().await.must("reading the head")?;
        let limit = limits().max_limit;
        let mut find = Find {
            limit: Some(limit),
            ..Find::default()
        };
        let (mut pages, mut rows) = (0u64, 0u64);
        // Filled after each page is measured, so no page pays for it.
        let mut seen = std::collections::HashSet::new();
        let mut worst: Option<Heap> = None;
        let started = Instant::now();
        loop {
            let m = Measure::start(&self.given.meter, false);
            let reply = cat
                .find(&self.own, &TypeName::from(TY), find.clone())
                .await
                .must(&format!("finding page {} of `reading`", pages + 1))?;
            let (heap, _) = m.stop();
            if let Some(h) = heap
                && worst.is_none_or(|w| h.held() > w.held())
            {
                worst = Some(h);
            }
            let Found::Records {
                rows: page,
                next_cursor,
            } = reply.data
            else {
                return Err(Failure::new(
                    "finding `reading` records without an aggregate answered groups",
                ));
            };
            ensure!(
                page.len() as u64 <= u64::from(limit),
                "a page of `find` with a limit of {limit} answered {} records",
                page.len()
            );
            for record in &page {
                ensure!(
                    seen.insert(record.key.clone()),
                    "walking `reading` through `find` page by page answered `{}` twice; a \
                     walk answers every record once",
                    record.key
                );
            }
            pages += 1;
            rows += page.len() as u64;
            match next_cursor {
                Some(c) => find.cursor = Some(c),
                None => break,
            }
        }
        ensure!(
            seen.len() as u64 == self.live,
            "walking `reading` through `find` page by page answered {rows} records, {} of \
             them distinct; the lake holds {}",
            seen.len(),
            self.live
        );
        self.row(
            "find-walk",
            Claim::Reported("lakelands#134"),
            records,
            head.0 + 1,
            format!("{pages} pages of at most {limit}; the largest page"),
            (worst, started.elapsed()),
        );
        Ok(())
    }

    /// Rebuild the projection; every answer stays as it was.
    async fn rebuild(&mut self, store: &dyn LakeStore, records: u64) -> Result<(), Failure> {
        let head = store.head().await.must("reading the head")?;
        let before = answers(store, &self.own).await?;
        ensure!(
            before.0 == self.live,
            "finding every `reading` answered {} records; the lake holds {}",
            before.0,
            self.live
        );
        let m = Measure::start(&self.given.meter, false);
        let rebuilt = store.rebuild().await.must("rebuilding the projection")?;
        let measured = m.stop();
        ensure!(
            rebuilt == head,
            "the rebuild answered {rebuilt}; the head was {head}"
        );
        let after = answers(store, &self.own).await?;
        ensure!(
            after == before,
            "after the rebuild, finding every `reading` answered {} records hashing to {}; \
             before, {} hashing to {}",
            after.0,
            after.1,
            before.0,
            before.1
        );
        self.row(
            "rebuild",
            Claim::Reported("lakelands#130"),
            records,
            head.0 + 1,
            format!("{} records read back", before.0),
            measured,
        );
        Ok(())
    }

    /// Open the lake again, as a host's next process does.
    async fn reopen(
        &mut self,
        store: Arc<dyn LakeStore>,
        records: u64,
    ) -> Result<Arc<dyn LakeStore>, Failure> {
        let head = store.head().await.must("reading the head")?;
        let before = answers(store.as_ref(), &self.own).await?;
        let seams = Seams {
            backings: backing::backings(),
            executors: backing::executors(),
            ..Seams::default()
        };
        let m = Measure::start(&self.given.meter, false);
        let store = (self.given.reopen)(store, seams)
            .await
            .map_err(|e| Failure::new(format!("opening the large lake again failed: {e}")))?;
        let measured = m.stop();
        let again = store.head().await.must("reading the head")?;
        let after = answers(store.as_ref(), &self.own).await?;
        // A new writer may record its own opening; the records stay.
        ensure!(
            again >= head && after == before,
            "opened again, the lake's head is {again} and it answers {} records hashing to {}; \
             before, {head} and {} hashing to {}",
            after.0,
            after.1,
            before.0,
            before.1
        );
        self.row(
            "reopen",
            Claim::Reported("lakelands#130"),
            records,
            head.0 + 1,
            String::new(),
            measured,
        );
        Ok(store)
    }
}

/// The report file, beside the check's scratch directory.
fn report_file(scratch: &Path) -> PathBuf {
    scratch.with_extension("jsonl")
}

async fn a_large_lake_goes_through_ingest_the_feed_and_export_in_memory_that_does_not_grow_with_it(
    store: Arc<dyn LakeStore>,
    given: Given,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(cat.as_ref(), &own, reading()).await?;
    let _ = fs::remove_dir_all(&given.scratch);
    let first = u64::from(given.large_lake.max(20));
    let mut size = Size {
        given: &given,
        own,
        rows: Vec::new(),
        live: 0,
    };
    let mut store = store;
    let mut from = 0;
    for records in [first, first * u64::from(GROWTH)] {
        size.ingest(store.as_ref(), from, records - from).await?;
        store.checkpoint().await.must("taking a checkpoint")?;
        let log = size.feed(store.as_ref(), records).await?;
        let dir = given.scratch.join(format!("export-{records}"));
        size.export(store.as_ref(), records, &log, &dir).await?;
        size.find_walk(store.as_ref(), records).await?;
        size.rebuild(store.as_ref(), records).await?;
        store = size.reopen(store, records).await?;
        from = records;
    }
    let _ = fs::remove_dir_all(&given.scratch);
    report(&given, &size.rows)
}

/// Print every row, append them to the report file, and fail for every
/// held operation whose heap grew past the bound.
fn report(given: &Given, rows: &[Row]) -> Result<(), Failure> {
    let file = report_file(&given.scratch);
    let mut out = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .ok();
    eprintln!(
        "streaming on `{}`: report rows appended to {}",
        given.harness,
        file.display()
    );
    let mut failures = Vec::new();
    for row in rows {
        let first = rows
            .iter()
            .find(|r| r.phase == row.phase)
            .filter(|r| r.records < row.records)
            .and_then(|r| r.heap);
        let against = match (first, row.heap) {
            (Some(f), Some(h)) => {
                let limit = bound(f.held());
                let within = h.held() <= limit;
                if !within && row.claim == Claim::Held {
                    failures.push(format!(
                        "{} at {} records held {} MiB of heap, against {} MiB at {} records; \
                         at most {} MiB is allowed",
                        row.phase,
                        row.records,
                        mib(h.held()),
                        mib(f.held()),
                        rows.iter()
                            .find(|r| r.phase == row.phase)
                            .map_or(0, |r| r.records),
                        mib(limit)
                    ));
                }
                let verdict = match (within, row.claim) {
                    (true, _) => "within".to_owned(),
                    (false, Claim::Held) => "OVER".to_owned(),
                    (false, Claim::Reported(until)) => {
                        format!("over, reported only until {until}")
                    }
                };
                Some((limit, verdict))
            }
            _ => None,
        };
        let secs = row.elapsed.as_secs_f64();
        let rate = row.events as f64 / secs.max(1e-9);
        let heap = match row.heap {
            Some(h) => format!(
                "heap {}→{}→{} MiB, held {} MiB",
                mib(h.before),
                mib(h.peak),
                mib(h.after),
                mib(h.held())
            ),
            None => "heap not measured".to_owned(),
        };
        let bound_text = against.as_ref().map_or(String::new(), |(limit, verdict)| {
            format!(", bound {} MiB: {verdict}", mib(*limit))
        });
        eprintln!(
            "streaming on `{}`: {:<13} {:>8} records {:>8} events {:>8.2} s {:>9.0} events/s  \
             {heap}{bound_text}, VmHWM {}  {}",
            given.harness,
            row.phase,
            row.records,
            row.events,
            secs,
            rate,
            row.hwm
                .map_or("not measured".to_owned(), |b| format!("{} MiB", mib(b))),
            row.what
        );
        if let Some(out) = out.as_mut() {
            let line = json!({
                "engine": given.harness,
                "phase": row.phase,
                "held": row.claim == Claim::Held,
                "records": row.records,
                "events": row.events,
                "seconds": secs,
                "heap": row.heap.map(|h| json!({
                    "before": h.before,
                    "peak": h.peak,
                    "after": h.after,
                    "held": h.held(),
                })),
                "bound": against.as_ref().map(|(limit, _)| *limit),
                "verdict": against.as_ref().map(|(_, v)| v.clone()),
                "vm_hwm": row.hwm,
                "what": row.what,
            });
            let _ = writeln!(out, "{line}");
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Failure::new(format!(
            "an operation over the whole lake held more memory as the lake grew: {}",
            failures.join("; ")
        )))
    }
}

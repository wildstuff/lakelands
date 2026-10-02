//! A large lake goes through the store in memory that does not grow with
//! it (`docs/decisions/016-two-write-doors-by-origin.md`, the change feed
//! in `docs/seams.md`,
//! `docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`).
//!
//! The work that reads or writes a whole lake has to work in pages. That
//! is ingest from a source, the feed read from the start, an export, its
//! verification and its import as a new lake (lakelands#215; on the
//! memory engine the imported lake stays on the heap, so, as for ingest,
//! what the import held beyond what it leaves), a walk of a type through
//! `find` page by page, a
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
//! Ingest, the feed, the export and its verification are held to this,
//! and so are the walk through `find`, which measures each page on its
//! own, as its largest page: a page holds its rows, not the type
//! (lakelands#134), and the first pages ordered by a field the lake's
//! `sort_index.<type>` setting names, which read the order index and not
//! the type (lakelands#137), and the rebuild, which folds the log a page
//! at a time (lakelands#130), and the reopen, which reads the log a page at a time
//! and on SQLite keeps a mark per 256 events of its index, not the index
//! (lakelands#133), and a walk over link records: a pattern query of
//! three steps walking a relation carried by link records backwards from
//! one start. Every record from the third on links to two earlier ones by
//! a fixed rule, so the lake holds about twice as many links as records,
//! and the walk answers the same rows at every size: what it holds
//! depends on what it reaches, not on the lake. Its time is reported
//! only; on the memory engine and on PostgreSQL it still grows with the
//! links (lakelands#190). And one hybrid search per size, k = 10, for a
//! fixed text through the suite's embedding backend: every record holds a
//! vector, so the vector side scans the whole field, and each engine keeps
//! only the best candidates as it goes, not the field (lakelands#197); then
//! the same search with the lake asking for an index on the field, which
//! PostgreSQL answers from pgvector's index and the other engines by the
//! same scan (`docs/decisions/043-an-index-asked-for-answers-its-set.md`).
//! And the whole-graph measures over `follows` (lakelands#149): the ten
//! records of highest degree over the whole type, which read the seeds a
//! page at a time and keep the best rows, held; the components of the
//! whole type, which hold every record they reach and so grow with the
//! graph, reported; and the same components with the lake's
//! `max_graph_nodes` at half the first size, refused at the cap at every
//! size, so held by the cap. A
//! harness without a meter runs everything else the check claims and
//! reports the heap as not measured.
//!
//! **A reported line.** One more operation is measured against the same
//! bound and reported, but never fails the check: the same first pages
//! ordered by a field, before the setting asks for its index: a page then
//! sorts the type, which is that order's stated ceiling (lakelands#193).
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
use lakelands_api::ontology::{
    Authority, Cardinality, EmbedDecl, FieldDecl, FieldType, RelationDecl, StorageKind, TypeDecl,
};
use lakelands_api::query::{
    Depth, Pattern, Projection, QueryResult, Search, SearchMode, Step, Walk,
};
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::{
    Find, Found, LakeStore, Position, Principal, ReadAt, RecordKey, SourceId, SourceRecord,
    TypeName,
};
use serde_json::json;
use sha2::{Digest as _, Sha256};

use crate::conformance::backing::EMBED_MODEL;
use crate::conformance::fixture::{
    Must, declare, doors, field, limits, owner, pause, reason, type_decl,
};
use crate::conformance::{Failure, Given, Meter, backing, next};
use crate::export;
use crate::links::{link_key, link_type_name};

check!(
    /// A large lake goes through ingest, the feed and export in bounded memory.
    STREAMS = a_large_lake_goes_through_ingest_the_feed_and_export_in_memory_that_does_not_grow_with_it,
    "016, the change feed and 030: ingest reads its source only as windows commit, and ingest, the feed read from the start, an export with its verification and its import into a new lake, each page of a walk of a type through find, a rebuild, a reopen, a three-hop walk over link records, a three-hop walk over a derived relation, a hybrid search with k = 10 over every record's vector, without and with a vector index asked for, the ten records of highest degree over the whole type, and components refused at the lake's max_graph_nodes, hold as much memory at four times a lake's size as at its first size, give or take a quarter and 1 MiB, and the search answers what it must",
    given,
    embedding
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
/// The relation the walk follows, and the source its links arrive from.
const FOLLOWS: &str = "follows";
/// The derived relation from a record to the records holding its note:
/// itself alone, since every note is its own (044).
const SAME_NOTE: &str = "same_note";
const LINKS: &str = "chains";
/// The record whose note the search asks for, and the one the walk
/// starts from.
const SOUGHT: u64 = 42;
const WALK_FROM: u64 = 1;
/// How many hops the walk takes, one step each.
const HOPS: usize = 3;
/// How many hits the search asks for.
const K: u32 = 10;

/// `reading`: a sequence number, a site, a value and a note of 200
/// characters, all from the source; the note is searchable, and `vec`
/// embeds the site through the suite's backend. The 97 sites are 97
/// texts, so the backend is called 97 times whatever the lake's size,
/// and every record holds a vector. `follows` links a record to two
/// earlier ones, by link records that arrive from a source.
fn reading() -> TypeDecl {
    let source = |name: &str, kind| {
        let mut f = field(name, kind);
        f.authority = Authority::Source;
        f
    };
    let mut ty = type_decl(
        TY,
        vec![
            source("seq", StorageKind::Number),
            source("site", StorageKind::Text),
            source("value", StorageKind::Number),
            FieldDecl {
                searchable: true,
                ..source("note", StorageKind::Text)
            },
            FieldDecl {
                ty: FieldType {
                    dimensions: Some(2),
                    ..FieldType::kind(StorageKind::Vector)
                },
                authority: Authority::Derived,
                embed: Some(EmbedDecl {
                    from: vec!["site".into()],
                    model: EMBED_MODEL.into(),
                }),
                ..field("vec", StorageKind::Text)
            },
        ],
    );
    ty.relations = vec![RelationDecl {
        name: FOLLOWS.into(),
        target: TY.into(),
        cardinality: Cardinality::ManyToMany,
        authority: Authority::Source,
        ..Default::default()
    }];
    ty.relations.push(RelationDecl {
        derived: Some(lakelands_api::ontology::Derived {
            pairs: vec![lakelands_api::ontology::MatchPair {
                from: "note".into(),
                to: "note".into(),
            }],
        }),
        ..RelationDecl::to(SAME_NOTE, TY)
    });
    ty
}

fn key(i: u64) -> RecordKey {
    format!("r{i:09}").as_str().into()
}

/// The note of record `i`: one word, its own.
fn note(i: u64) -> String {
    format!("{:08x}", i.wrapping_mul(2_654_435_761) as u32).repeat(25)
}

/// Record `i` of the source, as first delivered.
fn content(i: u64, value: f64) -> SourceRecord {
    SourceRecord {
        upstream: None,
        ty: TypeName::from(TY),
        key: key(i),
        fields: crate::conformance::fixture::patch(json!({
            "seq": i,
            "site": format!("site-{}", i % 97),
            "value": value,
            "note": note(i),
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

/// Whether record `i` was delivered by the lake of `first` records and
/// its growth: the records at offsets 7 and 13 of every twenty of a
/// delivery never are.
fn delivered(i: u64, first: u64) -> bool {
    let from = if i < first { 0 } else { first };
    !matches!((i - from) % 20, 7 | 13)
}

/// The records `i` follows: `i / 2 - 1` and `i / 2`, from record 2 on,
/// where both ends were delivered. Each record reached walking `follows`
/// backwards has four followers, `2x` to `2x + 3`, so a walk from one
/// start reaches the same records at every size of the lake.
fn follows(i: u64, first: u64) -> impl Iterator<Item = u64> {
    let targets = if i >= 2 && delivered(i, first) {
        vec![i / 2 - 1, i / 2]
    } else {
        Vec::new()
    };
    targets.into_iter().filter(move |t| delivered(*t, first))
}

/// A whole-graph measure of every record over `follows`, both ways.
fn measure(project: Projection) -> Pattern {
    let mut p = Pattern::traverse(
        TypeName::from(TY),
        key(0),
        vec![Step {
            relation: FOLLOWS.into(),
            walk: Walk::Both,
            ty: None,
            filter: None,
            depth: Depth::default(),
        }],
    );
    p.start.key = None;
    p.project = project;
    p
}

/// The records of the largest component over `follows` in a lake of
/// `size` records, by a union-find over the rule.
fn largest_component(size: u64, first: u64) -> u64 {
    fn root(parent: &mut [u64], mut i: u64) -> u64 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }
    let mut parent: Vec<u64> = (0..size).collect();
    for i in 0..size {
        for t in follows(i, first) {
            let (a, b) = (root(&mut parent, i), root(&mut parent, t));
            parent[a as usize] = b;
        }
    }
    let mut count = std::collections::BTreeMap::<u64, u64>::new();
    for i in (0..size).filter(|i| delivered(*i, first)) {
        *count.entry(root(&mut parent, i)).or_default() += 1;
    }
    count.into_values().max().unwrap_or(0)
}

/// The link records of `follows` from records `from` to `end`, computed
/// as they are polled, like [`Readings`].
struct Links {
    first: u64,
    at: u64,
    end: u64,
    ahead: Vec<SourceRecord>,
}

impl Stream for Links {
    type Item = SourceRecord;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
        let link = link_type_name(&TY.into(), &FOLLOWS.into());
        while self.ahead.is_empty() && self.at < self.end {
            let i = self.at;
            self.at += 1;
            let first = self.first;
            self.ahead = follows(i, first)
                .map(|t| SourceRecord {
                    upstream: None,
                    ty: link.clone(),
                    key: link_key(&key(i), &key(t)),
                    fields: crate::conformance::fixture::patch(json!({
                        "from": key(i).as_str(),
                        "to": key(t).as_str(),
                    })),
                    seen_at: None,
                })
                .collect();
        }
        Poll::Ready(self.ahead.pop())
    }
}

/// How many rows the walk of [`HOPS`] steps backwards over `follows`
/// from [`WALK_FROM`] answers in a lake of `size` records: one per path.
fn walk_rows(size: u64, first: u64) -> u64 {
    // The end of every path so far, once per path.
    let mut ends = vec![WALK_FROM];
    for _ in 0..HOPS {
        ends = ends
            .iter()
            .flat_map(|&x| (2 * x..2 * x + 4).map(move |y| (x, y)))
            .filter(|&(x, y)| y < size && follows(y, first).any(|t| t == x))
            .map(|(_, y)| y)
            .collect();
    }
    ends.len() as u64
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
    /// Measured against the same bound and reported, never failed, until
    /// the index or the fix that earns the claim holds it; names the
    /// issue that measures it.
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
    /// How many `follows` links it holds.
    links: u64,
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
            what.clone(),
            checked,
        );
        // The export imported as a new lake of the engine (lakelands#215):
        // on the memory engine the imported lake stays on the heap by
        // design, so, as for ingest, what it held beyond what it leaves.
        let seams = lakelands_api::seam::Seams {
            backings: crate::conformance::backing::backings(),
            embed: Some(crate::conformance::backing::embedder()),
            executors: crate::conformance::backing::executors(),
            ..lakelands_api::seam::Seams::default()
        };
        let m = Measure::start(&self.given.meter, true);
        let imported = (self.given.import)(dir.to_path_buf(), store.lake().clone(), None, seams)
            .await
            .map_err(|e| Failure::new(format!("importing {}: {e}", dir.display())))?
            .store;
        let measured = m.stop();
        let head = imported
            .head()
            .await
            .must("reading the imported lake's head")?;
        ensure!(
            head == through,
            "the lake imported from the export has its head at {head}; the export's is {through}"
        );
        imported.close().await.must("closing the imported lake")?;
        self.row("import", Claim::Held, records, log.events, what, measured);
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
            Claim::Held,
            records,
            head.0 + 1,
            format!("{pages} pages of at most {limit}; the largest page"),
            (worst, started.elapsed()),
        );
        Ok(())
    }

    /// Walk the first pages of `reading` ordered by its `note`, a text field
    /// every record holds with its own value, first as a lake without an
    /// order index sorts them (reported: a page holds the type,
    /// lakelands#193), then after the setting `sort_index.reading` asks for
    /// the index on `note` (held: a page holds its rows, lakelands#137);
    /// then retire the setting.
    async fn sorted_walks(&mut self, store: &dyn LakeStore, records: u64) -> Result<(), Failure> {
        self.sorted_walk(
            store,
            records,
            "find-sorted-fallback",
            Claim::Reported("lakelands#193"),
        )
        .await?;
        doors(store)
            .upsert(
                &self.own,
                &TypeName::from(lakelands_api::seam::SETTING_TYPE),
                &RecordKey::from("sort_index.reading"),
                [
                    ("key".into(), json!("sort_index.reading")),
                    ("value".into(), json!(["note"])),
                ]
                .into(),
                lakelands_api::WriteOpts::default(),
            )
            .await
            .must("asking for the order index on `note`")?;
        self.sorted_walk(store, records, "find-sorted", Claim::Held)
            .await?;
        // Retired, so the next size's first walk sorts again.
        doors(store)
            .retire(
                &self.own,
                &TypeName::from(lakelands_api::seam::SETTING_TYPE),
                &RecordKey::from("sort_index.reading"),
                lakelands_api::WriteOpts::default(),
            )
            .await
            .must("retiring the order index on `note`")?;
        Ok(())
    }

    /// Walk the first pages of `reading` ordered by its `note`, a page of
    /// `max_limit` at a time with the cursor, measuring each page on its
    /// own, as `phase`.
    async fn sorted_walk(
        &mut self,
        store: &dyn LakeStore,
        records: u64,
        phase: &'static str,
        claim: Claim,
    ) -> Result<(), Failure> {
        const PAGES: u64 = 5;
        let cat = doors(store);
        let head = store.head().await.must("reading the head")?;
        let limit = limits().max_limit;
        let mut find = Find {
            limit: Some(limit),
            order_by: vec![Order {
                field: "note".into(),
                by: OrderBy::Value,
                dir: Direction::Asc,
            }],
            ..Find::default()
        };
        let mut previous: Option<String> = None;
        let mut worst: Option<Heap> = None;
        let started = Instant::now();
        for page in 1..=PAGES {
            let m = Measure::start(&self.given.meter, false);
            let reply = cat
                .find(&self.own, &TypeName::from(TY), find.clone())
                .await
                .must(&format!("finding page {page} of `reading` by `note`"))?;
            let (heap, _) = m.stop();
            if let Some(h) = heap
                && worst.is_none_or(|w| h.held() > w.held())
            {
                worst = Some(h);
            }
            let Found::Records { rows, next_cursor } = reply.data else {
                return Err(Failure::new(
                    "finding `reading` records without an aggregate answered groups",
                ));
            };
            for r in &rows {
                let note = r
                    .fields
                    .get("note")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
                ensure!(
                    previous <= note,
                    "walking `reading` by `note` answered {note:?} after {previous:?}"
                );
                previous = note;
            }
            match next_cursor {
                Some(c) => find.cursor = Some(c),
                None => break,
            }
        }
        self.row(
            phase,
            claim,
            records,
            head.0 + 1,
            format!("{PAGES} pages of at most {limit} by `note`; the largest page"),
            (worst, started.elapsed()),
        );
        Ok(())
    }

    /// Link every record delivered from `from` to `size` to the two it
    /// [`follows`], through a source; not measured.
    async fn link(&mut self, store: &dyn LakeStore, from: u64, size: u64) -> Result<(), Failure> {
        let first = u64::from(self.given.large_lake.max(20));
        let want: u64 = (from..size).map(|i| follows(i, first).count() as u64).sum();
        let what = format!("ingesting the `{FOLLOWS}` links of records {from} to {size}");
        let mut commits = doors(store)
            .ingest(
                &self.own,
                &SourceId::from(LINKS),
                Box::pin(Links {
                    first,
                    at: from,
                    end: size,
                    ahead: Vec::new(),
                }),
            )
            .await
            .map_err(|e| Failure::new(format!("{what} was refused: {e}")))?;
        let mut got = 0u64;
        while let Some(w) = next(&mut commits).await {
            got += u64::from(w.must(&what)?.records);
        }
        ensure!(
            got == want,
            "{what}: the windows answered {got} links; the source delivered {want}"
        );
        self.links += got;
        Ok(())
    }

    /// One hybrid search for the note of [`SOUGHT`]: the backend embeds
    /// the text, the lexical side finds the one record holding it, and
    /// the vector side ranks every vector of `vec`. Then the same search
    /// once the setting `vector_index.reading` asks for an index on `vec`,
    /// through the index where the engine keeps one (lakelands#197); the
    /// setting is retired again, so the lake grows without the index.
    async fn searches(&mut self, store: &dyn LakeStore, records: u64) -> Result<(), Failure> {
        self.search(store, records, "search", "hybrid").await?;
        self.ask_vector_index(store, json!(["vec"]))
            .await
            .must("asking for the vector index on `vec`")?;
        self.search(store, records, "search-indexed", "hybrid (index)")
            .await?;
        self.ask_vector_index(store, json!([]))
            .await
            .must("asking for no vector index again")?;
        Ok(())
    }

    /// Write the setting `vector_index.reading` with `value`.
    async fn ask_vector_index(
        &self,
        store: &dyn LakeStore,
        value: serde_json::Value,
    ) -> lakelands_api::Result<lakelands_api::Outcome> {
        doors(store)
            .upsert(
                &self.own,
                &TypeName::from(lakelands_api::seam::SETTING_TYPE),
                &RecordKey::from("vector_index.reading"),
                [
                    ("key".into(), json!("vector_index.reading")),
                    ("value".into(), value),
                ]
                .into(),
                lakelands_api::WriteOpts::default(),
            )
            .await
    }

    /// One hybrid search for the note of [`SOUGHT`], reported as `phase`;
    /// it must say it ranked by `ranked_by`.
    async fn search(
        &mut self,
        store: &dyn LakeStore,
        records: u64,
        phase: &'static str,
        ranked_by: &str,
    ) -> Result<(), Failure> {
        let head = store.head().await.must("reading the head")?;
        let sought = key(SOUGHT);
        let m = Measure::start(&self.given.meter, false);
        let got = doors(store)
            .search(
                &self.own,
                Search {
                    ty: TypeName::from(TY),
                    mode: SearchMode::Hybrid {
                        text: note(SOUGHT),
                        field: "vec".into(),
                        vector: None,
                        fields: None,
                    },
                    k: K,
                    cursor: None,
                    filter: None,
                    with_meta: false,
                    at: ReadAt::default(),
                },
            )
            .await
            .must(&format!("a hybrid search for the note of `{sought}`"))?
            .data;
        let measured = m.stop();
        let keys: Vec<&str> = got.hits.iter().map(|h| h.record.key.as_str()).collect();
        ensure!(
            got.ranked_by == ranked_by
                && got.hits.len() == K as usize
                && keys.contains(&sought.as_str()),
            "a hybrid search with k = {K} for the note only `{sought}` holds, with the suite's \
             backend bound, must say it ranked by {ranked_by:?}, answer {K} hits and find \
             `{sought}`; it ranked by {:?} and answered {keys:?}",
            got.ranked_by
        );
        if ranked_by == "hybrid" {
            ensure!(
                keys.first() == Some(&sought.as_str()),
                "the exact hybrid search must put `{sought}` first; it answered {keys:?}"
            );
        }
        self.row(
            phase,
            Claim::Held,
            records,
            head.0 + 1,
            format!("{ranked_by}, k = {K}, over {} vectors", self.live),
            measured,
        );
        Ok(())
    }

    /// Walk `follows` backwards from [`WALK_FROM`], one step per hop, as
    /// rows, page by page, measured as one query.
    async fn traverse(&mut self, store: &dyn LakeStore, records: u64) -> Result<(), Failure> {
        let head = store.head().await.must("reading the head")?;
        let first = u64::from(self.given.large_lake.max(20));
        let want = walk_rows(records, first);
        let step = || Step {
            relation: FOLLOWS.into(),
            walk: Walk::In,
            ty: None,
            filter: None,
            depth: Depth { min: 1, max: 1 },
        };
        let mut pattern = Pattern::traverse(
            TypeName::from(TY),
            key(WALK_FROM),
            (0..HOPS).map(|_| step()).collect(),
        );
        pattern.limit = Some(limits().max_limit);
        let cat = doors(store);
        let (mut rows, mut pages) = (0u64, 0u64);
        let m = Measure::start(&self.given.meter, false);
        loop {
            let reply = cat.query(&self.own, pattern.clone()).await.must(&format!(
                "walking `{FOLLOWS}` back {HOPS} hops, page {}",
                pages + 1
            ))?;
            let QueryResult::Rows {
                rows: page,
                next_cursor,
            } = reply.data
            else {
                return Err(Failure::new("a rows projection answered something else"));
            };
            rows += page.len() as u64;
            pages += 1;
            match next_cursor {
                Some(c) => pattern.cursor = Some(c),
                None => break,
            }
        }
        let measured = m.stop();
        ensure!(
            rows == want,
            "walking `{FOLLOWS}` back {HOPS} hops from `{}` answered {rows} rows; the links \
             make {want} paths",
            key(WALK_FROM)
        );
        self.row(
            "traverse",
            Claim::Held,
            records,
            head.0 + 1,
            format!(
                "{HOPS} hops, {rows} rows in {pages} pages, over {} links",
                self.links
            ),
            measured,
        );
        Ok(())
    }

    /// Walk the derived `same_note` out of [`WALK_FROM`], [`HOPS`] hops:
    /// each hop is one lookup of a note at the head, so the walk holds a
    /// record whatever the lake's size, never the type (044).
    async fn derived_walk(&mut self, store: &dyn LakeStore, records: u64) -> Result<(), Failure> {
        let head = store.head().await.must("reading the head")?;
        let step = || Step {
            relation: SAME_NOTE.into(),
            walk: Walk::Out,
            ty: None,
            filter: None,
            depth: Depth { min: 1, max: 1 },
        };
        let pattern = Pattern::traverse(
            TypeName::from(TY),
            key(WALK_FROM),
            (0..HOPS).map(|_| step()).collect(),
        );
        let m = Measure::start(&self.given.meter, false);
        let reply = doors(store)
            .query(&self.own, pattern)
            .await
            .must(&format!("walking `{SAME_NOTE}` {HOPS} hops"))?;
        let measured = m.stop();
        let QueryResult::Rows { rows, .. } = reply.data else {
            return Err(Failure::new("a rows projection answered something else"));
        };
        ensure!(
            rows.len() == 1,
            "walking `{SAME_NOTE}` {HOPS} hops from `{}` answered {} rows; every note is its \
             own, so the one path is the record itself",
            key(WALK_FROM),
            rows.len()
        );
        self.row(
            "derived-walk",
            Claim::Held,
            records,
            head.0 + 1,
            format!("{HOPS} hops over a derived relation, one note lookup each"),
            measured,
        );
        Ok(())
    }

    /// The ten records of highest degree over `follows`, both ways, over
    /// the whole type: the seeds are read a page at a time and the best
    /// rows kept, so the measure holds a page, not the type.
    async fn degree(&mut self, store: &dyn LakeStore, records: u64) -> Result<(), Failure> {
        let head = store.head().await.must("reading the head")?;
        let first = u64::from(self.given.large_lake.max(20));
        let mut degree: std::collections::BTreeMap<u64, u64> = (0..records)
            .filter(|i| delivered(*i, first))
            .map(|i| (i, 0))
            .collect();
        for i in 0..records {
            for t in follows(i, first) {
                *degree.entry(i).or_default() += 1;
                *degree.entry(t).or_default() += 1;
            }
        }
        let mut ranked: Vec<(u64, u64)> = degree.into_iter().map(|(i, d)| (d, i)).collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let want: Vec<String> = ranked
            .iter()
            .take(10)
            .map(|(d, i)| format!("{}={d}", key(*i)))
            .collect();
        let mut pattern = measure(Projection::Degree);
        pattern.limit = Some(10);
        let m = Measure::start(&self.given.meter, false);
        let reply = doors(store)
            .query(&self.own, pattern)
            .await
            .must(&format!("the degree of every `{TY}` over `{FOLLOWS}`"))?;
        let measured = m.stop();
        let QueryResult::Rows { rows, .. } = reply.data else {
            return Err(Failure::new("a degree answered something other than rows"));
        };
        let got: Vec<String> = rows
            .iter()
            .map(|r| format!("{}={}", r["key"].as_str().unwrap_or_default(), r["degree"]))
            .collect();
        ensure!(
            got == want,
            "the ten records of highest degree over `{FOLLOWS}` are {want:?}; the measure \
             answered {got:?}"
        );
        self.row(
            "degree",
            Claim::Held,
            records,
            head.0 + 1,
            format!("top 10 of {} records, over {} links", self.live, self.links),
            measured,
        );
        Ok(())
    }

    /// The components of the whole type over `follows`: they hold every
    /// record they reach, so they grow with the graph by definition, and
    /// are reported; their bound is the lake's `max_graph_nodes`, which
    /// the next line holds.
    async fn components(&mut self, store: &dyn LakeStore, records: u64) -> Result<(), Failure> {
        let head = store.head().await.must("reading the head")?;
        let largest = largest_component(records, u64::from(self.given.large_lake.max(20)));
        let mut pattern = measure(Projection::Components);
        pattern.limit = Some(1);
        let m = Measure::start(&self.given.meter, false);
        let reply = doors(store)
            .query(&self.own, pattern)
            .await
            .must(&format!("the components of every `{TY}` over `{FOLLOWS}`"))?;
        let measured = m.stop();
        let QueryResult::Rows { rows, .. } = reply.data else {
            return Err(Failure::new(
                "components answered something other than rows",
            ));
        };
        let size = rows.first().and_then(|r| r["size"].as_u64());
        ensure!(
            size == Some(largest),
            "the largest component over `{FOLLOWS}` holds {largest} records; the measure \
             answered {size:?}"
        );
        self.row(
            "components",
            Claim::Reported("lakelands#149"),
            records,
            head.0 + 1,
            format!("largest {largest} of {} records", self.live),
            measured,
        );
        Ok(())
    }

    /// The same components with the lake's `max_graph_nodes` at half the
    /// first size: refused as soon as the walk holds one record more than
    /// the cap, at every size, so what it holds is bounded by the cap and
    /// not by the lake. Answers the lake opened with that cap.
    async fn components_capped(
        &mut self,
        store: Arc<dyn LakeStore>,
        records: u64,
    ) -> Result<Arc<dyn LakeStore>, Failure> {
        let cap = self.given.large_lake.max(20) / 2;
        let capped = lakelands_api::location::Limits {
            max_graph_nodes: cap,
            ..limits()
        };
        let store = (self.given.relimit)(store, Seams::default(), capped)
            .await
            .map_err(|e| {
                Failure::new(format!("opening the lake with a cap of {cap} failed: {e}"))
            })?;
        let head = store.head().await.must("reading the head")?;
        let m = Measure::start(&self.given.meter, false);
        let answer = doors(store.as_ref())
            .query(&self.own, measure(Projection::Components))
            .await;
        let measured = m.stop();
        ensure!(
            matches!(reason(&answer), Some(lakelands_api::Refusal::TooLarge { max }) if *max == cap),
            "the components of {} records under a cap of {cap} are refused as too_large; \
             the measure answered {answer:?}",
            self.live
        );
        self.row(
            "components-capped",
            Claim::Held,
            records,
            head.0 + 1,
            format!("refused at a cap of {cap}"),
            measured,
        );
        Ok(store)
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
            Claim::Held,
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
            embed: Some(backing::embedder()),
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
            Claim::Held,
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
        links: 0,
    };
    let mut store = store;
    let mut from = 0;
    for records in [first, first * u64::from(GROWTH)] {
        size.ingest(store.as_ref(), from, records - from).await?;
        size.link(store.as_ref(), from, records).await?;
        store.checkpoint().await.must("taking a checkpoint")?;
        let log = size.feed(store.as_ref(), records).await?;
        let dir = given.scratch.join(format!("export-{records}"));
        size.export(store.as_ref(), records, &log, &dir).await?;
        size.find_walk(store.as_ref(), records).await?;
        size.sorted_walks(store.as_ref(), records).await?;
        size.searches(store.as_ref(), records).await?;
        size.traverse(store.as_ref(), records).await?;
        size.derived_walk(store.as_ref(), records).await?;
        size.degree(store.as_ref(), records).await?;
        size.components(store.as_ref(), records).await?;
        store = size.components_capped(store, records).await?;
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
                    (false, Claim::Reported(by)) => format!("over, reported only ({by})"),
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
                "reported_by": match row.claim {
                    Claim::Held => None,
                    Claim::Reported(by) => Some(by),
                },
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

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
//! dominate. On memory and SQLite the operations held here measure the
//! same at both sizes to within 0.3 MiB, so the slack is kept small: at 4 MiB,
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
//! the type (lakelands#137), and the same pages with writes between them,
//! to another type and then to records the later pages answer, which the
//! cursor keeps out of the walk: a page at its earlier position reads the
//! head's index and the keys changed since (lakelands#382). Each of these
//! pages is also held to the rows the engine reads for it, counted by
//! [`crate::engines::rows_read_in_process`]: its page, the one row more the
//! store asks for, three more, two per key changed since, and the lake's
//! settings, which the store reads by key for every read. And the rebuild, which folds the log a page
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
//! size, so held by the cap. And every record grouped by its site, by
//! `find`'s aggregate and by a view that is then counted: a group keeps
//! what its measures fold, never its records, so each holds a batch and
//! the groups (lakelands#388). And the owner's first page of a kept view
//! in key order while one group's whole face differs from its open one,
//! which holds the page and that group, not the view (lakelands#628), and
//! a sum over the view and a pattern started on one of its groups, which
//! hold a page of it (lakelands#731). The view, a group per record, is fed
//! before the feed and the export, so each size's import and rebuild fold a
//! view as large: its fold holds what a page of the log touches. On SQLite
//! import and rebuild hold 12.2 MiB at both sizes at a page of 1 024 events
//! and 6.1 MiB at 512; import held 5.7 MiB at the first size with no kept
//! view. On PostgreSQL import holds 17.2 and 18.4 MiB at 1 100 and 4 400
//! records: the driver keeps buffers sized to the largest batch it sent, so
//! the import bounds a batch in bytes (lakelands#1241). A harness without a
//! meter runs everything else the check claims and reports the heap as not
//! measured.
//!
//! **A reported line.** One more operation is measured against the same
//! bound and reported, but never fails the check: the same first pages
//! ordered by a field, before the setting asks for its index: a page then
//! sorts the type, which is that order's stated ceiling (lakelands#193).
//! The rows it reads are held, to the type and one page: a page sorts the
//! type once. Every walk ordered by `note` answers the keys this one does.
//!
//! **What a green run does not show.** Two limits of the measurement:
//!
//! - The 1 MiB slack hides a hoard below about 87 bytes per record at
//!   the suite's default sizes: 1 MiB over the 12 000 records added
//!   between 2 000 and 8 000, a reading and a member of the kept view for
//!   each. A harness that asks for a smaller first size widens that (about
//!   160 bytes per record at 1 100). A hoard of the log's raw bytes, about
//!   400 per event, is caught at either. The long run (25 000 and 100 000)
//!   narrows it to about 7 bytes per record.
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
use lakelands_api::event::{Digest, Event, EventBody};
use lakelands_api::ontology::{
    Authority, Cardinality, FieldDecl, RelationDecl, StorageKind, TypeDecl,
};
use lakelands_api::query::{Pattern, Projection, QueryResult, Search, SearchMode, Step, Walk};
use lakelands_api::read::{AggFunc, Direction, OrderBy};
use lakelands_api::seam::{FEED_EXAMINED, FeedFilter, SETTING_TYPE, Seams, VIEW_KEPT_SETTING};
use lakelands_api::{
    Find, Found, LakeStore, Position, Principal, ReadAt, RecordKey, SourceId, SourceRecord,
    TypeName,
};
use serde_json::json;
use sha2::{Digest as _, Sha256};

use crate::conformance::backing::EMBED_MODEL;
use crate::conformance::fixture::{
    Lake, Must, agg, aggregate, declare, doors, embedding, field, ingest, limits, order, owner,
    pattern, pause, reason, restrict, source_record, sourced, step, type_decl, view_of,
};
use crate::conformance::{Failure, Given, Held, Meter, Store, backing, next};
use crate::export;
use crate::links::{link_key, link_type_name};

check!(
    STREAMS =
        a_large_lake_goes_through_ingest_the_feed_and_export_in_memory_that_does_not_grow_with_it,
    "016, the change feed and 030: ingest reads its source only as windows commit, and ingest, the feed read from the start, an export with its verification and its import into a new lake, each page of a walk of a type through find, the first pages of a walk ordered by an indexed field, with and without writes between them, each also reading its page and a few rows more, not the type, a rebuild, a reopen, a three-hop walk over link records, a three-hop walk over a derived relation, the owner's page of a kept view, in key order, while one group's whole face differs from its open one (lakelands#628), and its sum over every group and a pattern started on one group (lakelands#731), a hybrid search with k = 10 over every record's vector, without and with a vector index asked for, the ten records of highest degree over the whole type, and components refused at the lake's max_graph_nodes, hold as much memory at four times a lake's size as at its first size, give or take a quarter and 1 MiB, and the search answers what it must; and (lakelands#678) a filtered poll reads only what the projection's index names: one whose filter matches nothing answers none and next past the head in one poll, one record's history from 0 answers its events in one poll, the checkpoints are each answered once in at most one poll more than there are, and a poll with no filter examines at most FEED_EXAMINED events whatever its limit",
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

/// [`HOPS`] steps over `relation`, each binding its record in a row under
/// a name of its own, `hop0` on (018, extended).
fn hops(relation: &str, walk: Walk) -> Vec<Step> {
    (0..HOPS)
        .map(|i| Step {
            named: Some(format!("hop{i}")),
            ..step(relation, walk)
        })
        .collect()
}
const LINKS: &str = "chains";
/// The view of every `reading` by its site, and what it and `find`'s
/// aggregate measure of each site: every aggregate there is.
const BY_SITE: &str = "by_site";
const MEASURES: [(&str, AggFunc, &str); 5] = [
    ("n", AggFunc::Count, "count()"),
    ("total", AggFunc::Sum, "sum(value)"),
    ("mean", AggFunc::Avg, "avg(value)"),
    ("low", AggFunc::Min, "min(value)"),
    ("high", AggFunc::Max, "max(value)"),
];
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
    let mut ty = type_decl(
        TY,
        vec![
            sourced("seq", StorageKind::Number),
            sourced("site", StorageKind::Text),
            sourced("value", StorageKind::Number),
            FieldDecl {
                searchable: true,
                ..sourced("note", StorageKind::Text)
            },
            embedding("vec", 2, &["site"], EMBED_MODEL),
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
            filter: None,
        }),
        ..RelationDecl::to(SAME_NOTE, TY)
    });
    ty
}

fn by_site() -> TypeDecl {
    let mut fields = vec![field("site", StorageKind::Text)];
    fields.extend(MEASURES.map(|(name, ..)| field(name, StorageKind::Number)));
    let measures = MEASURES.map(|(name, _, expr)| (name, expr));
    view_of(BY_SITE, TY, &["site"], fields, &measures)
}

fn key(i: u64) -> RecordKey {
    format!("r{i:09}").as_str().into()
}

/// The note of record `i`: one word, its own.
fn note(i: u64) -> String {
    format!("{:08x}", i.wrapping_mul(2_654_435_761) as u32).repeat(25)
}

/// Record `i` of the source, as first delivered.
/// The seams a reopened or imported lake is given: the suite's backings,
/// embedder and executors.
fn seams() -> Seams {
    Seams {
        backings: backing::backings(),
        embed: Some(backing::embedder()),
        executors: backing::executors(),
        ..Seams::default()
    }
}

fn content(i: u64, value: f64) -> SourceRecord {
    let fields =
        json!({"seq": i, "site": format!("site-{}", i % 97), "value": value, "note": note(i)});
    source_record(TY, key(i).as_str(), fields)
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
    pattern(TY, None, vec![step(FOLLOWS, Walk::Both)], project)
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
                .map(|t| {
                    let fields = json!({"from": key(i).as_str(), "to": key(t).as_str()});
                    source_record(link.as_str(), link_key(&key(i), &key(t)).as_str(), fields)
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
        let Found::Records {
            rows, next_cursor, ..
        } = reply.data
        else {
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
    async fn ingest(&mut self, store: &dyn LakeStore, from: u64, n: u64) -> Held {
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
    async fn export(&mut self, store: &dyn LakeStore, records: u64, log: &Log, dir: &Path) -> Held {
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
        let m = Measure::start(&self.given.meter, true);
        let imported = (self.given.import)(dir.to_path_buf(), store.lake().clone(), None, seams())
            .await
            .map_err(|e| Failure::new(format!("importing {}: {e}", dir.display())))?
            .store;
        let measured = m.stop();
        let head = imported.head();
        let head = head.await.must("reading the imported lake's head")?;
        ensure!(
            head == through,
            "the lake imported from the export has its head at {head}; the export's is {through}"
        );
        imported.close().await.must("closing the imported lake")?;
        self.row("import", Claim::Held, records, log.events, what, measured);
        let _ = fs::remove_dir_all(dir);
        Ok(())
    }

    /// Count every `reading` through `find`'s `count()`, once with a filter
    /// the engine applies and once with one only the store evaluates, each
    /// measured: a count holds a batch, whatever the size of the type.
    async fn counts(&mut self, store: &dyn LakeStore, records: u64) -> Held {
        let cat = doors(store);
        let head = store.head().await.must("reading the head")?;
        for (phase, filter) in [
            ("count", "seq >= 0"),
            ("count-store", "seq >= 0 and now() > now() - 1d"),
        ] {
            let find = Find {
                filter: Some(filter.into()),
                aggregate: Some(lakelands_api::read::Aggregate {
                    measures: vec![crate::conformance::fixture::agg(
                        "n",
                        lakelands_api::read::AggFunc::Count,
                        None,
                    )],
                    group_by: Vec::new(),
                }),
                ..Find::default()
            };
            let started = Instant::now();
            let m = Measure::start(&self.given.meter, false);
            let reply = cat
                .find(&self.own, &TypeName::from(TY), find)
                .await
                .must(&format!("counting `reading` where {filter}"))?;
            let (heap, _) = m.stop();
            let counted = match &reply.data {
                Found::Groups { rows } => rows
                    .first()
                    .and_then(|g| g.values.get("n"))
                    .and_then(serde_json::Value::as_str)
                    .and_then(|n| n.parse::<u64>().ok()),
                Found::Records { .. } => None,
            };
            ensure!(
                counted == Some(self.live),
                "counting `reading` where {filter} must answer the {} live records; it \
                 answered {:?}",
                self.live,
                reply.data
            );
            self.row(
                phase,
                Claim::Held,
                records,
                head.0 + 1,
                format!("a count of {} where {filter}", self.live),
                (heap, started.elapsed()),
            );
        }
        Ok(())
    }

    /// Group every `reading` by its site, through `find`'s aggregate and
    /// then by counting the view `by_site`, each measured: a group keeps
    /// what its measures fold, never its records, so each holds a batch
    /// and the groups, whatever the size of the type (lakelands#388).
    async fn groups(&mut self, store: &dyn LakeStore, records: u64) -> Held {
        let cat = doors(store);
        let head = store.head().await.must("reading the head")?;
        let of = |func| (func != AggFunc::Count).then_some("value");
        let mut grouped = aggregate(MEASURES.map(|(n, func, _)| agg(n, func, of(func))).into());
        grouped.aggregate.as_mut().expect("an aggregate").group_by = vec!["site".into()];
        let counted = aggregate(vec![agg("n", AggFunc::Count, None)]);
        // The aggregate counts every live record, and the view holds a
        // record for each group the aggregate answered.
        let mut want = self.live;
        for (phase, ty, find) in [("aggregate", TY, grouped), ("view-count", BY_SITE, counted)] {
            let started = Instant::now();
            let m = Measure::start(&self.given.meter, false);
            let reply = cat
                .find(&self.own, &TypeName::from(ty), find)
                .await
                .must(&format!("grouping `{ty}` by site"))?;
            let (heap, _) = m.stop();
            let Found::Groups { rows } = &reply.data else {
                return Err(Failure::new(format!(
                    "an aggregate over `{ty}` answered records"
                )));
            };
            let n =
                |g: &lakelands_api::read::Group| g.values.get("n")?.as_str()?.parse::<u64>().ok();
            let got: u64 = rows.iter().filter_map(n).sum();
            ensure!(
                got == want,
                "{phase} over `{ty}` must count {want}; it counted {got}: {rows:?}"
            );
            want = rows.len() as u64;
            let what = format!("{got} counted over `{ty}`, answered in {want} rows");
            let measured = (heap, started.elapsed());
            self.row(phase, Claim::Held, records, head.0 + 1, what, measured);
        }
        Ok(())
    }

    /// The owner's first page, in key order, of `ticks`: a kept view with
    /// a group for each `tick`, one of which reads otherwise on the whole
    /// face, where a second member is restricted. It holds the page and
    /// that group, not the view (lakelands#628). A sum over every group and
    /// a pattern started on one hold a page of it (lakelands#731). It is fed
    /// before the feed and the export, so each size's import carries a view
    /// as large.
    async fn whole_face(&mut self, store: &dyn LakeStore, from: u64, records: u64) -> Held {
        let (cat, own) = (doors(store), self.own.clone());
        let tick = |key: &str, g: u64| source_record("tick", key, json!({"g": g}));
        if from == 0 {
            let ty = type_decl("tick", vec![sourced("g", StorageKind::Number)]);
            declare(cat.as_ref(), &own, ty).await?;
            let fields = vec![
                field("g", StorageKind::Number),
                field("c", StorageKind::Number),
            ];
            let view = view_of("ticks", "tick", &["g"], fields, &[("c", "count()")]);
            declare(cat.as_ref(), &own, view).await?;
            let l = Lake::of(store);
            l.set(&format!("{VIEW_KEPT_SETTING}ticks"), json!(true))
                .await?;
            let held = ingest(cat.as_ref(), &own, SOURCE, vec![tick("held", 0)]).await;
            held.must("feeding a tick")?;
            restrict(store, "tick", "held", "held").await?;
        }
        let ticks = (from..records)
            .map(|i| tick(&format!("t{i:09}"), i))
            .collect();
        let fut = ingest(cat.as_ref(), &own, SOURCE, ticks);
        fut.await.must("feeding ticks")?;
        let head = store.head().await.must("reading the head")?;
        let page = Find {
            limit: Some(10),
            ..Find::default()
        };
        let (started, m) = (Instant::now(), Measure::start(&self.given.meter, false));
        let reply = cat.find(&own, &"ticks".into(), page).await;
        let measured = (m.stop().0, started.elapsed());
        let reply = reply.must("the owner's first page of `ticks`")?;
        let Found::Records { rows, .. } = &reply.data else {
            return Err(Failure::new("a page of `ticks` answered groups"));
        };
        let first = rows.first().and_then(|r| r.fields.get("c"));
        ensure!(
            rows.len() == 10 && first == Some(&json!(2)),
            "the owner's page of `ticks` holds ten groups, the first `g` 0 of two members, one \
             restricted; it held {} and the first counts {first:?}",
            rows.len()
        );
        let what = format!("the owner's page of 10 of {records} kept groups, one differing");
        self.row(
            "kept-whole-page",
            Claim::Held,
            records,
            head.0 + 1,
            what,
            measured,
        );
        // A sum over every group and a pattern started on one: each reads
        // the kept records a page at a time (lakelands#731).
        let summed = aggregate(vec![agg("s", AggFunc::Sum, Some("c"))]);
        let (started, m) = (Instant::now(), Measure::start(&self.given.meter, false));
        let reply = cat.find(&own, &"ticks".into(), summed).await;
        let measured = (m.stop().0, started.elapsed());
        let reply = reply.must("the owner's sum over `ticks`")?;
        let sum = match &reply.data {
            Found::Groups { rows } => rows.first().and_then(|g| g.values.get("s")).cloned(),
            _ => None,
        };
        ensure!(
            sum == Some(json!((records + 1).to_string())),
            "the owner's sum of `c` over `ticks` counts every tick and the restricted one, {}; \
             it answered {sum:?}",
            records + 1
        );
        let what = format!("the owner's sum over {records} kept groups");
        self.row("kept-sum", Claim::Held, records, head.0 + 1, what, measured);
        let mut start = pattern(
            "ticks",
            None,
            Vec::new(),
            Projection::Rows { fields: Vec::new() },
        );
        start.start.filter = Some(lakelands_api::expr::Condition::Text(format!(
            "g = {}",
            records / 2
        )));
        let (started, m) = (Instant::now(), Measure::start(&self.given.meter, false));
        let reply = cat.query(&own, start).await;
        let measured = (m.stop().0, started.elapsed());
        let reply = reply.must("a pattern started on one group of `ticks`")?;
        let QueryResult::Rows { rows, .. } = &reply.data else {
            return Err(Failure::new("a pattern projecting rows answered a graph"));
        };
        ensure!(
            rows.len() == 1,
            "a pattern started on the group `g = {}` of `ticks` holds it alone; it held {}",
            records / 2,
            rows.len()
        );
        let what = format!("a pattern started on one of {records} kept groups");
        self.row(
            "kept-start",
            Claim::Held,
            records,
            head.0 + 1,
            what,
            measured,
        );
        Ok(())
    }

    /// Jump to the last page of `reading` with `offset`, asking for its
    /// `total`, measured: the rows skipped are read and dropped a page at
    /// a time, and the total counted a batch at a time, whatever the size
    /// of the type (045).
    async fn jump(&mut self, store: &dyn LakeStore, records: u64) -> Held {
        let cat = doors(store);
        let head = store.head().await.must("reading the head")?;
        let limit = 25u32;
        let skip = self.live.saturating_sub(u64::from(limit));
        let find = Find {
            offset: Some(u32::try_from(skip).unwrap_or(u32::MAX)),
            limit: Some(limit),
            total: true,
            ..Find::default()
        };
        let started = Instant::now();
        let m = Measure::start(&self.given.meter, false);
        let reply = cat
            .find(&self.own, &TypeName::from(TY), find)
            .await
            .must(&format!("jumping {skip} rows into `reading`"))?;
        let (heap, _) = m.stop();
        let Found::Records { rows, total, .. } = reply.data else {
            return Err(Failure::new("a page of `reading` answered groups"));
        };
        ensure!(
            rows.len() as u64 == self.live - skip && total == Some(self.live),
            "the page {skip} rows into `reading` holds its last {} records and a total of {}; \
             it held {} and {total:?}",
            self.live - skip,
            self.live,
            rows.len()
        );
        self.row(
            "jump",
            Claim::Held,
            records,
            head.0 + 1,
            format!("a page {skip} rows in, with its total"),
            (heap, started.elapsed()),
        );
        Ok(())
    }

    /// Walk every `reading` through `find`, a page of `max_limit` at a
    /// time with the cursor, measuring each page on its own: a page holds
    /// its rows, whatever the size of the type.
    async fn find_walk(&mut self, store: &dyn LakeStore, records: u64) -> Held {
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
                ..
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
    /// order index sorts them: a page holds the type, its heap reported
    /// (lakelands#193), the rows it reads held to the type and a page. Then
    /// after the setting `sort_index.reading` asks for the index on `note`:
    /// a page holds its rows (lakelands#137) and reads at most three more.
    /// Then the same walk with writes between its pages, which the cursor
    /// keeps out of it: to another type before the second page, and to `n`
    /// of the walk's own records before the fourth; a page reads at most
    /// three rows more than it answers, and two per record changed since
    /// its position. Every walk answers what the first, a sort of the
    /// type, answered. Then retire the setting.
    async fn sorted_walks(&mut self, store: &dyn LakeStore, records: u64) -> Held {
        let limit = u64::from(limits().max_limit);
        // The store asks the engine for a page and one more row, which
        // tells it whether another page follows, and reads the lake's
        // settings by key for every read: this check writes four.
        let asked = limit + 1 + 4;
        let live = self.live;
        let sorted = self.sorted_walk(store, records, "find-sorted-fallback", None, |_| {
            live + asked
        });
        let sorted = sorted.await?;
        let l = Lake::of(store);
        l.set("sort_index.reading", json!(["note"])).await?;
        let indexed = self.sorted_walk(store, records, "find-sorted", None, |_| asked + 3);
        let indexed = indexed.await?;
        // Eight records the fourth and fifth pages answer, spread over both.
        let changing: Vec<RecordKey> = sorted
            .iter()
            .skip(3 * limit as usize)
            .step_by((limit as usize / 4).max(1))
            .take(8)
            .cloned()
            .collect();
        let n = changing.len() as u64;
        let written = self.sorted_walk(
            store,
            records,
            "find-sorted-after-writes",
            Some(&changing),
            |page| asked + 3 + if page >= 4 { 2 * n } else { 0 },
        );
        let written = written.await?;
        for (phase, keys) in [
            ("find-sorted", indexed),
            ("find-sorted-after-writes", written),
        ] {
            ensure!(
                keys == sorted,
                "{phase} answered {} keys, the first walk, a sort of the type, {}; from the \
                 first that differs on, {:?} against {:?}",
                keys.len(),
                sorted.len(),
                keys.iter().zip(&sorted).find(|(a, b)| a != b),
                sorted
                    .iter()
                    .zip(&keys)
                    .find(|(a, b)| a != b)
                    .map(|(a, _)| a)
            );
        }
        // Retired, so the next size's first walk sorts again.
        l.retire(SETTING_TYPE, "sort_index.reading").await?;
        Ok(())
    }

    /// Walk the first pages of `reading` ordered by its `note`, a page of
    /// `max_limit` at a time with the cursor, measuring each page on its
    /// own, as `phase`, and holding the rows the engine reads for page `p`
    /// to `most(p)`. With records `changing`, a setting is written before
    /// the second page and those records are delivered again, with another
    /// value, before the fourth (at the first size the walk may end
    /// sooner). Answers the keys walked.
    async fn sorted_walk(
        &mut self,
        store: &dyn LakeStore,
        records: u64,
        phase: &'static str,
        changing: Option<&[RecordKey]>,
        most: impl Fn(u64) -> u64,
    ) -> Result<Vec<RecordKey>, Failure> {
        const PAGES: u64 = 5;
        let cat = doors(store);
        let head = store.head().await.must("reading the head")?;
        let limit = limits().max_limit;
        let mut find = Find {
            limit: Some(limit),
            order_by: vec![order("note", OrderBy::Value, Direction::Asc)],
            ..Find::default()
        };
        let mut previous: Option<String> = None;
        let mut walked = Vec::new();
        let mut worst: Option<Heap> = None;
        let started = Instant::now();
        for page in 1..=PAGES {
            if changing.is_some() && page == 2 {
                Lake::of(store)
                    .set("streaming.between", json!(page))
                    .await?;
            }
            if let Some(changing) = changing.filter(|_| page == 4) {
                // The value only, one this size has not delivered: the
                // note, and so the order, stays.
                let again = changing
                    .iter()
                    .filter_map(|k| k.as_str().strip_prefix('r')?.parse::<u64>().ok())
                    .map(|i| content(i, first_value(i) + records as f64 + 0.5))
                    .collect();
                let windows = lakelands_api::catalog::ingest_all(
                    cat.as_ref(),
                    &self.own,
                    &SourceId::from(SOURCE),
                    again,
                )
                .await
                .must("delivering records of `reading` again")?;
                let put: u64 = windows.iter().map(|w| u64::from(w.records)).sum();
                ensure!(
                    put == changing.len() as u64,
                    "delivering {} records of `reading` again with another value put {put}",
                    changing.len()
                );
            }
            let m = Measure::start(&self.given.meter, false);
            let before = crate::engines::rows_read_in_process();
            let reply = cat
                .find(&self.own, &TypeName::from(TY), find.clone())
                .await
                .must(&format!("finding page {page} of `reading` by `note`"))?;
            let read = crate::engines::rows_read_in_process() - before;
            let (heap, _) = m.stop();
            if let Some(h) = heap
                && worst.is_none_or(|w| h.held() > w.held())
            {
                worst = Some(h);
            }
            ensure!(
                read <= most(page),
                "{phase}: page {page} of {limit} by `note` read {read} rows of the type's {}; \
                 at most {} are allowed",
                self.live,
                most(page)
            );
            let Found::Records {
                rows, next_cursor, ..
            } = reply.data
            else {
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
                walked.push(r.key.clone());
            }
            match next_cursor {
                Some(c) => find.cursor = Some(c),
                None => break,
            }
        }
        let claim = match phase {
            // A page sorts the type, which is that order's stated ceiling.
            "find-sorted-fallback" => Claim::Reported("lakelands#193"),
            _ => Claim::Held,
        };
        self.row(
            phase,
            claim,
            records,
            head.0 + 1,
            format!("{PAGES} pages of at most {limit} by `note`; the largest page"),
            (worst, started.elapsed()),
        );
        Ok(walked)
    }

    /// Link every record delivered from `from` to `size` to the two it
    /// [`follows`], through a source; not measured.
    async fn link(&mut self, store: &dyn LakeStore, from: u64, size: u64) -> Held {
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
    async fn searches(&mut self, store: &dyn LakeStore, records: u64) -> Held {
        self.search(store, records, "search", "hybrid").await?;
        let l = Lake::of(store);
        l.set("vector_index.reading", json!(["vec"])).await?;
        self.search(store, records, "search-indexed", "hybrid (index)")
            .await?;
        l.set("vector_index.reading", json!([])).await?;
        Ok(())
    }

    /// One hybrid search for the note of [`SOUGHT`], reported as `phase`;
    /// it must say it ranked by `ranked_by`.
    async fn search(
        &mut self,
        store: &dyn LakeStore,
        records: u64,
        phase: &'static str,
        ranked_by: &str,
    ) -> Held {
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
                        text_fields: None,
                        weight: None,
                        text_weight: None,
                        model: None,
                    },
                    k: K,
                    cursor: None,
                    filter: None,
                    with_meta: false,
                    at: ReadAt::default(),
                    rerank: None,
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
    async fn traverse(&mut self, store: &dyn LakeStore, records: u64) -> Held {
        let head = store.head().await.must("reading the head")?;
        let first = u64::from(self.given.large_lake.max(20));
        let want = walk_rows(records, first);
        let mut pattern =
            Pattern::traverse(TypeName::from(TY), key(WALK_FROM), hops(FOLLOWS, Walk::In));
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
    async fn derived_walk(&mut self, store: &dyn LakeStore, records: u64) -> Held {
        let head = store.head().await.must("reading the head")?;
        let pattern = Pattern::traverse(
            TypeName::from(TY),
            key(WALK_FROM),
            hops(SAME_NOTE, Walk::Out),
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
    async fn degree(&mut self, store: &dyn LakeStore, records: u64) -> Held {
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
    async fn components(&mut self, store: &dyn LakeStore, records: u64) -> Held {
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
    async fn components_capped(&mut self, store: Store, records: u64) -> Result<Store, Failure> {
        let cap = self.given.large_lake.max(20) / 2;
        let capped = lakelands_api::location::Limits {
            max_graph_nodes: cap,
            ..limits()
        };
        let store = (self.given.relimit)(store, Seams::default(), capped);
        let store = store.await.map_err(|e| {
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
    async fn rebuild(&mut self, store: &dyn LakeStore, records: u64) -> Held {
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
    async fn reopen(&mut self, store: Store, records: u64) -> Result<Store, Failure> {
        let head = store.head().await.must("reading the head")?;
        let before = answers(store.as_ref(), &self.own).await?;
        let m = Measure::start(&self.given.meter, false);
        let store = (self.given.reopen)(store, seams())
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

/// A filtered poll reads only what the projection's index names
/// (lakelands#678): one whose filter matches nothing answers none and
/// `next` past the head in one poll, whatever the log's length; one
/// record's history from 0 answers its events in one poll; the
/// checkpoints are each answered once, `at` among them, in at most one
/// poll more than there are; and a poll with no filter still examines at
/// most [`FEED_EXAMINED`] events, whatever its `limit`.
async fn bounded(store: &dyn LakeStore, log: &Log, at: Position) -> Held {
    let (feed, bound) = (store.feed(), u64::from(FEED_EXAMINED));
    let kind = |k: &str| FeedFilter {
        kinds: vec![k.into()],
        ..FeedFilter::default()
    };
    let (never, checkpoints) = (kind("trigger_fired"), kind("checkpoint"));
    let none = feed.poll(Position::GENESIS, &never, 100);
    let none = none.await.must("polling for what the log never holds")?;
    ensure!(
        none.events.is_empty() && none.next == none.head.next(),
        "a poll from 0 for a kind the log of {} events never holds answered {} events and next \
         {}; it answers none and next past the head {} in one poll",
        log.events,
        none.events.len(),
        none.next,
        none.head
    );
    let one = FeedFilter {
        types: vec![TY.into()],
        keys: vec![key(3).as_str().into()],
        ..FeedFilter::default()
    };
    let history = feed.poll(Position::GENESIS, &one, 100);
    let history = history.await.must("polling one record's history")?;
    let about = |e: &Event| match &e.body {
        EventBody::RecordIngested { ty, key: k, .. } => ty == TY && *k == key(3),
        _ => false,
    };
    ensure!(
        !history.events.is_empty()
            && history.events.iter().all(about)
            && history.events.is_sorted_by(|a, b| a.position < b.position)
            && history.next == history.head.next(),
        "a poll from 0 for the record `{}` answered {:?} and next {} over a log of {} events; \
         it answers that record's events in order, and next past the head, in one poll",
        key(3),
        positions(&history.events),
        history.next,
        log.events
    );
    let (mut from, mut polls, mut found) = (Position::GENESIS, 0, Vec::new());
    loop {
        let b = feed.poll(from, &checkpoints, 1);
        let b = b.await.must("polling the checkpoints")?;
        ensure!(
            b.events.len() <= 1,
            "a poll with a limit of 1 answered {} events",
            b.events.len()
        );
        found.extend(b.events.iter().map(|e| e.position));
        (from, polls) = (b.next, polls + 1);
        if from > b.head {
            break;
        }
    }
    ensure!(
        found.len() as u64 == log.checkpoints
            && found.contains(&at)
            && found.is_sorted_by(|a, b| a < b)
            && polls <= log.checkpoints + 1,
        "following the checkpoints answered {found:?} in {polls} polls; the log holds {} \
         checkpoints, {at} among them, each answered once in at most one poll more",
        log.checkpoints
    );
    let every = FeedFilter::default();
    let all = feed.poll(Position::GENESIS, &every, u32::MAX);
    let all = all.await.must("polling the feed with no limit")?;
    let n = bound.min(log.events);
    ensure!(
        all.events.len() as u64 == n && all.next.0 == n,
        "a poll from 0 with no filter and a limit of {} answers {n} events and next {n}, the \
         bound; it answered {} and next {}",
        u32::MAX,
        all.events.len(),
        all.next
    );
    Ok(())
}

/// The positions of `events`.
fn positions(events: &[Event]) -> Vec<u64> {
    events.iter().map(|e| e.position.0).collect()
}

/// The report file, beside the check's scratch directory.
fn report_file(scratch: &Path) -> PathBuf {
    scratch.with_extension("jsonl")
}

async fn a_large_lake_goes_through_ingest_the_feed_and_export_in_memory_that_does_not_grow_with_it(
    store: Store,
    given: Given,
) -> Held {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(cat.as_ref(), &own, reading()).await?;
    declare(cat.as_ref(), &own, by_site()).await?;
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
        size.whole_face(store.as_ref(), from, records).await?;
        let at = store.checkpoint().await.must("taking a checkpoint")?;
        let log = size.feed(store.as_ref(), records).await?;
        bounded(store.as_ref(), &log, at).await?;
        let dir = given.scratch.join(format!("export-{records}"));
        size.export(store.as_ref(), records, &log, &dir).await?;
        size.find_walk(store.as_ref(), records).await?;
        size.counts(store.as_ref(), records).await?;
        size.groups(store.as_ref(), records).await?;
        size.jump(store.as_ref(), records).await?;
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
fn report(given: &Given, rows: &[Row]) -> Held {
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

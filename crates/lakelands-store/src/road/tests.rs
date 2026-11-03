//! The road under a test job (099, § The order of the work): a counter
//! whose every turn sleeps under the writer and keeps how far it has
//! counted as a projection output, as the acts keep their lists. It runs
//! on the memory engine, on SQLite, and on PostgreSQL where
//! `LAKELANDS_TEST_PG_URL` names one (the `pg` leg of
//! `scripts/conformance.sh` runs it there).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lakelands_api::engine::OpenMode;
use lakelands_api::event::Digest;
use lakelands_api::location::{EngineConfig, PgTls, PgTlsMode, PgTuning};
use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::{SETTING_TYPE, Seams};
use lakelands_api::write::Selection;
use lakelands_api::{LakeStore, Location, Position, Timestamp};
use lakelands_core::conformance::fixture::{self as fx, Lake as Doors};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use super::*;
use crate::doors::{Store, opened_owing, standing_by};
use crate::exec;

/// Where the counter keeps how far it has counted.
const KEPT: &str = "test:road";

/// What a test sees of its lake's counter, found again by a counter
/// [`resume`] sends ([`owes`] is a plain function and captures nothing).
#[derive(Default)]
struct Seen {
    /// Turns taken, by every counter of the lake.
    turns: AtomicU32,
    /// How long each turn held the writer.
    held: Mutex<Vec<Duration>>,
    /// Each fold's events' keys, in order.
    folds: Mutex<Vec<Vec<String>>>,
    /// How many times [`Job::reach`] asked the test to write.
    asked: AtomicU32,
    /// Where the counter stops for the test: what it says, and the go.
    pause: Mutex<Option<Pause>>,
    /// The last counter's ending.
    ended: Mutex<Option<oneshot::Sender<Result<()>>>>,
}

/// A counter's stop: where it stands, and the test's go.
struct Pause {
    at: &'static str,
    says: mpsc::UnboundedSender<&'static str>,
    go: std::sync::mpsc::Receiver<()>,
}

impl Seen {
    /// Stop at `at` where the test asked to, until it says go.
    fn stop(&self, at: &'static str) {
        let pause = lock(&self.pause);
        let Some(p) = pause.as_ref().filter(|p| p.at == at) else {
            return;
        };
        p.says.send(at).expect("the test listens");
        p.go.recv_timeout(Duration::from_secs(20))
            .expect("the test says go");
    }

    /// Every event folded so far.
    fn folded(&self) -> usize {
        lock(&self.folds).iter().map(Vec::len).sum()
    }
}

/// Each lake's [`Seen`], by name.
static SEEN: Mutex<BTreeMap<String, Arc<Seen>>> = Mutex::new(BTreeMap::new());

fn seen(lake: &str) -> Arc<Seen> {
    lock(&SEEN).entry(lake.to_owned()).or_default().clone()
}

/// The test job: counts to `of`, a turn a step, each turn `slept` under
/// the writer.
struct Counter {
    of: u32,
    done: u32,
    slept: Duration,
    reach: Reach,
    seen: Arc<Seen>,
}

/// The counter's reach where a test names none: every write about a note.
fn notes() -> Reach {
    Reach {
        types: vec!["note".into()],
        ..Reach::default()
    }
}

/// How far the counter of `view`'s lake has counted, if it has started.
fn kept(view: &dyn ProjectionView) -> Option<(u32, u32, u64)> {
    let (_, v) = view.output(KEPT, &Digest::zero()).ok()??;
    let n = |f: &str| v[f].as_u64().expect("a count");
    Some((n("done") as u32, n("of") as u32, n("slept_ms")))
}

/// The kinds the test's open finishes: the store's, and the counter's.
const KINDS: &[Owes] = &[
    crate::purge::owed,
    crate::remove::owed,
    crate::forget::owed,
    crate::remove::settle_owed,
    crate::build::owed,
    owes,
];

#[test]
fn every_kind_the_store_owes_the_test_also_runs() {
    // The test's open runs the store's kinds beside its own counter; a
    // kind the store adds is named in `KINDS` too.
    assert_eq!(
        OWED.len() + 1,
        KINDS.len(),
        "add the store's kinds to `KINDS`"
    );
}

/// The counters the log and the lists at `view` leave owed.
fn owes(lake: &Lake, view: &dyn ProjectionView) -> Result<Vec<Box<dyn Job>>> {
    let Some((done, of, slept)) = kept(view).filter(|(done, of, _)| done < of) else {
        return Ok(Vec::new());
    };
    let seen = seen(lake.id.as_str());
    let slept = Duration::from_millis(slept);
    Ok(vec![Box::new(Counter {
        of,
        done,
        slept,
        reach: notes(),
        seen,
    })])
}

impl Job for Counter {
    fn reach(&self) -> Reach {
        // Asked to write while everything written is folded, the test
        // writes past a view that has not seen it: the tail a first turn
        // finds is too long.
        let asked = self.seen.asked.load(Ordering::SeqCst);
        if self.seen.folded() == 3 * asked as usize && lock(&self.seen.pause).is_some() {
            self.seen.asked.fetch_add(1, Ordering::SeqCst);
            self.seen.stop("reach");
        }
        self.reach.clone()
    }

    fn walk(&mut self, _: &Lake, view: &dyn ProjectionView) -> Result<()> {
        if let Some((done, ..)) = kept(view) {
            self.done = done;
        }
        self.seen.stop("walk");
        Ok(())
    }

    fn fold(&mut self, tail: &[Event]) -> Result<()> {
        let keys = tail
            .iter()
            .flat_map(|e| lakelands_core::feed::about(&e.body));
        let keys = keys.filter_map(|a| a.key.map(|k| k.to_string())).collect();
        lock(&self.seen.folds).push(keys);
        Ok(())
    }

    fn turn(&mut self, t: &Turn<'_>, w: &mut Writer) -> Result<Turned> {
        let began = Instant::now();
        std::thread::sleep(self.slept);
        self.done += 1;
        let head = w.tip.as_ref().map_or(Position::GENESIS, |(p, _)| *p);
        let kept = json!({"done": self.done, "of": self.of,
            "slept_ms": self.slept.as_millis() as u64});
        let projection = t.engine.projection();
        projection.begin()?;
        let put = (projection.put_output(KEPT, &Digest::zero(), &kept, head))
            .and_then(|()| projection.commit(head));
        if put.is_err() {
            projection.rollback();
        }
        put?;
        lock(&self.seen.held).push(began.elapsed());
        self.seen.turns.fetch_add(1, Ordering::SeqCst);
        Ok(if self.done < self.of {
            Turned::More
        } else {
            Turned::Whole
        })
    }

    fn end(&mut self, ended: Result<()>) {
        if let Some(tx) = lock(&self.seen.ended).take() {
            let _ = tx.send(ended);
        }
    }
}

/// An engine a lake of the test is kept on.
#[derive(Clone, Copy, Debug)]
enum On {
    Memory,
    Sqlite,
    Postgres,
}

/// Every engine this run reaches: PostgreSQL only where a URL names it.
fn engines() -> Vec<On> {
    let mut on = vec![On::Memory, On::Sqlite];
    match pg_url() {
        Some(_) => on.push(On::Postgres),
        None => eprintln!("road: LAKELANDS_TEST_PG_URL is not set; no PostgreSQL to run against"),
    }
    on
}

fn pg_url() -> Option<String> {
    std::env::var("LAKELANDS_TEST_PG_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
}

/// A lake of the test: where it is kept, the seams that open it again,
/// and what is cleared when the test is done.
struct Place {
    location: Location,
    seams: Seams,
    dir: Option<PathBuf>,
    schema: Option<(String, String)>,
}

impl Place {
    fn new(on: On, test: &str) -> Place {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let lake = format!("road-{test}-{on:?}-{pid}-{n}").to_lowercase();
        let opener: Arc<dyn lakelands_api::engine::EngineOpener> = match on {
            On::Memory => Arc::new(lakelands_engine_mem::MemoryOpener::new()),
            On::Sqlite => Arc::new(lakelands_engine_sqlite::SqliteOpener),
            On::Postgres => Arc::new(lakelands_engine_pg::PgOpener),
        };
        let seams = Seams {
            engines: vec![opener],
            ..Seams::default()
        };
        let (mut place, dir, schema) = (Location::memory(lake.as_str()), None, None);
        let mut place = match on {
            On::Memory => Place {
                location: place,
                seams,
                dir,
                schema,
            },
            On::Sqlite => {
                let dir = std::env::temp_dir().join(&lake);
                place = Location::sqlite(lake.as_str(), dir.clone());
                Place {
                    location: place,
                    seams,
                    dir: Some(dir),
                    schema,
                }
            }
            On::Postgres => {
                let url = pg_url().expect("a PostgreSQL URL");
                let schema = format!("lakelands_test_{pid}_road_{n}");
                place.engine = EngineConfig::Postgres {
                    url: url.clone(),
                    schema: Some(schema.clone()),
                    tls: PgTls {
                        mode: PgTlsMode::Disable,
                        ..PgTls::default()
                    },
                    tuning: PgTuning::default(),
                    tablespaces: Default::default(),
                };
                Place {
                    location: place,
                    seams,
                    dir,
                    schema: Some((url, schema)),
                }
            }
        };
        place.location.limits.part_tail = 1_000;
        place
    }

    /// The lake, opened as the store opens it but for the kinds its
    /// writer finishes: the counter's, beside the store's.
    async fn open(&self, mode: OpenMode) -> Arc<Lake> {
        let (at, seams) = (self.location.clone(), self.seams.clone());
        let lake = opened_owing(at, seams, mode, KINDS).await;
        lake.expect("the lake opens")
    }

    /// Open as [`Self::open`] does, again while a lake a test stopped, or
    /// the road beside a writer, still holds the lease: the handle that
    /// held it lets it go as its last reference drops, a moment after the
    /// test stopped it or the road's turn ended.
    async fn open_when_free(&self, mode: OpenMode) -> Arc<Lake> {
        for _ in 0..1_000 {
            let (at, seams) = (self.location.clone(), self.seams.clone());
            match opened_owing(at, seams, mode, KINDS).await {
                Ok(lake) => return lake,
                Err(e)
                    if matches!(
                        e.situation(),
                        Some(
                            lakelands_api::error::Situation::LeaseHeld { .. }
                                | lakelands_api::error::Situation::LeaseLocked { .. }
                        )
                    ) =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(e) => panic!("the lake opens: {e:?}"),
            }
        }
        panic!("the lake the test stopped never freed its lease");
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
        if let Some((url, schema)) = self.schema.take() {
            // The client runs a runtime of its own: not on a runtime's thread.
            let dropped = std::thread::spawn(move || {
                let mut client = postgres::Client::connect(&url, postgres::NoTls)?;
                client.batch_execute(&format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE"))
            });
            let _ = dropped.join();
        }
    }
}

/// A lake with a type `note` and a type `other`, and its doors.
async fn lake_of(place: &Place) -> (Arc<Lake>, Doors) {
    let lake = place.open(OpenMode::Create).await;
    let doors = Doors::of(&Store {
        lake: lake.clone(),
        owns: false,
    });
    for ty in ["note", "other"] {
        let decl = fx::type_decl(ty, vec![fx::field("n", StorageKind::Text)]);
        doors.declare(decl).await.expect("the type is declared");
    }
    (lake, doors)
}

/// Send a counter to `of` on `lake`'s road: how its lake's test sees it,
/// and its ending.
fn count(lake: &Arc<Lake>, of: u32, slept: Duration) -> (Arc<Seen>, oneshot::Receiver<Result<()>>) {
    count_reaching(lake, of, slept, notes())
}

/// [`count`], reaching what `reach` names.
fn count_reaching(
    lake: &Arc<Lake>,
    of: u32,
    slept: Duration,
    reach: Reach,
) -> (Arc<Seen>, oneshot::Receiver<Result<()>>) {
    let seen = seen(lake.id.as_str());
    let (tx, ended) = oneshot::channel();
    *lock(&seen.ended) = Some(tx);
    let job = Counter {
        of,
        done: 0,
        slept,
        reach,
        seen: seen.clone(),
    };
    send(lake, Box::new(job));
    (seen, ended)
}

/// The counter's ending, within `within`.
async fn ending(ended: oneshot::Receiver<Result<()>>, within: Duration) -> Result<()> {
    let ended = tokio::time::timeout(within, ended).await;
    ended
        .expect("the counter ends in time")
        .expect("the counter ends")
}

/// A write sent while a job runs waits for at most the turn in front of
/// it: the runner queues the job's next turn only when its last one has
/// ended, so the write is already in the queue ahead of it. A turn is
/// told by the turns the job counted while the write waited, since a
/// sleep on a loaded machine runs longer than it was asked to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_waits_for_at_most_the_turn_in_front_of_it() {
    let (of, turn) = (40, Duration::from_millis(50));
    for on in engines() {
        let place = Place::new(on, "waits");
        let (lake, doors) = lake_of(&place).await;
        let (seen, ended) = count(&lake, of, turn);
        while seen.turns.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let mut waited = Vec::new();
        for i in 0..20 {
            let (at, before) = (Instant::now(), seen.turns.load(Ordering::SeqCst));
            let wrote = doors
                .upsert("note", &format!("w{i}"), json!({"n": "x"}))
                .await;
            wrote.expect("the write lands");
            let passed = seen.turns.load(Ordering::SeqCst) - before;
            assert!(passed <= 1, "{on:?}: write {i} waited for {passed} turns");
            waited.push(at.elapsed());
        }
        let turns = seen.turns.load(Ordering::SeqCst);
        assert!(
            turns < of,
            "{on:?}: the job ran beside all the writes ({turns} turns)"
        );
        ending(ended, Duration::from_secs(60))
            .await
            .expect("the counter is whole");
        assert_eq!(
            seen.turns.load(Ordering::SeqCst),
            of,
            "{on:?}: one turn a step"
        );
        let mut held = lock(&seen.held).clone();
        let (waited, held) = (spread(&mut waited), spread(&mut held));
        eprintln!(
            "road {on:?}: 20 writes beside {of} turns of {turn:?}, held {held}: waited {waited}"
        );
    }
}

/// The `record_retired`s of `lake`'s log after `from` through `through`.
fn retirements(lake: &Lake, from: Position, through: Position) -> usize {
    let mut n = 0;
    for page in lake.pages(from.next()) {
        let Ok(page) = page else { break };
        for e in page {
            if e.position > through {
                return n;
            }
            if matches!(
                e.body,
                lakelands_api::event::EventBody::RecordRetired { .. }
            ) {
                n += 1;
            }
        }
    }
    n
}

/// The head of `lake`'s log.
fn head(lake: &Lake) -> Position {
    let view = lake.engine.view(None).expect("a view");
    view.position().unwrap_or(Position::GENESIS)
}

/// A gate the test holds at the writer: an entry that takes the turn
/// after whatever part is in flight, reads the log's tip, and waits for
/// the test to let it go. The act queues its next part behind it; a write
/// the test queues behind that part waits for it and for nothing before
/// it. The tip is the log as everything in front of the gate left it, so
/// neither the scheduler's interleaving nor a projection mark behind the
/// log can widen what the write is counted to have waited for (099).
struct Gate {
    /// The log's tip when the gate took the turn.
    tip: Position,
    /// Letting it go: dropping it.
    release: Option<std::sync::mpsc::Sender<()>>,
    /// The entry's task.
    held: tokio::task::JoinHandle<()>,
}

impl Gate {
    /// Take the turn after the part in flight, read the tip, and hold.
    async fn hold(lake: &Arc<Lake>) -> Gate {
        let (said, tip) = oneshot::channel();
        let (go, went) = std::sync::mpsc::channel::<()>();
        let gate = Arc::clone(lake);
        let held = tokio::spawn(async move {
            let _ = exec::write(&gate, move |t| {
                let tip = t.engine.tip()?.map_or(Position::GENESIS, |(at, _)| at);
                let _ = said.send(tip);
                let _ = went.recv();
                Ok(())
            })
            .await;
        });
        let tip = tip.await.expect("the gate reads the log's tip");
        Gate {
            tip,
            release: Some(go),
            held,
        }
    }

    /// Let what stands behind it run.
    fn release(mut self) -> tokio::task::JoinHandle<()> {
        drop(self.release.take());
        self.held
    }
}

/// Wait up to `within`, yielding and never sleeping, until `ready` holds
/// or fail saying `what`: what the test asks is the writer's queue, which
/// nothing tells it of, so it asks again rather than sleeping on an
/// estimate of how long the road takes.
async fn until(within: Duration, ready: impl Fn() -> bool, what: &str) {
    let until = Instant::now() + within;
    while !ready() {
        assert!(Instant::now() < until, "{what}");
        tokio::task::yield_now().await;
    }
}

/// Wait until `n` entries stand in `lake`'s writer queue, behind the gate
/// that holds the turn.
async fn behind(lake: &Lake, n: usize) {
    until(
        Duration::from_secs(20),
        || exec::queued(lake) >= n,
        &format!("the writer's queue never held {n} entries"),
    )
    .await;
}

/// The step's own numbers (099, R2): a part is the act's turn, and a write
/// sent beside the act waits for at most the part in front of it. The test
/// holds the writer between parts, queues the act's next part and then the
/// write behind its gate, and reads the wait from the log — the
/// retirements between the gate's tip and the write's own position. The
/// parts' duration comes from the act's turns after its first, which the
/// log tells are over once `source_purged` is in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_waits_for_at_most_a_part_of_a_purge() {
    for on in engines() {
        // A part of one record: every part is one retirement, so a write
        // that waited for two parts would find two of them in the log.
        let (of, room) = (200usize, 1u32);
        let mut place = Place::new(on, "purge-waits");
        place.location.limits.part_events = room;
        let (lake, doors) = lake_of(&place).await;
        // A type only a source writes, so `crm` may bring its records.
        doors
            .declare(fx::type_decl(
                "brought",
                vec![fx::sourced("n", StorageKind::Text)],
            ))
            .await
            .expect("the type is declared");
        let records = (0..of)
            .map(|i| fx::source_record("brought", &format!("n{i}"), json!({"n": "x"})))
            .collect();
        doors
            .ingest("crm", records)
            .await
            .expect("the source brings its records");
        let start = head(&lake);
        let (tx, ended) = oneshot::channel();
        crate::purge::start(&lake, "crm".into(), None, None, Some(tx));
        // Wait for the act's first turn: `source_purged` is its event.
        let first = start.next();
        until(
            Duration::from_secs(30),
            || head(&lake) >= first,
            "the act's first turn appends `source_purged`",
        )
        .await;
        let mut waited = Vec::new();
        let mut passed = Vec::new();
        for i in 0..20 {
            // The gate takes the writer after the part in flight; the
            // act's next part and then the write stand behind it, in that
            // order.
            let gate = Gate::hold(&lake).await;
            let tip = gate.tip;
            behind(&lake, 1).await;
            let key = format!("w{i}");
            let other = fx::Lake {
                cat: doors.cat.clone(),
                own: doors.own.clone(),
            };
            let writing =
                tokio::spawn(async move { other.upsert("other", &key, json!({"n": "x"})).await });
            behind(&lake, 2).await;
            let at = Instant::now();
            let released = gate.release();
            let wrote = writing
                .await
                .expect("the write's task ends")
                .expect("the write lands");
            released.await.expect("the gate's entry ends");
            let lakelands_api::Outcome::Applied { position, .. } = wrote else {
                panic!("{on:?}: the write applied");
            };
            waited.push(at.elapsed());
            let retired = retirements(&lake, tip, position);
            assert!(
                retired <= room as usize,
                "{on:?}: write {i} waited for {retired} retirements, more than a part of {room}"
            );
            passed.push(retired);
        }
        let whole = tokio::time::timeout(Duration::from_secs(120), ended)
            .await
            .expect("the act ends in time")
            .expect("the act answers");
        whole.expect("the act is whole");
        let parts = (of as u32).div_ceil(room) + u32::from((of as u32).is_multiple_of(room));
        assert!(
            passed.iter().any(|n| *n > 0),
            "{on:?}: every write landed before a part did"
        );
        let waited = spread(&mut waited);
        eprintln!(
            "purge {on:?}: 20 writes beside parts of {room}, the retirements each waited for \
             {passed:?}: waited {waited}"
        );

        // A part of `part_events`: the act's turns after its first, timed
        // from `source_purged` to its answer.
        let (of, room) = (2_000usize, 500u32);
        let mut place = Place::new(on, "purge-part-time");
        place.location.limits.part_events = room;
        let (lake, doors) = lake_of(&place).await;
        doors
            .declare(fx::type_decl(
                "brought",
                vec![fx::sourced("n", StorageKind::Text)],
            ))
            .await
            .expect("the type is declared");
        let records = (0..of)
            .map(|i| fx::source_record("brought", &format!("n{i}"), json!({"n": "x"})))
            .collect();
        doors
            .ingest("crm", records)
            .await
            .expect("the source brings its records");
        let start = head(&lake);
        let (tx, ended) = oneshot::channel();
        crate::purge::start(&lake, "crm".into(), None, None, Some(tx));
        let first = start.next();
        until(
            Duration::from_secs(30),
            || head(&lake) >= first,
            "the act's first turn appends `source_purged`",
        )
        .await;
        let began = Instant::now();
        let whole = tokio::time::timeout(Duration::from_secs(180), ended)
            .await
            .expect("the act ends in time")
            .expect("the act answers");
        whole.expect("the act is whole");
        let timed = began.elapsed();
        let turns = (of as u32).div_ceil(room) + u32::from((of as u32).is_multiple_of(room));
        let each = timed.as_secs_f64() * 1e3 / f64::from(turns);
        assert!(parts > 1);
        eprintln!(
            "purge {on:?}: {of} retirements in {turns} turns after the first of a part of {room}: \
             {timed:?} after `source_purged`, {each:.1} ms a turn"
        );
    }
}

/// A remove holds the writer for at most the part in front of a write,
/// as a purge does: the gate below the parts puts the write behind the
/// act's next part alone, and the act's turns after its first are timed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_waits_for_at_most_a_part_of_a_remove() {
    let all = || Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    for on in engines() {
        // A part of one record: every part is one retirement, so a write
        // that waited for two parts would find two of them in the log.
        let (of, room) = (200usize, 1u32);
        let mut place = Place::new(on, "remove-waits");
        place.location.limits.part_events = room;
        let (lake, doors) = lake_of(&place).await;
        // A type only a source writes, so `crm` may bring its records.
        doors
            .declare(fx::type_decl(
                "brought",
                vec![fx::sourced("n", StorageKind::Text)],
            ))
            .await
            .expect("the type is declared");
        let records = (0..of)
            .map(|i| fx::source_record("brought", &format!("n{i}"), json!({"n": "x"})))
            .collect();
        doors
            .ingest("crm", records)
            .await
            .expect("the source brings its records");
        let start = head(&lake);
        let (tx, ended) = oneshot::channel();
        crate::remove::start(&lake, "crm".into(), all(), false, None, Some(tx));
        // Wait for the act's first turn: `values_removed` is its event.
        let first = start.next();
        until(
            Duration::from_secs(30),
            || head(&lake) >= first,
            "the act's first turn appends `values_removed`",
        )
        .await;
        let mut waited = Vec::new();
        let mut passed = Vec::new();
        for i in 0..20 {
            // The gate takes the writer after the part in flight; the
            // act's next part and then the write stand behind it, in that
            // order.
            let gate = Gate::hold(&lake).await;
            let tip = gate.tip;
            behind(&lake, 1).await;
            let key = format!("w{i}");
            let other = fx::Lake {
                cat: doors.cat.clone(),
                own: doors.own.clone(),
            };
            let writing =
                tokio::spawn(async move { other.upsert("other", &key, json!({"n": "x"})).await });
            behind(&lake, 2).await;
            let at = Instant::now();
            let released = gate.release();
            let wrote = writing
                .await
                .expect("the write's task ends")
                .expect("the write lands");
            released.await.expect("the gate's entry ends");
            let lakelands_api::Outcome::Applied { position, .. } = wrote else {
                panic!("{on:?}: the write applied");
            };
            waited.push(at.elapsed());
            let retired = retirements(&lake, tip, position);
            assert!(
                retired <= room as usize,
                "{on:?}: write {i} waited for {retired} retirements, more than a part of {room}"
            );
            passed.push(retired);
        }
        let whole = tokio::time::timeout(Duration::from_secs(180), ended)
            .await
            .expect("the act ends in time")
            .expect("the act answers");
        whole.expect("the act is whole");
        assert!(
            passed.iter().any(|n| *n > 0),
            "{on:?}: every write landed before a part did"
        );
        let waited = spread(&mut waited);
        eprintln!(
            "remove {on:?}: 20 writes beside parts of {room}, the retirements each waited for \
             {passed:?}: waited {waited}"
        );

        // A part of `part_events`: the act's turns after its first, timed
        // from `values_removed` to its answer.
        let (of, room) = (2_000usize, 500u32);
        let mut place = Place::new(on, "remove-part-time");
        place.location.limits.part_events = room;
        let (lake, doors) = lake_of(&place).await;
        doors
            .declare(fx::type_decl(
                "brought",
                vec![fx::sourced("n", StorageKind::Text)],
            ))
            .await
            .expect("the type is declared");
        let records = (0..of)
            .map(|i| fx::source_record("brought", &format!("n{i}"), json!({"n": "x"})))
            .collect();
        doors
            .ingest("crm", records)
            .await
            .expect("the source brings its records");
        let (tx, ended) = oneshot::channel();
        crate::remove::start(&lake, "crm".into(), all(), false, None, Some(tx));
        let began = Instant::now();
        let whole = tokio::time::timeout(Duration::from_secs(180), ended)
            .await
            .expect("the act ends in time")
            .expect("the act answers");
        whole.expect("the act is whole");
        let timed = began.elapsed();
        let turns = (of as u32).div_ceil(room) + u32::from((of as u32).is_multiple_of(room));
        let each = timed.as_secs_f64() * 1e3 / f64::from(turns);
        assert!(turns > 1);
        eprintln!(
            "remove {on:?}: {of} retirements in {turns} turns after the first of a part of {room}: \
             {timed:?} after `values_removed`, {each:.1} ms a turn"
        );
    }
}

/// A forgetting holds the writer for at most the part in front of a
/// write, as a removal and a purge do: the gate below the parts puts the
/// write behind the act's next part alone, and the act's turns after its
/// first are timed from `source_forgotten`. Its first turn appends the
/// `source_forgotten` and the first part together.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_waits_for_at_most_a_part_of_a_forget() {
    for on in engines() {
        // A part of one record: every part is one retirement, so a write
        // that waited for two parts would find two of them in the log.
        let (of, room) = (200usize, 1u32);
        let mut place = Place::new(on, "forget-waits");
        place.location.limits.part_events = room;
        let (lake, doors) = lake_of(&place).await;
        // A type only a source writes, so `crm` may bring its records.
        // None of its fields is erasable, so forgetting `crm` needs no key.
        doors
            .declare(fx::type_decl(
                "brought",
                vec![fx::sourced("n", StorageKind::Text)],
            ))
            .await
            .expect("the type is declared");
        let records = (0..of)
            .map(|i| fx::source_record("brought", &format!("n{i}"), json!({"n": "x"})))
            .collect();
        doors
            .ingest("crm", records)
            .await
            .expect("the source brings its records");
        let start = head(&lake);
        let (tx, ended) = oneshot::channel();
        crate::forget::start(&lake, vec!["crm".into()], None, false, Some(tx));
        // Wait for the act's first turn: `source_forgotten` is its event.
        let first = start.next();
        until(
            Duration::from_secs(30),
            || head(&lake) >= first,
            "the act's first turn appends `source_forgotten`",
        )
        .await;
        let mut waited = Vec::new();
        let mut passed = Vec::new();
        for i in 0..20 {
            // The gate takes the writer after the part in flight; the
            // act's next part and then the write stand behind it, in that
            // order.
            let gate = Gate::hold(&lake).await;
            let tip = gate.tip;
            behind(&lake, 1).await;
            let key = format!("w{i}");
            let other = fx::Lake {
                cat: doors.cat.clone(),
                own: doors.own.clone(),
            };
            let writing =
                tokio::spawn(async move { other.upsert("other", &key, json!({"n": "x"})).await });
            behind(&lake, 2).await;
            let at = Instant::now();
            let released = gate.release();
            let wrote = writing
                .await
                .expect("the write's task ends")
                .expect("the write lands");
            released.await.expect("the gate's entry ends");
            let lakelands_api::Outcome::Applied { position, .. } = wrote else {
                panic!("{on:?}: the write applied");
            };
            waited.push(at.elapsed());
            let retired = retirements(&lake, tip, position);
            assert!(
                retired <= room as usize,
                "{on:?}: write {i} waited for {retired} retirements, more than a part of {room}"
            );
            passed.push(retired);
        }
        let whole = tokio::time::timeout(Duration::from_secs(180), ended)
            .await
            .expect("the act ends in time")
            .expect("the act answers");
        whole.expect("the act is whole");
        assert!(
            passed.iter().any(|n| *n > 0),
            "{on:?}: every write landed before a part did"
        );
        let waited = spread(&mut waited);
        eprintln!(
            "forget {on:?}: 20 writes beside parts of {room}, the retirements each waited for \
             {passed:?}: waited {waited}"
        );

        // A part of `part_events`: the act's turns after its first, timed
        // from `source_forgotten` to its answer.
        let (of, room) = (2_000usize, 500u32);
        let mut place = Place::new(on, "forget-part-time");
        place.location.limits.part_events = room;
        let (lake, doors) = lake_of(&place).await;
        doors
            .declare(fx::type_decl(
                "brought",
                vec![fx::sourced("n", StorageKind::Text)],
            ))
            .await
            .expect("the type is declared");
        let records = (0..of)
            .map(|i| fx::source_record("brought", &format!("n{i}"), json!({"n": "x"})))
            .collect();
        doors
            .ingest("crm", records)
            .await
            .expect("the source brings its records");
        let start = head(&lake);
        let (tx, ended) = oneshot::channel();
        crate::forget::start(&lake, vec!["crm".into()], None, false, Some(tx));
        let whole = tokio::time::timeout(Duration::from_secs(180), ended)
            .await
            .expect("the act ends in time")
            .expect("the act answers");
        whole.expect("the act is whole");
        // The act's turns after its first, as the log's wall time tells
        // them: from its `source_forgotten` to its `act_whole`.
        let mut times = Vec::new();
        for page in lake.pages(start.next()) {
            for e in page.expect("a page of the act's log") {
                match e.body {
                    lakelands_api::event::EventBody::SourceForgotten { .. } => {
                        times.push(e.known_at)
                    }
                    lakelands_api::event::EventBody::ActWhole { .. } => times.push(e.known_at),
                    _ => {}
                }
            }
        }
        let [sf, whole] = times.as_slice() else {
            panic!(
                "{on:?}: the act's `source_forgotten` and `act_whole` are in the log: {times:?}"
            );
        };
        let timed = Duration::from_micros(
            u64::try_from(whole.0 - sf.0).expect("the log's time moves forward"),
        );
        // The first turn appends the `source_forgotten` and the first part,
        // so the interval holds every part, the first among them.
        let parts = (of as u32).div_ceil(room);
        let each = timed.as_secs_f64() * 1e3 / f64::from(parts.max(1));
        assert!(parts > 1);
        eprintln!(
            "forget {on:?}: {of} retirements in {parts} parts of {room}: {timed:?} from \
             `source_forgotten` to `act_whole`, {each:.1} ms a part"
        );
    }
}

/// The forget act's job, to drive its walk and its first turn by hand with
/// another write between them.
fn a_forget_job(set: Vec<SourceId>, report_id: Option<String>, known: bool) -> Box<dyn Job> {
    Box::new(crate::forget::act_job(set, report_id, known, None))
}

/// A declaration between the walk and the first turn moves what is
/// erasable: the first turn judges the log under the ontology it finds, so
/// a value written in plaintext before its field was erasable refuses the
/// act, as it does when the walk saw the field already (058, 099).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_field_made_erasable_between_the_walk_and_the_first_turn_refuses_the_forget() {
    for on in engines() {
        let place = Place::new(on, "forget-shape");
        let (lake, doors) = lake_of(&place).await;
        let ty = fx::type_decl("brought", vec![fx::sourced("n", StorageKind::Text)]);
        doors.declare(ty).await.expect("the type is declared");
        doors
            .ingest(
                "crm",
                vec![fx::source_record("brought", "n1", json!({"n": "Ada"}))],
            )
            .await
            .expect("the source brings a record");
        let start = head(&lake);
        let mut job = a_forget_job(vec!["crm".into()], None, false);
        // The walk, off the writer, at the field not erasable yet.
        let view = lake.engine.view(None).expect("a view");
        job.walk(&lake, view.as_ref()).expect("the act walks");
        drop(view);
        // Another hand makes the field erasable before the first turn.
        let mut to = fx::type_decl("brought", vec![fx::sourced("n", StorageKind::Text)]);
        to.fields[0].erasable = true;
        doors.migrate(to).await.expect("the type migrates");
        let before = head(&lake);
        let out = exec::write(&lake, move |t| super::turn(t, job.as_mut())).await;
        let e = out.expect_err("the act refuses a value in plaintext");
        assert!(
            e.to_string().contains("holds 1 value(s) of `crm`")
                && e.to_string().contains("not forgotten"),
            "{on:?}: the act refuses the plaintext `n1` under the field erasable now: {e}"
        );
        assert_eq!(
            head(&lake),
            before,
            "{on:?}: the refused act appends nothing"
        );
        // Nothing of the act's was appended.
        let mut wrote = false;
        for page in lake.pages(start.next()) {
            for e in page.expect("a page of the log") {
                wrote |= matches!(
                    e.body,
                    lakelands_api::event::EventBody::SourceForgotten { .. }
                        | lakelands_api::event::EventBody::RecordRetired { .. }
                );
            }
        }
        assert!(!wrote, "{on:?}: the refused act appended nothing");
    }
}

/// A declaration that takes a field's `initial` away between the walk and
/// the first turn makes the record another hand wrote co-owned again: the
/// first turn gathers under the ontology it finds there and leaves the
/// record standing, where the walk's gathering would retire it (058, 099).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_initial_taken_away_between_the_walk_and_the_first_turn_keeps_the_record() {
    for on in engines() {
        let place = Place::new(on, "forget-shape-initial");
        let (lake, doors) = lake_of(&place).await;
        let mut m = fx::field("m", StorageKind::Text);
        m.initial = Some(json!("x"));
        let ty = fx::type_decl("brought", vec![fx::sourced("n", StorageKind::Text), m]);
        doors.declare(ty).await.expect("the type is declared");
        doors
            .ingest(
                "crm",
                vec![fx::source_record("brought", "m1", json!({"n": "Ada"}))],
            )
            .await
            .expect("the source brings a record");
        doors
            .upsert("brought", "m1", json!({"m": "theirs"}))
            .await
            .expect("another hand writes the field");
        let start = head(&lake);
        let mut job = a_forget_job(vec!["crm".into()], None, false);
        // The walk, off the writer, at the field's `initial` standing.
        let view = lake.engine.view(None).expect("a view");
        job.walk(&lake, view.as_ref()).expect("the act walks");
        drop(view);
        // The initial goes: the field the other hand wrote co-owns again.
        let ty = fx::type_decl(
            "brought",
            vec![
                fx::sourced("n", StorageKind::Text),
                fx::field("m", StorageKind::Text),
            ],
        );
        doors
            .declare(ty)
            .await
            .expect("the type is declared again without the initial");
        let out = exec::write(&lake, move |t| super::turn(t, job.as_mut())).await;
        out.expect("the act runs");
        assert_eq!(
            retirements(&lake, start, head(&lake)),
            0,
            "{on:?}: the record another hand wrote is left standing"
        );
    }
}

/// A removal act of the set between the walk and the first turn takes what
/// the report's id named: the first turn judges the id again, and the act
/// under it is refused stale (099).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_removal_between_the_walk_and_the_first_turn_stales_the_forgets_report() {
    for on in engines() {
        let place = Place::new(on, "forget-stale");
        let (lake, doors) = lake_of(&place).await;
        let ty = fx::type_decl("brought", vec![fx::sourced("n", StorageKind::Text)]);
        doors.declare(ty).await.expect("the type is declared");
        doors
            .ingest(
                "crm",
                vec![fx::source_record("brought", "n1", json!({"n": "Ada"}))],
            )
            .await
            .expect("the source brings a record");
        let crm = "crm".into();
        let id = crate::forget::preflight(&lake, &crm)
            .expect("the preflight")
            .report_id;
        let mut job = a_forget_job(vec![crm.clone()], Some(id), false);
        // The walk, off the writer, at the report's position.
        let view = lake.engine.view(None).expect("a view");
        job.walk(&lake, view.as_ref()).expect("the act walks");
        drop(view);
        // Another act of the source takes what the report named.
        let every = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
        let store = Store {
            lake: lake.clone(),
            owns: false,
        };
        let removed = store.remove_values(&crm, &every, false, None).await;
        removed.expect("the removal act");
        let before = head(&lake);
        let out = exec::write(&lake, move |t| super::turn(t, job.as_mut())).await;
        let e = out.expect_err("the act refuses the report the removal emptied");
        assert!(
            e.to_string()
                .contains("not of this forgetting at its position"),
            "{on:?}: the act under the report's id is refused stale: {e}"
        );
        assert_eq!(
            head(&lake),
            before,
            "{on:?}: the refused act appends nothing"
        );
    }
}

/// The median and the longest of `d`.
fn spread(d: &mut [Duration]) -> String {
    d.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    format!(
        "p50 {:.1} ms, max {:.1} ms",
        ms(d[d.len() / 2]),
        ms(d[d.len() - 1])
    )
}

/// A runner stopped between turns, as the lake's last handle goes, leaves
/// what it has done in the lists; the writer's next open finds the rest
/// owed and runs it beside the writer, answering before it is done, and
/// the count ends at its end, no step taken twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_runner_stopped_between_turns_is_finished_by_the_next_open() {
    let (of, turn) = (12, Duration::from_millis(40));
    for on in engines() {
        let place = Place::new(on, "stopped");
        let (lake, doors) = lake_of(&place).await;
        let (seen, ended) = count(&lake, of, turn);
        while seen.turns.load(Ordering::SeqCst) < 3 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop((lake, doors));
        let stopped = ending(ended, Duration::from_secs(10)).await;
        assert!(stopped.is_err(), "{on:?}: a runner whose lake went stops");
        let done = seen.turns.load(Ordering::SeqCst);
        assert!(done < of, "{on:?}: stopped at {done} of {of}");

        let (tx, ended) = oneshot::channel();
        *lock(&seen.ended) = Some(tx);
        let at = Instant::now();
        let lake = place.open(OpenMode::Write).await;
        let opened_in = at.elapsed();
        let left = {
            let view = lake.engine.view(None).expect("a view");
            kept(view.as_ref()).expect("the count is kept").0
        };
        assert!(
            left < of,
            "{on:?}: the open answered before the job was whole"
        );
        ending(ended, Duration::from_secs(30))
            .await
            .expect("the open finishes it");
        let view = lake.engine.view(None).expect("a view");
        let (counted, ..) = kept(view.as_ref()).expect("the count is kept");
        assert_eq!(counted, of, "{on:?}: counted to its end");
        assert_eq!(
            seen.turns.load(Ordering::SeqCst),
            of,
            "{on:?}: no step taken twice"
        );
        eprintln!("road {on:?}: stopped at {done} of {of}; the open answered in {opened_in:?}");
    }
}

/// The writes after a job's view reach its first turn, under the writer,
/// before anything else it does; a write about a type it does not reach
/// does not. Past one turn's read (`limits.part_tail`) they are folded
/// off the writer first, and the first turn reads only what came after.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_tail_past_the_view_reaches_the_first_turn() {
    for on in engines() {
        for (bound, notes) in [(1_000, 1), (2, 5)] {
            let mut place = Place::new(on, "tail");
            place.location.limits.part_tail = bound;
            let (lake, doors) = lake_of(&place).await;
            let seen = seen(lake.id.as_str());
            let (says, mut said) = mpsc::unbounded_channel();
            let (go, went) = std::sync::mpsc::channel();
            *lock(&seen.pause) = Some(Pause {
                at: "walk",
                says,
                go: went,
            });
            let (_, ended) = count(&lake, 1, Duration::ZERO);
            said.recv().await.expect("the counter walks");
            for i in 0..notes {
                doors
                    .upsert("note", &format!("n{i}"), json!({"n": "x"}))
                    .await
                    .expect("a note");
            }
            doors
                .upsert("other", "o", json!({"n": "x"}))
                .await
                .expect("another write");
            go.send(()).expect("the counter waits");
            ending(ended, Duration::from_secs(10))
                .await
                .expect("the counter is whole");
            let folds = lock(&seen.folds).clone();
            let keys: Vec<String> = (0..notes).map(|i| format!("n{i}")).collect();
            // Off the writer in pieces of one turn's read, then the first
            // turn's own read, which is empty.
            let want = if notes as u32 > bound {
                let mut pieces: Vec<Vec<String>> = keys
                    .chunks(bound as usize)
                    .map(<[String]>::to_vec)
                    .collect();
                pieces.push(Vec::new());
                pieces
            } else {
                vec![keys]
            };
            assert_eq!(
                folds, want,
                "{on:?}: {notes} notes past the view, {bound} a turn"
            );
            assert_eq!(
                seen.turns.load(Ordering::SeqCst),
                1,
                "{on:?}: it turned once"
            );
        }
    }
}

/// A lake written faster than a job's view moves: each first turn finds
/// the tail past the view longer than one turn reads, and after
/// [`TRIES`] the job answers unavailable, its turn never taken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tail_that_outruns_the_view_ends_the_job_unavailable() {
    for on in engines() {
        let mut place = Place::new(on, "overrun");
        place.location.limits.part_tail = 2;
        let (lake, doors) = lake_of(&place).await;
        let seen = seen(lake.id.as_str());
        let (says, mut said) = mpsc::unbounded_channel();
        let (go, went) = std::sync::mpsc::channel();
        *lock(&seen.pause) = Some(Pause {
            at: "reach",
            says,
            go: went,
        });
        let (_, mut ended) = count(&lake, 1, Duration::ZERO);
        let mut written = 0;
        let ended = loop {
            tokio::select! {
                end = &mut ended => break end.expect("the counter ends"),
                Some(_) = said.recv() => {
                    for _ in 0..3 {
                        let key = format!("n{written}");
                        doors.upsert("note", &key, json!({"n": "x"})).await.expect("a note");
                        written += 1;
                    }
                    go.send(()).expect("the counter waits");
                }
            }
        };
        let err = ended.expect_err("the job overran");
        let want = Situation::TailOverran {
            tries: TRIES,
            bound: 2,
        };
        assert_eq!(err.situation(), Some(&want), "{on:?}: {err}");
        assert_eq!(
            seen.turns.load(Ordering::SeqCst),
            0,
            "{on:?}: no turn was taken"
        );
        assert_eq!(written, 3 * TRIES, "{on:?}: a burst before each try");
    }
}

/// A standby that takes the lease from a writer whose last handle went
/// finishes what that writer's runner left, as a writer's open does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_standby_that_takes_the_lease_finishes_what_the_writer_left() {
    let (of, turn) = (12, Duration::from_millis(40));
    for on in engines() {
        let place = Place::new(on, "standby");
        let (lake, doors) = lake_of(&place).await;
        let (seen, ended) = count(&lake, of, turn);
        while seen.turns.load(Ordering::SeqCst) < 3 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let (at, seams, every) = (place.location.clone(), place.seams.clone(), turn / 2);
        let (standing, tries) = standing_by(at, seams, every, KINDS)
            .await
            .expect("the standby opens");
        drop((lake, doors));
        let stopped = ending(ended, Duration::from_secs(10)).await;
        assert!(stopped.is_err(), "{on:?}: a runner whose lake went stops");
        let done = seen.turns.load(Ordering::SeqCst);
        assert!(done < of, "{on:?}: stopped at {done} of {of}");

        let (tx, ended) = oneshot::channel();
        *lock(&seen.ended) = Some(tx);
        tokio::time::timeout(Duration::from_secs(10), tries)
            .await
            .expect("the standby takes the lease in time")
            .expect("the standby takes the lease");
        ending(ended, Duration::from_secs(30))
            .await
            .expect("the standby finishes it");
        let view = standing.engine.view(None).expect("a view");
        let (counted, ..) = kept(view.as_ref()).expect("the count is kept");
        assert_eq!(counted, of, "{on:?}: counted to its end");
        assert_eq!(
            seen.turns.load(Ordering::SeqCst),
            of,
            "{on:?}: no step taken twice"
        );
    }
}

/// A lake with a type `fed`, whose field only a source writes.
async fn fed(doors: &Doors) {
    let decl = fx::type_decl("fed", vec![fx::sourced("n", StorageKind::Text)]);
    doors.declare(decl).await.expect("the type is declared");
}

/// `doors`' source `source` feeds a record of `fed` at `key`.
async fn feed(doors: &Doors, source: &str, key: &str) {
    let record = fx::source_record("fed", key, json!({"n": "x"}));
    doors
        .ingest(source, vec![record])
        .await
        .expect("the source feeds");
}

/// A job that reaches by source is reached by every write of that source
/// past its view, by the log index's rows of the source, and by no write
/// of another source or of the owner, whatever type it is about.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_tail_reaches_a_job_by_its_source() {
    for on in engines() {
        for (bound, fed_in) in [(1_000, 1), (1, 3)] {
            let mut place = Place::new(on, "source");
            place.location.limits.part_tail = bound;
            let (lake, doors) = lake_of(&place).await;
            fed(&doors).await;
            let seen = seen(lake.id.as_str());
            let (says, mut said) = mpsc::unbounded_channel();
            let (go, went) = std::sync::mpsc::channel();
            *lock(&seen.pause) = Some(Pause {
                at: "walk",
                says,
                go: went,
            });
            let reach = Reach {
                sources: vec!["s1".into()],
                ..Reach::default()
            };
            let (_, ended) = count_reaching(&lake, 1, Duration::ZERO, reach);
            said.recv().await.expect("the counter walks");
            for i in 0..fed_in {
                feed(&doors, "s1", &format!("a{i}")).await;
                feed(&doors, "s2", &format!("b{i}")).await;
            }
            doors
                .upsert("note", "n", json!({"n": "x"}))
                .await
                .expect("the owner writes");
            go.send(()).expect("the counter waits");
            ending(ended, Duration::from_secs(10))
                .await
                .expect("the counter is whole");
            let folded: Vec<String> = lock(&seen.folds).concat();
            let want: Vec<String> = (0..fed_in).map(|i| format!("a{i}")).collect();
            assert_eq!(
                folded, want,
                "{on:?}: {fed_in} records of s1 past the view, {bound} a turn"
            );
        }
    }
}

/// Where the index decides nothing the log is read, and it answers what
/// the index answers; a type's keys narrow the index's rows, so the bound
/// counts only the events that may reach the job, each once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_log_answers_where_the_index_decides_nothing() {
    for on in engines() {
        let place = Place::new(on, "logged");
        let (lake, doors) = lake_of(&place).await;
        fed(&doors).await;
        let view = lake.engine.view(None).expect("a view");
        let past = view.position();
        drop(view);
        for i in 0..5 {
            let key = format!("n{i}");
            doors
                .upsert("note", &key, json!({"n": "x"}))
                .await
                .expect("a note");
        }
        doors
            .upsert("other", "o", json!({"n": "x"}))
            .await
            .expect("another");
        feed(&doors, "s1", "n1").await;
        feed(&doors, "s2", "b").await;
        let reach = Reach {
            sources: vec!["s1".into()],
            types: vec!["note".into()],
            keys: ["n1".into(), "n3".into()].into(),
        };
        let view = lake.engine.view(None).expect("a view");
        let keys = |read: Option<Vec<Event>>| {
            read.map(|r| {
                let about = r.iter().flat_map(|e| lakelands_core::feed::about(&e.body));
                about
                    .filter_map(|a| a.key.map(|k| k.to_string()))
                    .collect::<Vec<_>>()
            })
        };
        let indexed = read_tail(&lake, view.as_ref(), &reach, past, 1_000).expect("a read");
        let want = Some(vec!["n1".to_owned(), "n3".to_owned(), "n1".to_owned()]);
        assert_eq!(keys(indexed), want, "{on:?}: by the index");
        let from = past.map_or(Position::GENESIS, Position::next);
        let head = view.position().expect("a head");
        let read = logged(&lake, &reach, from, head, 1_000).expect("a read");
        assert_eq!(keys(read), want, "{on:?}: by the log");

        let three = read_tail(&lake, view.as_ref(), &reach, past, 3).expect("a read");
        assert!(three.is_some(), "{on:?}: three events may reach the job");
        let two = read_tail(&lake, view.as_ref(), &reach, past, 2).expect("a read");
        assert!(two.is_none(), "{on:?}: more than two");
        let all = logged(&lake, &reach, from, head, 3).expect("a read");
        assert!(all.is_none(), "{on:?}: the log counts every event");
    }
}

/// A setting's take-back stopped between two chunks of its drop is owed in
/// the projection's mark `dropping`, and the next writer's open finishes
/// the drop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_drop_stopped_between_chunks_is_finished_by_the_next_open() {
    for on in engines() {
        let mut place = Place::new(on, "drop-stopped");
        place.location.limits.order_chunk = 1;
        let (lake, doors) = lake_of(&place).await;
        let decl = fx::type_decl("row", vec![fx::sourced("n", StorageKind::Text)]);
        doors.declare(decl).await.expect("the type is declared");
        let rows = (0..2_000)
            .map(|i| fx::source_record("row", &format!("n{i:05}"), json!({"n": format!("{i:05}")})))
            .collect();
        doors.ingest("load", rows).await.expect("the rows");
        doors
            .set("sort_index.row", json!(["n"]))
            .await
            .expect("the setting is whole");
        let view = lake.engine.view(None).expect("a view");
        let built = view.chosen(&"row".into()).expect("the chosen");
        assert!(
            built.setting_building.is_empty(),
            "{on:?}: the build is not whole, so there is no setting to take back"
        );
        drop(view);
        let mut take_back =
            tokio::spawn(async move { doors.retire(SETTING_TYPE, "sort_index.row").await });
        // The retire's fold marks the field `dropping` before the door's
        // drop runs its chunks: stop the lake there, as a process that died.
        let mut marked = false;
        for _ in 0..30_000 {
            let view = lake.engine.view(None).expect("a view");
            let chosen = view.chosen(&"row".into()).expect("the chosen");
            if !chosen.dropping.is_empty() {
                marked = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        if !marked && take_back.is_finished() {
            let answer = (&mut take_back).await;
            panic!("{on:?}: the take-back answered {answer:?} before any mark");
        }
        assert!(marked, "{on:?}: the retire marked no field dropping");
        take_back.abort();
        let _ = take_back.await;
        drop(lake);

        let lake = place.open_when_free(OpenMode::Write).await;
        let view = lake.engine.view(None).expect("a view");
        let left = view.chosen(&"row".into()).expect("the chosen");
        assert!(
            !left.dropping.is_empty(),
            "{on:?}: the drop was whole before the open, so nothing was owed"
        );
        drop(view);
        let at = Instant::now();
        let mut cleared = false;
        while at.elapsed() < Duration::from_secs(60) {
            let view = lake.engine.view(None).expect("a view");
            let chosen = view.chosen(&"row".into()).expect("the chosen");
            if chosen.dropping.is_empty() {
                cleared = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            cleared,
            "{on:?}: the open never finished the drop; `n` is still dropping"
        );
        // A writer whose drop is whole owes none: a second open finds no
        // mark and starts nothing.
        drop(lake);
        let lake = place.open(OpenMode::Write).await;
        let view = lake.engine.view(None).expect("a view");
        let chosen = view.chosen(&"row".into()).expect("the chosen");
        assert!(
            chosen.dropping.is_empty(),
            "{on:?}: a reopened lake owes a drop it already finished"
        );
        assert!(
            chosen.setting_building.is_empty(),
            "{on:?}: a take-back left a build owed"
        );
    }
}

/// A setting's build stopped between any two of its turns is owed in the
/// projection's mark, and the next writer's open finishes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_setting_build_stopped_between_turns_is_finished_by_the_next_open() {
    for on in engines() {
        let mut place = Place::new(on, "build-stopped");
        place.location.limits.order_chunk = 1;
        let (lake, doors) = lake_of(&place).await;
        let decl = fx::type_decl("row", vec![fx::sourced("n", StorageKind::Text)]);
        doors.declare(decl).await.expect("the type is declared");
        let rows = (0..2_000)
            .map(|i| fx::source_record("row", &format!("n{i:05}"), json!({"n": format!("{i:05}")})))
            .collect();
        doors.ingest("load", rows).await.expect("the rows");
        let mut set = tokio::spawn(async move { doors.set("sort_index.row", json!(["n"])).await });
        // The setting's append marks the field building before the door
        // sends the build: stop the lake there, as a process that died.
        let mut marked = false;
        for _ in 0..30_000 {
            let view = lake.engine.view(None).expect("a view");
            let chosen = view.chosen(&"row".into()).expect("the chosen");
            if !chosen.setting_building.is_empty() {
                marked = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        if !marked && set.is_finished() {
            let answer = (&mut set).await;
            panic!("{on:?}: the set answered {answer:?} before any mark");
        }
        assert!(marked, "{on:?}: the append marked no field building");
        set.abort();
        let _ = set.await;
        drop(lake);

        let lake = place.open_when_free(OpenMode::Write).await;
        let view = lake.engine.view(None).expect("a view");
        let left = view.chosen(&"row".into()).expect("the chosen");
        assert!(
            !left.setting_building.is_empty(),
            "{on:?}: the build was whole before the open, so nothing was owed"
        );
        drop(view);
        let at = Instant::now();
        let mut whole = false;
        while at.elapsed() < Duration::from_secs(60) {
            let view = lake.engine.view(None).expect("a view");
            let chosen = view.chosen(&"row".into()).expect("the chosen");
            if chosen.setting_building.is_empty() {
                whole = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            whole,
            "{on:?}: the open never finished the build; `n` is still building"
        );
        // A writer whose build is whole owes none: a second open, taken
        // once the road's last turn has let the lake go, finds no mark and
        // starts nothing. The handle that held the lease lets it go a
        // moment after the drop, and PostgreSQL takes longer to say so, so
        // the open waits as the first one after the stop did.
        drop(lake);
        let lake = place.open_when_free(OpenMode::Write).await;
        let view = lake.engine.view(None).expect("a view");
        let chosen = view.chosen(&"row".into()).expect("the chosen");
        assert!(
            chosen.setting_building.is_empty(),
            "{on:?}: a reopened lake owes a build it already finished"
        );
    }
}

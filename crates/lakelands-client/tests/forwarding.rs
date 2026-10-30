//! The forwarding handle: the door checks and the statement checks
//! through one on SQLite and on PostgreSQL, and what they do not reach —
//! a door that passes the call on, a writer lost after the call was sent,
//! the write context crossing the link, and a read that waits for the
//! handle's own write.
//!
//! Each check's lake is opened twice on one engine: once for writing,
//! served by `serve_http` on loopback with the forwarder among its
//! `hosts`, and once without the lease, under a `ForwardingLake` that
//! names that server as its writer door. The check runs against the
//! forwarding handle; what it reads comes from the second handle's
//! engine, what it writes from the first's door.
//!
//! PostgreSQL is named by `LAKELANDS_TEST_PG_URL`, read here, in test
//! code; without it those checks stand down, and `scripts/conformance.sh
//! pg` runs them against one. Every lake on it gets a schema of its own,
//! dropped when its check ends.

mod common;

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use lakelands_api::engine::OpenMode;
use lakelands_api::error::{Error, Exchange, Remote, Situation};
use lakelands_api::id::{Position, RecordKey, SourceId, TypeName};
use lakelands_api::location::{EngineConfig, PgTls, PgTlsMode, PgTuning, WriterDoor};
use lakelands_api::principal::WriteOrigin;
use lakelands_api::read::Get;
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::time::Timestamp;
use lakelands_api::write::{Outcome, WriteOpts};
use lakelands_api::{LakeStore, Location, SourceRecord, WriteContext};
use lakelands_client::{ForwardConfig, ForwardingLake};
use lakelands_door_checks::checks::{patch, setup};
use lakelands_door_checks::statements::{InProcess, Statements};
use lakelands_door_checks::stream;
use lakelands_server::HostEntry;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// The forwarder's key, among the writer's `hosts`.
const HOST_KEY: &str = "h-forwarder";

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a runtime")
    })
}

/// A fresh lake name: every check gets its own lake.
fn fresh() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "finance-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn host() -> HostEntry {
    HostEntry {
        id: "forwarder".into(),
        keys: vec![HOST_KEY.into()],
        lakes: Vec::new(),
        principals: None,
    }
}

async fn open(location: Location, mode: OpenMode) -> Arc<dyn LakeStore> {
    let seams = Seams {
        engines: vec![
            Arc::new(lakelands_engine_sqlite::SqliteOpener),
            Arc::new(lakelands_engine_pg::PgOpener),
        ],
        ..Seams::default()
    };
    lakelands_store::open(location, seams, mode)
        .await
        .expect("the lake opens")
}

fn sqlite_at() -> Location {
    let lake = fresh();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("forwarding")
        .join(&lake);
    let _ = std::fs::remove_dir_all(&dir);
    Location::sqlite(lake, dir)
}

/// The test server's URL, if one is configured.
fn pg_url() -> Option<String> {
    std::env::var("LAKELANDS_TEST_PG_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
}

fn pg_at(url: &str) -> Location {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let schema = format!(
        "lakelands_test_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    Location {
        engine: EngineConfig::Postgres {
            url: url.to_owned(),
            schema: Some(schema),
            tls: PgTls {
                mode: PgTlsMode::Disable,
                ..PgTls::default()
            },
            tuning: PgTuning::default(),
            tablespaces: Default::default(),
        },
        ..Location::memory(fresh())
    }
}

/// What a lake holds open beside its forwarding handle, released when
/// the test that made it ends.
struct Held {
    writer: Arc<dyn LakeStore>,
    local: Arc<dyn LakeStore>,
    server: JoinHandle<()>,
    location: Location,
}

impl Held {
    async fn release(self) {
        self.server.abort();
        let _ = self.server.await;
        let _ = self.local.close().await;
        let _ = self.writer.close().await;
        if let EngineConfig::Postgres {
            url,
            schema: Some(schema),
            ..
        } = &self.location.engine
        {
            drop_schema(url, schema);
        }
    }
}

/// Drop a test lake's schema. On a thread of its own: the synchronous
/// client blocks on a runtime of its own, which an async body cannot.
fn drop_schema(url: &str, schema: &str) {
    let (url, schema) = (url.to_owned(), schema.to_owned());
    let _ = std::thread::spawn(move || {
        if let Ok(mut client) = postgres::Client::connect(&url, postgres::NoTls) {
            let _ = client.batch_execute(&format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE"));
        }
    })
    .join();
}

/// A lake at `location`: its writer served on loopback, and a forwarding
/// handle over a second handle without the lease.
async fn forwarding(location: Location) -> (Arc<dyn LakeStore>, Held) {
    let writer = open(location.clone(), OpenMode::Create).await;
    let (addr, _, server) = common::serve_hosting(writer.clone(), 20, vec![host()]).await;
    let local = open(location.clone(), OpenMode::Read).await;
    let at = Location {
        writers: vec![WriterDoor::new(format!("http://{addr}/"))],
        ..location.clone()
    };
    let handle = ForwardingLake::new(local.clone(), &at, ForwardConfig::new(HOST_KEY))
        .expect("the handle is configured");
    let held = Held {
        writer,
        local,
        server,
        location,
    };
    (Arc::new(handle), held)
}

thread_local! {
    /// What the check on this thread holds open.
    static HELD: RefCell<Vec<Held>> = const { RefCell::new(Vec::new()) };
}

/// A forwarding handle for a door check, its lake released when the
/// check ends.
fn for_check(location: Location) -> Arc<dyn LakeStore> {
    let (handle, held) = runtime().block_on(forwarding(location));
    HELD.with(|h| h.borrow_mut().push(held));
    handle
}

/// Run a door check, then release its lake, whether it passed or not.
fn block_on<F: std::future::Future<Output = ()>>(check: F) {
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            for held in HELD.with(|h| std::mem::take(&mut *h.borrow_mut())) {
                runtime().block_on(held.release());
            }
        }
    }
    let _release = Release;
    runtime().block_on(check)
}

mod on_sqlite {
    use super::*;

    fn sqlite() -> Arc<dyn LakeStore> {
        for_check(sqlite_at())
    }

    lakelands_door_checks::door_checks!(sqlite, block_on, forward);
    lakelands_door_checks::statement_checks!(|| in_process(sqlite()), block_on);
}

mod on_pg {
    use super::*;

    /// A forwarding handle on PostgreSQL; without one, a lake in memory
    /// that `run` never hands its check.
    fn pg() -> Arc<dyn LakeStore> {
        match pg_url() {
            Some(url) => for_check(pg_at(&url)),
            None => runtime().block_on(common::memory_lake(&fresh(), Default::default())),
        }
    }

    /// A check on PostgreSQL, standing down without one.
    fn run<F: std::future::Future<Output = ()>>(check: F) {
        if pg_url().is_none() {
            eprintln!("LAKELANDS_TEST_PG_URL is not set; no PostgreSQL to run against");
            return;
        }
        block_on(check)
    }

    lakelands_door_checks::door_checks!(pg, run, forward);
    lakelands_door_checks::statement_checks!(|| in_process(pg()), run);
}

/// A forwarding handle and the statement door of a host that holds it,
/// translating the statement itself.
fn in_process(lake: Arc<dyn LakeStore>) -> (Arc<dyn LakeStore>, Arc<dyn Statements>) {
    let door = InProcess(lake.catalog(WriteContext::default()));
    (lake, Arc::new(door))
}

fn get(k: &str) -> Get {
    Get {
        key: RecordKey::from(k),
        with_meta: false,
        at: Default::default(),
        params: Default::default(),
    }
}

fn opts(key: Option<&str>) -> WriteOpts {
    WriteOpts {
        expected_version: None,
        idempotency_key: key.map(Into::into),
        defer_computed_fields: false,
    }
}

fn position(o: &Outcome) -> Position {
    match o {
        Outcome::Applied { position, .. } => *position,
        o => panic!("not applied: {o:?}"),
    }
}

/// A write through the handle is read back through it at once: the
/// handle reads its own engine once it has folded what the writer
/// answered it.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_through_the_handle_is_read_back_through_it() {
    let (handle, held) = forwarding(sqlite_at()).await;
    let f = setup(handle.clone()).await;
    let (invoice, k) = (TypeName::from("invoice"), RecordKey::from("INV-1"));
    let o = (f.catalog)
        .upsert(
            &f.cast.clerk,
            &invoice,
            &k,
            patch(&[("status", json!("open"))]),
            opts(None),
        )
        .await
        .unwrap();
    let r = f
        .catalog
        .get(&f.cast.clerk, &invoice, get("INV-1"))
        .await
        .unwrap();
    assert_eq!(r.data.fields["status"], json!("open"));
    assert!(
        r.position >= position(&o),
        "the read is from after the write"
    );
    assert!(handle.head().await.unwrap() >= position(&o));
    assert!(!handle.is_writer());
    held.release().await;
}

/// The write context the handle's host gives its catalog is the one the
/// writer's log carries, whom the write is for with it (088).
#[tokio::test(flavor = "multi_thread")]
async fn the_write_context_crosses_to_the_writer() {
    let (handle, held) = forwarding(sqlite_at()).await;
    let f = setup(handle.clone()).await;
    let context = WriteContext {
        origin: WriteOrigin::Reactive,
        depth: 1,
        cause: Some(Position(1)),
        on_behalf_of: Some("human:alice".into()),
    };
    let o = (handle.catalog(context.clone()))
        .upsert(
            &f.cast.clerk,
            &TypeName::from("invoice"),
            &RecordKey::from("INV-1"),
            patch(&[("status", json!("open"))]),
            opts(None),
        )
        .await
        .unwrap();
    let at = position(&o);
    let batch = (held.writer.feed())
        .poll(Position(at.0 - 1), &FeedFilter::default(), 10)
        .await
        .unwrap();
    let event = (batch.events.iter())
        .find(|e| e.position == at)
        .expect("the write's event");
    // The cause and whom the write is for are the envelope's, never the
    // context's.
    let carried = WriteContext {
        cause: None,
        on_behalf_of: None,
        ..context.clone()
    };
    assert_eq!(
        (&event.context, event.cause, &event.on_behalf_of),
        (&carried, context.cause, &context.on_behalf_of)
    );
    held.release().await;
}

/// The address of a door nothing listens at: connecting is refused.
async fn nobody() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}/")
}

/// A handle over `local` to `doors`, in order.
fn to(local: &Arc<dyn LakeStore>, at: &Location, doors: Vec<String>) -> ForwardingLake {
    let at = Location {
        writers: doors.into_iter().map(WriterDoor::new).collect(),
        ..at.clone()
    };
    ForwardingLake::new(local.clone(), &at, ForwardConfig::new(HOST_KEY)).unwrap()
}

/// A door that was not reached, and one served by a handle without the
/// lease, pass the call to the next; with no door that holds the lease,
/// the call is `NotForwarded`, naming how many were tried, and nothing
/// was written.
#[tokio::test(flavor = "multi_thread")]
async fn a_door_without_the_lease_passes_the_call_on() {
    let location = sqlite_at();
    let (_, held) = forwarding(location.clone()).await;
    let standby = open(location.clone(), OpenMode::Read).await;
    let (standby_at, _, standby_server) =
        common::serve_hosting(standby.clone(), 20, vec![host()]).await;
    let standby_url = format!("http://{standby_at}/");
    let writer_url = {
        // The writer's door, as the held handle names it.
        let (addr, _, server) = common::serve_hosting(held.writer.clone(), 20, vec![host()]).await;
        (format!("http://{addr}/"), server)
    };
    let local = open(location.clone(), OpenMode::Read).await;
    let all = to(
        &local,
        &location,
        vec![nobody().await, standby_url.clone(), writer_url.0.clone()],
    );
    let f = setup(Arc::new(all)).await;
    let head = held.writer.head().await.unwrap();
    assert!(head > Position(0), "the declarations reached the writer");

    let none = to(&local, &location, vec![nobody().await, standby_url]);
    let e = (none.catalog(WriteContext::default()))
        .upsert(
            &f.cast.clerk,
            &TypeName::from("invoice"),
            &RecordKey::from("INV-1"),
            patch(&[("status", json!("open"))]),
            opts(None),
        )
        .await
        .unwrap_err();
    match e.situation() {
        Some(Situation::NotForwarded { doors: 2, last, .. }) => {
            let last = last.as_deref().and_then(Error::situation);
            assert!(matches!(last, Some(Situation::ReadOnly { .. })), "{last:?}");
        }
        other => panic!("not NotForwarded: {other:?}"),
    }
    assert_eq!(
        held.writer.head().await.unwrap(),
        head,
        "nothing was written"
    );
    standby_server.abort();
    writer_url.1.abort();
    let _ = standby.close().await;
    let _ = local.close().await;
    held.release().await;
}

/// A door that takes the call, passes it to `writer`, and cuts the
/// connection once the writer begins to answer: the call landed, and its
/// answer was lost.
async fn cutting(writer: std::net::SocketAddr) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let Ok((mut c, _)) = listener.accept().await else {
            return;
        };
        let mut w = TcpStream::connect(writer).await.unwrap();
        let (mut buf, mut answer) = ([0u8; 1 << 16], [0u8; 1]);
        loop {
            tokio::select! {
                n = c.read(&mut buf) => match n {
                    Ok(n) if n > 0 => w.write_all(&buf[..n]).await.unwrap(),
                    _ => return,
                },
                _ = w.read(&mut answer) => return,
            }
        }
    });
    format!("http://{addr}/")
}

/// A door that counts the connections it takes and answers none.
async fn counting() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let taken = Arc::new(AtomicUsize::new(0));
    let count = taken.clone();
    tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((s, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            open.push(s);
        }
    });
    (format!("http://{addr}/"), taken)
}

/// A writer lost after the call was sent: the call is `ForwardLost`, and
/// it is not sent to the next door. A retry under its idempotency key,
/// on a writer that holds the lease, answers the first outcome and
/// appends nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_writer_lost_after_the_call_is_sent_is_not_sent_again() {
    let location = sqlite_at();
    let (handle, held) = forwarding(location.clone()).await;
    let f = setup(handle).await;
    let (addr, _, server) = common::serve_hosting(held.writer.clone(), 20, vec![host()]).await;
    let (next, taken) = counting().await;
    let local = open(location.clone(), OpenMode::Read).await;
    let lost = to(&local, &location, vec![cutting(addr).await, next]);
    let (invoice, k) = (TypeName::from("invoice"), RecordKey::from("INV-1"));
    let write = |store: &dyn LakeStore| store.catalog(WriteContext::default());
    let before = held.writer.head().await.unwrap();
    let e = write(&lost)
        .upsert(
            &f.cast.clerk,
            &invoice,
            &k,
            patch(&[("status", json!("open"))]),
            opts(Some("once")),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(e.situation(), Some(Situation::ForwardLost { .. })),
        "{e:?}"
    );
    assert_eq!(
        taken.load(Ordering::SeqCst),
        0,
        "nothing was sent to the next door"
    );
    let landed = held.writer.head().await.unwrap();
    assert!(landed > before, "the call landed");

    let again = to(&local, &location, vec![format!("http://{addr}/")]);
    let o = write(&again)
        .upsert(
            &f.cast.clerk,
            &invoice,
            &k,
            patch(&[("status", json!("open"))]),
            opts(Some("once")),
        )
        .await
        .unwrap();
    assert_eq!(position(&o), landed, "the retry answers the first outcome");
    assert_eq!(
        held.writer.head().await.unwrap(),
        landed,
        "and appends nothing"
    );
    server.abort();
    let _ = local.close().await;
    held.release().await;
}

/// A door that answers every call with `refusal`, as a writer whose own
/// remote end failed, naming position `at`, and counts the calls.
async fn refusing(refusal: &Error, at: u64) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = serde_json::to_string(refusal).unwrap();
    let answer = format!(
        "HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json\r\n\
         lakelands-position: {at}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let taken = Arc::new(AtomicUsize::new(0));
    let count = taken.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 1 << 16];
            let _ = s.read(&mut buf).await;
            let _ = s.write_all(answer.as_bytes()).await;
        }
    });
    (format!("http://{addr}/"), taken)
}

/// A writer that answers with its own refusal of a remote end, as when
/// its verb's executor fails, answers the call: the refusal is the
/// caller's as it came, and the call goes to no other door. A read after
/// it waits for the position that answer named, and no longer.
#[tokio::test(flavor = "multi_thread")]
async fn a_writers_own_remote_refusal_is_the_calls_answer() {
    let location = sqlite_at();
    let (handle, held) = forwarding(location.clone()).await;
    let f = setup(handle).await;
    let refusal = Error::unavailable(Situation::Remote {
        remote: Remote::Executor {
            name: "mailer".into(),
        },
        exchange: Exchange::TimedOut { secs: 5 },
    });
    let ahead = held.writer.head().await.unwrap().0 + 1_000;
    let (door, asked) = refusing(&refusal, ahead).await;
    let (next, taken) = counting().await;
    let local = open(location.clone(), OpenMode::Read).await;
    let at = Location {
        writers: [door, next].into_iter().map(WriterDoor::new).collect(),
        ..location.clone()
    };
    let mut config = ForwardConfig::new(HOST_KEY);
    config.read_wait_millis = 200;
    let refused = ForwardingLake::new(local.clone(), &at, config).unwrap();
    let cat = refused.catalog(WriteContext::default());
    let (invoice, k) = (TypeName::from("invoice"), RecordKey::from("INV-1"));
    let e = cat
        .upsert(
            &f.cast.clerk,
            &invoice,
            &k,
            patch(&[("status", json!("open"))]),
            opts(None),
        )
        .await
        .unwrap_err();
    assert_eq!(e, refusal, "the writer's refusal, as it came");
    assert_eq!(asked.load(Ordering::SeqCst), 1, "the writer was asked once");
    assert_eq!(taken.load(Ordering::SeqCst), 0, "and no other door at all");

    let started = tokio::time::Instant::now();
    let read = cat.get(&f.cast.clerk, &invoice, get("INV-1")).await;
    let took = started.elapsed();
    assert!(
        matches!(
            read.as_ref().err().and_then(Error::situation),
            Some(Situation::NotFoldedYet { at, .. }) if at.0 == ahead
        ),
        "a read waits for the position the writer named: {read:?}"
    );
    let bound = Duration::from_millis(200);
    assert!(
        took >= bound && took < bound + Duration::from_millis(500),
        "and gives up at its bound: {took:?}"
    );
    let _ = local.close().await;
    held.release().await;
}

/// The six host operations that need the lease are `ReadOnly` through
/// the handle, whatever they would have done, the purge's record to
/// retire among them, and nothing is appended.
#[tokio::test(flavor = "multi_thread")]
async fn the_host_operations_that_need_the_lease_are_read_only() {
    let location = sqlite_at();
    let (handle, held) = forwarding(location).await;
    let f = setup(handle.clone()).await;
    let (invoice, k) = (TypeName::from("invoice"), RecordKey::from("INV-1"));
    (handle.catalog(WriteContext::default()))
        .upsert(
            &f.cast.clerk,
            &invoice,
            &k,
            patch(&[("status", json!("open"))]),
            opts(None),
        )
        .await
        .unwrap();
    // A record the purge would retire.
    let banked = SourceRecord {
        source: None,
        upstream: None,
        ty: invoice.clone(),
        key: RecordKey::from("INV-2"),
        fields: patch(&[("amount", json!("120.50"))]),
        seen_at: None,
        delivery: None,
        resubmits: None,
    };
    let mut acks = (f.catalog)
        .ingest(&f.cast.importer, &"bank".into(), stream::of(vec![banked]))
        .await
        .unwrap();
    for ack in stream::collect(&mut acks).await {
        ack.unwrap();
    }
    let head = held.writer.head().await.unwrap();
    let today = Timestamp::from_date(2026, 3, 1).unwrap();
    let answers = [
        ("rebuild", handle.rebuild().await.map(|_| ())),
        ("checkpoint", handle.checkpoint().await.map(|_| ())),
        (
            "purge_source",
            (handle
                .purge_source(&SourceId::from("bank"), None, None)
                .await)
                .map(|_| ()),
        ),
        (
            "restrict",
            (handle.restrict(&invoice, &k, "held".into()).await).map(|_| ()),
        ),
        (
            "unrestrict",
            handle.unrestrict(&invoice, &k).await.map(|_| ()),
        ),
        ("age", handle.age(today).await.map(|_| ())),
    ];
    for (op, answer) in answers {
        assert!(
            matches!(
                answer.as_ref().err().and_then(Error::situation),
                Some(Situation::ReadOnly { .. })
            ),
            "`{op}` through the handle: {answer:?}"
        );
    }
    assert_eq!(held.writer.head().await.unwrap(), head, "nothing appended");
    held.release().await;
}

/// The handle's own engine, beside the writer, reaches a write once it
/// is folded; a position it has not folded is `NotFoldedYet` at the
/// bound, naming what it has folded.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_waits_for_the_handles_writes_and_no_longer() {
    let location = sqlite_at();
    let (handle, held) = forwarding(location).await;
    let f = setup(handle.clone()).await;
    let head = held.writer.head().await.unwrap();
    assert!(
        held.local
            .reaches(head, Duration::from_secs(2))
            .await
            .unwrap()
            >= head
    );
    let past = Position(head.0 + 5);
    let bound = Duration::from_millis(200);
    let started = tokio::time::Instant::now();
    let e = held.local.reaches(past, bound).await.unwrap_err();
    let took = started.elapsed();
    match e.situation() {
        Some(Situation::NotFoldedYet { at, folded }) => {
            assert_eq!(*at, past);
            assert_eq!(*folded, head);
        }
        other => panic!("not NotFoldedYet: {other:?}"),
    }
    assert!(took >= bound, "it waited the bound: {took:?}");
    assert!(
        took < bound + Duration::from_millis(500),
        "and gave up at it: {took:?}"
    );
    drop(f);
    held.release().await;
}

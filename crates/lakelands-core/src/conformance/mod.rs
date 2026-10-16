//! The conformance suite: Lakelands' specification, as checks every
//! engine must pass.
//!
//! A behaviour a lake promises is claimed here, once, and not beside one
//! engine. Every check talks to a lake through the contract alone — a
//! [`LakeStore`] and the [`Catalog`](lakelands_api::Catalog) it hands out
//! — so a check that could only pass on one engine's storage proves
//! nothing and cannot be written. What is checked is meaning: that a
//! write touches only its fields, that a read at an earlier position sees
//! the lake as it was there, that a decimal stays exact, that a field the
//! principal may not write refuses the whole write. Each check names the
//! decision record or contract document that states its rule.
//!
//! # Running it
//!
//! An engine's test implements [`Harness`], which opens a brand-new,
//! empty lake for every check with the seams the runner hands it, and
//! calls [`run_all`]; the [`Report`]
//! lists every pass and every failure, and the test fails unless
//! [`Report::is_green`]. The suite never stops at the first failure, and
//! a check that panics is reported as failed rather than taking the
//! others down with it. It uses only std futures: the test that calls it
//! brings the async runtime.
//!
//! # Adding a check
//!
//! A check is one `async fn(Store) -> Held` whose name is a sentence in
//! snake_case, stating what must hold, and one `check!` line turning it
//! into a [`Check`] constant, whose doc is its `about`. Adding a check
//! is one new file in `checks/` (or one function in an existing file of
//! the same subject) plus one line in the registry,
//! [`checks::ALL`](crate::conformance::checks::ALL), in `checks/mod.rs`.
//! Nothing else registers a check, and nothing else may run one. The
//! helpers every check needs (principals, a small ontology, reading back)
//! are in [`fixture`].
//!
//! A failure message is written for the person who reads the report: it
//! says what was done, what came back and what the rule is, so the
//! engine's author can act on it without opening the check.
//!
//! The report keeps each check's [`Tally`]: the assertions it evaluated
//! and the places in the suite they are written at. A rewrite of checks
//! proves it dropped no comparison when every tally on one engine is
//! equal or higher after it (a check polling while writes run varies in
//! evaluations from run to run, never in places).

use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use futures_core::Stream;
use lakelands_api::LakeStore;
use lakelands_api::seam::Seams;

/// Turns an `async fn` of the suite into a [`Check`] constant whose
/// `name` is the function's own name and whose doc is its `about`, so
/// the line takes no `///` of its own. A trailing `, embedding` runs the
/// check on a lake with the suite's embedding backend bound; a trailing
/// `, reopens` hands the check the harness's [`Reopen`] as its second
/// argument, a trailing `, scratch` the directory of its own the
/// harness supplies ([`Harness::scratch`]), a trailing `, given` all
/// of [`Given`], and `, given, embedding` both.
macro_rules! check {
    ($konst:ident = $run:ident, $about:expr) => {
        check!(@make $konst = $run, $about, false, |store, _| Box::pin($run(store)));
    };
    ($konst:ident = $run:ident, $about:expr, embedding) => {
        check!(@make $konst = $run, $about, true, |store, _| Box::pin($run(store)));
    };
    ($konst:ident = $run:ident, $about:expr, reopens) => {
        check!(@make $konst = $run, $about, false,
            |store, given| Box::pin($run(store, given.reopen)));
    };
    ($konst:ident = $run:ident, $about:expr, scratch) => {
        check!(@make $konst = $run, $about, false,
            |store, given| Box::pin($run(store, given.scratch)));
    };
    ($konst:ident = $run:ident, $about:expr, given) => {
        check!(@make $konst = $run, $about, false,
            |store, given| Box::pin($run(store, given)));
    };
    ($konst:ident = $run:ident, $about:expr, given, embedding) => {
        check!(@make $konst = $run, $about, true,
            |store, given| Box::pin($run(store, given)));
    };
    (@make $konst:ident = $run:ident, $about:expr, $embedding:expr, $call:expr) => {
        #[doc = $about]
        pub const $konst: $crate::conformance::Check = $crate::conformance::Check {
            name: stringify!($run),
            about: $about,
            run: $call,
            embedding: $embedding,
        };
    };
}

/// Fails the check with a formatted message unless the condition holds.
macro_rules! ensure {
    ($cond:expr, $($msg:tt)+) => {{
        $crate::conformance::tally();
        if !$cond {
            return Err($crate::conformance::Failure::new(format!($($msg)+)));
        }
    }};
}

/// Fails the check unless the answer is a refusal as invalid whose case
/// matches the pattern:
/// `refused_as!(what, answer, Case::UnknownField { field, .. } if field == "x")`.
macro_rules! refused_as {
    ($what:expr, $answer:expr, $case:pat $(if $guard:expr)?) => {
        $crate::conformance::fixture::refused_as(
            $what,
            $answer,
            stringify!($case $(if $guard)?),
            |said| matches!(&said.case, $case $(if $guard)?),
        )
    };
}

pub mod backing;
pub mod checks;
pub mod engine;
pub mod fixture;

/// Opens lakes for the suite: one engine, as its test configures it.
///
/// The runner hands the harness the seams each lake binds besides the
/// engine: always [`backing::backings`], which the checks of functions
/// computed outside the store call, and [`backing::executors`], which the
/// checks of verbs reaching outside run, and [`backing::embedder`] for the
/// checks that need an embedding backend. The harness adds its engine's
/// opener and binds nothing else.
///
/// Every lake a harness opens has the default
/// [`Limits`](lakelands_api::location::Limits): the runner holds each
/// fresh lake to them ([`fixture::default_limits`]) before its check runs,
/// and the checks that meet a limit (a walk's depth, a page's size) read
/// it from [`fixture::limits`].
pub trait Harness: Send + Sync {
    /// The engine's name, as the report shows it.
    fn name(&self) -> &str;

    /// A brand-new, empty lake with `seams` and the engine. Every check
    /// gets its own, so no check sees another's writes and positions are
    /// the check's alone.
    fn fresh(&self, seams: Seams) -> FreshLake<'_>;

    /// How this harness opens a lake it handed out again: the [`Reopen`]
    /// a check that meets the lake's next writer is given.
    fn reopener(&self) -> Reopen;

    /// How this harness opens a lake it handed out again with other
    /// [`Limits`](lakelands_api::location::Limits): the [`Relimit`] a check
    /// that needs a lake past one of its ceilings is given.
    fn relimiter(&self) -> Relimit;

    /// A directory of the harness's own, which a check that writes files
    /// (an export) writes under: each such check is given the
    /// subdirectory named after it, and removes what it wrote. The suite
    /// reads no environment, so the engine's test chooses where it lies.
    fn scratch(&self) -> PathBuf;

    /// What the harness counts of its process's heap, for the checks that
    /// claim an operation over a whole lake runs in bounded memory. A
    /// harness without one runs those checks for what they claim besides
    /// memory, and they report the heap as not measured.
    fn meter(&self) -> Option<Arc<dyn Meter>> {
        None
    }

    /// How this harness imports an export (`lakelands_store::import`) as
    /// a lake of its engine: the [`Import`] a check of the import is
    /// given. A harness that imports nothing answers `unbound`.
    fn importer(&self) -> Import {
        Arc::new(|_, _, _, _| {
            Box::pin(async {
                let op = "import".to_owned();
                let seam = lakelands_api::error::Seam::Operation { op };
                Err(lakelands_api::Error::unbound(seam))
            })
        })
    }

    /// How this harness writes to a lake's log below the store and opens
    /// the lake again: the [`Continue`] a check of a lake that another
    /// release's writer left is given. A harness that reaches no engine
    /// answers `unbound`.
    fn continuer(&self) -> Continue {
        Arc::new(|_, _, _, _, _| {
            Box::pin(async {
                let op = "continue".to_owned();
                let seam = lakelands_api::error::Seam::Operation { op };
                Err(lakelands_api::Error::unbound(seam))
            })
        })
    }

    /// How this harness opens a lake it handed out again through an engine
    /// the check wraps around its own: the [`Fault`] a check that injects
    /// a fault below the store is given. A harness without one answers
    /// `unbound`.
    fn faulter(&self) -> Fault {
        Arc::new(|_, _, _| {
            Box::pin(async {
                let op = "fault".to_owned();
                let seam = lakelands_api::error::Seam::Operation { op };
                Err(lakelands_api::Error::unbound(seam))
            })
        })
    }

    /// How this harness opens a twin of a lake it handed out: a brand-new
    /// lake of its engine under the same id, in a place of its own
    /// ([`Twin`]). A harness without one answers `unbound`.
    fn twinner(&self) -> Twin {
        Arc::new(|_, _| {
            Box::pin(async {
                let op = "twin".to_owned();
                let seam = lakelands_api::error::Seam::Operation { op };
                Err(lakelands_api::Error::unbound(seam))
            })
        })
    }

    /// How many records the suite's large lake holds at its first size;
    /// it grows to four times this. [`LARGE_LAKE`] unless the engine's
    /// test asks for more, as a long run does.
    fn large_lake(&self) -> u32 {
        LARGE_LAKE
    }
}

/// The records of the suite's large lake at its first size, when the
/// harness does not name another: more than one page of the feed and of
/// an export at the default limits, small enough for every pull request.
pub const LARGE_LAKE: u32 = 2_000;

/// A count of the heap the process holds, kept by the engine's test
/// binary (its global allocator), so that the suite can measure what an
/// operation holds while it runs. The suite never installs an allocator.
pub trait Meter: Send + Sync {
    /// The bytes allocated and not yet freed, now.
    fn live(&self) -> u64;

    /// The most [`Meter::live`] has been since the last reset.
    fn peak(&self) -> u64;

    /// Start a new peak at what is live now.
    fn reset_peak(&self);
}

/// What the runner hands a check beside its lake: the harness's
/// [`Reopen`], the check's own directory under [`Harness::scratch`], and
/// the harness's [`Meter`] and large-lake size.
pub struct Given {
    /// How the harness opens the check's lake again.
    pub reopen: Reopen,
    /// How the harness opens the check's lake again with other limits.
    pub relimit: Relimit,
    /// A directory for the check alone; it may not exist yet.
    pub scratch: PathBuf,
    /// The harness's [`Meter`], if it has one.
    pub meter: Option<Arc<dyn Meter>>,
    /// The harness's [`Harness::large_lake`].
    pub large_lake: u32,
    /// The harness's name, for a check that reports figures.
    pub harness: String,
    /// How the harness imports an export as a lake of its engine.
    pub import: Import,
    /// How the harness writes to a lake's log below the store and opens
    /// the lake again.
    pub continued: Continue,
    /// How the harness opens a twin of the check's lake.
    pub twin: Twin,
    /// How the harness opens the check's lake again through a wrapped
    /// engine.
    pub fault: Fault,
}

/// Closes a lake the harness opened and opens it again for writing with
/// `seams` and the harness's engine, as a host's next process opens it:
/// the same lake, a new writer.
pub type Reopen = Arc<dyn Fn(Arc<dyn LakeStore>, Seams) -> FreshLake<'static> + Send + Sync>;

/// Closes a lake the harness opened and opens it again for writing, as
/// [`Reopen`] does, through the engine [`Wrap`] makes of the harness's
/// own: a fault a check injects below the store, on every engine
/// (lakelands#1161). It is a seam of the suite, not of the store.
pub type Fault = Arc<dyn Fn(Arc<dyn LakeStore>, Seams, Wrap) -> FreshLake<'static> + Send + Sync>;

/// What a check makes of the harness's engine opener for a [`Fault`].
pub type Wrap = Arc<
    dyn Fn(
            Arc<dyn lakelands_api::engine::EngineOpener>,
        ) -> Arc<dyn lakelands_api::engine::EngineOpener>
        + Send
        + Sync,
>;

/// Closes a lake the harness opened and opens it again for writing, as
/// [`Reopen`] does, with its location's limits replaced by these.
pub type Relimit = Arc<
    dyn Fn(Arc<dyn LakeStore>, Seams, lakelands_api::location::Limits) -> FreshLake<'static>
        + Send
        + Sync,
>;

/// Open a brand-new lake of the harness's engine under the id named, in a
/// place of its own beside every lake that bears it: the twin a check of
/// the rights grid builds as it builds its own lake, so that the two
/// answer alike everything that does not differ between them
/// (lakelands#962). It is a seam of the suite, not of the store.
pub type Twin = Arc<dyn Fn(lakelands_api::LakeId, Seams) -> FreshLake<'static> + Send + Sync>;

/// Import the export in a directory as the lake named, of the harness's
/// engine: into a brand-new lake, or, given a store, into the lake that
/// store holds, which is closed first (an import must refuse it when it
/// holds events).
pub type Import = Arc<
    dyn Fn(PathBuf, lakelands_api::LakeId, Option<Arc<dyn LakeStore>>, Seams) -> ImportFuture
        + Send
        + Sync,
>;

/// Closes the lake a store holds, if one is given, appends lines to the
/// log of the lake named, through its engine's seam and below the store,
/// as another release's writer would, and opens the lake through the
/// store in the mode named. A harness makes it of [`continue_below`] and
/// [`below`].
pub type Continue = Arc<
    dyn Fn(
            Option<Arc<dyn LakeStore>>,
            lakelands_api::LakeId,
            Lines,
            lakelands_api::engine::OpenMode,
            Seams,
        ) -> ContinueFuture
        + Send
        + Sync,
>;

/// The lines a [`Continue`] appends, made from the log's tip and the
/// fence of the lease they are appended under; none, to append nothing.
/// With them, how many positions past the tip they leave the projection
/// is then committed through, as no fold leaves it: 0 leaves the
/// projection where it is.
pub type Lines = Box<
    dyn FnOnce(
            (lakelands_api::Position, lakelands_api::event::Digest),
            u64,
        ) -> (Vec<lakelands_api::engine::CanonicalEvent>, u64)
        + Send,
>;

/// The future a [`Continue`] answers.
pub type ContinueFuture =
    Pin<Box<dyn Future<Output = lakelands_api::Result<Continued>> + Send + 'static>>;

/// What a [`Continue`] found.
pub struct Continued {
    /// What the store's open answered.
    pub opened: lakelands_api::Result<Arc<dyn LakeStore>>,
    /// The lake below the store after the lines were appended, before
    /// that open.
    pub before: Below,
    /// The lake below the store after that open, where it was refused.
    pub after: Option<Below>,
}

/// What a lake says below the store: its log's tip, the position its
/// projection is folded through, and whether its lease is free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Below {
    /// The log's tip.
    pub tip: Option<(lakelands_api::Position, lakelands_api::event::Digest)>,
    /// The position the projection is folded through.
    pub folded_through: Option<lakelands_api::Position>,
    /// Whether the lease could be taken, and was given back.
    pub lease_free: bool,
}

/// Reads a lake below the store, through its engine's seam: for a
/// harness's [`Continue`].
pub fn below(
    opener: &dyn lakelands_api::engine::EngineOpener,
    location: &lakelands_api::Location,
) -> lakelands_api::Result<Below> {
    let engine = opener.open(location, lakelands_api::engine::OpenMode::Write)?;
    let lease = engine.take_lease("the suite, below the store");
    if let Ok(lease) = &lease {
        engine.release_lease(lease)?;
    }
    Ok(Below {
        tip: engine.tip()?,
        folded_through: engine.projection().folded_through()?,
        lease_free: lease.is_ok(),
    })
}

/// Appends `lines` to a lake's log through its engine's seam, under a
/// lease taken for it and given back, commits the projection ahead of
/// the log where they ask it, and reads the lake [`below`]: for a
/// harness's [`Continue`].
pub fn continue_below(
    opener: &dyn lakelands_api::engine::EngineOpener,
    location: &lakelands_api::Location,
    lines: Lines,
) -> lakelands_api::Result<Below> {
    let engine = opener.open(location, lakelands_api::engine::OpenMode::Write)?;
    let lease = engine.take_lease("another release")?;
    let appended = match engine.tip()? {
        Some(tip) => {
            let (lines, ahead) = lines(tip, lease.fence);
            engine.append(&lease, &lines).and_then(|()| {
                let Some((tip, _)) = engine.tip()?.filter(|_| ahead > 0) else {
                    return Ok(());
                };
                let projection = engine.projection();
                projection.begin()?;
                projection.commit(lakelands_api::Position(tip.0 + ahead))
            })
        }
        None => Ok(()),
    };
    engine.release_lease(&lease)?;
    drop(engine);
    appended?;
    below(opener, location)
}

/// The future an [`Import`] answers.
pub type ImportFuture =
    Pin<Box<dyn Future<Output = lakelands_api::Result<ImportedLake>> + Send + 'static>>;

/// A lake an [`Import`] made: opened read-only, so that nothing is
/// appended to it, and how to open that lake for writing. It has the name
/// of the lake it came from, so a [`Reopen`], which finds a lake by name,
/// would find that one; `writer` finds this one.
pub struct ImportedLake {
    /// The imported lake, opened read-only.
    pub store: Arc<dyn LakeStore>,
    /// Closes a handle onto the imported lake and opens it for writing.
    pub writer: Reopen,
}

/// The future [`Harness::fresh`] answers: a new, empty lake, or why none
/// could be opened.
pub type FreshLake<'a> =
    Pin<Box<dyn Future<Output = lakelands_api::Result<Arc<dyn LakeStore>>> + Send + 'a>>;

/// The lake a check is handed: its store, as the contract sees it.
pub type Store = Arc<dyn LakeStore>;

/// What a check, or a step of one, answers: it held, or why not.
pub type Held = Result<(), Failure>;

/// The future one check runs as.
pub type CheckFuture = Pin<Box<dyn Future<Output = Held> + Send>>;

/// One behaviour every engine must honour.
#[derive(Clone, Copy)]
pub struct Check {
    /// A sentence in snake_case stating what must hold.
    pub name: &'static str,
    /// Where the rule is stated, and why it matters, in one sentence.
    pub about: &'static str,
    /// The check itself, run against a fresh lake; a check that reopens
    /// its lake uses the harness's [`Reopen`], one that writes files its
    /// scratch directory, and the others ignore what they are [`Given`].
    pub run: fn(Arc<dyn LakeStore>, Given) -> CheckFuture,
    /// Whether its lake binds the suite's embedding backend; without it,
    /// the lake has none.
    pub embedding: bool,
}

impl fmt::Debug for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Check").field("name", &self.name).finish()
    }
}

/// Why a check failed, for the person who reads the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// What was done, what came back, and the rule it breaks.
    pub message: String,
}

impl Failure {
    /// A failure with this message.
    pub fn new(message: impl Into<String>) -> Failure {
        Failure {
            message: message.into(),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

/// What a run of the suite found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The harness the suite ran against.
    pub harness: String,
    /// The checks that passed, in registry order.
    pub passed: Vec<&'static str>,
    /// The checks that failed, in registry order, with why.
    pub failed: Vec<(&'static str, Failure)>,
    /// Every check run, in registry order, with what it asserted.
    pub tallies: Vec<(&'static str, Tally)>,
}

impl Report {
    /// Whether every check passed.
    pub fn is_green(&self) -> bool {
        self.failed.is_empty()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "conformance on `{}`: {} passed, {} failed",
            self.harness,
            self.passed.len(),
            self.failed.len()
        )?;
        for (name, failure) in &self.failed {
            writeln!(f, "  FAILED {name}: {failure}")?;
        }
        Ok(())
    }
}

/// What one check asserted while it ran.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// The assertions evaluated, each time one was.
    pub evaluations: u64,
    /// The distinct places in the suite those assertions are written at.
    pub sites: usize,
}

/// What the check being run has asserted so far.
#[derive(Default)]
struct Counting {
    evaluations: u64,
    sites: HashSet<(&'static str, u32, u32)>,
}

thread_local! {
    static COUNTING: RefCell<Counting> = RefCell::default();
}

/// Counts one assertion of the check being run, at the place that wrote
/// it: `ensure!`, `refused_as!` and [`Must`](fixture::Must) call it.
#[track_caller]
pub fn tally() {
    let at = std::panic::Location::caller();
    COUNTING.with_borrow_mut(|c| {
        c.evaluations += 1;
        c.sites.insert((at.file(), at.line(), at.column()));
    });
}

/// Runs `step` with `counting` as this thread's count, so that checks
/// polled in turn on one thread, or on several, never mix their counts.
fn counting<T>(counting: &mut Counting, step: impl FnOnce() -> T) -> T {
    let outer = COUNTING.replace(std::mem::take(counting));
    let out = step();
    *counting = COUNTING.replace(outer);
    out
}

impl Counting {
    fn tally(&self) -> Tally {
        Tally {
            evaluations: self.evaluations,
            sites: self.sites.len(),
        }
    }
}

/// Drives `fut` with a count of its own, answering what it asserted too.
async fn counted<F: Future>(fut: F) -> (F::Output, Tally) {
    let (mut count, mut fut) = (Counting::default(), std::pin::pin!(fut));
    let out = std::future::poll_fn(|cx| counting(&mut count, || fut.as_mut().poll(cx))).await;
    (out, count.tally())
}

/// Runs every registered check against fresh lakes from `harness`, in
/// registry order, and reports them all.
pub async fn run_all(harness: &dyn Harness) -> Report {
    run_checks(harness, checks::ALL).await
}

/// Runs the one registered check called `name`; `None` if there is none.
pub async fn run_one(harness: &dyn Harness, name: &str) -> Option<Result<(), Failure>> {
    let check = checks::ALL.iter().find(|c| c.name == name)?;
    Some(run_check(harness, check).await)
}

/// Runs `checks` in order, never stopping at a failure.
async fn run_checks(harness: &dyn Harness, checks: &[Check]) -> Report {
    let mut report = Report {
        harness: harness.name().to_owned(),
        ..Report::default()
    };
    for check in checks {
        let (outcome, tally) = counted(run_check(harness, check)).await;
        report.tallies.push((check.name, tally));
        match outcome {
            Ok(()) => report.passed.push(check.name),
            Err(failure) => report.failed.push((check.name, failure)),
        }
    }
    report
}

/// Runs one check on a fresh lake. A harness that cannot open one, and a
/// check that panics, are failures of that check.
async fn run_check(harness: &dyn Harness, check: &Check) -> Result<(), Failure> {
    let seams = Seams {
        backings: backing::backings(),
        embed: check.embedding.then(backing::embedder),
        executors: backing::executors(),
        ..Seams::default()
    };
    let store = match caught(harness.fresh(seams)).await {
        Ok(Ok(store)) => store,
        Ok(Err(e)) => {
            return Err(Failure::new(format!(
                "the harness could not open a fresh lake: {e}"
            )));
        }
        Err(panic) => {
            return Err(Failure::new(format!(
                "the harness panicked opening a fresh lake: {panic}"
            )));
        }
    };
    fixture::default_limits(store.as_ref())?;
    let given = Given {
        reopen: harness.reopener(),
        relimit: harness.relimiter(),
        scratch: harness.scratch().join(check.name),
        meter: harness.meter(),
        large_lake: harness.large_lake(),
        harness: harness.name().to_owned(),
        import: harness.importer(),
        continued: harness.continuer(),
        twin: harness.twinner(),
        fault: harness.faulter(),
    };
    match caught((check.run)(store, given)).await {
        Ok(outcome) => outcome,
        Err(panic) => Err(Failure::new(format!("the check panicked: {panic}"))),
    }
}

/// Drives `fut`, turning a panic while it is polled into an `Err` with
/// the panic's message.
async fn caught<F: Future>(fut: F) -> Result<F::Output, String> {
    let mut fut = std::pin::pin!(fut);
    std::future::poll_fn(move |cx| {
        match std::panic::catch_unwind(AssertUnwindSafe(|| fut.as_mut().poll(cx))) {
            Ok(Poll::Ready(out)) => Poll::Ready(Ok(out)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => Poll::Ready(Err(panic_message(payload.as_ref()))),
        }
    })
    .await
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic without a message".to_owned()
    }
}

/// The next item of a stream, or `None` when it has ended: how a check
/// drains the [`CommitStream`](lakelands_api::CommitStream) `ingest`
/// answers.
pub async fn next<S: Stream + Unpin + ?Sized>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)).await
}

#[cfg(test)]
mod tests;

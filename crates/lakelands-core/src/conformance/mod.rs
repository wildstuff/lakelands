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
//! A check is one `async fn(Arc<dyn LakeStore>) -> Result<(), Failure>`
//! whose name is a sentence in snake_case, stating what must hold, and
//! one `check!` line turning it into a [`Check`] constant. Adding a check
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

use std::fmt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use futures_core::Stream;
use lakelands_api::LakeStore;
use lakelands_api::seam::Seams;

/// Turns an `async fn` of the suite into a [`Check`] constant whose
/// `name` is the function's own name. A trailing `, embedding` runs the
/// check on a lake with the suite's embedding backend bound; a trailing
/// `, reopens` hands the check the harness's [`Reopen`] as its second
/// argument.
macro_rules! check {
    ($(#[$doc:meta])* $konst:ident = $run:ident, $about:expr) => {
        check!(@make $(#[$doc])* $konst = $run, $about, false, |store, _| Box::pin($run(store)));
    };
    ($(#[$doc:meta])* $konst:ident = $run:ident, $about:expr, embedding) => {
        check!(@make $(#[$doc])* $konst = $run, $about, true, |store, _| Box::pin($run(store)));
    };
    ($(#[$doc:meta])* $konst:ident = $run:ident, $about:expr, reopens) => {
        check!(@make $(#[$doc])* $konst = $run, $about, false,
            |store, reopen| Box::pin($run(store, reopen)));
    };
    (@make $(#[$doc:meta])* $konst:ident = $run:ident, $about:expr, $embedding:expr, $call:expr) => {
        $(#[$doc])*
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
    ($cond:expr, $($msg:tt)+) => {
        if !$cond {
            return Err($crate::conformance::Failure::new(format!($($msg)+)));
        }
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
}

/// Closes a lake the harness opened and opens it again for writing with
/// `seams` and the harness's engine, as a host's next process opens it:
/// the same lake, a new writer.
pub type Reopen = Arc<dyn Fn(Arc<dyn LakeStore>, Seams) -> FreshLake<'static> + Send + Sync>;

/// The future [`Harness::fresh`] answers: a new, empty lake, or why none
/// could be opened.
pub type FreshLake<'a> =
    Pin<Box<dyn Future<Output = lakelands_api::Result<Arc<dyn LakeStore>>> + Send + 'a>>;

/// The future one check runs as.
pub type CheckFuture = Pin<Box<dyn Future<Output = Result<(), Failure>> + Send>>;

/// One behaviour every engine must honour.
#[derive(Clone, Copy)]
pub struct Check {
    /// A sentence in snake_case stating what must hold.
    pub name: &'static str,
    /// Where the rule is stated, and why it matters, in one sentence.
    pub about: &'static str,
    /// The check itself, run against a fresh lake; a check that reopens
    /// its lake uses the harness's [`Reopen`], the others ignore it.
    pub run: fn(Arc<dyn LakeStore>, Reopen) -> CheckFuture,
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
        match run_check(harness, check).await {
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
    match caught((check.run)(store, harness.reopener())).await {
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

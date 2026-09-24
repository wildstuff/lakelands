//! Tests of the runner itself, against lakes that fail on purpose.
//!
//! The checks are not run against a real store here (there is none in
//! this crate); what is tested is that the runner reports every check,
//! never stops early, and turns a panic or a harness that cannot open a
//! lake into a failure of that check.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use lakelands_api::describe::{LakeDescription, TypeDescription};
use lakelands_api::ontology::FieldDecl;
use lakelands_api::query::{Hits, Pattern, QueryResult, Search};
use lakelands_api::seam::{ChangeFeed, FeedBatch, FeedFilter, Seams};
use lakelands_api::write::{
    Declaration, Migration, MigrationReport, Proposal, ProposalFilter, ProposedWrite,
};
use lakelands_api::{
    Catalog, CommitStream, Error, Find, Found, Get, LakeId, LakeStore, Outcome, Patch, Position,
    Principal, ProposalId, ReadAt, Record, RecordKey, Reply, Result, SourceId, SourceStream,
    TypeName, VerbName, Verification, WriteContext, WriteOpts,
};

use super::{
    Check, Failure, FreshLake, Harness, Reopen, Report, checks, run_all, run_checks, run_one,
};

/// Drives a future to completion on this thread. Everything the tests
/// poll is ready at once, so a pending poll is a bug in the test.
fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..1_000_000 {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
    }
    panic!("a test future never became ready");
}

fn down() -> Error {
    Error::Unavailable {
        message: "this lake fails every call on purpose".to_owned(),
    }
}

type Answer<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Writes the method of an `async_trait` trait by hand: every argument
/// is ignored and the answer is always [`down`].
macro_rules! refuse {
    ($name:ident <$($lt:lifetime),*> ($($arg:ident : $ty:ty),*) -> $out:ty) => {
        fn $name<'life0, $($lt,)* 'async_trait>(
            &'life0 self,
            $($arg: $ty),*
        ) -> Answer<'async_trait, $out>
        where
            'life0: 'async_trait,
            $($lt: 'async_trait,)*
            Self: 'async_trait,
        {
            $(let _ = $arg;)*
            Box::pin(async { Err(down()) })
        }
    };
}

/// A catalog whose every door fails.
struct DownCatalog(LakeId);

impl Catalog for DownCatalog {
    fn lake(&self) -> &LakeId {
        &self.0
    }
    refuse!(describe<'life1>(who: &'life1 Principal, at: ReadAt) -> Reply<LakeDescription>);
    refuse!(describe_type<'life1, 'life2>(
        who: &'life1 Principal, ty: &'life2 TypeName, at: ReadAt
    ) -> Reply<TypeDescription>);
    refuse!(find<'life1, 'life2>(
        who: &'life1 Principal, ty: &'life2 TypeName, find: Find
    ) -> Reply<Found>);
    refuse!(get<'life1, 'life2>(
        who: &'life1 Principal, ty: &'life2 TypeName, get: Get
    ) -> Reply<Record>);
    refuse!(upsert<'life1, 'life2, 'life3>(
        who: &'life1 Principal, ty: &'life2 TypeName, key: &'life3 RecordKey,
        patch: Patch, opts: WriteOpts
    ) -> Outcome);
    refuse!(retire<'life1, 'life2, 'life3>(
        who: &'life1 Principal, ty: &'life2 TypeName, key: &'life3 RecordKey, opts: WriteOpts
    ) -> Outcome);
    refuse!(link<'life1, 'life2, 'life3, 'life4, 'life5>(
        who: &'life1 Principal, ty: &'life2 TypeName, key: &'life3 RecordKey,
        relation: &'life4 lakelands_api::RelationName, to: &'life5 RecordKey,
        fields: Patch, opts: WriteOpts
    ) -> Outcome);
    refuse!(unlink<'life1, 'life2, 'life3, 'life4, 'life5>(
        who: &'life1 Principal, ty: &'life2 TypeName, key: &'life3 RecordKey,
        relation: &'life4 lakelands_api::RelationName, to: &'life5 RecordKey, opts: WriteOpts
    ) -> Outcome);
    refuse!(call<'life1, 'life2, 'life3>(
        who: &'life1 Principal, verb: &'life2 VerbName, key: &'life3 RecordKey,
        payload: Patch, opts: WriteOpts
    ) -> Outcome);
    refuse!(query<'life1>(who: &'life1 Principal, pattern: Pattern) -> Reply<QueryResult>);
    refuse!(search<'life1>(who: &'life1 Principal, search: Search) -> Reply<Hits>);
    refuse!(propose<'life1>(who: &'life1 Principal, write: ProposedWrite, why: String) -> Outcome);
    refuse!(accept<'life1, 'life2>(who: &'life1 Principal, proposal: &'life2 ProposalId) -> Outcome);
    refuse!(reject<'life1, 'life2>(
        who: &'life1 Principal, proposal: &'life2 ProposalId, reason: String
    ) -> Outcome);
    refuse!(proposals<'life1>(
        who: &'life1 Principal, filter: ProposalFilter
    ) -> Reply<Vec<Proposal>>);
    refuse!(declare<'life1>(who: &'life1 Principal, declaration: Declaration) -> Outcome);
    refuse!(apply<'life1>(who: &'life1 Principal, declarations: Vec<Declaration>) -> Outcome);
    refuse!(add_field<'life1, 'life2>(
        who: &'life1 Principal, ty: &'life2 TypeName, field: FieldDecl
    ) -> Outcome);
    refuse!(migrate<'life1>(
        who: &'life1 Principal, migration: Migration
    ) -> Reply<MigrationReport>);
    refuse!(migrate_apply<'life1, 'life2>(
        who: &'life1 Principal, migration: Migration, report_id: &'life2 str
    ) -> Outcome);
    refuse!(ingest<'life1, 'life2>(
        who: &'life1 Principal, source: &'life2 SourceId, records: SourceStream
    ) -> CommitStream);
}

/// A change feed that fails every call.
struct DownFeed;

impl ChangeFeed for DownFeed {
    refuse!(poll<'life1>(from: Position, filter: &'life1 FeedFilter, limit: u32) -> FeedBatch);
    refuse!(ack<'life1>(consumer: &'life1 str, through: Position) -> ());
}

/// A store whose every call fails, and whose doors fail too.
struct DownStore(LakeId);

impl LakeStore for DownStore {
    fn lake(&self) -> &LakeId {
        &self.0
    }
    fn is_writer(&self) -> bool {
        false
    }
    fn limits(&self) -> lakelands_api::location::Limits {
        lakelands_api::location::Limits::default()
    }
    fn catalog(&self, _: WriteContext) -> Arc<dyn Catalog> {
        Arc::new(DownCatalog(self.0.clone()))
    }
    fn feed(&self) -> Arc<dyn ChangeFeed> {
        Arc::new(DownFeed)
    }
    refuse!(head<>() -> Position);
    refuse!(verify<>(from: Option<Position>, through: Option<Position>) -> Verification);
    refuse!(rebuild<>() -> Position);
    refuse!(restrict<'life1, 'life2>(ty: &'life1 TypeName, key: &'life2 RecordKey, reason: String) -> Position);
    refuse!(unrestrict<'life1, 'life2>(ty: &'life1 TypeName, key: &'life2 RecordKey) -> Position);
    refuse!(age<>(today: lakelands_api::Timestamp) -> lakelands_api::write::AgeReport);
    refuse!(purge_source<'life1>(source: &'life1 lakelands_api::SourceId) -> lakelands_api::write::PurgeReport);
    refuse!(checkpoint<>() -> Position);
    refuse!(close<>() -> ());
}

/// Hands out lakes whose every call fails.
struct DownHarness;

impl Harness for DownHarness {
    fn name(&self) -> &str {
        "down"
    }
    fn fresh(&self, _: Seams) -> FreshLake<'_> {
        Box::pin(async {
            let store: Arc<dyn LakeStore> = Arc::new(DownStore(LakeId::from("conformance")));
            Ok(store)
        })
    }
    fn reopener(&self) -> Reopen {
        Arc::new(|_, _| Box::pin(async { Err(down()) }))
    }
}

/// Cannot open a lake at all.
struct NoLakes;

impl Harness for NoLakes {
    fn name(&self) -> &str {
        "no-lakes"
    }
    fn fresh(&self, _: Seams) -> FreshLake<'_> {
        Box::pin(async { Err(down()) })
    }
    fn reopener(&self) -> Reopen {
        Arc::new(|_, _| Box::pin(async { Err(down()) }))
    }
}

fn all_names() -> Vec<&'static str> {
    checks::ALL.iter().map(|c| c.name).collect()
}

fn failed_names(report: &Report) -> Vec<&'static str> {
    report.failed.iter().map(|(name, _)| *name).collect()
}

#[test]
fn a_store_that_fails_every_call_fails_every_check_without_a_panic() {
    let report = block_on(run_all(&DownHarness));
    assert!(!report.is_green());
    assert!(report.passed.is_empty(), "passed: {:?}", report.passed);
    assert_eq!(failed_names(&report), all_names());
    for (name, failure) in &report.failed {
        assert!(
            !failure.message.contains("panicked"),
            "`{name}` panicked instead of failing: {failure}"
        );
        assert!(
            failure.message.contains("unavailable"),
            "`{name}` failed without saying why the call failed: {failure}"
        );
    }
}

#[test]
fn a_harness_that_cannot_open_a_lake_fails_every_check() {
    let report = block_on(run_all(&NoLakes));
    assert_eq!(report.harness, "no-lakes");
    assert_eq!(failed_names(&report), all_names());
    assert!(
        report
            .failed
            .iter()
            .all(|(_, f)| f.message.contains("could not open")),
        "{report}"
    );
}

async fn passes(_: Arc<dyn LakeStore>) -> std::result::Result<(), Failure> {
    Ok(())
}

async fn panics(_: Arc<dyn LakeStore>) -> std::result::Result<(), Failure> {
    panic!("on purpose")
}

async fn fails(_: Arc<dyn LakeStore>) -> std::result::Result<(), Failure> {
    ensure!(1 + 1 == 3, "arithmetic holds, so this fails");
    Ok(())
}

check!(PASSES = passes, "passes");
check!(PANICS = panics, "panics");
check!(FAILS = fails, "fails");

#[test]
fn a_panicking_check_fails_alone_and_the_run_goes_on() {
    let list: [Check; 4] = [PANICS, PASSES, FAILS, PASSES];
    let report = block_on(run_checks(&DownHarness, &list));
    assert_eq!(report.passed, ["passes", "passes"]);
    assert_eq!(failed_names(&report), ["panics", "fails"]);
    assert_eq!(
        report.failed[0].1,
        Failure::new("the check panicked: on purpose")
    );
    assert_eq!(
        report.failed[1].1,
        Failure::new("arithmetic holds, so this fails")
    );
    let shown = report.to_string();
    assert!(shown.contains("2 passed, 2 failed"), "{shown}");
}

#[test]
fn run_one_runs_the_named_check_and_knows_no_other() {
    let first = checks::ALL[0].name;
    let got = block_on(run_one(&DownHarness, first));
    assert!(matches!(got, Some(Err(_))), "{got:?}");
    assert_eq!(block_on(run_one(&DownHarness, "no_such_check")), None);
}

#[test]
fn every_check_has_a_unique_sentence_for_a_name_and_says_what_it_is_about() {
    let mut seen = std::collections::BTreeSet::new();
    for check in checks::ALL {
        assert!(
            seen.insert(check.name),
            "`{}` is registered twice",
            check.name
        );
        assert!(
            check.name.split('_').count() >= 4
                && check
                    .name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "`{}` is not a sentence in snake_case",
            check.name
        );
        assert!(
            !check.about.is_empty(),
            "`{}` says nothing about itself",
            check.name
        );
    }
}

#[test]
fn next_drains_a_stream_in_order_and_then_ends() {
    use crate::conformance::fixture::{source_record, stream};
    let mut s = stream(vec![
        source_record("feed", "a", serde_json::json!({})),
        source_record("feed", "b", serde_json::json!({})),
    ]);
    let keys: Vec<String> = block_on(async {
        let mut keys = Vec::new();
        while let Some(r) = super::next(&mut s).await {
            keys.push(r.key.0);
        }
        keys
    });
    assert_eq!(keys, ["a", "b"]);
}

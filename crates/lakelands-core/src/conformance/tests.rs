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

use super::fixture::Must;
use super::{
    Check, Failure, Fault, FreshLake, Given, Harness, Held, Relimit, Reopen, Report, Store, Tally,
    checks, run_all, run_checks, run_one,
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
    Error::unavailable(lakelands_api::error::Situation::Moved)
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
        who: &'life1 Principal, verb: &'life2 VerbName, key: Option<&'life3 RecordKey>,
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
    refuse!(quarantined<'life1>(
        who: &'life1 Principal, filter: lakelands_api::quarantine::QuarantineFilter
    ) -> Reply<Vec<lakelands_api::quarantine::Quarantined>>);
    refuse!(dismiss_quarantined<'life1, 'life2>(
        who: &'life1 Principal, id: &'life2 lakelands_api::quarantine::QuarantineId
    ) -> lakelands_api::quarantine::QuarantineClose);
    refuse!(declare<'life1>(who: &'life1 Principal, declaration: Declaration) -> Outcome);
    refuse!(apply<'life1>(who: &'life1 Principal, declarations: Vec<Declaration>) -> Outcome);
    refuse!(recompute<'life1>(
        who: &'life1 Principal, what: lakelands_api::write::Recompute
    ) -> Outcome);
    refuse!(add_field<'life1, 'life2>(
        who: &'life1 Principal, ty: &'life2 TypeName, field: FieldDecl
    ) -> Outcome);
    refuse!(migrate<'life1>(
        who: &'life1 Principal, migration: Migration
    ) -> Reply<MigrationReport>);
    refuse!(migrate_apply<'life1, 'life2>(
        who: &'life1 Principal, migration: Migration, report_id: &'life2 str
    ) -> Outcome);
    refuse!(migrate_value_type<'life1>(
        who: &'life1 Principal, to: lakelands_api::write::ValueTypeMigration
    ) -> Reply<lakelands_api::write::ValueTypeReport>);
    refuse!(migrate_value_type_apply<'life1, 'life2>(
        who: &'life1 Principal, to: lakelands_api::write::ValueTypeMigration, report_id: &'life2 str
    ) -> Outcome);
    refuse!(accept_planned<'life1, 'life2, 'life3>(
        who: &'life1 Principal, proposal: &'life2 ProposalId, plan_id: &'life3 str
    ) -> Outcome);
    refuse!(plan<'life1>(
        who: &'life1 Principal, change: lakelands_api::write::Change
    ) -> Reply<lakelands_api::write::Plan>);
    refuse!(enact<'life1, 'life2>(
        who: &'life1 Principal, change: lakelands_api::write::Change, plan_id: &'life2 str
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
    fn bind_embed(&self, _: Arc<dyn lakelands_api::seam::EmbedBackend>) -> Result<()> {
        Err(down())
    }
    refuse!(head<>() -> Position);
    refuse!(verify<>(from: Option<Position>, through: Option<Position>) -> Verification);
    refuse!(rebuild<>() -> Position);
    refuse!(restrict<'life1, 'life2>(ty: &'life1 TypeName, key: &'life2 RecordKey, reason: String) -> Position);
    refuse!(unrestrict<'life1, 'life2>(ty: &'life1 TypeName, key: &'life2 RecordKey) -> Position);
    refuse!(age<>(today: lakelands_api::Timestamp) -> lakelands_api::write::AgeReport);
    refuse!(purge_source<'life1, 'life2, 'life3>(source: &'life1 lakelands_api::SourceId, ty: Option<&'life2 TypeName>, id: Option<&'life3 str>) -> lakelands_api::write::PurgeReport);
    refuse!(purge_preflight<'life1, 'life2>(source: &'life1 lakelands_api::SourceId, ty: Option<&'life2 TypeName>) -> lakelands_api::write::PurgeReport);
    refuse!(forget_source<'life1, 'life2>(source: &'life1 lakelands_api::SourceId, id: Option<&'life2 str>) -> lakelands_api::write::ForgetReport);
    refuse!(forget_preflight<'life1>(source: &'life1 lakelands_api::SourceId) -> lakelands_api::write::ForgetReport);
    refuse!(remove_values<'life1, 'life2, 'life3>(source: &'life1 lakelands_api::SourceId, selection: &'life2 lakelands_api::write::Selection, shredded: bool, id: Option<&'life3 str>) -> lakelands_api::write::RemoveReport);
    refuse!(remove_values_preflight<'life1, 'life2>(source: &'life1 lakelands_api::SourceId, selection: &'life2 lakelands_api::write::Selection) -> lakelands_api::write::RemoveReport);
    refuse!(compact<>() -> lakelands_api::write::CompactReport);
    refuse!(position_at<>(known_at: lakelands_api::Timestamp) -> Option<Position>);
    refuse!(vector_coverage<'life1>(ty: &'life1 TypeName) -> Vec<lakelands_api::read::VectorCoverage>);
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
            let store: Store = Arc::new(DownStore(LakeId::from("conformance")));
            Ok(store)
        })
    }
    fn reopener(&self) -> Reopen {
        Arc::new(|_, _| Box::pin(async { Err(down()) }))
    }
    fn relimiter(&self) -> Relimit {
        Arc::new(|_, _, _| Box::pin(async { Err(down()) }))
    }
    fn faulter(&self) -> Fault {
        Arc::new(|_, _, _| Box::pin(async { Err(down()) }))
    }
    fn scratch(&self) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("lakelands-conformance-{}", std::process::id()))
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
    fn relimiter(&self) -> Relimit {
        Arc::new(|_, _, _| Box::pin(async { Err(down()) }))
    }
    fn scratch(&self) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("lakelands-conformance-{}", std::process::id()))
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

async fn passes(_: Store) -> Held {
    Ok(())
}

async fn panics(_: Store) -> Held {
    panic!("on purpose")
}

async fn fails(_: Store) -> Held {
    ensure!(1 + 1 == 3, "arithmetic holds, so this fails");
    Ok(())
}

async fn asserts(_: Store) -> Held {
    for n in 0..3 {
        ensure!(n < 3, "{n} is below three");
    }
    Ok::<_, Error>(1).must("one answer")?;
    Ok::<_, Error>(2).must("another")?;
    Err::<(), _>(down()).must_fail("a refusal")?;
    Ok(())
}

check!(PASSES = passes, "passes");
check!(ASSERTS = asserts, "asserts");
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
fn a_check_is_tallied_by_the_assertions_it_evaluates_and_where_they_are_written() {
    let list: [Check; 4] = [ASSERTS, PANICS, FAILS, PASSES];
    let report = block_on(run_checks(&DownHarness, &list));
    let tally = |evaluations, sites| Tally { evaluations, sites };
    assert_eq!(
        report.tallies,
        [
            ("asserts", tally(6, 4)),
            ("panics", tally(0, 0)),
            ("fails", tally(1, 1)),
            ("passes", tally(0, 0)),
        ]
    );
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

/// A catalog that carries no delivery id to the store refuses one, rather
/// than drop it, and so a delivery sealed apart (065), a commit window
/// of its own (016) and a quarantine (085); with none, its `ingest_with`
/// is its `ingest`.
#[test]
fn a_catalog_that_carries_no_delivery_refuses_one() {
    use lakelands_api::error::Case;
    use lakelands_api::write::IngestOpts;
    let cat = DownCatalog(LakeId::from("l"));
    let who = Principal {
        kind: lakelands_api::PrincipalKind::Human,
        id: "owner".into(),
        lake: LakeId::from("l"),
        grants: lakelands_api::Grants::owner(),
    };
    let src = SourceId::from("crm");
    let with = |delivery: Option<&str>| IngestOpts {
        delivery: delivery.map(Into::into),
        ..IngestOpts::default()
    };
    let named = block_on(cat.ingest_with(
        &who,
        &src,
        with(Some("batch-7")),
        super::fixture::stream(vec![]),
    ));
    let case = named.err().and_then(|e| e.case().map(|s| s.case.clone()));
    assert_eq!(case, Some(Case::DeliveryNotCarried));
    let none = block_on(cat.ingest_with(&who, &src, with(None), super::fixture::stream(vec![])));
    assert!(
        matches!(&none, Err(e) if e.situation() == down().situation()),
        "with no delivery it ingests, and this catalog's ingest is down"
    );
    let apart = IngestOpts {
        sealed_apart: true,
        ..IngestOpts::default()
    };
    let apart = block_on(cat.ingest_with(&who, &src, apart, super::fixture::stream(vec![])));
    let case = apart.err().and_then(|e| e.case().map(|s| s.case.clone()));
    assert_eq!(case, Some(Case::SealedApartNotCarried));
    let window = IngestOpts {
        commit_window: Some(2),
        ..IngestOpts::default()
    };
    let window = block_on(cat.ingest_with(&who, &src, window, super::fixture::stream(vec![])));
    let case = window.err().and_then(|e| e.case().map(|s| s.case.clone()));
    assert_eq!(case, Some(Case::CommitWindowNotCarried));
    // A quarantine is refused first, whatever else the call asks.
    let quarantine = IngestOpts {
        quarantine_refused: true,
        ..with(Some("batch-8"))
    };
    let quarantine =
        block_on(cat.ingest_with(&who, &src, quarantine, super::fixture::stream(vec![])));
    let case = quarantine
        .err()
        .and_then(|e| e.case().map(|s| s.case.clone()));
    assert_eq!(case, Some(Case::QuarantineRefusedNotCarried));
}

/// `requires_unmet` is, by value, the refusal the checks built by hand.
#[test]
fn requires_unmet_equals_the_refusal_built_by_hand() {
    use lakelands_api::Refusal;
    use lakelands_api::error::{Case, Printed};
    let by_hand = |part: Option<&str>| {
        let case = Case::RequiresUnmet {
            verb: "vet".into(),
            ty: "invoice".into(),
            key: "i1".into(),
            part: part.map(|p| Box::new(Printed(p.into()))),
        };
        Error::Refused {
            field: None,
            reason: Refusal::Precondition(Box::new(case.into())),
        }
    };
    for part in [None, Some("status = 'open'")] {
        let got = super::fixture::requires_unmet::<()>("vet", "invoice", "i1", part);
        assert_eq!(got, Err(by_hand(part)));
    }
}

/// A [`Given`] of the down harness whose reopen and relimit are the ones given.
fn given_of(reopen: Reopen, relimit: Relimit) -> Given {
    let h = DownHarness;
    Given {
        reopen,
        relimit,
        scratch: h.scratch(),
        meter: h.meter(),
        large_lake: h.large_lake(),
        harness: h.name().to_owned(),
        import: h.importer(),
        continued: h.continuer(),
        twin: h.twinner(),
        fault: h.faulter(),
        location: None,
        ahead: h.aheader(),
        lower: h.lowerer(),
        standby: h.standbyer(),
    }
}

fn a_store() -> Store {
    Arc::new(DownStore(LakeId::from("conformance")))
}

/// `at_one_set_change` opens the lake again with the default limits but
/// one membership change per write, and the default seams.
#[test]
fn at_one_set_change_relimits_as_the_checks_did_by_hand() {
    use lakelands_api::location::Limits;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let relimit: Relimit = Arc::new(move |store, _, limits| {
        log.lock().unwrap().push(limits);
        Box::pin(async move { Ok(store) })
    });
    let given = given_of(DownHarness.reopener(), relimit);
    block_on(super::fixture::at_one_set_change(&given, a_store(), "x")).unwrap();
    let by_hand = Limits {
        max_set_changes: 1,
        ..Limits::default()
    };
    assert_eq!(*seen.lock().unwrap(), [by_hand]);
    let failing = given_of(DownHarness.reopener(), DownHarness.relimiter());
    let got = block_on(super::fixture::at_one_set_change(&failing, a_store(), "x"));
    assert_eq!(got.err().unwrap().to_string(), format!("x: {}", down()));
}

/// `reopen_with_keys` hands the lake a new, empty key store and answers
/// that same store.
#[test]
fn reopen_with_keys_opens_with_a_new_key_store_and_answers_it() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let reopen: Reopen = Arc::new(move |store, seams| {
        log.lock().unwrap().push(seams.keys);
        Box::pin(async move { Ok(store) })
    });
    let (keys, _) = block_on(super::fixture::reopen_with_keys(&reopen, a_store(), "x")).unwrap();
    assert!(keys.is_new());
    let seen = seen.lock().unwrap();
    let given = seen[0].as_ref().expect("the seams carry a key store");
    assert_eq!(
        Arc::as_ptr(given).cast::<()>(),
        Arc::as_ptr(&keys).cast::<()>()
    );
    assert_eq!(seen.len(), 1);
}

/// A sealed `record_ingested` whose patch is `patch`, whose box is
/// `boxed` and whose writer is `writer`.
fn sealed_event(
    patch: serde_json::Value,
    boxed: &str,
    writer: &str,
) -> lakelands_api::event::Event {
    serde_json::from_value(serde_json::json!({
        "v": 20, "lake": "finance", "position": 1, "id": "0".repeat(32),
        "prev": lakelands_api::event::Digest::zero(), "known_at": "2026-09-24T00:00:00.000000Z",
        "writer": writer, "origin": "source", "source": "crm",
        "context": {"origin": "direct", "depth": 0}, "fence": 0,
        "sealed": {"key": "k1", "fields": ["name"], "box": boxed},
        "body": {"kind": "record_ingested", "type": "person", "key": "p1",
                 "version": 1, "patch": patch, "made": true},
        "hash": lakelands_api::event::Digest::zero(),
    }))
    .unwrap()
}

#[test]
fn the_plaintext_search_goes_red_on_a_leak_and_ignores_the_box() {
    use checks::erasable::plain_outside_box as clean;
    let issue_box = "WVQGaXJMNhbAda80xQzut+RoXUztV4s1tah588X5";
    let ok = sealed_event(
        serde_json::json!({"city": "Oslo"}),
        "QUJDREVGR0g=",
        "system:a",
    );
    assert!(clean(&ok, &["Ada"]));
    // The ciphertext of the flake: `Ada` in the box text by chance.
    let chance = sealed_event(serde_json::json!({"city": "Oslo"}), issue_box, "system:a");
    assert!(clean(&chance, &["Ada"]));
    // A leak in the patch, and in another field.
    let in_patch = sealed_event(serde_json::json!({"name": "Ada"}), issue_box, "system:a");
    assert!(!clean(&in_patch, &["Ada"]));
    let in_writer = sealed_event(serde_json::json!({"city": "Oslo"}), issue_box, "user:Ada");
    assert!(!clean(&in_writer, &["Ada"]));
    assert!(!clean(&in_patch, &["Bob", "Ada"]));
}

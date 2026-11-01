//! The ticker: `serve` hands each lake it writes the date, so its aging
//! rules run daily (`docs/decisions/036-aging-runs-by-date-once.md`).
//!
//! The date is the day of the lake's calendar holding the host's clock:
//! the clock is read here, outside the store, and cut in the zone the lake
//! names (`LakeStore::zone`, its `zone` setting,
//! `docs/decisions/101-a-lake-keeps-its-calendar-in-one-named-zone.md`),
//! and the date is handed to `LakeStore::age` as the argument; the lake
//! never reads a clock (`docs/decisions/002-determinism-is-the-boundary.md`).
//! The ticker ages once when it starts and again whenever that date rolls
//! over, looking every `check_secs`, and reads the zone at every look, so
//! a zone changed while it runs moves the next rollover. Aging is
//! idempotent by date, so a restart or a second run on one date writes
//! nothing new. A date the process was down for is not aged afterwards: a
//! rule is a condition on today, so the next date's run moves every record
//! that should have moved. A run that fails, or a zone that cannot be
//! read, is tried again at the next check, never sooner. Only a process
//! that holds a lake as its writer runs a ticker for it.

use std::sync::Arc;
use std::time::Duration;

use lakelands_api::expr::zone;
use lakelands_api::write::AgeReport;
use lakelands_api::{LakeStore, Timestamp};
use serde::{Deserialize, Serialize};

/// The `aging` section of a root's `lakelands.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgingConfig {
    /// How often the ticker looks whether the date rolled over, in
    /// seconds.
    #[serde(default = "default_check")]
    pub check_secs: u64,
}

fn default_check() -> u64 {
    60
}

impl AgingConfig {
    /// Refuse a check of zero seconds. A host checks this when it loads
    /// the configuration.
    pub fn check(&self) -> Result<(), String> {
        self.checked().map(|_| ())
    }

    /// The configuration as the ticker takes it, or why it is none.
    pub fn checked(&self) -> Result<Aging, String> {
        if self.check_secs == 0 {
            return Err("the aging section's `check_secs` is zero".into());
        }
        Ok(Aging {
            check: Duration::from_secs(self.check_secs),
        })
    }

    /// Why `config`, a root's configuration as written, is refused where it
    /// names the fixed offset aging once read: the date is the lake's own,
    /// read in its zone, and nothing in the root moves it.
    pub fn gone(config: &str) -> Option<&'static str> {
        let config: serde_json::Value = serde_json::from_str(config).ok()?;
        config.pointer("/aging/utc_offset").map(|_| {
            "`aging.utc_offset` is gone: the date `serve` ages a lake for is the day of the \
             lake's own calendar, read in the zone its `zone` setting names"
        })
    }
}

/// A checked aging configuration: a check that is not zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aging {
    check: Duration,
}

/// Where the ticker reads the time: the host's clock, or a test's.
pub trait Clock: Send + Sync {
    /// Now, as a UTC instant.
    fn now(&self) -> Timestamp;
}

/// The host's clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let micros = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_micros());
        Timestamp(i64::try_from(micros).unwrap_or(i64::MAX))
    }
}

/// One lake's ticker.
pub struct Ticker {
    store: Arc<dyn LakeStore>,
    check: Duration,
    clock: Arc<dyn Clock>,
    /// The date last aged, once a run succeeded.
    aged: Option<Timestamp>,
    /// The date last tried, aged or not.
    tried: Option<Timestamp>,
}

impl Ticker {
    /// A ticker for a lake this process writes.
    pub fn new(store: Arc<dyn LakeStore>, aging: Aging, clock: Arc<dyn Clock>) -> Ticker {
        Ticker {
            store,
            check: aging.check,
            clock,
            aged: None,
            tried: None,
        }
    }

    /// Today: the day of the lake's calendar holding the clock's now, read
    /// in the zone the lake names, at UTC midnight, as `age` takes it.
    pub async fn today(&self) -> lakelands_api::Result<Timestamp> {
        let zone = self.store.zone().await?;
        Ok(Timestamp(zone::day(self.clock.now().0, &zone)))
    }

    /// The date last aged, if a run succeeded.
    pub fn aged(&self) -> Option<Timestamp> {
        self.aged
    }

    /// One check: age today when it is not the date last aged. Answers the
    /// run's report, or its failure, which the next check tries again;
    /// `None` when there was nothing to do.
    pub async fn tick(&mut self) -> Option<lakelands_api::Result<AgeReport>> {
        let today = match self.today().await {
            Ok(today) => today,
            Err(e) => return Some(Err(e)),
        };
        if self.aged == Some(today) {
            return None;
        }
        self.tried = Some(today);
        let run = self.store.age(today).await;
        if run.is_ok() {
            self.aged = Some(today);
        }
        Some(run)
    }

    /// Check at start and every `check_secs` until the process ends,
    /// saying each run, and each failure, in one line on standard error.
    pub async fn run(mut self) {
        loop {
            match self.tick().await {
                Some(Ok(r)) => eprintln!(
                    "lakelands: aged lake `{}` for {}: {} rule(s), {} moved, {} guard(s) failed{}",
                    self.store.lake(),
                    date(r.date),
                    r.rules.len(),
                    r.transitions,
                    r.failed_guards,
                    crate::commands::refused_words(r.refused)
                ),
                Some(Err(e)) => eprintln!(
                    "lakelands: aging lake `{}`{} failed, trying again at the next check: {e}",
                    self.store.lake(),
                    self.tried
                        .map(|d| format!(" for {}", date(d)))
                        .unwrap_or_default()
                ),
                None => {}
            }
            tokio::time::sleep(self.check).await;
        }
    }
}

/// A date at UTC midnight, as `YYYY-MM-DD`.
fn date(t: Timestamp) -> String {
    t.to_string().chars().take(10).collect()
}

/// The tickers `serve` starts: one per lake it holds as the writer, when
/// the root configures aging. Answers how many it started.
pub fn start(config: Option<&AgingConfig>, lakes: &[Arc<dyn LakeStore>]) -> usize {
    let Some(config) = config else {
        return 0;
    };
    // `Root::open` refused a configuration that is none; said again here
    // rather than run under a check nobody wrote.
    let aging = match config.checked() {
        Ok(a) => a,
        Err(why) => {
            eprintln!("lakelands: no ticker runs: {why}");
            return 0;
        }
    };
    let mut started = std::collections::BTreeSet::new();
    for store in lakes.iter().filter(|s| s.is_writer()) {
        if started.insert(store.lake().clone()) {
            tokio::spawn(Ticker::new(store.clone(), aging, Arc::new(SystemClock)).run());
        }
    }
    started.len()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use lakelands_api::engine::OpenMode;
    use lakelands_api::seam::Seams;
    use lakelands_api::{Location, WriteContext};

    use super::*;

    /// A clock the test moves.
    struct Manual(Mutex<Timestamp>);

    impl Clock for Manual {
        fn now(&self) -> Timestamp {
            *self.0.lock().unwrap()
        }
    }

    fn at(s: &str) -> Timestamp {
        Timestamp::parse(s).unwrap()
    }

    fn day(s: &str) -> Timestamp {
        let (y, m, d) = (&s[0..4], &s[5..7], &s[8..10]);
        Timestamp::from_date(y.parse().unwrap(), m.parse().unwrap(), d.parse().unwrap()).unwrap()
    }

    fn config() -> AgingConfig {
        AgingConfig { check_secs: 60 }
    }

    fn aging() -> Aging {
        config().checked().unwrap()
    }

    /// Set the lake's `zone` setting, as its owner.
    async fn set_zone(lake: &Arc<dyn LakeStore>, zone: &str) {
        let owner = lakelands_api::Principal {
            kind: lakelands_api::principal::PrincipalKind::Human,
            id: "owner".into(),
            lake: lake.lake().clone(),
            grants: lakelands_api::principal::Grants::owner(),
        };
        let ty = lakelands_api::seam::SETTING_TYPE.into();
        let key = lakelands_api::seam::ZONE_SETTING;
        let patch = [("key".into(), key.into()), ("value".into(), zone.into())].into();
        let cat = lake.catalog(WriteContext::default());
        cat.upsert(&owner, &ty, &key.into(), patch, Default::default())
            .await
            .unwrap();
    }

    async fn memory_lake(name: &str) -> Arc<dyn LakeStore> {
        lakelands_store::open(
            Location::memory(name),
            Seams {
                engines: vec![Arc::new(lakelands_engine_mem::MemoryOpener::new())],
                ..Seams::default()
            },
            OpenMode::Create,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn the_date_is_the_clocks_in_the_lake_s_zone() {
        let clock = Arc::new(Manual(Mutex::new(at("2026-12-01T23:30:00Z"))));
        let lake = memory_lake("t").await;
        let t = Ticker::new(lake.clone(), aging(), clock.clone());
        assert_eq!(t.today().await.unwrap(), day("2026-12-01"), "UTC unset");
        set_zone(&lake, "Europe/Berlin").await;
        assert_eq!(
            t.today().await.unwrap(),
            day("2026-12-02"),
            "23:30 UTC is past midnight in Berlin"
        );
        *clock.0.lock().unwrap() = at("2026-12-02T03:00:00Z");
        set_zone(&lake, "America/St_Johns").await;
        assert_eq!(
            t.today().await.unwrap(),
            day("2026-12-01"),
            "03:00 UTC is still the day before at -03:30"
        );
    }

    #[tokio::test]
    async fn it_ages_at_start_and_again_only_when_the_date_rolls_over() {
        let lake = memory_lake("ticker-rollover").await;
        let clock = Arc::new(Manual(Mutex::new(at("2026-12-01T08:00:00Z"))));
        let mut t = Ticker::new(lake.clone(), aging(), clock.clone());
        let first = t.tick().await.expect("the start ages").unwrap();
        assert_eq!(first.date, day("2026-12-01"));
        assert_eq!(t.aged(), Some(day("2026-12-01")));
        // Later the same day: nothing to do.
        *clock.0.lock().unwrap() = at("2026-12-01T23:59:00Z");
        assert!(t.tick().await.is_none());
        // Past midnight: the next date.
        *clock.0.lock().unwrap() = at("2026-12-02T00:01:00Z");
        let next = t.tick().await.expect("the rollover ages").unwrap();
        assert_eq!(next.date, day("2026-12-02"));
        // Days the process was down for are not aged afterwards: after a
        // gap, the ticker ages today, once.
        *clock.0.lock().unwrap() = at("2026-12-09T10:00:00Z");
        let later = t.tick().await.expect("the new date ages").unwrap();
        assert_eq!(later.date, day("2026-12-09"));
        assert!(t.tick().await.is_none());
        let _ = lake.catalog(WriteContext::default());
    }

    /// A lake whose `age` fails, counting the tries.
    struct Failing {
        inner: Arc<dyn LakeStore>,
        tries: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LakeStore for Failing {
        fn lake(&self) -> &lakelands_api::LakeId {
            self.inner.lake()
        }
        fn is_writer(&self) -> bool {
            true
        }
        fn limits(&self) -> lakelands_api::location::Limits {
            self.inner.limits()
        }
        async fn head(&self) -> lakelands_api::Result<lakelands_api::Position> {
            self.inner.head().await
        }
        fn catalog(&self, c: WriteContext) -> Arc<dyn lakelands_api::Catalog> {
            self.inner.catalog(c)
        }
        fn feed(&self) -> Arc<dyn lakelands_api::seam::ChangeFeed> {
            self.inner.feed()
        }
        async fn verify(
            &self,
            from: Option<lakelands_api::Position>,
            through: Option<lakelands_api::Position>,
        ) -> lakelands_api::Result<lakelands_api::catalog::Verification> {
            self.inner.verify(from, through).await
        }
        async fn rebuild(&self) -> lakelands_api::Result<lakelands_api::Position> {
            self.inner.rebuild().await
        }
        async fn restrict(
            &self,
            ty: &lakelands_api::TypeName,
            key: &lakelands_api::RecordKey,
            reason: String,
        ) -> lakelands_api::Result<lakelands_api::Position> {
            self.inner.restrict(ty, key, reason).await
        }
        async fn unrestrict(
            &self,
            ty: &lakelands_api::TypeName,
            key: &lakelands_api::RecordKey,
        ) -> lakelands_api::Result<lakelands_api::Position> {
            self.inner.unrestrict(ty, key).await
        }
        async fn checkpoint(&self) -> lakelands_api::Result<lakelands_api::Position> {
            self.inner.checkpoint().await
        }
        async fn age(&self, _: Timestamp) -> lakelands_api::Result<AgeReport> {
            self.tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // Another writer holds the lake for a moment.
            let (holder, fence) = ("another writer".to_owned(), 1);
            Err(lakelands_api::Error::unavailable(
                lakelands_api::error::Situation::LeaseHeld { holder, fence },
            ))
        }
        async fn purge_source(
            &self,
            source: &lakelands_api::SourceId,
            ty: Option<&lakelands_api::TypeName>,
            id: Option<&str>,
        ) -> lakelands_api::Result<lakelands_api::write::PurgeReport> {
            self.inner.purge_source(source, ty, id).await
        }
        async fn purge_preflight(
            &self,
            source: &lakelands_api::SourceId,
            ty: Option<&lakelands_api::TypeName>,
        ) -> lakelands_api::Result<lakelands_api::write::PurgeReport> {
            self.inner.purge_preflight(source, ty).await
        }
        async fn upsert_preflight(
            &self,
            who: &lakelands_api::Principal,
            context: lakelands_api::WriteContext,
            ty: &lakelands_api::TypeName,
            key: &lakelands_api::RecordKey,
            patch: lakelands_api::record::Patch,
            expected: Option<lakelands_api::Version>,
        ) -> lakelands_api::Result<()> {
            self.inner
                .upsert_preflight(who, context, ty, key, patch, expected)
                .await
        }
        async fn forget_source(
            &self,
            source: &lakelands_api::SourceId,
            id: Option<&str>,
        ) -> lakelands_api::Result<lakelands_api::write::ForgetReport> {
            self.inner.forget_source(source, id).await
        }
        async fn forget_preflight(
            &self,
            source: &lakelands_api::SourceId,
        ) -> lakelands_api::Result<lakelands_api::write::ForgetReport> {
            self.inner.forget_preflight(source).await
        }
        async fn forget_sources(
            &self,
            sources: &[lakelands_api::SourceId],
            id: Option<&str>,
        ) -> lakelands_api::Result<lakelands_api::write::ForgetSetReport> {
            self.inner.forget_sources(sources, id).await
        }
        async fn forget_sources_preflight(
            &self,
            sources: &[lakelands_api::SourceId],
        ) -> lakelands_api::Result<lakelands_api::write::ForgetSetReport> {
            self.inner.forget_sources_preflight(sources).await
        }
        async fn remove_values(
            &self,
            source: &lakelands_api::SourceId,
            selection: &lakelands_api::write::Selection,
            shredded: bool,
            id: Option<&str>,
        ) -> lakelands_api::Result<lakelands_api::write::RemoveReport> {
            self.inner
                .remove_values(source, selection, shredded, id)
                .await
        }
        async fn remove_values_preflight(
            &self,
            source: &lakelands_api::SourceId,
            selection: &lakelands_api::write::Selection,
        ) -> lakelands_api::Result<lakelands_api::write::RemoveReport> {
            self.inner.remove_values_preflight(source, selection).await
        }
        async fn compact(&self) -> lakelands_api::Result<lakelands_api::write::CompactReport> {
            self.inner.compact().await
        }
        fn bind_embed(
            &self,
            backend: Arc<dyn lakelands_api::seam::EmbedBackend>,
        ) -> lakelands_api::Result<()> {
            self.inner.bind_embed(backend)
        }
        async fn vector_coverage(
            &self,
            ty: &lakelands_api::TypeName,
        ) -> lakelands_api::Result<Vec<lakelands_api::read::VectorCoverage>> {
            self.inner.vector_coverage(ty).await
        }
        async fn zone(&self) -> lakelands_api::Result<String> {
            self.inner.zone().await
        }
        async fn position_at(
            &self,
            known_at: lakelands_api::Timestamp,
        ) -> lakelands_api::Result<Option<lakelands_api::Position>> {
            self.inner.position_at(known_at).await
        }
        async fn close(&self) -> lakelands_api::Result<()> {
            self.inner.close().await
        }
    }

    #[tokio::test]
    async fn a_run_that_fails_is_tried_again_at_the_next_check() {
        let lake = Arc::new(Failing {
            inner: memory_lake("ticker-fails").await,
            tries: Default::default(),
        });
        let clock = Arc::new(Manual(Mutex::new(at("2026-12-01T08:00:00Z"))));
        let mut t = Ticker::new(lake.clone(), aging(), clock);
        assert!(matches!(t.tick().await, Some(Err(_))));
        assert_eq!(t.aged(), None);
        assert!(
            matches!(t.tick().await, Some(Err(_))),
            "the same date is tried again at the next check"
        );
        assert_eq!(lake.tries.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    /// Let the ticker's task run what it can without the clock moving.
    async fn settle() {
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
    }

    /// The loop itself, on a paused clock that only the test moves: a
    /// check at start, then one per `check_secs`, never sooner. A failing
    /// lake is tried exactly once per period.
    #[tokio::test(start_paused = true)]
    async fn the_loop_checks_at_start_and_once_every_check_secs_never_sooner() {
        let lake = Arc::new(Failing {
            inner: memory_lake("ticker-loop").await,
            tries: Default::default(),
        });
        let tries = || lake.tries.load(std::sync::atomic::Ordering::SeqCst);
        let clock = Arc::new(Manual(Mutex::new(at("2026-12-01T08:00:00Z"))));
        let task = tokio::spawn(Ticker::new(lake.clone(), aging(), clock).run());
        settle().await;
        assert_eq!(tries(), 1, "a check at start");
        tokio::time::advance(Duration::from_secs(59)).await;
        settle().await;
        assert_eq!(tries(), 1, "nothing before check_secs has passed");
        tokio::time::advance(Duration::from_secs(2)).await;
        settle().await;
        assert_eq!(tries(), 2, "the next check, once");
        tokio::time::advance(Duration::from_secs(60)).await;
        settle().await;
        assert_eq!(tries(), 3, "one more per period");
        task.abort();
    }

    #[tokio::test]
    async fn a_process_that_only_reads_a_lake_runs_no_ticker_for_it() {
        let opener = lakelands_engine_mem::MemoryOpener::new();
        let open = |mode| {
            lakelands_store::open(
                Location::memory("ticker-reader"),
                Seams {
                    engines: vec![Arc::new(opener.clone())],
                    ..Seams::default()
                },
                mode,
            )
        };
        let writer = open(OpenMode::Create).await.unwrap();
        let reader = open(OpenMode::Read).await.unwrap();
        assert!(writer.is_writer() && !reader.is_writer());
        let cfg = config();
        assert_eq!(start(Some(&cfg), std::slice::from_ref(&reader)), 0);
        assert_eq!(
            start(Some(&cfg), &[writer.clone(), writer]),
            1,
            "one per lake"
        );
        assert_eq!(start(None, &[reader]), 0, "no aging section, no ticker");
    }

    #[test]
    fn a_zero_check_or_a_fixed_offset_is_refused() {
        assert!(config().check().is_ok());
        assert!(AgingConfig { check_secs: 0 }.check().is_err());
        let named = r#"{"aging": {"utc_offset": "+01:00"}}"#;
        assert!(AgingConfig::gone(named).is_some_and(|why| why.contains("`zone`")));
        assert_eq!(AgingConfig::gone(r#"{"aging": {"check_secs": 5}}"#), None);
    }
}

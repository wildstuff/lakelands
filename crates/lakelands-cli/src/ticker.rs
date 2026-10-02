//! The ticker: `serve` hands each lake it writes the date, so its aging
//! rules run daily (`docs/decisions/036-aging-runs-by-date-once.md`).
//!
//! The date is the host's clock at the configured UTC offset, read here,
//! outside the store, and handed to `LakeStore::age` as the argument; the
//! lake never reads a clock (`docs/decisions/002-determinism-is-the-boundary.md`).
//! The ticker ages once when it starts and again whenever that date rolls
//! over, looking every `check_secs`. Aging is idempotent by date, so a
//! restart or a second run on one date writes nothing new. A date the
//! process was down for is not aged afterwards: a rule is a condition on
//! today, so the next date's run moves every record that should have
//! moved. A run that fails is tried again at the next check, never
//! sooner. Only a process that holds a lake as its writer runs a ticker
//! for it.

use std::sync::Arc;
use std::time::Duration;

use lakelands_api::write::AgeReport;
use lakelands_api::{LakeStore, Timestamp};
use serde::{Deserialize, Serialize};

/// Microseconds in a day.
const DAY: i64 = 86_400_000_000;

/// The `aging` section of a root's `lakelands.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgingConfig {
    /// The UTC offset the date is read at: `+01:00`, `-05:30`, `Z`. A
    /// fixed offset, not a zone: a daylight-saving shift moves the
    /// rollover by an hour, which a rule over dates does not notice.
    #[serde(default = "default_offset")]
    pub utc_offset: String,
    /// How often the ticker looks whether the date rolled over, in
    /// seconds.
    #[serde(default = "default_check")]
    pub check_secs: u64,
}

fn default_offset() -> String {
    "Z".into()
}

fn default_check() -> u64 {
    60
}

impl AgingConfig {
    /// The offset in seconds, or why it is not one. An offset is `Z`, or
    /// `+HH:MM` / `-HH:MM` within fourteen hours.
    pub fn offset_secs(&self) -> Result<i64, String> {
        let o = self.utc_offset.as_str();
        if o == "Z" {
            return Ok(0);
        }
        let bad = || format!("`{o}` is not a UTC offset: write `Z`, or `+HH:MM` or `-HH:MM`");
        let (sign, rest) = match o.as_bytes().first() {
            Some(b'+') => (1, &o[1..]),
            Some(b'-') => (-1, &o[1..]),
            _ => return Err(bad()),
        };
        let (h, m) = rest.split_once(':').ok_or_else(bad)?;
        if h.len() != 2 || m.len() != 2 {
            return Err(bad());
        }
        let (h, m): (i64, i64) = (h.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?);
        if m >= 60 || h * 60 + m > 14 * 60 {
            return Err(bad());
        }
        Ok(sign * (h * 3600 + m * 60))
    }

    /// Refuse an offset that is none, or a check of zero seconds. A host
    /// checks this when it loads the configuration.
    pub fn check(&self) -> Result<(), String> {
        self.checked().map(|_| ())
    }

    /// The configuration as the ticker takes it, or why it is none.
    pub fn checked(&self) -> Result<Aging, String> {
        let offset = self.offset_secs()?;
        if self.check_secs == 0 {
            return Err("the aging section's `check_secs` is zero".into());
        }
        Ok(Aging {
            offset_micros: offset * 1_000_000,
            check: Duration::from_secs(self.check_secs),
        })
    }
}

/// A checked aging configuration: an offset that is one, and a check
/// that is not zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Aging {
    offset_micros: i64,
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
    offset_micros: i64,
    check: Duration,
    clock: Arc<dyn Clock>,
    /// The date last aged, once a run succeeded.
    aged: Option<Timestamp>,
}

impl Ticker {
    /// A ticker for a lake this process writes.
    pub fn new(store: Arc<dyn LakeStore>, aging: Aging, clock: Arc<dyn Clock>) -> Ticker {
        Ticker {
            store,
            offset_micros: aging.offset_micros,
            check: aging.check,
            clock,
            aged: None,
        }
    }

    /// Today: the date of the clock's now at the offset, at UTC midnight,
    /// as `age` takes it.
    pub fn today(&self) -> Timestamp {
        let local = self.clock.now().0.saturating_add(self.offset_micros);
        Timestamp(local.div_euclid(DAY) * DAY)
    }

    /// The date last aged, if a run succeeded.
    pub fn aged(&self) -> Option<Timestamp> {
        self.aged
    }

    /// One check: age today when it is not the date last aged. Answers the
    /// run's report, or its failure, which the next check tries again;
    /// `None` when there was nothing to do.
    pub async fn tick(&mut self) -> Option<lakelands_api::Result<AgeReport>> {
        let today = self.today();
        if self.aged == Some(today) {
            return None;
        }
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
                    "lakelands: aged lake `{}` for {}: {} rule(s), {} moved, {} guard(s) failed",
                    self.store.lake(),
                    date(r.date),
                    r.rules.len(),
                    r.transitions,
                    r.failed_guards
                ),
                Some(Err(e)) => eprintln!(
                    "lakelands: aging lake `{}` for {} failed, trying again at the next check: {e}",
                    self.store.lake(),
                    date(self.today())
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
    // rather than run under an offset nobody wrote.
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

    fn config(offset: &str) -> AgingConfig {
        AgingConfig {
            utc_offset: offset.into(),
            check_secs: 60,
        }
    }

    fn aging(offset: &str) -> Aging {
        config(offset).checked().unwrap()
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

    #[test]
    fn the_date_is_the_clocks_at_the_offset() {
        let clock = Arc::new(Manual(Mutex::new(at("2026-12-01T23:30:00Z"))));
        let lake: Arc<dyn LakeStore> = Arc::new(lakelands_mock::MockLake::new("t"));
        let utc = Ticker::new(lake.clone(), aging("Z"), clock.clone());
        assert_eq!(utc.today(), day("2026-12-01"));
        let east = Ticker::new(lake.clone(), aging("+01:00"), clock.clone());
        assert_eq!(
            east.today(),
            day("2026-12-02"),
            "23:30 UTC is past midnight at +01:00"
        );
        *clock.0.lock().unwrap() = at("2026-12-02T03:00:00Z");
        let west = Ticker::new(lake, aging("-05:30"), clock);
        assert_eq!(
            west.today(),
            day("2026-12-01"),
            "03:00 UTC is still the day before at -05:30"
        );
    }

    #[tokio::test]
    async fn it_ages_at_start_and_again_only_when_the_date_rolls_over() {
        let lake = memory_lake("ticker-rollover").await;
        let clock = Arc::new(Manual(Mutex::new(at("2026-12-01T08:00:00Z"))));
        let mut t = Ticker::new(lake.clone(), aging("Z"), clock.clone());
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
            Err(lakelands_api::Error::Unavailable {
                message: "the lake is busy for a moment".into(),
            })
        }
        async fn purge_source(
            &self,
            source: &lakelands_api::SourceId,
        ) -> lakelands_api::Result<lakelands_api::write::PurgeReport> {
            self.inner.purge_source(source).await
        }
        async fn compact(&self) -> lakelands_api::Result<lakelands_api::write::CompactReport> {
            self.inner.compact().await
        }
        async fn vector_coverage(
            &self,
            ty: &lakelands_api::TypeName,
        ) -> lakelands_api::Result<Vec<lakelands_api::read::VectorCoverage>> {
            self.inner.vector_coverage(ty).await
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
        let mut t = Ticker::new(lake.clone(), aging("Z"), clock);
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
        let task = tokio::spawn(Ticker::new(lake.clone(), aging("Z"), clock).run());
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
        let cfg = config("Z");
        assert_eq!(start(Some(&cfg), std::slice::from_ref(&reader)), 0);
        assert_eq!(
            start(Some(&cfg), &[writer.clone(), writer]),
            1,
            "one per lake"
        );
        assert_eq!(start(None, &[reader]), 0, "no aging section, no ticker");
    }

    #[test]
    fn an_offset_that_is_none_or_a_zero_check_is_refused() {
        for bad in ["+1:00", "01:00", "+15:00", "+01:60", "UTC", "+01"] {
            assert!(config(bad).check().is_err(), "{bad}");
        }
        for good in ["Z", "+00:00", "+14:00", "-05:30"] {
            assert!(config(good).check().is_ok(), "{good}");
        }
        let zero = AgingConfig {
            utc_offset: "Z".into(),
            check_secs: 0,
        };
        assert!(zero.check().is_err());
    }
}

//! An export read back by a verifier that shares no code with Lakelands:
//! `docs/log-format/vectors/verify.py` in its directory mode, over a lake
//! with a checkpoint (two files) and a re-key (both format versions); and
//! an export whose feed breaks, which leaves nothing behind.
//!
//! The test needs `python3`, which CI has for the conformance page; a run
//! without it fails saying so, never passes quietly.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use lakelands_api::catalog::Verification;
use lakelands_api::engine::OpenMode;
use lakelands_api::location::Limits;
use lakelands_api::seam::{ChangeFeed, FeedBatch, FeedFilter, Seams};
use lakelands_api::write::{AgeReport, Declaration, Migration, PurgeReport};
use lakelands_api::{
    Catalog, Error, Grants, LakeId, LakeStore, Location, Patch, Position, Principal, PrincipalKind,
    RecordKey, Result, SourceId, Timestamp, TypeName, WriteContext, WriteOpts,
};
use lakelands_core::export::SegmentFile;
use lakelands_engine_mem::MemoryOpener;
use serde_json::{Value, json};

fn account(key: &str) -> Value {
    json!({"name": "account", "key": [key],
           "fields": [
               {"name": "number", "type": {"kind": "text"}, "required": true},
               {"name": "iban", "type": {"kind": "text"}, "required": true},
               {"name": "holder", "type": {"kind": "text"}}]})
}

fn owner(lake: &LakeId) -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: lake.clone(),
        grants: Grants::owner(),
    }
}

async fn account_at(store: &dyn LakeStore, number: &str, iban: &str) {
    let fields = json!({"number": number, "iban": iban, "holder": "Ada"});
    let patch: Patch = serde_json::from_value(fields).unwrap();
    store
        .catalog(WriteContext::default())
        .upsert(
            &owner(store.lake()),
            &TypeName::from("account"),
            &RecordKey::from(number),
            patch,
            WriteOpts::default(),
        )
        .await
        .unwrap();
}

/// A lake in memory with `account` keyed by `number`, and two accounts.
async fn lake(name: &str) -> Arc<dyn LakeStore> {
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        ..Seams::default()
    };
    let store: Arc<dyn LakeStore> =
        lakelands_store::open(Location::memory(name), seams, OpenMode::Create)
            .await
            .unwrap();
    let mut decl = account("number");
    decl["declare"] = json!("type");
    let decl: Declaration = serde_json::from_value(decl).unwrap();
    store
        .catalog(WriteContext::default())
        .declare(&owner(store.lake()), decl)
        .await
        .unwrap();
    account_at(store.as_ref(), "1", "DE1").await;
    account_at(store.as_ref(), "2", "DE2").await;
    store
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_py_verifies_an_export_with_a_checkpoint_and_a_rekey() {
    let store = lake("export-python").await;
    store.checkpoint().await.unwrap();
    let (cat, who) = (store.catalog(WriteContext::default()), owner(store.lake()));
    let to = Migration {
        to: serde_json::from_value(account("iban")).unwrap(),
        renames: vec![],
    };
    let report = cat.migrate(&who, to.clone()).await.unwrap().data;
    assert!(report.lossless, "{report:?}");
    cat.migrate_apply(&who, to, &report.report_id)
        .await
        .unwrap();

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("export-python-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let manifest = lakelands_core::export::write(store.as_ref(), &dir)
        .await
        .unwrap();
    assert_eq!(manifest.segments.len(), 2, "{manifest:?}");
    assert_eq!(manifest.versions, [1, 2], "{manifest:?}");

    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/vectors/verify.py");
    let run = Command::new("python3").arg(&script).arg(&dir).output();
    let run = match run {
        Ok(run) => run,
        Err(e) => panic!(
            "this test runs `python3 {}` over an export and needs python3 on the PATH: {e}",
            script.display()
        ),
    };
    let said = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "verify.py refused the export:\n{said}"
    );
    let lake_says = store.verify(None, None).await.unwrap();
    let last = format!(
        "verified through position {}, hash {}",
        lake_says.through, lake_says.hash.0
    );
    assert!(
        said.trim_end().ends_with(&last),
        "verify.py ends with `{last}`, and it said:\n{said}"
    );

    // Each damage alone, its hash made right, refused at its position:
    // the first event in format 0, and the checkpoint naming the wrong
    // position before it.
    let (sealed, active) = (&manifest.segments[0], &manifest.segments[1]);
    let damages: [Damage<'_>; 2] = [
        (sealed, &|e| e["v"] = json!(0), sealed.first),
        (active, &|e| e["body"]["through"] = json!(0), active.first),
    ];
    for (segment, change, position) in damages {
        let path = dir.join(&segment.file);
        let good = std::fs::read(&path).unwrap();
        let end = good.iter().position(|b| *b == b'\n').unwrap();
        let mut event: Value = serde_json::from_slice(&good[..end]).unwrap();
        change(&mut event);
        event.as_object_mut().unwrap().remove("hash");
        event["hash"] = json!(lakelands_core::canonical::sha256_hex(&event));
        let mut bytes = lakelands_core::canonical::to_vec(&event);
        bytes.extend(&good[end..]);
        std::fs::write(&path, bytes).unwrap();
        let said = python(&script, &dir);
        assert!(
            said.contains(&format!("FAIL position {position}:")),
            "verify.py refuses the damage at {position}:\n{said}"
        );
        std::fs::write(&path, &good).unwrap();
    }
    let path = dir.join(&active.file);
    let short = dir.join(format!("log/{}.jsonl", active.first));
    std::fs::rename(&path, &short).unwrap();
    let said = python(&script, &dir);
    assert!(
        said.contains(&format!("FAIL position {}:", active.first)),
        "verify.py refuses a file not named in 20 digits:\n{said}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A damage to one file's first line, and the position it is refused at.
type Damage<'a> = (&'a SegmentFile, &'a dyn Fn(&mut Value), Position);

/// The log format's negative vector, a `known_at` going back at position
/// 6, is refused there by `verify.py` too (log-format §1.3).
#[test]
fn verify_py_refuses_the_negative_vector_whose_known_at_goes_back() {
    let vectors = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/vectors");
    let said = python(
        &vectors.join("verify.py"),
        &vectors.join("negative/known-at-goes-back"),
    );
    assert!(
        said.contains("FAIL position 6:") && said.contains("known_at"),
        "verify.py refuses the negative vector at position 6:\n{said}"
    );
}

/// What `verify.py <dir>` prints.
fn python(script: &std::path::Path, dir: &std::path::Path) -> String {
    let run = Command::new("python3")
        .arg(script)
        .arg(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&run.stdout).into_owned()
}

/// A lake's feed, answering one event a poll, that breaks at a position
/// or leaves one out.
struct FailingFeed {
    inner: Arc<dyn LakeStore>,
    fault: Fault,
}

enum Fault {
    BreaksAt(Position),
    Skips(Position),
}

#[async_trait::async_trait]
impl ChangeFeed for FailingFeed {
    async fn poll(&self, from: Position, filter: &FeedFilter, _: u32) -> Result<FeedBatch> {
        match self.fault {
            Fault::BreaksAt(at) if from >= at => Err(Error::Unavailable {
                message: format!("the feed broke at {from}"),
            }),
            Fault::Skips(at) if from == at => self.inner.feed().poll(at.next(), filter, 1).await,
            _ => self.inner.feed().poll(from, filter, 1).await,
        }
    }
    async fn ack(&self, consumer: &str, through: Position) -> Result<()> {
        self.inner.feed().ack(consumer, through).await
    }
}

struct BreaksMidway(Arc<FailingFeed>);

#[async_trait::async_trait]
impl LakeStore for BreaksMidway {
    fn lake(&self) -> &LakeId {
        self.0.inner.lake()
    }
    fn is_writer(&self) -> bool {
        self.0.inner.is_writer()
    }
    fn limits(&self) -> Limits {
        self.0.inner.limits()
    }
    async fn head(&self) -> Result<Position> {
        self.0.inner.head().await
    }
    fn catalog(&self, context: WriteContext) -> Arc<dyn Catalog> {
        self.0.inner.catalog(context)
    }
    fn feed(&self) -> Arc<dyn ChangeFeed> {
        self.0.clone()
    }
    async fn verify(
        &self,
        from: Option<Position>,
        through: Option<Position>,
    ) -> Result<Verification> {
        self.0.inner.verify(from, through).await
    }
    async fn rebuild(&self) -> Result<Position> {
        self.0.inner.rebuild().await
    }
    async fn restrict(&self, ty: &TypeName, key: &RecordKey, reason: String) -> Result<Position> {
        self.0.inner.restrict(ty, key, reason).await
    }
    async fn unrestrict(&self, ty: &TypeName, key: &RecordKey) -> Result<Position> {
        self.0.inner.unrestrict(ty, key).await
    }
    async fn checkpoint(&self) -> Result<Position> {
        self.0.inner.checkpoint().await
    }
    async fn age(&self, today: Timestamp) -> Result<AgeReport> {
        self.0.inner.age(today).await
    }
    async fn purge_source(&self, source: &SourceId, id: Option<&str>) -> Result<PurgeReport> {
        self.0.inner.purge_source(source, id).await
    }
    async fn purge_preflight(&self, source: &SourceId) -> Result<PurgeReport> {
        self.0.inner.purge_preflight(source).await
    }
    async fn compact(&self) -> Result<lakelands_api::write::CompactReport> {
        self.0.inner.compact().await
    }
    async fn vector_coverage(
        &self,
        ty: &lakelands_api::TypeName,
    ) -> Result<Vec<lakelands_api::read::VectorCoverage>> {
        self.0.inner.vector_coverage(ty).await
    }
    async fn position_at(&self, known_at: lakelands_api::Timestamp) -> Result<Option<Position>> {
        self.0.inner.position_at(known_at).await
    }
    async fn close(&self) -> Result<()> {
        self.0.inner.close().await
    }
}

/// An export whose feed breaks after the first file and part of the
/// second leaves nothing behind: no partial `log/` to pass for a shorter
/// log, and the directory as it was, so the next export is not refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_export_leaves_nothing_behind() {
    let store = lake("export-failing").await;
    let sealed_at = store.checkpoint().await.unwrap();
    account_at(store.as_ref(), "3", "DE3").await;
    account_at(store.as_ref(), "4", "DE4").await;
    let broken: Arc<dyn LakeStore> = Arc::new(BreaksMidway(Arc::new(FailingFeed {
        inner: store.clone(),
        fault: Fault::BreaksAt(sealed_at.next().next()),
    })));
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("export-failing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (absent, empty) = (base.join("absent"), base.join("empty"));
    std::fs::create_dir_all(&empty).unwrap();
    for dir in [&absent, &empty] {
        let said = lakelands_core::export::write(broken.as_ref(), dir).await;
        assert!(
            matches!(&said, Err(Error::Unavailable { message }) if message.contains("the feed broke")),
            "{said:?}"
        );
    }
    assert!(!absent.exists(), "an absent directory stays absent");
    let left: Vec<_> = std::fs::read_dir(&empty).unwrap().collect();
    assert!(left.is_empty(), "an empty directory stays empty: {left:?}");
    // The next export, through the lake's own feed, goes ahead.
    let manifest = lakelands_core::export::write(store.as_ref(), &empty)
        .await
        .unwrap();
    assert_eq!(manifest.segments.len(), 2, "{manifest:?}");
    let _ = std::fs::remove_dir_all(&base);
}

/// A feed that leaves an event out exports nothing: the gap is refused
/// by position, not written as a log that would fail to verify.
#[tokio::test(flavor = "multi_thread")]
async fn an_export_through_a_feed_that_skips_an_event_is_refused() {
    let store = lake("export-skipping").await;
    let skipping: Arc<dyn LakeStore> = Arc::new(BreaksMidway(Arc::new(FailingFeed {
        inner: store.clone(),
        fault: Fault::Skips(Position(2)),
    })));
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("export-skipping-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let said = lakelands_core::export::write(skipping.as_ref(), &dir).await;
    assert!(
        matches!(&said, Err(Error::Unavailable { message })
            if message.contains("answered position 3 where 2 was next")),
        "{said:?}"
    );
    assert!(!dir.exists(), "nothing is left behind");
}

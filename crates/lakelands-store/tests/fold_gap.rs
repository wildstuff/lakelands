//! Nothing is appended while the projection is behind the log
//! (lakelands#105).
//!
//! An append is durable before it is folded. When the fold fails, the
//! events are in the log and the projection is not: the append answers
//! the fold's error, `head()` keeps naming the last folded position, and
//! the next append first folds what the log holds past the projection. A
//! fold that keeps failing keeps the lake where it is: the next append
//! answers the same error and appends nothing, rather than folding past
//! the events it could not fold.
//!
//! The engine here is the memory engine behind a wrapper whose projection
//! refuses to store records while it is armed.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use lakelands_api::engine::{
    CanonicalEvent, EngineOpener, LakeEngine, Lease, OpenMode, ProjectionStore, ProjectionView,
    VectorIndex,
};
use lakelands_api::event::{Digest, EventBody, EventId, SegmentDigest};
use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, TypeDecl};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::{Declaration, Outcome, Proposal};
use lakelands_api::{
    Catalog, Error, Get, Grants, LakeId, LakeStore, Location, Position, Principal, PrincipalKind,
    ReadAt, RecordKey, Result, TypeName, WriteContext,
};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

/// Always fail, while armed with this many.
const ALWAYS: u32 = u32::MAX;

struct Failing {
    inner: Box<dyn LakeEngine>,
    armed: Arc<AtomicU32>,
    /// How far `folded_through` answers behind what the inner engine
    /// folded: a projection behind the log, as a reader may find it.
    lag: Arc<AtomicU64>,
    /// How many segments were sealed.
    sealed: Arc<AtomicU32>,
}

impl LakeEngine for Failing {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn lake(&self) -> &LakeId {
        self.inner.lake()
    }
    fn take_lease(&self, holder: &str) -> Result<Lease> {
        self.inner.take_lease(holder)
    }
    fn release_lease(&self, lease: &Lease) -> Result<()> {
        self.inner.release_lease(lease)
    }
    fn tip(&self) -> Result<Option<(Position, Digest)>> {
        self.inner.tip()
    }
    fn append(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.inner.append(lease, events)
    }
    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        self.inner.read(from, limit)
    }
    fn seal_segment(&self, lease: &Lease) -> Result<Option<SegmentDigest>> {
        self.sealed.fetch_add(1, Ordering::SeqCst);
        self.inner.seal_segment(lease)
    }
    fn segments(&self) -> Result<Vec<SegmentDigest>> {
        self.inner.segments()
    }
    fn projection(&self) -> &dyn ProjectionStore {
        self
    }
    fn view(&self, as_of: Option<Position>) -> Result<Box<dyn ProjectionView + '_>> {
        self.inner.view(as_of)
    }
    fn vectors(&self) -> Option<&dyn VectorIndex> {
        self.inner.vectors()
    }
}

impl ProjectionStore for Failing {
    fn begin(&self) -> Result<()> {
        self.inner.projection().begin()
    }
    fn commit(&self, through: Position) -> Result<()> {
        self.inner.projection().commit(through)
    }
    fn rollback(&self) {
        self.inner.projection().rollback()
    }
    fn folded_through(&self) -> Result<Option<Position>> {
        let lag = self.lag.load(Ordering::SeqCst);
        Ok(self
            .inner
            .projection()
            .folded_through()?
            .map(|p| Position(p.0.saturating_sub(lag))))
    }
    fn put_declaration(
        &self,
        declaration: &Declaration,
        version: u32,
        schema_hash: Option<&str>,
        at: Position,
    ) -> Result<()> {
        self.inner
            .projection()
            .put_declaration(declaration, version, schema_hash, at)
    }
    fn put_record(&self, record: &StoredRecord) -> Result<()> {
        let armed = self.armed.load(Ordering::SeqCst);
        if armed > 0 {
            if armed != ALWAYS {
                self.armed.store(armed - 1, Ordering::SeqCst);
            }
            return Err(Error::Unavailable {
                message: "the projection refused a record (planted)".into(),
            });
        }
        self.inner.projection().put_record(record)
    }
    fn put_proposal(&self, proposal: &Proposal, at: Position) -> Result<()> {
        self.inner.projection().put_proposal(proposal, at)
    }
    fn put_membership(
        &self,
        set: &str,
        ty: &TypeName,
        key: &RecordKey,
        member: bool,
        at: Position,
    ) -> Result<()> {
        self.inner
            .projection()
            .put_membership(set, ty, key, member, at)
    }
    fn put_output(&self, seam: &str, input: &Digest, output: &Value, at: Position) -> Result<()> {
        self.inner.projection().put_output(seam, input, output, at)
    }
    fn mark_applied(&self, id: &EventId, at: Position) -> Result<()> {
        self.inner.projection().mark_applied(id, at)
    }
    fn clear(&self) -> Result<()> {
        self.inner.projection().clear()
    }
}

struct FailingOpener {
    inner: MemoryOpener,
    armed: Arc<AtomicU32>,
    lag: Arc<AtomicU64>,
    sealed: Arc<AtomicU32>,
}

impl EngineOpener for FailingOpener {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn open(&self, location: &Location, mode: OpenMode) -> Result<Box<dyn LakeEngine>> {
        Ok(Box::new(Failing {
            inner: self.inner.open(location, mode)?,
            armed: self.armed.clone(),
            lag: self.lag.clone(),
            sealed: self.sealed.clone(),
        }))
    }
}

fn owner(lake: &str) -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: lake.into(),
        grants: Grants::owner(),
    }
}

struct Lake {
    name: String,
    seams: Seams,
    armed: Arc<AtomicU32>,
    lag: Arc<AtomicU64>,
    sealed: Arc<AtomicU32>,
}

/// A lake with the type `note`, and the switch that makes its folds fail.
async fn lake() -> (
    Arc<dyn LakeStore>,
    Arc<dyn Catalog>,
    Principal,
    Arc<AtomicU32>,
) {
    let (store, cat, who, l) = lake_with_switches().await;
    (store, cat, who, l.armed)
}

/// The same lake, with the switches that make its folds fail or its
/// projection answer behind, and what reopens it.
async fn lake_with_switches() -> (Arc<dyn LakeStore>, Arc<dyn Catalog>, Principal, Lake) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!("fold-gap-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let armed = Arc::new(AtomicU32::new(0));
    let lag = Arc::new(AtomicU64::new(0));
    let sealed = Arc::new(AtomicU32::new(0));
    let seams = Seams {
        engines: vec![Arc::new(FailingOpener {
            inner: MemoryOpener::new(),
            armed: armed.clone(),
            lag: lag.clone(),
            sealed: sealed.clone(),
        })],
        ..Seams::default()
    };
    let store = lakelands_store::open(
        Location::memory(name.as_str()),
        seams.clone(),
        OpenMode::Create,
    )
    .await
    .unwrap();
    let cat = store.catalog(WriteContext::default());
    let who = owner(&name);
    let text = FieldDecl {
        name: "text".into(),
        ty: FieldType::kind(StorageKind::Text),
        required: false,
        authority: Default::default(),
        derived: None,
        function: None,
        depends_on: vec![],
        on_input_change: Default::default(),
        sensitive: false,
        searchable: false,
        description: None,
        label: None,
        embed: None,
    };
    cat.declare(
        &who,
        Declaration::Type(TypeDecl {
            name: "note".into(),
            key: vec![],
            fields: vec![text],
            relations: vec![],
            view: None,
            valid_from: None,
            valid_to: None,
            field_meta: true,
            language: None,
            description: None,
            label: None,
        }),
    )
    .await
    .unwrap();
    (
        store,
        cat,
        who,
        Lake {
            name,
            seams,
            armed,
            lag,
            sealed,
        },
    )
}

async fn write(cat: &dyn Catalog, who: &Principal, key: &str) -> Result<Outcome> {
    cat.upsert(
        who,
        &"note".into(),
        &key.into(),
        [("text".into(), json!(key))].into(),
        Default::default(),
    )
    .await
}

/// The record events the log holds after `from`.
async fn logged(store: &dyn LakeStore, from: Position) -> Vec<String> {
    store
        .feed()
        .poll(from.next(), &FeedFilter::default(), 100)
        .await
        .unwrap()
        .events
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::RecordUpserted { key, .. } => Some(key.to_string()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_fold_is_folded_again_before_the_next_append() {
    let (store, cat, who, armed) = lake().await;
    let before = store.head().await.unwrap();
    armed.store(1, Ordering::SeqCst);
    let failed = write(cat.as_ref(), &who, "n1").await;
    assert!(
        matches!(failed, Err(Error::Unavailable { .. })),
        "the fold failed, and the append answered {failed:?}"
    );
    assert_eq!(
        store.head().await.unwrap(),
        before,
        "head() names the last folded position, not the unfolded append"
    );
    let next = write(cat.as_ref(), &who, "n2").await.unwrap();
    // The feed reads the log up to the head: `n1` was in the log all along.
    assert_eq!(logged(store.as_ref(), before).await, ["n1", "n2"]);
    assert_eq!(
        store.head().await.unwrap(),
        next.position(),
        "after the next append, head() is its last position"
    );
    let n1 = cat
        .get(
            &who,
            &"note".into(),
            Get {
                key: "n1".into(),
                with_meta: false,
                at: ReadAt::default(),
            },
        )
        .await
        .expect("the record whose fold failed is folded before the next append");
    assert_eq!(n1.data.fields.get("text"), Some(&json!("n1")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fold_that_keeps_failing_appends_nothing_more() {
    let (store, cat, who, armed) = lake().await;
    let before = store.head().await.unwrap();
    armed.store(ALWAYS, Ordering::SeqCst);
    assert!(write(cat.as_ref(), &who, "n1").await.is_err());
    let again = write(cat.as_ref(), &who, "n2").await;
    assert!(
        matches!(again, Err(Error::Unavailable { .. })),
        "the projection is still behind, and the next append answered {again:?}"
    );
    assert_eq!(store.head().await.unwrap(), before);
    // Once the fold works again, the lake catches up and goes on.
    armed.store(0, Ordering::SeqCst);
    let n3 = write(cat.as_ref(), &who, "n3").await.unwrap();
    assert_eq!(store.head().await.unwrap(), n3.position());
    assert_eq!(
        logged(store.as_ref(), before).await,
        ["n1", "n3"],
        "`n1` was in the log; `n2`, whose append could not catch up, was never appended"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_handle_names_the_position_its_projection_has_folded() {
    let (store, cat, who, l) = lake_with_switches().await;
    write(cat.as_ref(), &who, "n1").await.unwrap();
    let tip = store.head().await.unwrap();
    // The projection answers one position behind the log.
    l.lag.store(1, Ordering::SeqCst);
    let reader = lakelands_store::open(
        Location::memory(l.name.as_str()),
        l.seams.clone(),
        OpenMode::Read,
    )
    .await
    .unwrap();
    assert_eq!(
        reader.head().await.unwrap(),
        Position(tip.0 - 1),
        "a read-only handle names what its projection has folded, not the log's tip"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn checkpoint_and_rebuild_after_a_failed_fold_fold_it_and_publish_the_head() {
    for maintenance in ["checkpoint", "rebuild"] {
        let (store, cat, who, armed) = lake().await;
        armed.store(1, Ordering::SeqCst);
        assert!(write(cat.as_ref(), &who, "n1").await.is_err());
        let answered = match maintenance {
            "checkpoint" => store.checkpoint().await.unwrap(),
            _ => store.rebuild().await.unwrap(),
        };
        assert_eq!(
            store.head().await.unwrap(),
            answered,
            "after `{maintenance}`, head() is the position it answered"
        );
        let n1 = cat
            .get(
                &who,
                &"note".into(),
                Get {
                    key: "n1".into(),
                    with_meta: false,
                    at: ReadAt::default(),
                },
            )
            .await;
        assert!(n1.is_ok(), "after `{maintenance}`, `n1` is folded: {n1:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_checkpoint_seals_nothing_while_the_projection_cannot_catch_up() {
    let (store, cat, who, l) = lake_with_switches().await;
    l.armed.store(ALWAYS, Ordering::SeqCst);
    assert!(write(cat.as_ref(), &who, "n1").await.is_err());
    let sealed = l.sealed.load(Ordering::SeqCst);
    let checkpoint = store.checkpoint().await;
    assert!(
        checkpoint.is_err(),
        "the projection cannot catch up, and the checkpoint answered {checkpoint:?}"
    );
    assert_eq!(
        l.sealed.load(Ordering::SeqCst),
        sealed,
        "a segment was sealed over events the projection never folded"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn close_keeps_the_head_at_the_last_folded_position() {
    let (store, cat, who, armed) = lake().await;
    let before = store.head().await.unwrap();
    armed.store(1, Ordering::SeqCst);
    assert!(write(cat.as_ref(), &who, "n1").await.is_err());
    store.close().await.unwrap();
    assert_eq!(
        store.head().await.unwrap(),
        before,
        "closing gave up the lease and published the unfolded tip as the head"
    );
}

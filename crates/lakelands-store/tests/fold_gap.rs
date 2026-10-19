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
    Ranked, Rankings, VectorIndex,
};
use lakelands_api::error::{Flaw, Part, Situation};
use lakelands_api::event::{Digest, EventBody, SegmentDigest};
use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, TypeDecl};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::{Declaration, Outcome, Proposal};
use lakelands_api::{
    Catalog, Error, Find, Get, Grants, LakeId, LakeStore, Location, Position, Principal,
    PrincipalKind, ReadAt, RecordKey, Refusal, Result, TypeName, WriteContext,
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
    /// Refuse every record state from this position on (`u64::MAX`: none).
    fail_from: Arc<AtomicU64>,
    /// Refuse every read of the log from this position on (`u64::MAX`:
    /// none).
    read_fail_from: Arc<AtomicU64>,
    /// Leave the event at this position out of every read of the log
    /// (`u64::MAX`: none): a log that lost it.
    dropped: Arc<AtomicU64>,
    /// Refuse this many compactions, and count those done.
    compact: (Arc<AtomicU32>, Arc<AtomicU32>),
}

impl LakeEngine for Failing {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn rebuild_rate(&self) -> u64 {
        self.inner.rebuild_rate()
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
    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.inner.import(lease, events)
    }
    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        if from.0 >= self.read_fail_from.load(Ordering::SeqCst) {
            return Err(lakelands_api::Error::unavailable(
                lakelands_api::error::Situation::Storage {
                    doing: lakelands_api::error::Doing::Write,
                    part: lakelands_api::error::Part::Projection,
                    detail: lakelands_api::error::Detail::scrubbed(format!(
                        "the log could not be read from position {from} (planted)"
                    )),
                },
            ));
        }
        let dropped = self.dropped.load(Ordering::SeqCst);
        let mut events = self.inner.read(from, limit)?;
        events.retain(|e| e.position.0 != dropped);
        Ok(events)
    }
    fn seal_segment(
        &self,
        lease: &Lease,
        imported: bool,
        first: &mut dyn FnMut(&SegmentDigest) -> Result<Vec<CanonicalEvent>>,
    ) -> Result<Option<SegmentDigest>> {
        self.sealed.fetch_add(1, Ordering::SeqCst);
        self.inner.seal_segment(lease, imported, first)
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
    fn ranked(&self, ty: &TypeName, asked: &Rankings, k: u32) -> Result<Ranked> {
        self.inner.ranked(ty, asked, k)
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
        if record.position.0 >= self.fail_from.load(Ordering::SeqCst) {
            return Err(lakelands_api::Error::unavailable(
                lakelands_api::error::Situation::Storage {
                    doing: lakelands_api::error::Doing::Write,
                    part: lakelands_api::error::Part::Projection,
                    detail: lakelands_api::error::Detail::scrubbed(format!(
                        "the projection refused a record at position {} (planted)",
                        record.position
                    )),
                },
            ));
        }
        let armed = self.armed.load(Ordering::SeqCst);
        if armed > 0 {
            if armed != ALWAYS {
                self.armed.store(armed - 1, Ordering::SeqCst);
            }
            return Err(lakelands_api::Error::unavailable(
                lakelands_api::error::Situation::Storage {
                    doing: lakelands_api::error::Doing::Write,
                    part: lakelands_api::error::Part::Projection,
                    detail: lakelands_api::error::Detail::scrubbed(
                        "the projection refused a record (planted)",
                    ),
                },
            ));
        }
        self.inner.projection().put_record(record)
    }
    fn end_head_only(&self, name: &TypeName, at: Position) -> Result<()> {
        self.inner.projection().end_head_only(name, at)
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
    fn mark_applied(&self, applied: &lakelands_api::engine::Applied) -> Result<()> {
        self.inner.projection().mark_applied(applied)
    }
    fn clear(&self) -> Result<()> {
        self.inner.projection().clear()
    }
    fn compact(&self) -> Result<lakelands_api::engine::Reclaimed> {
        let (fails, done) = &self.compact;
        if fails.load(Ordering::SeqCst) > 0 {
            fails.fetch_sub(1, Ordering::SeqCst);
            let lake = self.lake().to_string();
            return Err(Error::unavailable(lakelands_api::error::Situation::Busy {
                lake,
            }));
        }
        done.fetch_add(1, Ordering::SeqCst);
        self.inner.projection().compact()
    }
}

struct FailingOpener {
    inner: MemoryOpener,
    armed: Arc<AtomicU32>,
    lag: Arc<AtomicU64>,
    sealed: Arc<AtomicU32>,
    fail_from: Arc<AtomicU64>,
    read_fail_from: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    compact: (Arc<AtomicU32>, Arc<AtomicU32>),
}

impl EngineOpener for FailingOpener {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn log_ends(&self, location: &Location) -> Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        self.inner.log_ends(location)
    }
    fn open(&self, location: &Location, mode: OpenMode) -> Result<Box<dyn LakeEngine>> {
        Ok(Box::new(Failing {
            inner: self.inner.open(location, mode)?,
            armed: self.armed.clone(),
            lag: self.lag.clone(),
            sealed: self.sealed.clone(),
            fail_from: self.fail_from.clone(),
            read_fail_from: self.read_fail_from.clone(),
            dropped: self.dropped.clone(),
            compact: self.compact.clone(),
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
    fail_from: Arc<AtomicU64>,
    read_fail_from: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    compact: (Arc<AtomicU32>, Arc<AtomicU32>),
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
    let fail_from = Arc::new(AtomicU64::new(u64::MAX));
    let read_fail_from = Arc::new(AtomicU64::new(u64::MAX));
    let dropped = Arc::new(AtomicU64::new(u64::MAX));
    let compact = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
    let seams = Seams {
        engines: vec![Arc::new(FailingOpener {
            inner: MemoryOpener::new(),
            armed: armed.clone(),
            lag: lag.clone(),
            sealed: sealed.clone(),
            fail_from: fail_from.clone(),
            read_fail_from: read_fail_from.clone(),
            dropped: dropped.clone(),
            compact: compact.clone(),
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
        annotations: Default::default(),
        name: "text".into(),
        ty: FieldType::kind(StorageKind::Text),
        required: false,
        authority: Default::default(),
        derived: None,
        function: None,
        computed_at: None,
        depends_on: vec![],
        on_input_change: Default::default(),
        initial: None,
        sensitive: false,
        searchable: false,
        erasable: false,
        description: None,
        label: None,
        embed: None,
    };
    cat.declare(
        &who,
        Declaration::Type(TypeDecl {
            constraints: Default::default(),
            annotations: Default::default(),
            insert_only: false,
            retired_by: Default::default(),
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
            fail_from,
            read_fail_from,
            dropped,
            compact,
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
                params: Default::default(),
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
                    params: Default::default(),
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

/// A read as of a position takes its clock from the event there
/// (lakelands#571); a log that holds no event at a folded position is
/// damaged, and the read is refused, naming it, never answered under the
/// wall clock.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_as_of_a_position_the_log_lost_is_refused_as_damaged() {
    let (_store, cat, who, l) = lake_with_switches().await;
    let at = write(cat.as_ref(), &who, "a").await.unwrap().position();
    write(cat.as_ref(), &who, "b").await.unwrap();
    l.dropped.store(at.0, Ordering::SeqCst);
    let e = notes_at(cat.as_ref(), &who, Some(at)).await.unwrap_err();
    let lost = matches!(e.situation(), Some(Situation::Damaged {
        part: Part::Log,
        position: Some(p),
        flaw: Flaw::NoEvent { position },
    }) if *p == at && *position == at);
    assert!(lost, "{e:?}");
    notes_at(cat.as_ref(), &who, None).await.unwrap();
}

/// A page of the log, as the store folds it (lakelands#130).
const PAGE: u64 = 1024;

/// The first `limit` notes as of `as_of` (the head when `None`), as the
/// reply serialises with its cursor's clock left out, or the refusal it
/// is.
async fn notes_at(cat: &dyn Catalog, who: &Principal, as_of: Option<Position>) -> Result<String> {
    let find = Find {
        limit: Some(50),
        at: ReadAt {
            as_of,
            valid_at: None,
            clock: None,
        },
        ..Find::default()
    };
    let mut reply = serde_json::to_value(cat.find(who, &"note".into(), find).await?).unwrap();
    // The page and its cursor's place, without the clock the cursor carries:
    // a read at the head and one as of its position read two clocks
    // (lakelands#571).
    if let Some(c) = reply["data"]["next_cursor"].as_str() {
        let bytes: Vec<u8> = (0..c.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&c[i..i + 2], 16).unwrap())
            .collect();
        let mut state: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        state.as_object_mut().unwrap().remove("now");
        reply["data"]["next_cursor"] = state;
    }
    Ok(reply.to_string())
}

/// A rebuild folds the log a page at a time, each page committed. One that
/// stops in its third page leaves the projection folded through the end
/// of its second, and `head()` names that position: a read there, or at
/// the head, answers as it did before the rebuild; a read past it (the
/// head before the rebuild, or the third page's first position) is
/// refused as out of range; and the next open folds the rest, after which
/// every read answers as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_rebuild_stopped_in_a_page_answers_through_the_last_page_and_the_next_open_finishes_it() {
    let (store, cat, who, l) = lake_with_switches().await;
    for i in 0..2600 {
        write(cat.as_ref(), &who, &format!("n{i:04}"))
            .await
            .unwrap();
    }
    let head = store.head().await.unwrap();
    assert!(
        head.0 >= 3 * PAGE - 500,
        "the log spans three pages: {head}"
    );
    let second_page_end = Position(2 * PAGE - 1);
    let at_page = notes_at(cat.as_ref(), &who, Some(second_page_end))
        .await
        .unwrap();
    let at_head = notes_at(cat.as_ref(), &who, None).await.unwrap();

    l.fail_from.store(2 * PAGE + 5, Ordering::SeqCst);
    let failed = store.rebuild().await;
    assert!(
        matches!(failed, Err(Error::Unavailable { .. })),
        "the third page's fold failed, and the rebuild answered {failed:?}"
    );
    assert_eq!(
        store.head().await.unwrap(),
        second_page_end,
        "head() names the last position folded: the end of the second page"
    );
    assert_eq!(
        notes_at(cat.as_ref(), &who, None).await.unwrap(),
        at_page,
        "a read at the head answers the lake as of the second page's end"
    );
    for as_of in [Some(head), Some(Position(2 * PAGE))] {
        let past = notes_at(cat.as_ref(), &who, as_of).await;
        assert!(
            matches!(
                past,
                Err(Error::Refused {
                    reason: Refusal::OutOfRange { .. },
                    ..
                })
            ),
            "a read as of {as_of:?}, past the last page folded, is out of range: {past:?}"
        );
    }
    assert_eq!(
        notes_at(cat.as_ref(), &who, Some(second_page_end))
            .await
            .unwrap(),
        at_page,
        "a read at the end of the last page folded answers as before the rebuild"
    );

    l.fail_from.store(u64::MAX, Ordering::SeqCst);
    store.close().await.unwrap();
    drop((store, cat));
    let store = lakelands_store::open(
        Location::memory(l.name.as_str()),
        l.seams.clone(),
        OpenMode::Write,
    )
    .await
    .unwrap();
    let cat = store.catalog(WriteContext::default());
    // The open takes the lease, one event past the rebuild's head.
    assert_eq!(store.head().await.unwrap(), head.next());
    assert_eq!(
        notes_at(cat.as_ref(), &who, Some(head)).await.unwrap(),
        at_head,
        "after the next open folded the rest, a read at the old head answers as before"
    );
}

/// A rebuild whose read of the log fails in its third page stops as one
/// whose fold fails does: the projection is folded through the second
/// page, `head()` names that position, and the lake is behind. An append
/// then first folds the rest; while the log still cannot be read it
/// answers that error and appends nothing, and once it can, the append
/// folds the rest, lands after the log's last event, and every read at
/// the old head answers as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_rebuild_whose_log_read_fails_leaves_the_lake_behind_until_a_fold_catches_up() {
    let (store, cat, who, l) = lake_with_switches().await;
    for i in 0..2600 {
        write(cat.as_ref(), &who, &format!("n{i:04}"))
            .await
            .unwrap();
    }
    let head = store.head().await.unwrap();
    let at_head = notes_at(cat.as_ref(), &who, None).await.unwrap();

    l.read_fail_from.store(2 * PAGE, Ordering::SeqCst);
    let failed = store.rebuild().await;
    assert!(
        matches!(failed, Err(Error::Unavailable { .. })),
        "the third page could not be read, and the rebuild answered {failed:?}"
    );
    assert_eq!(store.head().await.unwrap(), Position(2 * PAGE - 1));

    let refused = write(cat.as_ref(), &who, "late").await;
    assert!(
        matches!(refused, Err(Error::Unavailable { .. })),
        "the lake is behind and the log still cannot be read past the second page; the \
         append answered {refused:?}"
    );
    assert_eq!(
        store.head().await.unwrap(),
        Position(2 * PAGE - 1),
        "nothing moved"
    );

    l.read_fail_from.store(u64::MAX, Ordering::SeqCst);
    let landed = write(cat.as_ref(), &who, "late").await.unwrap();
    assert_eq!(
        landed.position(),
        head.next(),
        "the append folded the rest first, and lands right after the log's last event: the \
         refused append wrote nothing"
    );
    assert_eq!(
        notes_at(cat.as_ref(), &who, Some(head)).await.unwrap(),
        at_head,
        "every event the rebuild could not read is folded"
    );
}

/// A forgetting stopped between its rebuild and its reclaim
/// (`docs/decisions/058-a-source-may-be-forgotten.md`) is reclaimed by the
/// act's repeat, which appends nothing, or by the next writer's open, and
/// an open after that reclaims nothing again: only
/// an engine's fault can stop the reclaim, so the suite holds the other
/// stops and this one stands here.
#[tokio::test(flavor = "multi_thread")]
async fn a_forgetting_stopped_before_its_reclaim_is_reclaimed_by_the_next_open() {
    let (store, cat, who, l) = lake_with_switches().await;
    write(cat.as_ref(), &who, "n1").await.unwrap();
    let (fails, done) = &l.compact;
    fails.store(1, Ordering::SeqCst);
    let stopped = store.forget_source(&"crm".into(), None).await;
    assert!(
        stopped.is_ok_and(|r| r.reclaim_owed.is_some() && r.reclaimed.is_none()),
        "the act stopped at its reclaim answers, owing it (lakelands#1179)"
    );
    let before = done.load(Ordering::SeqCst);
    // The act's repeat finishes what is owed, and appends nothing again.
    let head = store.head().await.unwrap();
    let finished = store.forget_source(&"crm".into(), None).await.unwrap();
    assert!(
        finished.reclaimed.is_some(),
        "the repeat reclaims: {finished:?}"
    );
    assert_eq!(
        store.head().await.unwrap(),
        head,
        "the repeat appends nothing"
    );
    assert_eq!(
        done.load(Ordering::SeqCst),
        before + 1,
        "the repeat reclaims once"
    );
    let again = store.forget_source(&"crm".into(), None).await.unwrap();
    assert!(
        again.reclaimed.is_none(),
        "a repeat of the done act reclaims nothing: {again:?}"
    );
    // A stop the open finishes.
    fails.store(1, Ordering::SeqCst);
    let owed = store.forget_source(&"erp".into(), None).await.unwrap();
    assert!(owed.reclaim_owed.is_some(), "{owed:?}");
    let before = done.load(Ordering::SeqCst);
    let reopen = || async {
        lakelands_store::open(
            Location::memory(l.name.as_str()),
            l.seams.clone(),
            OpenMode::Write,
        )
        .await
        .unwrap()
    };
    store.close().await.unwrap();
    drop((store, cat));
    let store = reopen().await;
    assert_eq!(
        done.load(Ordering::SeqCst),
        before + 1,
        "the open reclaims what the act owed"
    );
    store.close().await.unwrap();
    drop(store);
    let _store = reopen().await;
    assert_eq!(
        done.load(Ordering::SeqCst),
        before + 1,
        "the next open owes nothing"
    );
}

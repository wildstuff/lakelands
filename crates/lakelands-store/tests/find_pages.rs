//! A `find` page reads a page, not the type (lakelands#134).
//!
//! The store asks the engine for the page it answers: its order, the row
//! after which it starts, and `limit + 1` rows. A filter the engine cannot
//! apply is applied by the store over engine pages read one after another.
//! The engine here is the memory engine behind a wrapper that counts the
//! rows its views hand the store: the whole type crossing the seam for one
//! page is what the tests refuse.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lakelands_api::engine::{
    CanonicalEvent, Declared, EngineOpener, LakeEngine, Lease, OpenMode, ProjectionStore,
    ProjectionView, Scan, VectorIndex,
};
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::expr::Condition;
use lakelands_api::query::{Edge, Walk};
use lakelands_api::read::{Find, Found};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::seam::Seams;
use lakelands_api::write::{Declaration, Proposal, ProposalFilter};
use lakelands_api::{
    Catalog, Grants, LakeId, LakeStore, Location, Position, Principal, PrincipalKind, RecordKey,
    RelationName, Result, TypeName, WriteContext,
};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

/// What the views have handed out: the rows of every `scan`, and the most
/// one `scan` handed out.
#[derive(Default)]
struct Handed {
    rows: AtomicUsize,
    most: AtomicUsize,
}

struct Counting {
    inner: Box<dyn LakeEngine>,
    handed: Arc<Handed>,
}

struct CountingView<'a> {
    inner: Box<dyn ProjectionView + 'a>,
    handed: Arc<Handed>,
}

impl ProjectionView for CountingView<'_> {
    fn position(&self) -> Option<Position> {
        self.inner.position()
    }
    fn declarations(&self) -> Result<Vec<Declared>> {
        self.inner.declarations()
    }
    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        self.inner.get(ty, key)
    }
    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        self.inner.get_many(ty, keys)
    }
    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>> {
        let rows = self.inner.scan(ty, scan)?;
        self.handed.rows.fetch_add(rows.len(), Ordering::SeqCst);
        self.handed.most.fetch_max(rows.len(), Ordering::SeqCst);
        Ok(rows)
    }
    fn count(&self, ty: &TypeName, scan: &Scan) -> Result<u64> {
        self.inner.count(ty, scan)
    }
    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Vec<Edge>>> {
        self.inner.edges(ty, keys, relation, walk)
    }
    fn lexical(
        &self,
        ty: &TypeName,
        text: &str,
        fields: Option<&[lakelands_api::id::FieldName]>,
        k: u32,
    ) -> Result<Vec<(RecordKey, f64)>> {
        self.inner.lexical(ty, text, fields, k)
    }
    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        self.inner.proposals(filter)
    }
    fn is_member(&self, set: &str, ty: &TypeName, key: &RecordKey) -> Result<bool> {
        self.inner.is_member(set, ty, key)
    }
    fn members(&self, set: &str, ty: &TypeName) -> Result<BTreeSet<RecordKey>> {
        self.inner.members(set, ty)
    }
    fn applied(&self, id: &EventId) -> Result<Option<Position>> {
        self.inner.applied(id)
    }
    fn output(&self, seam: &str, input: &Digest) -> Result<Option<Value>> {
        self.inner.output(seam, input)
    }
}

impl LakeEngine for Counting {
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
    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.inner.import(lease, events)
    }
    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        self.inner.read(from, limit)
    }
    fn seal_segment(
        &self,
        lease: &Lease,
        imported: bool,
        first: &mut dyn FnMut(&SegmentDigest) -> Result<Vec<CanonicalEvent>>,
    ) -> Result<Option<SegmentDigest>> {
        self.inner.seal_segment(lease, imported, first)
    }
    fn segments(&self) -> Result<Vec<SegmentDigest>> {
        self.inner.segments()
    }
    fn projection(&self) -> &dyn ProjectionStore {
        self.inner.projection()
    }
    fn view(&self, as_of: Option<Position>) -> Result<Box<dyn ProjectionView + '_>> {
        Ok(Box::new(CountingView {
            inner: self.inner.view(as_of)?,
            handed: self.handed.clone(),
        }))
    }
    fn vectors(&self) -> Option<&dyn VectorIndex> {
        self.inner.vectors()
    }
}

struct CountingOpener {
    inner: MemoryOpener,
    handed: Arc<Handed>,
}

impl EngineOpener for CountingOpener {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn open(&self, location: &Location, mode: OpenMode) -> Result<Box<dyn LakeEngine>> {
        Ok(Box::new(Counting {
            inner: self.inner.open(location, mode)?,
            handed: self.handed.clone(),
        }))
    }
}

const NOTES: usize = 1000;

/// A lake holding `NOTES` notes, `n0000` to `n0999`, and the count of the
/// rows its views hand out.
async fn notes(lake: &str) -> (Arc<dyn LakeStore>, Arc<dyn Catalog>, Principal, Arc<Handed>) {
    let handed = Arc::new(Handed::default());
    let seams = Seams {
        engines: vec![Arc::new(CountingOpener {
            inner: MemoryOpener::new(),
            handed: handed.clone(),
        })],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory(lake), seams, OpenMode::Create)
        .await
        .unwrap();
    let cat = store.catalog(WriteContext::default());
    let who = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: lake.into(),
        grants: Grants::owner(),
    };
    let note: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "note",
        "fields": [{"name": "text", "type": {"kind": "text"}}]
    }))
    .unwrap();
    cat.declare(&who, note).await.unwrap();
    for i in 0..NOTES {
        let key = format!("n{i:04}");
        cat.upsert(
            &who,
            &"note".into(),
            &key.as_str().into(),
            [("text".into(), json!(key))].into(),
            Default::default(),
        )
        .await
        .unwrap();
    }
    (store, cat, who, handed)
}

fn page(filter: Option<&str>, cursor: Option<lakelands_api::Cursor>) -> Find {
    Find {
        filter: filter.map(|f| Condition::Text(f.into())),
        limit: Some(10),
        cursor,
        ..Find::default()
    }
}

async fn keys_of(
    cat: &dyn Catalog,
    who: &Principal,
    find: Find,
) -> (Vec<String>, Option<lakelands_api::Cursor>) {
    match cat.find(who, &"note".into(), find).await.unwrap().data {
        Found::Records {
            rows, next_cursor, ..
        } => (
            rows.into_iter().map(|r| r.key.to_string()).collect(),
            next_cursor,
        ),
        Found::Groups { .. } => panic!("a find without an aggregate answers records"),
    }
}

/// A page of 10 over 1 000 records is answered from at most 11 rows,
/// without a filter and with one only the store can evaluate, and a walk
/// of the whole type never hands more than 11 rows for one page.
#[tokio::test(flavor = "multi_thread")]
async fn a_page_of_ten_is_answered_from_eleven_rows_not_the_type() {
    let (_store, cat, who, handed) = notes("find-pages").await;
    // `known_at` is provenance, which only the store reads: this filter
    // holds for every note and stays in the store.
    let everything = "known_at(text) > '2000-01-01'";
    for filter in [None, Some(everything)] {
        handed.rows.store(0, Ordering::SeqCst);
        let (keys, next) = keys_of(cat.as_ref(), &who, page(filter, None)).await;
        let want: Vec<String> = (0..10).map(|i| format!("n{i:04}")).collect();
        assert_eq!(keys, want, "{filter:?}");
        assert!(next.is_some(), "{filter:?}: more pages follow");
        let rows = handed.rows.load(Ordering::SeqCst);
        assert!(
            rows <= 11,
            "{filter:?}: a page of 10 crossed the seam as {rows} rows"
        );
    }

    // Walking every page: each record once, in order, and no page read
    // from more than 11 rows at a time.
    for filter in [None, Some(everything)] {
        handed.most.store(0, Ordering::SeqCst);
        let (mut all, mut cursor) = (Vec::new(), None);
        loop {
            let (keys, next) = keys_of(cat.as_ref(), &who, page(filter, cursor)).await;
            all.extend(keys);
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        let want: Vec<String> = (0..NOTES).map(|i| format!("n{i:04}")).collect();
        assert_eq!(all, want, "{filter:?}");
        let most = handed.most.load(Ordering::SeqCst);
        assert!(most <= 11, "{filter:?}: one scan handed {most} rows");
    }
}

/// A selective filter only the store can evaluate reads engine pages one
/// after another until its page is full: its cost is the rows read, never
/// the type at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_selective_store_filter_reads_engine_pages_one_after_another() {
    let (_store, cat, who, handed) = notes("find-pages-selective").await;
    let hundredths: Vec<String> = (0..10).map(|i| format!("'n{:04}'", i * 100)).collect();
    let every_hundredth = format!(
        "known_at(text) > '2000-01-01' and text in ({})",
        hundredths.join(", ")
    );
    handed.most.store(0, Ordering::SeqCst);
    let (keys, next) = keys_of(cat.as_ref(), &who, page(Some(&every_hundredth), None)).await;
    let want: Vec<String> = (0..10).map(|i| format!("n{:04}", i * 100)).collect();
    assert_eq!(keys, want);
    assert!(next.is_none(), "ten hundredths are all there are");
    let most = handed.most.load(Ordering::SeqCst);
    assert!(most <= 11, "one scan handed {most} rows");
}

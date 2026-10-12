//! A `find` page reads a page, not the type (lakelands#134), and a
//! pattern's page of rows reads a page of starts (lakelands#940), and a
//! walk over a derived relation asks one scan a page of them
//! (lakelands#971).
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
    ProjectionView, Ranked, Rankings, Scan, VectorIndex,
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
/// one `scan` handed out; and how often they were asked for records by
/// key and for edges.
#[derive(Default)]
struct Handed {
    rows: AtomicUsize,
    most: AtomicUsize,
    gets: AtomicUsize,
    edges: AtomicUsize,
    scans: AtomicUsize,
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
        self.handed.gets.fetch_add(1, Ordering::SeqCst);
        self.inner.get(ty, key)
    }
    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        self.handed.gets.fetch_add(1, Ordering::SeqCst);
        self.inner.get_many(ty, keys)
    }
    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>> {
        let rows = self.inner.scan(ty, scan)?;
        self.handed.scans.fetch_add(1, Ordering::SeqCst);
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
        self.handed.edges.fetch_add(1, Ordering::SeqCst);
        self.inner.edges(ty, keys, relation, walk)
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
    fn output(&self, seam: &str, input: &Digest) -> Result<Option<(Position, Value)>> {
        self.inner.output(seam, input)
    }

    fn matching(
        &self,
        filter: &lakelands_api::seam::FeedFilter,
        from: Position,
        through: Position,
        limit: u32,
    ) -> Result<lakelands_api::engine::Matched> {
        self.inner.matching(filter, from, through, limit)
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
    fn ranked(&self, ty: &TypeName, asked: &Rankings, k: u32) -> Result<Ranked> {
        self.inner.ranked(ty, asked, k)
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
    fn log_ends(&self, location: &Location) -> Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        self.inner.log_ends(location)
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

/// A lake of 100 customers, `x` among them, and `n` invoices billed to
/// customer `i % 100`, on the counting engine.
async fn invoices(lake: &str, n: usize) -> (Arc<dyn Catalog>, Principal, Arc<Handed>) {
    let (store, cat, who, handed) = notes(lake).await;
    drop(store);
    for d in [
        json!({"declare": "type", "name": "cust", "fields": [{"name": "name", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "inv", "fields": [
            {"name": "amount", "type": {"kind": "number"}},
            {"name": "customer", "type": {"kind": "reference", "target": "cust"}}],
            "relations": [{"name": "billed_to", "target": "cust", "via": "customer"}]}),
    ] {
        cat.declare(&who, serde_json::from_value(d).unwrap())
            .await
            .unwrap();
    }
    let put = |ty: &'static str, key: String, fields: serde_json::Value| {
        let (cat, who) = (cat.clone(), who.clone());
        async move {
            let patch = serde_json::from_value(fields).unwrap();
            let key = key.as_str().into();
            cat.upsert(&who, &ty.into(), &key, patch, Default::default())
                .await
                .unwrap();
        }
    };
    for c in 0..100 {
        let name = if c == 0 {
            "x".to_owned()
        } else {
            format!("n{c:02}")
        };
        put("cust", format!("c{c:02}"), json!({"name": name})).await;
    }
    for i in 0..n {
        let fields = json!({"amount": (i * 37) % 1000, "customer": format!("c{:02}", i % 100)});
        put("inv", format!("i{i:05}"), fields).await;
    }
    (cat, who, handed)
}

/// What a read asks of the engine: rows scanned, reads by key, edges.
fn asked(handed: &Handed, read: impl FnOnce()) -> (usize, usize, usize) {
    for c in [&handed.rows, &handed.gets, &handed.edges] {
        c.store(0, Ordering::SeqCst);
    }
    read();
    let n = |c: &AtomicUsize| c.load(Ordering::SeqCst);
    (n(&handed.rows), n(&handed.gets), n(&handed.edges))
}

/// A first page of 50 rows from 10 000 invoices out to their customer
/// reads one page of 256 starts and asks the engine for their edges and
/// their customers a few times, where a walk a start asked four times a
/// start; a middle page and a page ordered by the start's own field the
/// same; and a step `where` that keeps one customer in a hundred walks
/// the starts a page at a time until its page is full, with one read and
/// one edge question a page of starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_patterns_page_of_rows_reads_a_page_of_starts() {
    use lakelands_api::query::{Pattern, QueryResult};
    let n = 10_000;
    let (cat, who, handed) = invoices("pattern-pages", n).await;
    let rows = |filter: Option<&str>,
                order: Option<&str>,
                cursor: Option<lakelands_api::Cursor>| {
        let mut step = json!({"relation": "billed_to", "walk": "out"});
        if let Some(f) = filter {
            step["where"] = json!(f);
        }
        let mut p = json!({"start": {"type": "inv"}, "steps": [step], "project": {"as": "rows"}, "limit": 50});
        if let Some(o) = order {
            p["order_by"] = json!([{"field": o}]);
        }
        let mut p: Pattern = serde_json::from_value(p).unwrap();
        p.cursor = cursor;
        p
    };
    let query = |p: Pattern| {
        let (cat, who) = (cat.clone(), who.clone());
        let answer = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            rt.block_on(cat.query(&who, p)).unwrap().data
        });
        match answer.join().unwrap() {
            QueryResult::Rows { rows, next_cursor } => (rows.len(), next_cursor),
            _ => panic!("rows"),
        }
    };
    let mut cursor = None;
    let first = asked(&handed, || {
        let (len, next) = query(rows(None, None, None));
        assert_eq!(len, 50);
        cursor = next;
    });
    let middle = asked(&handed, || {
        assert_eq!(query(rows(None, None, cursor.clone())).0, 50)
    });
    let ordered = asked(&handed, || {
        assert_eq!(query(rows(None, Some("inv.amount"), None)).0, 50)
    });
    for (what, (scanned, gets, edges)) in
        [("first", first), ("middle", middle), ("ordered", ordered)]
    {
        assert!(
            scanned <= 2 * 256 && gets <= 4 && edges <= 2,
            "a {what} page of 50 rows reads a page of starts: {scanned} rows scanned, {gets} reads \
             by key, {edges} edge questions"
        );
    }
    let (scanned, gets, edges) = asked(&handed, || {
        let (len, next) = query(rows(Some("name = 'x'"), None, None));
        assert_eq!((len, next.is_some()), (50, true));
    });
    let pages = n.div_ceil(256);
    assert!(
        scanned <= n + 256 && gets <= pages && edges <= pages,
        "a step `where` that keeps one customer in a hundred walks the starts a page at a time: \
         {scanned} rows scanned, {gets} reads by key, {edges} edge questions over {pages} pages"
    );
}

/// A walk over a derived relation asks the engine one scan for a page of
/// near records, not one a record: rows in from 100 terms to their 2 000
/// invoices, and out from a page of 256 invoices to their terms
/// (lakelands#971).
#[tokio::test(flavor = "multi_thread")]
async fn a_derived_walk_asks_one_scan_a_page_of_starts() {
    use lakelands_api::query::{Pattern, QueryResult};
    let (store, cat, who, handed) = notes("derived-walk").await;
    drop(store);
    for d in [
        json!({"declare": "type", "name": "terms", "key": ["no"], "fields": [{"name": "no", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "inv", "fields": [{"name": "terms_no", "type": {"kind": "text"}}],
            "relations": [{"name": "of_terms", "target": "terms", "inverse": "invoices", "derived": {"match": [{"from": "terms_no", "to": "no"}]}}]}),
    ] {
        cat.declare(&who, serde_json::from_value(d).unwrap())
            .await
            .unwrap();
    }
    let put = |ty: &'static str, key: String, fields: serde_json::Value| {
        let (cat, who) = (cat.clone(), who.clone());
        async move {
            let patch = serde_json::from_value(fields).unwrap();
            cat.upsert(
                &who,
                &ty.into(),
                &key.as_str().into(),
                patch,
                Default::default(),
            )
            .await
            .unwrap();
        }
    };
    for t in 0..100 {
        put(
            "terms",
            format!("t{t:03}"),
            json!({"no": format!("t{t:03}")}),
        )
        .await;
    }
    for i in 0..2000 {
        put(
            "inv",
            format!("i{i:05}"),
            json!({"terms_no": format!("t{:03}", i % 100)}),
        )
        .await;
    }
    let rows = |start: &str, relation: &str, limit: u32| -> Pattern {
        serde_json::from_value(
            json!({"start": {"type": start}, "steps": [{"relation": relation, "walk": "out"}],
            "project": {"as": "rows"}, "limit": limit}),
        )
        .unwrap()
    };
    for (what, p, want) in [
        ("in from every terms", rows("terms", "invoices", 1000), 1000),
        ("out from every invoice", rows("inv", "of_terms", 50), 50),
    ] {
        handed.scans.store(0, Ordering::SeqCst);
        let QueryResult::Rows { rows, .. } = cat.query(&who, p).await.unwrap().data else {
            panic!("rows");
        };
        let scans = handed.scans.load(Ordering::SeqCst);
        assert!(
            rows.len() == want && scans <= 4,
            "{what}: {} rows asked {scans} scans of the engine; a page of starts asks one scan of them and one for their matches",
            rows.len()
        );
    }
}

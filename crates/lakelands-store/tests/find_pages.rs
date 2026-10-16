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
    /// Of `gets`, those that asked for one record alone.
    ones: AtomicUsize,
    /// The views opened at a past position.
    pasts: AtomicUsize,
    /// The declarations read whole at a past position.
    declared_then: AtomicUsize,
    /// The ranges of replaced declarations read.
    replaced: AtomicUsize,
    /// The proposals the engine answered.
    proposed: AtomicUsize,
    /// The pages of proposals asked, and the whole listings.
    pages: AtomicUsize,
    wholes: AtomicUsize,
    /// The counts asked.
    counts: AtomicUsize,
}

struct Counting {
    inner: Box<dyn LakeEngine>,
    handed: Arc<Handed>,
}

struct CountingView<'a> {
    inner: Box<dyn ProjectionView + 'a>,
    handed: Arc<Handed>,
    past: bool,
}

impl ProjectionView for CountingView<'_> {
    fn position(&self) -> Option<Position> {
        self.inner.position()
    }
    fn declarations(&self) -> Result<Vec<Declared>> {
        if self.past {
            self.handed.declared_then.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.declarations()
    }
    fn replaced(&self, from: Position, to: Position) -> Result<Option<Vec<Declared>>> {
        self.handed.replaced.fetch_add(1, Ordering::SeqCst);
        self.inner.replaced(from, to)
    }
    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        self.handed.gets.fetch_add(1, Ordering::SeqCst);
        self.handed.ones.fetch_add(1, Ordering::SeqCst);
        self.inner.get(ty, key)
    }
    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        self.handed.gets.fetch_add(1, Ordering::SeqCst);
        if keys.len() == 1 {
            self.handed.ones.fetch_add(1, Ordering::SeqCst);
        }
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
        self.handed.counts.fetch_add(1, Ordering::SeqCst);
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
        let all = self.inner.proposals(filter)?;
        self.handed.wholes.fetch_add(1, Ordering::SeqCst);
        self.handed.proposed.fetch_add(all.len(), Ordering::SeqCst);
        Ok(all)
    }
    fn proposals_before(
        &self,
        filter: &ProposalFilter,
        before: Option<Position>,
    ) -> Result<Option<Vec<Proposal>>> {
        let page = self.inner.proposals_before(filter, before)?;
        self.handed.pages.fetch_add(1, Ordering::SeqCst);
        let n = page.as_ref().map_or(0, Vec::len);
        self.handed.proposed.fetch_add(n, Ordering::SeqCst);
        Ok(page)
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
        if as_of.is_some() {
            self.handed.pasts.fetch_add(1, Ordering::SeqCst);
        }
        Ok(Box::new(CountingView {
            inner: self.inner.view(as_of)?,
            handed: self.handed.clone(),
            past: as_of.is_some(),
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

/// A step's `where` reads the records the walk's visibility read for the
/// batch, in every walk that reads one, not each record again on its own
/// (lakelands#1027): a chain's graph, a reach and rows ask no single
/// record of the engine, and each walks a step's records together, one
/// edge question a step. A neighbourhood's starts are its first records,
/// so its first page reads those and one more (lakelands#963), and so
/// does every later page inside them, walking back to the starts' type
/// alone (lakelands#1050).
#[tokio::test]
async fn a_walk_with_a_where_reads_no_record_alone() {
    use lakelands_api::query::{Pattern, QueryResult};
    let (store, cat, who, handed) = notes("where-cache").await;
    drop(store);
    let ty = |name: &str, own: &str, up: Option<(&str, &str, &str)>| {
        let mut t = json!({"declare": "type", "name": name, "fields": [{"name": own, "type": {"kind": "text"}}]});
        if let Some((rel, field, target)) = up {
            t["fields"]
                .as_array_mut()
                .unwrap()
                .push(json!({"name": field, "type": {"kind": "reference", "target": target}}));
            t["relations"] = json!([{"name": rel, "target": target, "via": field}]);
        }
        serde_json::from_value::<Declaration>(t).unwrap()
    };
    for d in [
        ty("region", "name", None),
        ty("customer", "name", Some(("in_region", "region", "region"))),
        ty("invoice", "status", Some(("of", "cust", "customer"))),
        // A payment follows the one before it, and a refund `pays` one:
        // a relation onto the start's own type, and another type's of the
        // same name as a step's (lakelands#1050).
        serde_json::from_value(json!({"declare": "type", "name": "payment", "fields": [
            {"name": "amount", "type": {"kind": "text"}},
            {"name": "inv", "type": {"kind": "reference", "target": "invoice"}},
            {"name": "prev", "type": {"kind": "reference", "target": "payment"}}],
            "relations": [{"name": "pays", "target": "invoice", "via": "inv"},
                          {"name": "follows", "target": "payment", "via": "prev"}]}))
        .unwrap(),
        ty("refund", "amount", Some(("pays", "pay", "payment"))),
    ] {
        cat.declare(&who, d).await.unwrap();
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
    for i in 0..2 {
        put("region", format!("r{i}"), json!({"name": "r"})).await;
    }
    for i in 0..10 {
        put(
            "customer",
            format!("c{i}"),
            json!({"name": "c", "region": format!("r{}", i % 2)}),
        )
        .await;
    }
    for i in 0..40 {
        let status = if i % 2 == 0 { "open" } else { "paid" };
        put(
            "invoice",
            format!("i{i:02}"),
            json!({"status": status, "cust": format!("c{}", i % 10)}),
        )
        .await;
    }
    for i in 0..80 {
        let mut payment = json!({"amount": "1", "inv": format!("i{:02}", i % 40)});
        if i > 0 {
            payment["prev"] = json!(format!("p{:02}", i - 1));
        }
        put("payment", format!("p{i:02}"), payment).await;
    }
    for i in 0..10 {
        put(
            "refund",
            format!("r{i}"),
            json!({"amount": "1", "pay": format!("p{i:02}")}),
        )
        .await;
    }
    let walk = |project: &str| -> Pattern {
        serde_json::from_value(json!({"start": {"type": "region"}, "steps": [
            {"relation": "in_region", "walk": "in"},
            {"relation": "of", "walk": "in", "where": "status = 'open'"},
            {"relation": "pays", "walk": "in"}], "project": {"as": project}, "limit": 1000}))
        .unwrap()
    };
    // A chain's graph holds the five customers whose invoices are open.
    for (project, want) in [
        ("graph", 2 + 5 + 20 + 40),
        ("reach", 10 + 20 + 40),
        ("rows", 40),
    ] {
        handed.ones.store(0, Ordering::SeqCst);
        handed.edges.store(0, Ordering::SeqCst);
        let answered = match cat.query(&who, walk(project)).await.unwrap().data {
            QueryResult::Rows { rows, .. } => rows.len(),
            QueryResult::Graph { nodes, .. } => nodes.len(),
            QueryResult::Groups { rows } => rows.len(),
        };
        let ones = handed.ones.load(Ordering::SeqCst);
        let edges = handed.edges.load(Ordering::SeqCst);
        assert!(
            answered == want && ones == 0 && edges == 3,
            "a {project} over a step with a `where` answered {answered} where {want} are due, and asked {ones} records of the engine one at a time and {edges} edge questions; it reads what its visibility read, and walks a step's records together"
        );
    }
    // Each page inside the starts reads them from where the last ended
    // (lakelands#1050), the tenth as the first, and with a step's `where`
    // reads no record alone.
    for step in [
        json!({"relation": "pays", "walk": "out", "depth": {"min": 0, "max": 1}}),
        json!({"relation": "pays", "type": "invoice", "walk": "out", "depth": {"min": 0, "max": 1}, "where": "status = 'open'"}),
        json!({"relation": "follows", "walk": "out", "depth": {"min": 0, "max": 1}, "where": "amount = '1'"}),
    ] {
        let mut hood: Pattern = serde_json::from_value(json!({"start": {"type": "payment"},
            "steps": [step], "project": {"as": "graph"}, "limit": 5}))
        .unwrap();
        // A page inside the starts asks no more than the first, but the
        // one read of earlier starts that a step onto their type makes.
        let (mut first, onto) = (0, usize::from(step["relation"] == "follows"));
        for page in 1..=10 {
            handed.rows.store(0, Ordering::SeqCst);
            handed.ones.store(0, Ordering::SeqCst);
            handed.edges.store(0, Ordering::SeqCst);
            handed.gets.store(0, Ordering::SeqCst);
            let QueryResult::Graph {
                nodes, next_cursor, ..
            } = cat.query(&who, hood.clone()).await.unwrap().data
            else {
                panic!("a graph");
            };
            let rows = handed.rows.load(Ordering::SeqCst);
            let ones = handed.ones.load(Ordering::SeqCst);
            let edges = handed.edges.load(Ordering::SeqCst);
            let gets = handed.gets.load(Ordering::SeqCst);
            first = if page == 1 { gets } else { first };
            assert!(
                nodes.len() == 5 && rows <= 6 && ones == 0 && edges <= 2 && gets <= first + onto,
                "a neighbourhood of 80 payments over {step} answered {} on page {page} of 5, read {rows} records by scan, asked {ones} alone, {gets} by key ({first} on page 1) and {edges} edge questions; it reads its page of starts and one more, no record alone, and walks back to its starts' type alone",
                nodes.len()
            );
            hood.cursor = next_cursor;
        }
    }
}

/// A listing of proposals judges the entries of its page and no more
/// (lakelands#888): each decided entry costs a read of the past, so a
/// listing of 50 over 80 decided calls reads the past at most 50 times.
#[tokio::test]
async fn a_listing_of_proposals_judges_its_page_and_no_more() {
    let (store, cat, who, handed) = notes("listing").await;
    let declare = |v: serde_json::Value| serde_json::from_value::<Declaration>(v).unwrap();
    let mark = json!({"declare": "verb", "name": "mark", "on": "note", "risk": "high",
                      "effect": "write", "set": [{"field": "text", "value": "'x'"}]});
    cat.declare(&who, declare(mark)).await.unwrap();
    // A clerk's own call, older than the 80 it is not shown.
    let clerk = Principal {
        kind: PrincipalKind::Agent,
        id: "clerk".into(),
        grants: Grants {
            default: lakelands_api::Access::Write,
            ..Grants::default()
        },
        ..who.clone()
    };
    let (verb, key) = ("mark".into(), "n0000".into());
    let called = cat.call(
        &clerk,
        &verb,
        Some(&key),
        Default::default(),
        Default::default(),
    );
    called.await.unwrap();
    for _ in 0..80 {
        let (verb, key) = ("mark".into(), "n0000".into());
        let called = cat.call(
            &who,
            &verb,
            Some(&key),
            Default::default(),
            Default::default(),
        );
        let lakelands_api::Outcome::Proposed { proposal, .. } = called.await.unwrap() else {
            panic!("a proposal");
        };
        cat.accept(&who, &proposal).await.unwrap();
    }
    let later = json!({"declare": "type", "name": "later", "fields": [{"name": "t", "type": {"kind": "text"}}]});
    cat.declare(&who, declare(later)).await.unwrap();
    let reviewer = Principal {
        kind: PrincipalKind::Agent,
        id: "reviewer".into(),
        grants: Grants {
            default: lakelands_api::Access::Read,
            review: true,
            ..Grants::default()
        },
        ..who.clone()
    };
    handed.pasts.store(0, Ordering::SeqCst);
    handed.proposed.store(0, Ordering::SeqCst);
    let listed = cat
        .proposals(&reviewer, Default::default())
        .await
        .unwrap()
        .data;
    let pasts = handed.pasts.load(Ordering::SeqCst);
    let proposed = handed.proposed.load(Ordering::SeqCst);
    assert!(
        listed.len() == 50 && pasts <= 50,
        "a listing of {} over 80 decided calls read the past {pasts} times; it judges its page of 50 and no more",
        listed.len()
    );
    // And the engine answered it a page, not every proposal (lakelands#1063).
    assert_eq!(
        proposed, 50,
        "the listing's page of 50, none hidden, is one page of the engine's"
    );
    // Its own entry behind the 80 newest it is not shown: the pages grow,
    // seven of them (1 + 2 + … + 64), the rows the hidden ones and its own.
    let listing = |who: Principal, filter: ProposalFilter| {
        let cat = cat.clone();
        for c in [&handed.pages, &handed.wholes, &handed.proposed] {
            c.store(0, Ordering::SeqCst);
        }
        async move { cat.proposals(&who, filter).await.unwrap().data }
    };
    let one = ProposalFilter {
        limit: Some(1),
        ..Default::default()
    };
    let mine = listing(clerk.clone(), one.clone()).await;
    let asked = [&handed.pages, &handed.wholes, &handed.proposed].map(|c| c.load(Ordering::SeqCst));
    assert!(
        mine.len() == 1 && asked[0] <= 8 && asked[1] == 0 && asked[2] <= 81,
        "a listing of one behind 80 hidden asks pages that grow: {asked:?} pages, wholes, rows"
    );
    // A past listing by a reader without `restricted` is paged as well.
    let head = store.head().await.unwrap();
    let past = ProposalFilter {
        at: lakelands_api::read::ReadAt {
            as_of: Some(head),
            valid_at: None,
            clock: None,
        },
        ..Default::default()
    };
    let listed = listing(reviewer, past).await;
    let asked = [&handed.pages, &handed.wholes, &handed.proposed].map(|c| c.load(Ordering::SeqCst));
    assert!(
        listed.len() == 50 && asked[1] == 0 && asked[2] == 50,
        "a past listing by a reader without `restricted` reads pages: {asked:?}"
    );
    drop(store);
}

/// A listing reads the declarations' past once, not once an entry
/// (lakelands#1064): a reviewer kept from a field, listing decided
/// migrations each of which its own accept declared again, judges each on
/// the type as it was proposed against, and reads no view's declarations
/// at a past position for it, but ranges of what was replaced, at most
/// one an entry.
#[tokio::test(flavor = "multi_thread")]
async fn a_listing_reads_the_declarations_past_once() {
    let (store, cat, who, handed) = notes("past").await;
    let doc = |n: usize| {
        let mut fields = vec![
            json!({"name": "title", "type": {"kind": "text"}}),
            json!({"name": "secret", "type": {"kind": "text"}}),
        ];
        fields.extend((0..n).map(|i| json!({"name": format!("f{i}"), "type": {"kind": "text"}})));
        json!({"name": "doc", "fields": fields})
    };
    let mut first = doc(0);
    first["declare"] = json!("type");
    cat.declare(&who, serde_json::from_value(first).unwrap())
        .await
        .unwrap();
    let mut mid = None;
    for i in 0..40 {
        if i == 20 {
            mid = Some(store.head().await.unwrap());
        }
        let write = json!({"write": "migrate", "migration": {"to": doc(i + 1)}});
        let proposed = cat.propose(&who, serde_json::from_value(write).unwrap(), "m".into());
        let lakelands_api::Outcome::Proposed { proposal, .. } = proposed.await.unwrap() else {
            panic!("a proposal");
        };
        cat.accept(&who, &proposal).await.unwrap();
    }
    let secret = lakelands_api::principal::TypeGrants {
        fields: [("secret".into(), lakelands_api::Access::None)].into(),
        ..Default::default()
    };
    let kept = Principal {
        kind: PrincipalKind::Agent,
        id: "kept".into(),
        grants: Grants {
            default: lakelands_api::Access::Read,
            review: true,
            types: [("doc".into(), secret)].into(),
            ..Grants::default()
        },
        ..who.clone()
    };
    handed.declared_then.store(0, Ordering::SeqCst);
    handed.replaced.store(0, Ordering::SeqCst);
    let listed = cat.proposals(&kept, Default::default()).await.unwrap().data;
    let (then, ranges) = (
        handed.declared_then.load(Ordering::SeqCst),
        handed.replaced.load(Ordering::SeqCst),
    );
    assert!(
        listed.len() == 40 && then == 0 && ranges <= 40,
        "a listing of {} decided migrations read the declarations whole at a past position {then} times and {ranges} ranges of the replaced; it reads none whole and a range at most once an entry",
        listed.len()
    );
    // A listing as of a past position, which reads each record held as at
    // the head through a view of its own, asks the same: the declarations
    // whole only at its own position (its shape, and its past), and
    // ranges, one for the holds' history and at most one an entry.
    handed.declared_then.store(0, Ordering::SeqCst);
    handed.replaced.store(0, Ordering::SeqCst);
    let mut then_filter = lakelands_api::write::ProposalFilter::default();
    then_filter.at.as_of = mid;
    let listed = cat.proposals(&kept, then_filter).await.unwrap().data;
    let (then, ranges) = (
        handed.declared_then.load(Ordering::SeqCst),
        handed.replaced.load(Ordering::SeqCst),
    );
    assert!(
        listed.len() == 20 && then <= 2 && ranges <= 21,
        "a listing as of {mid:?} of {} decided migrations read the declarations whole at a past position {then} times and {ranges} ranges; it reads them whole only at its own position, and a range at most once an entry and once for the holds",
        listed.len()
    );
    drop(store);
}

/// A plan reads the records its references name a page of keys at a time,
/// never one by one (lakelands#1141): 400 records name 100 targets, and the
/// plan of the function that computes them, declared again, asks no record
/// alone but its first keys.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plan_reads_the_targets_its_references_name_a_page_at_a_time() {
    let (store, cat, who, handed) = notes("plan-pages").await;
    drop(store);
    let f = |expr: &str| {
        json!({"declare": "function", "name": "f", "backing": "expr", "expr": expr,
            "inputs": [{"name": "s", "type": {"kind": "text"}}]})
    };
    for d in [
        json!({"declare": "type", "name": "p", "fields": [{"name": "n", "type": {"kind": "text"}}]}),
        f("s"),
        json!({"declare": "type", "name": "t", "fields": [{"name": "s", "type": {"kind": "text"}},
            {"name": "r", "type": {"kind": "reference", "target": "p"}, "authority": "derived",
             "function": "f", "depends_on": ["s"]}]}),
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
    for i in 0..100 {
        put("p", format!("a{i}"), json!({"n": "x"})).await;
    }
    for i in 0..400 {
        put(
            "t",
            format!("k{i:03}"),
            json!({"s": format!("a{}", i % 100)}),
        )
        .await;
    }
    handed.gets.store(0, Ordering::SeqCst);
    handed.ones.store(0, Ordering::SeqCst);
    let declarations = vec![serde_json::from_value(f("if(s = 'a0', 'none', s)")).unwrap()];
    let change = lakelands_api::write::Change::Declare { declarations };
    let plan = cat.plan(&who, change).await.unwrap().data;
    let unlive = &plan.types[0].unlive[0];
    let (gets, ones) = (
        handed.gets.load(Ordering::SeqCst),
        handed.ones.load(Ordering::SeqCst),
    );
    assert_eq!(unlive.records, 4, "{plan:?}");
    assert!(
        ones <= unlive.sample_keys.len() && gets <= ones + 2,
        "the plan asked {gets} reads by key, {ones} of them for one record alone"
    );
}

/// A declaration that computes a type again of which nothing is judged (no
/// constraint, no reference) plans with no walk and no count: every record
/// is computed again, uncounted (lakelands#1141).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plan_with_nothing_judged_reads_no_record() {
    let (store, cat, who, handed) = notes("plan-uncounted").await;
    drop(store);
    let g = |expr: &str| {
        json!({"declare": "function", "name": "g", "backing": "expr", "expr": expr,
            "inputs": [{"name": "s", "type": {"kind": "text"}}]})
    };
    for d in [
        g("s"),
        json!({"declare": "type", "name": "tx", "fields": [{"name": "s", "type": {"kind": "text"}},
            {"name": "d", "type": {"kind": "text"}, "authority": "derived", "function": "g",
             "depends_on": ["s"]}]}),
    ] {
        cat.declare(&who, serde_json::from_value(d).unwrap())
            .await
            .unwrap();
    }
    for i in 0..50 {
        let patch = serde_json::from_value(json!({"s": format!("a{i}")})).unwrap();
        let key = format!("x{i}").as_str().into();
        cat.upsert(&who, &"tx".into(), &key, patch, Default::default())
            .await
            .unwrap();
    }
    for n in [&handed.rows, &handed.scans, &handed.counts, &handed.gets] {
        n.store(0, Ordering::SeqCst);
    }
    let declarations = vec![serde_json::from_value(g("if(s = 'a0', 'b', s)")).unwrap()];
    let change = lakelands_api::write::Change::Declare { declarations };
    let plan = cat.plan(&who, change).await.unwrap().data;
    let read = [&handed.rows, &handed.scans, &handed.counts].map(|n| n.load(Ordering::SeqCst));
    assert_eq!(plan.types[0].recomputed, None, "{plan:?}");
    assert_eq!(
        read,
        [0, 0, 0],
        "the plan read rows, scans and counts of `tx`"
    );
}

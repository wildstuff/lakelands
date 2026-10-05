//! The memory engine behind a wrapper that remembers what was asked of
//! it and can fail a call: the most rows one `scan` handed out, a scan of a
//! type to fail, the scans, the puts of records and the commits in the
//! order they came, a commit or a `begin` to refuse, and a look at the
//! lake before each commit, while its transaction is open.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use lakelands_api::engine::{
    CanonicalEvent, Declared, EngineOpener, LakeEngine, Lease, OpenMode, ProjectionStore,
    ProjectionView, Reclaimed, Scan, VectorIndex,
};
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::query::{Edge, Walk};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Declaration, Proposal, ProposalFilter};
use lakelands_api::{Error, LakeId, Location, Position, RecordKey, RelationName, Result, TypeName};
use lakelands_engine_mem::MemoryOpener;

/// One thing asked of the engine.
#[derive(Clone, Debug, PartialEq)]
pub enum Asked {
    /// A scan of a type, by a view.
    Scan(TypeName),
    /// A record put, at its position.
    Put(TypeName, RecordKey, Position),
    /// A commit through a position, whether the engine took it or not.
    Commit(Position),
}

/// A look at the lake before a commit through a position.
pub type Look = Box<dyn FnMut(Position) + Send>;

/// What was asked of the engine, and what it is to fail.
#[derive(Default)]
pub struct Probe {
    /// The most rows one `scan` handed out.
    pub most: AtomicUsize,
    /// A scan to fail: the n-th of a type from now, counted from 1.
    pub fail: Mutex<Option<(TypeName, usize)>>,
    /// The scans, the puts of records and the commits, in order.
    pub asked: Mutex<Vec<Asked>>,
    /// A commit to refuse: the n-th from now, counted from 1.
    pub refuse_commit: Mutex<Option<usize>>,
    /// A `begin` to refuse: the n-th from now, counted from 1.
    pub refuse_begin: Mutex<Option<usize>>,
    /// Called before each commit reaches the engine, with the position it
    /// commits through: its transaction is still open.
    pub before_commit: Mutex<Option<Look>>,
}

impl Probe {
    /// What was asked since the last call, in order.
    pub fn take(&self) -> Vec<Asked> {
        std::mem::take(&mut self.asked.lock().unwrap())
    }

    fn asked(&self, what: Asked) {
        self.asked.lock().unwrap().push(what);
    }

    /// Whether this call is the one `planted` names: it counts down.
    fn refused(planted: &Mutex<Option<usize>>) -> bool {
        let mut left = planted.lock().unwrap();
        let now = left.as_mut().is_some_and(|n| {
            *n -= 1;
            *n == 0
        });
        if now {
            *left = None;
        }
        now
    }
}

pub struct Probed {
    inner: Box<dyn LakeEngine>,
    probe: Arc<Probe>,
}

struct ProbedView<'a> {
    inner: Box<dyn ProjectionView + 'a>,
    probe: Arc<Probe>,
}

impl ProjectionView for ProbedView<'_> {
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
        let mut fail = self.probe.fail.lock().unwrap();
        if let Some((failing, left)) = fail.as_mut().filter(|(failing, _)| failing == ty) {
            *left -= 1;
            if *left == 0 {
                let message = format!("the scan of `{failing}` failed (planted)");
                *fail = None;
                return Err(Error::Unavailable { message });
            }
        }
        drop(fail);
        self.probe.asked(Asked::Scan(ty.clone()));
        let rows = self.inner.scan(ty, scan)?;
        self.probe.most.fetch_max(rows.len(), Ordering::SeqCst);
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

impl LakeEngine for Probed {
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
    fn seal_segment(&self, lease: &Lease) -> Result<Option<SegmentDigest>> {
        self.inner.seal_segment(lease)
    }
    fn segments(&self) -> Result<Vec<SegmentDigest>> {
        self.inner.segments()
    }
    fn projection(&self) -> &dyn ProjectionStore {
        self
    }
    fn view(&self, as_of: Option<Position>) -> Result<Box<dyn ProjectionView + '_>> {
        Ok(Box::new(ProbedView {
            inner: self.inner.view(as_of)?,
            probe: self.probe.clone(),
        }))
    }
    fn vectors(&self) -> Option<&dyn VectorIndex> {
        self.inner.vectors()
    }
}

/// The memory engine's opener, every lake it opens probed.
pub struct ProbedOpener(pub MemoryOpener, pub Arc<Probe>);

impl ProjectionStore for Probed {
    fn begin(&self) -> Result<()> {
        if Probe::refused(&self.probe.refuse_begin) {
            let message = "the `begin` was refused (planted)".to_owned();
            return Err(Error::Unavailable { message });
        }
        self.inner.projection().begin()
    }
    fn commit(&self, through: Position) -> Result<()> {
        self.probe.asked(Asked::Commit(through));
        if let Some(look) = self.probe.before_commit.lock().unwrap().as_mut() {
            look(through);
        }
        if Probe::refused(&self.probe.refuse_commit) {
            let message = format!("the commit through {through} was refused (planted)");
            return Err(Error::Unavailable { message });
        }
        self.inner.projection().commit(through)
    }
    fn rollback(&self) {
        self.inner.projection().rollback();
    }
    fn folded_through(&self) -> Result<Option<Position>> {
        self.inner.projection().folded_through()
    }
    fn put_declaration(
        &self,
        declaration: &Declaration,
        version: u32,
        schema_hash: Option<&str>,
        at: Position,
    ) -> Result<()> {
        let p = self.inner.projection();
        p.put_declaration(declaration, version, schema_hash, at)
    }
    fn put_record(&self, record: &StoredRecord) -> Result<()> {
        let (ty, key) = (record.ty.clone(), record.key.clone());
        self.probe.asked(Asked::Put(ty, key, record.position));
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
        let p = self.inner.projection();
        p.put_membership(set, ty, key, member, at)
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
    fn compact(&self) -> Result<Reclaimed> {
        self.inner.projection().compact()
    }
}

impl EngineOpener for ProbedOpener {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn open(&self, location: &Location, mode: OpenMode) -> Result<Box<dyn LakeEngine>> {
        Ok(Box::new(Probed {
            inner: self.0.open(location, mode)?,
            probe: self.1.clone(),
        }))
    }
}

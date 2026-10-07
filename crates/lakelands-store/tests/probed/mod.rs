//! The memory engine behind a wrapper that remembers what was asked of
//! it and can fail a call: the most rows one `scan` handed out, a scan of a
//! type to fail, the scans, the puts of records and the commits in the
//! order they came, a commit, a `begin` or a view to refuse, a look at
//! the lake before each commit, while its transaction is open, and a seal
//! that hands another digest than the engine took or asks twice; and it
//! counts the rows its scans hand out by type and the reads of a kept
//! view's state, and keeps the last record put under each kept name and
//! key, and the entries of the kept views' orders since the last clear.
//! [`calls`] reads what a lake owes as the drain would.

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
    /// An append to refuse: the n-th from now, counted from 1, as invalid
    /// where `refuse_as_invalid`.
    pub refuse_append: Mutex<Option<usize>>,
    /// Whether a refused append lands first: the engine holds its events
    /// and answers a failure.
    pub lands: std::sync::atomic::AtomicBool,
    /// A `begin` to refuse: the n-th from now, counted from 1.
    pub refuse_begin: Mutex<Option<usize>>,
    /// Whether a refused `begin` is answered as invalid, not unavailable.
    pub refuse_as_invalid: std::sync::atomic::AtomicBool,
    /// How many times `begin` was asked.
    pub begins: AtomicUsize,
    /// A view to refuse: the n-th asked for from now, counted from 1.
    pub refuse_view: Mutex<Option<usize>>,
    /// Whether a scan of a view's kept records is handed out reversed.
    pub reversed: std::sync::atomic::AtomicBool,
    /// The rows the scans of each type handed out, in all.
    pub rows: Mutex<std::collections::BTreeMap<TypeName, usize>>,
    /// How many times a kept view's state was read (`sys:kept`).
    pub states: AtomicUsize,
    /// The last record put under each kept name and key.
    pub kept: Mutex<std::collections::BTreeMap<(TypeName, RecordKey), StoredRecord>>,
    /// The entries of the kept views' orders put since the last clear, by
    /// name and key, an ended one left out.
    pub orders: Mutex<std::collections::BTreeMap<(TypeName, RecordKey), StoredRecord>>,
    /// Called before each commit reaches the engine, with the position it
    /// commits through: its transaction is still open.
    pub before_commit: Mutex<Option<Look>>,
    /// Whether a seal hands its `first` another digest than the engine's.
    pub another_digest: std::sync::atomic::AtomicBool,
    /// How many events the log's reads handed out.
    pub events_read: AtomicUsize,
    /// How often a view was asked for the declarations in force.
    pub declarations: AtomicUsize,
    /// How often a view was asked for the lake's list of renames.
    pub renames: AtomicUsize,
    /// A reader to park inside the next `declarations`: it says so on the
    /// first and waits on the second before it reads.
    pub park: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
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
        self.probe.declarations.fetch_add(1, Ordering::SeqCst);
        let park = self.probe.park.lock().unwrap().take();
        if let Some((parked, go)) = park {
            parked.send(()).unwrap();
            go.recv().unwrap();
        }
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
        let mut rows = self.inner.scan(ty, scan)?;
        self.probe.most.fetch_max(rows.len(), Ordering::SeqCst);
        *self
            .probe
            .rows
            .lock()
            .unwrap()
            .entry(ty.clone())
            .or_default() += rows.len();
        if self.probe.reversed.load(Ordering::SeqCst) && lakelands_api::seam::is_kept_name(ty) {
            rows.reverse();
        }
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
        if seam == "sys:kept" {
            self.probe.states.fetch_add(1, Ordering::SeqCst);
        }
        if seam == "sys:renames" {
            self.probe.renames.fetch_add(1, Ordering::SeqCst);
        }
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
        if Probe::refused(&self.probe.refuse_append) {
            if self.probe.lands.load(Ordering::SeqCst) {
                self.inner.append(lease, events)?;
            }
            let message = "the append was refused (planted)".to_owned();
            if self.probe.refuse_as_invalid.load(Ordering::SeqCst) {
                let reason = lakelands_api::Refusal::Invalid { message };
                return Err(Error::Refused {
                    field: None,
                    reason,
                });
            }
            return Err(Error::Unavailable { message });
        }
        self.inner.append(lease, events)
    }
    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.inner.import(lease, events)
    }
    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        let read = self.inner.read(from, limit)?;
        self.probe
            .events_read
            .fetch_add(read.len(), Ordering::SeqCst);
        Ok(read)
    }
    fn seal_segment(
        &self,
        lease: &Lease,
        imported: bool,
        first: &mut dyn FnMut(&SegmentDigest) -> Result<Vec<CanonicalEvent>>,
    ) -> Result<Option<SegmentDigest>> {
        let other = self.probe.another_digest.load(Ordering::SeqCst);
        self.inner.seal_segment(lease, imported, &mut |sealed| {
            let mut sealed = sealed.clone();
            if other {
                sealed.digest = Digest("0".repeat(64));
            }
            first(&sealed)
        })
    }
    fn segments(&self) -> Result<Vec<SegmentDigest>> {
        self.inner.segments()
    }
    fn projection(&self) -> &dyn ProjectionStore {
        self
    }
    fn view(&self, as_of: Option<Position>) -> Result<Box<dyn ProjectionView + '_>> {
        if Probe::refused(&self.probe.refuse_view) {
            let message = "the view was refused (planted)".to_owned();
            return Err(Error::Unavailable { message });
        }
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
        self.probe.begins.fetch_add(1, Ordering::SeqCst);
        if Probe::refused(&self.probe.refuse_begin) {
            let message = "the `begin` was refused (planted)".to_owned();
            if self.probe.refuse_as_invalid.load(Ordering::SeqCst) {
                let reason = lakelands_api::Refusal::Invalid { message };
                return Err(Error::Refused {
                    field: None,
                    reason,
                });
            }
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
        self.probe
            .asked(Asked::Put(ty.clone(), key.clone(), record.position));
        if lakelands_api::seam::is_kept_name(&ty) {
            self.probe
                .kept
                .lock()
                .unwrap()
                .insert((ty, key), record.clone());
        } else if lakelands_api::seam::is_head_only(&ty) {
            let mut orders = self.probe.orders.lock().unwrap();
            match record.is_ended() {
                true => drop(orders.remove(&(ty, key))),
                false => drop(orders.insert((ty, key), record.clone())),
            }
        }
        self.inner.projection().put_record(record)
    }
    fn end_head_only(&self, name: &TypeName, at: Position) -> Result<()> {
        let mut orders = self.probe.orders.lock().unwrap();
        orders.retain(|(ty, _), _| ty != name);
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
        self.probe.orders.lock().unwrap().clear();
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

/// The calls the lake owes now, entry by entry, a sweep's over every
/// page; the entries taken.
pub fn calls(lake: &lakelands_store::lake::Lake) -> Vec<lakelands_store::backed::Due> {
    use lakelands_store::backed::{Of, page};
    let owed = std::mem::take(&mut lake.writer().owed);
    let mut calls = Vec::new();
    for mut o in owed {
        loop {
            let (rows, end) = page(lake, &o).expect("a page");
            if let (Of::Sweep { after, .. }, Some((key, _))) = (&mut o.of, rows.last()) {
                *after = Some(key.clone());
            }
            calls.extend(rows.into_iter().flat_map(|(_, c)| c).map(|(d, _)| d));
            if end {
                break;
            }
        }
    }
    calls
}

//! A handle onto one lake held in memory: the log and lease, the
//! projection's write side, and the vector index.
//!
//! Handles to one lake share its state; each handle keeps only its open
//! mode, its own open transaction and the lease it took, which it gives
//! up when it is dropped. A handle opened read-only refuses the lease,
//! every append and every write to the projection.

use std::sync::{Arc, Mutex};

use lakelands_api::engine::{
    CanonicalEvent, Declared, LakeEngine, Lease, OpenMode, ProjectionStore, ProjectionView,
    Reclaimed, VectorIndex,
};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::id::{LakeId, Position, RecordKey, TypeName};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Declaration, Proposal};

use crate::projection::Put;
use crate::view::MemView;
use crate::{Lake, NAME, invalid, lock, unavailable};

/// One handle onto a lake held in memory.
pub struct MemoryLake {
    lake: LakeId,
    mode: OpenMode,
    shared: Arc<Mutex<Lake>>,
    /// The puts of the open transaction, if one is open.
    pending: Mutex<Option<Vec<Put>>>,
    /// The lease this handle took and has not given up.
    held: Mutex<Option<Lease>>,
}

impl MemoryLake {
    pub(crate) fn new(lake: LakeId, mode: OpenMode, shared: Arc<Mutex<Lake>>) -> MemoryLake {
        MemoryLake {
            lake,
            mode,
            shared,
            pending: Mutex::new(None),
            held: Mutex::new(None),
        }
    }

    /// Refuses a write to the log through a handle opened read-only.
    fn writable(&self) -> Result<()> {
        match self.mode {
            OpenMode::Read => Err(unavailable(format!(
                "lake `{}` is open read-only here; open it for writing to take the lease or \
                 append",
                self.lake
            ))),
            OpenMode::Write | OpenMode::Create => Ok(()),
        }
    }

    /// Holds a put for the open transaction.
    fn stage(&self, put: Put) -> Result<()> {
        self.writable()?;
        match lock(&self.pending).as_mut() {
            Some(puts) => {
                puts.push(put);
                Ok(())
            }
            None => Err(invalid(
                "the projection is written inside a transaction: call `begin` first",
            )),
        }
    }
}

impl LakeEngine for MemoryLake {
    fn name(&self) -> &str {
        NAME
    }

    fn lake(&self) -> &LakeId {
        &self.lake
    }

    fn take_lease(&self, holder: &str) -> Result<Lease> {
        self.writable()?;
        let lease = lock(&self.shared).log.take_lease(holder)?;
        *lock(&self.held) = Some(lease.clone());
        Ok(lease)
    }

    fn release_lease(&self, lease: &Lease) -> Result<()> {
        lock(&self.shared).log.release_lease(lease);
        let mut held = lock(&self.held);
        if held.as_ref() == Some(lease) {
            *held = None;
        }
        Ok(())
    }

    fn tip(&self) -> Result<Option<(Position, Digest)>> {
        Ok(lock(&self.shared).log.tip())
    }

    fn append(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.writable()?;
        lock(&self.shared).log.append(lease, events)
    }

    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.writable()?;
        lock(&self.shared).log.import(lease, events)
    }

    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        Ok(lock(&self.shared).log.read(from, limit))
    }

    fn seal_segment(&self, lease: &Lease) -> Result<Option<SegmentDigest>> {
        self.writable()?;
        lock(&self.shared).log.seal(lease)
    }

    fn segments(&self) -> Result<Vec<SegmentDigest>> {
        Ok(lock(&self.shared).log.segments())
    }

    fn projection(&self) -> &dyn ProjectionStore {
        self
    }

    fn view(&self, as_of: Option<Position>) -> Result<Box<dyn ProjectionView + '_>> {
        let folded = lock(&self.shared).projection.folded;
        let at = lakelands_core::engines::view_at(as_of, folded)?;
        Ok(Box::new(MemView {
            lake: Arc::clone(&self.shared),
            at,
        }))
    }

    fn vectors(&self) -> Option<&dyn VectorIndex> {
        Some(self)
    }
}

/// A handle that is dropped gives up the lease it holds, as a process that
/// ends does: the lake is closed when its last writer's handle goes. A
/// lease already lost to a later writer stays with that writer.
impl Drop for MemoryLake {
    fn drop(&mut self) {
        if let Some(lease) = lock(&self.held).take() {
            lock(&self.shared).log.release_lease(&lease);
        }
    }
}

impl ProjectionStore for MemoryLake {
    /// Opens a transaction. Beginning while one is open abandons the open
    /// one, as a rollback would.
    fn begin(&self) -> Result<()> {
        self.writable()?;
        *lock(&self.pending) = Some(Vec::new());
        Ok(())
    }

    fn commit(&self, through: Position) -> Result<()> {
        let puts = lock(&self.pending)
            .take()
            .ok_or_else(|| invalid("there is no transaction to commit: call `begin` first"))?;
        lock(&self.shared).projection.commit(puts, through)
    }

    fn rollback(&self) {
        *lock(&self.pending) = None;
    }

    fn folded_through(&self) -> Result<Option<Position>> {
        Ok(lock(&self.shared).projection.folded)
    }

    fn put_declaration(
        &self,
        declaration: &Declaration,
        version: u32,
        schema_hash: Option<&str>,
        at: Position,
    ) -> Result<()> {
        self.stage(Put::Declaration(Declared {
            declaration: declaration.clone(),
            version,
            schema_hash: schema_hash.map(str::to_owned),
            since: at,
        }))
    }

    fn put_record(&self, record: &StoredRecord) -> Result<()> {
        self.stage(Put::Record(record.clone()))
    }

    fn put_proposal(&self, proposal: &Proposal, at: Position) -> Result<()> {
        self.stage(Put::Proposal {
            proposal: proposal.clone(),
            at,
        })
    }

    fn put_membership(
        &self,
        set: &str,
        ty: &TypeName,
        key: &RecordKey,
        member: bool,
        at: Position,
    ) -> Result<()> {
        self.stage(Put::Membership {
            set: set.to_owned(),
            ty: ty.clone(),
            key: key.clone(),
            member,
            at,
        })
    }

    fn put_output(&self, seam: &str, input: &Digest, output: &Value, at: Position) -> Result<()> {
        self.stage(Put::Output {
            seam: seam.to_owned(),
            input: input.clone(),
            output: output.clone(),
            at,
        })
    }

    fn mark_applied(&self, id: &EventId, at: Position) -> Result<()> {
        self.stage(Put::Applied { id: id.clone(), at })
    }

    /// Drops the projection, its kept history and the vector index; the
    /// log and the lease stay. An open transaction is abandoned.
    fn clear(&self) -> Result<()> {
        self.writable()?;
        *lock(&self.pending) = None;
        let mut lake = lock(&self.shared);
        lake.projection = Default::default();
        lake.vectors.clear();
        Ok(())
    }

    /// The memory engine keeps no storage to give back: it measures and
    /// reclaims nothing.
    fn compact(&self) -> Result<Reclaimed> {
        self.writable()?;
        Ok(Reclaimed::nothing())
    }
}

impl VectorIndex for MemoryLake {
    fn put(&self, ty: &TypeName, key: &RecordKey, field: &str, vector: &[f32]) -> Result<()> {
        lock(&self.shared).vectors.put(ty, key, field, vector);
        Ok(())
    }

    fn remove(&self, ty: &TypeName, key: &RecordKey, field: &str) -> Result<()> {
        lock(&self.shared).vectors.remove(ty, key, field);
        Ok(())
    }

    fn nearest(
        &self,
        ty: &TypeName,
        field: &str,
        query: &[f32],
        k: u32,
    ) -> Result<Vec<(RecordKey, f64)>> {
        lock(&self.shared).vectors.nearest(ty, field, query, k)
    }

    fn count(&self, ty: &TypeName, field: &str) -> Result<u64> {
        Ok(lock(&self.shared).vectors.count(ty, field))
    }
}

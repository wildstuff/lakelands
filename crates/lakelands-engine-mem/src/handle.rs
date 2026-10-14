//! A handle onto one lake held in memory: the log and lease, the
//! projection's write side, and the vector index.
//!
//! Handles to one lake share its state; each handle keeps only its open
//! mode, its own open transaction and the lease it took, which it gives
//! up when it is dropped. A handle opened read-only refuses the lease,
//! every append and every write to the projection; one opened for writing
//! appends and seals only under the lease it took itself.

use std::sync::{Arc, Mutex};

use lakelands_api::engine::{
    Applied, CanonicalEvent, Declared, LakeEngine, Lease, OpenMode, ProjectionStore,
    ProjectionView, Ranked, Rankings, Reclaimed, VectorIndex,
};
use lakelands_api::error::{Act, Case, Error, Result, Situation};
use lakelands_api::event::{Digest, SegmentDigest};
use lakelands_api::id::{LakeId, Position, RecordKey, TypeName};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Declaration, Proposal};

use crate::projection::Put;
use crate::vectors::Indexed;
use crate::view::MemView;
use crate::{Lake, NAME, REBUILD_RATE, invalid, lock};

/// One handle onto a lake held in memory.
pub struct MemoryLake {
    lake: LakeId,
    mode: OpenMode,
    shared: Arc<Mutex<Lake>>,
    /// The puts of the open transaction, if one is open.
    pending: Mutex<Option<Vec<Put>>>,
    /// The vectors the open transaction indexes and takes out (`None`).
    indexing: Mutex<Vec<Indexed>>,
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
            indexing: Mutex::default(),
            held: Mutex::new(None),
        }
    }

    /// Refuses a write to the log through a handle opened read-only.
    fn writable(&self) -> Result<()> {
        match self.mode {
            OpenMode::Read => Err(Error::unavailable(Situation::ReadOnly {
                lake: Some(self.lake.to_string()),
                leased_elsewhere: false,
            })),
            OpenMode::Write | OpenMode::Create | OpenMode::Rebuild => Ok(()),
        }
    }

    /// Refuses an append, an import or a seal through a handle that does
    /// not hold `lease`: a copy of another handle's lease writes nothing.
    fn holding(&self, lease: &Lease, undone: Act) -> Result<()> {
        self.writable()?;
        if lock(&self.held).as_ref() == Some(lease) {
            return Ok(());
        }
        Err(Error::unavailable(Situation::LeaseNotHeld {
            fence: Some(lease.fence),
            undone: Some(undone),
        }))
    }

    /// Indexes a vector or takes it out when the open transaction commits,
    /// as its puts land, so that no ranking reads a vector of a state the
    /// projection does not hold yet; at once where none is open.
    fn index(&self, vector: Indexed) {
        match lock(&self.pending).is_some() {
            true => lock(&self.indexing).push(vector),
            false => lock(&self.shared).vectors.set(vector),
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
            None => Err(lakelands_core::engines::unstaged()),
        }
    }
}

impl LakeEngine for MemoryLake {
    fn name(&self) -> &str {
        NAME
    }

    fn rebuild_rate(&self) -> u64 {
        REBUILD_RATE
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
        self.holding(lease, Act::Appended)?;
        lock(&self.shared).log.append(lease, events)
    }

    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.holding(lease, Act::Imported)?;
        lock(&self.shared).log.import(lease, events)
    }

    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        Ok(lock(&self.shared).log.read(from, limit))
    }

    fn seal_segment(
        &self,
        lease: &Lease,
        imported: bool,
        first: &mut dyn FnMut(&SegmentDigest) -> Result<Vec<CanonicalEvent>>,
    ) -> Result<Option<SegmentDigest>> {
        self.holding(lease, Act::Sealed)?;
        lock(&self.shared).log.seal(lease, imported, first)
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

    /// One lock holds the projection and the vectors: the lists and the
    /// position are of one state.
    fn ranked(&self, ty: &TypeName, asked: &Rankings, k: u32) -> Result<Ranked> {
        let lake = lock(&self.shared);
        let p = &lake.projection;
        let mut lists = Vec::new();
        if let Some((text, fields)) = &asked.text {
            let head = p.records_at(ty, p.folded);
            lists.push(p.type_at(ty, p.folded).map_or_else(Vec::new, |decl| {
                lakelands_core::engines::lexical(decl, head, text, fields.as_deref(), k)
            }));
        }
        for (field, query) in &asked.vectors {
            lists.push(lake.vectors.nearest(ty, field, query, k)?);
        }
        let position = p.folded;
        Ok(Ranked { position, lists })
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
        lock(&self.indexing).clear();
        Ok(())
    }

    fn commit(&self, through: Position) -> Result<()> {
        let puts = lock(&self.pending)
            .take()
            .ok_or_else(|| invalid(Case::CommitUnbegun))?;
        let vectors = std::mem::take(&mut *lock(&self.indexing));
        let mut lake = lock(&self.shared);
        lake.projection.commit(puts, through)?;
        vectors.into_iter().for_each(|v| lake.vectors.set(v));
        Ok(())
    }

    fn rollback(&self) {
        *lock(&self.pending) = None;
        lock(&self.indexing).clear();
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
        match lakelands_api::seam::is_head_only(&record.ty) {
            true => self.stage(Put::Head(record.clone())),
            false => self.stage(Put::Record(record.clone())),
        }
    }

    fn end_head_only(&self, name: &TypeName, at: Position) -> Result<()> {
        lakelands_core::engines::head_only(name)?;
        let ty = name.clone();
        self.stage(Put::HeadsEnded { ty, at })
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

    fn mark_applied(&self, applied: &Applied) -> Result<()> {
        self.stage(Put::Applied(applied.clone()))
    }

    /// Drops the projection, its kept history and the vector index; the
    /// log and the lease stay. An open transaction is abandoned.
    fn clear(&self) -> Result<()> {
        self.writable()?;
        self.rollback();
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
        self.index((
            ty.clone(),
            key.clone(),
            field.to_owned(),
            Some(vector.to_vec()),
        ));
        Ok(())
    }

    fn remove(&self, ty: &TypeName, key: &RecordKey, field: &str) -> Result<()> {
        self.index((ty.clone(), key.clone(), field.to_owned(), None));
        Ok(())
    }

    fn count(&self, ty: &TypeName, field: &str) -> Result<u64> {
        Ok(lock(&self.shared).vectors.count(ty, field))
    }
}

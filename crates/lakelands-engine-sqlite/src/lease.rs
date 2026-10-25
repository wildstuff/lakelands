//! The writer lease: a file holding the fencing token, under an operating
//! system lock.
//!
//! `lease` in the lake's directory holds the last fencing token handed
//! out and who holds it now, as one JSON object. Taking the lease takes an
//! exclusive lock on that file without waiting: when another handle holds
//! it — in this process or any other — the take fails at once, naming the
//! holder the file records. The lock is the operating system's, so it is
//! released when its process ends, however it ends, and a crashed writer
//! never keeps the lake from the next one. The taker writes a token one
//! greater than the file's and syncs it before it may append.
//!
//! The lock keeps two writers from holding the lease at once; the token is
//! what refuses a stale writer. Every append and every seal is checked
//! against the lease this handle holds, and a writer that released or lost
//! its lease holds none, so its token is refused whatever the file says
//! now (`docs/decisions/008-one-writer-per-lake.md`). The check and the
//! append or seal it stands before are one step: the handle keeps the
//! lease it holds locked from the check to the end of the write, so it
//! cannot give the lease up between them, and the file's lock, held from
//! the take to the release, keeps every other handle from taking it.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs4::{FileExt, TryLockError};
use lakelands_api::engine::Lease;
use lakelands_api::error::Result;
use serde::{Deserialize, Serialize};

use lakelands_api::error::{Detail, Doing, Flaw, Part};

use crate::{damaged, io_error};

/// What the lease file records.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Recorded {
    fence: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    holder: String,
    /// The last position an import appended, while every event in the
    /// log came through one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    imported: Option<u64>,
    /// The greatest fence an import carried over; the next lease is
    /// greater. Kept apart from `fence`, which the lease held while
    /// importing must go on matching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    import_fence: Option<u64>,
}

/// The lease a handle holds, with the locked file that makes it so.
pub(crate) struct Held {
    pub(crate) lease: Lease,
    file: File,
}

/// A lease dropped without [`LeaseFile::release`] still unlocks before its
/// file closes: the lock belongs to the open file, and a child forked
/// while it was held keeps a copy until it execs.
impl Drop for Held {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// A lake's lease file.
pub(crate) struct LeaseFile {
    path: PathBuf,
}

fn read(file: &mut File) -> Result<Recorded> {
    let mut text = String::new();
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_to_string(&mut text))
        .map_err(|e| io_error(Doing::Read, Part::LeaseFile, e))?;
    if text.trim().is_empty() {
        return Ok(Recorded::default());
    }
    serde_json::from_str(&text).map_err(|e| {
        let flaw = Flaw::NotJson {
            detail: Detail::json(&e),
        };
        damaged(Part::LeaseFile, None, flaw)
    })
}

fn write(file: &mut File, recorded: &Recorded) -> Result<()> {
    let mut text = serde_json::to_string(recorded).expect("a lease serialises");
    text.push('\n');
    file.set_len(0)
        .and_then(|()| file.seek(SeekFrom::Start(0)))
        .and_then(|_| file.write_all(text.as_bytes()))
        .and_then(|()| file.sync_all())
        .map_err(|e| io_error(Doing::Write, Part::LeaseFile, e))
}

impl LeaseFile {
    pub(crate) fn new(dir: &Path) -> LeaseFile {
        LeaseFile {
            path: dir.join("lease"),
        }
    }

    fn open(&self) -> Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.path)
            .map_err(|e| io_error(Doing::Open, Part::LeaseFile, e))
    }

    /// Who the file says holds the lease, and with which token.
    pub(crate) fn current(&self) -> Result<(u64, String)> {
        let r = read(&mut self.open()?)?;
        Ok((r.fence, r.holder))
    }

    /// Whether no handle holds the lease: a shared lock on the file, taken
    /// without waiting and given back at once, is refused while the lease
    /// is held. A take that falls in that moment is refused as held, as
    /// one racing another taker is; a lake whose file is not there yet has
    /// never been leased.
    pub(crate) fn free(&self) -> Result<bool> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(e) => return Err(io_error(Doing::Open, Part::LeaseFile, e)),
        };
        match FileExt::try_lock_shared(&file) {
            Ok(()) => {
                FileExt::unlock(&file).map_err(|e| io_error(Doing::Unlock, Part::LeaseFile, e))?;
                Ok(true)
            }
            Err(TryLockError::WouldBlock) => Ok(false),
            Err(TryLockError::Error(e)) => Err(io_error(Doing::Lock, Part::LeaseFile, e)),
        }
    }

    /// Take the lease for `holder`, or say who holds it.
    pub(crate) fn take(&self, holder: &str) -> Result<Held> {
        let mut file = self.open()?;
        // Called through the trait: the standard library's own `try_lock`
        // is newer than the workspace's minimum Rust version.
        match FileExt::try_lock(&file) {
            Ok(()) => {}
            Err(TryLockError::Error(e)) => return Err(io_error(Doing::Lock, Part::LeaseFile, e)),
            Err(TryLockError::WouldBlock) => {
                let (fence, held_by) = self.current()?;
                return Err(lakelands_core::engines::lease_held(&held_by, fence));
            }
        }
        let mut recorded = read(&mut file)?;
        recorded.fence = recorded.fence.max(recorded.import_fence.unwrap_or(0)) + 1;
        recorded.holder = holder.to_owned();
        write(&mut file, &recorded)?;
        let lease = Lease {
            fence: recorded.fence,
            holder: recorded.holder,
        };
        Ok(Held { lease, file })
    }

    /// The last position an import appended, while the log holds nothing
    /// else.
    pub(crate) fn imported(&self, held: &mut Held) -> Result<Option<u64>> {
        Ok(read(&mut held.file)?.imported)
    }

    /// Record that an import appended through `last`, with fences up to
    /// `fence`: a later lease is greater than all of them.
    pub(crate) fn record_import(&self, held: &mut Held, last: u64, fence: u64) -> Result<()> {
        let mut recorded = read(&mut held.file)?;
        recorded.holder = held.lease.holder.clone();
        recorded.imported = Some(last);
        recorded.import_fence = Some(recorded.import_fence.unwrap_or(0).max(fence));
        write(&mut held.file, &recorded)
    }

    /// Give a held lease up: the file keeps its token and names no holder,
    /// and the lock is released. The lock is released whatever happens; a
    /// failure to clear the holder is reported after it.
    pub(crate) fn release(&self, mut held: Held) -> Result<()> {
        // The file keeps the greatest token and what an import recorded.
        let cleared = read(&mut held.file).and_then(|mut recorded| {
            recorded.fence = recorded.fence.max(held.lease.fence);
            recorded.holder.clear();
            write(&mut held.file, &recorded)
        });
        let unlocked =
            FileExt::unlock(&held.file).map_err(|e| io_error(Doing::Unlock, Part::LeaseFile, e));
        drop(held);
        cleared.and(unlocked)
    }
}

#[cfg(test)]
mod tests;

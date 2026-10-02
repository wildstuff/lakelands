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
//! now (`docs/decisions/008-one-writer-per-lake.md`).

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs4::{FileExt, TryLockError};
use lakelands_api::engine::Lease;
use lakelands_api::error::Result;
use serde::{Deserialize, Serialize};

use crate::{io_error, unavailable};

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

fn read(file: &mut File, path: &Path) -> Result<Recorded> {
    let mut text = String::new();
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_to_string(&mut text))
        .map_err(|e| io_error("reading", path, e))?;
    if text.trim().is_empty() {
        return Ok(Recorded::default());
    }
    serde_json::from_str(&text)
        .map_err(|e| unavailable(format!("the lease file {} is damaged: {e}", path.display())))
}

fn write(file: &mut File, path: &Path, recorded: &Recorded) -> Result<()> {
    let mut text = serde_json::to_string(recorded).expect("a lease serialises");
    text.push('\n');
    file.set_len(0)
        .and_then(|()| file.seek(SeekFrom::Start(0)))
        .and_then(|_| file.write_all(text.as_bytes()))
        .and_then(|()| file.sync_all())
        .map_err(|e| io_error("writing", path, e))
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
            .map_err(|e| io_error("opening", &self.path, e))
    }

    /// Who the file says holds the lease, and with which token.
    pub(crate) fn current(&self) -> Result<(u64, String)> {
        let r = read(&mut self.open()?, &self.path)?;
        Ok((r.fence, r.holder))
    }

    /// Take the lease for `holder`, or say who holds it.
    pub(crate) fn take(&self, holder: &str) -> Result<Held> {
        let mut file = self.open()?;
        // Called through the trait: the standard library's own `try_lock`
        // is newer than the workspace's minimum Rust version.
        match FileExt::try_lock(&file) {
            Ok(()) => {}
            Err(TryLockError::Error(e)) => return Err(io_error("locking", &self.path, e)),
            Err(TryLockError::WouldBlock) => {
                let (fence, held_by) = self.current()?;
                return Err(unavailable(format!(
                    "the lake's writer lease is held by `{held_by}` (fence {fence}); it must be \
                     released before another writer can take it"
                )));
            }
        }
        let mut recorded = read(&mut file, &self.path)?;
        recorded.fence = recorded.fence.max(recorded.import_fence.unwrap_or(0)) + 1;
        recorded.holder = holder.to_owned();
        write(&mut file, &self.path, &recorded)?;
        let lease = Lease {
            fence: recorded.fence,
            holder: recorded.holder,
        };
        Ok(Held { lease, file })
    }

    /// The last position an import appended, while the log holds nothing
    /// else.
    pub(crate) fn imported(&self, held: &mut Held) -> Result<Option<u64>> {
        Ok(read(&mut held.file, &self.path)?.imported)
    }

    /// Record that an import appended through `last`, with fences up to
    /// `fence`: a later lease is greater than all of them.
    pub(crate) fn record_import(&self, held: &mut Held, last: u64, fence: u64) -> Result<()> {
        let mut recorded = read(&mut held.file, &self.path)?;
        recorded.holder = held.lease.holder.clone();
        recorded.imported = Some(last);
        recorded.import_fence = Some(recorded.import_fence.unwrap_or(0).max(fence));
        write(&mut held.file, &self.path, &recorded)
    }

    /// Give a held lease up: the file keeps its token and names no holder,
    /// and the lock is released. The lock is released whatever happens; a
    /// failure to clear the holder is reported after it.
    pub(crate) fn release(&self, mut held: Held) -> Result<()> {
        // The file keeps the greatest token and what an import recorded.
        let cleared = read(&mut held.file, &self.path).and_then(|mut recorded| {
            recorded.fence = recorded.fence.max(held.lease.fence);
            recorded.holder.clear();
            write(&mut held.file, &self.path, &recorded)
        });
        let unlocked =
            FileExt::unlock(&held.file).map_err(|e| io_error("unlocking", &self.path, e));
        drop(held);
        cleared.and(unlocked)
    }
}

#[cfg(test)]
mod tests;

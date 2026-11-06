//! The writer lease: a file holding the fencing token, under an operating
//! system lock.
//!
//! `lease` in the lake's directory holds the last fencing token handed
//! out and who holds it now, as one JSON object. Taking the lease takes an
//! exclusive lock on that file without waiting: when another handle holds
//! it — in this process or any other — the take fails at once, naming the
//! holder the file records where the file can be read. The lock is the
//! operating system's, so it is released when its process ends, however
//! it ends, and a crashed writer never keeps the lake from the next one.
//! The taker writes a token one greater than the file's and syncs it
//! before it may append.
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

/// What Windows answers a read with when another handle holds the file's
/// lock: `ERROR_LOCK_VIOLATION`. No Unix read fails with it — there it is
/// `EDOM`, which no file operation returns — so testing the number
/// costs nothing on either.
const LOCK_VIOLATION: i32 = 33;

fn read_text(file: &mut File) -> std::io::Result<String> {
    let mut text = String::new();
    file.seek(SeekFrom::Start(0))?;
    file.read_to_string(&mut text)?;
    Ok(text)
}

fn parse(text: &str) -> Result<Recorded> {
    if text.trim().is_empty() {
        return Ok(Recorded::default());
    }
    serde_json::from_str(text).map_err(|e| {
        let flaw = Flaw::NotJson {
            detail: Detail::json(&e),
        };
        damaged(Part::LeaseFile, None, flaw)
    })
}

fn read(file: &mut File) -> Result<Recorded> {
    let text = read_text(file).map_err(|e| io_error(Doing::Read, Part::LeaseFile, e))?;
    parse(&text)
}

/// The fence and holder a read of the file answers. A read the operating
/// system refused because another handle holds the file's lock names no
/// holder: Windows makes the lock cover the file's bytes, so a handle
/// that does not hold the lease cannot read who does, and a caller that
/// holds none is answered the same whether the file cannot be read or
/// says nothing. Every other failure is still the storage's.
fn answer(asked: std::io::Result<String>) -> Result<(u64, String)> {
    match asked {
        Ok(text) => parse(&text).map(|r| (r.fence, r.holder)),
        Err(e) if e.raw_os_error() == Some(LOCK_VIOLATION) => Ok((0, String::new())),
        Err(e) => Err(io_error(Doing::Read, Part::LeaseFile, e)),
    }
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

    /// Who the file says holds the lease, and with which token: no holder
    /// when another handle's lock refuses the read, which is all a caller
    /// that does not hold the lease can be told.
    pub(crate) fn current(&self) -> Result<(u64, String)> {
        answer(read_text(&mut self.open()?))
    }

    /// Who the file says holds the lease, and with which token, read
    /// through the file this handle holds locked: a holder must not open
    /// a second handle for it, which Windows refuses while the lock is
    /// held.
    pub(crate) fn recorded(&self, held: &mut Held) -> Result<(u64, String)> {
        let r = read(&mut held.file)?;
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

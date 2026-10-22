//! The projection a rebuild builds beside the lake's own
//! (`docs/decisions/071-a-rebuild-builds-beside-the-projection.md`).
//!
//! The scratch is `projection.db.rebuild`, in the lake's directory, so the
//! swap is a rename within one directory: atomic, and the projection the
//! lake answers from is whole on either side of it. Beside it,
//! `projection.db.rebuild.mark` is the file a handle building the scratch
//! keeps locked, as the lease file is locked, and holds the scratch's mark
//! once it is built: the lake, the layout, the position the scratch is
//! folded through and the log's hash there, as one JSON object. A mark that
//! is empty or does not parse marks nothing. The mark is written after the
//! scratch's journal is checkpointed into its file, so a mark that is there
//! names a scratch whole at it; one folded on since is no longer what the
//! mark names, and is removed as an unmarked one is.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs4::{FileExt, TryLockError};
use lakelands_api::engine::CanonicalEvent;
use lakelands_api::error::{Doing, Error, Part, Result, Situation};
use lakelands_api::event::Digest;
use lakelands_api::id::{LakeId, Position};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::log::Log;
use crate::{io_error, projection, sql};

/// The scratch's file, in the lake's directory.
pub(crate) const SCRATCH: &str = "projection.db.rebuild";

/// The file locked while the scratch is open, holding its mark.
const MARK: &str = "projection.db.rebuild.mark";

/// What a built scratch is marked with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Mark {
    lake: String,
    layout: u64,
    pub(crate) position: Option<u64>,
    hash: Option<String>,
}

impl Mark {
    /// The mark of a scratch of `lake` folded through `at`, with the log's
    /// event there.
    pub(crate) fn of(lake: &LakeId, at: Option<&CanonicalEvent>) -> Mark {
        Mark {
            lake: lake.to_string(),
            layout: projection::LAYOUT,
            position: at.map(|e| e.position.0),
            hash: at.map(|e| e.hash.0.clone()),
        }
    }

    /// The mark of a scratch of `lake` folded through the log's `tip`.
    pub(crate) fn of_tip(lake: &LakeId, tip: Option<&(Position, Digest)>) -> Mark {
        Mark {
            lake: lake.to_string(),
            layout: projection::LAYOUT,
            position: tip.map(|(p, _)| p.0),
            hash: tip.map(|(_, h)| h.0.clone()),
        }
    }
}

/// The scratch of one lake directory, locked: while this is held, no other
/// handle opens, removes or swaps it. The lock is the operating system's,
/// so a builder that ends however it ends gives it up.
pub(crate) struct Building {
    file: File,
    dir: PathBuf,
}

/// Unlocks, and takes the mark file with it where no scratch is left.
impl Drop for Building {
    fn drop(&mut self) {
        if !self.path().exists() {
            let _ = std::fs::remove_file(self.dir.join(MARK));
        }
        let _ = FileExt::unlock(&self.file);
    }
}

/// The refusal of a scratch another handle has open.
pub(crate) fn rebuilding(lake: &LakeId) -> Error {
    let lake = lake.to_string();
    Error::unavailable(Situation::Rebuilding { lake })
}

/// Whether a scratch, or its mark, is left in `dir`.
pub(crate) fn left(dir: &Path) -> bool {
    dir.join(SCRATCH).exists() || dir.join(MARK).exists()
}

/// Lock the scratch of the lake in `dir`; none while another handle has it.
pub(crate) fn lock(dir: &Path) -> Result<Option<Building>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(MARK))
        .map_err(|e| io_error(Doing::Open, Part::Projection, e))?;
    match FileExt::try_lock(&file) {
        Ok(()) => Ok(Some(Building {
            file,
            dir: dir.to_owned(),
        })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(io_error(Doing::Lock, Part::Projection, e)),
    }
}

/// Remove the scratch an earlier builder left in `dir` unless it may still
/// be swapped in: one another handle has open is left to it, and so is one
/// marked by a build of another layout, whose scratch this one does not
/// read, while the log holds its mark.
pub(crate) fn sweep(dir: &Path, lake: &LakeId, log: &mut Log) -> Result<()> {
    if !left(dir) {
        return Ok(());
    }
    let Some(mut building) = lock(dir)? else {
        return Ok(());
    };
    if building.fit(lake, log, true)?.is_none() {
        building.remove()?;
    }
    Ok(())
}

impl Building {
    /// The scratch's file.
    pub(crate) fn path(&self) -> PathBuf {
        self.dir.join(SCRATCH)
    }

    /// The mark the file holds, if it holds one.
    pub(crate) fn mark(&mut self) -> Result<Option<Mark>> {
        let mut text = String::new();
        self.file
            .seek(SeekFrom::Start(0))
            .and_then(|_| self.file.read_to_string(&mut text))
            .map_err(|e| io_error(Doing::Read, Part::Projection, e))?;
        Ok(serde_json::from_str(&text).ok())
    }

    /// Write `mark`, or none, durably.
    pub(crate) fn set(&mut self, mark: Option<&Mark>) -> Result<()> {
        let text = mark.map_or_else(String::new, |m| {
            serde_json::to_string(m).expect("a mark serialises") + "\n"
        });
        self.file
            .set_len(0)
            .and_then(|()| self.file.seek(SeekFrom::Start(0)))
            .and_then(|_| self.file.write_all(text.as_bytes()))
            .and_then(|()| self.file.sync_all())
            .map_err(|e| io_error(Doing::Write, Part::Projection, e))
    }

    /// The scratch's mark, when it is fit to keep: marked for `lake`, the
    /// mark's position holding its hash in `log`, and, unless it is
    /// another build's and `others` keeps those, laid out at this build's
    /// layout and folded through the mark.
    pub(crate) fn fit(
        &mut self,
        lake: &LakeId,
        log: &mut Log,
        others: bool,
    ) -> Result<Option<Mark>> {
        let Some(mark) = self.mark()? else {
            return Ok(None);
        };
        let held = match mark.position {
            Some(p) => log
                .read(Position(p), 1)?
                .pop()
                .map(|e| (e.position.0, e.hash.0)),
            None => None,
        };
        if mark.lake != lake.as_str() || held != mark.position.zip(mark.hash.clone()) {
            return Ok(None);
        }
        if mark.layout != projection::LAYOUT {
            return Ok(others.then_some(mark));
        }
        // A file that is gone, or not a projection, is no scratch to keep.
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let folded = Connection::open_with_flags(self.path(), flags)
            .map_err(sql)
            .and_then(|conn| {
                let laid =
                    projection::made(&conn)? && projection::layout(&conn)? == Some(mark.layout);
                Ok(if laid {
                    projection::folded(&conn)?.map(|p| p.0)
                } else {
                    None
                })
            });
        Ok((folded.ok().flatten() == mark.position && mark.position.is_some()).then_some(mark))
    }

    /// Remove the scratch and its mark; the lock stays held.
    pub(crate) fn remove(&mut self) -> Result<()> {
        for end in ["", "-wal", "-shm", "-journal"] {
            let path = self.dir.join(format!("{SCRATCH}{end}"));
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(io_error(Doing::Write, Part::Projection, e));
                }
                _ => {}
            }
        }
        self.set(None)
    }

    /// Fold the scratch's journal into its file and leave it with none, so
    /// that the file alone is the scratch: busy while another connection
    /// has it open.
    pub(crate) fn settle(&self, lake: &LakeId) -> Result<()> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(self.path(), flags).map_err(sql)?;
        let mode = conn.query_row("PRAGMA journal_mode = DELETE", [], |r| {
            r.get::<_, String>(0)
        });
        if mode.ok().as_deref() != Some("delete") {
            return Err(rebuilding(lake));
        }
        Ok(())
    }
}

//! A file held under an exclusive lock for as long as its holder lives:
//! one mirror per connection, one deliverer per endpoint.
//!
//! The lock is the operating system's `flock`, which belongs to the open
//! file, not to one descriptor. A child the process forks while the lock
//! is held keeps a copy of the descriptor until it execs, so closing the
//! holder's descriptor alone could leave the lock held until then, and a
//! holder started again at once would meet its own lock. [`FileLock`]
//! unlocks before it closes, which releases the lock whoever else still
//! holds a copy.

use std::fs::{File, OpenOptions};
use std::path::Path;

/// A file under an exclusive lock, released when this is dropped.
#[derive(Debug)]
pub struct FileLock(File);

impl FileLock {
    /// Lock the file at `path`, making it when absent. `Ok(None)` when
    /// another holder, in this process or another, has it locked; an error
    /// names the file.
    pub fn try_acquire(path: &Path) -> Result<Option<FileLock>, String> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|err| format!("{} cannot be opened: {err}", path.display()))?;
        // Called through the trait: the standard library's own `try_lock`
        // is newer than the workspace's minimum Rust version.
        match fs4::FileExt::try_lock(&file) {
            Ok(()) => Ok(Some(FileLock(file))),
            Err(fs4::TryLockError::WouldBlock) => Ok(None),
            Err(fs4::TryLockError::Error(err)) => {
                Err(format!("{} cannot be locked: {err}", path.display()))
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs4::FileExt::unlock(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A copy of the descriptor, as a child forked while the lock was held
    /// keeps until it execs, does not keep the lock once it is dropped; a
    /// second lock while the first lives is refused.
    #[test]
    fn a_dropped_lock_is_released_while_a_copy_of_its_descriptor_lives() {
        let dir = std::env::temp_dir().join(format!("lakelands-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("held.lock");
        let first = FileLock::try_acquire(&path)
            .unwrap()
            .expect("the first lock");
        let copy = first.0.try_clone().unwrap();
        assert!(
            FileLock::try_acquire(&path).unwrap().is_none(),
            "a second lock is refused while the first lives"
        );
        drop(first);
        assert!(
            FileLock::try_acquire(&path).unwrap().is_some(),
            "the lock outlived its holder through a copy of its descriptor"
        );
        drop(copy);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

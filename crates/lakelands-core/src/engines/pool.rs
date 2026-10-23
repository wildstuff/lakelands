//! A pool of reader connections, so the reads of one lake run side by
//! side, each in a read transaction on a connection of its own. One is
//! opened only when every open one is taken, up to the pool's size: at
//! most [`MOST`], since past 16, measured on 16 cores, they answered no
//! more reads a second; and for an engine whose reads run on this
//! machine's cores, at most one per core ([`per_core`]), since past that
//! they wait on the cores instead.

use std::ops::{Deref, DerefMut};
use std::sync::{Condvar, Mutex, PoisonError};

use lakelands_api::error::Result;

use crate::lock;

/// The most connections a pool opens, however many cores.
pub const MOST: usize = 16;

/// The size of a pool whose reads run on this machine's cores.
pub fn per_core() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().min(MOST))
}

/// Connections of one kind `C`, at most its size open.
pub struct Pool<C> {
    slots: Mutex<Slots<C>>,
    given: Condvar,
    size: usize,
}

struct Slots<C> {
    idle: Vec<C>,
    taken: usize,
    /// Set by [`Pool::shut`] until [`Pool::open`].
    shut: bool,
}

/// A connection taken from a pool, given back when dropped.
pub struct Taken<'p, C> {
    pool: &'p Pool<C>,
    conn: Option<C>,
}

impl<C> Pool<C> {
    /// A pool of at most `size` connections, the `idle` ones open in it.
    pub fn new(idle: Vec<C>, size: usize) -> Pool<C> {
        let (taken, shut) = (0, false);
        Pool {
            slots: Mutex::new(Slots { idle, taken, shut }),
            given: Condvar::new(),
            size,
        }
    }

    /// An idle connection, else one `open` opens while fewer than the
    /// size are open, else the first one given back.
    pub fn take(&self, open: impl FnOnce() -> Result<C>) -> Result<Taken<'_, C>> {
        let full = |s: &mut Slots<C>| s.shut || (s.idle.is_empty() && s.taken >= self.size);
        let waited = self.given.wait_while(lock(&self.slots), full);
        let mut slots = waited.unwrap_or_else(PoisonError::into_inner);
        slots.taken += 1;
        let (pool, conn) = (self, slots.idle.pop());
        let mut taken = Taken { pool, conn };
        drop(slots);
        if taken.conn.is_none() {
            taken.conn = Some(open()?);
        }
        Ok(taken)
    }

    /// Close every connection, once those taken are given back; a read
    /// waits until [`Self::open`].
    pub fn shut(&self) {
        let mut slots = lock(&self.slots);
        (slots.shut, slots.idle) = (true, Vec::new());
        drop(self.given.wait_while(slots, |s| s.taken > 0));
    }

    /// Let reads take connections again after [`Self::shut`].
    pub fn open(&self) {
        lock(&self.slots).shut = false;
        self.given.notify_all();
    }
}

impl<C> Drop for Taken<'_, C> {
    /// A connection given back while its read panics is closed: it may be
    /// inside that read's transaction.
    fn drop(&mut self) {
        let mut slots = lock(&self.pool.slots);
        slots.taken -= 1;
        if slots.shut {
            // Closed before `shut` returns.
            drop(self.conn.take());
            return self.pool.given.notify_all();
        } else if !std::thread::panicking() {
            slots.idle.extend(self.conn.take());
        }
        self.pool.given.notify_one();
    }
}

impl<C> Deref for Taken<'_, C> {
    type Target = C;

    fn deref(&self) -> &C {
        self.conn.as_ref().expect("a connection until given back")
    }
}

impl<C> DerefMut for Taken<'_, C> {
    fn deref_mut(&mut self) -> &mut C {
        self.conn.as_mut().expect("a connection until given back")
    }
}

#[cfg(test)]
mod tests;

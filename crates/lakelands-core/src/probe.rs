//! Throwaway timing counters for the lakelands#1575 measurement; not product.
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

macro_rules! counters { ($($n:ident),*) => { $(pub static $n: AtomicU64 = AtomicU64::new(0);)*
    pub fn snapshot() -> Vec<(&'static str, u64)> { vec![$((stringify!($n), $n.load(Relaxed))),*] }
    pub fn reset() { $($n.store(0, Relaxed);)* } } }
counters!(
    POOL_TAKES, POOL_WAITS, POOL_WAIT_NS,
    CHOSEN_N, CHOSEN_NS, TALLY_NS, NAME_SORTED_NS, DESCRIBE_NS, READING_NS
);
pub fn add(c: &AtomicU64, since: Instant) {
    c.fetch_add(since.elapsed().as_nanos() as u64, Relaxed);
}
pub fn inc(c: &AtomicU64) {
    c.fetch_add(1, Relaxed);
}

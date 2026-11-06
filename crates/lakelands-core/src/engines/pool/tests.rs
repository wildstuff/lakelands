use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use lakelands_api::error::Error;

use super::Pool;

const LONG: Duration = Duration::from_secs(10);
const SHORT: Duration = Duration::from_millis(100);

/// A connection that counts itself closed, slowly.
struct Conn(Arc<AtomicUsize>);

impl Drop for Conn {
    fn drop(&mut self) {
        thread::sleep(SHORT);
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// A connection that failed to open takes no place in the pool.
#[test]
fn an_open_that_fails_gives_its_place_back() {
    let pool = Arc::new(Pool::<u8>::new(Vec::new(), 1));
    assert!(pool.take(|| Err(Error::not_granted())).is_err());
    let (said, heard) = mpsc::channel();
    let other = pool.clone();
    thread::spawn(move || said.send(*other.take(|| Ok(7)).unwrap()));
    assert_eq!(heard.recv_timeout(LONG), Ok(7), "the place is free");
}

/// A pool at its size gives the next read the connection given back,
/// opening none.
#[test]
fn a_full_pool_gives_the_next_read_the_one_given_back() {
    let pool = Arc::new(Pool::new(vec![1u8], 1));
    let first = pool.take(|| unreachable!()).unwrap();
    let (said, heard) = mpsc::channel();
    let other = pool.clone();
    thread::spawn(move || said.send(*other.take(|| Ok(2)).unwrap()));
    assert!(heard.recv_timeout(SHORT).is_err(), "it waits");
    drop(first);
    assert_eq!(heard.recv_timeout(LONG), Ok(1));
}

/// `shut` returns once the taken connection is given back and closed,
/// and a read waits until `open`, on a connection opened anew.
#[test]
fn a_shut_pool_closes_its_connections_and_reads_wait_for_open() {
    let closed = Arc::new(AtomicUsize::new(0));
    let pool = Arc::new(Pool::new(vec![Conn(closed.clone())], 2));
    let held = pool.take(|| unreachable!()).unwrap();
    let (shut, has_shut) = mpsc::channel();
    let (other, counted) = (pool.clone(), closed.clone());
    thread::spawn(move || {
        other.shut();
        shut.send(counted.load(Ordering::SeqCst)).unwrap();
    });
    assert!(
        has_shut.recv_timeout(SHORT).is_err(),
        "it waits for the read"
    );
    drop(held);
    assert_eq!(has_shut.recv_timeout(LONG), Ok(1), "closed when it returns");
    let (said, heard) = mpsc::channel();
    let (other, opened) = (pool.clone(), closed.clone());
    thread::spawn(move || {
        let _conn = other.take(|| Ok(Conn(opened))).unwrap();
        said.send(()).unwrap();
    });
    assert!(
        heard.recv_timeout(SHORT).is_err(),
        "a read waits while shut"
    );
    pool.open();
    assert!(heard.recv_timeout(LONG).is_ok());
}

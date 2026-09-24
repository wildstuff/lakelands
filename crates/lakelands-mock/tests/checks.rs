//! The door checks, run on the mock.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use lakelands_api::LakeStore;
use lakelands_mock::MockLake;

/// Runs a future that never waits: every door of the mock answers at
/// once, so a no-op waker is enough and no runtime is needed.
fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(out) = f.as_mut().poll(&mut cx) {
            return out;
        }
    }
}

lakelands_mock::door_checks!(
    || Arc::new(MockLake::new("finance")) as Arc<dyn LakeStore>,
    block_on
);

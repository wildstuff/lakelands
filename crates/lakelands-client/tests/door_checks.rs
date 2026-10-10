//! The door checks, run unchanged on a lake reached over the REST door:
//! a fresh in-memory store per check, served by `serve_http` on
//! loopback, with a resolver whose entries are the checks' cast, each
//! principal with its grants and the key `k-<id>`, and a `RemoteLake`
//! keyed by those ids. The checks that call a host operation, and those
//! that read where one in-process feed poll stops (`in_process_feed`),
//! are marked so in `door_checks!` and are ignored here, each saying why.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use lakelands_api::LakeStore;
use lakelands_api::location::Limits;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a runtime")
    })
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    runtime().block_on(f)
}

/// A fresh lake name: every check gets its own lake.
fn fresh() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "finance-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn remote() -> Arc<dyn LakeStore> {
    let store = block_on(common::memory_lake(&fresh(), Limits::default()));
    let (addr, keys) = block_on(common::serve(store.clone()));
    Arc::new(common::remote(addr, store.lake(), keys))
}

lakelands_door_checks::door_checks!(remote, block_on, remote);

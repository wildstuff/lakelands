//! The door checks, run on the store over memory and SQLite.
//!
//! Every check runs against `lakelands_store::open` on both engines. A
//! check that passes on one and fails on the other is a divergence between
//! the engines, fixed in the store or the engine, never in the check.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};

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

fn open(location: Location) -> Arc<dyn LakeStore> {
    let seams = Seams {
        engines: vec![
            Arc::new(lakelands_engine_mem::MemoryOpener::new()),
            Arc::new(lakelands_engine_sqlite::SqliteOpener),
        ],
        ..Seams::default()
    };
    block_on(lakelands_store::open(location, seams, OpenMode::Create)).expect("the lake opens")
}

fn memory() -> Arc<dyn LakeStore> {
    open(Location::memory(fresh()))
}

fn sqlite() -> Arc<dyn LakeStore> {
    let lake = fresh();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("door-checks")
        .join(&lake);
    let _ = std::fs::remove_dir_all(&dir);
    open(Location::sqlite(lake, dir))
}

mod on_memory {
    use super::*;
    lakelands_door_checks::door_checks!(memory, block_on);
}

mod on_sqlite {
    use super::*;
    lakelands_door_checks::door_checks!(sqlite, block_on);
}

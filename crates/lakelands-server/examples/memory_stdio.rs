//! An MCP server over stdio on a lake held in memory, holding the door
//! checks' fixture ontology and served as the lake's owner: something to
//! point `mcp_call` at without a directory or a database.
//!
//! ```sh
//! cargo run -p lakelands-server --example mcp_call -- \
//!   --server "cargo run -q -p lakelands-server --example memory_stdio" tools/list
//! ```

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let seams = Seams {
        engines: vec![Arc::new(lakelands_engine_mem::MemoryOpener::new())],
        ..Seams::default()
    };
    let opened = lakelands_store::open(Location::memory("finance"), seams, OpenMode::Create).await;
    let store: Arc<dyn LakeStore> = opened.map_err(std::io::Error::other)?;
    let fixture = lakelands_door_checks::checks::setup(store.clone()).await;
    lakelands_server::serve_stdio(store, fixture.cast.owner).await
}

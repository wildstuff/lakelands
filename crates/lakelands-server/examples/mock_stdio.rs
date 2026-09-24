//! An MCP server over stdio on an in-memory mock lake holding the mock's
//! fixture ontology, served as the lake's owner: something to point
//! `mcp_call` at without a store.
//!
//! ```sh
//! cargo run -p lakelands-server --example mcp_call -- \
//!   --server "cargo run -q -p lakelands-server --example mock_stdio" tools/list
//! ```

use std::sync::Arc;

use lakelands_api::LakeStore;
use lakelands_mock::MockLake;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let store: Arc<dyn LakeStore> = Arc::new(MockLake::new("finance"));
    let fixture = lakelands_mock::checks::setup(store.clone()).await;
    lakelands_server::serve_stdio(store, fixture.cast.owner).await
}

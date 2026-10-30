//! A cursor handed out before a hybrid search's answer named its vector
//! leg in `unranked` carries no marker. Where no embedding backend is
//! bound, its page ranks by its text, as it did, and says so
//! (`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
//!
//! The suite holds a cursor opaque, so the row is here: a cursor of this
//! lake, its marker taken out, stands for one of a lake before it.

use std::sync::Arc;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::query::{Hits, Search, SearchMode};
use lakelands_api::read::ReadAt;
use lakelands_api::seam::{EmbedBackend, Seams};
use lakelands_api::{Cursor, LakeStore, Location, TypeName, canonical};
use lakelands_core::conformance::backing;
use lakelands_core::conformance::fixture::{Lake, doors, embedding, owner, searchable, type_decl};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

async fn open(
    opener: &Arc<dyn EngineOpener>,
    embed: Option<Arc<dyn EmbedBackend>>,
    mode: OpenMode,
) -> Arc<dyn LakeStore> {
    let seams = Seams {
        engines: vec![opener.clone()],
        backings: backing::backings(),
        embed,
        ..Seams::default()
    };
    let at = Location::memory("unmarked-cursor");
    lakelands_store::open(at, seams, mode)
        .await
        .expect("a handle on the lake")
}

/// A page of a hybrid search for "quayside harbour" over `e`, without a
/// vector.
async fn page(store: &dyn LakeStore, cursor: Option<Cursor>) -> Hits {
    let mode = SearchMode::Hybrid {
        text: "quayside harbour".into(),
        field: "e".into(),
        vector: None,
        fields: None,
        text_fields: None,
        weight: None,
        text_weight: None,
        model: None,
    };
    let search = Search {
        ty: TypeName::from("doc"),
        mode,
        k: 1,
        cursor,
        filter: None,
        with_meta: false,
        matches: false,
        at: ReadAt::default(),
        rerank: None,
        text_as_of: false,
    };
    let got = doors(store).search(&owner(store), search).await;
    got.expect("a page of the search").data
}

/// `c` without its `unranked`.
fn unmarked(c: &Cursor) -> Cursor {
    let bytes: Vec<u8> = (0..c.0.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&c.0[i..i + 2], 16).expect("a hex cursor"))
        .collect();
    let mut state: serde_json::Value = serde_json::from_slice(&bytes).expect("a JSON cursor");
    let marker = state.as_object_mut().and_then(|s| s.remove("unranked"));
    assert!(
        marker.is_some(),
        "the first page's cursor carries `unranked`"
    );
    Cursor(canonical::hex(&canonical::of(&state)))
}

#[tokio::test]
async fn a_cursor_without_a_marker_ranks_by_its_text_where_no_backend_is_bound() {
    let opener: Arc<dyn EngineOpener> = Arc::new(MemoryOpener::new());
    let store = open(&opener, Some(backing::embedder()), OpenMode::Create).await;
    let e = embedding("e", 2, &["body"], backing::EMBED_MODEL);
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("doc", vec![searchable("body"), e]))
        .await
        .unwrap();
    l.upsert("doc", "a", json!({"body": "quayside"}))
        .await
        .unwrap();
    l.upsert("doc", "b", json!({"body": "harbour"}))
        .await
        .unwrap();
    store.close().await.unwrap();
    let store = open(&opener, None, OpenMode::Write).await;
    let first = page(store.as_ref(), None).await;
    let cursor = first.next_cursor.as_ref().expect("a second page");
    let next = page(store.as_ref(), Some(unmarked(cursor))).await;
    let keys = |h: &Hits| {
        h.hits
            .iter()
            .map(|h| h.record.key.0.clone())
            .collect::<Vec<_>>()
    };
    assert!(
        first.unranked.is_some(),
        "the first page names its vector leg"
    );
    assert_eq!(next.ranked_by, "lexical");
    assert_eq!(next.unranked, first.unranked);
    assert_eq!(keys(&next).len(), 1);
    assert_ne!(keys(&next), keys(&first));
}

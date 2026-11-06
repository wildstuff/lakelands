//! A read of the feed index by a key and a type takes the key's index, not
//! the type's: a scoped forget names the keys of one source, and the type
//! holds most of the lake's routes (lakelands#1659). The plan is that of
//! the statement the view's feed read runs, built by the same code.

use std::path::PathBuf;

use lakelands_api::Location;
use lakelands_api::engine::{About, Applied, EngineOpener, OpenMode};
use lakelands_api::event::EventId;
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_api::seam::FeedFilter;
use lakelands_engine_sqlite::{SqliteOpener, keyed_routes_plan};

#[test]
fn a_keyed_read_of_the_feed_takes_the_key_index_where_the_type_is_the_less_selective_column() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("forget-key-index-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let engine = SqliteOpener
        .open(&Location::sqlite("forget", &dir), OpenMode::Create)
        .unwrap();
    // One type for every record, a key for each.
    for first in (1..=20_000u64).step_by(500) {
        let p = engine.projection();
        p.begin().unwrap();
        for at in first..first + 500 {
            p.mark_applied(&Applied {
                id: EventId(format!("{at:032x}")),
                at: Position(at),
                kind: "record_ingested",
                about: vec![About {
                    ty: Some(TypeName::from("t")),
                    key: Some(RecordKey::from(format!("k{at:05}"))),
                }],
                source: None,
            })
            .unwrap();
        }
        p.commit(Position(first + 499)).unwrap();
    }
    drop(engine);
    let db = rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    let filter = FeedFilter {
        types: vec![TypeName::from("t")],
        keys: vec![RecordKey::from("k00042".to_owned())],
        ..FeedFilter::default()
    };
    let plan = keyed_routes_plan(&db, &filter, "k00042")
        .unwrap()
        .expect("the type and key are held");
    assert!(
        plan.iter().any(|d| d.contains("routes_key")),
        "the plan takes the key index: {plan:?}"
    );
    assert!(
        !plan.iter().any(|d| d.contains("routes_ty")),
        "the plan does not take the type index: {plan:?}"
    );
}

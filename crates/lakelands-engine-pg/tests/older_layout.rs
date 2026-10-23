//! A projection an older build laid out (lakelands#1151): its `routes`
//! without the `source` column, its states without the `procedure` a
//! computed field's metadata names (lakelands#1191), its states holding
//! their rows' type, key and position and their false flags
//! (lakelands#1318), and its mark one layout back. A reader's open is refused naming the layouts; the
//! writer's open rebuilds it from the log under its lease, saying why,
//! and the next writer's open rebuilds nothing.
//!
//! A build between those leaner states and the vectors kept apart
//! (lakelands#1323) marked the current layout with its states holding
//! their vectors and `vectors` a head index: its projection is told apart
//! by that table's shape and read as the layout before. The check goes at
//! the release, which ships both changes as one layout.

mod common;

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::error::Situation;
use lakelands_api::ontology::StorageKind::{Decimal, Text};
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::fixture::{
    Lake, computed_by, source_record, sourced, type_decl, vector,
};
use lakelands_engine_pg::PgOpener;
use serde_json::json;

async fn open(location: &Location, mode: OpenMode) -> lakelands_api::Result<Arc<dyn LakeStore>> {
    let seams = Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    };
    lakelands_store::open(location.clone(), seams, mode).await
}

/// The first column of the first row `statement` answers.
fn one(url: &str, statement: &str) -> i64 {
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    client.query_one(statement, &[]).unwrap().get(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writers_open_rebuilds_a_projection_an_older_build_laid_out() {
    let Some(url) = common::url_or_skip("older layout") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "older", &schema);
    let store = open(&location, OpenMode::Create).await.unwrap();
    let l = Lake::of(store.as_ref());
    let twice = json!({"declare": "function", "name": "twice", "backing": "expr",
        "inputs": [{"name": "n", "type": {"kind": "decimal"}}], "expr": "n * 2"});
    let twice = serde_json::from_value(twice).unwrap();
    l.declare_as(twice, "declaring `twice`").await.unwrap();
    let fields = vec![
        sourced("text", Text),
        sourced("n", Decimal),
        computed_by("double", Decimal, "twice", &["n"]),
    ];
    l.declare(type_decl("note", fields)).await.unwrap();
    let a = source_record("note", "a", json!({"text": "kept", "n": "2"}));
    l.ingest("crm", vec![a]).await.unwrap();
    store.close().await.unwrap();
    let layout = format!("SELECT n FROM \"{schema}\".meta WHERE k = 'layout'");
    let wanted = tokio::task::block_in_place(|| one(&url, &layout));
    let older = format!(
        "DROP INDEX \"{schema}\".routes_source; ALTER TABLE \"{schema}\".routes DROP COLUMN source; \
         UPDATE \"{schema}\".meta SET n = {} WHERE k = 'layout'",
        wanted - 1
    );
    let unnamed = format!(
        "UPDATE \"{schema}\".records SET state = (jsonb_build_object('type', ty, 'key', key, \
         'position', since, 'retired', false, 'restricted', false) \
         || replace(state, '\"procedure\":\"twice\",', '')::jsonb)::text"
    );
    tokio::task::block_in_place(|| {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        let n = client.execute(&unnamed, &[]).unwrap();
        assert!(n > 0, "a state names `twice` as its procedure");
        client.batch_execute(&older).unwrap();
    });
    let refused = open(&location, OpenMode::Read).await.err().unwrap();
    assert!(
        matches!(refused.situation(), Some(Situation::ProjectionLayout { found, .. }) if *found == Some(wanted as u64 - 1)),
        "a reader's open of the older layout is refused: {refused:?}"
    );
    let store = open(&location, OpenMode::Write).await.unwrap();
    let why = store.rebuilt_at_open();
    assert!(
        why.as_ref()
            .is_some_and(|w| w.contains(&format!("layout {}", wanted - 1))),
        "the writer's open says why it rebuilt: {why:?}"
    );
    let note = Lake::of(store.as_ref()).read("note", "a").await.unwrap();
    assert_eq!(note.fields["text"], "kept");
    let procedure = note.meta["double"].procedure.as_deref();
    assert_eq!(procedure, Some("twice"), "the rebuild names the function");
    let sourced = format!("SELECT count(*) FROM \"{schema}\".routes WHERE source = 'crm'");
    let (sourced, now) = tokio::task::block_in_place(|| (one(&url, &sourced), one(&url, &layout)));
    assert!(
        sourced > 0 && now == wanted,
        "the rebuild indexed crm's events at layout {wanted}"
    );
    let held = format!(
        "SELECT count(*) FROM \"{schema}\".records WHERE state::jsonb ?| \
         array['type', 'key', 'position'] OR state::jsonb @> '{{\"retired\": false}}' \
         OR state::jsonb @> '{{\"restricted\": false}}'"
    );
    let held = tokio::task::block_in_place(|| one(&url, &held));
    assert_eq!(
        held, 0,
        "no state holds what its row holds, or a false flag"
    );
    store.close().await.unwrap();
    let store = open(&location, OpenMode::Write).await.unwrap();
    assert_eq!(
        store.rebuilt_at_open(),
        None,
        "the next writer's open rebuilds nothing"
    );
    store.close().await.unwrap();
    common::drop_schema(&url, &schema);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writers_open_rebuilds_a_projection_laid_out_between_the_two_changes() {
    let Some(url) = common::url_or_skip("in-between layout") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "between", &schema);
    let store = open(&location, OpenMode::Create).await.unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("pin", vec![vector("v", 3)]))
        .await
        .unwrap();
    let elements = json!([0.1, -0.5, 3]);
    l.upsert("pin", "k", json!({"v": elements})).await.unwrap();
    store.close().await.unwrap();
    let layout = format!("SELECT n FROM \"{schema}\".meta WHERE k = 'layout'");
    let wanted = tokio::task::block_in_place(|| one(&url, &layout));
    // The in-between build's projection: the elements in the state, as
    // `f32`s in a head index, the mark the current one.
    let between = format!(
        "UPDATE \"{schema}\".records SET state = jsonb_set(state::jsonb - 'vectors', \
         '{{fields,v}}', '[0.1,-0.5,3]')::text; DROP TABLE \"{schema}\".vectors; \
         CREATE TABLE \"{schema}\".vectors (ty TEXT NOT NULL, field TEXT NOT NULL, \
         key TEXT NOT NULL, data BYTEA NOT NULL, PRIMARY KEY (ty, field, key))"
    );
    let data: Vec<u8> = [0.1f32, -0.5, 3.0]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    let row = format!("INSERT INTO \"{schema}\".vectors VALUES ('pin', 'v', 'k', $1)");
    tokio::task::block_in_place(|| {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        client.batch_execute(&between).unwrap();
        client.execute(&row, &[&data]).unwrap();
    });
    let refused = open(&location, OpenMode::Read).await.err().unwrap();
    assert!(
        matches!(refused.situation(), Some(Situation::ProjectionLayout { found, .. }) if *found == Some(wanted as u64 - 1)),
        "a reader's open of the in-between layout is refused as the layout before: {refused:?}"
    );
    let store = open(&location, OpenMode::Write).await.unwrap();
    let why = store.rebuilt_at_open();
    assert!(
        why.as_ref()
            .is_some_and(|w| w.contains(&format!("layout {}", wanted - 1))),
        "the writer's open says why it rebuilt: {why:?}"
    );
    let pin = Lake::of(store.as_ref()).read("pin", "k").await.unwrap();
    assert_eq!(
        pin.fields["v"], elements,
        "the rebuilt projection reads the elements"
    );
    let inline =
        format!("SELECT count(*) FROM \"{schema}\".records WHERE state::jsonb -> 'fields' ? 'v'");
    let inline = tokio::task::block_in_place(|| one(&url, &inline));
    assert_eq!(inline, 0, "no state holds a vector's elements");
    store.close().await.unwrap();
    let store = open(&location, OpenMode::Write).await.unwrap();
    assert_eq!(
        store.rebuilt_at_open(),
        None,
        "the next writer's open rebuilds nothing"
    );
    store.close().await.unwrap();
    common::drop_schema(&url, &schema);
}

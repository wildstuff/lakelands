//! Which commits wait for their flush
//! (`docs/decisions/061-a-projection-commit-waits-for-no-flush.md`): the
//! log's appends and seals always, the projection's commits never. No
//! test sees a WAL flush, so the setting is read where it acts, inside
//! the engine's own transactions, by triggers on the log's and the
//! projection's tables. By hand, with `LAKELANDS_TEST_PG_URL` set.

mod common;

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::write::Outcome;
use lakelands_api::{
    Get, Grants, LakeStore, Principal, PrincipalKind, ReadAt, Result, WriteContext,
};
use lakelands_engine_pg::PgOpener;
use serde_json::json;

/// Install the probe in `schema`: each statement on the log or on the
/// projection's records leaves a row with its table, the transaction's
/// `synchronous_commit` and the session's default of it.
fn probe(url: &str, schema: &str) {
    let sql = format!(
        "CREATE TABLE \"{schema}\".probe (tab text, setting text, session text); \
         CREATE FUNCTION \"{schema}\".probe() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
           INSERT INTO \"{schema}\".probe VALUES (TG_TABLE_NAME, \
             current_setting('synchronous_commit'), \
             (SELECT reset_val FROM pg_settings WHERE name = 'synchronous_commit')); \
           RETURN NULL; END $$; \
         CREATE TRIGGER probe AFTER INSERT ON \"{schema}\".log \
           FOR EACH STATEMENT EXECUTE FUNCTION \"{schema}\".probe(); \
         CREATE TRIGGER probe AFTER INSERT OR UPDATE ON \"{schema}\".records \
           FOR EACH STATEMENT EXECUTE FUNCTION \"{schema}\".probe();"
    );
    run(url, &sql, None);
}

/// Run `statements`, then `query`, on a connection of their own, and
/// answer the query's rows: three columns of text.
fn run(url: &str, statements: &str, query: Option<String>) -> Vec<(String, String, String)> {
    let (url, statements) = (url.to_owned(), statements.to_owned());
    std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        client.batch_execute(&statements).unwrap();
        let rows = query.map(|q| client.query(&q, &[]).unwrap());
        (rows.unwrap_or_default().iter())
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect()
    })
    .join()
    .unwrap()
}

/// A lake in `schema`, opened for writing with `lake_url`, with the type
/// `entry` declared, and its owner.
async fn lake(lake_url: &str, schema: &str) -> (Arc<dyn LakeStore>, Principal) {
    let location = common::location(lake_url, "ledger", schema);
    let seams = Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    };
    let store = lakelands_store::open(location, seams, OpenMode::Create)
        .await
        .unwrap();
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "ledger".into(),
        grants: Grants::owner(),
    };
    let cat = store.catalog(WriteContext::default());
    let declared = json!({"declare": "type", "name": "entry",
        "fields": [{"name": "note", "type": {"kind": "text"}}]});
    let declared = serde_json::from_value(declared).unwrap();
    if cat
        .describe_type(&owner, &"entry".into(), ReadAt::default())
        .await
        .is_err()
    {
        cat.declare(&owner, declared).await.unwrap();
    }
    (store, owner)
}

/// Write the record `key` of `entry`, its note the key.
async fn put(store: &dyn LakeStore, owner: &Principal, key: &str) -> Result<Outcome> {
    let patch = serde_json::from_value(json!({ "note": key })).unwrap();
    let (ty, key) = ("entry".into(), key.into());
    let cat = store.catalog(WriteContext::default());
    cat.upsert(owner, &ty, &key, patch, Default::default())
        .await
}

/// Write to a lake opened with `lake_url`, its transactions probed, and
/// answer what the probe saw: per table, each statement's setting and
/// its session's default.
async fn written(url: &str, lake_url: &str) -> Vec<(String, String, String)> {
    let schema = common::fresh_schema();
    let (store, owner) = lake(lake_url, &schema).await;
    probe(url, &schema);
    put(store.as_ref(), &owner, "a").await.unwrap();
    store.checkpoint().await.unwrap();
    store.close().await.unwrap();
    let query = format!("SELECT tab, setting, session FROM \"{schema}\".probe ORDER BY tab");
    let rows = run(url, "", Some(query));
    common::drop_schema(url, &schema);
    rows
}

/// The tables the probe saw, each with the settings its statements ran
/// under, deduplicated.
fn seen(rows: &[(String, String, String)]) -> Vec<(String, String)> {
    let mut seen: Vec<(String, String)> = rows.iter().map(|r| (r.0.clone(), r.1.clone())).collect();
    seen.dedup();
    seen
}

#[tokio::test(flavor = "multi_thread")]
async fn the_log_commits_synced_and_the_projection_unsynced() {
    let Some(url) = common::url_or_skip("the synced commits") else {
        return;
    };
    let rows = written(&url, &url).await;
    let want = [("log", "on"), ("records", "off")].map(|(t, s)| (t.to_owned(), s.to_owned()));
    assert_eq!(seen(&rows), want, "{rows:?}");
}

/// A session whose default is `off`, as a role, a database or the server
/// may set it, appends synced all the same: the append sets its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_defaulting_to_unsynced_still_appends_synced() {
    let Some(url) = common::url_or_skip("the synced commits") else {
        return;
    };
    let joint = if url.contains('?') { '&' } else { '?' };
    let unsynced = format!("{url}{joint}options=-c%20synchronous_commit%3Doff");
    let rows = written(&url, &unsynced).await;
    assert!(
        rows.iter().all(|r| r.2 == "off"),
        "the session's default: {rows:?}"
    );
    let want = [("log", "on"), ("records", "off")].map(|(t, s)| (t.to_owned(), s.to_owned()));
    assert_eq!(seen(&rows), want, "{rows:?}");
}

/// What a crash can take is a window's fold whose commit was not flushed,
/// never its append: the log holds the window and the projection does not.
/// The lost fold is made here as a refused one, which leaves the same
/// state, and a writer's open folds the window again from the log.
#[tokio::test(flavor = "multi_thread")]
async fn a_fold_lost_after_its_append_is_folded_again_by_a_writers_open() {
    let Some(url) = common::url_or_skip("the synced commits") else {
        return;
    };
    let schema = common::fresh_schema();
    let (store, owner) = lake(&url, &schema).await;
    put(store.as_ref(), &owner, "a").await.unwrap();
    let folded = store.head().await.unwrap();
    let refuse = format!(
        "CREATE FUNCTION \"{schema}\".lose() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'the fold is lost'; END $$; \
         CREATE TRIGGER lose BEFORE INSERT ON \"{schema}\".records FOR EACH ROW \
         WHEN (NEW.key = 'b') EXECUTE FUNCTION \"{schema}\".lose();"
    );
    run(&url, &refuse, None);
    let lost = put(store.as_ref(), &owner, "b")
        .await
        .unwrap_err()
        .to_string();
    assert!(lost.contains("the fold is lost"), "{lost}");
    assert_eq!(
        store.head().await.unwrap(),
        folded,
        "the projection is behind its log"
    );
    store.close().await.unwrap();
    run(
        &url,
        &format!("DROP TRIGGER lose ON \"{schema}\".records"),
        None,
    );

    let (store, owner) = lake(&url, &schema).await;
    let get = Get {
        key: "b".into(),
        with_meta: false,
        at: ReadAt::default(),
        params: Default::default(),
    };
    let cat = store.catalog(WriteContext::default());
    let b = cat.get(&owner, &"entry".into(), get).await;
    assert_eq!(
        b.expect("b is folded").data.fields.get("note"),
        Some(&json!("b"))
    );
    store.close().await.unwrap();
    common::drop_schema(&url, &schema);
}

//! A position `head()` answers is readable at once (lakelands#105).
//!
//! While a writer appends, a reader asks the lake for its head and reads
//! as of it. The store publishes a new head only after the append's fold
//! has committed, so the engine never refuses a read as of a head the
//! lake just named. On this engine the projection refuses positions past
//! what it has folded, which is what made the order visible.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lakelands_api::engine::OpenMode;
use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, TypeDecl};
use lakelands_api::read::{Find, ReadAt};
use lakelands_api::seam::Seams;
use lakelands_api::write::Declaration;
use lakelands_api::{
    Error, Grants, LakeStore, Location, Principal, PrincipalKind, Refusal, WriteContext,
};
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

fn owner() -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "heads".into(),
        grants: Grants::owner(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_as_of_the_head_is_never_refused_while_a_writer_appends() {
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("heads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let seams = Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    };
    let store: Arc<dyn LakeStore> =
        lakelands_store::open(Location::sqlite("heads", &dir), seams, OpenMode::Create)
            .await
            .unwrap();
    let cat = store.catalog(WriteContext::default());
    let note = FieldDecl {
        annotations: Default::default(),
        name: "note".into(),
        ty: FieldType::kind(StorageKind::Text),
        required: false,
        authority: Default::default(),
        derived: None,
        function: None,
        computed_at: None,
        depends_on: vec![],
        on_input_change: Default::default(),
        initial: None,
        sensitive: false,
        searchable: false,
        erasable: false,
        description: None,
        label: None,
        embed: None,
    };
    cat.declare(
        &owner(),
        Declaration::Type(TypeDecl {
            annotations: Default::default(),
            name: "entry".into(),
            key: vec![],
            fields: vec![note],
            relations: vec![],
            view: None,
            valid_from: None,
            valid_to: None,
            field_meta: true,
            insert_only: false,
            language: None,
            description: None,
            label: None,
        }),
    )
    .await
    .unwrap();

    let done = Arc::new(AtomicBool::new(false));
    let writer = {
        let (cat, done) = (cat.clone(), done.clone());
        tokio::spawn(async move {
            for i in 0..300 {
                cat.upsert(
                    &owner(),
                    &"entry".into(),
                    &format!("e{i}").into(),
                    [("note".into(), json!(format!("note {i}")))].into(),
                    Default::default(),
                )
                .await
                .unwrap();
            }
            done.store(true, Ordering::Release);
        })
    };
    let mut reads = 0u32;
    let mut refused = Vec::new();
    while !done.load(Ordering::Acquire) {
        let head = store.head().await.unwrap();
        let find = Find {
            at: ReadAt {
                as_of: Some(head),
                valid_at: None,
            },
            limit: Some(1),
            ..Default::default()
        };
        match cat.find(&owner(), &"entry".into(), find).await {
            Ok(_) => {}
            Err(Error::Refused {
                reason: Refusal::OutOfRange { head: named },
                ..
            }) => refused.push((head, named)),
            Err(e) => panic!("reading as of {head}: {e}"),
        }
        reads += 1;
    }
    writer.await.unwrap();
    assert!(reads > 0, "the reader never read while the writer wrote");
    assert!(
        refused.is_empty(),
        "{} of {reads} reads as of the head were refused as out of range; head() answered, \
         then the refusal named, for example: {:?}",
        refused.len(),
        &refused[..refused.len().min(5)]
    );
}

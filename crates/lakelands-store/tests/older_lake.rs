//! A writer's open declares each system type the lake does not hold yet
//! (`docs/seams.md`, `SystemVocabulary`): it appends its lease and the
//! declaration, and nothing else. So a lake gains a system type a later
//! build adds, the next time a writer opens it.
//!
//! The contract cannot make a lake that lacks one, since every lake the
//! store creates declares them all at once, so the test writes one
//! through the engine seam with `lake_created`, the lease and
//! `sys.setting` alone, and `sys.aging_rule` is the type it gains. It
//! runs on the memory engine, the store's one engine among its test
//! dependencies.

use std::sync::Arc;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::event::{Digest, EventBody, LOG_FORMAT_VERSION};
use lakelands_api::seam::{AGING_RULE_TYPE, SETTING_TYPE, Seams, setting_type};
use lakelands_api::write::Declaration;
use lakelands_api::{
    Grants, LakeStore, Location, Position, Principal, PrincipalKind, ReadAt, Timestamp,
    WriteContext,
};
use lakelands_engine_mem::MemoryOpener;
use lakelands_store::log::{Draft, seal};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lake_that_lacks_a_system_type_gains_it_on_its_next_open_for_writing() {
    let opener = MemoryOpener::new();
    let lake = "made-before-aging";
    let location = Location::memory(lake);

    // The lake as the store made it before `sys.aging_rule` existed.
    {
        let engine = opener
            .open(&location, OpenMode::Create)
            .expect("the engine creates the lake");
        let lease = engine.take_lease("an older writer").expect("the lease");
        let now = Timestamp(1_790_000_000_000_000);
        let setting = setting_type();
        let hash = lakelands_core::shape::schema_hash(&setting).expect("a schema hash");
        let drafts = vec![
            Draft::system(
                now,
                EventBody::LakeCreated {
                    format: LOG_FORMAT_VERSION,
                },
            ),
            Draft::system(
                now,
                EventBody::LeaseTaken {
                    fence: lease.fence,
                    holder: lease.holder.clone(),
                },
            ),
            Draft::system(
                now,
                EventBody::Declared {
                    declaration: Declaration::Type(setting),
                    schema_hash: Some(hash),
                    version: 1,
                    with_the_set: false,
                },
            ),
        ];
        let mut prev = Digest::zero();
        let mut stored = Vec::new();
        for (i, d) in drafts.into_iter().enumerate() {
            let position = Position(i as u64);
            let fence = if i == 0 { 0 } else { lease.fence };
            let (event, canon) = seal(&location.lake, position, &prev, fence, d);
            prev = event.hash.clone();
            stored.push(canon);
        }
        engine
            .append(&lease, &stored)
            .expect("the older lake's log");
        engine
            .release_lease(&lease)
            .expect("the older writer lets go");
    }

    let store: Arc<dyn LakeStore> = lakelands_store::open(
        location,
        Seams {
            engines: vec![Arc::new(opener)],
            ..Seams::default()
        },
        OpenMode::Write,
    )
    .await
    .expect("the store opens the older lake for writing");
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: lake.into(),
        grants: Grants::owner(),
    };
    let described = store
        .catalog(WriteContext::default())
        .describe(&owner, ReadAt::default())
        .await
        .expect("describing the lake")
        .data;
    let names: Vec<&str> = described.types.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&AGING_RULE_TYPE) && names.contains(&SETTING_TYPE),
        "a writer's open declares `{AGING_RULE_TYPE}` in a lake made before it; the lake \
         holds {names:?}"
    );

    // One append after the older log: this writer's lease and the type.
    let log = store
        .feed()
        .poll(Position(3), &Default::default(), 100)
        .await
        .expect("reading the log after the older lake's three events")
        .events;
    let kinds: Vec<&str> = log.iter().map(|e| e.body.kind()).collect();
    assert_eq!(
        kinds,
        ["lease_taken", "declared"],
        "the open appends its lease and the type's declaration, and nothing else"
    );
    assert!(
        matches!(&log[1].body, EventBody::Declared { declaration: Declaration::Type(t), .. }
            if t.name.as_str() == AGING_RULE_TYPE),
        "the declaration is `{AGING_RULE_TYPE}`: {:?}",
        log[1].body
    );
}

/// A lake whose `sys.setting` was declared before it had a `group`: a
/// writer's open declares it again, one version on, and stamps the
/// settings the lake holds in the same append, `store` on a store's
/// setting and none on a host's key (063, extended).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writer_s_open_gives_older_settings_their_group() {
    use lakelands_api::seam::aging_rule_type;
    use lakelands_api::{RecordKey, TypeName, Version};
    let opener = MemoryOpener::new();
    let lake = "made-before-groups";
    let location = Location::memory(lake);
    {
        let engine = opener
            .open(&location, OpenMode::Create)
            .expect("the engine creates the lake");
        let lease = engine.take_lease("an older writer").expect("the lease");
        let now = Timestamp(1_790_000_000_000_000);
        let mut setting = setting_type();
        setting.fields.retain(|f| f.name.as_str() != "group");
        let declared = |t: lakelands_api::ontology::TypeDecl| {
            let hash = lakelands_core::shape::schema_hash(&t).expect("a schema hash");
            Draft::system(
                now,
                EventBody::Declared {
                    declaration: Declaration::Type(t),
                    schema_hash: Some(hash),
                    version: 1,
                    with_the_set: false,
                },
            )
        };
        let held = |key: &str, value: serde_json::Value| {
            Draft::system(
                now,
                EventBody::RecordUpserted {
                    ty: TypeName::from(SETTING_TYPE),
                    key: RecordKey::from(key),
                    version: Version(1),
                    patch: [("key".into(), key.into()), ("value".into(), value)].into(),
                    made: true,
                },
            )
        };
        let drafts = vec![
            Draft::system(
                now,
                EventBody::LakeCreated {
                    format: LOG_FORMAT_VERSION,
                },
            ),
            Draft::system(
                now,
                EventBody::LeaseTaken {
                    fence: lease.fence,
                    holder: lease.holder.clone(),
                },
            ),
            declared(setting),
            declared(aging_rule_type()),
            held("view_kept.v", serde_json::json!(true)),
            held("misc", serde_json::json!(1)),
        ];
        let mut prev = Digest::zero();
        let mut stored = Vec::new();
        for (i, d) in drafts.into_iter().enumerate() {
            let fence = if i == 0 { 0 } else { lease.fence };
            let (event, canon) = seal(&location.lake, Position(i as u64), &prev, fence, d);
            prev = event.hash.clone();
            stored.push(canon);
        }
        engine
            .append(&lease, &stored)
            .expect("the older lake's log");
        engine
            .release_lease(&lease)
            .expect("the older writer lets go");
    }
    let store: Arc<dyn LakeStore> = lakelands_store::open(
        location,
        Seams {
            engines: vec![Arc::new(opener)],
            ..Seams::default()
        },
        OpenMode::Write,
    )
    .await
    .expect("the store opens the older lake for writing");
    let log = store
        .feed()
        .poll(Position(6), &Default::default(), 100)
        .await
        .expect("reading the log after the older lake's six events")
        .events;
    let said: Vec<String> = (log.iter())
        .map(|e| match &e.body {
            EventBody::Declared {
                declaration: Declaration::Type(t),
                version,
                ..
            } => format!("declared {} {version}", t.name),
            EventBody::RecordUpserted {
                key,
                version,
                patch,
                ..
            } => format!("upserted {key} {} {:?}", version.0, patch.get("group")),
            other => other.kind().to_owned(),
        })
        .collect();
    assert_eq!(
        said,
        [
            "lease_taken".to_owned(),
            format!("declared {SETTING_TYPE} 2"),
            "upserted view_kept.v 2 Some(String(\"store\"))".to_owned(),
        ],
        "the open declares `{SETTING_TYPE}` again at version 2 and stamps the store's \
         setting `store`; the host's key holds no group"
    );
}

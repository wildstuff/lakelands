use std::time::Duration;

use lakelands_api::principal::TypeGrants;
use lakelands_api::{Access, Grants, PrincipalKind, Refusal};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

use super::*;

fn principal(id: &str, grants: Grants) -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: id.into(),
        lake: "waits".into(),
        grants,
    }
}

fn account(field: &str) -> lakelands_api::ontology::TypeDecl {
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    let fields = [text("a"), text(field)];
    serde_json::from_value(json!({"name": "account", "fields": fields})).expect("a type")
}

/// A write is followed through the lake's renames before it waits for the
/// writer. One that waited behind a migration that renames a field came
/// with grants that do not know the rename: it is refused to be sent
/// again, and sent again it is judged by the grant on the old name
/// (`docs/decisions/006-grants-not-roles.md`, extended).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_that_waited_behind_a_rename_is_sent_again() {
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        ..Seams::default()
    };
    let location = Location::memory("waits");
    let lake = Lake::open(&location, seams, OpenMode::Create).expect("the lake opens");
    let lake = Arc::new(lake);
    let store = Store { lake: lake.clone() };
    let cat = store.catalog(WriteContext::default());
    let owner = principal("owner", Grants::owner());
    let (ty, key) = (
        TypeName::from("account"),
        lakelands_api::RecordKey::from("1"),
    );
    let declared = cat
        .declare(&owner, Declaration::Type(account("holder")))
        .await;
    declared.expect("the owner declares `account`");
    let patch = |f: &str| -> lakelands_api::Patch {
        serde_json::from_value(json!({f: "x"})).expect("a patch")
    };
    let wrote = cat
        .upsert(&owner, &ty, &key, patch("a"), Default::default())
        .await;
    wrote.expect("the owner writes the record");
    // It writes every field of `account` but `holder`, which it reads.
    let mut grants = Grants {
        default: Access::Write,
        ..Grants::default()
    };
    let held = TypeGrants {
        default: None,
        fields: [("holder".into(), Access::Read)].into(),
    };
    grants.types.insert(ty.clone(), held);
    let narrow = principal("narrow", grants);

    // A thread holds the writer; the upsert follows its grants and waits;
    // under the writer, `holder` becomes `owner`.
    let (held, holding) = tokio::sync::oneshot::channel();
    let renames = {
        let (lake, owner) = (lake.clone(), owner.clone());
        tokio::task::spawn_blocking(move || {
            let mut w = lake.writer();
            held.send(())
                .expect("the test waits for the writer to be held");
            std::thread::sleep(Duration::from_millis(300));
            let call = Call {
                lake: &lake,
                who: &owner,
                context: WriteContext::default(),
                followed: None,
            };
            let m = Migration {
                to: account("owner"),
                renames: vec![("holder".into(), "owner".into())],
            };
            let view = lake.engine.view(None).expect("a view");
            let now = lake.clock(&mut w);
            let plan = call.preflight_plan(&w, view.as_ref(), &m, Some(now));
            let drafts = call.migration_drafts(&w, m, plan.expect("the plan"), now, 0);
            drop(view);
            (lake.append(&mut w, drafts.expect("the drafts"))).expect("the migration is appended");
        })
    };
    holding.await.expect("the writer is held");
    let waiting = {
        let (cat, narrow, ty, key) = (cat.clone(), narrow.clone(), ty.clone(), key.clone());
        let owner_field = patch("owner");
        tokio::spawn(async move {
            (cat.upsert(&narrow, &ty, &key, owner_field, Default::default())).await
        })
    };
    renames.await.expect("the migration ends");

    let answer = waiting.await.expect("the upsert ends");
    assert!(
        matches!(&answer, Err(Error::Unavailable { message }) if message.contains("send it again")),
        "a write that waited behind a rename is refused to be sent again: {answer:?}"
    );
    let again = cat
        .upsert(&narrow, &ty, &key, patch("owner"), Default::default())
        .await;
    assert!(
        matches!(&again, Err(Error::Refused { field: Some(f), reason: Refusal::NotGranted }) if f.as_str() == "owner"),
        "sent again, it is held by the grant on `holder`: {again:?}"
    );
}

use std::time::Duration;

use lakelands_api::event::EventBody;
use lakelands_api::principal::TypeGrants;
use lakelands_api::{Access, Grants, PrincipalKind, Refusal};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

use super::*;
use crate::writes::Judging;

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

/// `who` as its call is seen at the head now, for a call whose head
/// another handle's migration moves between its sight and its writer.
async fn seen_now(lake: &Arc<Lake>, who: &Principal) -> Seen {
    let (lake, who) = (lake.clone(), who.clone());
    (exec::run(move || seen(&lake, who)).await).expect("the call is seen")
}

type Seen = (Principal, Option<usize>, Option<writes::Shown>);

/// A call `seen` before the head moved, decided by `f` at a turn now.
async fn decided_late<T: Send + 'static>(
    lake: &Arc<Lake>,
    (who, followed, shown): Seen,
    f: impl FnOnce(&Call<'_>) -> Result<T> + Send + 'static,
) -> Result<T> {
    exec::write(lake, move |lake| {
        let context = WriteContext::default();
        f(&Call {
            lake,
            who: &who,
            context,
            followed,
            shown,
        })
    })
    .await
}

/// A write is followed through the lake's renames at its turn, before it
/// takes the writer. One that waited behind a migration that renames a
/// field is followed through the rename: it is judged by the grant on the
/// old name (`docs/decisions/006-grants-not-roles.md`, extended). A call
/// followed before the rename, as one is when another handle renames
/// between its sight and its writer, is refused to be sent again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_that_waited_behind_a_rename_is_judged_by_it() {
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
        ..Default::default()
    };
    grants.types.insert(ty.clone(), held);
    let narrow = principal("narrow", grants);
    let before = seen_now(&lake, &narrow).await;

    // A write holds its turn; the upsert waits for its own; under the
    // writer, `holder` becomes `owner`.
    let (held, holding) = tokio::sync::oneshot::channel();
    let renames = {
        let (turns, owner) = (lake.clone(), owner.clone());
        let migrate = move |lake: &crate::exec::Turn<'_>| {
            let mut w = lake.writer();
            held.send(())
                .expect("the test waits for the writer to be held");
            std::thread::sleep(Duration::from_millis(300));
            let call = Call {
                lake,
                who: &owner,
                context: WriteContext::default(),
                followed: None,
                shown: None,
            };
            let m = Migration {
                renames: vec![("holder".into(), "owner".into())],
                ..Migration::to(account("owner"))
            };
            let view = lake.engine.view(None).expect("a view");
            let now = lake.clock(&mut w);
            let act = (Some(now), Judging::Act);
            let plan = call.preflight_plan(&w, view.as_ref(), (&m, None, None), act);
            let drafts = call.migration_drafts(&w, m, plan.expect("the plan"), now, 0);
            drop(view);
            (lake.append(&mut w, drafts.expect("the drafts"))).expect("the migration is appended");
            Ok(())
        };
        tokio::spawn(async move { crate::exec::write(&turns, migrate).await })
    };
    holding.await.expect("the writer is held");
    let waiting = {
        let (cat, narrow, ty, key) = (cat.clone(), narrow.clone(), ty.clone(), key.clone());
        let owner_field = patch("owner");
        tokio::spawn(async move {
            (cat.upsert(&narrow, &ty, &key, owner_field, Default::default())).await
        })
    };
    (renames.await.expect("the migration ends")).expect("it is applied");

    let answer = waiting.await.expect("the upsert ends");
    assert!(
        matches!(&answer, Err(Error::Refused { field: Some(f), reason: Refusal::NotGranted }) if f.as_str() == "owner"),
        "a write that waited behind a rename is held by the grant on `holder`: {answer:?}"
    );
    let late = decided_late(&lake, before, move |c| {
        let write = writes::Write::Upsert(ty, key, patch("owner"));
        c.record(&write, Default::default())
    });
    let late = late.await;
    assert!(
        matches!(&late, Err(Error::Unavailable { situation })
            if **situation == lakelands_api::error::Situation::Moved),
        "a call followed before the rename is refused to be sent again: {late:?}"
    );
}

/// The same behind a relation's rename (lakelands#429): a principal kept
/// from the link type by a default alone links while the relation is
/// renamed. The link waited for its turn and names a relation the lake
/// no longer has; by the new name it is held by the grant on the old link
/// type. One followed before the rename is refused to be sent again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_that_waited_behind_a_relations_rename_is_judged_by_it() {
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        ..Seams::default()
    };
    let lake = Lake::open(&Location::memory("waits"), seams, OpenMode::Create);
    let lake = Arc::new(lake.expect("the lake opens"));
    let store = Store { lake: lake.clone() };
    let cat = store.catalog(WriteContext::default());
    let owner = principal("owner", Grants::owner());
    let doc = |relation: &str| -> lakelands_api::ontology::TypeDecl {
        let text = json!({"name": "t", "type": {"kind": "text"}});
        let cites = json!({"name": relation, "target": "doc", "cardinality": "many_to_many"});
        serde_json::from_value(json!({"name": "doc", "fields": [text], "relations": [cites]}))
            .expect("a type")
    };
    let declared = cat.declare(&owner, Declaration::Type(doc("cites"))).await;
    declared.expect("the owner declares `doc`");
    for key in ["a", "b"] {
        let patch = serde_json::from_value(json!({"t": "x"})).expect("a patch");
        let (ty, key) = (TypeName::from("doc"), lakelands_api::RecordKey::from(key));
        let wrote = cat.upsert(&owner, &ty, &key, patch, Default::default());
        wrote.await.expect("the owner writes the record");
    }
    // It writes everything but the links of `cites`, by a default alone.
    let mut grants = Grants {
        default: Access::Write,
        ..Grants::default()
    };
    let none = TypeGrants {
        default: Some(Access::None),
        ..TypeGrants::default()
    };
    grants.types.insert("sys.link.doc.cites".into(), none);
    let narrow = principal("narrow", grants);
    let before = seen_now(&lake, &narrow).await;
    let (held, holding) = tokio::sync::oneshot::channel();
    let renames = {
        let (turns, owner) = (lake.clone(), owner.clone());
        let migrate = move |lake: &crate::exec::Turn<'_>| {
            let mut w = lake.writer();
            held.send(()).expect("the test waits for the writer");
            std::thread::sleep(Duration::from_millis(300));
            let (context, followed) = (WriteContext::default(), None);
            let call = Call {
                lake,
                who: &owner,
                context,
                followed,
                shown: None,
            };
            let m = Migration {
                relation_renames: vec![("cites".into(), "quotes".into())],
                ..Migration::to(doc("quotes"))
            };
            let view = lake.engine.view(None).expect("a view");
            let now = lake.clock(&mut w);
            let act = (Some(now), Judging::Act);
            let plan = call.preflight_plan(&w, view.as_ref(), (&m, None, None), act);
            let drafts = call.migration_drafts(&w, m, plan.expect("the plan"), now, 0);
            drop(view);
            (lake.append(&mut w, drafts.expect("the drafts"))).expect("the rename is appended");
            Ok(())
        };
        tokio::spawn(async move { crate::exec::write(&turns, migrate).await })
    };
    holding.await.expect("the writer is held");
    let link = |relation: &'static str| {
        let (cat, narrow) = (cat.clone(), narrow.clone());
        async move {
            let (ty, a, b) = ("doc".into(), "a".into(), "b".into());
            let (fields, opts) = (Default::default(), Default::default());
            (cat.link(&narrow, &ty, &a, &relation.into(), &b, fields, opts)).await
        }
    };
    let waiting = tokio::spawn(link("cites"));
    (renames.await.expect("the rename ends")).expect("it is applied");
    let answer = waiting.await.expect("the link ends");
    assert!(
        matches!(
            &answer,
            Err(Error::Refused {
                field: None,
                reason: Refusal::Invalid(said)
            }) if matches!(said.case, lakelands_api::error::Case::NoLinkRelation { .. })
        ),
        "a link that waited behind its relation's rename names a relation gone: {answer:?}"
    );
    let again = link("quotes").await;
    // One kept from every field of a type is answered as if it had none.
    let held = matches!(&again, Err(Error::Refused { field: Some(f), .. }) if f.as_str() == "from");
    assert!(
        held,
        "by the new name, it is held by the grant on the old link type: {again:?}"
    );
    let late = decided_late(&lake, before, |c| {
        let (ty, a, b) = ("doc".into(), "a".into(), "b".into());
        let (fields, opts) = (Default::default(), Default::default());
        c.record(
            &writes::Write::Link(ty, a, "quotes".into(), b, fields),
            opts,
        )
    });
    let late = late.await;
    assert!(
        matches!(&late, Err(Error::Unavailable { situation })
            if **situation == lakelands_api::error::Situation::Moved),
        "a link followed before the rename is refused to be sent again: {late:?}"
    );
}

/// An executor that keeps the records it is handed.
struct Kept(std::sync::Mutex<Vec<Option<lakelands_api::Record>>>);

#[async_trait]
impl lakelands_api::seam::VerbExecutor for Kept {
    fn name(&self) -> &str {
        "kept"
    }

    async fn execute(&self, invocation: &VerbInvocation) -> Result<serde_json::Value> {
        let mut kept = self.0.lock().expect("the executor's records");
        kept.push(invocation.record.clone());
        Ok(json!({}))
    }
}

/// An executor is handed the record as it stood at the position that
/// made its verb due, where the call was judged, not as a later write
/// left it (013, extended).
#[tokio::test]
async fn the_executor_reads_the_record_as_its_call_was_judged() {
    let kept = Arc::new(Kept(Default::default()));
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        executors: vec![kept.clone()],
        ..Seams::default()
    };
    let location = Location::memory("waits");
    let lake = Arc::new(Lake::open(&location, seams, OpenMode::Create).expect("the lake opens"));
    let cat = Store { lake: lake.clone() }.catalog(WriteContext::default());
    let owner = principal("owner", Grants::owner());
    let (ty, key) = (
        TypeName::from("account"),
        lakelands_api::RecordKey::from("1"),
    );
    let declared = cat.declare(&owner, Declaration::Type(account("holder")));
    declared.await.expect("the owner declares `account`");
    let verb = json!({"name": "notify", "on": "account", "risk": "low",
                      "effect": "external", "executor": "kept"});
    let verb = serde_json::from_value(verb).expect("a verb");
    cat.declare(&owner, Declaration::Verb(verb))
        .await
        .expect("the owner declares `notify`");
    let write = |a: &str| serde_json::from_value(json!({"a": a})).expect("a patch");
    let wrote = cat.upsert(&owner, &ty, &key, write("then"), Default::default());
    wrote.await.expect("the owner writes the record");
    let notify = VerbName::from("notify");
    let called = cat.call(
        &owner,
        &notify,
        Some(&key),
        Default::default(),
        Default::default(),
    );
    called.await.expect("the owner calls `notify`");
    let log = lake.read_events(Position::GENESIS, 100).expect("the log");
    let called = log
        .iter()
        .find(|e| matches!(e.body, EventBody::VerbCalled { .. }));
    let called = called.expect("the call is logged").position;
    let rewrote = cat.upsert(&owner, &ty, &key, write("now"), Default::default());
    rewrote.await.expect("the owner writes the record again");
    let ext = External {
        executor: "kept".into(),
        verb: notify,
        ty,
        key: Some(key),
        payload: Default::default(),
        position: called,
    };
    let doors = Doors {
        lake,
        context: WriteContext::default(),
    };
    let _ = doors.execute(ext, MadeDue::Call { keyed: false }).await;
    let kept = kept.0.lock().expect("the executor's records");
    let handed: Vec<_> = (kept.iter().flatten())
        .map(|r| r.fields.get("a").cloned())
        .collect();
    assert_eq!(
        handed,
        [Some(json!("then")), Some(json!("then"))],
        "the executor is handed the record as it stood when `notify` was called"
    );
}

/// A write's references are hidden by the head's types at its turn,
/// before it takes the writer. One that waited behind a migration that
/// hides a type it refers to is judged by the moved types: the reference
/// is a field it may not know of
/// (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
/// extended). One seen before the types moved is refused to be sent
/// again; one the types moved under without hiding more is applied.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_that_waited_while_the_types_moved_is_judged_by_them() {
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        ..Seams::default()
    };
    let lake =
        Lake::open(&Location::memory("waits"), seams, OpenMode::Create).expect("the lake opens");
    let lake = Arc::new(lake);
    let store = Store { lake: lake.clone() };
    let cat = store.catalog(WriteContext::default());
    let owner = principal("owner", Grants::owner());
    fn typed(d: serde_json::Value) -> lakelands_api::ontology::TypeDecl {
        serde_json::from_value(d).expect("a type")
    }
    fn text(n: &str) -> serde_json::Value {
        json!({"name": n, "type": {"kind": "text"}})
    }
    fn payroll(key: &[&str]) -> lakelands_api::ontology::TypeDecl {
        let code = json!({"name": "code", "type": {"kind": "text"}, "required": true});
        typed(json!({"name": "payroll", "key": key, "fields": [code]}))
    }
    let payee = json!({"name": "payee", "type": {"kind": "reference", "target": "payroll"}});
    let ticket = typed(json!({"name": "ticket", "fields": [text("no"), payee]}));
    for t in [payroll(&[]), ticket] {
        cat.declare(&owner, Declaration::Type(t))
            .await
            .expect("the owner declares");
    }
    // It is shown `payroll` by its type-level grant, `code` held at `none`:
    // keyed by `code`, `payroll` is not shown to it (006, extended).
    let mut grants = Grants {
        default: Access::Write,
        ..Grants::default()
    };
    let held = TypeGrants {
        default: Some(Access::Read),
        fields: [("code".into(), Access::None)].into(),
        ..Default::default()
    };
    grants.types.insert("payroll".into(), held);
    let narrow = principal("narrow", grants);
    // `wide` is kept from a type the lake does not hold: the migration
    // hides nothing more from it, so its write that waited is applied.
    let mut grants = Grants {
        default: Access::Write,
        ..Grants::default()
    };
    let none = TypeGrants {
        default: Some(Access::None),
        ..Default::default()
    };
    grants.types.insert("ghost".into(), none);
    let wide = principal("wide", grants);
    let (ty, key) = (
        TypeName::from("ticket"),
        lakelands_api::RecordKey::from("t1"),
    );
    let patch = || -> lakelands_api::Patch {
        serde_json::from_value(json!({"payee": "p1"})).expect("a patch")
    };
    let (before, wide_before) = (seen_now(&lake, &narrow).await, seen_now(&lake, &wide).await);

    let (held, holding) = tokio::sync::oneshot::channel();
    let keyed = {
        let (turns, owner) = (lake.clone(), owner.clone());
        let migrate = move |lake: &crate::exec::Turn<'_>| {
            let mut w = lake.writer();
            held.send(())
                .expect("the test waits for the writer to be held");
            std::thread::sleep(Duration::from_millis(300));
            let call = Call {
                lake,
                who: &owner,
                context: WriteContext::default(),
                followed: None,
                shown: None,
            };
            let m = Migration::to(payroll(&["code"]));
            let view = lake.engine.view(None).expect("a view");
            let now = lake.clock(&mut w);
            let act = (Some(now), Judging::Act);
            let plan = call.preflight_plan(&w, view.as_ref(), (&m, None, None), act);
            let drafts = call.migration_drafts(&w, m, plan.expect("the plan"), now, 0);
            drop(view);
            (lake.append(&mut w, drafts.expect("the drafts"))).expect("the migration is appended");
            Ok(())
        };
        tokio::spawn(async move { crate::exec::write(&turns, migrate).await })
    };
    holding.await.expect("the writer is held");
    let waiting = {
        let (cat, narrow, ty, key) = (cat.clone(), narrow.clone(), ty.clone(), key.clone());
        tokio::spawn(
            async move { (cat.upsert(&narrow, &ty, &key, patch(), Default::default())).await },
        )
    };
    let passing = {
        let (cat, ty) = (cat.clone(), ty.clone());
        let no = serde_json::from_value(json!({"no": "7"})).expect("a patch");
        tokio::spawn(
            async move { (cat.upsert(&wide, &ty, &"t2".into(), no, Default::default())).await },
        )
    };
    (keyed.await.expect("the migration ends")).expect("it is applied");
    let passed = passing.await.expect("the upsert ends");
    assert!(
        passed.is_ok(),
        "a write that waited while types moved that hide nothing from it is applied: {passed:?}"
    );
    let answer = waiting.await.expect("the upsert ends");
    assert!(
        matches!(&answer, Err(Error::Refused { field: Some(f), reason: Refusal::Invalid { .. } }) if f.as_str() == "payee"),
        "a write that waited while its types moved finds `payee` a field it may not know of: {answer:?}"
    );
    let late = {
        let (ty, key) = (ty.clone(), key.clone());
        decided_late(&lake, before, move |c| {
            c.record(&writes::Write::Upsert(ty, key, patch()), Default::default())
        })
    };
    let late = late.await;
    assert!(
        matches!(&late, Err(Error::Unavailable { situation })
            if **situation == lakelands_api::error::Situation::TypesMoved),
        "a call seen before its types moved is refused to be sent again: {late:?}"
    );
    let no = serde_json::from_value(json!({"no": "8"})).expect("a patch");
    let wide_late = decided_late(&lake, wide_before, move |c| {
        let write = writes::Write::Upsert(ty, "t3".into(), no);
        c.record(&write, Default::default())
    });
    let wide_late = wide_late.await;
    assert!(
        wide_late.is_ok(),
        "one seen before types moved that hide nothing more from it is applied: {wide_late:?}"
    );
}

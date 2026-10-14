//! A migration decides again every record whose fields it changes, as the
//! fold leaves it: one whose reference to a moved key is rewritten, and a
//! link whose relation's fields it converts
//! (`docs/decisions/004-the-change-feed-is-the-log.md`, extended).

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind::{self, Boolean, Text};
use lakelands_api::ontology::{RelationDecl, TypeDecl};
use lakelands_api::seam::Seams;
use lakelands_api::write::WriteOpts;
use lakelands_api::{LakeStore, Outcome, Position};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, events_from, field, find_where, log_events, object_set_on, patch, reference,
    type_decl, type_of,
};
use crate::conformance::{Failure, Given, Held, Store};

/// `name` with the required text fields `id` and `code`, keyed by `key`,
/// and `more` fields.
fn keyed(name: &str, key: &str, more: Vec<lakelands_api::ontology::FieldDecl>) -> TypeDecl {
    let mut fields = vec![field("id", Text), field("code", Text)];
    fields.iter_mut().for_each(|f| f.required = true);
    let mut t = type_decl(name, [fields, more].concat());
    t.key = vec![key.into()];
    t
}

/// The membership changes caused by the event at `at`, each as
/// `set type key +` (joined) or `-` (left).
async fn changed(store: &dyn LakeStore, at: Position) -> Result<Vec<String>, Failure> {
    let events = events_from(store, at).await?;
    let caused = events.into_iter().filter_map(|e| match e.body {
        EventBody::ObjectSetChanged {
            set,
            ty,
            key,
            joined,
        } if e.cause == Some(at) => Some(format!(
            "{set} {ty} {key} {}",
            if joined { '+' } else { '-' }
        )),
        _ => None,
    });
    Ok(caused.collect())
}

check!(
    SET_REKEYED_REFERENCES = a_rekey_decides_the_sets_that_read_a_rewritten_reference,
    "004, extended: a re-keying migration appends, caused by its `migrated`, an `object_set_changed` for each record whose membership changes once its references to the moved keys are rewritten: a record of the migrated type under its new key, and a record of another type that holds such a reference, naming that type; a set whose filter reads no rewritten reference gets none, and one that reads the far record across such a reference keeps its members, the far record read under its new key (055, extended, lakelands#567)"
);

async fn a_rekey_decides_the_sets_that_read_a_rewritten_reference(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let person = |key| keyed("person", key, vec![reference("boss", "person")]);
    l.declare(person("id")).await?;
    let task = type_decl("task", vec![field("n", Text), reference("owner", "person")]);
    l.declare(task).await?;
    for (name, of, filter) in [
        ("under_x", "person", "boss = 'X'"),
        ("of_x", "task", "owner = 'X'"),
        ("of_x2", "task", "owner = 'X2'"),
        ("named", "task", "n = 't'"),
        ("coded", "task", "owner.code = 'X2'"),
    ] {
        l.declare_as(object_set_on(name, of, filter), name).await?;
    }
    l.upsert("person", "X", json!({"id": "X", "code": "X2"}))
        .await?;
    l.upsert("person", "A", json!({"id": "A", "code": "A2", "boss": "X"}))
        .await?;
    l.upsert("task", "t1", json!({"n": "t", "owner": "X"}))
        .await?;
    // Keyed by `code`: `X` becomes `X2`, and `A2.boss` and `t1.owner` with it.
    let at = store.head().await.must("the head")?.next();
    l.migrate(person("code")).await?;
    let got = changed(store.as_ref(), at).await?;
    let want = ["under_x person A2 -", "of_x task t1 -", "of_x2 task t1 +"];
    ensure!(
        got == want,
        "re-keying `person` rewrites `boss` and `owner`, so it must change {want:?} and \
         nothing of `named`, nor of `coded`, which reads the person across `owner` under \
         its new key; its append changed {got:?}"
    );
    Ok(())
}

check!(
    SET_OVER_LINKS = a_migration_decides_the_sets_over_its_link_types,
    "004, extended: a migration that converts a relation's field appends, directly after its `migrated` and caused by it, an `object_set_changed` for each link whose membership under a set of the link type changes, and its answer names those sets; a re-key of the owner moves a link's membership with it and changes only the set that reads a rewritten end; after either, and after the lake is opened again and its projection rebuilt, `find` under each set's filter returns exactly the links the log says are in it, and a later write of the link leaves a set under its new key",
    given
);

const LINKS: &str = "sys.link.doc.signed_by";
const SETS: [(&str, &str); 3] = [
    ("word", "ok = 'true'"),
    ("yes", "ok = true"),
    ("of_d1", "from = 'd1'"),
];

/// `doc` keyed by `key`, its relation `signed_by` carrying `ok` of `kind`.
fn doc(key: &str, kind: StorageKind) -> TypeDecl {
    let mut t = keyed("doc", key, vec![]);
    t.relations = vec![RelationDecl {
        fields: vec![field("ok", kind)],
        ..RelationDecl::to("signed_by", "person")
    }];
    t
}

/// Fails unless `find` under each set's filter returns the links the log
/// says are in it: the latest `object_set_changed` of each, a re-keyed
/// link's under its new key.
async fn sets_read_as_logged(store: &dyn LakeStore, when: &str) -> Held {
    let mut held = BTreeMap::new();
    for e in log_events(store).await? {
        match e.body {
            EventBody::ObjectSetChanged {
                set, key, joined, ..
            } => drop(held.insert((set, key.to_string()), joined)),
            EventBody::Migrated { rekeyed, .. } => {
                for r in rekeyed {
                    let (from, to) = (r.from.to_string(), r.to.to_string());
                    let moved: Vec<_> = held.keys().filter(|m| m.1 == from).cloned().collect();
                    for m in moved {
                        let joined = held.remove(&m).unwrap_or(false);
                        held.insert((m.0, to.clone()), joined);
                    }
                }
            }
            _ => {}
        }
    }
    let l = Lake::of(store);
    for (set, filter) in SETS {
        let found = l.all_found(LINKS, find_where(filter)).await?;
        let got: BTreeSet<String> = found.into_iter().map(|r| r.key.to_string()).collect();
        let logged = held.iter().filter(|(m, joined)| m.0 == set && **joined);
        let want: BTreeSet<String> = logged.map(|(m, _)| m.1.clone()).collect();
        ensure!(
            got == want,
            "{when}, `find` under `{set}`'s filter returns {got:?}; the log says it holds {want:?}"
        );
    }
    Ok(())
}

/// Links the doc `from` to `p1`, or writes that link again, with `ok`.
async fn link(l: &Lake, from: &str, ok: serde_json::Value) -> Result<Outcome, Failure> {
    let (doc, by, from, to) = ("doc".into(), "signed_by".into(), from.into(), "p1".into());
    let (ok, opts) = (patch(json!({"ok": ok})), WriteOpts::default());
    let linked = (l.cat).link(&l.own, &doc, &from, &by, &to, ok, opts);
    linked.await.must("linking the doc to `p1`")
}

async fn a_migration_decides_the_sets_over_its_link_types(store: Store, given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_of("person", &[("name", Text)])).await?;
    l.declare(doc("id", Text)).await?;
    for (name, filter) in SETS {
        l.declare_as(object_set_on(name, LINKS, filter), name)
            .await?;
    }
    l.upsert("person", "p1", json!({"name": "p"})).await?;
    l.upsert("doc", "d1", json!({"id": "d1", "code": "D1"}))
        .await?;
    link(&l, "d1", json!("true")).await?;
    // `ok` becomes a boolean: the link leaves `word` and joins `yes`.
    let at = store.head().await.must("the head")?.next();
    let answer = l.migrate(doc("id", Boolean)).await?;
    let got = changed(store.as_ref(), at).await?;
    let want = [
        format!("word {LINKS} d1:p1 -"),
        format!("yes {LINKS} d1:p1 +"),
    ];
    let named = matches!(&answer, Outcome::Applied { set_off, .. } if set_off.joined == ["yes"]
        && set_off.left == ["word"] && set_off.through.is_some());
    ensure!(
        got == want && named,
        "converting the relation's `ok` must change {want:?} and answer them; its append \
         changed {got:?} and it answered {answer:?}"
    );
    sets_read_as_logged(store.as_ref(), "after the conversion").await?;
    // The owner is keyed by `code`: the link becomes `D1:p1` and stays in
    // `yes`; its end is rewritten, so it leaves `of_d1`.
    let at = store.head().await.must("the head")?.next();
    l.migrate(doc("code", Boolean)).await?;
    let got = changed(store.as_ref(), at).await?;
    let want = [format!("of_d1 {LINKS} D1:p1 -")];
    ensure!(
        got == want,
        "re-keying `doc` must change {want:?} and move the rest with the link; its append \
         changed {got:?}"
    );
    sets_read_as_logged(store.as_ref(), "after the re-key").await?;
    let store = (given.reopen)(store, Seams::default())
        .await
        .map_err(|e| Failure::new(format!("opening the lake again: {e}")))?;
    // A rebuild folds the link's join and its move in one page.
    store.rebuild().await.must("rebuilding the projection")?;
    sets_read_as_logged(store.as_ref(), "after the lake is opened again and rebuilt").await?;
    // The rebuilt index holds the link in `yes` under its new key.
    let at = store.head().await.must("the head")?.next();
    link(&Lake::of(store.as_ref()), "D1", json!(false)).await?;
    let (got, want) = (
        changed(store.as_ref(), at).await?,
        [format!("yes {LINKS} D1:p1 -")],
    );
    ensure!(
        got == want,
        "the link, re-keyed to `D1:p1`, leaves `yes` under its new key: {want:?}; its write \
         changed {got:?}"
    );
    Ok(())
}

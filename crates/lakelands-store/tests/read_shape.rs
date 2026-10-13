//! A read shares what the lake declares at its position (lakelands#557):
//! the ontology, the declarations, the settings and the renames are built
//! once for a range of positions over which nothing was declared, renamed
//! or set, and every read in that range is answered as one that built them
//! itself.
//!
//! The rows run through the doors on the memory engine, behind the wrapper
//! (`probed`) that counts how often a view is asked for the declarations
//! in force, which a build asks once, and for the lake's renames. Each
//! count is taken after the write it follows, so it counts reads alone.

mod probed;

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::principal::TypeGrants;
use lakelands_api::read::{Find, Found};
use lakelands_api::seam::Seams;
use lakelands_api::write::Migration;
use lakelands_api::{
    Access, Error, Grants, LakeStore, Location, Position, PrincipalKind, TypeName,
};
use lakelands_core::conformance::fixture::{
    Lake, computed_by, field, field_access, find_where, principal, read, read_at, reads_all,
    restrict, source_record, sourced, type_decl, type_of, value,
};
use lakelands_engine_mem::MemoryOpener;
use probed::{Probe, ProbedOpener};
use serde_json::json;

/// The opener of `probe`'s memory lakes: two opens of one name through it
/// are two handles on one lake.
fn opener(probe: &Arc<Probe>) -> Arc<dyn EngineOpener> {
    Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))
}

/// A handle on the lake `name` of `opener`.
async fn open(opener: &Arc<dyn EngineOpener>, name: &str, mode: OpenMode) -> Arc<dyn LakeStore> {
    let seams = Seams {
        engines: vec![opener.clone()],
        ..Seams::default()
    };
    lakelands_store::open(Location::memory(name), seams, mode)
        .await
        .expect("a handle on the lake")
}

/// A new lake on the probed memory engine, with `t`: its text `a` and its
/// number `n`.
async fn lake(name: &str) -> (Arc<dyn LakeStore>, Arc<Probe>) {
    let probe = Arc::new(Probe::default());
    let store = open(&opener(&probe), name, OpenMode::Create).await;
    let l = Lake::of(store.as_ref());
    l.declare(type_of("t", &[("a", Text), ("n", Number)]))
        .await
        .unwrap();
    (store, probe)
}

/// How many shapes were built.
fn builds(probe: &Probe) -> usize {
    probe.declarations.load(Ordering::SeqCst)
}

/// Whether the owner is shown the type `ty` at `at`.
async fn shown(l: &Lake, ty: &str, at: Option<Position>) -> bool {
    match l.cat.describe_type(&l.own, &ty.into(), read_at(at)).await {
        Ok(_) => true,
        Err(Error::NotFound { .. }) => false,
        Err(e) => panic!("describing `{ty}`: {e}"),
    }
}

/// The keys of `t` the owner finds where `filter` holds.
async fn keys(l: &Lake, filter: &str) -> Vec<String> {
    let found = l.all_found("t", find_where(filter)).await.unwrap();
    found.iter().map(|r| r.key.to_string()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_at_the_head_build_the_shape_once() {
    let (store, probe) = lake("once").await;
    let l = Lake::of(store.as_ref());
    l.upsert("t", "k", json!({"a": "x", "n": 1})).await.unwrap();
    let was = builds(&probe);
    for i in 0..1000 {
        match i % 3 {
            0 => drop(l.describe_type("t").await.unwrap()),
            1 => drop(l.read("t", "k").await.unwrap()),
            _ => drop(l.all_found("t", Find::default()).await.unwrap()),
        }
    }
    assert_eq!(
        builds(&probe),
        was + 1,
        "1 000 reads at the head build once"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_ends_a_range_is_seen_by_the_next_read() {
    let (store, probe) = lake("ends").await;
    let l = Lake::of(store.as_ref());
    let f = |e: &str| {
        json!({"declare": "function", "name": "f", "backing": "expr", "expr": e,
               "inputs": [{"name": "n", "type": {"kind": "number"}}]})
    };
    l.declare_json(f("n * 2")).await.unwrap();
    let twice = computed_by("twice", Number, "f", &["n"]);
    l.declare(type_decl("doc", vec![field("n", Number), twice]))
        .await
        .unwrap();
    l.upsert("doc", "d", json!({"n": 2})).await.unwrap();
    l.upsert("t", "k", json!({"a": "x", "n": 10}))
        .await
        .unwrap();
    let twice = async || {
        let r = l.read("doc", "d").await.unwrap();
        value(&r, "twice").and_then(|v| v.as_f64())
    };
    assert_eq!(twice().await, Some(4.0));
    assert!(keys(&l, "n > setting('floor')").await.is_empty());
    // Each change, then two reads: the first builds once more and sees it.
    let was = {
        l.declare(type_of("u", &[("a", Text)])).await.unwrap();
        builds(&probe)
    };
    assert!(shown(&l, "u", None).await, "a type declared");
    assert!(shown(&l, "u", None).await);
    assert_eq!(builds(&probe), was + 1, "a type declared: one build");

    let was = {
        l.migrate(type_of("t", &[("a", Text), ("n", Number), ("b", Text)]))
            .await
            .unwrap();
        builds(&probe)
    };
    let fields = |d: lakelands_api::describe::TypeDescription| {
        d.fields.iter().any(|f| f.name.as_str() == "b")
    };
    assert!(fields(l.describe_type("t").await.unwrap()), "a migration");
    assert!(fields(l.describe_type("t").await.unwrap()));
    assert_eq!(builds(&probe), was + 1, "a migration: one build");

    let was = {
        l.declare_json(f("n * 3")).await.unwrap();
        builds(&probe)
    };
    assert_eq!(twice().await, Some(6.0), "a function declared again");
    assert_eq!(twice().await, Some(6.0));
    assert_eq!(
        builds(&probe),
        was + 1,
        "a function declared again: one build"
    );

    let was = {
        l.set("floor", json!(5)).await.unwrap();
        builds(&probe)
    };
    assert_eq!(
        keys(&l, "n > setting('floor')").await,
        ["k"],
        "a setting put"
    );
    assert_eq!(keys(&l, "n > setting('floor')").await, ["k"]);
    assert_eq!(builds(&probe), was + 1, "a setting put: one build");

    let was = {
        l.retire("sys.setting", "floor").await.unwrap();
        builds(&probe)
    };
    assert!(
        keys(&l, "n > setting('floor')").await.is_empty(),
        "a setting retired"
    );
    assert!(keys(&l, "n > setting('floor')").await.is_empty());
    assert_eq!(builds(&probe), was + 1, "a setting retired: one build");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_a_restriction_a_purge_and_a_kept_state_end_no_range() {
    let (store, probe) = lake("records").await;
    let l = Lake::of(store.as_ref());
    // `book` counts `t`'s records by `a`, kept; `s` is fed from a source.
    let book = json!({"declare": "type", "name": "book", "key": ["a"],
        "fields": [{"name": "a", "type": {"kind": "text"}}, {"name": "c", "type": {"kind": "number"}}],
        "view": {"source": "t", "group_by": ["a"], "measures": [{"name": "c", "expr": "count()"}]}});
    l.declare_json(book).await.unwrap();
    l.declare(type_decl("s", vec![sourced("a", Text)]))
        .await
        .unwrap();
    l.set("view_kept.book", json!(true)).await.unwrap();
    l.upsert("t", "k", json!({"a": "x", "n": 1})).await.unwrap();
    let fed = |key: &str| source_record("s", key, json!({"a": key}));
    l.ingest("feed", vec![fed("f"), fed("g")]).await.unwrap();
    let agent = reads_all(store.as_ref(), "agent");
    let count = async || {
        let r = l.read("book", "x").await.unwrap();
        value(&r, "c").and_then(|v| v.as_f64())
    };
    let found = async |ty: &str| {
        let all = lakelands_core::conformance::fixture::all(l.cat.as_ref(), &agent, ty);
        let keys = all.await.unwrap().into_iter().map(|r| r.key.to_string());
        keys.collect::<Vec<_>>()
    };
    assert_eq!(count().await, Some(1.0));
    assert_eq!(found("s").await, ["f", "g"]);

    l.upsert("t", "m", json!({"a": "x", "n": 3})).await.unwrap();
    let was = builds(&probe);
    assert_eq!(count().await, Some(2.0), "a write, and the kept state");
    assert_eq!(found("t").await, ["k", "m"]);
    assert_eq!(builds(&probe), was, "a write and a kept state: no build");

    restrict(store.as_ref(), "t", "m", "held").await.unwrap();
    let was = builds(&probe);
    assert_eq!(found("t").await, ["k"], "a restriction");
    assert_eq!(builds(&probe), was, "a restriction: no build");

    let purged = store.purge_source(&"feed".into(), None, None).await;
    purged.unwrap();
    let was = builds(&probe);
    assert!(found("s").await.is_empty(), "a purge");
    assert_eq!(builds(&probe), was, "a purge: no build");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_as_of_a_position_is_shown_the_shape_there() {
    let (store, probe) = lake("as-of").await;
    let l = Lake::of(store.as_ref());
    let then = l.upsert("t", "k", json!({"a": "x", "n": 1})).await.unwrap();
    l.declare(type_of("u", &[("a", Text)])).await.unwrap();
    let was = builds(&probe);
    let at = Some(then.position());
    assert!(
        !shown(&l, "u", at).await,
        "declared after the position read"
    );
    assert!(!shown(&l, "u", at).await);
    assert_eq!(
        builds(&probe),
        was + 1,
        "two reads as of one position: one build"
    );
    assert!(shown(&l, "u", None).await, "at the head");
    assert_eq!(builds(&probe), was + 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_walk_of_pages_keeps_its_first_pages_shape() {
    let (store, _probe) = lake("pages").await;
    let l = Lake::of(store.as_ref());
    for k in ["k1", "k2", "k3"] {
        l.upsert("t", k, json!({"a": k, "n": 1})).await.unwrap();
    }
    let (ty, page) = (TypeName::from("t"), Some(1));
    let find = |cursor| Find {
        limit: page,
        cursor,
        ..Find::default()
    };
    let rows = |f: Found| match f {
        Found::Records {
            rows, next_cursor, ..
        } => (rows, next_cursor),
        other => panic!("records: {other:?}"),
    };
    let (first, next) = rows(l.cat.find(&l.own, &ty, find(None)).await.unwrap().data);
    assert!(value(&first[0], "a").is_some());
    // `a` is removed from the type between the pages.
    l.migrate(type_of("t", &[("n", Number)])).await.unwrap();
    let (second, _) = rows(l.cat.find(&l.own, &ty, find(next)).await.unwrap().data);
    assert!(value(&second[0], "a").is_some(), "the walk keeps `a`");
    let now = l.read("t", "k3").await.unwrap();
    assert!(value(&now, "a").is_none(), "a read at the head has no `a`");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shape_is_carried_only_from_where_it_stands() {
    let (store, probe) = lake("stands").await;
    let l = Lake::of(store.as_ref());
    assert!(!shown(&l, "u", None).await);
    // The declaration is in the log; its fold fails.
    *probe.refuse_commit.lock().unwrap() = Some(1);
    let refused = l.try_declare(type_of("u", &[("a", Text)])).await;
    assert!(
        matches!(refused, Err(Error::Unavailable { .. })),
        "{refused:?}"
    );
    assert!(
        !shown(&l, "u", None).await,
        "a failed fold: answered as before"
    );
    // The next write folds the declaration, then appends: its own fold
    // begins past the range of the shape read before.
    l.upsert("t", "k", json!({"a": "x", "n": 1})).await.unwrap();
    assert!(
        shown(&l, "u", None).await,
        "the declaration the write caught up"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_handles_declaration_is_seen_by_the_next_read() {
    let probe = Arc::new(Probe::default());
    let opener = opener(&probe);
    let writer = open(&opener, "two", OpenMode::Create).await;
    let w = Lake::of(writer.as_ref());
    w.declare(type_of("t", &[("a", Text)])).await.unwrap();
    let reader = open(&opener, "two", OpenMode::Read).await;
    let r = Lake::of(reader.as_ref());
    assert!(shown(&r, "t", None).await && !shown(&r, "u", None).await);
    w.declare(type_of("u", &[("a", Text)])).await.unwrap();
    assert!(shown(&r, "u", None).await, "the reading handle's next read");
    assert!(shown(&w, "u", None).await, "the writing handle's");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_beside_declaring_appends_see_one_shape_whole() {
    let (store, _probe) = lake("beside").await;
    let l = Lake::of(store.as_ref());
    let declaring = async {
        let mut at = Vec::new();
        for i in 0..20 {
            let o = l
                .declare(type_of(&format!("u{i:02}"), &[("a", Text)]))
                .await;
            at.push(o.unwrap().position());
        }
        at
    };
    let reading = async {
        let mut seen = Vec::new();
        for _ in 0..200 {
            let d = l.cat.describe(&l.own, read_at(None)).await.unwrap();
            let names = d.data.types.iter().map(|t| t.name.to_string());
            let names: BTreeSet<String> = names.filter(|n| n.starts_with('u')).collect();
            seen.push((d.position, names));
        }
        seen
    };
    let (at, seen) = tokio::join!(declaring, reading);
    for (p, names) in seen {
        let then = at.iter().enumerate().filter(|(_, a)| **a <= p);
        let want: BTreeSet<String> = then.map(|(i, _)| format!("u{i:02}")).collect();
        assert_eq!(names, want, "a read at {p}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_of_a_reader_is_in_a_shape() {
    let (store, probe) = lake("readers").await;
    let l = Lake::of(store.as_ref());
    l.declare(type_of("u", &[("a", Text)])).await.unwrap();
    let only = |id: &str, ty: &str| {
        let mut grants = Grants::default();
        let read = TypeGrants {
            default: Some(Access::Read),
            ..TypeGrants::default()
        };
        grants.types.insert(ty.into(), read);
        principal(store.as_ref(), PrincipalKind::Agent, id, grants)
    };
    let (x, y) = (only("x", "t"), only("y", "u"));
    let names = async |who| {
        let d = l.cat.describe(who, read_at(None)).await.unwrap().data;
        let names = d.types.iter().map(|t| t.name.to_string());
        names.filter(|n| n == "t" || n == "u").collect::<Vec<_>>()
    };
    let was = builds(&probe);
    for _ in 0..3 {
        assert_eq!(names(&x).await, ["t"], "x is shown `t`");
        assert_eq!(names(&y).await, ["u"], "y is shown `u`");
    }
    assert_eq!(builds(&probe), was + 1, "two readers in turn: one build");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_is_followed_from_the_shapes_list() {
    let (store, probe) = lake("renames").await;
    let l = Lake::of(store.as_ref());
    l.upsert("t", "k", json!({"a": "x", "n": 1})).await.unwrap();
    let agent = field_access(store.as_ref(), "agent", "t", &[("a", Access::None)]);
    let get = async || read(l.cat.as_ref(), &agent, "t", "k").await.unwrap();
    let r = get().await;
    assert!(value(&r, "a").is_none() && value(&r, "n").is_some());
    let m = Migration {
        renames: vec![("a".into(), "b".into())],
        ..Migration::to(type_of("t", &[("b", Text), ("n", Number)]))
    };
    let id = l
        .cat
        .migrate(&l.own, m.clone())
        .await
        .unwrap()
        .data
        .report_id;
    l.cat.migrate_apply(&l.own, m, &id).await.unwrap();
    let (was, asked) = (builds(&probe), probe.renames.load(Ordering::SeqCst));
    for _ in 0..100 {
        let r = get().await;
        assert!(value(&r, "b").is_none(), "the grant on `a` holds for `b`");
    }
    assert_eq!(builds(&probe), was + 1, "one build after the rename");
    let read_again = probe.renames.load(Ordering::SeqCst) - asked;
    assert_eq!(read_again, 1, "the list is read by the build alone");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_shape_read_beside_a_rebuild_is_not_kept() {
    let (store, probe) = lake("rebuilt").await;
    let l = Lake::of(store.as_ref());
    l.upsert("t", "k", json!({"a": "x", "n": 1})).await.unwrap();
    // A head read parks inside its build; a rebuild clears the projection
    // and its first commit is refused, so it stays cleared; the read then
    // reads it.
    let ((parked, inside), (go, wait)) = (mpsc::channel(), mpsc::channel());
    *probe.park.lock().unwrap() = Some((parked, wait));
    let read = tokio::spawn({
        let store = store.clone();
        async move { shown(&Lake::of(store.as_ref()), "t", None).await }
    });
    tokio::task::spawn_blocking(move || inside.recv().unwrap())
        .await
        .unwrap();
    *probe.refuse_commit.lock().unwrap() = Some(1);
    assert!(
        store.rebuild().await.is_err(),
        "the rebuild's commit is refused"
    );
    go.send(()).unwrap();
    let _ = read.await;
    l.upsert("t", "k2", json!({"a": "y", "n": 2}))
        .await
        .unwrap();
    assert!(
        shown(&l, "t", None).await,
        "the head after a plain write shows `t`"
    );
}

//! A description at the head is read from the engine once for the writes
//! beside it (lakelands#1575): a record write moves the head but not what
//! a type is, so the writer answers the next description at its head from
//! the one it read, and reads again only past what changes a description:
//! a declaration, a setting, another principal.
//!
//! The rows run through the doors on the memory engine, behind the wrapper
//! (`probed`) that counts the views asked for: a description read from the
//! engine asks one, one answered from what the writer holds asks none.

mod probed;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use lakelands_api::describe::TypeDescription;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::seam::Seams;
use lakelands_api::{Access, FieldName, LakeStore, Location, Position, Principal, ReadAt, Reply};
use lakelands_core::conformance::fixture::{Lake, field_access, read_at, type_of};
use lakelands_engine_mem::MemoryOpener;
use probed::{Probe, ProbedOpener};
use serde_json::json;

/// How many records are written, each followed by a description.
const WRITES: usize = 50;

/// A handle on the lake `name` of `opener`: two opens of one name through
/// it are two handles on one lake.
async fn open(opener: &Arc<dyn EngineOpener>, name: &str, mode: OpenMode) -> Arc<dyn LakeStore> {
    let seams = Seams {
        engines: vec![opener.clone()],
        ..Seams::default()
    };
    lakelands_store::open(Location::memory(name), seams, mode)
        .await
        .expect("a handle on the lake")
}

/// A new lake with `t`: its text `a` and its number `n`, and the opener of
/// its memory engine.
async fn lake(name: &str) -> (Arc<dyn LakeStore>, Arc<Probe>, Arc<dyn EngineOpener>) {
    let probe = Arc::new(Probe::default());
    let opener: Arc<dyn EngineOpener> = Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()));
    let store = open(&opener, name, OpenMode::Create).await;
    let l = Lake::of(store.as_ref());
    l.declare(type_of("t", &[("a", Text), ("n", Number)]))
        .await
        .unwrap();
    (store, probe, opener)
}

/// `t` as `who` is told it at `at`, and the views that asked.
async fn described(
    l: &Lake,
    probe: &Probe,
    who: &Principal,
    at: ReadAt,
) -> (Reply<TypeDescription>, usize) {
    let was = probe.views.load(Ordering::SeqCst);
    let d = l.cat.describe_type(who, &"t".into(), at).await.unwrap();
    (d, probe.views.load(Ordering::SeqCst) - was)
}

fn fields(t: &Reply<TypeDescription>) -> Vec<&str> {
    t.data.fields.iter().map(|f| f.name.as_str()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn descriptions_beside_writes_read_the_engine_once() {
    let (store, probe, _) = lake("beside").await;
    let l = Lake::of(store.as_ref());
    let (mut views, mut last) = (0, Position::GENESIS);
    for i in 0..WRITES {
        let key = format!("k{i}");
        let wrote = l.upsert("t", &key, json!({"a": "x", "n": i}));
        let wrote = wrote.await.unwrap().position();
        let (d, asked) = described(&l, &probe, &l.own, ReadAt::default()).await;
        views += asked;
        assert!(
            d.position >= wrote,
            "a description is never behind a write before it"
        );
        last = d.position;
    }
    assert_eq!(
        views, 1,
        "{WRITES} descriptions beside {WRITES} writes ask one view"
    );
    let mut lake = 0;
    for _ in 0..3 {
        l.upsert("t", "k0", json!({"a": "y"})).await.unwrap();
        let was = probe.views.load(Ordering::SeqCst);
        let d = l.cat.describe(&l.own, ReadAt::default()).await.unwrap();
        lake += probe.views.load(Ordering::SeqCst) - was;
        assert!(d.position > last && d.data.types.iter().any(|t| t.name.as_str() == "t"));
        last = d.position;
    }
    assert_eq!(
        lake, 1,
        "the lake's descriptions beside writes ask one view"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_changes_a_description_is_told_by_the_next() {
    let (store, probe, _) = lake("changes").await;
    let l = Lake::of(store.as_ref());
    let head = ReadAt::default;
    let own = l.own.clone();
    described(&l, &probe, &own, head()).await;
    // A declaration that adds a field.
    let more = type_of("t", &[("a", Text), ("n", Number), ("b", Text)]);
    let declared = l.declare(more).await.unwrap().position();
    let (d, views) = described(&l, &probe, &own, head()).await;
    assert_eq!(
        (fields(&d), views),
        (vec!["a", "n", "b"], 1),
        "after a declaration"
    );
    let (d, views) = described(&l, &probe, &own, head()).await;
    assert_eq!((fields(&d), views), (vec!["a", "n", "b"], 0), "and again");
    // A setting.
    l.set("sort_index.t", json!(["n"])).await.unwrap();
    let (d, views) = described(&l, &probe, &own, head()).await;
    assert_eq!(
        (d.data.lifecycle.sorted, views),
        (vec!["n".into()], 1),
        "after a setting"
    );
    // Another grant set; then the same grants under the owner's id, another
    // principal, since what it may create is told by its writer; then the
    // first principal again.
    let clerk = field_access(store.as_ref(), "clerk", "t", &[("b", Access::None)]);
    let (d, views) = described(&l, &probe, &clerk, head()).await;
    assert_eq!(
        (fields(&d), views),
        (vec!["a", "n"], 1),
        "another grant set"
    );
    let as_owner = Principal {
        id: own.id.clone(),
        ..clerk.clone()
    };
    let (d, views) = described(&l, &probe, &as_owner, head()).await;
    assert_eq!(
        (fields(&d), views),
        (vec!["a", "n"], 1),
        "its grants on another id"
    );
    let (d, views) = described(&l, &probe, &own, head()).await;
    assert_eq!(
        (fields(&d), views),
        (vec!["a", "n", "b"], 0),
        "the first principal"
    );
    // A grant that admits a set of values is another principal too, and so
    // is one with another set.
    let mut sets = Vec::new();
    for values in [["x", "y"], ["x", "z"], ["x", "y"]] {
        let mut narrow = clerk.clone();
        let t = narrow.grants.types.entry("t".into()).or_default();
        let set = values.map(String::from).into_iter().collect();
        t.only_any.insert("a".into(), set);
        let (d, views) = described(&l, &probe, &narrow, head()).await;
        let told = d.data.may.only_any.get(&FieldName::from("a")).cloned();
        sets.push((
            told.unwrap_or_default().into_iter().collect::<Vec<_>>(),
            views,
        ));
    }
    let set = |v: [&str; 2], n| (v.map(String::from).to_vec(), n);
    let want = [set(["x", "y"], 1), set(["x", "z"], 1), set(["x", "y"], 0)];
    assert_eq!(sets, want, "a grant's sets of values, each told as granted");
    // A description as of a position reads it, every time.
    for _ in 0..2 {
        let (d, views) = described(&l, &probe, &own, read_at(Some(declared))).await;
        assert!(views > 0, "as of a position");
        assert_eq!(fields(&d), vec!["a", "n", "b"]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handle_without_the_lease_reads_every_description() {
    let (writer, probe, opener) = lake("reader").await;
    let w = Lake::of(writer.as_ref());
    let reader = open(&opener, "reader", OpenMode::Read).await;
    let r = Lake::of(reader.as_ref());
    let own = r.own.clone();
    for _ in 0..3 {
        let (d, views) = described(&r, &probe, &own, ReadAt::default()).await;
        assert_eq!((fields(&d), views), (vec!["a", "n"], 1));
    }
    let more = type_of("t", &[("a", Text), ("n", Number), ("b", Text)]);
    w.declare(more).await.unwrap();
    let (d, _) = described(&r, &probe, &own, ReadAt::default()).await;
    assert_eq!(
        fields(&d),
        vec!["a", "n", "b"],
        "the writer's declaration, at once"
    );
}

//! Forgetting a source reads the source's events by the feed index, not
//! the whole log (lakelands#1151): its preflight answers what a walk of
//! the log answers, and never opens another source's event. The engine is
//! the memory engine behind the wrapper of `probed`, which hands one
//! event out damaged, so that a read that opened it would refuse the log,
//! and lets the index decide nothing, so that the store walks the log.

mod probed;

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use lakelands_api::engine::OpenMode;
use lakelands_api::id::KeyId;
use lakelands_api::ontology::{FieldDecl, StorageKind::Text, TypeDecl};
use lakelands_api::seam::{DataKey, KeyStore, Seams};
use lakelands_api::{LakeId, Location, Position, Result, SourceId};
use lakelands_core::conformance::fixture::{Lake, source_record, sourced, type_decl};
use lakelands_engine_mem::MemoryOpener;
use probed::{Probe, ProbedOpener};
use serde_json::json;

/// Keys kept in memory, one version a source.
#[derive(Default)]
struct Keys(Mutex<BTreeMap<String, Option<[u8; 32]>>>);

impl KeyStore for Keys {
    fn create(&self, _: &LakeId, source: &SourceId) -> Result<KeyId> {
        let key = [source.as_str().len() as u8; 32];
        self.0.lock().unwrap().insert(source.to_string(), Some(key));
        Ok(KeyId::from("k1".to_owned()))
    }
    fn key(&self, _: &LakeId, source: &SourceId, _: &KeyId) -> Result<Option<DataKey>> {
        let keys = self.0.lock().unwrap();
        Ok(keys.get(source.as_str()).copied().flatten().map(DataKey))
    }
    fn destroy(&self, _: &LakeId, source: &SourceId) -> Result<()> {
        self.0.lock().unwrap().insert(source.to_string(), None);
        Ok(())
    }
    fn ids(&self, _: &LakeId, source: &SourceId) -> Result<Vec<KeyId>> {
        let keys = self.0.lock().unwrap();
        let held = keys.get(source.as_str()).is_some_and(Option::is_some);
        Ok(held
            .then(|| KeyId::from("k1".to_owned()))
            .into_iter()
            .collect())
    }
}

/// `person`, its `name` erasable or not.
fn person(erasable: bool) -> TypeDecl {
    let id = FieldDecl {
        required: true,
        ..sourced("id", Text)
    };
    let name = FieldDecl {
        erasable,
        ..sourced("name", Text)
    };
    TypeDecl {
        key: vec!["id".into()],
        ..type_decl("person", vec![id, name])
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_preflight_reads_the_sources_events_as_a_walk_does() {
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        keys: Some(Arc::new(Keys::default())),
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory("forget"), seams, OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    // crm's `p0` is written before `name` is erasable: plaintext in the log.
    l.declare(person(false)).await.unwrap();
    let p = |key: &str| source_record("person", key, json!({"id": key, "name": key}));
    l.ingest("crm", vec![p("p0")]).await.unwrap();
    l.declare(person(true)).await.unwrap();
    l.ingest("crm", vec![p("p1")]).await.unwrap();
    l.ingest("web", vec![p("p2")]).await.unwrap();
    let web = store.head().await.unwrap();
    l.ingest("crm", vec![p("p3")]).await.unwrap();
    let crm = SourceId::from("crm".to_owned());
    *probe.damaged.lock().unwrap() = Some(web);
    let indexed = store.forget_preflight(&crm).await.unwrap();
    assert_eq!(indexed.plaintext, 1, "crm's `p0` is plaintext: {indexed:?}");
    *probe.damaged.lock().unwrap() = None;
    probe.undecided.store(true, Ordering::SeqCst);
    let walked = store.forget_preflight(&crm).await.unwrap();
    assert_eq!(indexed, walked, "the index answers what the walk answers");
    *probe.damaged.lock().unwrap() = Some(web);
    let opened = store.forget_preflight(&crm).await;
    assert!(
        opened.is_err(),
        "the walk opens web's damaged event, which the index never named: {opened:?}"
    );
}

/// After the act, a preflight and a repeat of the act count the source's
/// recorded outputs by the index as a walk counts them: the source is
/// taken from the event as the log holds it, before its seal is erased.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_the_act_the_index_counts_the_sources_outputs_as_a_walk_does() {
    use lakelands_core::conformance::backing;
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        keys: Some(Arc::new(Keys::default())),
        backings: backing::backings(),
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory("forgot"), seams, OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    l.declare_json(
        json!({"declare": "function", "name": "shout", "backing": backing::KIND,
        "inputs": [{"name": "name", "type": {"kind": "text"}}],
        "config": {"token": "forget_index", "echo": "name"}}),
    )
    .await
    .unwrap();
    let mut t = person(true);
    let loud = lakelands_core::conformance::fixture::computed_by("loud", Text, "shout", &["name"]);
    t.fields.push(loud);
    l.declare(t).await.unwrap();
    let p = |key: &str| source_record("person", key, json!({"id": key, "name": key}));
    l.ingest("crm", vec![p("p1"), p("p2")]).await.unwrap();
    l.ingest("web", vec![p("p3")]).await.unwrap();
    let crm = SourceId::from("crm".to_owned());
    let act = store.forget_source(&crm, None).await.unwrap();
    assert_eq!(act.outputs, 2, "the act counts crm's two outputs: {act:?}");
    let indexed = store.forget_preflight(&crm).await.unwrap();
    let repeat = store.forget_source(&crm, None).await.unwrap();
    probe.undecided.store(true, Ordering::SeqCst);
    let walked = store.forget_preflight(&crm).await.unwrap();
    assert_eq!(
        walked.outputs, 2,
        "the walk counts crm's two outputs: {walked:?}"
    );
    assert_eq!(
        indexed, walked,
        "after the act the index answers what the walk answers"
    );
    assert_eq!(repeat.outputs, 2, "a repeat counts them too: {repeat:?}");
}

/// A source of more events than one read of the index answers is read
/// page by page: another source's event past the first page is never
/// opened, and the preflight finds the plaintext value the source wrote
/// after it, as the walk does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_source_past_one_page_of_the_index_is_read_page_by_page() {
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        keys: Some(Arc::new(Keys::default())),
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory("paged"), seams, OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    let lead = |erasable| TypeDecl {
        name: "lead".into(),
        ..person(erasable)
    };
    l.declare(person(true)).await.unwrap();
    l.declare(lead(false)).await.unwrap();
    let p = |key: String| source_record("person", &key, json!({"id": key, "name": key}));
    l.ingest("crm", (0..1100).map(|i| p(format!("c{i}"))).collect())
        .await
        .unwrap();
    l.ingest("web", vec![p("w".into())]).await.unwrap();
    let web = store.head().await.unwrap();
    // The lead `c1100` is written before its `name` is erasable: plaintext.
    let c = source_record("lead", "c1100", json!({"id": "c1100", "name": "c1100"}));
    l.ingest("crm", vec![c]).await.unwrap();
    l.declare(lead(true)).await.unwrap();
    let crm = SourceId::from("crm".to_owned());
    *probe.damaged.lock().unwrap() = Some(web);
    let indexed = store.forget_preflight(&crm).await.unwrap();
    assert_eq!(
        indexed.plaintext, 1,
        "the lead `c1100` is plaintext: {indexed:?}"
    );
    *probe.damaged.lock().unwrap() = None;
    probe.undecided.store(true, Ordering::SeqCst);
    let walked = store.forget_preflight(&crm).await.unwrap();
    assert_eq!(indexed, walked, "the pages answer what the walk answers");
}

/// A foreign log's ingest whose seal names another source than its
/// envelope, as an import takes it: the index keeps it under its
/// envelope's source, as forgetting's walk reads it, so the plaintext
/// value it holds is found either way and the act is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_foreign_ingest_is_indexed_under_its_envelopes_source() {
    use lakelands_api::canonical;
    use serde_json::Value;
    let typed = |name: &str, erasable: bool| TypeDecl {
        name: name.into(),
        ..person(erasable)
    };
    let keys: Arc<Keys> = Arc::new(Keys::default());
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        keys: Some(keys.clone()),
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory("foreign"), seams, OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(typed("person", false)).await.unwrap();
    l.declare(typed("secret", true)).await.unwrap();
    // web seals a value of `secret`, and crm's last event holds `alice`
    // in plaintext, `name` not erasable yet.
    let s = source_record("secret", "s0", json!({"id": "s0", "name": "s0"}));
    l.ingest("web", vec![s]).await.unwrap();
    let p = source_record("person", "p1", json!({"id": "p1", "name": "alice"}));
    l.ingest("crm", vec![p]).await.unwrap();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("foreign-ingest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    lakelands_core::export::write(store.as_ref(), &dir)
        .await
        .unwrap();
    // The export's last line, crm's ingest, given a seal naming web.
    let mut files: Vec<_> = std::fs::read_dir(dir.join("log"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    let file = files.last().unwrap().clone();
    let text = std::fs::read(&file).unwrap();
    let mut lines: Vec<Vec<u8>> = (text.strip_suffix(b"\n").unwrap())
        .split(|b| *b == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    let last = lines.last_mut().unwrap();
    let mut ev: Value = serde_json::from_slice(last).unwrap();
    assert_eq!(
        (&ev["source"], &ev["body"]["kind"]),
        (&json!("crm"), &json!("record_ingested"))
    );
    ev["sealed"] = json!({"key": "k1", "source": "web"});
    ev.as_object_mut().unwrap().remove("hash");
    ev["hash"] = json!(canonical::sha256_hex(&ev));
    *last = canonical::to_vec(&ev);
    let mut bytes = lines.join(&b'\n');
    bytes.push(b'\n');
    std::fs::write(&file, bytes).unwrap();
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        keys: Some(keys.clone()),
        ..Seams::default()
    };
    let at = Location::memory("foreign");
    lakelands_store::import(at.clone(), seams.clone(), dir.clone())
        .await
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let store = lakelands_store::open(at, seams, OpenMode::Write)
        .await
        .unwrap();
    Lake::of(store.as_ref())
        .declare(typed("person", true))
        .await
        .unwrap();
    let crm = SourceId::from("crm".to_owned());
    let indexed = store.forget_preflight(&crm).await.unwrap();
    probe.undecided.store(true, Ordering::SeqCst);
    let walked = store.forget_preflight(&crm).await.unwrap();
    probe.undecided.store(false, Ordering::SeqCst);
    assert_eq!(
        indexed.plaintext, 1,
        "crm's `alice` is plaintext: {indexed:?}"
    );
    assert_eq!(indexed, walked, "the index answers what the walk answers");
    let act = store.forget_source(&crm, None).await;
    let refused = act
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(
        refused.contains("plaintext"),
        "the act is refused over a plaintext value: {act:?}"
    );
}

/// A fold that meets a forgotten source's sealed values reads the log
/// once, not once a value: knowing the source forgotten, it never looks
/// ahead for an act of removal (065), and a version it looked ahead for
/// in vain it does not look for again. The act and a rebuild after it
/// each read a bounded multiple of the log, however many values the
/// source sealed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forgotten_sources_values_are_folded_without_reading_ahead_for_each() {
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        keys: Some(Arc::new(Keys::default())),
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory("ahead"), seams, OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(person(true)).await.unwrap();
    let p = |key: String| source_record("person", &key, json!({"id": key, "name": key}));
    for (source, n) in [("crm", 400), ("web", 400)] {
        let records = (0..n).map(|i| p(format!("{source}{i}"))).collect();
        l.ingest(source, records).await.unwrap();
    }
    let log = store.head().await.unwrap().0 as usize;
    let crm = SourceId::from("crm".to_owned());
    probe.events_read.store(0, Ordering::SeqCst);
    store.forget_source(&crm, None).await.unwrap();
    let act = probe.events_read.swap(0, Ordering::SeqCst);
    store.rebuild().await.unwrap();
    let rebuild = probe.events_read.load(Ordering::SeqCst);
    assert!(
        act <= 4 * log && rebuild <= 4 * log,
        "of a log of {log} events, the act read {act} and the rebuild {rebuild}"
    );
}

/// A rebuild that decides a set itself, from the first value it erased
/// that the set reads (lakelands#1232), pays what the writes paid to
/// decide it: the log read once, and one scan of the referring type for
/// each far write that moved what the set reads, never one a sealed value
/// nor a walk of the type.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_set_decided_again_scans_once_a_far_write() {
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        keys: Some(Arc::new(Keys::default())),
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory("decided"), seams, OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    let text = json!({"kind": "text"});
    l.declare_json(json!({"declare": "type", "name": "stop", "fields": [
        {"name": "place", "type": text, "authority": "source", "erasable": true},
        {"name": "open", "type": {"kind": "boolean"}}]}))
        .await
        .unwrap();
    l.declare_json(json!({"declare": "type", "name": "call", "fields": [
        {"name": "stop", "type": {"kind": "reference", "target": "stop"}}]}))
        .await
        .unwrap();
    let set = json!({"declare": "object_set", "name": "near", "of": "call",
                     "where": "stop.place = 'Rome' or stop.open"});
    l.declare_json(set).await.unwrap();
    const N: usize = 60;
    let stops = (0..N).map(|i| source_record("stop", &format!("s{i}"), json!({"place": "Rome"})));
    l.ingest("crm", stops.collect()).await.unwrap();
    for i in 0..N {
        l.upsert("stop", &format!("s{i}"), json!({"open": true}))
            .await
            .unwrap();
        l.upsert("call", &format!("c{i}"), json!({"stop": format!("s{i}")}))
            .await
            .unwrap();
    }
    store
        .forget_source(&SourceId::from("crm".to_owned()), None)
        .await
        .unwrap();
    let log = store.head().await.unwrap().0 as usize;
    probe.take();
    probe.events_read.store(0, Ordering::SeqCst);
    store.rebuild().await.unwrap();
    let scans = (probe.take().into_iter())
        .filter(|a| *a == probed::Asked::Scan("call".into()))
        .count();
    let read = probe.events_read.load(Ordering::SeqCst);
    // The far writes after the first erased value: each stop made, then
    // opened, each read back once though the set reads it by two paths.
    assert!(
        (1..=2 * N).contains(&scans) && read <= 2 * log,
        "the rebuild scanned `call` {scans} times for {} far writes, and read {read} events of \
         a log of {log}",
        2 * N
    );
}

/// A poll of the feed decides a reaction again only after an act, by
/// point reads (lakelands#1238): on a lake that never erased anything it
/// reads no record, and after forgetting `crm` the firings over its 40
/// stops, on each stop and across each call's edge, cost at most three
/// reads each, with no scan of a type.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_poll_decides_a_reaction_again_by_point_reads_only_after_an_act() {
    use lakelands_api::seam::FeedFilter;
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        keys: Some(Arc::new(Keys::default())),
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory("overruled"), seams, OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    let place = json!({"name": "place", "type": {"kind": "text"}, "authority": "source",
                       "erasable": true});
    l.declare_json(json!({"declare": "type", "name": "stop", "fields": [place]}))
        .await
        .unwrap();
    l.declare_json(json!({"declare": "type", "name": "call", "fields": [
        {"name": "stop", "type": {"kind": "reference", "target": "stop"}}]}))
        .await
        .unwrap();
    for (name, on, when) in [
        ("in_rome", "stop", "place"),
        ("call_in_rome", "call", "stop.place"),
    ] {
        let t = json!({"declare": "trigger", "name": name, "on": on,
                       "when": format!("{when} = 'Rome'")});
        l.declare_json(t).await.unwrap();
    }
    const N: usize = 40;
    let stops = (0..N).map(|i| source_record("stop", &format!("s{i}"), json!({"place": "Rome"})));
    l.ingest("crm", stops.collect()).await.unwrap();
    for i in 0..N {
        l.upsert("call", &format!("c{i}"), json!({"stop": format!("s{i}")}))
            .await
            .unwrap();
    }
    let fired = FeedFilter {
        kinds: vec!["trigger_fired".into()],
        ..FeedFilter::default()
    };
    let poll = async || {
        probe.take();
        probe.gets.lock().unwrap().clear();
        let b = store
            .feed()
            .poll(Position::GENESIS, &fired, 4096)
            .await
            .unwrap();
        let gets: usize = probe.gets.lock().unwrap().values().sum();
        let scans = probe
            .take()
            .into_iter()
            .filter(|a| {
                matches!(a, probed::Asked::Scan(t)
            if t.as_str() == "stop" || t.as_str() == "call")
            })
            .count();
        (b.events.len(), b.overruled.len(), gets, scans)
    };
    assert_eq!(poll().await, (2 * N, 0, 0, 0), "no act: nothing read");
    store
        .forget_source(&SourceId::from("crm".to_owned()), None)
        .await
        .unwrap();
    // The act's retirement of each stop fires `in_rome` again.
    let (events, overruled, gets, scans) = poll().await;
    assert!(
        (events, overruled, scans) == (3 * N, 3 * N, 0) && gets <= 3 * events,
        "after the act, {overruled} of {events} firings overruled, by {gets} reads and {scans} \
         scans"
    );
    // While a rebuild has refolded only part of the log, a poll stops at
    // the first reaction it cannot decide yet, and tells nothing past it.
    let all = store.feed().poll(Position::GENESIS, &fired, 4096).await;
    let ninth = all.unwrap().events[9].position;
    *probe.refolded.lock().unwrap() = Some(Position(ninth.0 - 1));
    let b = store.feed().poll(Position::GENESIS, &fired, 4096).await;
    *probe.refolded.lock().unwrap() = None;
    let b = b.unwrap();
    assert!(
        b.events.len() == 9 && b.next == ninth && b.overruled.len() == 9,
        "the poll stops at {ninth}: {b:?}"
    );
}

//! What a kept view costs where it costs (lakelands#575): a group whose
//! extreme lost its holder is read again in the fold of that event, over
//! 8 192 records at most, and past them it is given up, by the event and
//! so alike on every path that folds it; an append reads the state of the
//! views that read what it wrote and of no other, and a lake that keeps
//! no view reads none. Counted at the memory engine behind `probed`.

mod probed;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use lakelands_api::engine::OpenMode;
use lakelands_api::read::{Find, Found};
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::Seams;
use lakelands_api::{Access, Grants, Principal, PrincipalKind};
use lakelands_api::{LakeStore, Location, SourceRecord};
use lakelands_core::conformance::fixture::{
    Lake, describe_type, patch, principal, restrict, source_record,
};
use lakelands_engine_mem::MemoryOpener;
use probed::{Probe, ProbedOpener};
use serde_json::{Value, json};

/// A lake with `reading`, a source's, the view `peak` of each site's
/// greatest value and its twin, which no setting names, and `other`.
struct Peaks {
    store: Arc<dyn LakeStore>,
    probe: Arc<Probe>,
    l: Lake,
}

impl Peaks {
    async fn new(name: &str) -> Peaks {
        let probe = Arc::new(Probe::default());
        let seams = Seams {
            engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
            ..Seams::default()
        };
        let store = lakelands_store::open(Location::memory(name), seams, OpenMode::Create)
            .await
            .expect("a new lake");
        let l = Lake::of(store.as_ref());
        let field = |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}});
        let from =
            |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}, "authority": "source"});
        let peak = |name: &str| {
            json!({"declare": "type", "name": name, "key": ["site"],
                "fields": [field("site", "text"), field("peak", "decimal"), field("n", "number")],
                "view": {"source": "reading", "group_by": ["site"],
                    "measures": [{"name": "peak", "expr": "max(value)"},
                                 {"name": "n", "expr": "count()"}]}})
        };
        for d in [
            json!({"declare": "type", "name": "reading",
                "fields": [from("site", "text"), from("value", "decimal")]}),
            json!({"declare": "type", "name": "other", "fields": [field("x", "text")]}),
            peak("peak"),
            peak("peak_twin"),
        ] {
            l.declare_json(d).await.expect("declaring");
        }
        Peaks { store, probe, l }
    }

    /// `n` readings of `site` at the value 1, keyed `<site>-<from>` on.
    async fn feed(&self, site: &str, from: usize, n: usize) {
        let reading = |i: usize| -> SourceRecord {
            let key = format!("{site}-{i:05}");
            source_record("reading", &key, json!({"site": site, "value": "1"}))
        };
        let records = (from..from + n).map(reading).collect();
        let fed = self.l.ingest("meter", records).await;
        fed.expect("feeding the source");
    }

    async fn put(&self, ty: &str, key: &str, fields: Value) {
        let (ty, key) = (ty.into(), key.into());
        let wrote = (self.l.cat).upsert(&self.l.own, &ty, &key, patch(fields), Default::default());
        wrote.await.expect("a write");
    }

    /// The holder of a site's peak, under the key `<site>-top`.
    async fn top(&self, site: &str) {
        self.at(&format!("{site}-top"), site, "9").await;
    }

    /// The reading `key` of `site` at `value`, fed.
    async fn at(&self, key: &str, site: &str, value: &str) {
        let fed = source_record("reading", key, json!({"site": site, "value": value}));
        self.l.ingest("meter", vec![fed]).await.expect("a reading");
    }

    /// The fields of the kept record of `view` whose group is `site`, the
    /// site written `@`, as the engine was last handed it; `ended` for none.
    fn kept(&self, view: &str, site: &str) -> String {
        let kept = self.probe.kept.lock().unwrap();
        let name = lakelands_api::seam::kept_name(&view.into());
        let of = |r: &&StoredRecord| r.ty == name && r.fields.get("site") == Some(&site.into());
        match kept.values().find(of).cloned() {
            Some(mut r) => {
                r.fields.remove("site");
                json!([r.fields, r.restricted])
                    .to_string()
                    .replace(site, "@")
            }
            None => "ended".to_owned(),
        }
    }

    async fn keep(&self, on: bool) {
        self.keep_of("peak", on).await;
    }

    async fn keep_of(&self, view: &str, on: bool) {
        let key = format!("view_kept.{view}");
        let kept = json!({"key": key, "value": on});
        self.put("sys.setting", &key, kept).await;
    }

    /// What `describe_type` says of whether `peak` is kept, and what a
    /// read of it answers beside its twin's: `same` or the two.
    async fn says(&self) -> (String, String) {
        self.says_of("peak").await
    }

    /// [`Self::says`] of the view `view` and its twin.
    async fn says_of(&self, view: &str) -> (String, String) {
        self.says_to(&self.l.own, view).await
    }

    /// [`Self::says_of`], to `who`.
    async fn says_to(&self, who: &Principal, view: &str) -> (String, String) {
        let described = describe_type(self.l.cat.as_ref(), who, view).await;
        let described = described.expect("describing");
        let read = |ty: String| async move {
            let ty = ty.as_str().into();
            let found = self.l.cat.find(who, &ty, Find::default()).await;
            match found.expect("reading the view").data {
                Found::Records { rows, .. } => {
                    let rows = rows.iter().map(|r| json!([r.key, r.fields]).to_string());
                    rows.collect::<Vec<_>>().join(" ")
                }
                other => panic!("records: {other:?}"),
            }
        };
        let (kept, twin) = (
            read(view.to_owned()).await,
            read(format!("{view}_twin")).await,
        );
        let twin = twin.replace("_twin", "");
        let same = match kept == twin {
            true => "same".to_owned(),
            false => format!("{kept} against {twin}"),
        };
        (json!(described.lifecycle.kept).to_string(), same)
    }

    /// The rows of `reading` the engine handed out since the last call.
    fn read(&self) -> usize {
        self.read_of("reading")
    }

    /// The rows of `ty` the engine handed out since the last call.
    fn read_of(&self, ty: &str) -> usize {
        let mut rows = self.probe.rows.lock().unwrap();
        rows.remove(ty).unwrap_or(0)
    }

    /// Writes of a setting no view names until the next event lands `at`
    /// events into a page of a rebuild (1 024 events, from position 0).
    async fn pad(&self, at: u64) {
        loop {
            let head = self.store.head().await.expect("the head").0;
            if (head + 1) % 1024 == at {
                return;
            }
            let pad = json!({"key": "pad", "value": head});
            self.put("sys.setting", "pad", pad).await;
        }
    }
}

const KEPT: &str = r#"{"state":"kept"}"#;
const ONE: &str = r#"{"state":"computed","why":{"because":"not_whole","not_exact":1}}"#;

/// The event that takes a holder reads its group again: a group within
/// the bound is exact after it, and it read the records the view held
/// of that group and no more; a group past the bound, 8 193 live records,
/// is given up in that event. Nothing reads it again: not a write to the
/// stale holder, not a member that beats it and leaves, not a member more,
/// another site or another type; and a make mends it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_group_past_the_bound_is_given_up_in_the_event_that_took_its_holder() {
    let t = Peaks::new("kept-bound").await;
    t.feed("small", 0, 100).await;
    t.top("small").await;
    t.feed("big", 0, 8_192).await;
    t.top("big").await;
    t.keep(true).await;
    assert_eq!(t.says().await, (KEPT.to_owned(), "same".to_owned()));
    t.read();
    t.l.retire("reading", "small-top").await.expect("retiring");
    assert_eq!(
        t.read(),
        101,
        "the small group, read once: its hundred and the one that left"
    );
    assert_eq!(t.says().await, (KEPT.to_owned(), "same".to_owned()));
    t.read();
    t.at("big-top", "big", "5").await;
    assert_eq!(t.read(), 8_193, "the walk ends past the bound");
    assert_eq!(t.says().await, (ONE.to_owned(), "same".to_owned()));
    t.read();
    t.at("big-top", "big", "6").await;
    t.at("big-beat", "big", "10").await;
    t.at("big-beat", "big", "2").await;
    t.feed("big", 9_000, 1).await;
    t.feed("small", 200, 1).await;
    t.put("other", "o", json!({"x": "y"})).await;
    assert_eq!(t.read(), 0, "no later append reads a record of it");
    assert_eq!(t.says().await, (ONE.to_owned(), "same".to_owned()));
    t.keep(false).await;
    t.keep(true).await;
    assert_eq!(t.says().await, (KEPT.to_owned(), "same".to_owned()));
}

/// A group that fits the bound when its holder leaves, 8 192 live
/// records, and grows past it later is exact on every path: after the live appends, after a rebuild
/// whose page holds the event and the growth together, and where the fold
/// was behind and one transaction caught up with both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_fold_a_rebuild_and_a_catch_up_decide_alike() {
    let t = Peaks::new("kept-alike").await;
    t.feed("big", 0, 8_192).await;
    t.top("big").await;
    t.keep(true).await;
    t.pad(5).await;
    t.l.retire("reading", "big-top").await.expect("retiring");
    t.feed("big", 9_000, 300).await;
    let live = t.says().await;
    assert_eq!(live, (KEPT.to_owned(), "same".to_owned()));
    t.store.rebuild().await.expect("rebuilding");
    assert_eq!(t.says().await, live, "rebuilt");
    // The fold of the next holder's retirement is left behind, and the
    // feed's first append folds both: the group is past the bound at that
    // event on every path, and is given up on each.
    t.top("big").await;
    *t.probe.refuse_commit.lock().unwrap() = Some(1);
    let _ = t.l.retire("reading", "big-top").await;
    let refused = t.probe.refuse_commit.lock().unwrap().is_none();
    assert!(refused, "the fold of the retirement was left behind");
    t.feed("big", 10_000, 10).await;
    let caught_up = t.says().await;
    assert_eq!(caught_up, (ONE.to_owned(), "same".to_owned()));
    t.store.rebuild().await.expect("rebuilding");
    assert_eq!(t.says().await, caught_up, "rebuilt after the catch-up");
}

/// A group is read again as a migration earlier in the same transaction
/// leaves its members: a rebuild's page that holds a re-key of the view's
/// source and then the events that take two holders reads the records of
/// the pages before through that step, under the keys it gave them, as
/// the live appends found them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_group_is_read_again_through_a_migration_of_the_same_page() {
    let t = Peaks::new("kept-stepped").await;
    let field = |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}, "required": true});
    let acct = |key: &str| {
        json!({"name": "acct", "key": [key], "fields": [field("code", "text"),
            field("tag", "text"), field("site", "text"), field("n", "decimal")]})
    };
    let spread = |name: &str| {
        json!({"declare": "type", "name": name, "key": ["site"],
            "fields": [{"name": "site", "type": {"kind": "text"}},
                       {"name": "high", "type": {"kind": "decimal"}}],
            "view": {"source": "acct", "group_by": ["site"],
                "measures": [{"name": "high", "expr": "max(n)"}]}})
    };
    let mut declared = acct("code");
    declared["declare"] = json!("type");
    for d in [declared, spread("peak_of"), spread("peak_of_twin")] {
        t.l.declare_json(d).await.expect("declaring");
    }
    for (code, tag, n) in [("k1", "t9", "9"), ("k2", "t2", "5"), ("k3", "t3", "1")] {
        let fields = json!({"tag": tag, "site": "v", "n": n});
        t.put("acct", code, fields).await;
    }
    let kept = json!({"key": "view_kept.peak_of", "value": true});
    t.put("sys.setting", "view_kept.peak_of", kept).await;
    t.pad(5).await;
    let by_tag = serde_json::from_value(acct("tag")).expect("a type");
    t.l.migrate(by_tag).await.expect("re-keying the source");
    // The holder leaves, and then the one the group was read again to.
    t.l.retire("acct", "t9").await.expect("retiring");
    t.l.retire("acct", "t2").await.expect("retiring");
    let live = t.says_of("peak_of").await;
    assert_eq!(live.1, "same");
    assert!(live.0.contains("kept"), "{live:?}");
    t.store.rebuild().await.expect("rebuilding");
    assert_eq!(t.says_of("peak_of").await, live, "rebuilt");
}

/// A group that ends leaves nothing behind for a member that arrives
/// after it in the same transaction: a group given up whose members all
/// leave and a new one that arrives, the last leaving and the arrival in
/// one transaction, in two, and in one page of a rebuild, leave the same
/// record and the same state. An ingest commits a window of 32 events at
/// a time, and a stream in hand fills each: 8 194 events are 256 windows
/// and a last one of two, the last leaving and the arrival.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_group_that_ends_and_returns_starts_from_nothing_on_every_path() {
    let t = Peaks::new("kept-ends").await;
    for site in ["one", "two"] {
        t.feed(site, 0, 8_193).await;
        t.top(site).await;
    }
    t.keep(true).await;
    for site in ["one", "two"] {
        let top = format!("{site}-top");
        t.l.retire("reading", &top).await.expect("retiring");
    }
    for site in ["one", "two"] {
        let reading = |key: String, at: &str| {
            source_record("reading", &key, json!({"site": at, "value": "2"}))
        };
        let away = (0..8_193).map(|i| reading(format!("{site}-{i:05}"), "away"));
        let mut fed: Vec<SourceRecord> = away.collect();
        let new = reading(format!("{site}-new"), site);
        t.pad(5).await;
        if site == "two" {
            t.l.ingest("meter", fed).await.expect("moving away");
            fed = Vec::new();
        }
        fed.push(new);
        let windows = t.l.ingest("meter", fed).await.expect("feeding");
        let last = if site == "one" { 257 } else { 1 };
        assert_eq!(windows.len(), last, "`{site}`: the arrival's transaction");
    }
    let live = (t.says().await, t.kept("peak", "one"), t.kept("peak", "two"));
    assert_eq!(live.0, (KEPT.to_owned(), "same".to_owned()));
    assert_eq!(
        live.1, live.2,
        "the last leaving and the arrival in one transaction and in two"
    );
    t.store.rebuild().await.expect("rebuilding");
    let rebuilt = (t.says().await, t.kept("peak", "one"), t.kept("peak", "two"));
    assert_eq!(
        rebuilt, live,
        "rebuilt, the last leaving and the arrival in one page"
    );
}

/// A group given up on the whole face alone, where its extreme's holder
/// was a restricted record: the reader with the `restricted` grant is
/// answered by the computation, and the one without it, whose face is
/// exact, from the kept records.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_group_given_up_on_the_whole_face_alone_is_kept_for_the_open_one() {
    let t = Peaks::new("kept-whole").await;
    t.feed("big", 0, 8_193).await;
    t.top("big").await;
    let held = restrict(t.store.as_ref(), "reading", "big-top", "held").await;
    held.expect("restricting the holder");
    t.keep(true).await;
    t.l.retire("reading", "big-top").await.expect("retiring");
    let reads = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let plain = principal(t.store.as_ref(), PrincipalKind::Human, "plain", reads);
    let same = "same".to_owned();
    assert_eq!(t.says().await, (ONE.to_owned(), same.clone()));
    assert_eq!(t.says_to(&plain, "peak").await, (KEPT.to_owned(), same));
}

/// A group given up on the whole face alone whose open face's holder
/// leaves next: nothing is read, and the open face is given up too, for
/// good: the record keeps nothing that waits, and a later member finds
/// both faces given up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_group_given_up_on_one_face_gives_the_other_up_unread() {
    let t = Peaks::new("kept-both").await;
    t.feed("big", 0, 8_193).await;
    t.top("big").await;
    t.at("big-open", "big", "8").await;
    let held = restrict(t.store.as_ref(), "reading", "big-top", "held").await;
    held.expect("restricting the holder");
    t.keep(true).await;
    t.l.retire("reading", "big-top").await.expect("retiring");
    let reads = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let plain = principal(t.store.as_ref(), PrincipalKind::Human, "plain", reads);
    let same = "same".to_owned();
    assert_eq!(
        t.says_to(&plain, "peak").await,
        (KEPT.to_owned(), same.clone())
    );
    t.read();
    t.l.retire("reading", "big-open").await.expect("retiring");
    assert_eq!(t.read(), 0, "a group given up is read no more");
    let kept = t.kept("peak", "big");
    assert!(kept.contains("lost") && !kept.contains("waits"), "{kept}");
    assert_eq!(t.says().await, (ONE.to_owned(), same.clone()));
    assert_eq!(
        t.says_to(&plain, "peak").await,
        (ONE.to_owned(), same.clone())
    );
    // Given up for good, not left to wait: a later member finds it so.
    t.feed("big", 9_000, 1).await;
    assert_eq!(t.says().await, (ONE.to_owned(), same.clone()));
    assert_eq!(t.says_to(&plain, "peak").await, (ONE.to_owned(), same));
}

/// The bound is one for a group over all its sources and both lookups: a
/// group of 4 100 records in each of two sources is given up when its
/// holder leaves, though each source's would fit; a view over a view
/// reads every record of the inner view that bears the mark, whatever its
/// group, so an outer group of two members is given up by 8 193 of them;
/// and a walk that passes the bound in the first source ends there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bound_is_one_for_a_group_over_its_sources_and_the_marks() {
    let t = Peaks::new("kept-shared").await;
    let field = |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}});
    let from =
        |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}, "authority": "source"});
    let measure = |n: &str, e: &str| json!({"name": n, "expr": e});
    let view = |name: &str, by: &str, fields: Vec<Value>, view: Value| json!({"declare": "type", "name": name, "key": [by], "fields": fields, "view": view});
    let spare = json!({"declare": "type", "name": "spare",
        "fields": [from("site", "text"), from("value", "decimal")]});
    t.l.declare_json(spare).await.expect("declaring");
    for end in ["", "_twin"] {
        let pair = json!({"source": "reading", "group_by": ["site"],
            "measures": [measure("peak", "max(value)")],
            "merge": [{"source": "spare", "group_by": ["site"],
                       "measures": [measure("spares", "count()")]}]});
        let fields = vec![
            field("site", "text"),
            field("peak", "decimal"),
            field("spares", "number"),
        ];
        let zone = json!({"source": "reading", "group_by": ["site"],
            "measures": [measure("peak", "max(value)"), measure("n", "count()"),
                         measure("band", "if(max(value) > 5, 'high', 'low')")]});
        let zoned = vec![
            field("site", "text"),
            field("peak", "decimal"),
            field("n", "number"),
            field("band", "text"),
        ];
        let bands = json!({"source": format!("zone{end}"), "group_by": ["band"],
            "measures": [measure("high", "max(peak)")]});
        for d in [
            view(&format!("pair{end}"), "site", fields, pair),
            view(&format!("zone{end}"), "site", zoned, zone),
            view(
                &format!("bands{end}"),
                "band",
                vec![field("band", "text"), field("high", "decimal")],
                bands,
            ),
        ] {
            t.l.declare_json(d).await.expect("declaring");
        }
    }
    // Two sources, one bound.
    t.feed("big", 0, 4_100).await;
    t.top("big").await;
    let spares = (0..4_100).map(|i| {
        let key = format!("big-{i:05}");
        source_record("spare", &key, json!({"site": "big", "value": "1"}))
    });
    t.l.ingest("meter", spares.collect())
        .await
        .expect("feeding");
    t.keep_of("pair", true).await;
    assert_eq!(
        t.says_of("pair").await,
        (KEPT.to_owned(), "same".to_owned())
    );
    t.l.retire("reading", "big-top").await.expect("retiring");
    assert_eq!(t.says_of("pair").await, (ONE.to_owned(), "same".to_owned()));
    // An outer group of two, `high`, and 8 193 inner groups that differ.
    t.at("a-top", "a", "9").await;
    t.at("b-top", "b", "7").await;
    let marked = (0..8_193).flat_map(|i| {
        let site = format!("m-{i:05}");
        let at = |end: &str| {
            let key = format!("{site}-{end}");
            source_record("reading", &key, json!({"site": site, "value": "1"}))
        };
        [at("a"), at("b")]
    });
    t.l.ingest("meter", marked.collect())
        .await
        .expect("feeding");
    for i in 0..8_193 {
        let key = format!("m-{i:05}-b");
        let held = restrict(t.store.as_ref(), "reading", key.as_str(), "held").await;
        held.expect("restricting");
    }
    t.keep_of("zone", true).await;
    t.keep_of("bands", true).await;
    assert_eq!(
        t.says_of("bands").await,
        (KEPT.to_owned(), "same".to_owned())
    );
    t.at("a-top", "a", "6").await;
    assert_eq!(
        t.says_of("bands").await,
        (ONE.to_owned(), "same".to_owned())
    );
    // A walk past the bound in the view's own source reads nothing of the
    // merged one.
    t.feed("huge", 0, 8_193).await;
    t.at("huge-top", "huge", "3").await;
    let spares = (0..10).map(|i| {
        let key = format!("huge-{i:05}");
        source_record("spare", &key, json!({"site": "huge", "value": "1"}))
    });
    t.l.ingest("meter", spares.collect())
        .await
        .expect("feeding");
    t.read_of("spare");
    t.l.retire("reading", "huge-top").await.expect("retiring");
    assert_eq!(t.read_of("spare"), 0, "the walk ends at the bound");
    let two = ONE.replace("\"not_exact\":1", "\"not_exact\":2");
    assert_eq!(t.says_of("pair").await, (two, "same".to_owned()));
}

/// A lake that keeps no view reads no view's state, in a read of a view
/// or in an append; a lake that keeps one reads its state in an append
/// that writes a record it reads, once, and in no other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_append_pays_for_the_views_that_read_what_it_wrote() {
    let t = Peaks::new("kept-cost").await;
    let states = || t.probe.states.swap(0, Ordering::SeqCst);
    t.feed("a", 0, 3).await;
    t.put("other", "o", json!({"x": "y"})).await;
    t.says().await;
    assert_eq!(states(), 0, "no view is kept: no state is read");
    t.keep(true).await;
    states();
    t.put("other", "o", json!({"x": "z"})).await;
    assert_eq!(states(), 0, "an append to a type no kept view reads");
    t.feed("a", 10, 1).await;
    assert_eq!(states(), 1, "an append to the view's source");
}

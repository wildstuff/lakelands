//! What a kept view costs where it costs (lakelands#575, lakelands#629):
//! a group whose extreme lost its holder reads its next holder from the
//! extreme's order, an entry or a few, and never its members, whatever
//! their number, and so alike on every path that folds the event; an
//! append reads the state of the views that read what it wrote and of no
//! other, and a lake that keeps no view reads none. Counted at the memory
//! engine behind `probed`.

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

    /// Forget the rows the engine handed out so far.
    fn forget(&self) {
        self.probe.rows.lock().unwrap().clear();
    }

    /// The entries of the order of `view` the engine handed out since the
    /// last call.
    fn entries_read(&self, view: &str) -> usize {
        self.read_of(lakelands_api::seam::order_name(&view.into()).as_str())
    }

    /// The entries of the orders of `view` the engine holds, each its key
    /// and what it holds.
    fn entries(&self, view: &str) -> Vec<String> {
        let name = lakelands_api::seam::order_name(&view.into());
        let orders = self.probe.orders.lock().unwrap();
        let of = orders.iter().filter(|((ty, _), _)| *ty == name);
        of.map(|((_, key), r)| format!("{key} {}", json!(r.fields)))
            .collect()
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

/// A holder that leaves reads one entry of its group's order, small or
/// past any bound the members would set, 8 193 live records: the source is
/// not read, the group stays kept, and so does every later write to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_holder_that_leaves_reads_one_entry_whatever_its_group() {
    let t = Peaks::new("kept-bound").await;
    t.feed("small", 0, 100).await;
    t.top("small").await;
    t.feed("big", 0, 8_193).await;
    t.top("big").await;
    t.keep(true).await;
    assert_eq!(t.says().await, (KEPT.to_owned(), "same".to_owned()));
    t.forget();
    t.l.retire("reading", "small-top").await.expect("retiring");
    assert_eq!(
        (t.read(), t.entries_read("peak")),
        (0, 1),
        "the small group"
    );
    assert_eq!(t.says().await, (KEPT.to_owned(), "same".to_owned()));
    t.forget();
    t.at("big-top", "big", "5").await;
    assert_eq!((t.read(), t.entries_read("peak")), (0, 1), "the big group");
    assert_eq!(t.says().await, (KEPT.to_owned(), "same".to_owned()));
    t.read();
    t.at("big-top", "big", "6").await;
    t.at("big-beat", "big", "10").await;
    t.at("big-beat", "big", "2").await;
    t.feed("big", 9_000, 1).await;
    t.feed("small", 200, 1).await;
    t.put("other", "o", json!({"x": "y"})).await;
    assert_eq!(t.read(), 0, "no later append reads a record of the source");
    assert_eq!(t.says().await, (KEPT.to_owned(), "same".to_owned()));
    let live = t.entries("peak");
    assert_eq!(live.len(), 8_193 + 3 + 101, "one entry for each member");
    t.keep(false).await;
    assert!(t.entries("peak").is_empty(), "ended with the view");
    t.keep(true).await;
    assert_eq!(
        (t.says().await, t.entries("peak")),
        ((KEPT.to_owned(), "same".to_owned()), live)
    );
}

/// Holders that leave in turn read one entry each: an aging run, where
/// each reading leaves from the top, and a holder written a thousand times
/// to a lesser value that still holds, each time read from the order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn holders_that_leave_in_turn_read_one_entry_each() {
    let t = Peaks::new("kept-aging").await;
    let run = (0..200).map(|i| {
        let value = format!("{}", 1_000 + i);
        source_record(
            "reading",
            &format!("run-{i:03}"),
            json!({"site": "run", "value": value}),
        )
    });
    t.l.ingest("meter", run.collect()).await.expect("feeding");
    t.feed("last", 0, 10).await;
    t.at("last-top", "last", "5000").await;
    t.keep(true).await;
    t.forget();
    for i in (0..200).rev() {
        t.l.retire("reading", &format!("run-{i:03}"))
            .await
            .expect("retiring");
    }
    assert_eq!(
        t.entries_read("peak"),
        199,
        "one entry for each departure, none for the last"
    );
    for v in (2..1_002).rev() {
        t.at("last-top", "last", &format!("{v}")).await;
    }
    assert_eq!(
        t.entries_read("peak"),
        1_000,
        "one entry for each lesser value"
    );
    assert_eq!(t.read(), 0, "no member read");
    assert_eq!(t.says().await, (KEPT.to_owned(), "same".to_owned()));
}

/// The holder and the next leave in one append, in either order: where the
/// next left first, the seek passes over its entry, which the append ended
/// before the engine knows it, and reads the next batch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_holder_and_the_next_leave_in_one_append() {
    let t = Peaks::new("kept-two").await;
    for site in ["one", "two"] {
        t.feed(site, 0, 20).await;
        t.at(&format!("{site}-top"), site, "9").await;
        t.at(&format!("{site}-next"), site, "8").await;
        t.at(&format!("{site}-third"), site, "7").await;
    }
    t.keep(true).await;
    let away = |site: &str, end: &str| {
        let key = format!("{site}-{end}");
        source_record("reading", &key, json!({"site": "away", "value": "1"}))
    };
    t.entries_read("peak");
    t.l.ingest("meter", vec![away("one", "top"), away("one", "next")])
        .await
        .expect("moving");
    assert_eq!(t.entries_read("peak"), 2, "the next, then the third");
    t.l.ingest("meter", vec![away("two", "next"), away("two", "top")])
        .await
        .expect("moving");
    assert_eq!(
        t.entries_read("peak"),
        3,
        "the next, passed over, then the third"
    );
    assert_eq!(t.says().await, (KEPT.to_owned(), "same".to_owned()));
}

/// A group whose holders leave on a live fold, in a rebuild whose page
/// holds the events and the growth together, and where the fold was behind
/// and one transaction caught up with both, is kept with the same entries
/// on every path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_fold_a_rebuild_and_a_catch_up_decide_alike() {
    let t = Peaks::new("kept-alike").await;
    t.feed("big", 0, 8_192).await;
    t.top("big").await;
    t.keep(true).await;
    t.pad(5).await;
    t.l.retire("reading", "big-top").await.expect("retiring");
    t.feed("big", 9_000, 300).await;
    let live = (t.says().await, t.entries("peak"));
    assert_eq!(live.0, (KEPT.to_owned(), "same".to_owned()));
    t.store.rebuild().await.expect("rebuilding");
    assert_eq!((t.says().await, t.entries("peak")), live, "rebuilt");
    // The fold of the next holder's retirement is left behind, and the
    // feed's first append folds both.
    t.top("big").await;
    *t.probe.refuse_commit.lock().unwrap() = Some(1);
    let _ = t.l.retire("reading", "big-top").await;
    let refused = t.probe.refuse_commit.lock().unwrap().is_none();
    assert!(refused, "the fold of the retirement was left behind");
    t.feed("big", 10_000, 10).await;
    let caught_up = (t.says().await, t.entries("peak"));
    assert_eq!(caught_up.0, (KEPT.to_owned(), "same".to_owned()));
    t.store.rebuild().await.expect("rebuilding");
    assert_eq!(
        (t.says().await, t.entries("peak")),
        caught_up,
        "rebuilt after the catch-up"
    );
}

/// A view ended and made again in one transaction reads its next holder
/// from the entries the make put, never from those the engine held before
/// the end: a rebuild's page that stops keeping the view, moves a member
/// from 4 to 3, keeps it again and takes the holder, holds the moved
/// member at 3, as the live appends did.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_view_ended_and_made_again_in_one_transaction_reads_only_its_new_entries() {
    let t = Peaks::new("kept-again").await;
    t.feed("s", 0, 3).await;
    t.top("s").await;
    t.at("s-moved", "s", "4").await;
    t.keep(true).await;
    t.pad(0).await;
    t.keep(false).await;
    t.at("s-moved", "s", "3").await;
    t.keep(true).await;
    t.l.retire("reading", "s-top").await.expect("retiring");
    let live = (t.says().await, t.entries("peak"));
    assert_eq!(live.0, (KEPT.to_owned(), "same".to_owned()));
    t.store.rebuild().await.expect("rebuilding");
    assert_eq!((t.says().await, t.entries("peak")), live, "rebuilt");
}

/// The next holder is read from the order as a migration earlier in the
/// same transaction leaves the members: a rebuild's page that holds a
/// re-key of the view's source and then the events that take two holders
/// reads the entries under the keys the step gave the members, as the live
/// appends found them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_holder_is_read_from_the_order_through_a_migration_of_the_same_page() {
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
    // The holder leaves, and then the next one.
    t.l.retire("acct", "t9").await.expect("retiring");
    t.l.retire("acct", "t2").await.expect("retiring");
    let live = t.says_of("peak_of").await;
    assert_eq!(live.1, "same");
    assert!(live.0.contains("kept"), "{live:?}");
    let live = (live, t.entries("peak_of"));
    t.store.rebuild().await.expect("rebuilding");
    assert_eq!(
        (t.says_of("peak_of").await, t.entries("peak_of")),
        live,
        "rebuilt"
    );
}

/// A group that ends leaves nothing behind for a member that arrives
/// after it in the same transaction: a group whose members all leave and
/// a new one that arrives, the last leaving and the arrival in
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

/// The open face and the whole one read their own orders: when the holder
/// leaves, a restricted member holds the whole face, and the open face's
/// next holder is the next open member. Then the restricted holder leaves
/// too, and the whole face reads the open one's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_face_reads_its_own_next_holder() {
    let t = Peaks::new("kept-whole").await;
    t.feed("big", 0, 8_193).await;
    t.top("big").await;
    t.at("big-held", "big", "8").await;
    t.at("big-open", "big", "7").await;
    let held = restrict(t.store.as_ref(), "reading", "big-held", "held").await;
    held.expect("restricting");
    t.keep(true).await;
    let reads = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let plain = principal(t.store.as_ref(), PrincipalKind::Human, "plain", reads);
    let kept = (KEPT.to_owned(), "same".to_owned());
    for leaves in ["big-top", "big-held"] {
        t.read();
        t.l.retire("reading", leaves).await.expect("retiring");
        assert_eq!(t.read(), 0, "{leaves}: no member read");
        assert_eq!(t.says().await, kept, "{leaves}: the whole face");
        assert_eq!(
            t.says_to(&plain, "peak").await,
            kept,
            "{leaves}: the open face"
        );
    }
    let whole = t.kept("peak", "big");
    assert!(
        !whole.contains("lost") && !whole.contains("waits"),
        "{whole}"
    );
}

/// A view over a view reads its members' entries on each face: the inner
/// group's record moving between groups in one event, where the outer
/// holder's group falls and another rises past it, and an inner group whose
/// faces hold equal values written otherwise, `1` and `1.0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_view_over_a_view_reads_the_next_holder_on_each_face() {
    let t = Peaks::new("kept-nested").await;
    let field = |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}});
    let measure = |n: &str, e: &str| json!({"name": n, "expr": e});
    for end in ["", "_twin"] {
        // Every inner group is in the one outer group `all`.
        let zone = json!({"declare": "type", "name": format!("zone{end}"), "key": ["site"],
            "fields": [field("site", "text"), field("peak", "decimal"), field("band", "text")],
            "view": {"source": "reading", "group_by": ["site"],
                     "measures": [measure("peak", "max(value)"),
                                  measure("band", "if(count() > 0, 'all', 'none')")]}});
        let crest = json!({"declare": "type", "name": format!("crest{end}"), "key": ["band"],
            "fields": [field("band", "text"), field("high", "decimal")],
            "view": {"source": format!("zone{end}"), "group_by": ["band"],
                     "measures": [measure("high", "max(peak)")]}});
        for d in [zone, crest] {
            t.l.declare_json(d).await.expect("declaring");
        }
    }
    for (key, site, value) in [
        ("a1", "a", "10"),
        ("a2", "a", "3"),
        ("b1", "b", "5"),
        ("c1", "c", "8"),
    ] {
        t.at(key, site, value).await;
    }
    t.keep_of("zone", true).await;
    t.keep_of("crest", true).await;
    let kept = (KEPT.to_owned(), "same".to_owned());
    assert_eq!(t.says_of("crest").await, kept);
    // `a`'s peak falls from 10 to 3 and `b`'s rises from 5 to 12, in one
    // event: the holder `b` took stays, past `c`'s 8.
    t.at("a1", "b", "12").await;
    assert_eq!(t.says_of("crest").await, kept, "moved");
    // An inner group whose whole face holds `1.0` and whose open face `1`.
    t.at("b2", "d", "1").await;
    t.at("b0", "d", "1.0").await;
    let held = restrict(t.store.as_ref(), "reading", "b0", "held").await;
    held.expect("restricting");
    let reads = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let plain = principal(t.store.as_ref(), PrincipalKind::Human, "plain", reads);
    for gone in ["a1", "c1", "b1", "a2"] {
        t.l.retire("reading", gone).await.expect("retiring");
        assert_eq!(t.says_of("crest").await, kept, "{gone}: the whole face");
        assert_eq!(
            t.says_to(&plain, "crest").await,
            kept,
            "{gone}: the open face"
        );
    }
    let live = t.entries("crest");
    t.store.rebuild().await.expect("rebuilding");
    assert_eq!(t.entries("crest"), live, "rebuilt");
}

/// Two sources and a view over a view keep their extremes when a holder
/// leaves, whatever the number of their members: a group of 4 100 records
/// in each of two sources, and an outer group over 8 193 inner groups that
/// read otherwise on their whole face.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sources_and_views_of_any_size_keep_their_extremes() {
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
    assert_eq!(
        t.says_of("pair").await,
        (KEPT.to_owned(), "same".to_owned())
    );
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
        (KEPT.to_owned(), "same".to_owned())
    );
    // A holder that leaves in the view's own source reads nothing of the
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
    t.forget();
    t.l.retire("reading", "huge-top").await.expect("retiring");
    assert_eq!((t.read_of("spare"), t.read()), (0, 0), "no member read");
    assert_eq!(
        t.says_of("pair").await,
        (KEPT.to_owned(), "same".to_owned())
    );
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

/// A set of the view pays where a record the view reads is written, and
/// nowhere else: at an ingest the write path reads the view's state and
/// the fold reads it not again, and a write of another type reads none
/// (lakelands#1185).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_set_of_a_view_pays_where_the_view_is_written() {
    let t = Peaks::new("kept-set-cost").await;
    let states = || t.probe.states.swap(0, Ordering::SeqCst);
    t.keep(true).await;
    t.feed("a", 0, 3).await;
    let set = json!({"declare": "object_set", "name": "high", "of": "peak", "where": "peak > 5"});
    t.l.declare_json(set).await.expect("the set");
    states();
    t.put("other", "o", json!({"x": "z"})).await;
    assert_eq!(states(), 0, "an append to a type no view reads");
    t.feed("a", 10, 1).await;
    assert_eq!(states(), 1, "an append to the view's source");
    t.top("a").await;
    let (peak, high) = (
        "peak".into(),
        lakelands_core::conformance::fixture::find_where("in_set('high')"),
    );
    let topped = t.l.cat.find(&t.l.own, &peak, high);
    let topped = topped.await.expect("reading the set");
    assert!(
        matches!(topped.data, Found::Records { ref rows, .. } if rows.len() == 1),
        "`a` is high: {:?}",
        topped.data
    );
}

/// A write refused past `max_set_changes` is undone in the write path's
/// run of the hooks, which goes on from the writes accepted before it:
/// an aging run of twelve moves past a ceiling of four reads each group's
/// kept row once there, however many moves before it were refused, and
/// the fold reads the four it moved. The write path runs no kept view a
/// set does not hold: `by_status` is read by the fold alone
/// (lakelands#1185).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_write_costs_the_writes_before_it_nothing() {
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        ..Seams::default()
    };
    let mut at = Location::memory("kept-set-refused");
    at.limits.max_set_changes = 4;
    let store = lakelands_store::open(at, seams, OpenMode::Create);
    let store = store.await.expect("a new lake");
    let l = Lake::of(store.as_ref());
    let field = |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}});
    for d in [
        json!({"declare": "type", "name": "invoice",
            "fields": [field("customer", "text"), field("amount", "decimal"), field("status", "text")]}),
        json!({"declare": "type", "name": "balance", "key": ["customer"],
            "fields": [field("customer", "text"), field("total", "decimal")],
            "view": {"source": "invoice", "where": "status = 'open'", "group_by": ["customer"],
                "measures": [{"name": "total", "expr": "sum(amount)"}]}}),
        json!({"declare": "type", "name": "by_status", "key": ["status"],
            "fields": [field("status", "text"), field("n", "number")],
            "view": {"source": "invoice", "group_by": ["status"],
                "measures": [{"name": "n", "expr": "count()"}]}}),
    ] {
        l.declare_json(d).await.expect("declaring");
    }
    for view in ["balance", "by_status"] {
        let key = format!("view_kept.{view}");
        l.set(&key, json!(true)).await.expect("keeping");
    }
    let set =
        json!({"declare": "object_set", "name": "owing", "of": "balance", "where": "total > 100"});
    l.declare_json(set).await.expect("the set");
    for i in 0..12 {
        let open = json!({"customer": format!("c{i:02}"), "amount": "150", "status": "open"});
        l.upsert("invoice", &format!("i{i:02}"), open)
            .await
            .expect("an invoice");
    }
    let rule = json!({"on": "invoice", "field": "status", "from": "open", "to": "closed", "when": "amount > 100"});
    let aging = lakelands_api::seam::AGING_RULE_TYPE;
    l.upsert(aging, "close", rule).await.expect("the rule");
    let kept = lakelands_api::seam::kept_name(&"balance".into());
    probe.gets.lock().unwrap().clear();
    probe.states.swap(0, Ordering::SeqCst);
    let today = lakelands_api::Timestamp::from_date(2026, 3, 1).expect("a date");
    let aged = store.age(today).await.expect("aging");
    let run = aged.rules.iter().find(|r| r.rule == "close");
    assert!(
        run.is_some_and(|r| r.moved == 4 && r.failed_guards == 8),
        "four moves, eight failed guards: {aged:?}"
    );
    let read = probe.gets.lock().unwrap().get(&kept).copied();
    assert_eq!(read, Some(12 + 4), "each group's kept row read once");
    let by_status = lakelands_api::seam::kept_name(&"by_status".into());
    let read = probe.gets.lock().unwrap().get(&by_status).copied();
    let states = probe.states.load(Ordering::SeqCst);
    assert_eq!(
        (read, states),
        (Some(2), 3),
        "`by_status` read by the fold alone"
    );
}

/// A group flagged as not exact, or whose measure fails, stands for a
/// failure a write may clear without the flag moving, so a write to it
/// decides every group of the view for the sets that read it; a write to
/// an exact group decides its own alone (lakelands#1185).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_to_a_flagged_group_decides_every_group_of_its_view() {
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory("kept-flagged"), seams, OpenMode::Create);
    let store = store.await.expect("a new lake");
    let l = Lake::of(store.as_ref());
    let field = |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}});
    for d in [
        json!({"declare": "type", "name": "inv",
            "fields": [field("customer", "text"), field("amount", "decimal")]}),
        json!({"declare": "type", "name": "bal", "key": ["customer"],
            "fields": [field("customer", "text"), field("total", "decimal")],
            "view": {"source": "inv", "group_by": ["customer"],
                "measures": [{"name": "total", "expr": "sum(amount)"}]}}),
        json!({"declare": "type", "name": "tier", "key": ["customer"],
            "fields": [field("customer", "text"), field("k", "number")],
            "view": {"source": "bal", "group_by": ["customer"],
                "measures": [{"name": "k", "expr": "count()"}]}}),
    ] {
        l.declare_json(d).await.expect("declaring");
    }
    for view in ["bal", "tier"] {
        l.set(&format!("view_kept.{view}"), json!(true))
            .await
            .expect("keeping");
    }
    let set = json!({"declare": "object_set", "name": "tiered", "of": "tier", "where": "k >= 1"});
    l.declare_json(set).await.expect("the set");
    let groups = 10;
    for i in 0..groups {
        let fields = json!({"customer": format!("c{i}"), "amount": "50"});
        l.upsert("inv", &format!("e{i}"), fields)
            .await
            .expect("an invoice");
    }
    // `big` holds a sum the fold cannot keep exactly.
    for (k, a) in [
        ("b1", "70000000000000000000000000000"),
        ("b2", "-69999999999999999999999999890"),
    ] {
        let fields = json!({"customer": "big", "amount": a});
        l.upsert("inv", k, fields).await.expect("an invoice");
    }
    let snap = |probe: &Probe| {
        let kept = lakelands_api::seam::kept_name(&"tier".into());
        let gets = probe.gets.lock().unwrap().get(&kept).copied().unwrap_or(0);
        let scans = probe.rows.lock().unwrap().get(&kept).copied().unwrap_or(0);
        (gets, scans, probe.states.load(Ordering::SeqCst))
    };
    let a = snap(&probe);
    l.upsert("inv", "e3", json!({"customer": "c3", "amount": "60"}))
        .await
        .expect("a write to an exact group");
    let b = snap(&probe);
    l.upsert("inv", "b3", json!({"customer": "big", "amount": "1"}))
        .await
        .expect("a write to a flagged group");
    let c = snap(&probe);
    let (exact, flagged) = (b.0 + b.1 - a.0 - a.1, c.0 + c.1 - b.0 - b.1);
    assert!(
        exact < groups && flagged > groups,
        "a write to an exact group reads fewer than the {groups} groups' rows and one to a \
         flagged group reads every group's: {exact} and {flagged}"
    );
}

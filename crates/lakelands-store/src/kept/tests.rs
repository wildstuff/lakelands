use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::read::{Find, Found};
use lakelands_api::seam::Seams;
use lakelands_api::write::Declaration;
use lakelands_api::{
    Access, Catalog, Grants, LakeStore, Location, Principal, PrincipalKind, WriteContext,
};
use lakelands_engine_mem::MemoryOpener;
use serde_json::{Value, json};

use super::*;
use crate::doors::Store;
use crate::lake::Lake;

/// A lake on the memory engine, its doors, its owner and a reader who
/// reads everything but restricted records.
struct Kept {
    lake: Arc<Lake>,
    store: Store,
    cat: Arc<dyn Catalog>,
    owner: Principal,
    plain: Principal,
}

/// A view of `source` grouped by `by`, under `name` and under `name_twin`:
/// the first is the one a test keeps, the second is always computed.
fn views(
    name: &str,
    source: &str,
    by: &[&str],
    rest: Value,
    measures: &[(&str, &str)],
) -> Vec<Value> {
    let field = |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}});
    let mut fields: Vec<Value> = by.iter().map(|n| field(n, "text")).collect();
    fields.extend(measures.iter().map(|(n, _)| field(n, "decimal")));
    let measures: Vec<Value> = (measures.iter())
        .map(|(n, e)| json!({"name": n, "expr": e}))
        .collect();
    let mut view = json!({"source": source, "group_by": by, "measures": measures});
    for (k, v) in rest.as_object().into_iter().flatten() {
        view[k] = v.clone();
    }
    let declared = |name: String| json!({"declare": "type", "name": name, "key": by, "fields": fields, "view": view});
    vec![declared(name.to_owned()), declared(format!("{name}_twin"))]
}

/// A stored type with these fields, each `(name, kind)`.
fn stored(name: &str, fields: &[(&str, &str)]) -> Value {
    let fields: Vec<Value> = (fields.iter())
        .map(|(n, kind)| json!({"name": n, "type": {"kind": kind}}))
        .collect();
    json!({"declare": "type", "name": name, "fields": fields})
}

impl Kept {
    async fn new(declarations: Vec<Value>) -> Kept {
        let seams = Seams {
            engines: vec![Arc::new(MemoryOpener::new())],
            ..Seams::default()
        };
        let lake = Lake::open(&Location::memory("kept"), seams, OpenMode::Create);
        let lake = Arc::new(lake.expect("the lake opens"));
        let store = Store { lake: lake.clone() };
        let cat = store.catalog(WriteContext::default());
        let who = |id: &str, grants: Grants| Principal {
            kind: PrincipalKind::Human,
            id: id.into(),
            lake: "kept".into(),
            grants,
        };
        let reads = Grants {
            default: Access::Read,
            ..Grants::default()
        };
        let t = Kept {
            lake,
            store,
            cat,
            owner: who("owner", Grants::owner()),
            plain: who("plain", reads),
        };
        for d in declarations {
            t.declare(d).await;
        }
        t
    }

    async fn declare(&self, d: Value) {
        let d: Declaration = serde_json::from_value(d).expect("a declaration");
        self.cat.declare(&self.owner, d).await.expect("declaring");
    }

    async fn put(&self, ty: &str, key: &str, fields: Value) {
        let patch = serde_json::from_value(fields).expect("a patch");
        let (ty, key) = (TypeName::from(ty), RecordKey::from(key));
        let wrote = (self.cat).upsert(&self.owner, &ty, &key, patch, Default::default());
        wrote.await.expect("writing a record");
    }

    /// Feeds `reading` from one source in one call, each record as its
    /// key, its site and its value.
    async fn feed(&self, records: Vec<(String, String, &str)>) {
        let reading = |(key, site, value): (String, String, &str)| lakelands_api::SourceRecord {
            upstream: None,
            ty: "reading".into(),
            key: key.as_str().into(),
            fields: serde_json::from_value(json!({"site": site, "value": value})).expect("a patch"),
            seen_at: None,
        };
        let records = records.into_iter().map(reading).collect();
        let meter = lakelands_api::SourceId::from("meter");
        let fed = lakelands_api::ingest_all(self.cat.as_ref(), &self.owner, &meter, records);
        fed.await.expect("feeding the source");
    }

    async fn set(&self, key: &str, value: Value) {
        self.put("sys.setting", key, json!({"key": key, "value": value}))
            .await;
    }

    async fn retire(&self, ty: &str, key: &str) {
        let (ty, key) = (TypeName::from(ty), RecordKey::from(key));
        let gone = (self.cat).retire(&self.owner, &ty, &key, Default::default());
        gone.await.expect("retiring a record");
    }

    /// What `who` is answered for `ty`: every record, or the refusal; then
    /// the same records a page of one at a time in the descending order of
    /// their last field, with the total, and a `get` of each.
    async fn read(&self, who: &Principal, ty: &str) -> String {
        let ty = TypeName::from(ty);
        let shown = |r: &lakelands_api::Record| json!(r.fields).to_string();
        let rows = match self.cat.find(who, &ty, Find::default()).await {
            Ok(reply) => match reply.data {
                Found::Records { rows, .. } => rows,
                other => return format!("{other:?}"),
            },
            Err(e) => return format!("{e:?}"),
        };
        let mut out: Vec<String> = rows.iter().map(shown).collect();
        let Some(last) = rows.first().and_then(|r| r.fields.keys().next_back()) else {
            return out.join(" ");
        };
        let mut page = Find {
            order_by: vec![lakelands_api::read::Order {
                field: last.clone(),
                by: Default::default(),
                dir: lakelands_api::read::Direction::Desc,
            }],
            limit: Some(1),
            total: true,
            ..Find::default()
        };
        loop {
            let found = self.cat.find(who, &ty, page.clone()).await;
            let Found::Records {
                rows,
                next_cursor,
                total,
            } = found.expect("a page").data
            else {
                panic!("a page of records");
            };
            out.push(format!(
                "page {:?} of {total:?}",
                rows.iter().map(shown).collect::<Vec<_>>()
            ));
            page.cursor = next_cursor;
            if page.cursor.is_none() {
                break;
            }
        }
        for r in &rows {
            let get = lakelands_api::read::Get {
                key: r.key.clone(),
                with_meta: false,
                params: Default::default(),
                at: Default::default(),
            };
            let got = self.cat.get(who, &ty, get).await;
            out.push(format!(
                "get {}",
                got.map(|g| shown(&g.data)).unwrap_or_default()
            ));
        }
        out.join(" ")
    }

    /// The kept view `name` answers both readers what its twin answers;
    /// answers the owner's reading.
    async fn same(&self, name: &str, when: &str) -> String {
        let twin = format!("{name}_twin");
        for who in [&self.owner, &self.plain] {
            let (kept, computed) = (self.read(who, name).await, self.read(who, &twin).await);
            // A refusal names its view: the twins' names are told apart by
            // their ending alone.
            let computed = computed.replace("_twin", "");
            assert_eq!(kept, computed, "{when}, read by {}", who.id);
        }
        self.read(&self.owner, name).await
    }

    /// The state the fold keeps of `view`: whether it is whole, and its
    /// counters as `(not exact open, not exact whole, differ)`.
    fn state(&self, view: &str) -> (bool, [u64; 3]) {
        self.state_at(None, view)
    }

    /// [`Self::state`] as of `at`.
    fn state_at(&self, at: Option<Position>, view: &str) -> (bool, [u64; 3]) {
        let at = self.lake.engine.view(at).expect("a view of the projection");
        let s = state(at.as_ref(), &view.into()).expect("reading the state");
        let s = s.unwrap_or_default();
        (s.whole, [s.not_exact[0], s.not_exact[1], s.differ])
    }

    /// What `describe_type` says the owner of whether `view` is kept.
    async fn kept(&self, view: &str) -> String {
        self.kept_to(&self.owner, view).await
    }

    /// What `describe_type` says `who` of whether `view` is kept.
    async fn kept_to(&self, who: &Principal, view: &str) -> String {
        let ty = TypeName::from(view);
        let described = (self.cat).describe_type(who, &ty, Default::default());
        let lifecycle = described.await.expect("describing the view").data.lifecycle;
        json!(lifecycle.kept).to_string()
    }

    /// How many records the engine holds under the view's kept name.
    fn records(&self, view: &str) -> u64 {
        let at = self.lake.engine.view(None).expect("a view of the head");
        let scan = lakelands_api::engine::Scan {
            limit: u32::MAX,
            restricted: true,
            ..Default::default()
        };
        at.count(&lakelands_api::seam::kept_name(&view.into()), &scan)
            .expect("counting")
    }
}

const WHOLE: (bool, [u64; 3]) = (true, [0, 0, 0]);

/// A view under its setting is made over the records its source holds,
/// and follows a put, a change within a group, a member that moves to
/// another group and one that is retired: it answers what its twin
/// computes, from its kept records, and a group that loses its last
/// member loses its record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_view_follows_its_source() {
    let mut declarations = vec![stored("line", &[("kind", "text"), ("net", "decimal")])];
    let measures = [
        ("total", "sum(net)"),
        ("n", "count()"),
        ("mean", "avg(net)"),
    ];
    declarations.extend(views("book", "line", &["kind"], json!({}), &measures));
    let t = Kept::new(declarations).await;
    t.put("line", "a", json!({"kind": "k", "net": "3.50"}))
        .await;
    assert_eq!(t.state("book"), (false, [0; 3]), "no setting, no kept view");
    assert!(t.kept("book").await.contains("not_asked"));
    t.set("view_kept.book", json!(true)).await;
    assert_eq!(t.kept("book").await, r#"{"state":"kept"}"#);
    assert_eq!(
        (t.state("book"), t.records("book")),
        (WHOLE, 1),
        "made by its setting"
    );
    t.same("book", "made over one record").await;
    t.put("line", "b", json!({"kind": "k", "net": "4"})).await;
    t.put("line", "c", json!({"kind": "j", "net": "5.25"}))
        .await;
    t.same("book", "two more records").await;
    t.put("line", "b", json!({"net": "6.125"})).await;
    t.same("book", "a member changes within its group").await;
    t.put("line", "a", json!({"kind": "j"})).await;
    let moved = t.same("book", "a member moves to another group").await;
    assert!(moved.contains("8.75") && moved.contains("6.125"), "{moved}");
    t.retire("line", "b").await;
    t.same("book", "the last member of a group is retired")
        .await;
    assert_eq!(
        (t.state("book"), t.records("book")),
        (WHOLE, 1),
        "one group is left"
    );
    t.put("line", "d", json!({"kind": null, "net": "1"})).await;
    t.same("book", "a member with no group").await;
    assert_eq!(t.state("book"), WHOLE);
}

/// Retiring the setting ends the view's records and its state: it is
/// computed again; written again, it is made again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_view_is_kept_while_its_setting_holds() {
    let mut declarations = vec![stored("line", &[("kind", "text"), ("net", "decimal")])];
    declarations.extend(views(
        "book",
        "line",
        &["kind"],
        json!({}),
        &[("total", "sum(net)")],
    ));
    let t = Kept::new(declarations).await;
    t.set("view_kept.book", json!(true)).await;
    t.put("line", "a", json!({"kind": "k", "net": "3"})).await;
    assert_eq!((t.state("book"), t.records("book")), (WHOLE, 1));
    t.retire("sys.setting", "view_kept.book").await;
    assert_eq!(
        (t.state("book"), t.records("book")),
        ((false, [0; 3]), 0),
        "ended"
    );
    t.put("line", "b", json!({"kind": "k", "net": "4"})).await;
    t.same("book", "computed again").await;
    t.set("view_kept.book", json!(true)).await;
    assert_eq!(
        (t.state("book"), t.records("book")),
        (WHOLE, 1),
        "made again"
    );
    t.same("book", "made again").await;
    t.set("view_kept.book", json!(false)).await;
    assert_eq!(
        t.state("book"),
        (false, [0; 3]),
        "a setting that is not true keeps nothing"
    );
}

impl Kept {
    async fn restrict(&self, ty: &str, key: &str, restrict: bool) {
        let (ty, key) = (TypeName::from(ty), RecordKey::from(key));
        let done = match restrict {
            true => self.store.restrict(&ty, &key, "kept".into()).await,
            false => self.store.unrestrict(&ty, &key).await,
        };
        done.expect("restricting a record");
    }
}

/// A restricted member is a member of the whole face alone: the reader
/// with the grant and the one without are each answered what the twin
/// computes for them, the state counts the groups whose faces differ, and
/// a group whose members are all restricted has a record only the first
/// reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restricted_member_is_of_the_whole_face_alone() {
    let mut declarations = vec![stored("line", &[("kind", "text"), ("net", "decimal")])];
    let measures = [("total", "sum(net)"), ("n", "count()")];
    declarations.extend(views("book", "line", &["kind"], json!({}), &measures));
    let t = Kept::new(declarations).await;
    t.set("view_kept.book", json!(true)).await;
    for (key, kind, net) in [("a", "k", "3"), ("b", "k", "4"), ("c", "j", "5")] {
        t.put("line", key, json!({"kind": kind, "net": net})).await;
    }
    t.restrict("line", "b", true).await;
    t.same("book", "one member of two restricted").await;
    assert_eq!(t.state("book"), (true, [0, 0, 1]), "one group differs");
    t.restrict("line", "c", true).await;
    t.same("book", "the only member of a group restricted")
        .await;
    assert_eq!((t.state("book"), t.records("book")), ((true, [0, 0, 1]), 2));
    assert!(
        !t.read(&t.plain, "book").await.contains("\"j\""),
        "the open face has no `j`"
    );
    t.put("line", "b", json!({"net": "40"})).await;
    t.same("book", "a restricted member changes").await;
    t.restrict("line", "b", false).await;
    t.restrict("line", "c", false).await;
    t.same("book", "both lifted").await;
    assert_eq!(t.state("book"), WHOLE, "no group differs");
}

/// A merged source's groups meet the view's own by their keys, and a group
/// only one of them has is measured over the other's empty group.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_merged_source_is_folded_beside_the_views_own() {
    let mut declarations = vec![
        stored("invoice", &[("partner", "text"), ("amount", "decimal")]),
        stored("payment", &[("payer", "text"), ("paid", "decimal")]),
    ];
    let merge = json!({"merge": [{"source": "payment", "group_by": ["payer"],
        "measures": [{"name": "paid", "expr": "sum(paid)"}]}]});
    let measures = [("billed", "sum(amount)"), ("open", "sum(amount) - paid")];
    let mut both = views("debtor", "invoice", &["partner"], merge, &measures);
    for v in &mut both {
        let paid = json!({"name": "paid", "type": {"kind": "decimal"}});
        v["fields"].as_array_mut().expect("fields").push(paid);
    }
    declarations.extend(both);
    let t = Kept::new(declarations).await;
    t.put("invoice", "i1", json!({"partner": "p1", "amount": "100"}))
        .await;
    t.put("payment", "y1", json!({"payer": "p2", "paid": "7"}))
        .await;
    t.set("view_kept.debtor", json!(true)).await;
    t.same("debtor", "made over both sources").await;
    t.put("payment", "y2", json!({"payer": "p1", "paid": "30.5"}))
        .await;
    t.put("invoice", "i2", json!({"partner": "p2", "amount": "9"}))
        .await;
    let met = t
        .same("debtor", "each source gains a member of the other's group")
        .await;
    assert!(met.contains("69.5") && met.contains("\"2\""), "{met}");
    t.retire("invoice", "i1").await;
    t.same("debtor", "the view's own member of a group leaves")
        .await;
    assert_eq!(t.state("debtor"), WHOLE);
}

/// A view over a kept view folds that view's kept records, and follows
/// them as they change. The outer view's name sorts before the inner's:
/// the order they are put in is that of what they read, not of names.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_view_over_a_kept_view_follows_its_records() {
    let mut declarations = vec![stored("line", &[("kind", "text"), ("net", "decimal")])];
    declarations.extend(views(
        "book",
        "line",
        &["kind"],
        json!({}),
        &[("total", "sum(net)")],
    ));
    let over = json!({"where": "total > 4"});
    let measures = [("all", "sum(total)"), ("kinds", "count()")];
    for twin in ["book", "book_twin"] {
        let mut both = views("above", twin, &[], over.clone(), &measures);
        declarations.push(both.remove(usize::from(twin != "book")));
    }
    let t = Kept::new(declarations).await;
    t.put("line", "a", json!({"kind": "k", "net": "3"})).await;
    t.set("view_kept.above", json!(true)).await;
    assert_eq!(t.state("above"), (false, [0; 3]), "its source is not kept");
    assert!(t.kept("above").await.contains("not_kept_here"));
    t.set("view_kept.book", json!(true)).await;
    assert_eq!(
        (t.state("book"), t.state("above")),
        (WHOLE, WHOLE),
        "both are made"
    );
    t.same("above", "no group of the inner view is above 4")
        .await;
    t.put("line", "b", json!({"kind": "k", "net": "2.5"})).await;
    t.put("line", "c", json!({"kind": "j", "net": "9"})).await;
    let both = t.same("above", "two inner groups above 4").await;
    assert!(both.contains("14.5"), "{both}");
    t.retire("sys.setting", "view_kept.book").await;
    assert_eq!(
        (t.state("book"), t.state("above")),
        ((false, [0; 3]), (false, [0; 3]))
    );
    t.same("above", "both computed again").await;
}

/// A group can be on the open face alone: a view over a kept view whose
/// `where` holds over an inner group's open face and fails over its whole
/// one. The reader with the grant is answered no record of that group, as
/// the twin computes, the state counts it among the groups that differ,
/// and a view over that view takes no member from it on the whole face;
/// so too after a rebuild.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_group_on_the_open_face_alone_is_not_the_whole_faces() {
    let mut declarations = vec![stored("line", &[("g", "text"), ("n", "decimal")])];
    let inner = [("c", "count()"), ("total", "sum(n)"), ("mean", "avg(n)")];
    declarations.extend(views("sums", "line", &["g"], json!({}), &inner));
    let over = json!({"where": "total > 100"});
    let measures = [("c", "count()"), ("held", "sum(total)")];
    for (large, sums) in [("large", "sums"), ("large_twin", "sums_twin")] {
        let twin = usize::from(sums != "sums");
        let mut both = views("large", sums, &["g"], over.clone(), &measures);
        declarations.push(both.remove(twin));
        let mut both = views("top", large, &[], json!({}), &[("groups", "count()")]);
        declarations.push(both.remove(twin));
    }
    let t = Kept::new(declarations).await;
    for view in ["top", "large", "sums"] {
        t.set(&format!("view_kept.{view}"), json!(true)).await;
    }
    t.put("line", "m1", json!({"g": "m", "n": "150"})).await;
    t.put("line", "m2", json!({"g": "m", "n": "-100"})).await;
    t.same("large", "a group of 50 on both faces").await;
    t.restrict("line", "m2", true).await;
    let whole = t
        .same("large", "150 on the open face, 50 on the whole")
        .await;
    assert_eq!(whole, "", "the whole face has no group above 100");
    let open = t.read(&t.plain, "large").await;
    assert!(open.contains(r#""held":"150""#), "{open}");
    assert_eq!(t.state("large"), (true, [0, 0, 1]), "the group differs");
    let top = t.same("top", "over the group of the open face alone").await;
    assert!(top.contains(r#""groups":"0""#), "{top}");
    t.store.rebuild().await.expect("rebuilding");
    t.same("large", "rebuilt").await;
    t.same("top", "rebuilt").await;
    assert_eq!(t.state("large"), (true, [0, 0, 1]), "rebuilt");
    t.restrict("line", "m2", false).await;
    t.same("large", "lifted").await;
    assert_eq!((t.state("large"), t.state("top")), (WHOLE, WHOLE), "lifted");
}

/// A measure that fails over one group is counted and never returned: the
/// lake goes on, the view computes and refuses as its twin does, and it is
/// kept again when the group's measure evaluates.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_measure_is_counted_and_the_view_computes() {
    let mut declarations = vec![stored(
        "line",
        &[("kind", "text"), ("net", "decimal"), ("by", "decimal")],
    )];
    let measures = [("rate", "sum(net) / sum(by)")];
    declarations.extend(views("book", "line", &["kind"], json!({}), &measures));
    let t = Kept::new(declarations).await;
    t.put("line", "a", json!({"kind": "k", "net": "3", "by": "0"}))
        .await;
    t.put("line", "b", json!({"kind": "j", "net": "3", "by": "2"}))
        .await;
    t.set("view_kept.book", json!(true)).await;
    assert_eq!(t.state("book"), (true, [1, 1, 0]), "made, one group fails");
    let said = t.kept("book").await;
    let one = r#""because":"not_whole","not_exact":1"#;
    assert!(said.contains(one), "{said}");
    let refused = t.same("book", "a group divides by zero").await;
    assert!(refused.contains("Refused"), "{refused}");
    t.put("line", "c", json!({"kind": "k", "net": "1", "by": "4"}))
        .await;
    assert_eq!(t.state("book"), WHOLE, "the group evaluates again");
    t.same("book", "kept again").await;
}

/// `describe_type` says how many groups are not exact for the reader's
/// own face, and says it to a principal the view's read would answer: one
/// kept from a field the view reads is told nothing of its records.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_number_of_groups_not_exact_is_the_readers_faces_and_behind_the_probe() {
    let mut declarations = vec![stored(
        "line",
        &[("kind", "text"), ("net", "decimal"), ("by", "decimal")],
    )];
    let measures = [("rate", "sum(net) / sum(by)")];
    declarations.extend(views("book", "line", &["kind"], json!({}), &measures));
    let t = Kept::new(declarations).await;
    t.set("view_kept.book", json!(true)).await;
    t.put("line", "a", json!({"kind": "k", "net": "3", "by": "2"}))
        .await;
    t.put("line", "b", json!({"kind": "k", "net": "3", "by": "-2"}))
        .await;
    t.restrict("line", "b", true).await;
    assert_eq!(t.state("book"), (true, [0, 1, 1]), "the whole face fails");
    let mut blind = t.owner.clone();
    let none = [("by".into(), Access::None)].into();
    let line = lakelands_api::principal::TypeGrants {
        default: None,
        fields: none,
        ..Default::default()
    };
    blind.grants.types.insert("line".into(), line);
    let said = [&t.owner, &t.plain, &blind].map(|who| t.kept_to(who, "book"));
    let [owner, plain, blind] = said;
    assert_eq!(
        [owner.await, plain.await, blind.await],
        [
            r#"{"state":"computed","why":{"because":"not_whole","not_exact":1}}"#,
            r#"{"state":"kept"}"#,
            r#"{"state":"computed","why":{"because":"not_kept_here"}}"#,
        ]
    );
}

/// A group whose extreme lost its holder and whose last member left in
/// the same append leaves what two appends leave, and what a rebuild
/// leaves: no record, and a state that counts nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_group_that_ends_where_its_holder_left_leaves_nothing_behind() {
    let source =
        |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}, "authority": "source"});
    let reading = json!({"declare": "type", "name": "reading",
        "fields": [source("site", "text"), source("value", "decimal")]});
    let mut declarations = vec![reading];
    let open = json!({"where": "value > 0"});
    let measures = [("peak", "max(value)")];
    declarations.extend(views("peak", "reading", &["site"], open, &measures));
    let t = Kept::new(declarations).await;
    t.set("view_kept.peak", json!(true)).await;
    for (site, appends) in [("one", 1), ("two", 2)] {
        let (hi, lo) = (format!("{site}-hi"), format!("{site}-lo"));
        let at = || site.to_owned();
        t.feed(vec![(hi.clone(), at(), "9"), (lo.clone(), at(), "1")])
            .await;
        // The holder leaves the view's `where`, then the other member.
        let mut out = vec![(hi, at(), "0"), (lo, at(), "0")];
        if appends == 2 {
            t.feed(out.drain(..1).collect()).await;
        }
        t.feed(out).await;
        t.same("peak", "the group ended").await;
        assert_eq!((t.state("peak"), t.records("peak")), (WHOLE, 0), "{site}");
    }
    t.store.rebuild().await.expect("rebuilding");
    assert_eq!((t.state("peak"), t.records("peak")), (WHOLE, 0), "rebuilt");
}

/// A source or the view declared again makes the view again under the
/// new declaration; a setting its `where` names, written, does the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declaration_or_a_setting_it_reads_makes_the_view_again() {
    let line = |more: &[(&str, &str)]| {
        let mut fields = vec![("kind", "text"), ("net", "decimal")];
        fields.extend(more);
        stored("line", &fields)
    };
    let mut declarations = vec![line(&[])];
    let over = json!({"where": "net >= setting('least')"});
    declarations.extend(views(
        "book",
        "line",
        &["kind"],
        over.clone(),
        &[("total", "sum(net)")],
    ));
    let t = Kept::new(declarations).await;
    t.set("least", json!(2)).await;
    t.set("view_kept.book", json!(true)).await;
    t.put("line", "a", json!({"kind": "k", "net": "3"})).await;
    t.put("line", "b", json!({"kind": "k", "net": "1"})).await;
    t.same("book", "one member below the setting").await;
    t.set("least", json!(1)).await;
    let both = t
        .same("book", "the setting its `where` names is written")
        .await;
    assert!(both.contains("\"4\""), "{both}");
    t.declare(line(&[("note", "text")])).await;
    t.same("book", "its source is declared again").await;
    for v in views(
        "book",
        "line",
        &["kind"],
        over,
        &[("total", "sum(net) + 1")],
    ) {
        t.declare(v).await;
    }
    let again = t.same("book", "the view is declared again").await;
    assert!(again.contains("\"5\""), "{again}");
    assert_eq!(t.state("book"), WHOLE);
}

/// A declaration that computes a source's records again ends the view at
/// its position, and the view is made from the records once they are put:
/// a function a source's field calls declared again, the source declared
/// again with another derived expression, and both in one append, where
/// the view is not whole as of the first of them and whole as of the last.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declaration_that_computes_a_source_again_makes_the_view_again() {
    let scaled = |by: u8| {
        json!({"declare": "function", "name": "scaled", "backing": "expr", "expr": format!("net * {by}"),
               "inputs": [{"name": "net", "type": {"kind": "decimal"}}]})
    };
    let line = |fee: u8| {
        let computed = json!({"kind": "decimal"});
        json!({"declare": "type", "name": "line", "fields": [
            {"name": "kind", "type": {"kind": "text"}},
            {"name": "net", "type": {"kind": "decimal"}},
            {"name": "gross", "type": computed, "authority": "derived", "function": "scaled"},
            {"name": "fee", "type": computed, "authority": "derived", "derived": format!("net + {fee}")}]})
    };
    let mut declarations = vec![scaled(2), line(1)];
    let measures = [("total", "sum(gross)"), ("fees", "sum(fee)")];
    declarations.extend(views("book", "line", &["kind"], json!({}), &measures));
    let t = Kept::new(declarations).await;
    t.put("line", "a", json!({"kind": "k", "net": "3"})).await;
    t.put("line", "b", json!({"kind": "k", "net": "4"})).await;
    t.set("view_kept.book", json!(true)).await;
    let made = t.same("book", "made").await;
    assert!(
        made.contains(r#""fees":"9""#) && made.contains(r#""total":"14""#),
        "{made}"
    );
    t.declare(scaled(3)).await;
    let again = t
        .same("book", "a function its source calls is declared again")
        .await;
    assert!(again.contains(r#""total":"21""#), "{again}");
    assert_eq!((t.state("book"), t.records("book")), (WHOLE, 1));
    t.declare(line(2)).await;
    let again = t
        .same("book", "its source's derived expression is declared again")
        .await;
    assert!(again.contains(r#""fees":"11""#), "{again}");
    assert_eq!((t.state("book"), t.records("book")), (WHOLE, 1));
    let both = [scaled(4), line(3)].map(|d| serde_json::from_value(d).expect("a declaration"));
    let applied = t
        .cat
        .apply(&t.owner, both.into())
        .await
        .expect("declaring both");
    let last = applied.position();
    let again = t
        .same("book", "both are declared again in one append")
        .await;
    assert!(
        again.contains(r#""fees":"13""#) && again.contains(r#""total":"28""#),
        "{again}"
    );
    assert_eq!(
        [Position(last.0 - 1), last].map(|at| t.state_at(Some(at), "book")),
        [(false, [0; 3]), WHOLE],
        "not whole between the two declarations: a read there computes"
    );
}

/// Each event, as its JSON, appended raw to a lake of its own that keeps
/// `book` over `line` and `above` over `book`, is refused, saying what its
/// `named` says. Both views, the state of `book` and the renames read as
/// before it.
async fn not_folded(bodies: &[(Value, &str)]) {
    for (body, named) in bodies {
        let what = &body["kind"];
        let mut declarations = vec![stored("line", &[("kind", "text"), ("net", "decimal")])];
        let total = [("total", "sum(net)")];
        declarations.extend(views("book", "line", &["kind"], json!({}), &total));
        for twin in ["book", "book_twin"] {
            let mut both = views("above", twin, &[], json!({}), &[("all", "sum(total)")]);
            declarations.push(both.remove(usize::from(twin != "book")));
        }
        declarations.extend(views("loose", "line", &[], json!({}), &[("n", "count()")]));
        let t = Kept::new(declarations).await;
        t.put("line", "a", json!({"kind": "k", "net": "3"})).await;
        t.set("view_kept.book", json!(true)).await;
        t.set("view_kept.above", json!(true)).await;
        let rest = |t: &Kept| format!("{:?} {:?}", t.state("book"), t.lake.renames(None));
        let seen = [t.same("book", "").await, t.same("above", "").await];
        let before = rest(&t);
        let folded = {
            let mut w = t.lake.writer();
            let body = serde_json::from_value(body.clone()).expect("an event");
            let draft = crate::log::Draft::system(t.lake.clock(&mut w), body);
            let folded = t.lake.append(&mut w, vec![draft]);
            folded.expect_err("not folded").to_string()
        };
        let own = &t.owner;
        let now = [t.read(own, "book").await, t.read(own, "above").await];
        assert_eq!((now, rest(&t)), (seen, before), "{what}: as it was");
        assert!(folded.contains(named), "{what}: {folded}");
    }
}

/// A log that declares a type under a name of the store's own, writes a
/// record of one or moves one into a set is not folded: no door appends
/// any, and it would change a view's kept record, or an extreme's order,
/// behind the fold.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_event_that_names_a_name_of_the_stores_own_is_not_folded() {
    let mut bodies = Vec::new();
    for k in ["sys:kept.book", "sys:order.book", "sys:else"] {
        let named = format!("`{k}`: a name that begins `sys:`");
        let declared = json!({"kind": "declared", "declaration": stored(k, &[]), "version": 1});
        let retired = json!({"kind": "record_retired", "type": k, "key": "k", "version": 2});
        let set = json!({"kind": "object_set_changed", "set": "s", "type": k, "key": "k", "joined": true});
        bodies.extend([declared, retired, set].map(|b| (b, named.clone())));
    }
    let bodies: Vec<(Value, &str)> = bodies
        .iter()
        .map(|(b, n)| (b.clone(), n.as_str()))
        .collect();
    not_folded(&bodies).await;
}

/// An output under a seam of the store's own is not folded: no door
/// records under one, and the fold would read it as renames, or as a kept
/// view's state, it never made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_output_under_a_seam_of_the_stores_own_is_not_folded() {
    let said = |s| format!("records under `{s}`: a seam that begins `sys:` is the store's own");
    let output = |seam, input, output| json!({"kind": "output_recorded", "seam": seam, "input": input, "output": output});
    let zero = json!(lakelands_api::event::Digest::zero());
    let renames = output(crate::lake::RENAMES, zero, json!([]));
    let book = json!(crate::backed::digest(&Value::from("book")));
    let state = json!({"whole": false, "not_exact": [3, 3], "differ": 3});
    let (r, k) = (said(crate::lake::RENAMES), said(KEPT));
    not_folded(&[(renames, &r), (output(KEPT, book, state), &k)]).await;
}

/// A record event, or a set's member, under a view's name is not folded:
/// a view's records are computed and no door writes one. Under a kept
/// inner view the record would reach the outer view as a member; under a
/// view not kept it would wait for the view to be declared a stored type.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_of_a_view_is_not_folded() {
    let said = |ty| format!("writes a record of `{ty}`, a view, whose records are computed");
    let write = |kind, ty, patch| json!({"kind": kind, "type": ty, "key": "j", "version": 1, "patch": patch, "made": true});
    let set = json!({"kind": "object_set_changed", "set": "s", "type": "book", "key": "j", "joined": true});
    let upserted = write("record_upserted", "book", json!({"total": "40"}));
    let ingested = write("record_ingested", "loose", json!({"n": "7"}));
    let (book, loose) = (said("book"), said("loose"));
    not_folded(&[(upserted, &book), (ingested, &loose), (set, &book)]).await;
}

/// A view without group fields is one record, kept over no member too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ungrouped_view_is_one_record_over_no_member_too() {
    let mut declarations = vec![stored("line", &[("net", "decimal")])];
    let measures = [("total", "sum(net)"), ("n", "count()")];
    declarations.extend(views("sum", "line", &[], json!({}), &measures));
    let t = Kept::new(declarations).await;
    t.set("view_kept.sum", json!(true)).await;
    assert_eq!((t.state("sum"), t.records("sum")), (WHOLE, 1));
    t.same("sum", "over no record").await;
    t.put("line", "a", json!({"net": "2.5"})).await;
    t.restrict("line", "a", true).await;
    t.same("sum", "its only member restricted").await;
    t.retire("line", "a").await;
    t.same("sum", "over no record again").await;
    assert_eq!((t.state("sum"), t.records("sum")), (WHOLE, 1));
}

/// A rebuilt projection keeps what the live one kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rebuild_keeps_the_view_as_the_live_fold_did() {
    let mut declarations = vec![stored("line", &[("kind", "text"), ("net", "decimal")])];
    declarations.extend(views(
        "book",
        "line",
        &["kind"],
        json!({}),
        &[("total", "sum(net)")],
    ));
    let t = Kept::new(declarations).await;
    t.put("line", "a", json!({"kind": "k", "net": "3"})).await;
    t.set("view_kept.book", json!(true)).await;
    t.put("line", "b", json!({"kind": "k", "net": "4"})).await;
    t.restrict("line", "a", true).await;
    let live = (
        t.same("book", "live").await,
        t.state("book"),
        t.records("book"),
    );
    t.store.rebuild().await.expect("rebuilding");
    let rebuilt = (
        t.same("book", "rebuilt").await,
        t.state("book"),
        t.records("book"),
    );
    assert_eq!(live, rebuilt);
}

/// An extreme whose holder leaves is read again in the fold of that
/// event, by its group's values: however many holders one append takes,
/// each group is exact after its own event and nothing waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extreme_whose_holder_left_is_read_again() {
    let source =
        |n: &str, kind: &str| json!({"name": n, "type": {"kind": kind}, "authority": "source"});
    let reading = json!({"declare": "type", "name": "reading",
        "fields": [source("site", "text"), source("value", "decimal")]});
    let mut declarations = vec![reading];
    let measures = [
        ("peak", "max(value)"),
        ("least", "min(value)"),
        ("n", "count()"),
    ];
    declarations.extend(views("peak", "reading", &["site"], json!({}), &measures));
    let t = Kept::new(declarations).await;
    t.set("view_kept.peak", json!(true)).await;
    let sites = || (0..12).map(|i| format!("s{i:02}"));
    t.feed(sites().map(|s| (format!("{s}-high"), s, "9.5")).collect())
        .await;
    t.feed(sites().map(|s| (format!("{s}-low"), s, "1.25")).collect())
        .await;
    t.same("peak", "two readings a site").await;
    assert_eq!(t.state("peak"), WHOLE);
    // One holder leaves: its group is read again in that transaction.
    t.retire("reading", "s00-high").await;
    let one = t.same("peak", "a holder is retired").await;
    assert_eq!(t.state("peak"), WHOLE, "{one}");
    // Every site's holder gets a worse value, in as few transactions as the
    // source's windows make of it.
    let worse = sites().skip(1).map(|s| (format!("{s}-high"), s, "0.5"));
    t.feed(worse.collect()).await;
    t.same("peak", "eleven holders left").await;
    assert_eq!(t.state("peak"), WHOLE, "each read again in its own event");
    assert_eq!(t.kept("peak").await, r#"{"state":"kept"}"#);
    assert!(t.read(&t.owner, "peak").await.contains("\"peak\":\"1.25\""));
}

/// A view over a kept view whose group is not exact computes, as its twin
/// does over the computed inner view, and is the kept one again when the
/// inner group is exact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_view_over_a_view_that_is_not_exact_computes() {
    let mut declarations = vec![stored(
        "line",
        &[("kind", "text"), ("net", "decimal"), ("by", "decimal")],
    )];
    let inner = [("rate", "sum(net) / sum(by)")];
    declarations.extend(views("book", "line", &["kind"], json!({}), &inner));
    for twin in ["book", "book_twin"] {
        let mut both = views("ledger", twin, &[], json!({}), &[("all", "sum(rate)")]);
        declarations.push(both.remove(usize::from(twin != "book")));
    }
    let t = Kept::new(declarations).await;
    t.set("view_kept.book", json!(true)).await;
    t.set("view_kept.ledger", json!(true)).await;
    t.put("line", "a", json!({"kind": "k", "net": "3", "by": "2"}))
        .await;
    t.put("line", "b", json!({"kind": "j", "net": "3", "by": "0"}))
        .await;
    assert_eq!(t.state("book"), (true, [1, 1, 0]), "an inner group fails");
    let said = t.kept("ledger").await;
    assert!(
        said.contains(r#""not_exact":1"#),
        "the outer view says so too: {said}"
    );
    let refused = t.same("ledger", "an inner group divides by zero").await;
    assert!(refused.contains("Refused"), "{refused}");
    t.put("line", "b", json!({"by": "4"})).await;
    let both = t.same("ledger", "the inner group evaluates again").await;
    assert!(both.contains("2.25"), "{both}");
}

/// A setting a measure's argument names is as one its `where` names: the
/// view is made again when it is written and when it is retired.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_setting_a_measure_names_makes_the_view_again() {
    let mut declarations = vec![stored("line", &[("kind", "text"), ("net", "decimal")])];
    let measures = [("worth", "sum(round(net * setting('fx'), 2))")];
    declarations.extend(views("book", "line", &["kind"], json!({}), &measures));
    let t = Kept::new(declarations).await;
    t.set("fx", json!(2)).await;
    t.put("line", "a", json!({"kind": "k", "net": "1.25"}))
        .await;
    t.set("view_kept.book", json!(true)).await;
    assert!(t.state("book").0, "a measure that names a setting is kept");
    let twice = t.same("book", "under the first rate").await;
    assert!(twice.contains("2.5"), "{twice}");
    t.set("fx", json!(3)).await;
    let thrice = t
        .same("book", "the setting its measure names is written")
        .await;
    assert!(thrice.contains("3.75"), "{thrice}");
    t.retire("sys.setting", "fx").await;
    t.same("book", "the setting is retired").await;
}

/// A `value_type_migrated` that names other types than those that use the
/// value type is not folded: only an imported or damaged log carries one,
/// and its records would be renamed in types the ontology does not name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_value_rename_that_names_other_types_is_not_folded() {
    for (types, folds) in [
        (json!([]), false),
        (json!(["other"]), false),
        (json!(["line", "other"]), false),
        (json!(["line"]), true),
    ] {
        let vt = json!({"declare": "value_type", "name": "kinds", "kind": "text",
                        "one_of": ["k", "j"]});
        let line = json!({"declare": "type", "name": "line", "fields": [
            {"name": "kind", "type": {"kind": "text", "value_type": "kinds"}}]});
        let t = Kept::new(vec![vt, line]).await;
        t.put("line", "a", json!({"kind": "k"})).await;
        let was = t.read(&t.owner, "line").await;
        let body = json!({"kind": "value_type_migrated", "renames": [["k", "j"]],
                          "report_id": "r", "version": 2, "types": types,
                          "declaration": {"name": "kinds", "kind": "text", "one_of": ["j"]}});
        let folded = {
            let mut w = t.lake.writer();
            let body = serde_json::from_value(body).expect("an event");
            let draft = crate::log::Draft::system(t.lake.clock(&mut w), body);
            t.lake.append(&mut w, vec![draft]).map(drop)
        };
        let now = t.read(&t.owner, "line").await;
        match folds {
            true => assert!(folded.is_ok() && now.contains(r#"{"kind":"j"}"#), "{now}"),
            false => {
                let why = folded.expect_err("not folded").to_string();
                assert!(
                    why.contains("in other types than those that use it"),
                    "{types}: {why}"
                );
                assert_eq!(now, was, "{types}: as it was");
            }
        }
    }
}

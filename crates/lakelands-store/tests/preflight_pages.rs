//! A migration's preflight reads a type a page at a time (lakelands#480):
//! the report, its id, the moves, the membership changes and their order,
//! and every refusal are what they were when it read the type whole.
//!
//! The checks run through the doors on the memory engine, behind a wrapper
//! (`probed`) that remembers the most rows one `scan` handed out and can
//! fail the n-th scan of a type. The types have more records than two pages hold,
//! with what a page boundary could lose placed on both sides of one.

mod probed;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use lakelands_api::engine::OpenMode;
use lakelands_api::error::Case;
use lakelands_api::event::EventBody;
use lakelands_api::location::Limits;
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::Migration;
use lakelands_api::{
    Error, LakeStore, Location, Position, Refusal, RelationName, TypeName, WriteOpts,
};
use lakelands_core::conformance::fixture::{Lake, object_set_on, patch};
use lakelands_engine_mem::MemoryOpener;
use probed::{Probe, ProbedOpener};
use serde_json::json;

/// A new lake on the probed memory engine, with `max_set_changes`.
async fn lake(name: &str, max_set_changes: u32) -> (Arc<dyn LakeStore>, Arc<Probe>) {
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        ..Seams::default()
    };
    let location = Location {
        limits: Limits {
            max_set_changes,
            ..Limits::default()
        },
        ..Location::memory(name)
    };
    let store = lakelands_store::open(location, seams, OpenMode::Create)
        .await
        .expect("a new lake");
    (store, probe)
}

/// `item` keyed by `key`, with `n` of `n` and `flag` of `flag`, its
/// reference to itself named `peer`, and its relation `owner`'s `rank` of
/// `rank`.
fn item(key: &str, [n, flag, rank]: [&str; 3], peer: &str) -> serde_json::Value {
    let text = |name: &str| json!({"name": name, "type": {"kind": "text"}, "required": true});
    json!({"name": "item", "key": [key],
           "fields": [text("a"), text("b"), text("c"),
                      {"name": "n", "type": {"kind": n}},
                      {"name": "flag", "type": {"kind": flag}},
                      {"name": peer, "type": {"kind": "reference", "target": "item"}}],
           "relations": [{"name": "owner", "target": "person", "cardinality": "many_to_many",
                          "fields": [{"name": "rank", "type": {"kind": rank}}]}]})
}

fn migration(to: serde_json::Value, renames: &[(&str, &str)]) -> Migration {
    Migration {
        to: serde_json::from_value(to).expect("a type"),
        renames: (renames.iter())
            .map(|(from, to)| ((*from).into(), (*to).into()))
            .collect(),
        relation_renames: Vec::new(),
    }
}

/// The kinds of `n`, `flag` and the link's `rank` as the fixture stores them.
const STORED: [&str; 3] = ["text", "text", "number"];

fn key(i: usize) -> String {
    format!("k{i:04}")
}

const ITEMS: usize = 2_100;

/// The fixture: `ITEMS` items keyed by `a` over more than two pages, a
/// person, a note that refers to an item, a link with a rank; then a
/// re-key by `b`, which moves item 3 to the end and leaves an ended key
/// in the first page. `c` collides for items 1500 and 1501; item 1600
/// holds an `n` that is no number; item 5 refers to item 3, and item 3 is
/// linked to the person; items 0 and 1 hold the flag.
async fn items(store: &dyn LakeStore) -> Lake {
    let l = Lake::of(store);
    let declared = |mut d: serde_json::Value| {
        d["declare"] = json!("type");
        d
    };
    for d in [
        json!({"name": "person", "key": ["name"],
               "fields": [{"name": "name", "type": {"kind": "text"}, "required": true}]}),
        item("a", STORED, "peer"),
        json!({"name": "note",
               "fields": [{"name": "about", "type": {"kind": "reference", "target": "item"}}]}),
    ] {
        l.declare_json(declared(d)).await.unwrap();
    }
    l.upsert("person", "x", json!({"name": "x"})).await.unwrap();
    for i in 0..ITEMS {
        let k = key(i);
        let flag = if i < 2 { "true" } else { "false" };
        let mut fields = json!({"a": k, "b": k, "c": k, "n": i.to_string(), "flag": flag});
        match i {
            3 => fields["b"] = json!("zz0003"),
            5 => fields["peer"] = json!(key(3)),
            1500 | 1501 => fields["c"] = json!("dup"),
            1600 => fields["n"] = json!("abc"),
            _ => {}
        }
        l.upsert("item", &k, fields).await.unwrap();
    }
    l.upsert("note", "n1", json!({"about": key(3)}))
        .await
        .unwrap();
    let (item_ty, owner) = ("item".into(), "owner".into());
    let rank = patch(json!({"rank": 2}));
    (l.cat)
        .link(
            &l.own,
            &item_ty,
            &key(3).as_str().into(),
            &owner,
            &"x".into(),
            rank,
            WriteOpts::default(),
        )
        .await
        .unwrap();
    let by_b = serde_json::from_value(item("b", STORED, "peer")).unwrap();
    l.migrate(by_b).await.unwrap();
    l
}

/// The `object_set_changed` events after `after`: set, type, key, joined.
async fn changes(store: &dyn LakeStore, after: Position) -> Vec<(String, String, String, bool)> {
    let only = FeedFilter {
        kinds: vec!["object_set_changed".into()],
        ..FeedFilter::default()
    };
    let polled = store
        .feed()
        .poll(after.next(), &only, 10_000)
        .await
        .unwrap();
    let change = |e: &lakelands_api::event::Event| match &e.body {
        EventBody::ObjectSetChanged {
            set,
            ty,
            key,
            joined,
        } => (set.clone(), ty.to_string(), key.to_string(), *joined),
        other => panic!("an `object_set_changed`, not {other:?}"),
    };
    polled.events.iter().map(change).collect()
}

/// The report of a migration that re-keys, converts, renames the type's
/// reference to itself and retypes a link field, over more than two pages
/// with an ended key, a record that does not convert and a collision: its
/// id is the one the preflight made when it read the type whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_report_and_its_id_are_what_a_whole_read_made() {
    let (store, probe) = lake("preflight-report", 20_000).await;
    let l = items(store.as_ref()).await;
    let kinds = ["number", "text", "text"];
    let m = migration(item("c", kinds, "partner"), &[("peer", "partner")]);
    probe.most.store(0, Ordering::SeqCst);
    let report = l.cat.migrate(&l.own, m).await.unwrap().data;
    assert!(
        probe.most.load(Ordering::SeqCst) <= 1_024,
        "the preflight read {} rows in one scan; it reads a page at a time",
        probe.most.load(Ordering::SeqCst)
    );
    let rekeyed: Vec<_> = (report.rekeyed.iter())
        .map(|r| (r.ty.as_str(), r.records, r.collisions, r.missing))
        .collect();
    let references: Vec<_> = (report.references.iter())
        .map(|r| (r.ty.as_str(), r.field.as_str(), r.values))
        .collect();
    let collisions: Vec<_> = (report.sample_collisions.iter())
        .map(|c| (c.ty.as_str(), c.key.as_str()))
        .collect();
    assert_eq!(
        (
            report.records_scanned,
            report.lossless,
            rekeyed,
            references,
            collisions
        ),
        (
            ITEMS as u64 + 1,
            false,
            vec![("item", 3, 2, 0), ("sys.link.item.owner", 1, 0, 0)],
            vec![
                ("item", "partner", 1),
                ("note", "about", 1),
                ("sys.link.item.owner", "from", 1)
            ],
            vec![("item", "dup")]
        ),
        "item 3 moves back, 1500 and 1501 move onto one key; item 5, the note and the link \
         name a moved key"
    );
    assert_eq!(
        report.report_id, "f965663fbe9d3f2d1c82b14243017432b6326c3a4d93f4ad1bb78022ad0639bc",
        "the report's id, as the preflight that read the type whole made it, of the report \
         with what the migration restates (lakelands#554), and of it and the head alone \
         (lakelands#644)"
    );
}

/// A record that moves onto the key of a record that keeps its key, a
/// page apart: the two collide, the migration is not lossless, and
/// applying it is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_move_onto_a_key_that_stays_collides_across_pages() {
    let (store, _) = lake("preflight-collision", 20_000).await;
    let l = Lake::of(store.as_ref());
    let t = |key: &str| {
        let text = |name: &str| json!({"name": name, "type": {"kind": "text"}, "required": true});
        json!({"name": "t", "key": [key], "fields": [text("a"), text("c")]})
    };
    let mut first = t("a");
    first["declare"] = json!("type");
    l.declare_json(first).await.unwrap();
    for i in 0..1_100 {
        let c = if i == 1_050 { key(2) } else { key(i) };
        l.upsert("t", &key(i), json!({"a": key(i), "c": c}))
            .await
            .unwrap();
    }
    let m = migration(t("c"), &[]);
    let report = l.cat.migrate(&l.own, m.clone()).await.unwrap().data;
    let rekeyed: Vec<_> = (report.rekeyed.iter())
        .map(|r| (r.records, r.collisions, r.missing))
        .collect();
    assert_eq!(
        (report.lossless, rekeyed),
        (false, vec![(1, 2, 0)]),
        "item 1050 moves onto item 2's key, which item 2 keeps: both collide"
    );
    let applied = l.cat.migrate_apply(&l.own, m, &report.report_id).await;
    assert!(
        applied.is_err(),
        "a migration that is not lossless is refused: {applied:?}"
    );
}

/// One applied migration that re-keys: the memberships it changes come in
/// the order of the migrated type, the changed link type, the referencing
/// type; a membership that changes only through the moves is decided with
/// them; and past `max_set_changes` the refusal names the set whose change
/// passed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memberships_are_decided_with_the_moves_and_in_their_order() {
    for (max, refused) in [(20_000, None), (3, Some("from_three"))] {
        let (store, probe) = lake(&format!("preflight-members-{max}"), max).await;
        let l = items(store.as_ref()).await;
        // `c` no longer collides, and every `n` is a number.
        for (i, fields) in [
            (1_501, json!({"c": "k1501x"})),
            (1_600, json!({"n": "1600"})),
        ] {
            l.upsert("item", &key(i), fields).await.unwrap();
        }
        for (set, of, filter) in [
            // Item 5 refers to item 3, which moves from `zz0003` to `k0003`:
            // it joins only once the reference is rewritten.
            ("to_three", "item", "peer = 'k0003'"),
            ("flagged", "item", "flag = 'true'"),
            // The link from item 3 moves with it and names its new key.
            ("from_three", "sys.link.item.owner", "from = 'k0003'"),
            ("about_three", "note", "about = 'k0003'"),
        ] {
            l.declare_as(object_set_on(set, of, filter), set)
                .await
                .unwrap();
        }
        let m = migration(item("c", ["number", "boolean", "text"], "peer"), &[]);
        let report = l.cat.migrate(&l.own, m.clone()).await.unwrap().data;
        assert!(report.lossless, "{report:?}");
        let head = store.head().await.unwrap();
        probe.most.store(0, Ordering::SeqCst);
        let applied = l.cat.migrate_apply(&l.own, m, &report.report_id).await;
        match refused {
            Some(set) => assert!(
                matches!(&applied, Err(Error::Refused { field: Some(f), reason: Refusal::TooLarge { max: m } })
                    if f.as_str() == set && *m == max),
                "past three changes, the migrated type's, the refusal names `{set}`: {applied:?}"
            ),
            None => {
                applied.unwrap();
                assert_eq!(changes(store.as_ref(), head).await, expected(CHANGES));
                assert!(probe.most.load(Ordering::SeqCst) <= 1_024);
            }
        }
    }
}

/// The changes of the migration above, in the order it appends them.
const CHANGES: &[(&str, &str, &str, bool)] = &[
    ("flagged", "item", "k0000", false),
    ("flagged", "item", "k0001", false),
    ("to_three", "item", "k0005", true),
    ("from_three", "sys.link.item.owner", "k0003:x", true),
    ("about_three", "note", "n1", true),
];

fn expected(changes: &[(&str, &str, &str, bool)]) -> Vec<(String, String, String, bool)> {
    let owned = |(s, t, k, j): &(&str, &str, &str, bool)| {
        ((*s).to_owned(), (*t).to_owned(), (*k).to_owned(), *j)
    };
    changes.iter().map(owned).collect()
}

/// A migration without a move decides memberships in its one walk.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_migration_without_moves_decides_in_its_one_walk() {
    let (store, _) = lake("preflight-no-moves", 20_000).await;
    let l = items(store.as_ref()).await;
    l.upsert("item", &key(1_600), json!({"n": "1600"}))
        .await
        .unwrap();
    let flagged = object_set_on("flagged", "item", "flag = 'true'");
    l.declare_as(flagged, "flagged").await.unwrap();
    let head = store.head().await.unwrap();
    let kinds = ["number", "boolean", "number"];
    let to = serde_json::from_value(item("b", kinds, "peer")).unwrap();
    l.migrate(to).await.unwrap();
    assert_eq!(changes(store.as_ref(), head).await, expected(NO_MOVES));
}

/// The changes of the migration above.
const NO_MOVES: &[(&str, &str, &str, bool)] = &[
    ("flagged", "item", "k0000", false),
    ("flagged", "item", "k0001", false),
];

/// A scan that fails on the type's second page fails the preflight: it
/// answers that error, not a report made of the first page.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_that_cannot_be_read_fails_the_preflight() {
    let (store, probe) = lake("preflight-failing-page", 20_000).await;
    let l = items(store.as_ref()).await;
    let m = migration(item("b", ["number", "text", "number"], "peer"), &[]);
    *probe.fail.lock().unwrap() = Some(("item".into(), 2));
    let failed = l.cat.migrate(&l.own, m).await;
    assert!(
        matches!(&failed, Err(Error::Unavailable { situation }) if situation.to_string().contains("planted")),
        "the preflight answers the failed read: {failed:?}"
    );
}

/// A relation narrowed to one link per target counts its live links a
/// page at a time: two live links on one target, a page apart, refuse it;
/// with one of them unlinked, the retired link beside the live one does
/// not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_narrowed_relation_counts_live_links_across_pages() {
    let (store, probe) = lake("preflight-narrowed", 20_000).await;
    let l = Lake::of(store.as_ref());
    let doc = |cardinality: &str| {
        json!({"name": "doc", "fields": [{"name": "n", "type": {"kind": "number"}}],
               "relations": [{"name": "seen_by", "target": "person", "cardinality": cardinality}]})
    };
    let mut first = doc("many_to_many");
    first["declare"] = json!("type");
    let person = json!({"declare": "type", "name": "person", "key": ["name"],
        "fields": [{"name": "name", "type": {"kind": "text"}, "required": true}]});
    for d in [person, first] {
        l.declare_json(d).await.unwrap();
    }
    let (ty, seen_by): (TypeName, RelationName) = ("doc".into(), "seen_by".into());
    // 1 100 links in key order, the first and the last to the same person.
    for i in 0..1_100 {
        l.upsert("doc", &key(i), json!({"n": i})).await.unwrap();
        let to = if i == 0 || i == 1_099 {
            "q".to_owned()
        } else {
            format!("p{i:04}")
        };
        l.upsert("person", &to, json!({"name": to})).await.unwrap();
        let (from, to) = (key(i).as_str().into(), to.as_str().into());
        let linked = (l.cat).link(
            &l.own,
            &ty,
            &from,
            &seen_by,
            &to,
            patch(json!({})),
            WriteOpts::default(),
        );
        linked.await.unwrap();
    }
    let narrowed = migration(doc("one_to_many"), &[]);
    probe.most.store(0, Ordering::SeqCst);
    let refused = l.cat.migrate(&l.own, narrowed.clone()).await;
    assert!(
        matches!(&refused, Err(Error::Refused { reason: Refusal::Constraint(said), .. })
            if matches!(said.case, Case::NarrowingBreaksLinks { count: 1, .. })),
        "two live links to `q`, a page apart, break the narrower rule: {refused:?}"
    );
    assert!(probe.most.load(Ordering::SeqCst) <= 1_024);
    let (from, to) = (key(1_099).as_str().into(), "q".into());
    let unlinked = (l.cat).unlink(&l.own, &ty, &from, &seen_by, &to, WriteOpts::default());
    unlinked.await.unwrap();
    let accepted = l.cat.migrate(&l.own, narrowed).await;
    assert!(
        accepted.is_ok(),
        "a retired link beside a live one breaks nothing: {accepted:?}"
    );
}

/// A value type's preflight reads each using type a page at a time, and
/// lists its first misfits as it did: a field's before the next field's,
/// each field's by key, ten in all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_value_types_preflight_lists_its_misfits_by_field_across_pages() {
    let (store, probe) = lake("preflight-value-type", 20_000).await;
    let l = Lake::of(store.as_ref());
    let kind = |declare: bool, one_of: &[&str]| {
        let mut d = json!({"name": "kind", "kind": "text", "one_of": one_of});
        if declare {
            d["declare"] = json!("value_type");
        }
        d
    };
    let field = |name: &str| json!({"name": name, "type": {"kind": "text", "value_type": "kind"}});
    l.declare_json(kind(true, &["new", "old"])).await.unwrap();
    l.declare_json(json!({"declare": "type", "name": "t", "fields": [field("p"), field("q")]}))
        .await
        .unwrap();
    // `p` holds `old` twice in the second page, `q` twelve times in the first.
    for i in 0..1_100 {
        let old = |is: bool| if is { "old" } else { "new" };
        let (p, q) = (old(i == 1_050 || i == 1_060), old((1..=12).contains(&i)));
        l.upsert("t", &key(i), json!({"p": p, "q": q}))
            .await
            .unwrap();
    }
    let to: lakelands_api::ontology::ValueTypeDecl =
        serde_json::from_value(kind(false, &["new"])).unwrap();
    probe.most.store(0, Ordering::SeqCst);
    let report = l
        .cat
        .migrate_value_type(&l.own, to.into())
        .await
        .unwrap()
        .data;
    let fields: Vec<_> = (report.fields.iter())
        .map(|u| (u.field.as_str(), u.values_scanned, u.misfits))
        .collect();
    assert_eq!(
        (report.records_scanned, report.misfits, report.fits, fields),
        (1_100, 14, false, vec![("p", 1_100, 2), ("q", 1_100, 12)])
    );
    let first: Vec<_> = (report.first.iter())
        .map(|m| format!("{} {}", m.field, m.key))
        .collect();
    let listed = ["p k1050", "p k1060"].into_iter().map(str::to_owned);
    let listed: Vec<String> = listed
        .chain((1..=8).map(|i| format!("q {}", key(i))))
        .collect();
    assert_eq!(first, listed, "`p`'s misfits, a page on, come before `q`'s");
    assert_eq!(
        report.report_id, "0961385c926416d1e66063156210881f5f662d30981a0ae0bb4bb4e088a127fa",
        "the report's id, as the preflight that read the type whole made it"
    );
    assert!(
        probe.most.load(Ordering::SeqCst) <= 1_024,
        "the preflight read {} rows in one scan; it reads a page at a time",
        probe.most.load(Ordering::SeqCst)
    );
}

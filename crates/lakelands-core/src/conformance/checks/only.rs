//! A grant may admit only some records of a type
//! (`docs/decisions/051-a-grant-may-admit-only-some-records.md`).
//!
//! `only` on a type admits the records whose fields hold its values at
//! every door that reads records; its holder writes those records alone,
//! another's being to it a key that is free, and changes no shape.

use std::fmt::Debug;

use lakelands_api::error::{Case, Missing, Said};
use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::principal::PrincipalKind;
use lakelands_api::query::{Hits, QueryResult};
use lakelands_api::read::Found;
use lakelands_api::write::Outcome;
use lakelands_api::{
    Error, LakeStore, Position, Principal, ProposalId, RecordKey, Refusal, RelationName, TypeName,
    Value, VerbName,
};
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::conformance::backing::EXECUTOR;
use crate::conformance::fixture::{
    Lake, Must, alike_renamed, field, head_unmoved, principal, reason, restrict, searchable,
    type_decl, vector, view_of,
};
use crate::conformance::{Failure, Held, Store};

check!(
    ONLY_READS = a_grant_admits_only_its_records_at_every_read,
    "051: a principal whose grant holds `only` on a type is answered only the records whose fields hold its values: `get` of another's record is not found, `find` with a filter, an order, `total`, a count, a sum and a group count and sum its own alone, a read as of a position judges the records as they stood then, and a count of a kept view under its pairs, at the head and as of earlier, counts its own group"
);

check!(
    ONLY_SEARCH = a_search_finds_only_admitted_records,
    "051: a lexical, a vector and a hybrid search by a holder of `only` find only its admitted records, however they rank; a search as of a position keeps a candidate only where it is admitted then and at the head, a link by its ends, under the grant followed to the head, so a field renamed since changes nothing"
);

check!(
    ONLY_VIEWS = a_view_answers_a_holder_its_own_groups,
    "051: a view grouped by every field a source's `only` names answers its holder the groups it is admitted, kept or not; one that is not is computed from the admitted records alone, a view over it too; `only` on a view admits its groups by their fields; `describe_type` says `only some` of such a view to its holder; a pair on a kept view's measure admits a holder of the `restricted` grant only the groups whose whole face holds it too, through `find`, `total` and `get` (lakelands#628)"
);

check!(
    ONLY_LINKS = a_link_is_answered_where_its_ends_are_admitted,
    "051: a walk by a holder of `only` reaches only admitted records; a link type whose end type holds `only` answers `get`, `find`, `total` and a count over the links whose ends are admitted, a reference to any other answers as one to a link no record holds, and a call proposed on one, of a verb that writes it or reaches outside the lake, is listed without its write and is not its reviewer's to accept or to reject (lakelands#871); `only` on a link type admits its links by their fields in a walk too"
);

check!(
    ONLY_WRITES = a_holder_of_only_writes_its_own_records_alone,
    "051: a holder of `only` changes, makes, retires and makes again under its version, links, unlinks, ingests and calls a low-risk verb on its own records; another's record is to its write a key that is free, so a probe under a version, a missing required field, a disagreeing key field and a link ingested from it answer as on a free key, and another's retired key is made again, its version continued, `retire`, `unlink` and a verb answer not found, and a write that would pass, a handover and a re-delivery onto another's key are not granted, appending nothing; a link's cardinality names the other end only where its writer is answered it, restricted or not, and the link that holds, which `only` on the link type itself keeps from it at either end and a hold on the link record keeps from one without `restricted`, and holds, over the lake and the window, for a link that `only` on its link type admits by a field its source sends; an aging rule stays refused; a proposal on another's record, retired or not, is listed without its write, and one on a record held from the holder is not listed (lakelands#1008); a retry under a key used before the grant held answers as before; a reference into the type from another type, through `upsert`, `propose` or `ingest`, must name an admitted record"
);

check!(
    ONLY_SCHEMA = a_holder_of_only_changes_no_shape,
    "051: `declare`, `apply`, `recompute`, `add_field`, `migrate`, `migrate_apply`, `migrate_value_type`, `migrate_value_type_apply`, the proposal of a declaration or a migration of a type or a value type, and `accept` and `reject` of a schema proposal are refused to a principal holding `only` on any type, appending nothing, and a migration's proposal, of a type or a value type, is not listed to it"
);

check!(
    ONLY_GRANT_CHECKED = a_grant_that_cannot_be_applied_is_refused,
    "051: `describe_type` tells a holder its `only`, offers its writes, the type's links and its verbs but one above low risk that assigns a field its `only` names, and an upsert only where it writes every field its `only` names; `only` on a field the type does not declare, of another kind than text or reference, with a value that is not text, or on a field the holder may not read, refuses the doors that name the type as invalid, naming the type and never the field; the grant follows a rename of its field, and after the field is dropped a read at the head is refused while one as of before is answered; a walk or a view that reaches a type whose grant cannot be applied admits none of its records"
);

/// A value of the door API from its JSON.
pub(super) fn de<T: DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).expect("the check's JSON reads")
}

/// A principal `id` with the grants given as JSON.
pub(super) fn holder(store: &dyn LakeStore, id: &str, grants: Value) -> Principal {
    principal(store, PrincipalKind::Agent, id, de(grants))
}

/// `alice` reads every type and is admitted only her own claims; `more`
/// adds to her grants.
pub(super) fn alice(store: &dyn LakeStore, more: Value) -> Principal {
    let mut grants = json!({"default": "read", "types": {"claim": {"only": {"owner": "alice"}}}});
    if let (Some(g), Value::Object(more)) = (grants.as_object_mut(), more) {
        g.extend(more);
    }
    holder(store, "alice", grants)
}

/// `trip` t1; `claim` c1 (alice, 10, taxi), c2 (bob, 20, taxi) and c3
/// (alice, 5, hotel), each with a vector and linked to t1 over `on_trip`;
/// the views `spend`, the sum by owner, and `all_spend`, the sum of every
/// claim, both kept, where `views`. Answers the head.
pub(super) async fn claims(l: &Lake, views: bool) -> Result<Position, Failure> {
    l.declare(type_decl("trip", vec![field("name", Text)]))
        .await?;
    l.declare(claim_with("owner")).await?;
    if views {
        let sum = [("total", "sum(amount)")];
        let by = vec![field("owner", Text), field("total", Number)];
        l.declare(view_of("spend", "claim", &["owner"], by, &sum))
            .await?;
        let all = vec![field("total", Number)];
        l.declare(view_of("all_spend", "claim", &[], all, &sum))
            .await?;
        for view in ["spend", "all_spend"] {
            l.set(&format!("view_kept.{view}"), json!(true)).await?;
        }
    }
    l.upsert("trip", "t1", json!({"name": "rome"})).await?;
    let mut head = Position::GENESIS;
    for (key, owner, amount, note, v) in [
        ("c1", "alice", 10, "taxi", [1.0, 0.0]),
        ("c2", "bob", 20, "taxi", [1.0, 0.1]),
        ("c3", "alice", 5, "hotel", [0.0, 1.0]),
    ] {
        let fields = json!({"owner": owner, "amount": amount, "note": note, "v": v});
        l.upsert("claim", key, fields).await?;
        let (ty, key, on, t1) = names(key);
        let (label, opts) = (de(json!({"label": "trip"})), Default::default());
        let linked = l.cat.link(&l.own, &ty, &key, &on, &t1, label, opts).await;
        head = linked.must("linking a claim to `t1`")?.position();
    }
    Ok(head)
}

/// `claim`, the claim `key`, `on_trip` and `t1`, as the doors name them.
pub(super) fn names(key: &str) -> (TypeName, RecordKey, RelationName, RecordKey) {
    ("claim".into(), key.into(), "on_trip".into(), "t1".into())
}

/// `who`'s `get` of the record `key` of `ty` at `at`.
pub(super) async fn get(
    l: &Lake,
    who: &Principal,
    ty: &str,
    key: &str,
    at: Option<Position>,
) -> lakelands_api::Result<lakelands_api::Reply<lakelands_api::Record>> {
    let get = de(json!({"key": key, "as_of": at}));
    l.cat.get(who, &ty.into(), get).await
}

/// `claim` as [`claims`] declares it, its owner field named `owner`.
pub(super) fn claim_with(owner: &str) -> lakelands_api::ontology::TypeDecl {
    let fields = vec![
        field(owner, Text),
        field("amount", Number),
        searchable("note"),
        vector("v", 2),
    ];
    let mut claim = type_decl("claim", fields);
    let label = json!({"name": "label", "type": {"kind": "text"}, "searchable": true});
    claim.relations = vec![de(
        json!({"name": "on_trip", "target": "trip", "fields": [label]}),
    )];
    claim
}

/// Migrates `claim` to `to` as the owner, `owner` renamed to the first
/// field of `to` where `renamed`; answers the migration's position.
async fn migrate(
    l: &Lake,
    to: lakelands_api::ontology::TypeDecl,
    renamed: bool,
) -> Result<Position, Failure> {
    let mut m = crate::conformance::fixture::migration(to);
    if renamed {
        m.renames = vec![("owner".into(), m.to.fields[0].name.clone())];
    }
    let report = l.cat.migrate(&l.own, m.clone());
    let report = report.await.must("previewing a migration of `claim`")?;
    let done = l.cat.migrate_apply(&l.own, m, &report.data.report_id).await;
    Ok(done.must("migrating `claim`")?.position())
}

/// What a `find` answered, short: the keys, and the total where asked; or
/// each group's fields and values, numbers as numbers.
fn short(found: &Found) -> String {
    let value = |v: &Value| match v {
        Value::Number(n) => n.as_f64().map_or(n.to_string(), |f| f.to_string()),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    match found {
        Found::Records { rows, total, .. } => {
            let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
            let total = total.map(|t| format!(" total={t}")).unwrap_or_default();
            format!("{}{total}", keys.join(" "))
        }
        Found::Groups { rows } => {
            let group = |g: &lakelands_api::read::Group| {
                let pairs = g.by.iter().chain(&g.values);
                let pairs: Vec<String> = pairs.map(|(f, v)| format!("{f}={}", value(v))).collect();
                pairs.join(" ")
            };
            rows.iter().map(group).collect::<Vec<_>>().join("; ")
        }
    }
}

/// The keys of a search's hits, best first.
pub(super) fn hit_keys(hits: &Hits) -> String {
    let keys: Vec<&str> = hits.hits.iter().map(|h| h.record.key.as_str()).collect();
    keys.join(" ")
}

/// The keys of a walk's nodes of `ty`, in order.
pub(super) fn node_keys(result: &QueryResult, ty: &str) -> String {
    let QueryResult::Graph { nodes, .. } = result else {
        return format!("{result:?}");
    };
    let keys = nodes.iter().filter(|n| n.ty.as_str() == ty);
    keys.map(|n| n.key.as_str()).collect::<Vec<_>>().join(" ")
}

/// Fails unless each `find` of `rows`, one a line `type | what | find |
/// answer`, answers `who` as its line says; `BEFORE` in a find is `before`.
pub(super) async fn finds(l: &Lake, who: &Principal, rows: &str, before: Position) -> Held {
    let mut wrong = Vec::new();
    for row in rows.lines().map(str::trim).filter(|r| !r.is_empty()) {
        let [ty, what, find, want] = row.split(" | ").collect::<Vec<_>>()[..] else {
            panic!("a row is four cells: {row}");
        };
        let find = de(
            serde_json::from_str(&find.replace("BEFORE", &before.0.to_string()))
                .expect("a find reads"),
        );
        let found = l.cat.find(who, &ty.into(), find).await;
        let said = found.map_or_else(|e| e.to_string(), |f| short(&f.data));
        if said != want {
            wrong.push(format!("{what} on `{ty}`: `{said}`, not `{want}`"));
        }
    }
    let who = &who.id;
    ensure!(
        wrong.is_empty(),
        "`{who}` must be answered only what its grant admits: {wrong:?}"
    );
    Ok(())
}

/// Whether `e` says that the grant's `only` on `claim` cannot be applied.
fn unappliable(e: &Error) -> bool {
    matches!(e.case(), Some(Said { case: Case::OnlyUnappliable { ty }, .. }) if ty == "claim")
}

/// Fails unless `answer` is refused as not granted, naming no field.
pub(super) fn not_granted<T: Debug>(what: &str, answer: lakelands_api::Result<T>) -> Held {
    let (field, reason) = (None, Refusal::NotGranted);
    let refused = Error::Refused { field, reason };
    ensure!(
        answer.as_ref().err() == Some(&refused),
        "{what} by a holder of `only` must be refused as not granted, naming no field; it answered {answer:?}"
    );
    Ok(())
}

/// Fails unless `answer` is not found, as for a record that is not there.
pub(super) fn not_found<T: Debug>(what: &str, answer: lakelands_api::Result<T>) -> Held {
    let missing = matches!(answer, Err(Error::NotFound { .. }));
    ensure!(missing, "{what} must be not found; it answered {answer:?}");
    Ok(())
}

const READS: &str = r#"
    claim | every record | {} | c1 c3
    claim | a filter | {"where": "amount > 1"} | c1 c3
    claim | a filter the store evaluates | {"where": "amount > 1 and now() > '2000-01-01'"} | c1 c3
    claim | an order | {"order_by": [{"field": "amount", "dir": "desc"}]} | c1 c3
    claim | a total | {"total": true, "limit": 1} | c1 total=2
    claim | a count | {"aggregate": {"measures": [{"name": "n", "func": "count"}]}} | n=2
    claim | a sum | {"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "amount"}]}} | s=15
    claim | a group | {"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "amount"}], "group_by": ["owner"]}} | owner=alice s=15
"#;

const HANDED: &str = r#"
    claim | as of before | {"as_of": BEFORE} | c1 c3
    claim | at the head | {} | c1
    spend | a kept view's total | {"total": true} | alice total=1
    spend | a kept view's total as of before | {"total": true, "as_of": BEFORE} | alice total=1
    spend | a kept view's filtered count | {"where": "total > 0", "total": true} | alice total=1
"#;

async fn a_grant_admits_only_its_records_at_every_read(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    claims(&l, true).await?;
    let alice = alice(store.as_ref(), json!({}));
    let get = |key: &'static str, at: Option<Position>| get(&l, &alice, "claim", key, at);
    not_found("alice's `get` of bob's `c2`", get("c2", None).await)?;
    get("c1", None).await.must("alice's `get` of her `c1`")?;
    finds(&l, &alice, READS, Position::GENESIS).await?;
    // The owner hands `c3` to bob: it is alice's as of before, not after.
    let handed = l.upsert("claim", "c3", json!({"owner": "bob"})).await?;
    let before = Position(handed.position().0 - 1);
    let fut = get("c3", Some(before));
    fut.await.must("alice's `get` of `c3` as of before")?;
    not_found("alice's `get` of `c3` at the head", get("c3", None).await)?;
    finds(&l, &alice, HANDED, before).await
}

const SEARCHES: &str = r#"
    lexical | {"mode": "lexical", "text": "taxi"} | c1 | c1
    vector | {"mode": "vector", "field": "v", "vector": [1.0, 0.0]} | c1 c3 | c1
    hybrid | {"mode": "hybrid", "text": "taxi", "field": "v", "vector": [1.0, 0.0]} | c1 c3 | c1
"#;

async fn a_search_finds_only_admitted_records(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    claims(&l, false).await?;
    // With `restricted`, a fused search at the head would rank unread.
    let alice = alice(store.as_ref(), json!({"restricted": true}));
    let (cat, alice) = (&l.cat, &alice);
    let search = |what: &str, mode: &str, at: Option<Position>| {
        let mut s: Value = serde_json::from_str(mode).expect("a search reads");
        (s["type"], s["as_of"]) = (json!("claim"), json!(at));
        let what = format!("alice's {what} search as of {at:?}");
        async move { Ok::<_, Failure>(cat.search(alice, de(s)).await.must(&what)?.data) }
    };
    let rows = SEARCHES.lines().map(str::trim).filter(|r| !r.is_empty());
    let rows: Vec<Vec<&str>> = rows.map(|r| r.split(" | ").collect()).collect();
    let mut wrong = Vec::new();
    for row in &rows {
        let hits = search(row[0], row[1], None).await?;
        if hit_keys(&hits) != row[2] {
            wrong.push(format!(
                "{}: `{}`, not `{}`",
                row[0],
                hit_keys(&hits),
                row[2]
            ));
        }
        // Bob's `c2` moves no score: `c3` is second of alice's vectors.
        let c3 = hits.hits.iter().find(|h| h.record.key.as_str() == "c3");
        if row[0] == "hybrid" && c3.is_none_or(|h| h.score != 1.0 / 61.0) {
            wrong.push(format!("hybrid: `c3` scores {c3:?}, not 1/61"));
        }
    }
    // `c3` handed to bob, then `owner` renamed: as of before the handover
    // `c3` is alice's then and not at the head; `c1` is hers in both.
    let handed = l.upsert("claim", "c3", json!({"owner": "bob"})).await?;
    let before = Position(handed.position().0 - 1);
    migrate(&l, claim_with("holder"), true).await?;
    for row in &rows {
        let hits = search(row[0], row[1], Some(before)).await?;
        if hit_keys(&hits) != row[3] {
            wrong.push(format!(
                "{} as of before: `{}`, not `{}`",
                row[0],
                hit_keys(&hits),
                row[3]
            ));
        }
    }
    // A link is judged by its ends at the head too.
    let link = json!({"type": "sys.link.claim.on_trip", "mode": "lexical", "text": "trip",
                      "as_of": before});
    let hits = cat.search(alice, de(link));
    let hits = hits.await.must("alice's search of the links")?;
    if hit_keys(&hits.data) != "c1:t1" {
        wrong.push(format!(
            "the links as of before: `{}`",
            hit_keys(&hits.data)
        ));
    }
    ensure!(
        wrong.is_empty(),
        "alice must find only her own claims: {wrong:?}"
    );
    Ok(())
}

const VIEWS: &str = r#"
    spend | a kept view grouped by `owner` | {} | alice
    all_spend | a kept view of every claim | {"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "total"}]}} | s=15
    top | a view over a view | {"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "total"}]}} | s=15
"#;

const HELD_VIEWS: &str = r#"
    spend | `only` on a kept view | {} | bob
    by_owner | `only` on a view | {} | bob
    top | a view over a view held to bob | {"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "total"}]}} | s=20
"#;

async fn a_view_answers_a_holder_its_own_groups(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    claims(&l, true).await?;
    let by = vec![field("total", Number)];
    l.declare(view_of("top", "spend", &[], by, &[("total", "sum(total)")]))
        .await?;
    let by_owner = crate::conformance::fixture::count_view("by_owner", "claim", "owner");
    l.declare(by_owner).await?;
    let alice = alice(store.as_ref(), json!({}));
    finds(&l, &alice, VIEWS, Position::GENESIS).await?;
    not_found(
        "bob's group of `spend` to alice",
        get(&l, &alice, "spend", "bob", None).await,
    )?;
    for view in ["spend", "all_spend"] {
        let told = l
            .cat
            .describe_type(&alice, &view.into(), Default::default())
            .await;
        let kept = told.must("describing a view to alice")?.data.lifecycle.kept;
        let why = lakelands_api::describe::NotKept::OnlySome;
        let only_some = Some(lakelands_api::describe::Kept::Computed { why });
        ensure!(
            kept == only_some,
            "`{view}` must be `only some` to alice; it is {kept:?}"
        );
    }
    // `only` on the view itself admits its groups by their fields.
    let bob = json!({"only": {"owner": "bob"}});
    let types = json!({"spend": bob, "by_owner": bob});
    let carol = holder(
        store.as_ref(),
        "carol",
        json!({"default": "read", "types": types}),
    );
    finds(&l, &carol, HELD_VIEWS, Position::GENESIS).await?;
    // alice's `tier` is `gold` on the open face and `std` on the whole one,
    // where her restricted claim is negative.
    let tier = [("tier", "if(sum(amount) > 12, 'gold', 'std')")];
    let by = vec![field("owner", Text), field("tier", Text)];
    l.declare(view_of("tiers", "claim", &["owner"], by, &tier))
        .await?;
    l.set("view_kept.tiers", json!(true)).await?;
    l.upsert("claim", "c4", json!({"owner": "alice", "amount": -10}))
        .await?;
    restrict(store.as_ref(), "claim", "c4", "a refund").await?;
    let types = json!({"tiers": {"only": {"tier": "gold"}}});
    let grants = json!({"default": "read", "restricted": true, "types": types});
    let erin = holder(store.as_ref(), "erin", grants);
    finds(&l, &erin, GOLD, Position::GENESIS).await?;
    let got = get(&l, &erin, "tiers", "alice", None).await;
    not_found(
        "alice's group of `tiers`, `std` on the whole face, to erin",
        got,
    )
}

const GOLD: &str = r#"
    tiers | `only` on a measure of the whole face | {} | bob
    tiers | its total | {"total": true} | bob total=1
"#;

const LINKS: &str = r#"
    sys.link.claim.on_trip | the links | {} | c1:t1 c3:t1
    sys.link.claim.on_trip | their total | {"total": true} | c1:t1 c3:t1 total=2
    sys.link.claim.on_trip | their count | {"aggregate": {"measures": [{"name": "n", "func": "count"}]}} | n=2
"#;

async fn a_link_is_answered_where_its_ends_are_admitted(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    claims(&l, false).await?;
    let walk = json!({"start": {"type": "trip", "key": "t1"},
                      "steps": [{"relation": "on_trip", "walk": "in"}], "project": {"as": "graph"}});
    let cat = &l.cat;
    let walked = |who: Principal, want: &'static str| {
        let walk = de(walk.clone());
        async move {
            let walked = cat.query(&who, walk).await.must("a walk from `t1`")?;
            let reached = node_keys(&walked.data, "claim");
            let who = &who.id;
            ensure!(
                reached == want,
                "`{who}`'s walk from `t1` must reach `{want}`; it reached `{reached}`"
            );
            Ok::<_, Failure>(())
        }
    };
    let writer = alice(store.as_ref(), json!({"default": "write", "review": true}));
    let alice = alice(store.as_ref(), json!({}));
    walked(alice.clone(), "c1 c3").await?;
    finds(&l, &alice, LINKS, Position::GENESIS).await?;
    let link = get(&l, &alice, "sys.link.claim.on_trip", "c2:t1", None).await;
    not_found("the link from bob's `c2` to alice", link)?;
    // A reference to it answers her as one to a link that is not there
    // (lakelands#871).
    let memo = crate::conformance::fixture::reference("l", "sys.link.claim.on_trip");
    l.declare(type_decl("memo", vec![memo])).await?;
    let (memo, m1, mut said) = ("memo".into(), "m1".into(), Vec::new());
    for k in ["c2:t1", "c9:t1"] {
        let up = cat.upsert(&writer, &memo, &m1, de(json!({"l": k})), Default::default());
        said.push(format!("{:?}", up.await.err()).replace(k, "?"));
    }
    ensure!(
        said[0] == said[1] && said[0] != "None",
        "a reference to the link of bob's `c2` answers alice as one to no link: {said:?}"
    );
    // And a call proposed on it, of a verb that writes it or reaches
    // outside the lake, is listed to her as the retirement of bob's `c2`
    // is, without its write.
    let on = "sys.link.claim.on_trip";
    let mark = json!({"declare": "verb", "name": "mark", "on": on, "risk": "low",
        "effect": "write", "set": [{"field": "label", "value": "'seen'"}]});
    let chain = json!({"declare": "verb", "name": "chain", "on": on, "risk": "low",
        "effect": "external", "executor": EXECUTOR});
    l.declare_json(mark).await?;
    l.declare_json(chain).await?;
    for w in [
        json!({"write": "retire", "type": "claim", "key": "c2"}),
        json!({"write": "call", "verb": "mark", "key": "c2:t1", "payload": {}}),
        json!({"write": "call", "verb": "chain", "key": "c2:t1", "payload": {}}),
    ] {
        let proposed = cat.propose(&l.own, de(w), "the owner's".into()).await;
        proposed.must("the owner's proposal")?;
    }
    let listed = cat.proposals(&writer, Default::default()).await;
    let listed = listed.must("alice's listing")?.data;
    // Nor does she decide either: a reviewer decides what it is listed whole.
    let mut kept = Vec::new();
    for p in &listed {
        let accepted = cat.accept(&writer, &p.id).await;
        let rejected = cat.reject(&writer, &p.id, "no".into()).await;
        kept.push(reason(&accepted) == Some(&Refusal::NotGranted));
        kept.push(reason(&rejected) == Some(&Refusal::NotGranted));
    }
    ensure!(
        kept == [true; 6] && listed.iter().all(|p| p.write.is_none() && p.more),
        "a proposal on bob's claim and two on its link are listed to alice without their writes, and she accepts and rejects none ({kept:?}): {listed:?}"
    );
    // `only` on the link type itself admits its links by their fields.
    let types = json!({"sys.link.claim.on_trip": {"only": {"from": "c2"}}});
    walked(
        holder(
            store.as_ref(),
            "dave",
            json!({"default": "read", "types": types}),
        ),
        "c2",
    )
    .await
}

/// What a link answered, short where a relation's cardinality refused
/// it: the other end its case names, or that the window links already.
fn link_said<T>(answer: lakelands_api::Result<T>) -> String {
    let case = answer.as_ref().err().and_then(Error::case);
    match case.map(|s| s.case.clone()) {
        Some(Case::LinkedAlreadyFrom { to, .. }) => to.map_or_else(
            || "linked to no end".to_owned(),
            |k| format!("linked to `{k}`"),
        ),
        Some(Case::LinkedAlreadyTo { from, .. }) => from.map_or_else(
            || "linked from no end".to_owned(),
            |k| format!("linked from `{k}`"),
        ),
        Some(Case::WindowLinksAlready { .. }) => "linked in the window".to_owned(),
        _ => said(answer),
    }
}

/// What a write answered, short: `applied`, `not found`, or its refusal.
pub(super) fn said<T>(answer: lakelands_api::Result<T>) -> String {
    match answer {
        Ok(_) => "applied".to_owned(),
        Err(Error::NotFound { .. }) => "not found".to_owned(),
        Err(e) => e.to_string(),
    }
}

async fn a_holder_of_only_writes_its_own_records_alone(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    claims(&l, false).await?;
    let mut memo = crate::conformance::fixture::reference("claim", "claim");
    memo.authority = lakelands_api::ontology::Authority::Shared;
    l.declare(type_decl("memo", vec![memo])).await?;
    let shared = json!({"kind": "text"});
    l.declare_json(json!({"declare": "type", "name": "receipt", "fields": [
        {"name": "owner", "type": shared, "authority": "shared"},
        {"name": "sum", "type": {"kind": "number"}, "authority": "shared"}],
        "relations": [{"name": "for_trip", "target": "trip", "authority": "source"}]}))
        .await?;
    let number = json!({"kind": "number"});
    l.declare_json(
        json!({"declare": "type", "name": "slip", "key": ["code"], "fields": [
        {"name": "owner", "type": shared}, {"name": "sum", "type": number, "required": true},
        {"name": "code", "type": shared}]}),
    )
    .await?;
    let held_by = json!({"name": "held_by", "target": "claim", "cardinality": "many_to_one"});
    l.declare_json(json!({"declare": "type", "name": "desk",
        "fields": [{"name": "n", "type": {"kind": "text"}}], "relations": [held_by]}))
        .await?;
    for (name, risk) in [("pay", "low"), ("propose_pay", "high")] {
        let set = json!([{"field": "note", "value": "'paid'"}]);
        let verb = json!({"declare": "verb", "name": name, "on": "claim", "risk": risk,
                          "effect": "write", "set": set});
        l.declare_json(verb).await?;
    }
    // Bob's receipt `r2` and its link to `t1`, his slip `s2`; his
    // receipt `r3` and claim `c4`, retired.
    l.upsert("trip", "t5", json!({"name": "oslo"})).await?;
    for (ty, key) in [
        ("receipt", "r2"),
        ("slip", "s2"),
        ("receipt", "r3"),
        ("claim", "c4"),
    ] {
        let n = if ty == "claim" { "amount" } else { "sum" };
        l.upsert(ty, key, json!({"owner": "bob", n: 1})).await?;
    }
    for (ty, key) in [("receipt", "r3"), ("claim", "c4")] {
        l.retire(ty, key).await?;
    }
    let for_trip = crate::links::link_type_name(&"receipt".into(), &"for_trip".into());
    let link = |from: &str, to: &str| {
        let key = crate::links::link_key(&from.into(), &to.into());
        let fields = json!({"from": from, "to": to});
        de(json!({"type": for_trip.as_str(), "key": key.as_str(), "fields": fields}))
    };
    let fut = l.ingest("feed", vec![link("r2", "t1")]);
    fut.await.must("ingesting bob's link of `r2`")?;
    let (desk, held_by) = (TypeName::from("desk"), RelationName::from("held_by"));
    for d in ["d1", "d2"] {
        l.upsert("desk", d, json!({"n": d})).await?;
    }
    let (d1, d2) = (RecordKey::from("d1"), RecordKey::from("d2"));
    let (ty, c1, on, t1) = names("c1");
    let (none, opts) = (Default::default, Default::default);
    let c2 = RecordKey::from("c2");
    let fut = cat.link(&l.own, &desk, &d2, &held_by, &c2, none(), opts());
    fut.await.must("linking `d2` to bob's `c2`")?;
    // Bob proposes a change to his claim and a claim of his; alice writes
    // `c1` under a key before her grant holds `only`, and a retry answers
    // as that did.
    let bob = holder(store.as_ref(), "bob", json!({"default": "write"}));
    let proposed = cat
        .call(&bob, &"propose_pay".into(), Some(&c2), none(), opts())
        .await;
    let make = json!({"write": "upsert", "type": "claim", "key": "c5", "patch": {"owner": "bob"}});
    let made = cat.propose(&bob, de(make), "bob's".into()).await;
    let mut proposals = Vec::new();
    for outcome in [proposed, made] {
        let Outcome::Proposed { proposal, .. } = outcome.must("bob's proposal")? else {
            return Err(Failure::new("bob's writes must be proposed"));
        };
        proposals.push(proposal);
    }
    let keyed = de::<lakelands_api::write::WriteOpts>(json!({"idempotency_key": "k1"}));
    let free = holder(store.as_ref(), "alice", json!({"default": "write"}));
    let mine = || de(json!({"note": "mine"}));
    let first = cat.upsert(&free, &ty, &c1, mine(), keyed.clone()).await;
    let first = first.must("alice's write before her grant holds `only`")?;
    let hers = json!({"only": {"owner": "alice"}});
    let types = json!({"claim": hers, "receipt": hers, "slip": hers});
    let more = json!({"default": "write", "review": true, "ingest": true, "types": types});
    let alice = alice(store.as_ref(), more);
    let again = cat.upsert(&alice, &ty, &c1, mine(), keyed).await;
    let again = again.must("alice's retry under the same key")?;
    ensure!(
        again == first,
        "a retry must answer as its first call did, {first:?}; it answered {again:?}"
    );
    // An aging rule stays refused to her.
    let head = store.head().await.must("reading the head")?;
    let verb = VerbName::from("pay");
    let rule = json!({"on": "claim", "field": "note", "from": "taxi", "to": "cab", "when": "true"});
    let rules = TypeName::from(lakelands_api::seam::AGING_RULE_TYPE);
    let ruled = cat
        .upsert(&alice, &rules, &"r1".into(), de(rule), opts())
        .await;
    not_granted("an aging rule on the type", ruled)?;
    // Another's record is, to her write, a key that is free: every probe
    // answers as there, and a write that would pass is not granted.
    let (field, reason, gone) = (None, Refusal::NotGranted, "not found");
    let ng = Error::Refused { field, reason }.to_string();
    let (expected, actual) = (lakelands_api::Version(7), lakelands_api::Version(0));
    let conflict = Error::Conflict { expected, actual }.to_string();
    let at7 = || de::<lakelands_api::write::WriteOpts>(json!({"expected_version": 7}));
    let k = RecordKey::from;
    let mut rows: Vec<(String, String, String)> = Vec::new();
    let mut row = |what: &str, said: String, want: &str| {
        rows.push((what.to_owned(), said, want.to_owned()));
    };
    let (by, claim) = (&alice, &ty);
    let up = move |key: &'static str, patch: Value, o| async move {
        cat.upsert(by, claim, &k(key), de(patch), o).await
    };
    let changed = up("c2", json!({"note": "x"}), opts()).await;
    row("bob's `c2` changed", said(changed), &ng);
    let taken = up("c2", json!({"owner": "alice"}), opts()).await;
    row("bob's `c2` taken", said(taken), &ng);
    let bobs = up("c9", json!({"owner": "bob"}), opts()).await;
    row("a free key with bob's owner", said(bobs), &ng);
    let handed = up("c1", json!({"owner": "bob"}), opts()).await;
    row("her `c1` handed to bob", said(handed), &ng);
    for key in ["c2", "c9"] {
        let probe = up(key, json!({"owner": "alice"}), at7()).await;
        row(&format!("`{key}` under version 7"), said(probe), &conflict);
    }
    let retired = cat.retire(&alice, &ty, &c2, opts()).await;
    row("`retire` of bob's `c2`", said(retired), gone);
    let called = cat.call(&alice, &verb, Some(&c2), none(), opts()).await;
    row("a verb on bob's `c2`", said(called), gone);
    let unlinked = cat.unlink(&alice, &ty, &c2, &on, &t1, opts()).await;
    row("`unlink` of bob's `c2`", said(unlinked), gone);
    let (relation, trip) = (&on, &t1);
    let linked = move |key: &'static str| async move {
        let from = k(key);
        cat.link(by, claim, &from, relation, trip, none(), opts())
            .await
    };
    let (bobs, missing) = (said(linked("c2").await), said(linked("c9").await));
    let missing = missing.contains("not a live record").then_some(missing);
    row(
        "`link` from bob's `c2`",
        bobs.replace("c2", "c9"),
        missing.as_deref().unwrap_or("names no live record"),
    );
    let slips = TypeName::from("slip");
    let slips = &slips;
    let slip = move |key: &'static str, patch: Value| async move {
        said(cat.upsert(by, slips, &k(key), de(patch), opts()).await)
    };
    let (no_sum, wrong_code) = (
        json!({"owner": "alice"}),
        json!({"owner": "alice", "sum": 1, "code": "x"}),
    );
    for (what, patch) in [
        ("without its sum", no_sum),
        ("whose code disagrees", wrong_code),
    ] {
        let (bobs, free) = (slip("s2", patch.clone()).await, slip("s9", patch).await);
        row(
            &format!("a slip {what} on bob's `s2`"),
            bobs.replace("s2", "s9"),
            &free,
        );
    }
    let fed_link = |from: &str| {
        let fed = crate::conformance::fixture::ingest(cat, &alice, "feed", vec![link(from, "t5")]);
        async move { said(fed.await) }
    };
    let (bobs, free) = (fed_link("r2").await, fed_link("r9").await);
    row(
        "a link ingested from bob's `r2`",
        bobs.replace("r2", "r9"),
        &free,
    );
    head_unmoved(store.as_ref(), head, "every refused write by alice").await?;
    let fed = |key: &str, owner: &str| {
        let r = json!({"type": "receipt", "key": key, "fields": {"owner": owner, "sum": 1}});
        let fed = crate::conformance::fixture::ingest(cat, &alice, "feed", vec![de(r)]);
        async move { said(fed.await) }
    };
    row("a re-delivery onto bob's `r2`", fed("r2", "bob").await, &ng);
    row("a receipt with bob's owner", fed("r5", "bob").await, &ng);
    let ok = "applied";
    row("her receipt", fed("r1", "alice").await, ok);
    // A key whose record another retired is free to her: made again, its
    // version continued.
    row(
        "her receipt on bob's retired `r3`",
        fed("r3", "alice").await,
        ok,
    );
    let c4 = k("c4");
    let made = cat.upsert(&alice, &ty, &c4, de(json!({"owner": "alice"})), opts());
    let version = |o: &Outcome| match o {
        Outcome::Applied {
            version: Some(v), ..
        } => v.0,
        _ => 0,
    };
    let made = made.await.map(|o| format!("version {}", version(&o)));
    row(
        "her claim on bob's retired `c4`",
        made.unwrap_or_else(|e| e.to_string()),
        "version 3",
    );
    // Her own records she writes, links and calls on.
    let changed = up("c1", json!({"note": "x"}), opts()).await;
    row("her `c1` changed", said(changed), ok);
    let made = up("c7", json!({"owner": "alice"}), opts()).await;
    row("her new `c7`", said(made), ok);
    let called = cat.call(&alice, &verb, Some(&c1), none(), opts()).await;
    row("a verb on her `c1`", said(called), ok);
    let unlinked = cat.unlink(&alice, &ty, &c1, &on, &t1, opts()).await;
    row("`unlink` of her `c1`", said(unlinked), ok);
    row("`link` of her `c1`", said(linked("c1").await), ok);
    let c3 = k("c3");
    let retired = cat.retire(&alice, &ty, &c3, opts()).await;
    let version = match &retired {
        Ok(Outcome::Applied {
            version: Some(v), ..
        }) => v.0,
        _ => 0,
    };
    row("`retire` of her `c3`", said(retired), ok);
    let again = de(json!({"expected_version": version}));
    let again = cat.upsert(&alice, &ty, &c3, de(json!({"owner": "alice"})), again);
    row(
        "her `c3` made again under its version",
        said(again.await),
        ok,
    );
    // A link's cardinality names the other end only where its writer is
    // answered that record.
    let (desk, held_by) = (&desk, &held_by);
    let held = move |who: Principal, from: RecordKey, to: &'static str| async move {
        cat.link(&who, desk, &from, held_by, &k(to), none(), opts())
            .await
    };
    let hers = held(alice.clone(), d1.clone(), "c1").await;
    row("her `d1` held by her `c1`", said(hers), ok);
    let (own, other) = ("linked to `c1`", "linked to no end");
    let hers = link_said(held(alice.clone(), d1.clone(), "c3").await);
    row("`d1` already held by her `c1`", hers, own);
    let bobs = link_said(held(alice.clone(), d2.clone(), "c1").await);
    row("`d2` already held by bob's `c2`", bobs, other);
    restrict(store.as_ref(), "claim", "c2", "kept").await?;
    let carol = holder(store.as_ref(), "carol", json!({"default": "write"}));
    let restricted = link_said(held(carol.clone(), d2.clone(), "c1").await);
    row("`d2` held by a restricted `c2`", restricted, other);
    // Nor where the link that holds is itself held: two of the owner's
    // links onto alice's claims, each restricted, answer carol alike.
    let lt = crate::links::link_type_name(desk, held_by);
    for (d, c) in [("d3", "c1"), ("d4", "c3")] {
        l.upsert("desk", d, json!({"n": d})).await?;
        let (from, to) = (k(d), k(c));
        let linked = cat.link(&l.own, desk, &from, held_by, &to, none(), opts());
        linked.await.must("the owner's link of a desk")?;
        let key = crate::links::link_key(&from, &to);
        restrict(store.as_ref(), lt.as_str(), key, "kept").await?;
    }
    let d3 = held(carol.clone(), k("d3"), "c7").await;
    let d4 = held(carol.clone(), k("d4"), "c7").await;
    row(
        "`d3` and `d4`, each held by a restricted link",
        said(d3),
        &said(d4).replace("d4", "d3"),
    );
    let held_link = link_said(held(carol, k("d3"), "c7").await);
    row("`d3` held by a restricted link", held_link, other);
    // `only` on a link type, on a field of the link the source sends: a
    // link it admits holds the relation's cardinality, over the lake and
    // the window.
    let feed = json!([{"name": "feed", "type": {"kind": "text"}, "authority": "source"}]);
    l.declare_json(json!({"declare": "type", "name": "stub",
        "fields": [{"name": "n", "type": {"kind": "text"}}],
        "relations": [{"name": "sent", "target": "trip", "authority": "source",
                       "cardinality": "many_to_one", "fields": feed},
                      {"name": "pair", "target": "trip", "authority": "source",
                       "cardinality": "one_to_one", "fields": feed}]}))
        .await?;
    for s in ["s1", "s2", "s3", "s4", "s5"] {
        l.upsert("stub", s, json!({"n": s})).await?;
    }
    let lt = crate::links::link_type_name(&"stub".into(), &"sent".into());
    let pair = crate::links::link_type_name(&"stub".into(), &"pair".into());
    let only = json!({"only": {"feed": "a"}});
    let types = json!({lt.as_str(): only, pair.as_str(): only});
    let grants = json!({"default": "write", "ingest": true, "types": types});
    let kim = holder(store.as_ref(), "kim", grants);
    let stub = |from: &str, to: &str| {
        let key = crate::links::link_key(&from.into(), &to.into());
        let fields = json!({"from": from, "to": to, "feed": "a"});
        de(json!({"type": lt.as_str(), "key": key.as_str(), "fields": fields}))
    };
    let fed = |links| crate::conformance::fixture::ingest(cat, &kim, "feed", links);
    row("kim's s1 → t1", said(fed(vec![stub("s1", "t1")]).await), ok);
    let again = link_said(fed(vec![stub("s1", "t5")]).await);
    row("kim's s1 → t5", again, "linked to `t1`");
    let window = link_said(fed(vec![stub("s2", "t1"), stub("s2", "t5")]).await);
    row(
        "kim's s2 → t1 and t5 at once",
        window,
        "linked in the window",
    );
    // A link her `only` on the link type keeps from her names her no end,
    // from either side, whichever record that end is: the owner's two
    // pairs, fed by another feed, answer her alike.
    let paired = |from: &str, to: &str, feed: &str| {
        let key = crate::links::link_key(&from.into(), &to.into());
        let fields = json!({"from": from, "to": to, "feed": feed});
        vec![de(
            json!({"type": pair.as_str(), "key": key.as_str(), "fields": fields}),
        )]
    };
    for (from, to) in [("s3", "t1"), ("s4", "t5")] {
        l.ingest("feed", paired(from, to, "b"))
            .await
            .must("the owner's pair")?;
    }
    let (s3, s4) = (
        fed(paired("s3", "t5", "a")).await,
        fed(paired("s4", "t1", "a")).await,
    );
    row(
        "kim's s3 and s4, paired by another feed",
        said(s3),
        &said(s4).replace("s4", "s3"),
    );
    let (t1, t5) = (
        fed(paired("s5", "t1", "a")).await,
        fed(paired("s5", "t5", "a")).await,
    );
    row(
        "kim's s5 onto t1 and t5, paired by another feed",
        said(t1),
        &said(t5).replace("t5", "t1"),
    );
    let hers = link_said(fed(paired("s3", "t5", "a")).await);
    row("kim's s3 → t5", hers, "linked to no end");
    let hers = link_said(fed(paired("s5", "t1", "a")).await);
    row("kim's s5 → t1", hers, "linked from no end");
    let owners = link_said(l.ingest("feed", paired("s5", "t1", "b")).await);
    row("the owner's s5 → t1", owners, "linked from `s3`");
    let wrong: Vec<_> = rows.iter().filter(|(_, said, want)| said != want).collect();
    ensure!(
        wrong.is_empty(),
        "a holder of `only` writes its own records alone, another's answered as a free key: {wrong:?}"
    );
    // Bob's proposal of a claim is listed to alice without its write, and
    // so once his `c2` is retired; his call on `c2`, held since, is not
    // there to her (lakelands#1008).
    for retired in [false, true] {
        if retired {
            l.retire("claim", "c2").await?;
        }
        let listed = cat.proposals(&alice, Default::default()).await;
        let listed = listed.must("alice's listing")?.data;
        let bobs: Vec<_> = listed
            .iter()
            .filter(|p| proposals.contains(&p.id))
            .collect();
        ensure!(
            bobs.len() == 1
                && bobs[0].id == proposals[1]
                && bobs[0].write.is_none()
                && bobs[0].more,
            "bob's proposal of `c5` must be listed to alice without its write and his call on the held `c2` not at all, `c2` retired: {retired}; she is listed {bobs:?}"
        );
    }
    // A reference into the type names an admitted record, or none, at
    // `upsert`, `propose` and `ingest`.
    let (memo, m1) = (TypeName::from("memo"), RecordKey::from("m1"));
    let named = |key: &str| de(json!({"claim": key}));
    let proposed =
        json!({"write": "upsert", "type": "memo", "key": "m2", "patch": {"claim": "c3"}});
    let fed = de(json!({"type": "memo", "key": "m3", "fields": {"claim": "c3"}}));
    l.upsert("claim", "c3", json!({"owner": "bob"})).await?;
    for (what, other) in [
        (
            "`upsert`",
            cat.upsert(&alice, &memo, &m1, named("c3"), opts())
                .await
                .map(|_| ()),
        ),
        (
            "`propose`",
            cat.propose(&alice, de(proposed), "mine".into())
                .await
                .map(|_| ()),
        ),
        (
            "`ingest`",
            crate::conformance::fixture::ingest(cat, &alice, "feed", vec![fed])
                .await
                .map(|_| ()),
        ),
    ] {
        let says = other.as_ref().err().and_then(Error::case);
        ensure!(
            matches!(says, Some(Said { case: Case::NotLive { key, .. }, .. }) if key == "c3"),
            "a memo naming bob's `c3` through {what} must be refused as naming no live record; it answered {other:?}"
        );
    }
    let own = cat.upsert(&alice, &memo, &m1, named("c1"), opts()).await;
    own.must("alice's memo naming her own `c1`")?;
    Ok(())
}

async fn a_holder_of_only_changes_no_shape(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    claims(&l, false).await?;
    l.declare_json(json!({"declare": "value_type", "name": "label", "kind": "text"}))
        .await?;
    let mut wider = claim_with("owner");
    wider.fields.push(field("extra", Text));
    let other = json!({"declare": "type", "name": "other", "fields": [{"name": "n", "type": {"kind": "text"}}]});
    let label: lakelands_api::ontology::ValueTypeDecl =
        de(json!({"name": "label", "kind": "text", "one_of": ["a"]}));
    let writes = [
        json!({"write": "declare", "declarations": [other]}),
        json!({"write": "migrate", "migration": {"to": wider}}),
        json!({"write": "migrate_value_type", "to": label}),
    ];
    let mut proposals = Vec::new();
    for write in &writes {
        let proposed = cat.propose(own, de(write.clone()), "the owner's".into());
        let proposed = proposed.await;
        let Outcome::Proposed { proposal, .. } = proposed.must("the owner's proposal")? else {
            return Err(Failure::new("a proposal must be proposed"));
        };
        proposals.push(proposal);
    }
    let alice = alice(
        store.as_ref(),
        json!({"default": "write", "schema": true, "review": true}),
    );
    let head = store.head().await.must("reading the head")?;
    let m = || crate::conformance::fixture::migration(wider.clone());
    let (why, claim) = (|| "mine".to_owned(), TypeName::from("claim"));
    let recompute = lakelands_api::write::Recompute::Type(claim);
    not_granted("`declare`", cat.declare(&alice, de(other.clone())).await)?;
    not_granted("`apply`", cat.apply(&alice, vec![de(other)]).await)?;
    not_granted("`recompute`", cat.recompute(&alice, recompute).await)?;
    let extra = field("extra", Text);
    not_granted(
        "`add_field`",
        cat.add_field(&alice, &"trip".into(), extra).await,
    )?;
    not_granted("`migrate`", cat.migrate(&alice, m()).await)?;
    not_granted("`migrate_apply`", cat.migrate_apply(&alice, m(), "x").await)?;
    let vt = cat.migrate_value_type(&alice, label.clone().into()).await;
    not_granted("`migrate_value_type`", vt)?;
    let vt = cat.migrate_value_type_apply(&alice, label.into(), "x");
    let vt = vt.await;
    not_granted("`migrate_value_type_apply`", vt)?;
    for write in writes {
        not_granted(
            "a schema proposal",
            cat.propose(&alice, de(write), why()).await,
        )?;
    }
    not_granted(
        "`accept` of a declaration",
        cat.accept(&alice, &proposals[0]).await,
    )?;
    let rejected = cat.reject(&alice, &proposals[0], why()).await;
    not_granted("`reject` of a declaration", rejected)?;
    head_unmoved(store.as_ref(), head, "every schema write by alice").await?;
    let listed = cat.proposals(&alice, Default::default());
    let listed = listed.await.must("alice's listing")?;
    let listed: Vec<_> = listed.data.iter().map(|p| &p.id).collect();
    ensure!(
        listed == [&proposals[0]],
        "only the declaration's proposal may be listed to alice; she is listed {listed:?}"
    );
    Ok(())
}

const UNAPPLIED: &str = r#"
    a field the type does not declare | {"only": {"payer": "alice"}}
    a number field | {"only": {"amount": "10"}}
    a number field and value | {"only": {"amount": 10}}
    a value that is not text | {"only": {"owner": 1}}
    a field she may not read | {"only": {"owner": "alice"}, "fields": {"owner": "masked"}}
    a field she may not know of | {"only": {"owner": "alice"}, "fields": {"owner": "none"}}
"#;

/// The reads of the kept `spend`, grouped by the pair's field, and of a
/// view computed over it, that admit nothing to a grant that cannot be
/// applied (lakelands#771): `-` is an answer of nothing.
const KEPT_UNAPPLIED: &str = r#"
    spend | its records | {"total": true} | total=0
    spend | its sum | {"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "total"}]}} | s=0
    spend | its count | {"aggregate": {"measures": [{"name": "n", "func": "count"}]}} | n=0
    spend | its groups | {"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "total"}], "group_by": ["owner"]}} | -
    spend | its groups as of before | {"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "total"}], "group_by": ["owner"]}, "as_of": BEFORE} | -
    over_spend | a view over it | {"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "t"}]}} | s=0
"#;

async fn a_grant_that_cannot_be_applied_is_refused(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    claims(&l, false).await?;
    for (name, risk) in [("pay", "low"), ("propose_pay", "high")] {
        let set = json!([{"field": "note", "value": "'paid'"}]);
        let verb = json!({"declare": "verb", "name": name, "on": "claim", "risk": risk,
                          "effect": "write", "set": set});
        l.declare_json(verb).await?;
    }
    // She is offered her own writes and her verbs, the one that waits for
    // review too, since it assigns no field her `only` names; a create
    // only where she writes her owner field, which a retire asks of
    // anyone.
    let read_only = json!({"only": {"owner": "alice"}, "fields": {"owner": "read"}});
    for (more, creates) in [
        (json!({"default": "write"}), true),
        (
            json!({"default": "write", "types": {"claim": read_only}}),
            false,
        ),
    ] {
        let alice = alice(store.as_ref(), more);
        let told = l
            .cat
            .describe_type(&alice, &"claim".into(), Default::default())
            .await;
        let told = told.must("describing `claim` to alice")?.data;
        let (may, verbs) = (&told.may, &told.verbs);
        let only = serde_json::to_value(&may.only).expect("a map reads");
        let link: Vec<&str> = may.link.iter().map(|r| r.as_str()).collect();
        let verbs: Vec<&str> = verbs.iter().map(|v| v.as_str()).collect();
        ensure!(
            only == json!({"owner": "alice"})
                && may.upsert == creates
                && may.retire == creates
                && link == ["on_trip"]
                && verbs == ["pay", "propose_pay"],
            "`describe_type` must tell alice her `only`, her writes, the link and both verbs of `claim`, an upsert where she writes `owner` ({creates}); it says {may:?} and verbs {verbs:?}"
        );
    }
    let alice = alice(store.as_ref(), json!({"default": "write"}));
    let all = vec![field("n", Number)];
    l.declare(view_of(
        "all_claims",
        "claim",
        &[],
        all,
        &[("n", "count()")],
    ))
    .await?;
    let count = json!({"aggregate": {"measures": [{"name": "s", "func": "sum", "field": "n"}]}});
    let walk = json!({"start": {"type": "trip", "key": "t1"},
                      "steps": [{"relation": "on_trip", "walk": "in"}], "project": {"as": "graph"}});
    let mut wrong = Vec::new();
    for (what, grant) in UNAPPLIED.lines().filter_map(|r| r.trim().split_once(" | ")) {
        let grant: Value = serde_json::from_str(grant).expect("a grant reads");
        let eve = holder(
            store.as_ref(),
            "eve",
            json!({"default": "read", "types": {"claim": grant}}),
        );
        let said = l.cat.find(&eve, &"claim".into(), Default::default()).await;
        // The type alone is named: no field, no place, no other parameter.
        let refused = |e: &Error| {
            matches!(e, Error::Refused { field: None, reason: Refusal::Invalid(said) }
                if said.at.is_empty() && unappliable(e))
        };
        if !said.as_ref().is_err_and(refused) {
            wrong.push(format!("{what}: {said:?}"));
        }
        // A walk or a view that reaches the type admits none of its records.
        let walked = l.cat.query(&eve, de(walk.clone())).await;
        let walked = walked.map(|r| node_keys(&r.data, "claim"));
        let viewed = l
            .cat
            .find(&eve, &"all_claims".into(), de(count.clone()))
            .await;
        let viewed = viewed.map(|f| short(&f.data));
        if walked.as_deref().ok() != Some("") || viewed.as_deref().ok() != Some("s=0") {
            wrong.push(format!(
                "{what}: a walk reaches {walked:?}, a view counts {viewed:?}"
            ));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a grant that cannot be applied must refuse `find`, naming `claim` and no field: {wrong:?}"
    );
    // Renamed, the grant follows; dropped, a read at the head is refused
    // and one as of before is answered.
    let mut dropped = claim_with("owner");
    dropped.fields.remove(0);
    for (to, renamed) in [(claim_with("holder"), true), (dropped, false)] {
        let at = migrate(&l, to, renamed).await?;
        let then = "claim | as of before the migration | {\"as_of\": BEFORE} | c1 c3";
        finds(&l, &alice, then, Position(at.0 - 1)).await?;
    }
    let dropped = l
        .cat
        .find(&alice, &"claim".into(), Default::default())
        .await;
    ensure!(
        dropped.as_ref().is_err_and(unappliable),
        "after `owner` is dropped alice's `find` at the head must be refused; it answered {dropped:?}"
    );
    Ok(())
}

check!(
    ONLY_UNAPPLIED_ADMITS_NONE = a_grant_that_cannot_be_applied_admits_no_record_at_any_door,
    "051, extended (lakelands#771): a holder's `only` that cannot be applied admits none of the type's records, whatever a stored value holds: the proposal listing answers a reviewer every proposal on such a record without its write, and a reference into the type, a link's end too, is refused as naming no live record, exactly as an absent key is, and a second link over a relation that allows one names nothing of the first; an upsert, a verb, an unlink, a proposed upsert or retirement, a retirement and an ingest on a record whose value equals the pair answer as on one whose value does not, the link's end judged so too; a search as of a position admits no candidate where the grant cannot be applied to the head's declaration, a field made one the holder may not read there included, though its values stay, and no link whose end's type the grant so holds; a kept view grouped by the pair's field, and a view over it, answer nothing to such a grant through `find`, a sum, a count, groups, `total`, a read as of before and `get`, refuse it where it masks the pair's field, and answer as types not declared where it holds that field at `none`, since the view is grouped by it (lakelands#782)"
);

/// Grants on `claim` whose `only` cannot be applied though a stored value
/// equals its pair: a number, and an owner the holder may not read.
const UNAPPLIED_YET_HELD: &str = r#"
    a number pair | {"only": {"amount": 10}}
    a masked owner | {"only": {"owner": "alice"}, "fields": {"owner": "masked"}}
    a hidden owner | {"only": {"owner": "alice"}, "fields": {"owner": "none"}}
"#;

async fn a_grant_that_cannot_be_applied_admits_no_record_at_any_door(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    claims(&l, true).await?;
    let over = vec![field("t", Number)];
    l.declare(view_of(
        "over_spend",
        "spend",
        &[],
        over,
        &[("t", "sum(total)")],
    ))
    .await?;
    let before = store.head().await.must("reading the head")?.0.to_string();
    let memo = crate::conformance::fixture::reference("claim", "claim");
    l.declare(type_decl("memo", vec![memo])).await?;
    let held_by = json!({"name": "held_by", "target": "claim", "cardinality": "many_to_one"});
    l.declare_json(json!({"declare": "type", "name": "desk",
        "fields": [{"name": "n", "type": {"kind": "text"}}], "relations": [held_by]}))
        .await?;
    l.upsert("desk", "d1", json!({"n": "d1"})).await?;
    l.upsert("desk", "d2", json!({"n": "d2"})).await?;
    let (desk, d2, c1) = (TypeName::from("desk"), RecordKey::from("d2"), "c1".into());
    let (by, no, opts) = ("held_by".into(), Default::default, Default::default);
    let linked = cat.link(&l.own, &desk, &d2, &by, &c1, no(), opts());
    linked.await.must("linking `d2` to `c1`")?;
    let pay = json!({"declare": "verb", "name": "pay", "on": "claim", "risk": "low",
                     "effect": "write", "set": [{"field": "note", "value": "'paid'"}]});
    l.declare_json(pay).await?;
    let shared = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}, "authority": "shared"});
    let bill = [
        shared("owner", "text"),
        shared("amount", "number"),
        shared("note", "text"),
    ];
    l.declare_json(json!({"declare": "type", "name": "bill", "fields": bill}))
        .await?;
    for (key, owner, amount) in [("c1", "alice", 10), ("c2", "bob", 20)] {
        let bill = json!({"owner": owner, "amount": amount, "note": "n"});
        l.upsert("bill", &key.replace('c', "b"), bill).await?;
    }
    for key in ["c1", "c2"] {
        let w = json!({"write": "upsert", "type": "claim", "key": key, "patch": {"note": "x"}});
        let proposed = cat.propose(&l.own, de(w), "the owner's".into()).await;
        proposed.must("the owner's proposal")?;
    }
    // Every proposal is listed to her as one on a record she is not shown.
    let all = cat.proposals(&l.own, Default::default()).await;
    let mut all = all.must("the owner's listing")?.data;
    for p in &mut all {
        (p.write, p.more) = (None, true);
    }
    let mut wrong = Vec::new();
    for (what, grant) in UNAPPLIED_YET_HELD
        .lines()
        .filter_map(|r| r.trim().split_once(" | "))
    {
        let grant: Value = serde_json::from_str(grant).expect("a grant reads");
        let types = json!({"claim": grant, "bill": grant});
        let grants = json!({"default": "write", "review": true, "ingest": true, "types": types});
        let eve = &holder(store.as_ref(), "eve", grants);
        let listed = cat.proposals(eve, Default::default()).await;
        if !listed.as_ref().is_ok_and(|r| r.data == all) {
            wrong.push(format!("{what}: the listing {listed:?}"));
        }
        // A reference to her `c1`, as a field and as a link's end, is
        // refused whole as one to the absent `c9`.
        let refused = |c: &'static str| async move {
            let (memo, desk) = (TypeName::from("memo"), TypeName::from("desk"));
            let (m1, d1, to) = (
                RecordKey::from("m1"),
                RecordKey::from("d1"),
                RecordKey::from(c),
            );
            let patch = de(json!({"claim": c}));
            let upserted = cat.upsert(eve, &memo, &m1, patch, Default::default());
            let (held_by, none) = (RelationName::from("held_by"), Default::default());
            let linked = cat.link(eve, &desk, &d1, &held_by, &to, none, Default::default());
            let print = |e: Option<Error>| serde_json::to_string(&e).expect("an error prints");
            [print(upserted.await.err()), print(linked.await.err())].map(|e| e.replace(c, "c?"))
        };
        let (held, absent) = (refused("c1").await, refused("c9").await);
        if held != absent || absent.iter().any(|e| e == "null") {
            wrong.push(format!("{what}: `c1` {held:?}, the absent `c9` {absent:?}"));
        }
        // A second link of `d2`, which `held_by` allows one, names nothing
        // of the link that holds: it answers as one to the absent `c9`.
        let mut second = Vec::new();
        for c in ["c2", "c9"] {
            let to = c.into();
            let linked = cat.link(eve, &desk, &d2, &by, &to, no(), opts()).await;
            second.push(format!("{:?}", linked.err()).replace(c, "c?"));
        }
        if second[0] != second[1] || second[0].contains("c1") {
            wrong.push(format!("{what}: a second link of `d2` {second:?}"));
        }
        // Her writes on `c1`, whose value her pair equals, answer as on
        // `c2`, whose value it does not (and `b1` as `b2`): she owns neither.
        let writes = |c: &'static str| async move {
            let (claim, key, b) = (
                TypeName::from("claim"),
                RecordKey::from(c),
                c.replace('c', "b"),
            );
            let (on, t1, opts) = (
                RelationName::from("on_trip"),
                RecordKey::from("t1"),
                Default::default,
            );
            let print = |r: Result<(), Error>| match r {
                Ok(()) => "applied".to_owned(),
                Err(e) => serde_json::to_string(&e)
                    .expect("an error prints")
                    .replace(c, "c?"),
            };
            let up = json!({"write": "upsert", "type": "claim", "key": c, "patch": {"note": "x"}});
            let gone = json!({"write": "retire", "type": "claim", "key": c});
            let fed = de(json!({"type": "bill", "key": b, "fields": {"note": "x"}}));
            [
                print(
                    cat.upsert(eve, &claim, &key, de(json!({"note": "x"})), opts())
                        .await
                        .map(drop),
                ),
                print(
                    cat.call(eve, &"pay".into(), Some(&key), Default::default(), opts())
                        .await
                        .map(drop),
                ),
                print(
                    cat.unlink(eve, &claim, &key, &on, &t1, opts())
                        .await
                        .map(drop),
                ),
                print(cat.propose(eve, de(up), "eve's".into()).await.map(drop)),
                print(cat.propose(eve, de(gone), "eve's".into()).await.map(drop)),
                print(cat.retire(eve, &claim, &key, opts()).await.map(drop)),
                print(
                    crate::conformance::fixture::ingest(cat, eve, "feed", vec![fed])
                        .await
                        .map(drop),
                )
                .replace(&b, "b?"),
            ]
        };
        let (held, other) = (writes("c1").await, writes("c2").await);
        if held != other {
            wrong.push(format!(
                "{what}: writes on `c1` {held:?}, on `c2` {other:?}"
            ));
        }
    }
    for (what, grant) in UNAPPLIED.lines().filter_map(|r| r.trim().split_once(" | ")) {
        let grant: Value = serde_json::from_str(grant).expect("a grant reads");
        let eve = holder(
            store.as_ref(),
            "eve",
            json!({"default": "read", "types": {"claim": grant}}),
        );
        // A kept view grouped by the pair's field, and a view over it, admit
        // nothing; one whose pair she may not read refuses her outright.
        // One she may not know of hides a view grouped by it (lakelands#782).
        let unknown = grant["fields"]["owner"] == "none";
        let hidden = grant.get("fields").is_some() && !unknown;
        for row in KEPT_UNAPPLIED
            .lines()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        {
            let [ty, read, find, want] = row.split(" | ").collect::<Vec<_>>()[..] else {
                panic!("a row is four cells: {row}");
            };
            let find = serde_json::from_str(&find.replace("BEFORE", &before));
            let found = l
                .cat
                .find(&eve, &ty.into(), de(find.expect("a find reads")))
                .await;
            let said = match found {
                Ok(f) if !hidden => short(&f.data).trim().to_owned(),
                Err(Error::Refused {
                    reason: Refusal::NotGranted,
                    ..
                }) if hidden => want.replace('-', ""),
                Err(Error::NotFound { .. }) if unknown => want.replace('-', ""),
                other => format!("{other:?}"),
            };
            if said != want.replace('-', "") {
                wrong.push(format!("{what}: {read} of `{ty}`: {said}"));
            }
        }
        let got = get(&l, &eve, "spend", "alice", None).await;
        if !matches!(&got, Err(Error::NotFound { .. }) if !hidden)
            && !matches!(&got, Err(Error::Refused { reason: Refusal::NotGranted, .. }) if hidden)
        {
            wrong.push(format!("{what}: `get` of `spend` `alice`: {got:?}"));
        }
    }
    // A search as of before a declaration that makes the pair's field
    // one she may not read admits no candidate at the head, where her
    // grant cannot be applied, though the stored values stay.
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}, "searchable": n == "note"});
    let mut chit = json!({"declare": "type", "name": "chit",
                          "fields": [text("holder"), text("secret"), text("note")]});
    l.declare_json(chit.clone()).await?;
    for key in ["h1", "h3"] {
        l.upsert("chit", key, json!({"holder": "alice", "note": "taxi"}))
            .await?;
    }
    let before = store.head().await.must("reading the head")?;
    chit["fields"][0]["depends_on"] = json!(["secret"]);
    l.declare_json(chit).await?;
    l.upsert("chit", "h3", json!({"holder": "bob"})).await?;
    let grant = json!({"only": {"holder": "alice"}, "fields": {"secret": "none"}});
    let eve = holder(
        store.as_ref(),
        "eve",
        json!({"default": "read", "types": {"chit": grant}}),
    );
    let search = lakelands_api::query::Search {
        ty: "chit".into(),
        at: de(json!({"as_of": before.0})),
        ..super::search::doc_search(lakelands_api::query::SearchMode::lexical("taxi"))
    };
    let found = cat.search(&eve, search).await.map(|h| hit_keys(&h.data));
    if found.as_deref().ok() != Some("") {
        wrong.push(format!(
            "a search as of before `holder` came to depend on `secret`: {found:?}"
        ));
    }
    // So does a link whose end's type is held so: `trip`'s `name` comes to
    // depend on `tsecret`, which she may not read.
    let trip = |name: Value| {
        let fields = [
            json!({"name": "name", "type": {"kind": "text"}, "depends_on": name}),
            json!({"name": "tsecret", "type": {"kind": "text"}}),
        ];
        json!({"declare": "type", "name": "trip", "fields": fields})
    };
    l.declare_json(trip(json!([]))).await?;
    l.upsert("trip", "t2", json!({"name": "rome"})).await?;
    let (ty, c3, on, t1) = names("c3");
    let unlinked = cat.unlink(&l.own, &ty, &c3, &on, &t1, Default::default());
    unlinked.await.must("unlinking `c3` from `t1`")?;
    let (label, t2) = (de(json!({"label": "trip"})), RecordKey::from("t2"));
    let linked = cat.link(&l.own, &ty, &c3, &on, &t2, label, Default::default());
    linked.await.must("linking `c3` to `t2`")?;
    let before = store.head().await.must("reading the head")?;
    l.upsert("trip", "t2", json!({"name": "paris"})).await?;
    l.declare_json(trip(json!(["tsecret"]))).await?;
    let grant = json!({"only": {"name": "rome"}, "fields": {"tsecret": "none"}});
    let grants = json!({"default": "read", "types": {"trip": grant}});
    let eve = holder(store.as_ref(), "eve", grants);
    let search = lakelands_api::query::Search {
        ty: crate::links::link_type_name(&ty, &on),
        at: de(json!({"as_of": before.0})),
        ..super::search::doc_search(lakelands_api::query::SearchMode::lexical("trip"))
    };
    let found = cat.search(&eve, search).await.map(|h| hit_keys(&h.data));
    if found.as_deref().ok() != Some("") {
        wrong.push(format!(
            "a link search as of before `tsecret` hid `name`: {found:?}"
        ));
    }
    ensure!(
        wrong.is_empty(),
        "an `only` that cannot be applied admits no record, whatever a stored value holds: {wrong:?}"
    );
    Ok(())
}

/// Whether `answer`, about the proposal `id`, is what an id never made,
/// `none`, is answered, to the byte (lakelands#1008).
fn as_never<T>(
    answer: &lakelands_api::Result<T>,
    never: &lakelands_api::Result<T>,
    id: &ProposalId,
) -> bool {
    let renamed = ("none", id.as_str());
    matches!((answer, never), (Err(a), Err(b)) if alike_renamed("", a, b, renamed).is_ok())
}

check!(
    ONLY_LINKS_OF_LINKS = a_link_whose_end_is_a_link_is_answered_where_that_link_is,
    "051 and 006: a link whose end is a link is answered where that link is, by its own ends as deep as they go (lakelands#879). A holder of `only` on a type two links down, with `restricted` or without, is answered by `get`, `find`, `total`, a count, a view over the link type, a search and a walk only the links whose ends' ends are admitted, and `describe_type` says `only some` of the view; three links down, `find` and `total` answer it so too and `get` of another's is not found; one whose grant admits nothing counts none at either depth, and one who holds `restricted` and no `only` is answered every link and the view as kept. To the holder a reference to any other link, and a link onto it, answer as one to a link no record holds, a verb of low risk, one that reaches outside the lake and an `unlink` on it are not found, and a reference to its own is taken; a second link where one is allowed names the first only where its writer is answered it, at both depths; a call proposed on one is listed once, without its write, and is not its reviewer's to decide; its own proposal is pinned, accepted where nothing moved and no more once an end's end moved; a read and a search as of a position judge the ends as they stood then and at the head. One without `restricted` is not answered a link whose end's end is held, is named none by a second link while it is named another's, finds none by a low verb, an external verb or an `unlink`, and a call on one is not there to it, its accept and reject answered as an id never made (lakelands#1008), while it decides another and a call proposed with `restricted` is applied. Two link types a migration made each other's end are each asked once: a link between them is answered, a second link names no end that no record holds, and the link is not answered once an end's end in the ring is held; the inner link's relation is not removed while the outer relation targets its link type (lakelands#920), and the outer link is still not answered to one without `restricted`, who decides no call on it, and a call proposed with `restricted` is still applied"
);

const DEEP: &str = r#"
    sys.link.tag.tags | the links onto her claims' links | {} | g1:c1%3At1 g3:c3%3At1
    sys.link.tag.tags | their total | {"total": true} | g1:c1%3At1 g3:c3%3At1 total=2
    sys.link.tag.tags | their count | {"aggregate": {"measures": [{"name": "n", "func": "count"}]}} | n=2
    sys.link.note.about | a link onto a link onto a link | {"total": true} | n1:g1%3Ac1%253At1 total=1
    tagged | a kept view over the links | {} | trip g1 trip g3
"#;

const DEEP_NONE: &str = r#"
    sys.link.tag.tags | the links | {"aggregate": {"measures": [{"name": "n", "func": "count"}]}} | n=0
    sys.link.note.about | a link onto a link onto a link | {"aggregate": {"measures": [{"name": "n", "func": "count"}]}} | n=0
    tagged | a kept view over the links | {"aggregate": {"measures": [{"name": "n", "func": "count"}]}} | n=0
"#;

const DEEP_ALL: &str = r#"
    sys.link.tag.tags | every link | {"total": true} | g1:c1%3At1 g2:c2%3At1 g3:c3%3At1 total=3
    sys.link.tag.tags | their count | {"aggregate": {"measures": [{"name": "n", "func": "count"}]}} | n=3
    sys.link.note.about | every link onto a link onto a link | {"total": true} | n1:g1%3Ac1%253At1 n2:g2%3Ac2%253At1 total=2
"#;

const DEEP_HANDED: &str = r#"
    sys.link.tag.tags | as of before | {"as_of": BEFORE} | g1:c1%3At1 g3:c3%3At1
    sys.link.tag.tags | at the head | {} | g1:c1%3At1
"#;

/// On [`claims`]: `tag` g1, g2 and g3, each linked over `tags` to the link
/// of c1, c2 and c3 to t1; `note` n1 and n2, linked over `about` to g1's
/// and g2's link, and n3, linked to none; `memo`, a reference to a link
/// of `tags`; `tagged`, a kept view over them; the verbs on them: `seen`,
/// above low risk, `peek`, of low risk, and `ping`, which reaches outside.
async fn links_onto_links(l: &Lake) -> Result<(), Failure> {
    let (tags, on_trip) = ("sys.link.tag.tags", "sys.link.claim.on_trip");
    let label = json!({"name": "label", "type": {"kind": "text"}, "searchable": true});
    for (ty, name, target) in [("tag", "tags", on_trip), ("note", "about", tags)] {
        let mut t = type_decl(ty, vec![field("name", Text)]);
        t.relations = vec![de(
            json!({"name": name, "target": target, "fields": [label]}),
        )];
        l.declare(t).await?;
    }
    let memo = crate::conformance::fixture::reference("l", tags);
    l.declare(type_decl("memo", vec![memo])).await?;
    let tagged = crate::conformance::fixture::count_view("tagged", tags, "label");
    l.declare(tagged).await?;
    l.set("view_kept.tagged", json!(true)).await?;
    let seen = json!({"declare": "verb", "name": "seen", "on": tags,
        "risk": "high", "effect": "write", "set": [{"field": "label", "value": "'seen'"}]});
    let peek = json!({"declare": "verb", "name": "peek", "on": tags,
        "risk": "low", "effect": "write", "set": [{"field": "label", "value": "'seen'"}]});
    let ping = json!({"declare": "verb", "name": "ping", "on": tags, "risk": "low",
        "effect": "external", "executor": EXECUTOR});
    for verb in [seen, peek, ping] {
        l.declare_json(verb).await?;
    }
    for (ty, from, on, to) in [
        ("tag", "g1", "tags", "c1:t1"),
        ("tag", "g2", "tags", "c2:t1"),
        ("tag", "g3", "tags", "c3:t1"),
        ("note", "n1", "about", "g1:c1%3At1"),
        ("note", "n2", "about", "g2:c2%3At1"),
        ("note", "n3", "", ""),
    ] {
        l.upsert(ty, from, json!({"name": from})).await?;
        let label = de(json!({"label": format!("trip {from}")}));
        if !to.is_empty() {
            let (ty, from, on, to) = (ty.into(), from.into(), on.into(), to.into());
            let linked = l
                .cat
                .link(&l.own, &ty, &from, &on, &to, label, Default::default());
            linked.await.must("linking onto a link")?;
        }
    }
    Ok(())
}

/// Whether the verbs `peek` and `ping` on the link `key` of `tags`, and
/// the `unlink` of it from `tag` to `to`, answer `who` that the link
/// record is not found.
async fn gone(l: &Lake, who: &Principal, key: &str, (tag, to): (&str, &str)) -> Held {
    let (none, opts) = (Default::default, Default::default);
    let (key, ty, tag, tags, to) = (
        key.into(),
        "tag".into(),
        tag.into(),
        "tags".into(),
        to.into(),
    );
    let mut said = Vec::new();
    for verb in ["peek", "ping"] {
        let called = (l.cat)
            .call(who, &verb.into(), Some(&key), none(), opts())
            .await;
        said.push(called.err());
    }
    let unlinked = (l.cat).unlink(who, &ty, &tag, &tags, &to, opts()).await;
    said.push(unlinked.err());
    // The record's own: the link type and the key, of no other thing.
    let its = |e: &Option<Error>| {
        matches!(e, Some(Error::NotFound { missing: Missing::Record { ty, key: k } })
            if ty.as_str() == "sys.link.tag.tags" && *k == key)
    };
    let said: Vec<bool> = said.iter().map(its).collect();
    if said == [true; 3] {
        return Ok(());
    }
    let who = &who.id;
    Err(Failure::new(format!(
        "`peek`, `ping` and `unlink` on `{key}` by `{who}` are not found: {said:?}"
    )))
}

/// Whether `link` of each `[type, from, relation, to]` answers `who` the
/// fifth, as [`link_said`] says it.
async fn links_said(l: &Lake, who: &Principal, links: &[[&str; 5]]) -> Vec<Held> {
    let mut held = Vec::new();
    for [ty, from, on, to, want] in links {
        let (ty, from, on, end) = ((*ty).into(), (*from).into(), (*on).into(), (*to).into());
        let (none, opts) = (Default::default(), Default::default());
        let told = link_said((l.cat).link(who, &ty, &from, &on, &end, none, opts).await);
        let what = format!("`{from}` linked to `{to}` by `{}` answers: {told}", who.id);
        held.push(if told == *want {
            Ok(())
        } else {
            Err(Failure::new(what))
        });
    }
    held
}

async fn a_link_whose_end_is_a_link_is_answered_where_that_link_is(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    claims(&l, false).await?;
    links_onto_links(&l).await?;
    let (cat, tags, tagged) = (&l.cat, "sys.link.tag.tags", "tagged".into());
    let (g1, g2, g3, n2) = (
        "g1:c1%3At1",
        "g2:c2%3At1",
        "g3:c3%3At1",
        "n2:g2%3Ac2%253At1",
    );
    let search = |who: Principal, at: Option<Position>| {
        let search = json!({"type": tags, "mode": "lexical", "text": "trip", "as_of": at});
        async move {
            let hits = cat.search(&who, de(search)).await;
            Ok::<_, Failure>(hit_keys(&hits.must("a search of the links")?.data))
        }
    };
    let walk = |who: Principal, from: &'static str| {
        let walk = json!({"start": {"type": "note", "key": from},
            "steps": [{"relation": "about", "walk": "out"}], "project": {"as": "graph"}});
        async move {
            let walked = cat
                .query(&who, de(walk))
                .await
                .must("a walk over `about`")?;
            Ok::<_, Failure>(node_keys(&walked.data, tags))
        }
    };
    let kept = |who: Principal| {
        let tagged = &tagged;
        async move {
            let told = cat.describe_type(&who, tagged, Default::default()).await;
            Ok::<_, Failure>(told.must("describing `tagged`")?.data.lifecycle.kept)
        }
    };
    // A call of `verb` by `who` on the link `key`: proposed, or what it answered.
    let call = |who: Principal, verb: &'static str, key: &'static str| async move {
        let (v, k, none, opts) = (
            verb.into(),
            key.into(),
            Default::default(),
            Default::default(),
        );
        match cat.call(&who, &v, Some(&k), none, opts).await {
            Ok(Outcome::Proposed { proposal, .. }) => Ok(proposal),
            other => Err(Failure::new(format!(
                "`{verb}` on `{key}` by `{}` is not proposed: {other:?}",
                who.id
            ))),
        }
    };
    // Every line is asked, and each one that fails is told.
    let mut held: Vec<Held> = Vec::new();
    let said = |ok: bool, what: String| if ok { Ok(()) } else { Err(Failure::new(what)) };
    let nothing = json!({"default": "read", "restricted": true,
        "types": {"claim": {"only": {"amount": "10"}}}});
    let nobody = holder(s, "nobody", nothing);
    held.push(finds(&l, &nobody, DEEP_NONE, Position::GENESIS).await);
    // One who holds no `only` is answered every link, and the view kept.
    let free = holder(s, "free", json!({"default": "write", "restricted": true}));
    held.push(finds(&l, &free, DEEP_ALL, Position::GENESIS).await);
    let whole = kept(free.clone()).await?;
    let is_kept = whole == Some(lakelands_api::describe::Kept::Kept);
    held.push(said(
        is_kept,
        format!("`tagged` is told to one who holds no `only` as {whole:?}"),
    ));
    for more in [json!({"restricted": true}), json!({})] {
        let (who, alice) = (format!("alice with {more}"), alice(s, more));
        held.push(finds(&l, &alice, DEEP, Position::GENESIS).await);
        for (ty, key) in [(tags, g2), ("sys.link.note.about", n2)] {
            let link = get(&l, &alice, ty, key, None).await;
            held.push(not_found(&format!("`{key}` to {who}"), link));
        }
        let hers = get(&l, &alice, tags, g1, None).await;
        held.push(said(hers.is_ok(), format!("`{g1}` to {who}: {hers:?}")));
        let reached = (
            walk(alice.clone(), "n1").await?,
            walk(alice.clone(), "n2").await?,
        );
        held.push(said(
            reached == (g1.to_owned(), String::new()),
            format!("{who} walks from `n1` and from `n2` to {reached:?}"),
        ));
        let found = search(alice.clone(), None).await?;
        let hers = found == "g1:c1%3At1 g3:c3%3At1";
        held.push(said(hers, format!("{who} finds `{found}` by a search")));
        let kept = kept(alice).await?;
        let why = lakelands_api::describe::NotKept::OnlySome;
        let only_some = kept == Some(lakelands_api::describe::Kept::Computed { why });
        held.push(said(
            only_some,
            format!("`tagged` is told to {who} as {kept:?}"),
        ));
    }
    // A reference to such a link, and a link onto it, answer her as one
    // to a link that is not there.
    let grants = json!({"default": "write", "review": true, "restricted": true});
    let (writer, mut answers) = (alice(s, grants), Vec::new());
    for k in [g2, "g9:c9%3At1"] {
        let (memo, m1) = ("memo".into(), "m1".into());
        let up = cat.upsert(&writer, &memo, &m1, de(json!({"l": k})), Default::default());
        answers.push(format!("{:?}", up.await.err()).replace(k, "?"));
        let (note, n3, on, end) = ("note".into(), "n3".into(), "about".into(), k.into());
        let (none, opts) = (Default::default(), Default::default());
        let linked = cat.link(&writer, &note, &n3, &on, &end, none, opts);
        answers.push(format!("{:?}", linked.await.err()).replace(k, "?"));
    }
    let (memo, m1) = ("memo".into(), "m1".into());
    let own = cat.upsert(
        &writer,
        &memo,
        &m1,
        de(json!({"l": g1})),
        Default::default(),
    );
    let own = own.await;
    let as_none = answers[..2] == answers[2..] && !answers.contains(&"None".to_string());
    held.push(said(as_none && own.is_ok(), format!("a reference to a link onto bob's, and a link onto it, answer alice as one to no link: {answers:?}; a reference to her own: {own:?}")));
    // A second link where one is allowed names the first only where its
    // writer is answered it, a link with its ends.
    let no_end = "linked to no end";
    let hers = [
        ["tag", "g1", "tags", "c3:t1", "linked to `c1:t1`"],
        ["tag", "g2", "tags", "c1:t1", no_end],
        ["note", "n2", "about", g1, no_end],
        ["note", "n1", "about", g3, "linked to `g1:c1%3At1`"],
    ];
    held.extend(links_said(&l, &writer, &hers).await);
    // A call proposed on it is listed to her without its write, and is
    // not hers to decide.
    let on_bobs = json!({"write": "call", "verb": "seen", "key": g2, "payload": {}});
    let proposed = cat.propose(&l.own, de(on_bobs), "the owner's".into()).await;
    let Outcome::Proposed { proposal, .. } = proposed.must("the owner's proposal")? else {
        return Err(Failure::new("a proposal must be proposed"));
    };
    // Her own on g3's link is pinned: she holds `only` two links down.
    let pinned = call(writer.clone(), "seen", g3).await?;
    let listed = cat.proposals(&writer, Default::default()).await;
    let listed = listed.must("alice's listing")?.data;
    let bobs = listed.iter().filter(|p| p.id == proposal);
    let bobs: Vec<_> = bobs.map(|p| p.write.is_none() && p.more).collect();
    let rejected = cat.reject(&writer, &proposal, "no".into()).await;
    let kept = bobs == [true] && reason(&rejected) == Some(&Refusal::NotGranted);
    held.push(said(kept, format!("a call proposed on a link onto bob's is listed to alice once, without its write, and she does not reject it ({rejected:?}): {listed:?}")));
    // `c3` handed to bob: g3's link is hers as of before, and found by
    // no search of hers, which ranks at the head.
    let handed = l.upsert("claim", "c3", json!({"owner": "bob"})).await?;
    let before = Position(handed.position().0 - 1);
    let alice = alice(s, json!({"restricted": true}));
    held.push(finds(&l, &alice, DEEP_HANDED, before).await);
    let found = search(alice, Some(before)).await?;
    held.push(said(
        found == "g1:c1%3At1",
        format!("alice finds `{found}` as of before the handover"),
    ));
    // And her pinned proposal is accepted no more: an end's end moved.
    let accepted = cat.accept(&l.own, &pinned).await;
    let case = accepted.as_ref().err().and_then(Error::case);
    let moved = matches!(
        case.map(|s| &s.case),
        Some(Case::WriteChangedSinceProposed { .. })
    );
    held.push(said(
        moved,
        format!(
            "alice's proposal on g3's link, `c3` handed to bob since, is accepted: {accepted:?}"
        ),
    ));
    // One on g1's link, where nothing moved, is accepted.
    let accepted = cat
        .accept(&l.own, &call(writer.clone(), "seen", g1).await?)
        .await;
    held.push(said(
        accepted.is_ok(),
        format!("alice's proposal on g1's link, nothing moved since, is accepted: {accepted:?}"),
    ));
    // Without `restricted`, a link onto a link of a held record is not
    // there: no second link names it, and a call on it is not there to a
    // reviewer, decided as an id never made, where one on another link is
    // decided.
    restrict(s, "claim", "c1", "held").await?;
    let plain = holder(s, "plain", json!({"default": "write", "review": true}));
    let rows = "sys.link.tag.tags | no link onto a held claim's | {} | g2:c2%3At1 g3:c3%3At1";
    held.push(finds(&l, &plain, rows, Position::GENESIS).await);
    let held_from = [
        ["tag", "g1", "tags", "c2:t1", no_end],
        ["note", "n1", "about", g2, no_end],
        ["tag", "g2", "tags", "c3:t1", "linked to `c2:t1`"],
    ];
    held.extend(links_said(&l, &plain, &held_from).await);
    let on_held = json!({"write": "call", "verb": "seen", "key": g1, "payload": {}});
    let proposed = cat.propose(&l.own, de(on_held), "the owner's".into()).await;
    let Outcome::Proposed {
        proposal: on_held, ..
    } = proposed.must("the owner's proposal")?
    else {
        return Err(Failure::new("a proposal must be proposed"));
    };
    let decide = async |id: &ProposalId| {
        let rejected = cat.reject(&plain, id, "no".into()).await;
        [rejected, cat.accept(&plain, id).await]
    };
    let (decided, never) = (decide(&on_held).await, decide(&"none".into()).await);
    let kept = (decided.iter().zip(&never)).all(|(d, n)| as_never(d, n, &on_held));
    let other = cat.reject(&plain, &proposal, "no".into()).await;
    held.push(said(kept && other.is_ok(), format!("a call on a link onto a link of a held claim is decided by one without `restricted`: {decided:?}; one on another link: {other:?}")));
    // One who held `restricted` when it proposed has its call applied.
    let accepted = cat
        .accept(&l.own, &call(free.clone(), "seen", g1).await?)
        .await;
    held.push(said(accepted.is_ok(), format!("a call on a link onto a link of a held claim, proposed with `restricted`, is accepted: {accepted:?}")));
    // Two link types a migration made each other's end are each asked
    // once, and a link between them is judged as far as its records go.
    let ring = |name: &str, r: &str, target: &str| {
        let mut t = type_decl(name, vec![field("name", Text)]);
        let mark = json!({"name": "mark", "type": {"kind": "text"}});
        t.relations = vec![de(json!({"name": r, "target": target, "fields": [mark]}))];
        t
    };
    let own = &l.own;
    let migrated = |to| async move {
        let m = crate::conformance::fixture::migration(to);
        let report = cat.migrate(own, m.clone()).await.must("previewing")?;
        let done = cat.migrate_apply(own, m, &report.data.report_id).await;
        done.must("migrating a type of the ring").map(drop)
    };
    l.declare(ring("ringb", "s", "trip")).await?;
    l.declare(ring("ringa", "r", "sys.link.ringb.s")).await?;
    for (ty, from, on, to) in [("ringb", "b1", "s", "t1"), ("ringa", "a1", "r", "b1:t1")] {
        l.upsert(ty, from, json!({"name": from})).await?;
        let (ty, from, on, to) = (ty.into(), from.into(), on.into(), to.into());
        let (none, opts) = (Default::default(), Default::default());
        let linked = cat.link(&l.own, &ty, &from, &on, &to, none, opts);
        linked.await.must("linking in the ring")?;
    }
    migrated(ring("ringb", "s", "sys.link.ringa.r")).await?;
    let rows = r#"sys.link.ringa.r | a ring of link types | {"total": true} | a1:b1%3At1 total=1"#;
    held.push(finds(&l, &plain, rows, Position::GENESIS).await);
    // The end of `b1`'s link is a key no record holds now, and is named
    // to no one; with `b1` held, the link onto its link is not there.
    let second = [["ringb", "b1", "s", "a1:b1%3At1", no_end]];
    held.extend(links_said(&l, &plain, &second).await);
    restrict(s, "ringb", "b1", "held").await?;
    let rows =
        r#"sys.link.ringa.r | a ring's link, an end's end held | {"total": true} |  total=0"#;
    held.push(finds(&l, &plain, rows, Position::GENESIS).await);
    // The relation `s` is not removed while `r` targets its link type
    // (lakelands#920); a call on the link onto `b1`'s reaches the held
    // `b1`, so one proposed with `restricted` is applied.
    let rung = json!({"declare": "verb", "name": "rung", "on": "sys.link.ringa.r",
        "risk": "high", "effect": "write", "set": [{"field": "mark", "value": "'x'"}]});
    l.declare_json(rung).await?;
    let removal =
        crate::conformance::fixture::migration(type_decl("ringb", vec![field("name", Text)]));
    let removal = cat.migrate(own, removal).await;
    let refused = matches!(
        crate::conformance::fixture::invalid_case(&removal),
        Some(Case::MigrationTakesTarget { .. })
    );
    held.push(said(
        refused,
        format!("the removal of `s` while `r` targets its link type is refused: {removal:?}"),
    ));
    let rung = call(free, "rung", "a1:b1%3At1").await?;
    let rejected = cat.reject(&plain, &rung, "no".into()).await;
    let never = cat.reject(&plain, &"none".into(), "no".into()).await;
    let accepted = cat.accept(&l.own, &rung).await;
    let kept = as_never(&rejected, &never, &rung) && accepted.is_ok();
    held.push(said(kept, format!("a call on a link onto a link whose relation is removed, its end held: one without `restricted` is answered its reject as an id never made ({rejected:?}), and, proposed with `restricted`, it is accepted ({accepted:?})")));
    // Nor is such a link there to a verb or an `unlink`: of alice on the
    // link onto bob's, of the one without `restricted` on the link onto
    // the held claim's. Asked last: where one is applied the link is gone.
    held.push(gone(&l, &writer, g2, ("g2", "c2:t1")).await);
    held.push(gone(&l, &plain, g1, ("g1", "c1:t1")).await);
    let wrong: Vec<String> = held
        .into_iter()
        .filter_map(|h| Some(h.err()?.message))
        .collect();
    ensure!(
        wrong.is_empty(),
        "a link whose end is a link is answered where that link is: {wrong:#?}"
    );
    Ok(())
}

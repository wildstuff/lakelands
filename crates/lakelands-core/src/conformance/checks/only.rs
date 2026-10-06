//! A grant may admit only some records of a type
//! (`docs/decisions/051-a-grant-may-admit-only-some-records.md`).
//!
//! `only` on a type admits the records whose fields hold its values at
//! every door that reads records, and every write that reaches the type
//! or its shape is refused to its holder before anything is read.

use std::fmt::Debug;
use std::sync::Arc;

use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::principal::PrincipalKind;
use lakelands_api::query::{Hits, QueryResult};
use lakelands_api::read::Found;
use lakelands_api::write::Outcome;
use lakelands_api::{
    Error, LakeStore, Position, Principal, RecordKey, Refusal, RelationName, TypeName, Value,
    VerbName,
};
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, field, head_unmoved, principal, searchable, type_decl, vector, view_of,
};

check!(
    /// Every read answers a holder of `only` its admitted records alone.
    ONLY_READS = a_grant_admits_only_its_records_at_every_read,
    "051: a principal whose grant holds `only` on a type is answered only the records whose fields hold its values: `get` of another's record is not found, `find` with a filter, an order, `total`, a count, a sum and a group count and sum its own alone, a read as of a position judges the records as they stood then, and a count of a kept view under its pairs, at the head and as of earlier, counts its own group"
);

check!(
    /// Search answers a holder of `only` its admitted records alone.
    ONLY_SEARCH = a_search_finds_only_admitted_records,
    "051: a lexical, a vector and a hybrid search by a holder of `only` find only its admitted records, however they rank; a search as of a position keeps a candidate only where it is admitted then and at the head, a link by its ends, under the grant followed to the head, so a field renamed since changes nothing"
);

check!(
    /// A view is computed for a holder of `only` from its admitted records.
    ONLY_VIEWS = a_view_answers_a_holder_its_own_groups,
    "051: a view grouped by every field a source's `only` names answers its holder the groups it is admitted, kept or not; one that is not is computed from the admitted records alone, a view over it too; `only` on a view admits its groups by their fields; `describe_type` says `only some` of such a view to its holder"
);

check!(
    /// A link record is admitted where both its ends are.
    ONLY_LINKS = a_link_is_answered_where_its_ends_are_admitted,
    "051: a walk by a holder of `only` reaches only admitted records; a link type whose end type holds `only` answers `get`, `find`, `total` and a count over the links whose ends are admitted; `only` on a link type admits its links by their fields in a walk too"
);

check!(
    /// A holder of `only` writes nothing that reaches the type.
    ONLY_WRITES = every_write_by_a_holder_of_only_is_refused,
    "051: `upsert`, `retire`, `link`, `unlink`, a verb on the type, `propose`, `accept` and `reject` of a write to it, `ingest` and an aging rule on it are refused to a holder of `only` as not granted, naming no field, appending nothing; a proposal on another's record, retired or not, is listed to it without its write; a retry under a key used before the grant held answers as before; a reference into the type from another type, through `upsert`, `propose` or `ingest`, must name an admitted record"
);

check!(
    /// A holder of `only` changes no shape.
    ONLY_SCHEMA = a_holder_of_only_changes_no_shape,
    "051: `declare`, `apply`, `recompute`, `add_field`, `migrate`, `migrate_apply`, `migrate_value_type`, `migrate_value_type_apply`, the proposal of a declaration or a migration of a type or a value type, and `accept` and `reject` of a schema proposal are refused to a principal holding `only` on any type, appending nothing, and a migration's proposal, of a type or a value type, is not listed to it"
);

check!(
    /// A grant `only` that cannot be applied is refused at the doors that name its type.
    ONLY_GRANT_CHECKED = a_grant_that_cannot_be_applied_is_refused,
    "051: `describe_type` tells a holder its `only`, writes nothing and lists no verb or link of the type; `only` on a field the type does not declare, of another kind than text or reference, with a value that is not text, or on a field the holder may not read, refuses the doors that name the type as invalid, naming the type and never the field; the grant follows a rename of its field, and after the field is dropped a read at the head is refused while one as of before is answered; a walk or a view that reaches a type whose grant cannot be applied admits none of its records"
);

/// A value of the door API from its JSON.
fn de<T: DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).expect("the check's JSON reads")
}

/// A principal `id` with the grants given as JSON.
fn holder(store: &dyn LakeStore, id: &str, grants: Value) -> Principal {
    principal(store, PrincipalKind::Agent, id, de(grants))
}

/// `alice` reads every type and is admitted only her own claims; `more`
/// adds to her grants.
fn alice(store: &dyn LakeStore, more: Value) -> Principal {
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
async fn claims(l: &Lake, views: bool) -> Result<Position, Failure> {
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
fn names(key: &str) -> (TypeName, RecordKey, RelationName, RecordKey) {
    ("claim".into(), key.into(), "on_trip".into(), "t1".into())
}

/// `who`'s `get` of the record `key` of `ty` at `at`.
async fn get(
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
fn claim_with(owner: &str) -> lakelands_api::ontology::TypeDecl {
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
    let report = l
        .cat
        .migrate(&l.own, m.clone())
        .await
        .must("previewing a migration of `claim`")?;
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
fn hit_keys(hits: &Hits) -> String {
    let keys: Vec<&str> = hits.hits.iter().map(|h| h.record.key.as_str()).collect();
    keys.join(" ")
}

/// The keys of a walk's nodes of `ty`, in order.
fn node_keys(result: &QueryResult, ty: &str) -> String {
    let QueryResult::Graph { nodes, .. } = result else {
        return format!("{result:?}");
    };
    let keys = nodes.iter().filter(|n| n.ty.as_str() == ty);
    keys.map(|n| n.key.as_str()).collect::<Vec<_>>().join(" ")
}

/// Fails unless each `find` of `rows`, one a line `type | what | find |
/// answer`, answers `who` as its line says; `BEFORE` in a find is `before`.
async fn finds(l: &Lake, who: &Principal, rows: &str, before: Position) -> Result<(), Failure> {
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

/// Fails unless `answer` is refused as not granted, naming no field.
fn not_granted<T: Debug>(what: &str, answer: lakelands_api::Result<T>) -> Result<(), Failure> {
    let (field, reason) = (None, Refusal::NotGranted);
    let refused = Error::Refused { field, reason }.to_string();
    ensure!(
        answer.as_ref().err().map(ToString::to_string) == Some(refused),
        "{what} by a holder of `only` must be refused as not granted, naming no field; it answered {answer:?}"
    );
    Ok(())
}

/// Fails unless `answer` is not found, as for a record that is not there.
fn not_found<T: Debug>(what: &str, answer: lakelands_api::Result<T>) -> Result<(), Failure> {
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

async fn a_grant_admits_only_its_records_at_every_read(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
    get("c3", Some(before))
        .await
        .must("alice's `get` of `c3` as of before")?;
    not_found("alice's `get` of `c3` at the head", get("c3", None).await)?;
    finds(&l, &alice, HANDED, before).await
}

const SEARCHES: &str = r#"
    lexical | {"mode": "lexical", "text": "taxi"} | c1 | c1
    vector | {"mode": "vector", "field": "v", "vector": [1.0, 0.0]} | c1 c3 | c1
    hybrid | {"mode": "hybrid", "text": "taxi", "field": "v", "vector": [1.0, 0.0]} | c1 c3 | c1
"#;

async fn a_search_finds_only_admitted_records(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
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
    let hits = cat
        .search(alice, de(link))
        .await
        .must("alice's search of the links")?;
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

async fn a_view_answers_a_holder_its_own_groups(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
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
    finds(&l, &carol, HELD_VIEWS, Position::GENESIS).await
}

const LINKS: &str = r#"
    sys.link.claim.on_trip | the links | {} | c1:t1 c3:t1
    sys.link.claim.on_trip | their total | {"total": true} | c1:t1 c3:t1 total=2
    sys.link.claim.on_trip | their count | {"aggregate": {"measures": [{"name": "n", "func": "count"}]}} | n=2
"#;

async fn a_link_is_answered_where_its_ends_are_admitted(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
    let alice = alice(store.as_ref(), json!({}));
    walked(alice.clone(), "c1 c3").await?;
    finds(&l, &alice, LINKS, Position::GENESIS).await?;
    let link = get(&l, &alice, "sys.link.claim.on_trip", "c2:t1", None).await;
    not_found("the link from bob's `c2` to alice", link)?;
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

async fn every_write_by_a_holder_of_only_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    claims(&l, false).await?;
    let mut memo = crate::conformance::fixture::reference("claim", "claim");
    memo.authority = lakelands_api::ontology::Authority::Shared;
    l.declare(type_decl("memo", vec![memo])).await?;
    for (name, risk) in [("pay", "low"), ("propose_pay", "high")] {
        let set = json!([{"field": "note", "value": "'paid'"}]);
        let verb = json!({"declare": "verb", "name": name, "on": "claim", "risk": risk,
                          "effect": "write", "set": set});
        l.declare_json(verb).await?;
    }
    let (ty, c1, on, t1) = names("c1");
    let (none, opts) = (Default::default, Default::default);
    // Bob proposes a change to his claim and a claim of his; alice writes
    // `c1` under a key before her grant holds `only`, and a retry answers
    // as that did.
    let bob = holder(store.as_ref(), "bob", json!({"default": "write"}));
    let c2 = RecordKey::from("c2");
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
    let more = json!({"default": "write", "review": true, "ingest": true});
    let alice = alice(store.as_ref(), more);
    let again = cat.upsert(&alice, &ty, &c1, mine(), keyed).await;
    let again = again.must("alice's retry under the same key")?;
    ensure!(
        again == first,
        "a retry must answer as its first call did, {first:?}; it answered {again:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let (verb, high) = (VerbName::from("pay"), VerbName::from("propose_pay"));
    let retire = de(json!({"write": "retire", "type": "claim", "key": "c1"}));
    let record = de(json!({"type": "claim", "key": "c9", "fields": {"owner": "alice"}}));
    let rule = json!({"on": "claim", "field": "note", "from": "taxi", "to": "cab", "when": "true"});
    let rules = TypeName::from(lakelands_api::seam::AGING_RULE_TYPE);
    let feed = crate::conformance::fixture::ingest(cat, &alice, "feed", vec![record]);
    not_granted(
        "`upsert`",
        cat.upsert(&alice, &ty, &c1, mine(), opts()).await,
    )?;
    not_granted("`retire`", cat.retire(&alice, &ty, &c1, opts()).await)?;
    not_granted(
        "`link`",
        cat.link(&alice, &ty, &c1, &on, &t1, none(), opts()).await,
    )?;
    not_granted(
        "`unlink`",
        cat.unlink(&alice, &ty, &c1, &on, &t1, opts()).await,
    )?;
    not_granted(
        "a verb",
        cat.call(&alice, &verb, Some(&c1), none(), opts()).await,
    )?;
    not_granted(
        "a high-risk verb",
        cat.call(&alice, &high, Some(&c1), none(), opts()).await,
    )?;
    not_granted(
        "`propose`",
        cat.propose(&alice, retire, "mine".into()).await,
    )?;
    not_granted("`accept`", cat.accept(&alice, &proposals[0]).await)?;
    not_granted(
        "`reject`",
        cat.reject(&alice, &proposals[0], "no".into()).await,
    )?;
    not_granted("`ingest`", feed.await)?;
    let ruled = cat
        .upsert(&alice, &rules, &"r1".into(), de(rule), opts())
        .await;
    not_granted("an aging rule on the type", ruled)?;
    head_unmoved(store.as_ref(), head, "every write by alice").await?;
    // Bob's proposals on his claims are listed to alice without their
    // write, and so once his `c2` is retired.
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
            bobs.len() == 2 && bobs.iter().all(|p| p.write.is_none() && p.more),
            "bob's proposals must be listed to alice without their write, `c2` retired: {retired}; she is listed {bobs:?}"
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
        let says = other.as_ref().err().map(ToString::to_string);
        ensure!(
            says.is_some_and(|s| s.contains("not a live record")),
            "a memo naming bob's `c3` through {what} must be refused as naming no live record; it answered {other:?}"
        );
    }
    let own = cat.upsert(&alice, &memo, &m1, named("c1"), opts()).await;
    own.must("alice's memo naming her own `c1`")?;
    Ok(())
}

async fn a_holder_of_only_changes_no_shape(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
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
        let proposed = cat
            .propose(own, de(write.clone()), "the owner's".into())
            .await;
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
    let vt = cat.migrate_value_type(&alice, label.clone()).await;
    not_granted("`migrate_value_type`", vt)?;
    let vt = cat.migrate_value_type_apply(&alice, label, "x").await;
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
    let listed = cat
        .proposals(&alice, Default::default())
        .await
        .must("alice's listing")?;
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
    a value that is not text | {"only": {"owner": 1}}
    a field she may not read | {"only": {"owner": "alice"}, "fields": {"owner": "masked"}}
"#;

async fn a_grant_that_cannot_be_applied_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    claims(&l, false).await?;
    let set = json!([{"field": "note", "value": "'paid'"}]);
    let verb = json!({"declare": "verb", "name": "pay", "on": "claim", "risk": "low",
                      "effect": "write", "set": set});
    l.declare_json(verb).await?;
    let alice = alice(store.as_ref(), json!({"default": "write"}));
    let told = l
        .cat
        .describe_type(&alice, &"claim".into(), Default::default())
        .await;
    let told = told.must("describing `claim` to alice")?.data;
    let (may, verbs) = (&told.may, &told.verbs);
    let only = serde_json::to_value(&may.only).expect("a map reads");
    let writes = may.upsert || may.retire || !may.link.is_empty() || !verbs.is_empty();
    ensure!(
        only == json!({"owner": "alice"}) && !writes,
        "`describe_type` must tell alice her `only` and no write, verb or link of `claim`; it says {may:?} and verbs {verbs:?}"
    );
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
        let named = |e: &Error| {
            ["payer", "`owner`"]
                .iter()
                .all(|f| !e.to_string().contains(f))
        };
        let refused = |e: &Error| matches!(e, Error::Refused { field: None, .. }) && named(e);
        if !said
            .as_ref()
            .is_err_and(|e| refused(e) && e.to_string().contains("`claim` cannot be applied"))
        {
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
        dropped
            .as_ref()
            .is_err_and(|e| e.to_string().contains("cannot be applied")),
        "after `owner` is dropped alice's `find` at the head must be refused; it answered {dropped:?}"
    );
    Ok(())
}

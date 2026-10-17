//! An object set of a view the lake keeps: its memberships are decided at
//! every write that moves a group, in that write's own append, as a read
//! of the view would answer the set's condition
//! (`docs/decisions/066-a-set-holds-a-kept-view.md`).

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::event::EventBody;
use lakelands_api::location::Limits;
use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::{AGING_RULE_TYPE, Seams};
use lakelands_api::write::ProposedWrite;
use lakelands_api::{
    Access, Grants, LakeStore, Outcome, Position, PrincipalKind, Refusal, SourceId, Timestamp,
    TypeName, WriteOpts,
};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Lake, Must, all_found, events_from, find_where, head_unmoved, ingest, limits, migration, patch,
    principal, refused_on, restrict, source_record,
};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    KEPT_SET = a_set_of_a_kept_view_moves_in_the_append_of_every_write_that_moves_its_group,
    "049 and 055, extended (lakelands#1185): an object set of a kept view, over a merged source's measure, over a maximum and over a view of that view, is decided at every write that moves a group of the view, in that write's own append and caused by its event, at every door: `upsert`, a verb, `ingest` (two writes of one group in one append deciding once), `retire` and revival, a purge, an aging move and an accept; its answer names the set; a write that moves no group appends nothing more; after each step the members are the groups a read of the view's unkept twin answers for the set's condition, the group whose maximum's holder left, the group that ended and a group whose sum the fold cannot keep exactly among them; a read's `in_set` answers the kept members at the head and as of an earlier position, and a rebuild keeps them",
    given
);

check!(
    KEPT_SET_REFUSED = what_would_unkeep_a_set_of_a_view_is_refused,
    "049 and 055, extended (lakelands#1185): a set of a view the lake does not keep is refused `set_of_view`, saying to set `view_kept.<view>` first; while a set holds a kept view, the setting written `false` or retired, of the view or of one it reads through, and the view declared again so that it cannot be kept, are refused `view_kept_for_set` naming the set; a migration of a source that moves a field the view reads, a source declared again so that a field the view reads is computed again, and a value type's rename under a source's field the view reads, are refused `view_source_stepped` naming the set and the source; each appends nothing; a migration that moves no field the view reads lands, moving no membership",
    given
);

check!(
    KEPT_SET_REMADE = a_set_of_a_kept_view_is_decided_where_the_view_is_made_again,
    "049 and 055, extended (lakelands#1185): the view declared again with another `where`, a setting its measure names written, and the set declared again with another condition decide every group in their own append, caused by their event; a group whose measure fails under the setting leaves every set, as a read of the view refuses it, and so does every group of a view over it while one group it reads fails, decided again at the write that makes that group fail or clears it; a set whose `where` names a setting is decided again at that setting's write; a group whose sum stays inexact while its failing measure clears decides every group of the view over it; a setting declared with a value type is stamped to its scale, which decides a set's condition and a view's measure that read it; a source declared again with a field the view does not read moves nothing; after each the members are what the twin answers",
    given
);

check!(
    KEPT_SET_BOUND = what_a_set_of_a_kept_view_moves_is_held_to_the_ceiling,
    "004 and 055, extended (lakelands#1185): past the lake's `max_set_changes`, counted over the memberships a write moves in the sets of kept views with what it moves across an edge, the write is refused `too_large` naming the set to a writer shown it and nothing to another, a setting its measure names past it too, and a set's declaration over more groups than that, each appending nothing; and an aging move past it is counted as refused, `too_large` listed with its key, and no failed guard (lakelands#1182), the next move of the same group decided as if it stood, the holder of an extreme that leaves after a refused move read from the order that move left as it was",
    given
);

check!(
    KEPT_SET_HELD = a_set_of_a_kept_view_is_a_readers_only_where_nothing_of_a_source_is_held,
    "055 and 024, extended (lakelands#1185): a set of a kept view is a reader's only where nothing of a source is held from it: to one without `restricted`, holding `only` on a source, or kept from a field a source's measure reads, it is `set_undeclared` in a read's `in_set` and `describe` names it not, and twin lakes that differ in a held invoice answer such a reader alike; to a reader holding `restricted` and every field it is described and read; a writer is told the set moved only where it is answered the group, one holding `only` on a type no view reads among them"
);

/// The invoices, the charges a source feeds, the view `balance` of them by
/// customer, its twin no setting keeps, and the views `heavy` of each,
/// and the sets of the kept ones.
async fn lake(l: &Lake) -> Held {
    let (text, dec) = (StorageKind::Text, StorageKind::Decimal);
    let field = |n: &str, k: StorageKind| json!({"name": n, "type": {"kind": serde_json::to_value(k).unwrap_or_default()}});
    let sourced = |n: &str, k| {
        let mut f = field(n, k);
        f["authority"] = json!("source");
        f
    };
    let ty =
        |name: &str, fields: Vec<Value>| json!({"declare": "type", "name": name, "fields": fields});
    l.declare_json(json!({"declare": "value_type", "name": "state", "kind": "text"}))
        .await?;
    let mut status = field("status", text);
    status["type"]["value_type"] = json!("state");
    l.declare_json(ty(
        "invoice",
        vec![field("customer", text), field("amount", dec), status],
    ))
    .await?;
    l.declare_json(ty(
        "charge",
        vec![sourced("customer", text), sourced("amount", dec)],
    ))
    .await?;
    for end in ["", "_twin"] {
        l.declare_json(balance(end, "status = 'open'", "total + charged"))
            .await?;
        l.declare_json(json!({"declare": "type", "name": format!("heavy{end}"),
            "fields": [field("n", StorageKind::Number), field("held", dec)],
            "view": {"source": format!("balance{end}"), "where": "due > 100", "group_by": [],
                     "measures": [{"name": "n", "expr": "count()"}, {"name": "held", "expr": "sum(due)"}]}}))
            .await?;
    }
    for view in ["balance", "heavy"] {
        l.set(&format!("view_kept.{view}"), json!(true)).await?;
    }
    for (name, of, filter) in SETS {
        l.declare_json(set(name, of, filter)).await?;
    }
    Ok(())
}

/// The view `balance` and its twin (`end`), over the open invoices where
/// `filter` holds, merging the charges in, `due` computed as `due`.
fn balance(end: &str, filter: &str, due: &str) -> Value {
    let field = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    let measures = json!([{"name": "n", "expr": "count()"}, {"name": "total", "expr": "sum(amount)"},
        {"name": "top", "expr": "max(amount)"}, {"name": "due", "expr": due}]);
    let merge = json!([{"source": "charge", "group_by": ["customer"],
        "measures": [{"name": "charged", "expr": "sum(amount)"}]}]);
    let fields = ["total", "top", "charged", "due"].map(|f| field(f, "decimal"));
    let fields = [
        vec![field("customer", "text"), field("n", "number")],
        fields.to_vec(),
    ]
    .concat();
    json!({"declare": "type", "name": format!("balance{end}"), "key": ["customer"], "fields": fields,
        "view": {"source": "invoice", "where": filter, "group_by": ["customer"], "measures": measures,
                 "merge": merge}})
}

/// The sets of the kept views: their name, their view and their condition.
const SETS: [(&str, &str, &str); 3] = [
    ("owing", "balance", "due > 100"),
    ("topped", "balance", "top >= 80"),
    ("crowded", "heavy", "n >= 2"),
];

fn set(name: &str, of: &str, filter: &str) -> Value {
    json!({"declare": "object_set", "name": name, "of": of, "where": filter})
}

/// The members of every set as the log's memberships leave them.
async fn logged(s: &dyn LakeStore) -> Result<BTreeMap<String, BTreeSet<String>>, Failure> {
    let mut members: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in events_from(s, Position(1)).await? {
        if let EventBody::ObjectSetChanged {
            set, key, joined, ..
        } = e.body
        {
            let held = members.entry(set).or_default();
            match joined {
                true => held.insert(key.to_string()),
                false => held.remove(key.as_str()),
            };
        }
    }
    Ok(members)
}

/// Fails unless each set's members, as the log leaves them, are the keys
/// the view's twin answers for its condition.
async fn oracle(l: &Lake, s: &dyn LakeStore, sets: &[(&str, &str, &str)], when: &str) -> Held {
    let logged = logged(s).await?;
    for (name, of, filter) in sets {
        let twin = format!("{of}_twin");
        let read = all_found(l.cat.as_ref(), &l.own, &twin, find_where(filter)).await?;
        let want: BTreeSet<String> = read.into_iter().map(|r| r.key.to_string()).collect();
        let got = logged.get(*name).cloned().unwrap_or_default();
        ensure!(
            got == want,
            "{when}: `{name}` holds {got:?}, and a read of `{twin}` where {filter} answers {want:?}"
        );
    }
    Ok(())
}

/// The memberships the events from `at` change, each `(set, key, joined)`,
/// and whether each is caused by the first record event from `at`: a
/// verb's call comes before the write it makes.
async fn moved(
    s: &dyn LakeStore,
    at: Position,
) -> Result<(Vec<(String, String, bool)>, bool), Failure> {
    let (mut out, mut caused) = (Vec::new(), true);
    let events = events_from(s, at).await?;
    let written = |b: &EventBody| {
        matches!(
            b,
            EventBody::RecordUpserted { .. }
                | EventBody::RecordIngested { .. }
                | EventBody::RecordRetired { .. }
        )
    };
    let cause = events.iter().find(|e| written(&e.body)).map(|e| e.position);
    for e in events {
        if let EventBody::ObjectSetChanged {
            set, key, joined, ..
        } = e.body
        {
            caused &= e.cause == cause;
            out.push((set, key.to_string(), joined));
        }
    }
    out.sort();
    Ok((out, caused))
}

fn rows(r: &[(&str, &str, bool)]) -> Vec<(String, String, bool)> {
    let mut r: Vec<_> = (r.iter())
        .map(|(s, k, j)| ((*s).to_owned(), (*k).to_owned(), *j))
        .collect();
    r.sort();
    r
}

async fn next(s: &dyn LakeStore) -> Result<Position, Failure> {
    Ok(s.head().await.must("the head")?.next())
}

/// One step of a walk: what it moves, caused by its event, and the oracle
/// after it.
async fn stepped(
    l: &Lake,
    s: &dyn LakeStore,
    at: Position,
    want: &[(&str, &str, bool)],
    when: &str,
) -> Held {
    let (got, caused) = moved(s, at).await?;
    ensure!(
        got == rows(want) && caused,
        "{when} moves {:?} caused by its event; it moved {got:?} (caused by it: {caused})",
        rows(want)
    );
    oracle(l, s, &SETS, when).await
}

async fn a_set_of_a_kept_view_moves_in_the_append_of_every_write_that_moves_its_group(
    store: Store,
    given: Given,
) -> Held {
    let _ = given;
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    lake(&l).await?;
    let verb = json!({"declare": "verb", "name": "close", "on": "invoice", "risk": "low",
        "effect": "write", "set": [{"field": "status", "value": "'closed'"}]});
    l.declare_json(verb).await?;
    let open = |c: &str, a: &str| json!({"customer": c, "amount": a, "status": "open"});
    let at = next(s).await?;
    l.upsert("invoice", "i1", open("c1", "60")).await?;
    stepped(&l, s, at, &[], "`i1` of 60").await?;
    let at = next(s).await?;
    let wrote = l.upsert("invoice", "i2", open("c1", "90")).await?;
    stepped(
        &l,
        s,
        at,
        &[("owing", "c1", true), ("topped", "c1", true)],
        "`i2` of 90",
    )
    .await?;
    let named = matches!(&wrote, Outcome::Applied { set_off, .. }
        if set_off.joined == ["owing", "topped"]);
    ensure!(
        named,
        "the write's answer names `owing` and `topped`; it answered {wrote:?}"
    );
    // A verb closes `i2`: the maximum's holder leaves, and `c1` is owed 60.
    let at = next(s).await?;
    let ty = TypeName::from("invoice");
    let (verb, i2) = ("close".into(), "i2".into());
    let called = l.cat.call(
        &l.own,
        &verb,
        Some(&i2),
        patch(json!({})),
        WriteOpts::default(),
    );
    called.await.must("closing `i2`")?;
    stepped(
        &l,
        s,
        at,
        &[("owing", "c1", false), ("topped", "c1", false)],
        "`i2` closed by a verb",
    )
    .await?;
    // A charge fed: the merged source.
    let at = next(s).await?;
    let fed = vec![source_record(
        "charge",
        "k1",
        json!({"customer": "c1", "amount": "50"}),
    )];
    ingest(l.cat.as_ref(), &l.own, "crm", fed)
        .await
        .must("feeding `k1`")?;
    stepped(&l, s, at, &[("owing", "c1", true)], "`k1` fed").await?;
    // A second customer: the view of the view gains its second row.
    let at = next(s).await?;
    l.upsert("invoice", "i3", open("c2", "200")).await?;
    let both = [
        ("owing", "c2", true),
        ("topped", "c2", true),
        ("crowded", "", true),
    ];
    let ungrouped = lakelands_api::key::UNGROUPED;
    let both = both.map(|(s, k, j)| (s, if k.is_empty() { ungrouped } else { k }, j));
    stepped(&l, s, at, &both, "`i3` of 200").await?;
    // Retired, `c2`'s group ends; written again, it returns.
    let at = next(s).await?;
    l.retire("invoice", "i3").await?;
    let gone = both.map(|(s, k, _)| (s, k, false));
    stepped(&l, s, at, &gone, "`i3` retired").await?;
    let at = next(s).await?;
    l.upsert("invoice", "i3", open("c2", "200")).await?;
    stepped(&l, s, at, &both, "`i3` written again").await?;
    // A write that moves no group past a condition appends nothing more.
    let at = next(s).await?;
    l.upsert("invoice", "i3", json!({"amount": "210"})).await?;
    let more = events_from(s, at).await?.len();
    ensure!(
        more == 1,
        "a write that moves no membership appends its own event alone; it appended {more}"
    );
    // The source purged: `k1` leaves `c1`'s group.
    let at = next(s).await?;
    let purged = s.purge_source(&SourceId::from("crm"), None, None).await;
    purged.must("purging crm")?;
    stepped(
        &l,
        s,
        at,
        &[("owing", "c1", false), ("crowded", ungrouped, false)],
        "crm purged",
    )
    .await?;
    // An aging move closes `i3`.
    let rule = json!({"on": "invoice", "field": "status", "from": "open", "to": "closed", "when": "amount > 150"});
    l.upsert(AGING_RULE_TYPE, "close_large", rule).await?;
    let at = next(s).await?;
    let today = Timestamp::from_date(2026, 3, 1).expect("a date");
    s.age(today).await.must("aging")?;
    let closed = [("owing", "c2", false), ("topped", "c2", false)];
    stepped(&l, s, at, &closed, "`i3` closed by an aging move").await?;
    // An accept.
    let proposed = l.cat.propose(
        &l.own,
        ProposedWrite::Upsert {
            ty: ty.clone(),
            key: "i4".into(),
            patch: patch(open("c3", "500")),
            opts: WriteOpts::default(),
        },
        "why".into(),
    );
    let Outcome::Proposed { proposal, .. } = proposed.await.must("a proposal")? else {
        return Err(Failure::new("an upsert is proposed"));
    };
    let accepted = next(s).await?;
    l.cat.accept(&l.own, &proposal).await.must("the accept")?;
    // The accept's own event comes first; its write causes the changes.
    let (got, _) = moved(s, accepted).await?;
    ensure!(
        got == rows(&[("owing", "c3", true), ("topped", "c3", true)]),
        "the accept of `i4` of 500 takes `c3` into `owing` and `topped`; it moved {got:?}"
    );
    oracle(&l, s, &SETS, "the accept").await?;
    // Two writes of one group in one append decide once, by the second.
    let at = next(s).await?;
    let window = vec![
        source_record("charge", "k2", json!({"customer": "c3", "amount": "-450"})),
        source_record("charge", "k3", json!({"customer": "c3", "amount": "10"})),
    ];
    ingest(l.cat.as_ref(), &l.own, "erp", window)
        .await
        .must("feeding a window")?;
    let (got, _) = moved(s, at).await?;
    ensure!(
        got == rows(&[("owing", "c3", false)]),
        "`k2` and `k3` fed in one window take `c3` out of `owing` once; it moved {got:?}"
    );
    oracle(&l, s, &SETS, "the window").await?;
    // A read's `in_set` answers the kept members, now and as of before.
    let members = |at: Option<Position>| {
        let mut find = find_where("in_set('owing')");
        find.at.as_of = at;
        all_found(l.cat.as_ref(), &l.own, "balance", find)
    };
    let keys = |r: Vec<lakelands_api::Record>| {
        r.into_iter().map(|r| r.key.to_string()).collect::<Vec<_>>()
    };
    let (now, then) = (
        keys(members(None).await?),
        keys(members(Some(Position(at.0 - 1))).await?),
    );
    ensure!(
        now.is_empty() && then == ["c3"],
        "`in_set('owing')` over `balance` answers [] now and [\"c3\"] before the window; it answered {now:?} and {then:?}"
    );
    store.rebuild().await.must("rebuilding the projection")?;
    let at = next(s).await?;
    l.upsert("invoice", "i5", open("c3", "100")).await?;
    stepped(
        &l,
        s,
        at,
        &[("owing", "c3", true)],
        "after a rebuild, `i5` of 100",
    )
    .await?;
    // A sum whose members' magnitudes pass what the fold keeps exactly is
    // decided as a read of the view computes it.
    let at = next(s).await?;
    l.upsert("invoice", "i6", open("c9", "70000000000000000000000000000"))
        .await?;
    let joined = [
        ("owing", "c9", true),
        ("topped", "c9", true),
        ("crowded", ungrouped, true),
    ];
    stepped(&l, s, at, &joined, "`i6` of 7e28").await?;
    let at = next(s).await?;
    l.upsert(
        "invoice",
        "i7",
        open("c9", "-69999999999999999999999999850"),
    )
    .await?;
    stepped(&l, s, at, &[], "`i7`, leaving `c9` a total of 150").await
}

async fn what_would_unkeep_a_set_of_a_view_is_refused(store: Store, given: Given) -> Held {
    let _ = given;
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    lake(&l).await?;
    l.upsert(
        "invoice",
        "i1",
        json!({"customer": "c1", "amount": "150", "status": "open"}),
    )
    .await?;
    // A migration that moves nothing the view reads lands, and the view
    // made again moves no membership.
    let invoice = |amount: &str, note: bool| {
        let mut fields = vec![
            json!({"name": "customer", "type": {"kind": "text"}}),
            json!({"name": "amount", "type": {"kind": amount}}),
            json!({"name": "status", "type": {"kind": "text", "value_type": "state"}}),
        ];
        fields.extend(note.then(|| json!({"name": "note", "type": {"kind": "text"}})));
        serde_json::from_value::<lakelands_api::ontology::TypeDecl>(
            json!({"name": "invoice", "fields": fields}),
        )
        .expect("a type")
    };
    l.migrate(invoice("decimal", true)).await?;
    oracle(&l, s, &SETS, "a migration adding `note`").await?;
    let before = s.head().await.must("the head")?;
    let (setting, own) = (TypeName::from(lakelands_api::seam::SETTING_TYPE), &l.own);
    let unkept = async |key: &str| {
        let (key, value) = (key.into(), patch(json!({"value": false})));
        let wrote = l
            .cat
            .upsert(own, &setting, &key, value, WriteOpts::default());
        wrote.await.map(drop)
    };
    let declare = |d: Value| {
        l.cat
            .declare(own, serde_json::from_value(d).expect("a declaration"))
    };
    // A `where` that reads the clock makes a view one the lake cannot keep.
    let clocked = json!({"declare": "type", "name": "heavy",
        "fields": [{"name": "n", "type": {"kind": "number"}}, {"name": "held", "type": {"kind": "decimal"}}],
        "view": {"source": "balance", "where": "due > 100 and now() is not null", "group_by": [],
                 "measures": [{"name": "n", "expr": "count()"}, {"name": "held", "expr": "sum(due)"}]}});
    let renamed = lakelands_api::write::ValueTypeMigration {
        to: serde_json::from_value(json!({"name": "state", "kind": "text"})).expect("a value type"),
        renames: vec![("open".into(), "live".into())],
    };
    let at = |set: &str| json!([{"code": "object_set", "name": set}]);
    let kept = |ty: &str| json!({"code": "view_kept_for_set", "type": ty, "at": at("crowded")});
    let stepped = json!({"code": "view_source_stepped", "type": "heavy", "source": "invoice"});
    let of_view = json!({"code": "set_of_view", "type": "balance_twin", "at": at("twin")});
    let twin = declare(set("twin", "balance_twin", "due > 1")).await;
    let mut refusals = vec![("a set of a view no setting keeps", twin.map(drop), of_view)];
    for ty in ["balance", "heavy"] {
        let r = unkept(&format!("view_kept.{ty}")).await;
        refusals.push(("`view_kept.<view>` written false", r, kept(ty)));
    }
    let key = "view_kept.balance".into();
    let r = l
        .cat
        .retire(own, &setting, &key, WriteOpts::default())
        .await;
    refusals.push(("`view_kept.balance` retired", r.map(drop), kept("balance")));
    let r = declare(clocked).await.map(drop);
    refusals.push(("`heavy` declared reading the clock", r, kept("heavy")));
    let r = l.cat.migrate(own, migration(invoice("number", true))).await;
    let r = r.map(drop);
    refusals.push(("`invoice.amount` migrated to a number", r, stepped.clone()));
    let r = l.cat.migrate_value_type(own, renamed).await.map(drop);
    refusals.push(("`state` renamed under `invoice.status`", r, stepped));
    for (what, r, want) in refusals {
        crate::conformance::fixture::refused_like(what, r, &want)?;
    }
    head_unmoved(s, before, "the refused writes").await?;
    // A source's field the view reads, computed again.
    let item = |times: &str| {
        json!({"declare": "type", "name": "item", "fields": [
        {"name": "base", "type": {"kind": "decimal"}},
        {"name": "cost", "type": {"kind": "decimal"}, "authority": "derived", "derived": format!("base * {times}")}]})
    };
    l.declare_json(item("2")).await?;
    l.declare_json(json!({"declare": "type", "name": "costs", "fields": [{"name": "total", "type": {"kind": "decimal"}}],
        "view": {"source": "item", "group_by": [], "measures": [{"name": "total", "expr": "sum(cost)"}]}}))
        .await?;
    l.set("view_kept.costs", json!(true)).await?;
    l.declare_json(set("dear", "costs", "total > 10")).await?;
    let before = s.head().await.must("the head")?;
    let r = l
        .cat
        .declare(&l.own, serde_json::from_value(item("3")).expect("a type"))
        .await;
    let want = json!({"code": "view_source_stepped", "source": "item",
        "at": [{"code": "object_set", "name": "dear"}]});
    let what = "`item` declared again so that `cost`, which `costs` reads, computes otherwise";
    crate::conformance::fixture::refused_like(what, r, &want)?;
    head_unmoved(s, before, "the refused writes").await
}

async fn a_set_of_a_kept_view_is_decided_where_the_view_is_made_again(
    store: Store,
    given: Given,
) -> Held {
    let _ = given;
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    lake(&l).await?;
    for (key, c, a, status) in [
        ("i1", "c1", "60", "open"),
        ("i2", "c2", "150", "open"),
        ("i3", "c1", "50", "closed"),
    ] {
        let fields = json!({"customer": c, "amount": a, "status": status});
        l.upsert("invoice", key, fields).await?;
    }
    oracle(&l, s, &SETS, "the invoices").await?;
    // Declared again, the view and its twin each in its own append.
    let again = async |filter: &str, due: &str, want: &[(&str, &str, bool)], when: &str| -> Held {
        let at = next(s).await?;
        l.declare_json(balance("", filter, due)).await?;
        let (got, _) = moved(s, at).await?;
        let by = events_from(s, at)
            .await?
            .into_iter()
            .filter(|e| matches!(e.body, EventBody::ObjectSetChanged { .. }));
        let by: Vec<_> = by.filter_map(|e| e.cause).collect();
        ensure!(
            got == rows(want) && by.iter().all(|c| *c == at),
            "{when} moves {:?}, caused by its declaration; it moved {got:?}, caused by {by:?}",
            rows(want)
        );
        l.declare_json(balance("_twin", filter, due)).await?;
        oracle(&l, s, &SETS, when).await
    };
    let joined = [
        ("owing", "c1", true),
        ("crowded", lakelands_api::key::UNGROUPED, true),
    ];
    again(
        "status != 'void'",
        "total + charged",
        &joined,
        "`balance` over every invoice",
    )
    .await?;
    let off = [
        ("owing", "c1", false),
        ("owing", "c2", false),
        ("crowded", lakelands_api::key::UNGROUPED, false),
    ];
    again(
        "status != 'void'",
        "(total + charged) / setting('rate')",
        &off,
        "`due` divided by an unset rate",
    )
    .await?;
    // A setting the measure names, written, decides every group in its
    // append.
    let set_rate =
        async |rate: Value, want: &[(&str, &str, bool)], when: &str| -> Result<(), Failure> {
            let at = next(s).await?;
            l.set("rate", rate).await?;
            let (got, caused) = moved(s, at).await?;
            ensure!(
                got == rows(want) && (caused || got.is_empty()),
                "{when} moves {:?}, caused by its write; it moved {got:?}",
                rows(want)
            );
            Ok(())
        };
    let on = off.map(|(s, k, _)| (s, k, true));
    set_rate(json!(1), &on, "`rate` written 1").await?;
    oracle(&l, s, &SETS, "`rate` of 1").await?;
    // Divided by zero, each group's measure fails: it leaves every set,
    // and a read of the view is refused.
    let failed = [
        ("owing", "c1", false),
        ("owing", "c2", false),
        ("topped", "c2", false),
        ("crowded", lakelands_api::key::UNGROUPED, false),
    ];
    set_rate(json!(0), &failed, "`rate` written 0").await?;
    let read = l
        .cat
        .find(
            &l.own,
            &"balance_twin".into(),
            lakelands_api::Find::default(),
        )
        .await;
    ensure!(
        read.is_err(),
        "a read of `balance_twin` under a rate of 0 is refused; it answered {read:?}"
    );
    set_rate(
        json!(1),
        &failed.map(|(s, k, _)| (s, k, true)),
        "`rate` written 1 again",
    )
    .await?;
    oracle(&l, s, &SETS, "`rate` of 1 again").await?;
    // A measure that fails over `c1` alone: `heavy`, which reads it, is
    // refused by a read, and its group leaves `crowded` though `c2` and
    // `c4` stay heavy.
    l.upsert(
        "invoice",
        "i4",
        json!({"customer": "c4", "amount": "210", "status": "open"}),
    )
    .await?;
    let at = next(s).await?;
    let fails = "(total + charged) * 100 / (total - 110)";
    l.declare_json(balance("", "status != 'void'", fails))
        .await?;
    let (got, _) = moved(s, at).await?;
    let read = l
        .cat
        .find(&l.own, &"heavy".into(), Default::default())
        .await;
    let want = [
        ("owing", "c1", false),
        ("crowded", lakelands_api::key::UNGROUPED, false),
    ];
    ensure!(
        got == rows(&want) && read.is_err(),
        "`due` failing over `c1` moves {:?}, and a read of `heavy` is refused; it moved {got:?}, and the read answered {read:?}",
        rows(&want)
    );
    // `tier`, kept over `balance` by its count, holds a group for `c1`'s
    // count and one for the others': a write that clears `c1`'s failure
    // decides both, and one that makes it fail again takes both out.
    l.declare_json(json!({"declare": "type", "name": "tier", "key": ["n"],
        "fields": [{"name": "n", "type": {"kind": "number"}}, {"name": "k", "type": {"kind": "number"}}],
        "view": {"source": "balance", "group_by": ["n"], "measures": [{"name": "k", "expr": "count()"}]}}))
        .await?;
    l.set("view_kept.tier", json!(true)).await?;
    l.declare_json(set("tiered", "tier", "k >= 1")).await?;
    let tiered = [("tiered", "1", true), ("tiered", "2", true)];
    for (amount, joined) in [("70", true), ("60", false)] {
        let at = next(s).await?;
        let i1 = json!({"customer": "c1", "amount": amount, "status": "open"});
        l.upsert("invoice", "i1", i1).await?;
        let (got, _) = moved(s, at).await?;
        let mut want = want.to_vec();
        want.extend(tiered);
        let want: Vec<_> = want.into_iter().map(|(s, k, _)| (s, k, joined)).collect();
        ensure!(
            got == rows(&want),
            "`i1` of {amount} moves {:?}, every group of `tier` among them; it moved {got:?}",
            rows(&want)
        );
    }
    let mut back = want.map(|(s, k, _)| (s, k, true)).to_vec();
    back.extend(tiered);
    again(
        "status != 'void'",
        "(total + charged) / setting('rate')",
        &back,
        "`due` over every group again",
    )
    .await?;
    // A set whose `where` names a setting is decided again at its write.
    let over = ("over", "balance", "due > setting('limit')");
    l.set("limit", json!(100)).await?;
    l.declare_json(set(over.0, over.1, over.2)).await?;
    let at = next(s).await?;
    l.set("limit", json!(200)).await?;
    let (got, caused) = moved(s, at).await?;
    let want = rows(&[("over", "c1", false), ("over", "c2", false)]);
    ensure!(
        got == want && caused,
        "`limit` written 200 moves {want:?}, caused by its write; it moved {got:?}"
    );
    oracle(&l, s, &[over], "`limit` of 200").await?;
    // The set declared again with another condition.
    let at = next(s).await?;
    l.declare_json(set("owing", "balance", "due > 120")).await?;
    let (got, _) = moved(s, at).await?;
    ensure!(
        got == rows(&[("owing", "c1", false)]),
        "`owing` declared again over `due > 120` takes `c1` out; it moved {got:?}"
    );
    let sets = [("owing", "balance", "due > 120"), SETS[1], SETS[2]];
    oracle(&l, s, &sets, "`owing` declared again").await?;
    // A source declared again with a field the view does not read moves
    // nothing.
    let at = next(s).await?;
    l.declare_json(json!({"declare": "type", "name": "invoice", "fields": [
        {"name": "customer", "type": {"kind": "text"}}, {"name": "amount", "type": {"kind": "decimal"}},
        {"name": "status", "type": {"kind": "text", "value_type": "state"}}, {"name": "note", "type": {"kind": "text"}}]}))
        .await?;
    let (got, _) = moved(s, at).await?;
    ensure!(
        got.is_empty(),
        "`invoice` declared again with `note` moves nothing; it moved {got:?}"
    );
    oracle(&l, s, &sets, "`invoice` declared again").await?;
    flag_stays(&l, s).await?;
    stamped(&l, s).await
}

/// A group whose sum the fold cannot keep exactly and whose measure fails
/// then does not: the flag stays while the failure clears, and the groups
/// of the view over it are decided again all the same.
async fn flag_stays(l: &Lake, s: &dyn LakeStore) -> Held {
    let field = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    let inv = vec![field("customer", "text"), field("amount", "decimal")];
    l.declare_json(json!({"declare": "type", "name": "flt_inv", "fields": inv}))
        .await?;
    for end in ["", "_twin"] {
        let fields = ["customer", "n", "total", "due"].map(|f| match f {
            "customer" => field(f, "text"),
            "n" => field(f, "number"),
            _ => field(f, "decimal"),
        });
        l.declare_json(json!({"declare": "type", "name": format!("flt_bal{end}"), "key": ["customer"],
            "fields": fields,
            "view": {"source": "flt_inv", "group_by": ["customer"],
                "measures": [{"name": "n", "expr": "count()"}, {"name": "total", "expr": "sum(amount)"},
                             {"name": "due", "expr": "total * 100 / (total - 110)"}]}}))
            .await?;
        l.declare_json(
            json!({"declare": "type", "name": format!("flt_tier{end}"), "key": ["n"],
            "fields": [field("n", "number"), field("k", "number")],
            "view": {"source": format!("flt_bal{end}"), "group_by": ["n"],
                "measures": [{"name": "k", "expr": "count()"}]}}),
        )
        .await?;
    }
    for view in ["flt_bal", "flt_tier"] {
        l.set(&format!("view_kept.{view}"), json!(true)).await?;
    }
    l.declare_json(set("flt_tiered", "flt_tier", "k >= 1"))
        .await?;
    let sets = [("flt_tiered", "flt_tier", "k >= 1")];
    let (big, back) = (
        "70000000000000000000000000000",
        "-69999999999999999999999999890",
    );
    for (key, customer, amount) in [("f3", "c2", "50"), ("f1", "c1", big), ("f2", "c1", back)] {
        let fields = json!({"customer": customer, "amount": amount});
        l.upsert("flt_inv", key, fields).await?;
    }
    // `c1` sums to 110, so its `due` divides by zero: a read of the view
    // is refused and `c1`'s group is in no set.
    let read = (l.cat)
        .find(&l.own, &"flt_tier_twin".into(), Default::default())
        .await;
    ensure!(
        read.is_err(),
        "a read of `flt_tier_twin` while `c1` divides by zero is refused; it answered {read:?}"
    );
    // The failure clears while the sum stays inexact on the whole face.
    l.upsert("flt_inv", "f5", json!({"customer": "c1", "amount": "1"}))
        .await?;
    oracle(
        l,
        s,
        &sets,
        "the failure of `c1` cleared, its sum still inexact",
    )
    .await
}

/// A setting declared with a value type is stored as that type's scale
/// holds it, which moves a group a set's condition or a view's measure
/// reads it under: both are decided again where the declaration lands.
async fn stamped(l: &Lake, s: &dyn LakeStore) -> Held {
    let field = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    let inv = vec![field("customer", "text"), field("amount", "decimal")];
    l.declare_json(json!({"declare": "type", "name": "stp_inv", "fields": inv}))
        .await?;
    l.declare_json(
        json!({"declare": "value_type", "name": "whole", "kind": "decimal", "scale": 0}),
    )
    .await?;
    l.set("stp_rate", json!("0.5")).await?;
    l.set("stp_limit", json!("100.4")).await?;
    for end in ["", "_twin"] {
        let fields = ["customer", "due", "scaled"]
            .map(|f| field(f, if f == "customer" { "text" } else { "decimal" }));
        l.declare_json(
            json!({"declare": "type", "name": format!("stp_bal{end}"), "key": ["customer"],
            "fields": fields,
            "view": {"source": "stp_inv", "group_by": ["customer"],
                "measures": [{"name": "due", "expr": "sum(amount)"},
                             {"name": "scaled", "expr": "sum(amount) * setting('stp_rate')"}]}}),
        )
        .await?;
    }
    l.set("view_kept.stp_bal", json!(true)).await?;
    l.upsert(
        "stp_inv",
        "s1",
        json!({"customer": "c1", "amount": "100.2"}),
    )
    .await?;
    let sets = [
        ("stp_over", "stp_bal", "due > setting('stp_limit')"),
        ("stp_scaled", "stp_bal", "scaled >= 100"),
    ];
    for (name, of, filter) in sets {
        l.declare_json(set(name, of, filter)).await?;
    }
    oracle(l, s, &sets, "a limit of 100.4 and a rate of 0.5").await?;
    let members =
        async |name: &str| Ok::<_, Failure>(logged(s).await?.remove(name).unwrap_or_default());
    ensure!(
        members("stp_over").await?.is_empty() && members("stp_scaled").await?.is_empty(),
        "a limit of 100.4 and a rate of 0.5 take `c1`, whose due is 100.2, into no set"
    );
    // Each declaration stamps its setting's value to the scale of `whole`:
    // 100.4 becomes 100, and 0.5 becomes 1.
    for (key, set_name) in [("stp_limit", "stp_over"), ("stp_rate", "stp_scaled")] {
        let declared = json!({"declare": "setting", "key": key,
            "type": {"kind": "decimal", "value_type": "whole"}});
        l.declare_json(declared).await?;
        oracle(l, s, &sets, &format!("`{key}` declared whole")).await?;
        ensure!(
            members(set_name).await?.contains("c1"),
            "`{key}` declared with the value type `whole` stamps its value to the scale, which takes `c1` into `{set_name}`"
        );
    }
    Ok(())
}

async fn what_a_set_of_a_kept_view_moves_is_held_to_the_ceiling(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    lake(&l).await?;
    l.declare_json(balance(
        "",
        "status = 'open'",
        "(total + charged) * setting('rate')",
    ))
    .await?;
    l.set("rate", json!(1)).await?;
    for (key, c) in [("i1", "c1"), ("i2", "c2")] {
        l.upsert(
            "invoice",
            key,
            json!({"customer": c, "amount": "150", "status": "open"}),
        )
        .await?;
    }
    for (key, a) in [("a1", "90"), ("a2", "20")] {
        l.upsert(
            "invoice",
            key,
            json!({"customer": "c5", "amount": a, "status": "open"}),
        )
        .await?;
    }
    let rule = json!({"on": "invoice", "field": "status", "from": "open", "to": "closed", "when": "amount < 100"});
    l.upsert(AGING_RULE_TYPE, "close_small", rule).await?;
    let lower = Limits {
        max_set_changes: 1,
        ..limits()
    };
    let store = (given.relimit)(store, Seams::default(), lower)
        .await
        .map_err(|e| Failure::new(format!("opening at a ceiling of one: {e}")))?;
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let before = s.head().await.must("the head")?;
    let large = |r: &lakelands_api::Result<Outcome>| {
        matches!(refused_on(r), Some((_, Refusal::TooLarge { max: 1 })))
    };
    let open = json!({"customer": "c3", "amount": "150", "status": "open"});
    let (ty, i3) = (
        TypeName::from("invoice"),
        lakelands_api::RecordKey::from("i3"),
    );
    let r = l
        .cat
        .upsert(&l.own, &ty, &i3, patch(open.clone()), WriteOpts::default())
        .await;
    ensure!(
        large(&r),
        "`i3` of 150, moving `c3` into `owing` and `topped` under a ceiling of 1, is refused too_large naming a set; it answered {r:?}"
    );
    let blind = principal(
        s,
        PrincipalKind::Agent,
        "blind",
        Grants {
            default: Access::Write,
            ..Grants::default()
        },
    );
    let r = l
        .cat
        .upsert(&blind, &ty, &i3, patch(open), WriteOpts::default())
        .await;
    ensure!(
        matches!(
            &r,
            Err(lakelands_api::Error::Refused {
                field: None,
                reason: Refusal::TooLarge { max: 1 }
            })
        ),
        "to a writer not shown the sets the same write is refused too_large naming nothing; it answered {r:?}"
    );
    let (setting, rate) = (
        TypeName::from(lakelands_api::seam::SETTING_TYPE),
        lakelands_api::RecordKey::from("rate"),
    );
    let r = l
        .cat
        .upsert(
            &l.own,
            &setting,
            &rate,
            patch(json!({"value": 0.5})),
            WriteOpts::default(),
        )
        .await;
    ensure!(
        large(&r),
        "`rate` written 0.5, taking `c1` and `c2` out of `owing`, is refused too_large; it answered {r:?}"
    );
    let r = l
        .cat
        .declare(
            &l.own,
            serde_json::from_value(set("large", "balance", "top > 100")).expect("a set"),
        )
        .await;
    ensure!(
        matches!(
            refused_on(&r),
            Some(("large", Refusal::TooLarge { max: 1 }))
        ),
        "`large`, declared over two groups, is refused too_large naming it; it answered {r:?}"
    );
    head_unmoved(s, before, "the refused writes").await?;
    // An aging run: closing `a1` would take `c5` out of `owing` and
    // `topped` and is refused `too_large`; closing `a2` after it, decided as
    // if `a1` stood, takes `c5` out of `owing` alone.
    let at = next(s).await?;
    let today = Timestamp::from_date(2026, 3, 1).expect("a date");
    let aged = s.age(today).await.must("aging")?;
    let run = aged.rules.iter().find(|r| r.rule == "close_small");
    let (got, _) = moved(s, at).await?;
    ensure!(
        run.is_some_and(|r| r.moved == 1
            && r.failed_guards == 0
            && r.refused == 1
            && matches!(r.refusals.as_slice(),
                [m] if m.key.as_str() == "a1" && matches!(m.why, Refusal::TooLarge { max: 1 })))
            && got == rows(&[("owing", "c5", false)]),
        "the run moves `a2` alone, taking `c5` out of `owing`, `a1` refused `too_large` and no failed guard; it reported {aged:?} and moved {got:?}"
    );
    let two = Limits {
        max_set_changes: 2,
        ..limits()
    };
    let store = (given.relimit)(store, Seams::default(), two)
        .await
        .map_err(|e| Failure::new(format!("opening at a ceiling of two: {e}")))?;
    holder_leaves_after_a_refusal(store.as_ref()).await
}

/// A refused aging move that took an entry from `lo`'s order, then the
/// holder's move in the same run: the next holder is read from the order
/// the refused move must have left as it was.
async fn holder_leaves_after_a_refusal(s: &dyn LakeStore) -> Held {
    let l = Lake::of(s);
    let field = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    let inv = vec![
        field("customer", "text"),
        field("amount", "decimal"),
        field("status", "text"),
    ];
    l.declare_json(json!({"declare": "type", "name": "rh_inv", "fields": inv}))
        .await?;
    for end in ["", "_twin"] {
        let fields = [
            field("customer", "text"),
            field("total", "decimal"),
            field("lo", "decimal"),
        ];
        l.declare_json(json!({"declare": "type", "name": format!("rh_bal{end}"), "key": ["customer"],
            "fields": fields,
            "view": {"source": "rh_inv", "where": "status = 'open'", "group_by": ["customer"],
                "measures": [{"name": "total", "expr": "sum(amount)"}, {"name": "lo", "expr": "min(amount)"}]}}))
            .await?;
    }
    l.set("view_kept.rh_bal", json!(true)).await?;
    for (k, c, a) in [
        ("a0", "c0", "60"),
        ("b1", "c1", "10"),
        ("a2", "c1", "70"),
        ("c3", "c1", "80"),
    ] {
        let fields = json!({"customer": c, "amount": a, "status": "open"});
        l.upsert("rh_inv", k, fields).await?;
    }
    let sets = [
        ("rh_pos", "rh_bal", "total > 100"),
        ("rh_pos3", "rh_bal", "total > 95"),
        ("rh_lowish", "rh_bal", "lo < 75"),
    ];
    for (name, of, filter) in sets {
        l.declare_json(set(name, of, filter)).await?;
    }
    let rule = json!({"on": "rh_inv", "field": "status", "from": "open", "to": "closed", "when": "amount < 75"});
    l.upsert(AGING_RULE_TYPE, "rh_close", rule).await?;
    let today = Timestamp::from_date(2026, 4, 1).expect("a date");
    // `a2` is refused at a ceiling of two, and `b1`, the holder of `c1`'s
    // minimum, is accepted after it: `c1`'s next minimum is `a2`'s, read
    // from the order the refused move put back.
    let aged = s.age(today).await.must("aging")?;
    let run = aged.rules.iter().find(|r| r.rule == "rh_close");
    ensure!(
        run.is_some_and(|r| r.moved == 2
            && r.failed_guards == 0
            && r.refused == 1
            && matches!(r.refusals.as_slice(),
                [m] if m.key.as_str() == "a2" && matches!(m.why, Refusal::TooLarge { max: 2 }))),
        "a ceiling of two refuses one move of the run, `a2`, counted as refused, and accepts the \
         others; it reported {aged:?}"
    );
    oracle(&l, s, &sets, "the holder left after a refused move").await
}

/// A reader of every field, `restricted` where asked, writing where
/// `writes`.
fn reader(s: &dyn LakeStore, id: &str, restricted: bool, writes: bool) -> lakelands_api::Principal {
    let default = if writes { Access::Write } else { Access::Read };
    let grants = Grants {
        default,
        restricted,
        ..Grants::default()
    };
    principal(s, PrincipalKind::Agent, id, grants)
}

/// What `who` is answered of `owing`: whether `describe` lists it, and
/// what a read of `in_set('owing')` over `balance` answers.
async fn answered(l: &Lake, who: &lakelands_api::Principal) -> Result<(bool, String), Failure> {
    let d = l
        .cat
        .describe(who, Default::default())
        .await
        .must("describe")?;
    let listed = d.data.object_sets.iter().any(|s| s.name == "owing");
    let read = l
        .cat
        .find(who, &"balance".into(), find_where("in_set('owing')"))
        .await;
    let read = match read {
        Ok(r) => format!("{:?}", r.data),
        Err(e) => format!("{e:?}"),
    };
    Ok((listed, read))
}

async fn a_set_of_a_kept_view_is_a_readers_only_where_nothing_of_a_source_is_held(
    store: Store,
) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    lake(&l).await?;
    l.upsert(
        "invoice",
        "i1",
        json!({"customer": "c1", "amount": "90", "status": "open"}),
    )
    .await?;
    // Holding `restricted`, an `only` on a source or a field a measure
    // reads kept from it holds the set from it alike.
    let held = |id: &str, grants: lakelands_api::principal::TypeGrants| {
        let mut who = reader(s, id, true, false);
        who.grants.types.insert("invoice".into(), grants);
        who
    };
    let only = held(
        "only",
        lakelands_api::principal::TypeGrants {
            only: [("customer".into(), json!("c1"))].into(),
            ..Default::default()
        },
    );
    let kept = held(
        "kept",
        lakelands_api::principal::TypeGrants {
            fields: [("amount".into(), Access::None)].into(),
            ..Default::default()
        },
    );
    let plain = reader(s, "plain", false, false);
    let undeclared = |r: &str| r.contains("SetUndeclared") && r.contains("owing");
    let mut was = Vec::new();
    for who in [&plain, &only, &kept] {
        let (listed, read) = answered(&l, who).await?;
        ensure!(
            !listed && undeclared(&read),
            "to `{}` `owing` is not described and `in_set('owing')` is `set_undeclared`; it was answered {listed} and {read}",
            who.id
        );
        was.push(read);
    }
    // A held invoice takes `c1` into `owing` on the whole face: those
    // readers are answered as before.
    l.upsert(
        "invoice",
        "i9",
        json!({"customer": "c1", "amount": "40", "status": "open"}),
    )
    .await?;
    restrict(s, "invoice", "i9", "a hold").await?;
    for (who, was) in [&plain, &only, &kept].into_iter().zip(was) {
        let (_, read) = answered(&l, who).await?;
        ensure!(
            read == was,
            "a held invoice changes nothing `{}` is answered of `owing`: {was} before, {read} after",
            who.id
        );
    }
    let full = reader(s, "full", true, false);
    let (listed, read) = answered(&l, &full).await?;
    ensure!(
        listed && read.contains("\"c1\""),
        "a reader holding `restricted` and every field is described `owing` and reads `c1` in it; it was answered {listed} and {read}"
    );
    // A writer is told the set moved only where it is answered the group.
    // A writer holding `only` on a type no view reads is told as well.
    l.declare_json(json!({"declare": "type", "name": "memo", "fields": [{"name": "by", "type": {"kind": "text"}}]}))
        .await?;
    let mut w_only = reader(s, "w_only", true, true);
    let memo = lakelands_api::principal::TypeGrants {
        only: [("by".into(), json!("w_only"))].into(),
        ..Default::default()
    };
    w_only.grants.types.insert("memo".into(), memo);
    for (who, told) in [
        (reader(s, "w_plain", false, true), false),
        (reader(s, "w_full", true, true), true),
        (w_only, true),
    ] {
        let open = json!({"customer": "c2", "amount": "150", "status": "open"});
        let (ty, key) = (
            TypeName::from("invoice"),
            lakelands_api::RecordKey::from(who.id.as_str()),
        );
        let up = l
            .cat
            .upsert(&who, &ty, &key, patch(open), WriteOpts::default());
        let up = up.await.must("an invoice of c2")?;
        let named = matches!(&up, Outcome::Applied { set_off, .. } if set_off.joined.iter().any(|s| s == "owing"));
        ensure!(
            named == told,
            "`{}` is told `owing` moved only where it holds `restricted`; it was answered {up:?}",
            who.id
        );
        l.retire("invoice", &who.id).await?;
    }
    Ok(())
}

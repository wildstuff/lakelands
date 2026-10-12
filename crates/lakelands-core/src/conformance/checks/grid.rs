//! The rights grid (lakelands#962): a reader is answered alike by two twin
//! lakes that differ only in what it is not shown, at every door, to the
//! byte (`docs/decisions/006-grants-not-roles.md`,
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! `docs/decisions/051-a-grant-may-admit-only-some-records.md`).
//!
//! A cell is a feature, a reader and a door. Each twin pair is one script
//! run twice, so the two logs hold the same events in the same order. In
//! a pair of values the part it hides is written otherwise in the second
//! lake, each record its own value, ordered one way in the first and the
//! other way in the second. In a pair of existence the second lake holds
//! the hidden records under the vault, a type no reader is shown, so that
//! they are not there at all. The owner, who is shown everything, is the
//! control: at a door that shows the hidden part its two answers must
//! differ, or the twin hides nothing there. A red cell names its feature,
//! reader, door and record, and the first place the two answers part. A
//! cell red for an open issue is listed in [`KNOWN`] with that issue, and
//! must stay red until its fix removes it.

use std::collections::BTreeSet;

use lakelands_api::event::EventBody;
use lakelands_api::{Catalog, Grants, LakeId, Position, Principal, PrincipalKind, TypeName};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::conformance::backing;
use crate::conformance::fixture::{
    Lake, Must, doors, log_events, owner, patch, principal, restrict,
};
use crate::conformance::{Failure, Given, Held, Store};
use Door::*;
use Hidden::*;

check!(
    GRID_PLAIN = the_rights_grid_holds_on_the_plain_lake,
    "006, 024 and 051 (lakelands#962): on twin lakes that differ only in a held record, another's record, another's link, a link to a trip another's `only` keeps, a field kept from a reader or a type kept from a reviewer, either in its values (each record its own, ordered one way in one lake and the other way in the other) or in whether it is there at all (the second lake holding it under a type no reader is shown), each reader not shown that part (without `restricted`, `only` on a type, on a link type and on an end's type, masked, at `none` and `write_only` on a field, `review` and `schema` kept from a type) is answered alike to the byte by `get`, `get` as of a position, `find` with rows and total, a count and a group, a `where` and an order on the hidden field and its sum, min and max, a lexical and a vector search, a pattern's rows, graph, path, degree and components, `describe` and `describe_type`; the owner's answers differ at each of the 240 cells whose door shows the hidden part, so no twin is blind; twins whose logs part outside what they hide, in an event's kind, type, key or value, are a broken fixture; an instant the store stamps from its clock is compared by its rank among its answer's own, a fixture field bearing such a name refused, so a leak through an instant's absolute value is out of the grid's reach",
    given
);

/// Cells red for an open issue of 0.14.0, by the start of their name, and
/// the issue. A cell listed must be red; one that holds is removed by its
/// fix.
const KNOWN: &[(&str, u32)] = &[];

/// The cells whose owner's answers differ between the twins.
const CONTROLS: usize = 240;

/// The link type of `claim.on_trip`.
const LINK: &str = "sys.link.claim.on_trip";

/// The type the second lake of a pair of existence holds the hidden records
/// under, and the link types that reach it or hide a link; every reader
/// but the owner has each at `none`.
const VAULT: &str = "vault";
const VAULTED: [&str; 4] = [
    VAULT,
    "sys.link.vault.to_trip",
    "sys.link.claim.on_vault",
    "sys.link.claim.kept",
];

/// Every record's key, in the order its hidden values follow.
const KEYS: [&str; 11] = [
    "t1", "t2", "c1", "c2", "c3", "c9", "c1:t1", "c2:t1", "c3:t1", "c9:t1", "c1:t2",
];

/// The value twin `b` holds for `v` of the record `key` where it is hidden:
/// each record its own, in the order of [`KEYS`] in the first twin and the
/// other way in the second.
fn hidden_value(v: &Value, key: &str, b: bool) -> Value {
    let i = KEYS.iter().position(|k| *k == key).unwrap_or_default();
    let (up, down) = (i as u8, (KEYS.len() - 1 - i) as u8);
    match (v, b) {
        (Value::String(_), false) => json!(format!("alpha {}", (b'a' + up) as char)),
        (Value::String(_), true) => json!(format!("omega {}", (b'a' + down) as char)),
        (Value::Number(_), false) => json!(1000 + i),
        (Value::Number(_), true) => json!(2000 - i),
        (_, false) => json!([f64::from(up) / 10.0, 1.0]),
        (_, true) => json!([1.0, f64::from(up) / 10.0]),
    }
}

/// What a twin pair hides: the part its two lakes differ in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hidden {
    /// `c9`, held, and its link.
    Hold,
    /// Bob's `c2` and its link.
    Bobs,
    /// The link of bob's `c2`, by bob.
    BobsLink,
    /// `t2`, which an `only` on `trip` does not admit, and `c1`'s link to it.
    OtherTrip,
    /// Every claim's `secret`.
    Secret,
    /// Every trip and every link.
    TripKept,
}

const HIDDEN: [Hidden; 6] = [Hold, Bobs, BobsLink, OtherTrip, Secret, TripKept];

/// A record a door is asked of: its type, its key and the text field the
/// twins hold otherwise.
type Target = (&'static str, &'static str, &'static str);

impl Hidden {
    /// Whether the twins differ in `field` of the record `key` of `ty`; an
    /// owner's field never, since an `only` reads it.
    fn hides(self, ty: &str, key: &str, field: &str) -> bool {
        let part = match self {
            Hold => key.starts_with("c9"),
            Bobs => key.starts_with("c2"),
            BobsLink => key == "c2:t1",
            OtherTrip => key.ends_with("t2"),
            Secret => field == "secret",
            TripKept => ty != "claim",
        };
        part && !matches!(field, "owner" | "by")
    }

    /// The records each door is asked of.
    fn targets(self) -> &'static [Target] {
        match self {
            Hold => &[
                ("claim", "c9", "note"),
                ("claim", "c9", "amount"),
                (LINK, "c9:t1", "label"),
            ],
            Bobs => &[
                ("claim", "c2", "note"),
                ("claim", "c2", "amount"),
                (LINK, "c2:t1", "label"),
            ],
            BobsLink => &[(LINK, "c2:t1", "label")],
            OtherTrip => &[("trip", "t2", "name"), (LINK, "c1:t2", "label")],
            Secret => &[("claim", "c1", "secret")],
            TripKept => &[("trip", "t1", "name"), (LINK, "c1:t1", "label")],
        }
    }
}

/// Each reader, its grants, and the twins besides `Hold` it is compared
/// on: no reader but the owner holds `restricted`. The owner is the
/// control, on every twin.
const READERS: [(&str, &str, &[Hidden]); 9] = [
    ("owner", "", &[]),
    ("unrestricted", r#"{"default": "write"}"#, &[]),
    (
        "only_on_type",
        r#"{"default": "write", "types": {"claim": {"only": {"owner": "alice"}}}}"#,
        &[Bobs, BobsLink],
    ),
    (
        "only_link_type",
        r#"{"default": "write", "types": {"sys.link.claim.on_trip": {"only": {"by": "alice"}}}}"#,
        &[BobsLink],
    ),
    (
        "only_end_type",
        r#"{"default": "write", "types": {"trip": {"only": {"name": "rome"}}}}"#,
        &[OtherTrip],
    ),
    (
        "masked",
        r#"{"default": "write", "types": {"claim": {"fields": {"secret": "masked"}}}}"#,
        &[Secret],
    ),
    (
        "none",
        r#"{"default": "write", "types": {"claim": {"fields": {"secret": "none"}}}}"#,
        &[Secret],
    ),
    (
        "write_only",
        r#"{"default": "write", "types": {"claim": {"fields": {"secret": "write_only"}}}}"#,
        &[Secret],
    ),
    (
        "reviewer_partial",
        r#"{"default": "write", "review": true, "schema": true, "types": {"trip": {"default": "none"}}}"#,
        &[TripKept],
    ),
];

/// The doors of a cell.
#[derive(Clone, Copy, Debug)]
enum Door {
    Get,
    AsOf,
    Rows,
    Count,
    Group,
    Where,
    Order,
    Fold,
    Lexical,
    Vector,
    Matches,
    Graph,
    Path,
    Degree,
    Components,
    Describe,
    DescribeType,
}

const DOORS: [Door; 17] = [
    Get,
    AsOf,
    Rows,
    Count,
    Group,
    Where,
    Order,
    Fold,
    Lexical,
    Vector,
    Matches,
    Graph,
    Path,
    Degree,
    Components,
    Describe,
    DescribeType,
];

fn de<T: DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).expect("a door's arguments read")
}

/// An answer as the grid compares it.
fn said<T: Serialize>(answer: lakelands_api::Result<T>) -> Value {
    match answer {
        Ok(reply) => json!({ "ok": reply }),
        Err(e) => json!({ "err": e }),
    }
}

impl Door {
    /// Whether the door shows the part a pair hides, so that the owner's
    /// two answers must differ: a count and a measure show whether a record
    /// is there, not its values; a description shows neither.
    fn shows(self, gone: bool) -> bool {
        match self {
            Describe | DescribeType => false,
            Count | Degree | Components => gone,
            _ => true,
        }
    }

    /// `who`'s answer at this door about `target`, or `None` where the
    /// door asks nothing of it: a vector of a twin that hides none, a walk
    /// from a link, a door that reads no field about a number.
    async fn ask(
        self,
        cat: &dyn Catalog,
        who: &Principal,
        (ty, key, field): Target,
        mark: Position,
        hidden: Hidden,
    ) -> Option<Value> {
        let (t, link, num) = (TypeName::from(ty), ty == LINK, field == "amount");
        if num && !matches!(self, Group | Where | Order | Fold | Matches) {
            return None;
        }
        let count = json!([{"name": "n", "func": "count"}]);
        let walk = json!([{"relation": "on_trip", "walk": "both", "depth": {"min": 0, "max": 3}}]);
        let measure = |m: &str| {
            json!({"start": {"type": "claim"}, "project": {"as": m},
                                       "steps": [{"relation": "on_trip", "walk": "both"}]})
        };
        Some(match self {
            Get => said(
                cat.get(who, &t, de(json!({"key": key, "with_meta": true})))
                    .await,
            ),
            AsOf => {
                let get = json!({"key": key, "with_meta": true, "as_of": mark});
                said(cat.get(who, &t, de(get)).await)
            }
            Rows => said(cat.find(who, &t, de(json!({"total": true}))).await),
            Count => {
                let find = json!({"aggregate": {"measures": count}});
                said(cat.find(who, &t, de(find)).await)
            }
            Group => {
                let find = json!({"aggregate": {"measures": count, "group_by": [field]}});
                said(cat.find(who, &t, de(find)).await)
            }
            Where => {
                let cond = if num {
                    format!("{field} < 1500")
                } else {
                    format!("{field} < 'm'")
                };
                let find = json!({"where": cond, "total": true});
                said(cat.find(who, &t, de(find)).await)
            }
            Order => {
                let find = json!({"order_by": [{"field": field}]});
                said(cat.find(who, &t, de(find)).await)
            }
            Fold => {
                let of = |f: &str| json!({"name": f, "func": f, "field": field});
                let measures = if num {
                    json!([of("sum"), of("min"), of("max")])
                } else {
                    json!([of("min"), of("max")])
                };
                let find = json!({"aggregate": {"measures": measures}});
                said(cat.find(who, &t, de(find)).await)
            }
            Lexical => {
                let search =
                    json!({"type": ty, "mode": "lexical", "text": "alpha", "fields": [field]});
                said(cat.search(who, de(search)).await)
            }
            Vector if ty == "claim" && matches!(hidden, Hold | Bobs) => {
                let search =
                    json!({"type": ty, "mode": "vector", "field": "v", "vector": [0.0, 1.0]});
                said(cat.search(who, de(search)).await)
            }
            Matches => {
                let rows = json!(["x.key", format!("x.{field}")]);
                let p = json!({"start": {"type": ty, "as": "x"}, "project": {"as": "rows", "fields": rows}});
                said(cat.query(who, de(p)).await)
            }
            Graph if !link => {
                let p = json!({"start": {"type": ty, "key": key}, "steps": walk, "project": {"as": "graph"}});
                said(cat.query(who, de(p)).await)
            }
            Path if !link => {
                let p = json!({"start": {"type": "claim", "key": "c3"}, "steps": walk,
                               "to": [ty, key], "project": {"as": "graph"}});
                said(cat.query(who, de(p)).await)
            }
            Degree => said(cat.query(who, de(measure("degree"))).await),
            Components => said(cat.query(who, de(measure("components"))).await),
            Describe => said(cat.describe(who, Default::default()).await),
            DescribeType => said(cat.describe_type(who, &t, Default::default()).await),
            _ => return None,
        })
    }
}

/// Declares the plain lake and writes it as twin `b` of a pair: each field
/// `hidden` hides as [`hidden_value`] gives it, the first twin's values in
/// both where the pair is of existence (`gone`), whose second twin writes
/// each hidden record and link under the vault instead. `c9` is held last.
/// Answers the position a read as of one reads at, before a later write.
async fn plain(
    l: &Lake,
    store: &dyn lakelands_api::LakeStore,
    hidden: Hidden,
    gone: bool,
    b: bool,
) -> Result<Position, Failure> {
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}, "searchable": !matches!(n, "owner" | "by")});
    let rel = |name: &str, target: &str| {
        json!({"name": name, "target": target, "cardinality": "many_to_many",
               "fields": [text("by"), text("label")]})
    };
    let ty = |name: &str, fields: Value, relations: Value| json!({"declare": "type", "name": name, "fields": fields, "relations": relations});
    let fields = json!([text("owner"), {"name": "amount", "type": {"kind": "number"}}, text("note"),
                        text("secret"), {"name": "v", "type": {"kind": "vector", "dimensions": 2}}]);
    let mut vault = fields.clone();
    if let Some(f) = vault.as_array_mut() {
        f.push(text("name"));
    }
    l.declare_json(ty("trip", json!([text("name")]), json!([])))
        .await?;
    l.declare_json(ty(VAULT, vault, json!([rel("to_trip", "trip")])))
        .await?;
    let relations = json!([
        rel("on_trip", "trip"),
        rel("on_vault", VAULT),
        rel("kept", "trip")
    ]);
    l.declare_json(ty("claim", fields, relations)).await?;
    let vaulted = |ty: &str, key: &str| gone && b && hidden.hides(ty, key, "");
    let in_vault = |ty: &'static str, key: &str| if vaulted(ty, key) { VAULT } else { ty };
    let put = |ty: &str, key: &str, fields: Value| -> Result<Value, Failure> {
        let mut out = serde_json::Map::new();
        for (f, v) in fields.as_object().into_iter().flatten() {
            ensure!(
                !STAMPED.contains(&f.as_str()),
                "a broken fixture: its field `{f}` bears the name of an instant the store stamps"
            );
            let v = if hidden.hides(ty, key, f) {
                hidden_value(v, key, b && !gone)
            } else {
                v.clone()
            };
            out.insert(f.clone(), v);
        }
        Ok(Value::Object(out))
    };
    for (key, name) in [("t1", "rome"), ("t2", "oslo")] {
        let f = put("trip", key, json!({ "name": name }))?;
        l.upsert(in_vault("trip", key), key, f).await?;
    }
    for (key, owner, amount) in [
        ("c1", "alice", 10),
        ("c2", "bob", 20),
        ("c3", "alice", 5),
        ("c9", "alice", 7),
    ] {
        let f = json!({"owner": owner, "amount": amount, "note": "taxi", "secret": key, "v": [0.6, 0.8]});
        l.upsert(in_vault("claim", key), key, put("claim", key, f)?)
            .await?;
    }
    for (from, to) in [
        ("c1", "t1"),
        ("c2", "t1"),
        ("c3", "t1"),
        ("c9", "t1"),
        ("c1", "t2"),
    ] {
        let key = format!("{from}:{to}");
        let by = if from == "c2" { "bob" } else { "alice" };
        let fields = patch(put(LINK, &key, json!({"by": by, "label": "trip"}))?);
        let (ty, on) = match (
            vaulted("claim", from),
            vaulted("trip", to),
            vaulted(LINK, &key),
        ) {
            (true, _, _) => (VAULT, "to_trip"),
            (_, true, _) => ("claim", "on_vault"),
            (_, _, true) => ("claim", "kept"),
            _ => ("claim", "on_trip"),
        };
        let (ty, on, from, to) = (&ty.into(), &on.into(), &from.into(), &to.into());
        let linked = l
            .cat
            .link(&l.own, ty, from, on, to, fields, Default::default());
        linked.await.must("linking a claim to a trip")?;
    }
    let mark = store.head().await.must("reading the head")?;
    l.upsert("claim", "c3", json!({"note": "hotel"})).await?;
    restrict(store, in_vault("claim", "c9"), "c9", "a hold").await?;
    Ok(mark)
}

/// An event as the guard compares it: where it is about a record a pair
/// hides, each field the twins hold otherwise, and in a pair of existence
/// the type, put out of sight.
fn masked(body: &EventBody, hidden: Hidden, gone: bool) -> Value {
    fn walk(v: &mut Value, hidden: Hidden, gone: bool) {
        match v {
            Value::Object(m) => {
                if let (Some(Value::String(ty)), Some(Value::String(key))) =
                    (m.get("type").cloned(), m.get("key").cloned())
                {
                    if gone && hidden.hides(&ty, &key, "") {
                        m.insert("type".into(), json!("hidden"));
                    }
                    if let Some(Value::Object(p)) = m.get_mut("patch") {
                        (p.iter_mut())
                            .filter(|(f, _)| hidden.hides(&ty, &key, f))
                            .for_each(|(_, x)| *x = json!("hidden"));
                    }
                }
                m.values_mut().for_each(|x| walk(x, hidden, gone));
            }
            Value::Array(xs) => xs.iter_mut().for_each(|x| walk(x, hidden, gone)),
            _ => {}
        }
    }
    let mut v = serde_json::to_value(body).unwrap_or_default();
    walk(&mut v, hidden, gone);
    v
}

/// The twins of `hidden`, of existence where `gone`, each with the position
/// it is read as of: the first the check's own lake where it is handed one,
/// the second a twin; both one script. Refused as a broken fixture unless
/// their logs hold the same events in the same order, each of the same
/// type and key and alike outside the part the pair hides.
async fn twins(
    given: &Given,
    own: Option<Store>,
    lake: &LakeId,
    (hidden, gone): (Hidden, bool),
) -> Result<Vec<(Store, Position)>, Failure> {
    let mut out = Vec::new();
    let mut own = own;
    for b in [false, true] {
        let store = match own.take() {
            Some(store) => store,
            None => {
                let seams = lakelands_api::seam::Seams {
                    backings: backing::backings(),
                    executors: backing::executors(),
                    ..Default::default()
                };
                let twin = (given.twin)(lake.clone(), seams).await;
                twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?
            }
        };
        let l = Lake::of(store.as_ref());
        let mark = plain(&l, store.as_ref(), hidden, gone, b).await?;
        out.push((store, mark));
    }
    let mut logs = Vec::new();
    for (store, _) in &out {
        let events = log_events(store.as_ref()).await?;
        logs.push(Value::Array(
            events
                .iter()
                .map(|e| masked(&e.body, hidden, gone))
                .collect(),
        ));
    }
    let at = match (&logs[0], &logs[1]) {
        (Value::Array(x), Value::Array(y)) if x.len() != y.len() => {
            Some(format!("their lengths, {} and {} events", x.len(), y.len()))
        }
        (x, y) => parted(x, y, "event "),
    };
    ensure!(
        at.is_none() && out[0].1 == out[1].1,
        "a broken fixture: the twins of {hidden:?} (gone: {gone}) must hold the same events in the same order, alike outside the part they hide; they part at {}",
        at.unwrap_or_else(|| "the position they are read as of".into())
    );
    Ok(out)
}

/// The instants the store stamps from its clock, by their member's name.
const STAMPED: [&str; 3] = ["known_at", "seen_at", "proposed_at"];

/// The answer with each stamped instant replaced by its rank among the
/// answer's own: their order is kept, the clock dropped. Each twin's
/// clock is its own, so instants are ranked within one answer, never
/// across the two.
fn ranked(v: &mut Value) {
    fn each(v: &mut Value, f: &mut dyn FnMut(&mut Value)) {
        match v {
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    if STAMPED.contains(&k.as_str()) && x.is_string() {
                        f(x);
                    } else {
                        each(x, f);
                    }
                }
            }
            Value::Array(xs) => xs.iter_mut().for_each(|x| each(x, f)),
            _ => {}
        }
    }
    let mut seen = BTreeSet::new();
    each(v, &mut |x| {
        seen.insert(x.as_str().unwrap_or_default().to_owned());
    });
    let rank: Vec<String> = seen.into_iter().collect();
    each(v, &mut |x| {
        let at = rank.binary_search(&x.as_str().unwrap_or_default().to_owned());
        *x = json!(at.unwrap_or_default());
    });
}

/// The first place two answers part, and what each holds there.
fn parted(x: &Value, y: &Value, at: &str) -> Option<String> {
    match (x, y) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            keys.into_iter().find_map(|k| {
                let null = Value::Null;
                parted(
                    a.get(k).unwrap_or(&null),
                    b.get(k).unwrap_or(&null),
                    &format!("{at}.{k}"),
                )
            })
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => (a.iter().zip(b))
            .enumerate()
            .find_map(|(i, (p, q))| parted(p, q, &format!("{at}[{i}]"))),
        _ => (x != y).then(|| format!("{at}: {x} / {y}")),
    }
}

/// The reader `name` of the lake `store` holds, with `grants` and the vault
/// at `none`; the owner where they are empty.
fn reader(store: &dyn lakelands_api::LakeStore, name: &str, grants: &str) -> Principal {
    if grants.is_empty() {
        return owner(store);
    }
    let mut grants: Value = serde_json::from_str(grants).expect("a reader's grants read");
    for ty in VAULTED {
        grants["types"][ty] = json!({"default": "none"});
    }
    let grants: Grants = serde_json::from_value(grants).expect("a reader's grants read");
    principal(store, PrincipalKind::Agent, name, grants)
}

async fn the_rights_grid_holds_on_the_plain_lake(store: Store, given: Given) -> Held {
    let lake = store.lake().clone();
    let mut own = Some(store);
    let (mut red, mut blind, mut alike, mut controls, mut unasked) =
        (Vec::new(), Vec::new(), 0, 0, 0);
    let pairs = (HIDDEN.iter()).flat_map(|&h| [(h, false), (h, true)]);
    for (hidden, gone) in pairs.filter(|&(h, gone)| !(gone && h == Secret)) {
        let pair = twins(&given, own.take(), &lake, (hidden, gone)).await?;
        let (a, b, mark) = (
            doors(pair[0].0.as_ref()),
            doors(pair[1].0.as_ref()),
            pair[0].1,
        );
        for (name, grants, not_shown) in READERS {
            let control = grants.is_empty();
            if !control && hidden != Hold && !not_shown.contains(&hidden) {
                continue;
            }
            let who = reader(pair[0].0.as_ref(), name, grants);
            for &target in hidden.targets() {
                for door in DOORS {
                    let Some(mut x) = door.ask(a.as_ref(), &who, target, mark, hidden).await else {
                        unasked += 1;
                        continue;
                    };
                    let y = door.ask(b.as_ref(), &who, target, mark, hidden).await;
                    let mut y = y.unwrap_or_default();
                    ranked(&mut x);
                    ranked(&mut y);
                    let (ty, key, field) = target;
                    let pair = if gone { "gone" } else { "values" };
                    let cell =
                        format!("plain/{name}/{door:?} on {ty} {key}.{field} ({hidden:?}, {pair})");
                    match (control, x == y) {
                        (true, true) if door.shows(gone) => blind.push(cell),
                        (true, true) => {}
                        (true, false) => controls += 1,
                        (false, true) => alike += 1,
                        (false, false) => red.push((cell, parted(&x, &y, "").unwrap_or_default())),
                    }
                }
            }
        }
        for (store, _) in pair {
            store.close().await.must("closing a twin")?;
        }
    }
    let known = |cell: &str| KNOWN.iter().find(|(start, _)| cell.starts_with(start));
    let held: Vec<_> = (KNOWN.iter())
        .filter(|(start, _)| !red.iter().any(|(cell, _)| cell.starts_with(start)))
        .map(|(start, issue)| format!("{start} (lakelands#{issue})"))
        .collect();
    let new: Vec<_> = (red.iter())
        .filter(|(cell, _)| known(cell).is_none())
        .map(|(cell, at)| format!("{cell}: {at}"))
        .collect();
    ensure!(
        new.is_empty() && held.is_empty() && blind.is_empty() && controls == CONTROLS,
        "the rights grid on the plain lake: {alike} cells alike, {} red, {controls} controls \
         differ (the description says {CONTROLS}), {unasked} asked nothing; red and not known:\n{}\n\
         known and holding now, to be removed:\n{}\ncontrols alike, a twin that hides nothing there:\n{}",
        red.len(),
        new.join("\n"),
        held.join("\n"),
        blind.join("\n")
    );
    Ok(())
}

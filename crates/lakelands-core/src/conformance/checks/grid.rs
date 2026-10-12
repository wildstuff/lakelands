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
//! the hidden records and the links that reach them under the vault, each
//! one record of a type no reader is shown, so that they are not there at
//! all. The owner, who is shown everything, is the
//! control: at a door that shows the hidden part its two answers must
//! differ, or the twin hides nothing there. A red cell names its feature,
//! reader, door and record, and the first place the two answers part. A
//! cell red for an open issue is listed in [`KNOWN`] with that issue, and
//! must stay red until its fix removes it.
//!
//! The features stand on one lake beside the plain claims, trips and
//! links: a set of a set over each hidden field, a relation matched on
//! values, and one under a condition over the hidden field, a relation
//! whose target is a link type, and a view over a
//! valid-timed type, read at a valid time. Each asks the doors that read
//! it, of the records the hidden part reaches through it. The write doors
//! come after every read, each of every reader in turn: a reference to the
//! hidden record, written or computed, and a verb that reads across a
//! reference into it are among them, and the accept and reject of the
//! owner's proposal. A report is compared without the counts it keeps
//! whole.

use std::collections::BTreeSet;

use lakelands_api::event::EventBody;
use lakelands_api::write::{ProposedWrite, WriteOpts};
use lakelands_api::{
    Catalog, Grants, LakeId, Outcome, Position, Principal, PrincipalKind, ProposalId, RecordKey,
    TypeName,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::conformance::backing;
use crate::conformance::fixture::{
    self, Lake, Must, doors, log_events, owner, patch, principal, restrict, source_record,
};
use crate::conformance::{Failure, Given, Held, Store};
use Door::*;
use Feature::*;
use Hidden::*;

check!(
    GRID = the_rights_grid_holds_over_every_feature,
    "006, 024 and 051 (lakelands#962): on twin lakes that differ only in a held record, another's record, another's link, a link to a trip another's `only` keeps, a field kept from a reader or a type kept from a reviewer, either in its values (each record its own, ordered one way in one lake and the other way in the other) or in whether it is there at all (the second lake holding it, and each link that reaches it, as one record of a type no reader is shown), each reader not shown that part, every one of them holding `review` and `ingest` so that those doors reach the part (without `restricted`, `only` on a type, on a link type and on an end's type, masked, at `none` and `write_only` on a field, `review` and `schema` kept from a type) is answered alike to the byte by `get`, `get` as of a position, `find` with rows and total, a count and a group, a `where` and an order on the hidden field and its sum, min and max, a lexical and a vector search, a pattern's rows, graph, path, degree and components, `describe` and `describe_type`, the proposals listed, unfiltered, and the preview of a migration that makes the hidden field a number, sent as its author is shown the type, and of the value type of the hidden texts tightened, and through each feature that reaches the part: a set of a set over the hidden field, read by `find` and a count; a relation matched on owners, and one under a condition over the hidden field, each walked and measured by degree and components; a relation whose target is the link type, its links read at every door; and a view over valid-timed trips, read at a valid time by `find`, a count, a group, a `where` and an order, as is a trip whose valid time alone the twins hold otherwise, to a reader that reads it masked; after every read each reader writes, the owner last: the hidden field again, a link to the hidden record and from it, its unlinking and a link ingested, a verb's call and one whose `requires` reads the hidden field across a ticket's reference, a reference to it written and one computed from its key, a proposed retirement, an accept and a reject of the owner's proposal to write the hidden record again, and a retirement, each answered alike; the owner's answers differ at each of the 461 cells whose door shows the hidden part, so no twin is blind; twins whose logs part outside what they hide, in an event's kind, type, key or value, are a broken fixture, a set's memberships aside, which follow from the records; an instant the store stamps from its clock is compared by its rank among its answer's own, a fixture field bearing such a name refused, and the head an answer was read at, which those memberships move, is left out, with a proposal's id, which is a position, and how far a write's consequences reach, which a write tells on purpose (006: that something reacted), as an upsert in a pair of existence is not asked, a create on a free key, which a write tells on purpose too (006: what a holder's write tells), and a report's counts are left out, which 006 keeps whole on purpose, with what follows from them (`lossless`, `fits`, `more` and the report's id); the feed, the log as the host reads it, answers no principal and is no door of the grid; so a leak through an instant's absolute value, the head, a write's reach, a free key or a report's count is out of the grid's reach",
    given
);

/// Cells red for an open issue of 0.14.0, by the start of their name, and
/// the issue. A cell listed must be red; one that holds is removed by its
/// fix.
const KNOWN: &[(&str, u32)] = &[
    ("Plain/only_on_type/Proposals", 1016),
    ("Plain/only_on_type/Accept", 1016),
    ("Plain/only_on_type/Reject", 1016),
    ("Plain/only_end_type/Proposals", 1016),
    ("Plain/only_end_type/Accept", 1016),
    ("Plain/only_end_type/Reject", 1016),
];

/// The cells whose owner's answers differ between the twins.
const CONTROLS: usize = 461;

/// The link type of `claim.on_trip`.
const LINK: &str = "sys.link.claim.on_trip";

/// The link type of `memo.about`, whose target is [`LINK`].
const MEMO_LINK: &str = "sys.link.memo.about";

/// The view over the trips, and the valid time it is read at, after
/// every trip's `since`.
const VIEW: &str = "trip_names";
const SINCE: &str = "2020-01-01T00:00:00Z";
const VALID_AT: &str = "2021-01-01T00:00:00Z";

/// The `since` the second twin of [`Since`] holds for `t2`, after the
/// valid time.
const LATER: &str = "2022-01-01T00:00:00Z";

/// The type the second lake of a pair of existence holds the hidden records
/// and links under, each one record of it; every reader but the owner has
/// it at `none`.
const VAULT: &str = "vault";

/// Every record's key, in the order its hidden values follow.
const KEYS: [&str; 14] = [
    "t1",
    "t2",
    "c1",
    "c2",
    "c3",
    "c9",
    "c1:t1",
    "c2:t1",
    "c3:t1",
    "c9:t1",
    "c1:t2",
    "m1:c1%3At1",
    "m2:c2%3At1",
    "m9:c9%3At1",
];

/// The value twin `b` holds for `v` of the record `key` where it is hidden:
/// each record its own, in the order of [`KEYS`] in the first twin and the
/// other way in the second.
fn hidden_value(v: &Value, key: &str, b: bool) -> Value {
    let i = KEYS.iter().position(|k| *k == key).unwrap_or_default();
    let (up, down) = (i as u8, (KEYS.len() - 1 - i) as u8);
    match (v, b) {
        (Value::String(_), false) => json!(format!("alpha {}", (b'a' + up) as char)),
        (Value::String(_), true) => json!(format!("sigma {}", (b'a' + down) as char)),
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
    /// `t2`'s `since`: before the valid time in the first twin, after it in
    /// the second.
    Since,
}

const HIDDEN: [Hidden; 7] = [Hold, Bobs, BobsLink, OtherTrip, Secret, TripKept, Since];

/// What a target's record is of: the plain lake, or a feature atop it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Feature {
    /// The claims, trips and links alone.
    Plain,
    /// A set whose `where` names a set over the hidden field.
    SetOfSet,
    /// A relation matched on values: the claims of one owner.
    DerivedRelation,
    /// A relation matched on values under a condition over the hidden
    /// field: the claims of one owner whose `secret` sorts after.
    ConditionalEdge,
    /// A relation whose target is a link type: a memo on a claim's link.
    LinkOfLink,
    /// A view over the valid-timed trips, read at a valid time.
    ViewValid,
}

impl Feature {
    /// Whether `door` reads the feature, about a number where `num`: a
    /// write asks the plain lake, of a number only across a reference.
    fn asks(self, door: Door, num: bool) -> bool {
        if WRITES.contains(&door) {
            return self == Plain && (!num || door == Across);
        }
        match self {
            Plain => !num || matches!(door, Group | Where | Order | Fold | Matches),
            SetOfSet => matches!(door, Rows | Count),
            DerivedRelation | ConditionalEdge => matches!(door, Graph | Degree | Components),
            LinkOfLink => true,
            ViewValid => matches!(door, Get | Rows | Count | Group | Where | Order),
        }
    }

    /// The type a measure starts at and the relation a walk takes.
    fn walk(self) -> (&'static str, &'static str) {
        match self {
            DerivedRelation => ("claim", "peer"),
            ConditionalEdge => ("claim", "alike"),
            LinkOfLink => ("memo", "about"),
            _ => ("claim", "on_trip"),
        }
    }
}

/// A record a door is asked of: the feature it is of, its type, its key
/// and the text field the twins hold otherwise.
type Target = (Feature, &'static str, &'static str, &'static str);

impl Hidden {
    /// Whether the twins differ in `field` of the record `key` of `ty`; an
    /// owner's field never, since an `only` reads it, nor a trip's valid
    /// time but in [`Since`].
    fn hides(self, ty: &str, key: &str, field: &str) -> bool {
        // A link's key holds its ends', a memo's link's the link's too,
        // its `:` written `%3A` inside the component.
        let flat = key.replace("%3A", ":");
        let names = |k: &str| flat.split(':').any(|x| x == k);
        let part = match self {
            Hold => names("c9"),
            Bobs => names("c2"),
            BobsLink => flat.contains("c2:t1"),
            OtherTrip => names("t2"),
            Secret => field == "secret",
            TripKept => ty == "trip" || ty == VAULT || ty.starts_with("sys.link."),
            Since => ty == "trip" && key == "t2",
        };
        part && (field == "since") == (self == Since) && !matches!(field, "owner" | "by" | "claim")
    }

    /// The records each door is asked of.
    fn targets(self) -> &'static [Target] {
        match self {
            Hold => &[
                (Plain, "claim", "c9", "note"),
                (Plain, "claim", "c9", "amount"),
                (Plain, LINK, "c9:t1", "label"),
                (SetOfSet, "claim", "c9", "note"),
                (SetOfSet, "claim", "c9", "amount"),
                (DerivedRelation, "claim", "c9", "note"),
                (ConditionalEdge, "claim", "c9", "note"),
                (LinkOfLink, MEMO_LINK, "m9:c9%3At1", "label"),
            ],
            Bobs => &[
                (Plain, "claim", "c2", "note"),
                (Plain, "claim", "c2", "amount"),
                (Plain, LINK, "c2:t1", "label"),
                (SetOfSet, "claim", "c2", "note"),
                (SetOfSet, "claim", "c2", "amount"),
                (DerivedRelation, "claim", "c2", "note"),
                (ConditionalEdge, "claim", "c2", "note"),
                (LinkOfLink, MEMO_LINK, "m2:c2%3At1", "label"),
            ],
            BobsLink => &[
                (Plain, LINK, "c2:t1", "label"),
                (LinkOfLink, MEMO_LINK, "m2:c2%3At1", "label"),
            ],
            OtherTrip => &[
                (Plain, "trip", "t2", "name"),
                (Plain, LINK, "c1:t2", "label"),
                (ViewValid, VIEW, "", "name"),
            ],
            Secret => &[
                (Plain, "claim", "c1", "secret"),
                (SetOfSet, "claim", "c1", "secret"),
                (DerivedRelation, "claim", "c1", "secret"),
                (ConditionalEdge, "claim", "c1", "secret"),
            ],
            TripKept => &[
                (Plain, "trip", "t1", "name"),
                (Plain, LINK, "c1:t1", "label"),
                (LinkOfLink, MEMO_LINK, "m1:c1%3At1", "label"),
                (ViewValid, VIEW, "", "name"),
            ],
            Since => &[
                (ViewValid, VIEW, "", "name"),
                (ViewValid, "trip", "t2", "name"),
            ],
        }
    }
}

/// Each reader, its grants, and the twins besides `Hold` it is compared
/// on: no reader but the owner holds `restricted`. The owner is the
/// control, on every twin.
const READERS: [(&str, &str, &[Hidden]); 10] = [
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
    (
        "since_masked",
        r#"{"default": "write", "types": {"trip": {"fields": {"since": "masked"}}}}"#,
        &[Since],
    ),
];

/// The doors of a cell.
#[derive(Clone, Copy, Debug, PartialEq)]
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
    Proposals,
    Migrate,
    Retype,
    Upsert,
    Link,
    Unlink,
    Ingest,
    Call,
    Across,
    Named,
    Computed,
    Propose,
    Accept,
    Reject,
    Retire,
}

/// The write doors, asked after every read on both twins in step, each of
/// every reader before the next, the owner last: in a pair of existence
/// its writes apply in one twin alone.
const WRITES: [Door; 12] = [
    Across, Upsert, Link, Unlink, Ingest, Call, Named, Computed, Propose, Accept, Reject, Retire,
];

const DOORS: [Door; 20] = [
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
    Proposals,
    Migrate,
    Retype,
];

/// Where a pair is read: the position a read as of one reads at, and the
/// proposal the owner made of each record a pair hides, by its key.
type At = (Position, Vec<(&'static str, ProposalId)>);

/// The members of a migration's report that count every record, whether
/// its reader is answered it or not, and those that follow from them: 006
/// keeps a report's counts whole on purpose.
const WHOLE: [&str; 9] = [
    "records_scanned",
    "convertible",
    "not_convertible",
    "values_scanned",
    "misfits",
    "fits",
    "lossless",
    "more",
    "report_id",
];

/// The answer with each member [`WHOLE`] names left out.
fn trimmed(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|k, _| !WHOLE.contains(&k.as_str()));
            m.values_mut().for_each(trimmed);
        }
        Value::Array(xs) => xs.iter_mut().for_each(trimmed),
        _ => {}
    }
}

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
    /// two answers must differ: a count, a measure and a write show whether
    /// a record is there, not its values, but for a condition read across
    /// a reference; a description shows neither.
    fn shows(self, gone: bool) -> bool {
        match self {
            Describe | DescribeType | Upsert | Reject => false,
            Migrate => gone,
            Across => true,
            Count | Degree | Components => gone,
            _ => gone || !WRITES.contains(&self),
        }
    }

    /// `who`'s answer at this door about `target`, or `None` where the
    /// door asks nothing of it: a door that does not read the target's
    /// feature, a vector of a twin that hides none, a walk from a link.
    async fn ask(
        self,
        cat: &dyn Catalog,
        who: &Principal,
        (feature, ty, key, field): Target,
        (mark, ids): &At,
        (hidden, gone): (Hidden, bool),
    ) -> Option<Value> {
        let (t, num) = (TypeName::from(ty), field == "amount");
        let link = ty.starts_with("sys.link.");
        let (opts, k) = (WriteOpts::default(), RecordKey::from(key));
        // A record a reader writes is its own, keyed by it and the target.
        let alnum = |x: &str| x.replace(|c: char| !c.is_ascii_alphanumeric(), "");
        let fresh = RecordKey::from(format!("{}-{}", alnum(&who.id), alnum(key)));
        let ends = key.split_once(':').unwrap_or_default();
        let (from, to) = match ty {
            "trip" => ("c3", key),
            _ => (key, "t1"),
        };
        let by = || patch(json!({"by": "alice", "label": "w"}));
        if !feature.asks(self, num) {
            return None;
        }
        // A set of a set is read through its name, a view and its source
        // at a valid time.
        let at = |mut find: Value| {
            if feature == SetOfSet {
                find["where"] = json!(format!("in_set('outer_{field}')"));
            }
            if feature == ViewValid {
                find["valid_at"] = json!(VALID_AT);
            }
            find
        };
        let count = json!([{"name": "n", "func": "count"}]);
        let (start, on) = feature.walk();
        let walk = json!([{"relation": on, "walk": "both", "depth": {"min": 0, "max": 3}}]);
        let measure = |m: &str| {
            json!({"start": {"type": start}, "project": {"as": m},
                   "steps": [{"relation": on, "walk": "both"}]})
        };
        Some(match self {
            Get if !key.is_empty() => {
                let get = json!({"key": key, "with_meta": true});
                said(cat.get(who, &t, de(at(get))).await)
            }
            AsOf => {
                let get = json!({"key": key, "with_meta": true, "as_of": mark});
                said(cat.get(who, &t, de(get)).await)
            }
            Rows => said(cat.find(who, &t, de(at(json!({"total": true})))).await),
            Count => {
                let find = json!({"aggregate": {"measures": count}});
                said(cat.find(who, &t, de(at(find))).await)
            }
            Group => {
                let find = json!({"aggregate": {"measures": count, "group_by": [field]}});
                said(cat.find(who, &t, de(at(find))).await)
            }
            Where => {
                let cond = if num {
                    format!("{field} < 1500")
                } else {
                    format!("{field} < 'p'")
                };
                let find = json!({"where": cond, "total": true});
                said(cat.find(who, &t, de(at(find))).await)
            }
            Order => {
                let find = json!({"order_by": [{"field": field}]});
                said(cat.find(who, &t, de(at(find))).await)
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
                let p = json!({"start": {"type": start, "key": "c3"}, "steps": walk,
                               "to": [ty, key], "project": {"as": "graph"}});
                said(cat.query(who, de(p)).await)
            }
            Degree => said(cat.query(who, de(measure("degree"))).await),
            Components => said(cat.query(who, de(measure("components"))).await),
            Describe => said(cat.describe(who, Default::default()).await),
            DescribeType => said(cat.describe_type(who, &t, Default::default()).await),
            // The listing unfiltered: an entry whose write is withheld has
            // no type a filter could keep (lakelands#1008).
            Proposals if !link => said(cat.proposals(who, Default::default()).await),
            // The preview of the hidden field made a number, and of the
            // value type of the hidden texts tightened.
            Migrate if !link => {
                let mut to = declarations().into_iter().find(|d| d["name"] == ty)?;
                to.as_object_mut()?.remove("declare");
                let fields = to["fields"].as_array_mut()?.iter_mut();
                (fields.filter(|f| f["name"] == field))
                    .for_each(|f| f["type"] = json!({"kind": "number"}));
                // As its author is shown the type: a relation to a type it
                // is not shown, the store completes.
                let mut shown = Vec::new();
                for r in to["relations"].as_array()?.clone() {
                    let target = TypeName::from(r["target"].as_str()?);
                    let seen = cat.describe_type(who, &target, Default::default()).await;
                    shown.extend(seen.is_ok().then_some(r));
                }
                to["relations"] = json!(shown);
                said(cat.migrate(who, de(json!({ "to": to }))).await)
            }
            Retype if matches!(field, "note" | "secret") => {
                let to = json!({"name": "word", "kind": "text", "constraint": "value > 'b'"});
                said(cat.migrate_value_type(who, de(json!({ "to": to }))).await)
            }
            // In a pair of existence an upsert creates on a free key, which
            // a write tells on purpose (006).
            Upsert if !gone && !link => {
                let p = patch(json!({ field: "w" }));
                said(cat.upsert(who, &t, &k, p, opts).await)
            }
            Link if link => {
                let on = (&"memo".into(), &"m1".into(), &"about".into());
                said(cat.link(who, on.0, on.1, on.2, &k, by(), opts).await)
            }
            Link => {
                let (ty, on) = (&"claim".into(), &"on_trip".into());
                said(
                    cat.link(who, ty, &from.into(), on, &to.into(), by(), opts)
                        .await,
                )
            }
            Unlink if link => {
                let (ty, on) = (&"claim".into(), &"on_trip".into());
                let (from, to) = (&ends.0.into(), &ends.1.into());
                said(cat.unlink(who, ty, from, on, to, opts).await)
            }
            Ingest if !link => {
                let lt = "sys.link.claim.seen_on";
                let r = source_record(lt, &format!("{from}:{to}"), json!({"from": from, "to": to}));
                said(fixture::ingest(cat, who, "grid", vec![r]).await)
            }
            Call if !link => {
                let verb = format!("touch_{ty}").into();
                said(cat.call(who, &verb, Some(&k), patch(json!({})), opts).await)
            }
            // A ticket's verb reads the claim's field across its reference.
            Across if ty == "claim" && !gone => {
                let verb = format!("check_{field}").into();
                let ticket = RecordKey::from(format!("k:{key}"));
                let call = cat.call(who, &verb, Some(&ticket), patch(json!({})), opts);
                said(call.await)
            }
            // A reference to the target, written or computed from its key.
            Named | Computed if !link => {
                let (on, sent) = match self {
                    Named => ("named", ty.to_owned()),
                    _ => ("computed", format!("{ty}_key")),
                };
                let p = patch(json!({ sent: key }));
                said(cat.upsert(who, &on.into(), &fresh, p, opts).await)
            }
            Propose if !link => {
                let write = ProposedWrite::Retire {
                    ty: t,
                    key: k,
                    opts,
                };
                said(cat.propose(who, write, "the grid".into()).await)
            }
            Accept | Reject if !link => {
                let id = &ids.iter().find(|(k, _)| *k == key)?.1;
                match self {
                    Accept => said(cat.accept(who, id).await),
                    _ => said(cat.reject(who, id, "the grid".into()).await),
                }
            }
            Retire if !link => said(cat.retire(who, &t, &k, opts).await),
            _ => return None,
        })
    }
}

/// The declarations of the lake: the claims, trips and links, the vault,
/// and the features beside them.
fn declarations() -> Vec<Value> {
    let text = |n: &str| json!({"name": n, "type": {"kind": "text"}, "searchable": !matches!(n, "owner" | "by")});
    let date = |n: &str| json!({"name": n, "type": {"kind": "date_time"}});
    let rel = |name: &str, target: &str| {
        json!({"name": name, "target": target, "cardinality": "many_to_many",
               "fields": [text("by"), text("label")]})
    };
    let ty = |name: &str, fields: Value, relations: Value| json!({"declare": "type", "name": name, "fields": fields, "relations": relations});
    // The hidden texts are of one value type, which a report migrates.
    let word = |n: &str| {
        let mut f = text(n);
        f["type"]["value_type"] = json!("word");
        f
    };
    let fields = json!([text("owner"), {"name": "amount", "type": {"kind": "number"}}, word("note"),
                        word("secret"), {"name": "v", "type": {"kind": "vector", "dimensions": 2}}]);
    let mut vault = fields.clone();
    if let Some(f) = vault.as_array_mut() {
        f.extend([
            text("name"),
            date("since"),
            text("from"),
            text("to"),
            text("by"),
            text("label"),
            text("claim"),
        ]);
    }
    // A reference to a claim or a trip, written, or computed from a key.
    let to = |name: &str, target: &str| json!({"name": name, "type": {"kind": "reference", "target": target}});
    let computed = |target: &str| {
        let mut r = to(target, target);
        (r["authority"], r["derived"]) = (json!("derived"), json!(format!("{target}_key")));
        r
    };
    let verb = |name: &str, on: &str, requires: Option<String>, set: (&str, &str)| {
        let mut v = json!({"declare": "verb", "name": name, "on": on, "risk": "low", "effect": "write",
                           "set": [{"field": set.0, "value": set.1}]});
        if let Some(r) = requires {
            v["requires"] = json!(r);
        }
        v
    };
    let mut trip = ty("trip", json!([text("name"), date("since")]), json!([]));
    trip["valid_from"] = json!("since");
    // Claims of one owner are peers: a relation matched on values.
    let peer = json!({"name": "peer", "target": "claim", "cardinality": "many_to_many",
                      "derived": {"match": [{"from": "owner", "to": "owner"}]}});
    // And alike where the condition reads the hidden field.
    let alike = json!({"name": "alike", "target": "claim", "cardinality": "many_to_many",
                       "derived": {"match": [{"from": "owner", "to": "owner"}],
                                   "where": "from.secret < to.secret"}});
    let view = json!({"declare": "type", "name": VIEW, "key": ["name"],
        "fields": [{"name": "name", "type": {"kind": "text"}}, {"name": "n", "type": {"kind": "decimal"}}],
        "view": {"source": "trip", "group_by": ["name"], "measures": [{"name": "n", "expr": "count()"}]}});
    vec![
        json!({"declare": "value_type", "name": "word", "kind": "text"}),
        trip,
        ty(VAULT, vault, json!([])),
        ty(
            "claim",
            fields,
            json!([rel("on_trip", "trip"), peer, alike,
                   {"name": "seen_on", "target": "trip", "cardinality": "many_to_many", "authority": "source"}]),
        ),
        ty("memo", json!([text("note")]), json!([rel("about", LINK)])),
        view,
        ty(
            "ticket",
            json!([to("claim", "claim"), text("ok")]),
            json!([]),
        ),
        ty(
            "named",
            json!([to("claim", "claim"), to("trip", "trip")]),
            json!([]),
        ),
        ty(
            "computed",
            json!([
                text("claim_key"),
                text("trip_key"),
                computed("claim"),
                computed("trip")
            ]),
            json!([]),
        ),
        verb("touch_claim", "claim", None, ("note", "'touched'")),
        verb("touch_trip", "trip", None, ("name", "'touched'")),
        verb(
            "check_note",
            "ticket",
            Some("claim.note < 'p'".into()),
            ("ok", "'yes'"),
        ),
        verb(
            "check_amount",
            "ticket",
            Some("claim.amount < 1500".into()),
            ("ok", "'yes'"),
        ),
        verb(
            "check_secret",
            "ticket",
            Some("claim.secret < 'p'".into()),
            ("ok", "'yes'"),
        ),
    ]
}

/// Declares the lake and writes it as twin `b` of a pair: each field
/// `hidden` hides as [`hidden_value`] gives it, the first twin's values in
/// both where the pair is of existence (`gone`), whose second twin writes
/// each hidden record and link under the vault instead, one record of it.
/// Beside the claims, trips and links stand the features. `c9` is held
/// last. Answers the position a read as of one reads at, before a later
/// write.
async fn plain(
    l: &Lake,
    store: &dyn lakelands_api::LakeStore,
    hidden: Hidden,
    gone: bool,
    b: bool,
) -> Result<At, Failure> {
    for d in declarations() {
        l.declare_json(d).await?;
    }
    let vaulted = |ty: &str, key: &str| gone && b && hidden.hides(ty, key, "");
    let in_vault = |ty: &'static str, key: &str| if vaulted(ty, key) { VAULT } else { ty };
    let put = |ty: &str, key: &str, fields: Value| -> Result<Value, Failure> {
        let mut out = serde_json::Map::new();
        for (f, v) in fields.as_object().into_iter().flatten() {
            ensure!(
                !STAMPED.contains(&f.as_str()),
                "a broken fixture: its field `{f}` bears the name of an instant the store stamps"
            );
            let v = if hidden.hides(ty, key, f) && f == "since" {
                json!(if b { LATER } else { SINCE })
            } else if hidden.hides(ty, key, f) {
                hidden_value(v, key, b && !gone)
            } else {
                v.clone()
            };
            out.insert(f.clone(), v);
        }
        Ok(Value::Object(out))
    };
    // A link is a record of its link type: under the vault, one record.
    let link = async |(ty, from, on, to): (&str, &str, &str, &str), by: &str, label: &str| {
        let key = format!("{from}:{}", to.replace(':', "%3A"));
        let lt = format!("sys.link.{ty}.{on}");
        let fields = put(&lt, &key, json!({"by": by, "label": label}))?;
        if vaulted(&lt, &key) {
            let mut fields = fields;
            (fields["from"], fields["to"]) = (json!(from), json!(to));
            return l.upsert(VAULT, &key, fields).await.map(|_| ());
        }
        let (ty, on, from, to) = (&ty.into(), &on.into(), &from.into(), &to.into());
        let linked = (l.cat).link(&l.own, ty, from, on, to, patch(fields), Default::default());
        linked.await.must("linking").map(|_| ())
    };
    for (key, name) in [("t1", "rome"), ("t2", "oslo")] {
        let f = put("trip", key, json!({"name": name, "since": SINCE}))?;
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
        let by = if from == "c2" { "bob" } else { "alice" };
        link(("claim", from, "on_trip", to), by, "trip").await?;
    }
    // A ticket on each claim a pair hides, its verbs reading across.
    for c in ["c1", "c2", "c9"] {
        let key = format!("k:{c}");
        let f = put("ticket", &key, json!({ "claim": c }))?;
        l.upsert(in_vault("ticket", &key), &key, f).await?;
    }
    // A memo on each of three claims' links.
    for (memo, on) in [("m1", "c1:t1"), ("m2", "c2:t1"), ("m9", "c9:t1")] {
        l.upsert("memo", memo, json!({"note": "memo"})).await?;
        let by = if memo == "m2" { "bob" } else { "alice" };
        link(("memo", memo, "about", on), by, "memo").await?;
    }
    let mark = store.head().await.must("reading the head")?;
    l.upsert("claim", "c3", json!({"note": "hotel"})).await?;
    restrict(store, in_vault("claim", "c9"), "c9", "a hold").await?;
    // The owner proposes each record a pair hides written again.
    let mut ids = Vec::new();
    for (ty, key, fields) in [
        ("claim", "c1", json!({"note": "taxi", "secret": "c1"})),
        ("claim", "c2", json!({"note": "taxi", "secret": "c2"})),
        ("claim", "c9", json!({"note": "taxi", "secret": "c9"})),
        ("trip", "t1", json!({"name": "rome"})),
        ("trip", "t2", json!({"name": "oslo"})),
    ] {
        let (ty, patch) = (in_vault(ty, key).into(), patch(put(ty, key, fields)?));
        let write = ProposedWrite::Upsert {
            ty,
            key: key.into(),
            patch,
            opts: Default::default(),
        };
        match l.cat.propose(&l.own, write, "the grid".into()).await {
            Ok(Outcome::Proposed { proposal, .. }) => ids.push((key, proposal)),
            other => return Err(Failure::new(format!("the owner's proposal was {other:?}"))),
        }
    }
    // A set over each field a claim hides, and a set of each, declared
    // last: their memberships, which the hidden part moves, are logged
    // after every write.
    for (f, holds) in [
        ("note", "note < 'm'"),
        ("amount", "amount < 1500"),
        ("secret", "secret < 'm'"),
    ] {
        let set = |name: String, filter: String| json!({"declare": "object_set", "name": name, "of": "claim", "where": filter});
        l.declare_json(set(format!("inner_{f}"), holds.to_owned()))
            .await?;
        l.declare_json(set(format!("outer_{f}"), format!("in_set('inner_{f}')")))
            .await?;
    }
    Ok((mark, ids))
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
) -> Result<Vec<(Store, At)>, Failure> {
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
        let at = plain(&l, store.as_ref(), hidden, gone, b).await?;
        out.push((store, at));
    }
    let mut logs = Vec::new();
    for (store, _) in &out {
        // A set's memberships follow from the records, not the script.
        let events = log_events(store.as_ref()).await?;
        let script = |e: &&lakelands_api::event::Event| {
            !matches!(e.body, EventBody::ObjectSetChanged { .. })
        };
        logs.push(Value::Array(
            (events.iter().filter(script))
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
    // The head an answer was read at moves with every event, a set's
    // memberships among them, which the hidden part moves: it is left out,
    // and so are a proposal's id, which is a position, an ingest window's
    // position, and how far a write's consequences reach, which tells that
    // something reacted (006).
    let head = |ok: &mut Value| {
        if let Some(ok) = ok.as_object_mut() {
            ok.remove("through");
            for at in ["position", "proposal"] {
                ok.entry(at).and_modify(|x| *x = json!("the head"));
            }
        }
    };
    match v.get_mut("ok") {
        Some(Value::Array(windows)) => windows.iter_mut().for_each(head),
        Some(ok) => head(ok),
        None => {}
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
/// at `none`; the owner where they are empty. Each feeds and reviews too,
/// so that `ingest`, the listing and a decision reach what it is kept from
/// before they refuse it a grant.
fn reader(store: &dyn lakelands_api::LakeStore, name: &str, grants: &str) -> Principal {
    if grants.is_empty() {
        return owner(store);
    }
    let mut grants: Value = serde_json::from_str(grants).expect("a reader's grants read");
    grants["types"][VAULT] = json!({"default": "none"});
    (grants["ingest"], grants["review"]) = (json!(true), json!(true));
    let grants: Grants = serde_json::from_value(grants).expect("a reader's grants read");
    principal(store, PrincipalKind::Agent, name, grants)
}

async fn the_rights_grid_holds_over_every_feature(store: Store, given: Given) -> Held {
    let lake = store.lake().clone();
    let mut own = Some(store);
    let (mut red, mut blind, mut alike, mut controls, mut unasked) =
        (Vec::new(), Vec::new(), 0, 0, 0);
    let pairs = (HIDDEN.iter()).flat_map(|&h| [(h, false), (h, true)]);
    for (hidden, gone) in pairs.filter(|&(h, gone)| !(gone && matches!(h, Secret | Since))) {
        let pair = twins(&given, own.take(), &lake, (hidden, gone)).await?;
        let (a, b, at) = (
            doors(pair[0].0.as_ref()),
            doors(pair[1].0.as_ref()),
            &pair[0].1,
        );
        let reads = READERS.iter().flat_map(|r| DOORS.map(|d| (r, d)));
        let writes = (WRITES.iter()).flat_map(|&d| READERS.iter().rev().map(move |r| (r, d)));
        let cells: Vec<_> = reads.chain(writes).collect();
        for (&(name, grants, not_shown), door) in cells {
            let control = grants.is_empty();
            if !control && hidden != Hold && !not_shown.contains(&hidden) {
                continue;
            }
            let who = reader(pair[0].0.as_ref(), name, grants);
            for &target in hidden.targets() {
                {
                    let pair = (hidden, gone);
                    let Some(mut x) = door.ask(a.as_ref(), &who, target, at, pair).await else {
                        unasked += 1;
                        continue;
                    };
                    let y = door.ask(b.as_ref(), &who, target, at, pair).await;
                    let mut y = y.unwrap_or_default();
                    for v in [&mut x, &mut y] {
                        ranked(v);
                        if matches!(door, Migrate | Retype) {
                            trimmed(v);
                        }
                    }
                    let (feature, ty, key, field) = target;
                    let pair = if gone { "gone" } else { "values" };
                    let cell = format!(
                        "{feature:?}/{name}/{door:?} on {ty} {key}.{field} ({hidden:?}, {pair})"
                    );
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
        "the rights grid: {alike} cells alike, {} red, {controls} controls \
         differ (the description says {CONTROLS}), {unasked} asked nothing; red and not known:\n{}\n\
         known and holding now, to be removed:\n{}\ncontrols alike, a twin that hides nothing there:\n{}",
        red.len(),
        new.join("\n"),
        held.join("\n"),
        blind.join("\n")
    );
    Ok(())
}

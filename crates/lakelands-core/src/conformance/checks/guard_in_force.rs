//! A trigger's guard paired with another type's record in force
//! (`docs/decisions/100-a-guard-may-read-the-record-in-force.md`;
//! lakelands#1676): `price > in_force(box.hi, box.from <= at, box.symbol =
//! symbol)` reads, at the tick's write, `hi` of the bound of its symbol
//! whose `from` is the greatest at or below its `at`, among the bounds every
//! sight the write is judged by is answered, null where none is.

use super::only::holder;
use super::quarantine::{fed, kept};
use crate::conformance::fixture::{ingest, reviewer, source_record};
use crate::conformance::prelude::*;

check!(
    GUARD_IN_FORCE_FIRES_ON_ITS_OWN_BOUND = a_guard_fires_on_a_tick_above_its_own_bound,
    "100: `price > in_force(box.hi, box.from <= at, box.symbol = symbol)` declares, and fires on a tick above its symbol's bound, not on one below it, nor on another symbol's tick at the same price"
);

check!(
    GUARD_IN_FORCE_CROSSING = a_guard_fires_on_the_crossing,
    "100: with `preceding(price, at, symbol) <= in_force(…)` beside it, the guard fires on the tick that crosses the bound and not on the ticks after it above the bound; both calls read the bound in force at this tick's `at`, so a tick under a later, higher bound crosses it from a price above the bound before"
);

check!(
    GUARD_IN_FORCE_BOX = a_box_fires_on_leaving_either_side,
    "100: `price > in_force(box.hi, …) or price < in_force(box.lo, …)` fires on a tick above the box and on one below it, and not on one inside it"
);

check!(
    GUARD_IN_FORCE_LATER_BOUND_AND_BACKDATED = a_later_bound_governs_from_its_from,
    "100: a bound with a later `from` governs the ticks at or after it, one at `from` exactly included, and the bound before it the ticks before; a backdated tick, written after them, pairs the bound in force at its own `at`"
);

check!(
    GUARD_IN_FORCE_LAID_OVER_IN_AN_APPEND = a_delivery_lays_its_bounds_over_in_its_order,
    "100: in one delivery a bound before a tick governs it and one after it does not; a bound the delivery moves into another symbol no longer governs a tick of the old one after it, though its head version would; and a bound the delivery refuses and quarantines lays nothing over"
);

check!(
    GUARD_IN_FORCE_NO_BOUND = a_tick_of_no_bound_reads_null,
    "100: a tick of a symbol no bound has, or of no symbol, fires neither `price > in_force(…)` nor `not (price > in_force(…))` when strict, and fires `in_force(…) is null` and `not (price > in_force(…))` without `when_strict`; a tick above its bound fires the first strict or not"
);

check!(
    GUARD_IN_FORCE_UNDER_A_HIDING = a_guard_pairs_in_its_writer_s_sight,
    "100: a bound its writer's `only` does not answer is passed over and the older one paired; so is one whose `from` a hiding lowers; one whose `hi` a hiding lowers to `none` or to `masked` reads null to that writer, never the older bound's, and not to the owner; at an accept the bound one of the proposer's and the reviewer's sight passes over is passed over; and a write's answer does not name the trigger to a writer a hiding lowers a field it reads for, while the firing is logged"
);

check!(
    GUARD_IN_FORCE_WITHOUT_A_GRANT = a_writer_without_a_grant_on_the_bound_reads_null,
    "100: a writer with no read on `box`, on `box.from`, on `box.symbol` or on `box.hi`, or with `box.hi` masked, reads null, fires nothing and writes; the owner's write of the same tick fires"
);

check!(
    GUARD_IN_FORCE_REFUSED = a_guard_s_pairing_is_refused_naming_the_trigger,
    "100: said at the trigger, `in_force` in a trigger on a view is refused `in_force_here`; a bound by `>=` or `<`, a pairing with the trigger's own type or with a view `guard_in_force_form`; a matched pair on a number or of two kinds, an order pair of two kinds `guard_in_force_pair`; a type that is none `undeclared`, a field it lacks `unknown_field`, and a field of it computed at the read `reads_at_read`"
);

check!(
    GUARD_IN_FORCE_KEEPS_ITS_FIELDS = the_fields_a_guard_pairs_by_are_kept,
    "100: declaring `box` again or migrating it so that `from` goes, `symbol` becomes a number is refused `field_read_elsewhere` naming that field, and one that computes `hi` at the read `read_as_stored` naming `hi`, as for a field that pairs; migrating `tick` so that `at` becomes text is refused `guard_in_force_pair` at the trigger; and a trigger's author masked from `box.hi` is not granted the guard"
);

check!(
    GUARD_IN_FORCE_KEEPS_ITS_TYPE = the_type_a_guard_pairs_is_kept,
    "100: withdrawing `box` is refused `withdrawn_in_use`, naming the trigger that pairs it, then a field computed at the read that pairs it, and is applied once both have gone"
);

check!(
    GUARD_IN_FORCE_READ_APART_FROM_PRECEDING = a_guard_reads_in_force_apart_from_preceding,
    "100: a guard holding `in_force(box.hi, box.from <= at, box.symbol = symbol)` and `preceding(hi, box, from, at, symbol, symbol)`, whose series are spelled alike, reads each its own"
);

check!(
    GUARD_IN_FORCE_LATER_WRITE_MOVES_NO_FIRING = a_later_bound_moves_no_firing,
    "100: after a firing, a rewrite of the bound it paired, a new bound and the bound's retirement set nothing off, and the feed tells the firing as it was, overruled by nothing"
);

check!(
    GUARD_IN_FORCE_BOUND_TAKEN_BACK_STANDS = a_bound_taken_back_overrules_no_firing,
    "100 and 065: a removal of a source's values, a forget of a source and a purge of a source's bounds overrule no firing paired with a bound they take back, and the feed tells each; a removal of the source that wrote a tick overrules its firing; and of `price > in_force(…) and hand = 'a'`, a removal of the source of `hand` overrules the firing of its write while the firing of `price > in_force(…)` on that write stands"
);

/// `in_force` of the bound's `f`, at the tick's `at`, of its symbol.
fn paired(f: &str) -> String {
    format!("in_force(box.{f}, box.from <= at, box.symbol = symbol)")
}

/// The guard above the bound.
fn above() -> String {
    format!("price > {}", paired("hi"))
}

/// The type `ty`, keyed by `no`, of `fields` (`name:kind`, `text` where no
/// kind is named), its constraints `c`.
fn typed(ty: &str, fields: &str, c: Value) -> Value {
    let field = |f: &str| {
        let (n, k) = f.split_once(':').unwrap_or((f, "text"));
        json!({"name": n, "type": {"kind": k}, "authority": "shared"})
    };
    let fields: Vec<Value> = fields.split(' ').map(field).collect();
    json!({"declare": "type", "name": ty, "key": ["no"], "fields": fields, "constraints": c})
}

/// `box`'s fields.
const BOX: &str = "no symbol from:date_time hi:number lo:number hand";

/// `box`, its `hi` under 1 000.
fn bound_type(fields: &str) -> Value {
    typed(
        "box",
        fields,
        json!([{"name": "sane", "holds": "hi < 1000"}]),
    )
}

/// The trigger `name` on `on`, `when`.
fn trigger(name: &str, on: &str, when: &str) -> Value {
    json!({"declare": "trigger", "name": name, "on": on, "when": when})
}

/// `box`, `tick` and on `tick` each trigger `(name, when)`.
async fn declared(l: &Lake, triggers: &[(&str, &str)]) -> Held {
    l.declare_json(bound_type(BOX)).await?;
    let tick = typed(
        "tick",
        "no symbol at:date_time price:number hand",
        json!([]),
    );
    l.declare_json(tick).await?;
    for (name, when) in triggers {
        l.declare_json(trigger(name, "tick", when)).await?;
    }
    Ok(())
}

/// A time of 1 January 2026, `hh:mm`.
fn at(hm: &str) -> String {
    format!("2026-01-01T{hm}:00Z")
}

/// A bound of `symbol` from `from`, `hi`, `hand`.
fn bound(symbol: &str, from: &str, hi: f64, hand: &str) -> Value {
    json!({"symbol": symbol, "from": at(from), "hi": hi, "hand": hand})
}

/// Each bound `(key, symbol, from, hi, hand)`, the owner's.
async fn bounds(l: &Lake, all: &[(&str, &str, &str, f64, &str)]) -> Held {
    for (key, symbol, from, hi, hand) in all {
        l.upsert("box", key, bound(symbol, from, *hi, hand)).await?;
    }
    Ok(())
}

/// A tick of `symbol` at `at`, `price`, `hand`.
fn tick(symbol: &str, hm: &str, price: f64, hand: &str) -> Value {
    json!({"symbol": symbol, "at": at(hm), "price": price, "hand": hand})
}

/// The triggers fired after `after`, each with the key it fired on.
async fn fired(s: &dyn LakeStore, after: Position) -> Result<Vec<(String, String)>, Failure> {
    let mut fired = Vec::new();
    for e in events_after(s, after).await? {
        if let EventBody::TriggerFired { trigger, key, .. } = e.body {
            fired.push((trigger.to_string(), key.to_string()));
        }
    }
    Ok(fired)
}

/// The names of `fired`, sorted and joined by a space.
fn names(fired: impl IntoIterator<Item = String>) -> String {
    let mut names: Vec<String> = fired.into_iter().collect();
    names.sort();
    names.join(" ")
}

/// `who`'s write of `fields` at `key` of `tick`: the triggers it fired, and
/// those its answer names.
async fn wrote(
    s: &dyn LakeStore,
    l: &Lake,
    who: &Principal,
    key: &str,
    fields: Value,
) -> Result<(String, String), Failure> {
    let head = s.head().await.must("reading the head")?;
    let w = try_upsert(l.cat.as_ref(), who, "tick", key, fields).await;
    let Outcome::Applied { set_off, .. } = w.must("a tick")? else {
        return Err(Failure::new("the tick is applied".to_owned()));
    };
    let fired = fired(s, head).await?.into_iter().map(|(t, _)| t);
    let told = set_off.fired.into_iter().map(|t| t.to_string());
    Ok((names(fired), names(told)))
}

/// The triggers `who`'s write of `fields` at `key` of `tick` fired.
async fn fires(
    s: &dyn LakeStore,
    l: &Lake,
    who: &Principal,
    key: &str,
    fields: Value,
) -> Result<String, Failure> {
    Ok(wrote(s, l, who, key, fields).await?.0)
}

/// Each `(trigger, key)` the feed tells a firing of as overruled, sorted,
/// once each: a record's retirement sets its triggers off as well.
async fn overruled(s: &dyn LakeStore) -> Result<Vec<(String, String)>, Failure> {
    let filter = FeedFilter {
        kinds: vec!["trigger_fired".to_owned()],
        ..FeedFilter::default()
    };
    let polled = s.feed().poll(Position::GENESIS, &filter, 1024).await;
    let polled = polled.must("polling the feed")?;
    let mut out: Vec<_> = (polled.events.iter())
        .filter(|e| polled.overruled.contains(&e.position))
        .filter_map(|e| match &e.body {
            EventBody::TriggerFired { trigger, key, .. } => {
                Some((trigger.to_string(), key.to_string()))
            }
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// The declaration `json` describes, by `who`, answered as it came.
async fn declare_by(l: &Lake, who: &Principal, json: Value) -> Result<Answered, Failure> {
    let d = serde_json::from_value(json).map_err(|e| Failure::new(e.to_string()))?;
    Ok(l.cat.declare(who, d).await)
}

/// A door's answer.
type Answered = lakelands_api::Result<Outcome>;

/// A migration to the type `json` describes, answered as it came.
async fn migrated(l: &Lake, mut json: Value) -> Result<lakelands_api::Result<()>, Failure> {
    json.as_object_mut().map(|o| o.remove("declare"));
    let to = serde_json::from_value(json).map_err(|e| Failure::new(e.to_string()))?;
    Ok(l.cat.migrate(&l.own, Migration::to(to)).await.map(drop))
}

/// A hiding of `field` to `access` on a bound whose `hand` is `hand`.
fn hiding(hand: &str, field: &str, access: &str) -> Value {
    json!({"field": "hand", "values": [hand], "fields": {field: access}})
}

/// Grants that write everything, and on `box` `grant`.
fn on_box(grant: Value) -> Value {
    json!({"default": "write", "types": {"box": grant}})
}

async fn a_guard_fires_on_a_tick_above_its_own_bound(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    declared(&l, &[("up", &above())]).await?;
    bounds(
        &l,
        &[
            ("B1", "A", "09:00", 10.0, "a"),
            ("B2", "B", "09:00", 20.0, "a"),
        ],
    )
    .await?;
    let mut got = Vec::new();
    for (key, symbol, price) in [("T1", "A", 15.0), ("T2", "A", 5.0), ("T3", "B", 15.0)] {
        got.push(fires(s, &l, &l.own, key, tick(symbol, "10:00", price, "a")).await?);
    }
    ensure!(
        got == ["up", "", ""],
        "a tick above its symbol's bound fires, one below and another symbol's do not: {got:?}"
    );
    Ok(())
}

async fn a_guard_fires_on_the_crossing(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let p = paired("hi");
    let crossing = format!("price > {p} and preceding(price, at, symbol) <= {p}");
    declared(&l, &[("cross", &crossing)]).await?;
    bounds(
        &l,
        &[
            ("B1", "A", "09:00", 10.0, "a"),
            ("B2", "A", "10:45", 20.0, "a"),
        ],
    )
    .await?;
    let mut got = Vec::new();
    for (key, hm, price) in [
        ("T1", "10:00", 8.0),
        ("T2", "10:10", 12.0),
        ("T3", "10:20", 13.0),
        ("T4", "10:30", 9.0),
        ("T5", "10:40", 11.0),
        // Above 20 from 11: the bound at 10:40 was 10, at 10:50 it is 20.
        ("T6", "10:50", 25.0),
        ("T7", "11:00", 30.0),
    ] {
        got.push(fires(s, &l, &l.own, key, tick("A", hm, price, "a")).await?);
    }
    let want = ["", "cross", "", "", "cross", "cross", ""];
    ensure!(
        got == want,
        "the crossing fires, the ticks after it do not, and both sides read this tick's bound: \
         {got:?}"
    );
    Ok(())
}

async fn a_box_fires_on_leaving_either_side(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let out = format!("{} or price < {}", above(), paired("lo"));
    declared(&l, &[("out", &out)]).await?;
    let mut b = bound("A", "09:00", 10.0, "a");
    b["lo"] = json!(5);
    l.upsert("box", "B1", b).await?;
    let mut got = Vec::new();
    for (key, price) in [("T1", 12.0), ("T2", 7.0), ("T3", 3.0)] {
        got.push(fires(s, &l, &l.own, key, tick("A", "10:00", price, "a")).await?);
    }
    ensure!(
        got == ["out", "", "out"],
        "a tick above the box and one below it fire, one inside does not: {got:?}"
    );
    Ok(())
}

async fn a_later_bound_governs_from_its_from(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    declared(&l, &[("up", &above())]).await?;
    bounds(
        &l,
        &[
            ("B1", "A", "09:00", 10.0, "a"),
            ("B2", "A", "11:00", 20.0, "a"),
        ],
    )
    .await?;
    let mut got = Vec::new();
    for (key, hm, price) in [
        ("T1", "10:59", 15.0),
        ("T2", "11:00", 15.0),
        ("T3", "11:30", 25.0),
        ("T4", "11:30", 15.0),
        ("T5", "10:30", 15.0),
    ] {
        got.push(fires(s, &l, &l.own, key, tick("A", hm, price, "a")).await?);
    }
    ensure!(
        got == ["up", "", "up", "", "up"],
        "a later bound governs from its `from` on, the one before it before, and a backdated \
         tick pairs the bound at its own `at`: {got:?}"
    );
    Ok(())
}

async fn a_delivery_lays_its_bounds_over_in_its_order(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let (cat, own) = (l.cat.as_ref(), &l.own);
    declared(&l, &[("up", &above())]).await?;
    bounds(&l, &[("B1", "A", "09:00", 10.0, "a")]).await?;
    let t = |key: &str, price: f64| source_record("tick", key, tick("A", "10:00", price, "a"));
    let b = |key: &str, symbol: &str, from: &str, hi: f64| {
        source_record("box", key, bound(symbol, from, hi, "a"))
    };
    let firing = |k: &str| ("up".to_owned(), k.to_owned());
    // A bound between ticks governs the ticks after it.
    let head = s.head().await.must("reading the head")?;
    let one = vec![
        t("X1", 15.0),
        b("B9", "A", "09:30", 20.0),
        t("X2", 15.0),
        t("X3", 25.0),
    ];
    ingest(cat, own, "feed", one)
        .await
        .must("the first delivery")?;
    let first = fired(s, head).await?;
    // B9 leaves `A`: its head version is not read.
    let head = s.head().await.must("reading the head")?;
    let two = vec![b("B9", "Z", "09:30", 20.0), t("X4", 15.0)];
    ingest(cat, own, "feed", two)
        .await
        .must("the second delivery")?;
    let second = fired(s, head).await?;
    // A bound refused and quarantined lays nothing over.
    let head = s.head().await.must("reading the head")?;
    let three = vec![b("B8", "A", "09:45", 5000.0), t("X5", 15.0)];
    fed(&l, own, "feed", kept("third"), three)
        .await
        .must("the third delivery")?;
    let third = fired(s, head).await?;
    let none = l.read("box", "B8").await.is_err();
    ensure!(
        first == [firing("X1"), firing("X3")]
            && second == [firing("X4")]
            && third == [firing("X5")]
            && none,
        "a bound governs the ticks after it in a delivery, one moved away no longer does, and \
         one refused lays nothing over: {first:?}, {second:?}, {third:?}, B8 absent {none}"
    );
    Ok(())
}

async fn a_tick_of_no_bound_reads_null(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let below = format!("not ({})", above());
    let none = format!("{} is null", paired("hi"));
    let plain = [("up", above()), ("not_up", below.clone()), ("none", none)];
    let plain: Vec<_> = plain.iter().map(|(n, w)| (*n, w.as_str())).collect();
    declared(&l, &plain).await?;
    for (name, when) in [("up_strict", above()), ("not_up_strict", below)] {
        let mut t = trigger(name, "tick", &when);
        t["when_strict"] = json!(true);
        l.declare_json(t).await?;
    }
    bounds(&l, &[("B1", "A", "09:00", 10.0, "a")]).await?;
    let mut got = Vec::new();
    for (key, symbol, price) in [("T1", "C", 15.0), ("T2", "A", 15.0), ("T3", "A", 5.0)] {
        got.push(fires(s, &l, &l.own, key, tick(symbol, "10:00", price, "a")).await?);
    }
    let bare = json!({"at": at("10:00"), "price": 15, "hand": "a"});
    got.push(fires(s, &l, &l.own, "T4", bare).await?);
    let want = [
        "none not_up",
        "up up_strict",
        "not_up not_up_strict",
        "none not_up",
    ];
    ensure!(
        got == want,
        "a tick of no bound reads null: strict or not it does not fire above it, strict it does \
         not fire below it, not strict it fires `is null` and below; {got:?}"
    );
    Ok(())
}

async fn a_guard_pairs_in_its_writer_s_sight(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let own = &l.own;
    declared(&l, &[("up", &above())]).await?;
    bounds(
        &l,
        &[
            ("B1", "A", "09:00", 10.0, "b"),
            ("B2", "A", "10:00", 20.0, "a"),
            ("B3", "H", "09:00", 10.0, "a"),
            ("B4", "H", "10:00", 20.0, "z"),
            ("B5", "F", "09:00", 10.0, "a"),
            ("B6", "F", "10:00", 12.0, "y"),
        ],
    )
    .await?;
    let only = holder(s, "o", on_box(json!({"only": {"hand": "b"}})));
    let from = holder(
        s,
        "h",
        on_box(json!({"hide_where": [hiding("z", "from", "none")]})),
    );
    let none = holder(
        s,
        "n",
        on_box(json!({"hide_where": [hiding("y", "hi", "none")]})),
    );
    let masked = holder(
        s,
        "m",
        on_box(json!({"hide_where": [hiding("y", "hi", "masked")]})),
    );
    let at_half = |symbol: &str| tick(symbol, "10:30", 15.0, "a");
    let mut got = Vec::new();
    for (who, key, symbol) in [
        (&only, "O1", "A"),
        (own, "O2", "A"),
        (&from, "H1", "H"),
        (own, "H2", "H"),
        (&none, "N1", "F"),
        (&masked, "M1", "F"),
        (own, "F1", "F"),
    ] {
        got.push(wrote(s, &l, who, key, at_half(symbol)).await?);
    }
    let fire: Vec<_> = got.iter().map(|(f, _)| f.as_str()).collect();
    let told: Vec<_> = got.iter().map(|(_, t)| t.as_str()).collect();
    // At an accept the reviewer's sight passes B4 over; the owner's does not.
    let blind = json!({"default": "read", "review": true,
        "types": {"box": {"hide_where": [hiding("z", "from", "none")]}}});
    let blind = holder(s, "r", blind);
    let mut accepted = Vec::new();
    for (key, by) in [("P1", &blind), ("P2", &reviewer(s, false))] {
        let w = json!({"write": "upsert", "type": "tick", "key": key, "patch": at_half("H")});
        let w = serde_json::from_value(w).map_err(|e| Failure::new(e.to_string()))?;
        let p = l.cat.propose(own, w, "read".into()).await;
        let Outcome::Proposed { proposal, .. } = p.must("proposing a tick")? else {
            return Err(Failure::new("the tick is proposed".to_owned()));
        };
        let head = s.head().await.must("reading the head")?;
        (l.cat.accept(by, &proposal).await).must("accepting the tick")?;
        accepted.push(names(fired(s, head).await?.into_iter().map(|(t, _)| t)));
    }
    ensure!(
        fire == ["up", "", "up", "", "", "", "up"]
            && told == ["up", "", "", "", "", "", "up"]
            && accepted == ["up", ""],
        "a bound its writer is not answered, or whose `from` is hidden, is passed over; a \
         hidden `hi` reads null; an accept pairs in both sights; a writer a hiding lowers a \
         field for is not told: fired {fire:?}, told {told:?}, accepted {accepted:?}"
    );
    Ok(())
}

async fn a_writer_without_a_grant_on_the_bound_reads_null(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    declared(&l, &[("up", &above())]).await?;
    bounds(&l, &[("B1", "A", "09:00", 10.0, "a")]).await?;
    let fields = |f: &str, a: &str| on_box(json!({"fields": {f: a}}));
    let mut got = Vec::new();
    for (n, grants) in [
        ("box", on_box(json!({"default": "none"}))),
        ("from", fields("from", "none")),
        ("symbol", fields("symbol", "none")),
        ("hi", fields("hi", "none")),
        ("masked", fields("hi", "masked")),
    ] {
        let who = holder(s, n, grants);
        let key = format!("W-{n}");
        got.push(fires(s, &l, &who, &key, tick("A", "10:30", 15.0, "a")).await?);
    }
    got.push(fires(s, &l, &l.own, "W-own", tick("A", "10:30", 15.0, "a")).await?);
    ensure!(
        got == ["", "", "", "", "", "up"],
        "a writer with no read on `box`, its `from`, `symbol` or `hi`, or `hi` masked, writes \
         and fires nothing; the owner fires: {got:?}"
    );
    Ok(())
}

async fn a_guard_s_pairing_is_refused_naming_the_trigger(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    declared(&l, &[]).await?;
    let half = json!({"name": "half", "type": {"kind": "number"}, "authority": "derived",
        "derived": "hi / 2", "computed_at": "read"});
    let mut b = bound_type(BOX);
    if let Some(f) = b["fields"].as_array_mut() {
        f.push(half);
    }
    l.declare_json(b).await?;
    let f = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    let spot = json!({"declare": "type", "name": "spot", "key": ["symbol"],
        "fields": [f("symbol", "text"), f("n", "number")],
        "view": {"source": "tick", "group_by": ["symbol"],
                 "measures": [{"name": "n", "expr": "count()"}]}});
    l.declare_json(spot).await?;
    l.set("view_kept.spot", json!(true)).await?;
    let on_view = "spot in_force(box.hi, box.from <= n, box.symbol = symbol) is null";
    let mut tried = vec![(on_view.to_owned(), "in_force_here")];
    for row in [
        "box.hi, box.from >= at, box.symbol = symbol guard_in_force_form",
        "box.hi, box.from < at, box.symbol = symbol guard_in_force_form",
        "tick.price, tick.at <= at, tick.symbol = symbol guard_in_force_form",
        "spot.n, spot.n <= price, spot.symbol = symbol guard_in_force_form",
        "box.hi, box.from <= at, box.hi = price guard_in_force_pair",
        "box.hi, box.from <= at, box.symbol = price guard_in_force_pair",
        "box.hi, box.from <= price, box.symbol = symbol guard_in_force_pair",
        "none.hi, none.from <= at, none.symbol = symbol undeclared",
        "box.cost, box.from <= at, box.symbol = symbol unknown_field",
        "box.half, box.from <= at, box.symbol = symbol reads_at_read",
    ] {
        let (args, case) = row.rsplit_once(' ').unwrap_or((row, ""));
        tried.push((format!("tick price > in_force({args})"), case));
    }
    let mut red = Vec::new();
    for (on_when, case) in tried {
        let (on, when) = on_when.split_once(' ').unwrap_or(("", ""));
        let a = declare_by(&l, &l.own, trigger("t", on, when)).await?;
        let want = json!({"code": case, "at": [{"code": "trigger", "name": "t"}]});
        if !invalid_like(&a, &want) {
            red.push(format!("`{when}` on `{on}`: want {want}, got {a:?}"));
        }
    }
    ensure!(red.is_empty(), "{}", red.join("; "));
    Ok(())
}

async fn the_fields_a_guard_pairs_by_are_kept(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    declared(&l, &[("up", &above())]).await?;
    let lo = json!({"name": "hi", "type": {"kind": "number"}, "authority": "derived",
        "derived": "lo", "computed_at": "read"});
    let mut at_read = bound_type("no symbol from:date_time lo:number hand");
    if let Some(f) = at_read["fields"].as_array_mut() {
        f.push(lo);
    }
    let mut red = Vec::new();
    for (field, d) in [
        ("from", bound_type("no symbol hi:number lo:number hand")),
        (
            "symbol",
            bound_type(&BOX.replace("symbol", "symbol:number")),
        ),
        ("hi", at_read),
    ] {
        let declared = declare_by(&l, &l.own, d.clone()).await?.map(drop);
        let migrated = migrated(&l, d).await?;
        for a in [declared, migrated] {
            // One computed at the read is refused as 052's pairing is.
            let named = match invalid_case(&a) {
                Some(Case::FieldReadElsewhere { field: f, .. }) => f == field,
                Some(Case::ReadAsStored) => refused_on(&a).is_some_and(|(f, _)| f == field),
                _ => false,
            };
            if !named {
                red.push(format!("without `{field}` as it is: {a:?}"));
            }
        }
    }
    // Its own type migrated so that `at` is text no longer orders with
    // `box.from`: the guard's pairing is refused, named at the trigger.
    let tick = typed("tick", "no symbol at price:number hand", json!([]));
    let a = migrated(&l, tick).await?;
    let want = json!({"code": "guard_in_force_pair",
        "at": [{"code": "after_migration", "name": "tick"}, {"code": "trigger", "name": "up"}]});
    if !invalid_like(&a, &want) {
        red.push(format!("`tick` migrated so that `at` is text: {a:?}"));
    }
    let author = json!({"schema": true, "restricted": true, "default": "write",
        "types": {"box": {"fields": {"hi": "masked"}}}});
    let author = holder(s, "author", author);
    let a = declare_by(&l, &author, trigger("mine", "tick", &above())).await?;
    if !matches!(reason(&a), Some(Refusal::NotGranted)) {
        red.push(format!("an author masked from `box.hi`: {a:?}"));
    }
    ensure!(red.is_empty(), "{}", red.join("; "));
    Ok(())
}

async fn the_type_a_guard_pairs_is_kept(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    declared(&l, &[("up", &above())]).await?;
    let withdraw =
        |kind: &str, name: &str| json!({"declare": "withdraw", "kind": kind, "name": name});
    let in_use =
        |by: &str| json!({"code": "withdrawn_in_use", "kind": "type", "name": "box", "by": by});
    let by_trigger = declare_by(&l, &l.own, withdraw("type", "box")).await?;
    l.declare_json(withdraw("trigger", "up")).await?;
    let f = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    let read = json!({"name": "bound", "type": {"kind": "number"}, "authority": "derived",
        "derived": paired("hi"), "computed_at": "read"});
    let quote = json!({"declare": "type", "name": "quote",
        "fields": [f("symbol", "text"), f("at", "date_time"), read]});
    l.declare_json(quote).await?;
    let by_field = declare_by(&l, &l.own, withdraw("type", "box")).await?;
    l.declare_json(withdraw("type", "quote")).await?;
    let gone = declare_by(&l, &l.own, withdraw("type", "box")).await?;
    ensure!(
        invalid_like(&by_trigger, &in_use("the trigger `up`"))
            && invalid_like(&by_field, &in_use("the field `bound` of `quote`"))
            && gone.is_ok(),
        "`box` is kept while a trigger or a field pairs it, and withdrawn after: \
         {by_trigger:?}, {by_field:?}, {gone:?}"
    );
    Ok(())
}

async fn a_guard_reads_in_force_apart_from_preceding(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    l.declare_json(typed("box", "no symbol from hi:number", json!([])))
        .await?;
    let fields = "no box from at symbol hi:number price:number";
    l.declare_json(typed("quote", fields, json!([]))).await?;
    let when = "price > in_force(box.hi, box.from <= at, box.symbol = symbol) \
        and preceding(hi, box, from, at, symbol, symbol) = 7";
    l.declare_json(trigger("both", "quote", when)).await?;
    let b = json!({"symbol": "A", "from": "a", "hi": 10});
    l.upsert("box", "B1", b).await?;
    let q = |bx: &str, hi: f64, price: f64| json!({"box": bx, "from": "f", "at": "b", "symbol": "A", "hi": hi, "price": price});
    let mut got = Vec::new();
    for (key, fields) in [
        ("Q1", q("k", 7.0, 1.0)),
        ("Q2", q("l", 0.0, 15.0)),
        ("Q3", q("m", 0.0, 15.0)),
    ] {
        let head = s.head().await.must("reading the head")?;
        let w = try_upsert(l.cat.as_ref(), &l.own, "quote", key, fields).await;
        w.must("a quote")?;
        got.push(names(fired(s, head).await?.into_iter().map(|(t, _)| t)));
    }
    ensure!(
        got == ["", "both", ""],
        "`in_force` reads the bound, 10, and `preceding` the quote before, 7, though their \
         series are spelled alike: {got:?}"
    );
    Ok(())
}

async fn a_later_bound_moves_no_firing(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    declared(&l, &[("up", &above())]).await?;
    bounds(&l, &[("B1", "A", "09:00", 10.0, "a")]).await?;
    let first = fires(s, &l, &l.own, "T1", tick("A", "10:30", 15.0, "a")).await?;
    let head = s.head().await.must("reading the head")?;
    l.upsert("box", "B1", json!({"hi": 50})).await?;
    bounds(&l, &[("B2", "A", "10:00", 20.0, "a")]).await?;
    l.retire("box", "B1").await?;
    let after = fired(s, head).await?;
    let told = fired(s, Position::GENESIS).await?;
    let overruled = overruled(s).await?;
    ensure!(
        first == "up"
            && after.is_empty()
            && told == [("up".to_owned(), "T1".to_owned())]
            && overruled.is_empty(),
        "a later write of a bound sets nothing off and moves no firing: fired {first:?}, then \
         {after:?}; told {told:?}, overruled {overruled:?}"
    );
    Ok(())
}

async fn a_bound_taken_back_overrules_no_firing(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let (cat, own) = (l.cat.as_ref(), &l.own);
    let handed = format!("{} and hand = 'a'", above());
    declared(&l, &[("up", &above()), ("handed", &handed)]).await?;
    for (source, key, symbol) in [("s1", "B1", "A"), ("s2", "B2", "B"), ("s3", "B3", "C")] {
        let b = source_record("box", key, bound(symbol, "09:00", 10.0, "a"));
        ingest(cat, own, source, vec![b])
            .await
            .must("a source's bound")?;
    }
    bounds(&l, &[("B4", "D", "09:00", 10.0, "a")]).await?;
    let mut got = Vec::new();
    for (key, symbol) in [("TA", "A"), ("TB", "B"), ("TC", "C"), ("TH", "D")] {
        got.push(fires(s, &l, own, key, tick(symbol, "10:30", 15.0, "x")).await?);
    }
    let tk = source_record("tick", "TK", tick("D", "10:30", 15.0, "x"));
    ingest(cat, own, "ticks", vec![tk])
        .await
        .must("a source's tick")?;
    let hand = source_record("tick", "TH", json!({"hand": "a"}));
    ingest(cat, own, "hands", vec![hand])
        .await
        .must("a source's hand")?;
    let far = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    (s.remove_values(&"s1".into(), &far, false, None).await).must("removing s1")?;
    (s.forget_source(&"s2".into(), None).await).must("forgetting s2")?;
    (s.purge_source(&"s3".into(), Some(&"box".into()), None)
        .await)
        .must("purging s3")?;
    let bounds_gone = overruled(s).await?;
    (s.remove_values(&"ticks".into(), &far, false, None).await).must("removing ticks")?;
    (s.remove_values(&"hands".into(), &far, false, None).await).must("removing hands")?;
    let after = overruled(s).await?;
    let told = fired(s, Position::GENESIS).await?;
    let pair = |t: &str, k: &str| (t.to_owned(), k.to_owned());
    let kept = ["TA", "TB", "TC"]
        .iter()
        .all(|k| told.contains(&pair("up", k)));
    ensure!(
        got == ["up", "up", "up", "up"]
            && bounds_gone.is_empty()
            && kept
            && after == [pair("handed", "TH"), pair("up", "TK")],
        "a bound taken back overrules nothing, a tick's own value taken back does, and the \
         rest of a `when` is decided again: fired {got:?}, overruled {bounds_gone:?} then \
         {after:?}, told {told:?}"
    );
    Ok(())
}

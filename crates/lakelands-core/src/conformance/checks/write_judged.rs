//! What a constraint reads that only its write can judge: the write's
//! clock and the record before it in a series
//! (`docs/decisions/059-a-type-declares-its-constraints.md`, extended;
//! lakelands#1552, lakelands#1556). Every write door judges it; no walk
//! judges it again. A trigger's guard reads the record before it the same
//! way (`docs/decisions/004-the-change-feed-is-the-log.md`, extended;
//! lakelands#1604).

use lakelands_api::write::{Breach, MigrationReport, Outcome, ValueTypeMigration};

use crate::conformance::fixture::{Answer, ingest, reviewer, source_record};
use crate::conformance::prelude::*;

check!(
    CLOCK_JUDGED_AT_THE_WRITE = a_constraint_compares_the_record_with_the_write_s_clock,
    "059, extended (lakelands#1556): a constraint `datum <= today()` reads the write's clock; a write dated ahead is refused `constraint_broken` at `upsert`, at `ingest`, at a verb's write and at a proposal, and one dated today lands, also at an accept; a write that does not move `datum` is not judged, a migration that adds the constraint over a record dated ahead lists no breach and is lossless, and a declaration that computes the field it reads again is not refused for it; `now()` or `today()` other than as a clock term on one side of a comparison whose other side is a field, either with or without a duration literal added or taken away, as in `today() > '2027-01-01' or datum <= today()`, `now() > now()` or `today() > datum - datum + '2027-01-01'`, is refused `clock_not_compared`"
);

check!(
    PRECEDING_JUDGED_AT_THE_WRITE = a_constraint_reads_the_record_before_it_in_its_series,
    "059 and 057, extended (lakelands#1552): `preceding(stand, datum, meter)` reads `stand` of the live record of the same `meter` whose `datum` is the greatest below this one's, null where none is; a reading lower than it is refused `constraint_broken` at `upsert`, at a verb's write, at `ingest`, where the second of one delivery is judged against the first, and at an accept, against the head then, past a reading the reviewer is not answered; the first reading, a higher one, another meter's and a backdated one higher than the reading before it land; a write that moves nothing it reads is not judged, though the reading before it is now higher; a predecessor its writer is not answered, or whose `datum` or `meter` is hidden from it, is passed over, a field it reads hidden on it makes the write `not_granted`, and a second writer is judged against the first's reading; a series matched on a number is refused `preceding_unlooked`, one ordered by a JSON value `preceding_unordered`, a `preceding` in a `find` `preceding_here`, and a migration that adds the constraint lists no breach"
);

check!(
    WALK_JUDGES_NO_WRITE_PART = a_walk_judges_nothing_only_a_write_judges,
    "059, extended (lakelands#1552): a constraint each of whose parts reads the clock or the record before it is judged by no walk and grants none: over records a write would find breaking it, a value type's renames in a field it reads are granted to one masked from another field it reads and list no breach, as if it were not declared, and a migration that renames a field it reads lists none and is lossless"
);

check!(
    WALK_JUDGES_THE_REST = a_walk_judges_what_reads_neither_clock_nor_preceding,
    "059, extended (lakelands#1552): of a constraint some of whose parts read the clock or the record before it, a walk and a migration judge the rest and grant by the fields the rest reads: a migration that adds `stand >= 0 and datum <= today()` over a record of `stand` -1 dated ahead lists that record breaking it and is lossy, and under `phase != 'x' and datum <= today()` a value type's renames in `phase` are granted to one masked from `datum`"
);

check!(
    PRECEDING_SERIES_CHOSEN = the_record_before_is_chosen_in_its_series,
    "059, extended (lakelands#1552): `preceding(stand, at, line, site)`, matched on a text and a reference, reads among records of `at` tied the first by key; a record of another `line` or `site` is no predecessor; `upsert_preflight` answers `constraint_broken` as `upsert` does; a retired record is passed over; a write that moves a record into another series is judged against the record before it there; and a restricted record is passed over for a writer not granted `restricted`, though not for one granted it"
);

check!(
    PRECEDING_IN_A_GUARD = a_guard_reads_the_record_before_it_in_its_series,
    "004 and 059, extended (lakelands#1604): a trigger whose `when` is `preceding(at, at, meter) is not null and date_bucket(at, 1h, o) != date_bucket(preceding(at, at, meter), 1h, o)` declares, and fires in the write's append on a reading whose predecessor of its meter lies in another hour; not on a meter's first reading, nor on a second in the same hour, nor on another meter's first; a backdated reading fires where the reading before it lies in an earlier hour, also before every reading of an hour already logged, and not where one of its own hour does or none does; a later write of another field fires again; of three readings of one meter in one ingest delivery, at 09:10, 10:20 and 10:05, the second and the third fire, each judged at its turn against the one before it among those already judged; a predecessor its writer is not answered is passed over, and a field a hiding lowers on it reads as null to its writer and not to the owner; an act that takes back the `at` and `meter` another source wrote of a reading overrules the firings of its ingest and of a later write of its `hand`, and no other; a series matched on a number is refused `preceding_unlooked`, at a declaration and at a migration that makes the matched field a number, and a `preceding` in a trigger on a view `preceding_here`"
);

/// A door's answer.
type Answered = lakelands_api::Result<Outcome>;

/// Whether each answer is refused breaking the constraint `name`.
macro_rules! all_broken {
    ($name:expr, $($a:expr),+) => {
        [$(broken(&$a)),+].iter().all(|c| c.as_deref() == Some($name))
    };
}

/// Today's date and the day after the next: still ahead past midnight.
fn days() -> (String, String) {
    const DAY: i64 = 86_400_000_000;
    let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let micros = wall.map_or(0, |d| d.as_micros() as i64);
    let day = |n: i64| Timestamp((micros.div_euclid(DAY) + n) * DAY).day();
    (day(0).unwrap_or_default(), day(2).unwrap_or_default())
}

/// The constraint a refusal says is broken, if it says one is.
fn broken(a: &impl Answer) -> Option<String> {
    if let Some(Refusal::Constraint(said)) = reason(a)
        && let Case::ConstraintBroken { constraint, .. } = &said.case
    {
        return Some(constraint.to_string());
    }
    None
}

/// The type `ty` keyed by `key`, its fields `name:kind=of` (`of` a target or a value type;
/// `text` where no kind is named), which every door writes, its constraints `c`.
fn typed(ty: &str, key: &[&str], fields: &str, c: Value) -> Value {
    let field = |f: &str| {
        let (n, k) = f.split_once(':').unwrap_or((f, "text"));
        let kind = match k.split_once('=') {
            Some(("reference", to)) => json!({"kind": "reference", "target": to}),
            Some((k, vt)) => json!({"kind": k, "value_type": vt}),
            None => json!({"kind": k}),
        };
        json!({"name": n, "type": kind, "authority": "shared"})
    };
    let fields: Vec<Value> = fields.split(' ').map(field).collect();
    json!({"declare": "type", "name": ty, "key": key, "fields": fields, "constraints": c})
}

/// The value type `name` of the texts `open` and `x`.
fn one_of(name: &str, open: &str) -> Value {
    json!({"declare": "value_type", "name": name, "kind": "text", "one_of": [open, "x"]})
}

/// The declaration `json` describes, made.
async fn declare(l: &Lake, json: Value) -> Held {
    l.declare_json(json).await.map(drop)
}

/// The declaration `json` describes, answered as it came.
async fn try_declare(l: &Lake, json: Value) -> Result<Answered, Failure> {
    let d = serde_json::from_value(json).map_err(|e| Failure::new(e.to_string()))?;
    Ok(l.cat.declare(&l.own, d).await)
}

/// What a declaration's JSON describes.
fn decl<T: serde::de::DeserializeOwned>(mut json: Value) -> Result<T, Failure> {
    json.as_object_mut().map(|o| o.remove("declare"));
    serde_json::from_value(json).map_err(|e| Failure::new(format!("it does not read: {e}")))
}

/// The report of a migration of a type to the declaration `json`.
async fn preview(l: &Lake, json: Value) -> Result<MigrationReport, Failure> {
    let to = Migration::to(decl(json)?);
    let report = l.cat.migrate(&l.own, to).await;
    Ok(report.must("previewing a migration")?.data)
}

/// The owner's upsert of `fields` at `key` of `ty`, landed.
async fn up(l: &Lake, ty: &str, key: &str, fields: Value) -> Held {
    l.upsert(ty, key, fields).await.map(drop)
}

/// The owner's proposal of an upsert of `fields` at `key` of `ty`.
async fn proposed(l: &Lake, ty: &str, key: &str, fields: Value) -> Result<Answered, Failure> {
    let w = json!({"write": "upsert", "type": ty, "key": key, "patch": fields});
    let w = serde_json::from_value(w).map_err(|e| Failure::new(e.to_string()))?;
    Ok(l.cat.propose(&l.own, w, "read".into()).await)
}

/// The owner's proposal of `fields` at `key` of `ty`, proposed.
async fn propose(l: &Lake, ty: &str, key: &str, fields: Value) -> Result<ProposalId, Failure> {
    match proposed(l, ty, key, fields).await? {
        Ok(Outcome::Proposed { proposal, .. }) => Ok(proposal),
        other => Err(Failure::new(format!("not proposed: {other:?}"))),
    }
}

/// A verb named `verb`, called on `key`.
async fn call(l: &Lake, verb: &str, key: &str) -> Answered {
    let (verb, key) = (verb.into(), RecordKey::from(key));
    let (none, opts) = (patch(json!({})), Default::default());
    l.cat.call(&l.own, &verb, Some(&key), none, opts).await
}

/// `open` renamed `shut` in `vt` by one masked from `datum` of `ty`; what it finds breaking.
async fn masked_rename(l: &Lake, s: &Store, ty: &str, vt: &str) -> Result<Vec<Breach>, Failure> {
    let mut masked = field_access(s.as_ref(), "masked", ty, &[("datum", Access::Masked)]);
    (masked.grants.schema, masked.grants.restricted) = (true, true);
    let to = decl(one_of(vt, "shut"))?;
    let renames = vec![("open".into(), "shut".into())];
    let m = ValueTypeMigration { to, renames };
    let walked = l.cat.migrate_value_type(&masked, m).await;
    let walked = walked.must("a value type's renames by one masked")?;
    Ok(walked.data.breaches)
}

async fn a_constraint_compares_the_record_with_the_write_s_clock(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    let (today, later) = days();
    let ahead = json!([{"name": "not_ahead", "holds": "datum <= today()"}]);
    let of = |ty: &str, c: &Value| typed(ty, &["no"], "no datum:date note", c.clone());
    declare(&l, of("reading", &ahead)).await?;
    // Refused anywhere but as a clock term compared with a field.
    for holds in [
        "today() > '2027-01-01' or datum <= today()",
        "now() > now()",
        "today() > datum - datum + '2027-01-01'",
    ] {
        let a = try_declare(&l, of("odd", &json!([{"name": "c", "holds": holds}]))).await?;
        ensure!(
            matches!(invalid_case(&a), Some(Case::ClockNotCompared { .. })),
            "`{holds}` is refused `clock_not_compared`: {a:?}"
        );
    }
    let verb = write_verb("ahead", "reading", "datum", &format!("'{later}'"));
    l.declare_as(verb, "a verb").await?;
    let soon = try_upsert(cat, &l.own, "reading", "R-1", json!({"datum": later})).await;
    up(&l, "reading", "R-2", json!({"datum": today})).await?;
    let fed = vec![source_record("reading", "R-3", json!({"datum": later}))];
    let fed = ingest(cat, &l.own, "meter", fed).await;
    let verb = call(&l, "ahead", "R-2").await;
    let prop = proposed(&l, "reading", "R-4", json!({"datum": later})).await?;
    let p = propose(&l, "reading", "R-5", json!({"datum": today})).await?;
    let accepted = cat.accept(&reviewer(store.as_ref(), false), &p).await;
    accepted.must("an accept of a reading dated today")?;
    // Dated ahead before the constraint: a write leaving `datum` is not judged.
    declare(&l, of("late", &json!([]))).await?;
    up(&l, "late", "L-1", json!({"datum": "2999-01-01"})).await?;
    let report = preview(&l, of("late", &ahead)).await?;
    l.migrate(decl(of("late", &ahead))?).await?;
    up(&l, "late", "L-1", json!({"note": "kept"})).await?;
    let moved = try_upsert(cat, &l.own, "late", "L-1", json!({"datum": "2998-01-01"})).await;
    let clean = report.breaches.is_empty() && report.lossless;
    ensure!(
        all_broken!("not_ahead", soon, fed, verb, prop, moved) && clean,
        "a write dated ahead is refused `not_ahead` at upsert, ingest, a verb and a proposal, \
         a migration adding it over a record dated ahead lists no breach, and a write that \
         moves `datum` is judged: {soon:?}, {fed:?}, {verb:?}, {prop:?}, {report:?}, {moved:?}"
    );
    // A walk that computes the field it reads again does not judge it.
    let day = |derived: &str| {
        json!({"declare": "type", "name": "walked", "key": ["no"],
            "fields": [{"name": "no", "type": {"kind": "text"}},
                       {"name": "datum", "type": {"kind": "date"}},
                       {"name": "due", "type": {"kind": "date_time"}, "authority": "derived",
                        "derived": derived, "depends_on": ["datum"]}],
            "constraints": [{"name": "due_past", "holds": "due <= now()"}]})
    };
    declare(&l, day("datum")).await?;
    up(&l, "walked", "W-1", json!({"datum": today})).await?;
    let again = try_declare(&l, day("datum + 36500d")).await?;
    again.must("a declaration that computes `due` again past the clock")?;
    Ok(())
}

async fn a_constraint_reads_the_record_before_it_in_its_series(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    let rule = "preceding(stand, datum, meter) is null or stand >= preceding(stand, datum, meter)";
    let lower = json!([{"name": "never_lower", "holds": rule}]);
    let fields = "meter datum:date stand:number hand note";
    let reading = typed("reading", &["meter", "datum"], fields, lower.clone());
    declare(&l, reading).await?;
    // Refused: matched on a number, ordered by JSON, read in a `find`.
    let odd = "meter:number datum:json stand:number";
    for (holds, matched) in [
        ("preceding(stand, stand, meter) is null", "meter"),
        ("preceding(stand, datum, stand) is null", "stand"),
    ] {
        let c = json!([{"name": "c", "holds": holds}]);
        let a = try_declare(&l, typed("odd", &["stand"], odd, c)).await?;
        let right = matches!(invalid_case(&a),
            Some(Case::PrecedingUnlooked { field, .. }) if field.as_str() == matched);
        ensure!(right, "`{holds}` matched on a number is refused: {a:?}");
    }
    let c = json!([{"name": "c", "holds": "preceding(stand, datum, meter) is null"}]);
    let by_json = typed("odd", &["meter"], "meter datum:json stand:number", c);
    let a = try_declare(&l, by_json).await?;
    let unordered = matches!(invalid_case(&a), Some(Case::PrecedingUnordered { field, .. }) if field.as_str() == "datum");
    let q = find_where("preceding(stand, datum, meter) > 0");
    let found = cat.find(&l.own, &"reading".into(), q).await;
    ensure!(
        unordered && matches!(invalid_case(&found), Some(Case::PrecedingHere)),
        "a series ordered by JSON is refused `preceding_unordered`, and a `preceding` in a \
         `find` `preceding_here`: {a:?}, {found:?}"
    );
    let own = &l.own;
    let put = |m: &str, d: &str, stand: i64| {
        let key = format!("{m}:{d}");
        let fields = json!({"meter": m, "datum": d, "stand": stand, "hand": "a"});
        async move { try_upsert(cat, own, "reading", &key, fields).await }
    };
    let ok = |m: &str, d: &str, stand: i64, what: &'static str| {
        let put = put(m, d, stand);
        async move { put.await.must(what).map(drop) }
    };
    ok("M1", "2026-01-01", 10, "the first reading").await?;
    let low = put("M1", "2026-02-01", 5).await;
    ok("M1", "2026-02-01", 20, "a higher reading").await?;
    ok("M2", "2026-03-01", 1, "another meter's").await?;
    ok("M1", "2026-01-15", 12, "a backdated one higher").await?;
    let backdated = put("M1", "2026-01-20", 11).await;
    // Above the next: a write of that one moving nothing read is not judged.
    ok("M1", "2026-01-25", 25, "one above the next").await?;
    up(&l, "reading", "M1:2026-02-01", json!({"note": "read"})).await?;
    let zero = write_verb("zero", "reading", "stand", "0");
    l.declare_as(zero, "a verb").await?;
    let called = call(&l, "zero", "M1:2026-02-01").await;
    let one = |d: &str, stand: i64| {
        let fields = json!({"meter": "M3", "datum": d, "stand": stand});
        source_record("reading", &format!("M3:{d}"), fields)
    };
    let two = vec![one("2026-01-01", 10), one("2026-01-02", 5)];
    let fed = ingest(cat, &l.own, "meter", two).await;
    // Proposed above the reading before, accepted after a higher one lands.
    let fields = json!({"meter": "M1", "datum": "2026-03-01", "stand": 25, "hand": "a"});
    let proposal = propose(&l, "reading", "M1:2026-03-01", fields).await?;
    ok("M1", "2026-02-15", 30, "a reading between").await?;
    let rev = reviewer(store.as_ref(), false);
    let accepted = cat.accept(&rev, &proposal).await;
    ensure!(
        all_broken!("never_lower", low, backdated, called, fed, accepted),
        "a reading below the one before it is refused `never_lower` at upsert, backdated, \
         at a verb, within one delivery and at an accept: {low:?}, {backdated:?}, \
         {called:?}, {fed:?}, {accepted:?}"
    );
    // A higher reading between that the reviewer is not answered: passed over.
    ok("M9", "2026-01-01", 10, "a reading of M9").await?;
    let fields = json!({"meter": "M9", "datum": "2026-02-01", "stand": 20, "hand": "a"});
    let proposal = propose(&l, "reading", "M9:2026-02-01", fields).await?;
    let unseen = json!({"meter": "M9", "datum": "2026-01-10", "stand": 60, "hand": "r"});
    up(&l, "reading", "M9:2026-01-10", unseen).await?;
    let r =
        json!({"default": "read", "review": true, "types": {"reading": {"only": {"hand": "a"}}}});
    let blind = super::only::holder(store.as_ref(), "blind", r);
    let past = cat.accept(&blind, &proposal).await;
    past.must("an accept past a reading its reviewer is not answered")?;
    // A writer `only` hand `b` passes hand `a`'s; the owner is judged by it.
    let grants = json!({"default": "write", "types": {"reading": {"only": {"hand": "b"}}}});
    let b = super::only::holder(store.as_ref(), "b", grants);
    let fields = json!({"meter": "M1", "datum": "2026-04-01", "stand": 40, "hand": "b"});
    let past = try_upsert(cat, &b, "reading", "M1:2026-04-01", fields).await;
    past.must("a reading below one its writer is not answered")?;
    let after = put("M1", "2026-04-02", 35).await;
    // A predecessor, hand `b`'s, whose `stand` is hidden from its writer.
    let hide = json!({"field": "hand", "values": ["b"], "fields": {"stand": "none"}});
    let grants = json!({"default": "write", "types": {"reading": {"hide_where": [hide]}}});
    let h = super::only::holder(store.as_ref(), "h", grants);
    let fields = json!({"meter": "M1", "datum": "2026-05-01", "stand": 99, "hand": "c"});
    let hidden = try_upsert(cat, &h, "reading", "M1:2026-05-01", fields).await;
    // A predecessor whose `datum` is hidden, before the write or after, is passed over.
    let gauge = "no meter datum:date stand:number hand";
    declare(&l, typed("gauge", &["no"], gauge, lower.clone())).await?;
    let hide = json!({"field": "hand", "values": ["z"], "fields": {"datum": "none"}});
    let grants = json!({"default": "write", "types": {"gauge": {"hide_where": [hide]}}});
    let g = super::only::holder(store.as_ref(), "g", grants);
    let mut twins = Vec::new();
    for (m, z) in [("G1", "2026-01-10"), ("G2", "2026-03-10")] {
        let y = json!({"meter": m, "datum": "2026-01-01", "stand": 50, "hand": "a"});
        up(&l, "gauge", &format!("{m}y"), y).await?;
        let z = json!({"meter": m, "datum": z, "stand": 60, "hand": "z"});
        up(&l, "gauge", &format!("{m}z"), z).await?;
        let w = json!({"meter": m, "datum": "2026-02-01", "stand": 10, "hand": "c"});
        twins.push(try_upsert(cat, &g, "gauge", &format!("{m}w"), w).await);
    }
    // A migration that adds it over a series that breaks it.
    let plain = "meter datum:date stand:number";
    declare(&l, typed("walked", &["meter", "datum"], plain, json!([]))).await?;
    up(&l, "walked", "M1:2026-01-01", json!({"stand": 10})).await?;
    up(&l, "walked", "M1:2026-01-02", json!({"stand": 5})).await?;
    let report = preview(&l, typed("walked", &["meter", "datum"], plain, lower)).await?;
    let clean = report.breaches.is_empty() && report.lossless;
    let not_granted = matches!(reason(&hidden), Some(Refusal::NotGranted));
    ensure!(
        all_broken!("never_lower", after, twins[0], twins[1]) && not_granted && clean,
        "a second writer is judged against the first's reading, a predecessor whose `stand` is \
         hidden makes the write not granted, one whose `datum` is hidden is passed over, and a \
         migration adding `never_lower` over a falling series lists no breach: {after:?}, \
         {hidden:?}, {twins:?}, {report:?}"
    );
    Ok(())
}

async fn a_walk_judges_nothing_only_a_write_judges(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    declare(&l, one_of("tide", "open")).await?;
    let rest = "datum:date stand:number phase:text=tide";
    let kept = |meter: &str, c: Value| typed("kept", &["no"], &format!("no {meter} {rest}"), c);
    let rules = |meter: &str| {
        let b = format!("preceding(stand, datum, {meter})");
        json!([{"name": "dated", "holds": "phase = 'x' or datum <= today()"},
               {"name": "lower", "holds": format!("phase = 'x' or {b} is null or stand >= {b}")}])
    };
    // Dated ahead and falling, put there before the constraints.
    declare(&l, kept("meter", json!([]))).await?;
    for (no, datum, stand) in [("K-1", "2999-01-01", 10), ("K-2", "2999-02-01", 5)] {
        let r = json!({"meter": "A", "datum": datum, "stand": stand, "phase": "open"});
        up(&l, "kept", no, r).await?;
    }
    l.migrate(decl(kept("meter", rules("meter")))?).await?;
    let walked = masked_rename(&l, &store, "kept", "tide").await?;
    let m = Migration {
        renames: vec![("meter".into(), "serial".into())],
        ..Migration::to(decl(kept("serial", rules("serial")))?)
    };
    let migrated = l.cat.migrate(&l.own, m).await;
    let migrated = migrated.must("a migration renaming `meter`")?.data;
    ensure!(
        walked.is_empty() && migrated.breaches.is_empty() && migrated.lossless,
        "a walk granted to one masked from `datum` and a migration under constraints only a \
         write judges list no breach: {walked:?}, {migrated:?}"
    );
    Ok(())
}

async fn a_walk_judges_what_reads_neither_clock_nor_preceding(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    declare(&l, one_of("ebb", "open")).await?;
    let fields = "no datum:date stand:number phase:text=ebb";
    let mixed = |c: Value| typed("mixed", &["no"], fields, c);
    declare(&l, mixed(json!([]))).await?;
    let r = json!({"datum": "2999-01-01", "stand": -1, "phase": "open"});
    up(&l, "mixed", "X-1", r).await?;
    // A migration judges `stand >= 0` of it.
    let rest = json!([{"name": "rest", "holds": "stand >= 0 and datum <= today()"}]);
    let report = preview(&l, mixed(rest)).await?;
    let listed = |b: &Breach| {
        let keys: Vec<&str> = b.sample_keys.iter().map(|k| k.as_str()).collect();
        b.constraint.as_str() == "rest" && b.records == 1 && keys == ["X-1"]
    };
    let lists = !report.lossless && matches!(&report.breaches[..], [b] if listed(b));
    // A walk in `phase` reads `phase` of it, not `datum`.
    let kept = json!([{"name": "kept", "holds": "phase != 'x' and datum <= today()"}]);
    l.migrate(decl(mixed(kept))?).await?;
    let walked = masked_rename(&l, &store, "mixed", "ebb").await?;
    ensure!(
        lists && walked.is_empty(),
        "a migration adding `stand >= 0 and datum <= today()` over X-1 lists it, and the \
         renames leave `phase != 'x'` held: {report:?}, {walked:?}"
    );
    Ok(())
}

async fn the_record_before_is_chosen_in_its_series(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    declare(&l, typed("site", &["id"], "id name", json!([]))).await?;
    for id in ["A", "B"] {
        up(&l, "site", id, json!({"name": id})).await?;
    }
    let before = "preceding(stand, at, line, site)";
    let rise = json!([{"name": "rise", "holds": format!("{before} is null or stand >= {before}")}]);
    let ticks = "no line site:reference=site at:number stand:number";
    declare(&l, typed("tick", &["no"], ticks, rise)).await?;
    let tick = |line: &str, site: &str, at: f64, stand: i64| json!({"line": line, "site": site, "at": at, "stand": stand});
    let put = |who: &Principal, no: &str, fields: Value| {
        let (who, no) = (who.clone(), no.to_string());
        async move { try_upsert(cat, &who, "tick", &no, fields).await }
    };
    let own = &l.own;
    // Tied at 1: T-1 comes first by key.
    put(own, "T-1", tick("L", "A", 1.0, 50)).await.must("T-1")?;
    put(own, "T-2", tick("L", "A", 1.0, 10)).await.must("T-2")?;
    let tied = put(own, "T-3", tick("L", "A", 2.0, 20)).await;
    let (t, k, sent) = ("tick".into(), "T-3".into(), patch(tick("L", "A", 2.0, 20)));
    let judged = store.upsert_preflight(own, Default::default(), &t, &k, sent, None);
    let judged = judged.await;
    // Another site's, another line's.
    for (no, line, site) in [("T-4", "L", "B"), ("T-5", "M", "A")] {
        put(own, no, tick(line, site, 2.0, 5)).await.must(no)?;
    }
    l.retire("tick", "T-1").await?;
    put(own, "T-3", tick("L", "A", 2.0, 20)).await.must("T-3")?;
    let moved = put(own, "T-4", json!({"site": "A"})).await;
    // T-6 restricted, above T-3.
    put(own, "T-6", tick("L", "A", 3.0, 90)).await.must("T-6")?;
    restrict(store.as_ref(), "tick", "T-6", "kept").await?;
    let plain = super::only::holder(store.as_ref(), "plain", json!({"default": "write"}));
    let past = put(&plain, "T-7", tick("L", "A", 4.0, 30)).await;
    past.must("past a restricted record its writer is not granted")?;
    let grants = json!({"default": "write", "restricted": true});
    let boss = super::only::holder(store.as_ref(), "boss", grants);
    let held = put(&boss, "T-8", tick("L", "A", 3.5, 25)).await;
    ensure!(
        all_broken!("rise", tied, judged, moved, held),
        "the first of a tie by key, `upsert_preflight`, a move into another series and one \
         granted `restricted` against a restricted record are judged: {tied:?}, {judged:?}, \
         {moved:?}, {held:?}"
    );
    Ok(())
}

/// The triggers fired after `at`, each with the key it fired on.
async fn fired(store: &dyn LakeStore, at: Position) -> Result<Vec<(String, String)>, Failure> {
    let mut fired = Vec::new();
    for e in events_after(store, at).await? {
        if let EventBody::TriggerFired { trigger, key, .. } = e.body {
            fired.push((trigger.to_string(), key.to_string()));
        }
    }
    Ok(fired)
}

async fn a_guard_reads_the_record_before_it_in_its_series(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let (cat, s) = (l.cat.as_ref(), store.as_ref());
    let fields = "no meter at:date_time stand:number hand";
    declare(&l, typed("reading", &["no"], fields, json!([]))).await?;
    let bucket = |f: &str| format!("date_bucket({f}, 1h, '2026-01-01T00:00:00Z')");
    let p = "preceding(at, at, meter)";
    let hour = format!("{p} is not null and {} != {}", bucket("at"), bucket(p));
    let low = format!("{p} is not null and preceding(stand, at, meter) is null");
    let trigger = |name: &str, on: &str, when: &str| json!({"declare": "trigger", "name": name, "on": on, "when": when});
    declare(&l, trigger("hour", "reading", &hour)).await?;
    declare(&l, trigger("low", "reading", &low)).await?;
    let reading = |m: &str, at: &str, hand: &str| json!({"meter": m, "at": format!("2026-01-01T{at}:00Z"), "stand": 5, "hand": hand});
    // The triggers a write of `m` at `at` by `who` fires.
    let fires = |who: &Principal, no: &str, m: &str, at: &str, hand: &str| {
        let (who, no, fields) = (who.clone(), no.to_string(), reading(m, at, hand));
        async move {
            let head = s.head().await.must("reading the head")?;
            let w = try_upsert(cat, &who, "reading", &no, fields).await;
            w.must("a reading")?;
            let fired = fired(s, head).await?.into_iter().map(|(t, _)| t);
            Ok::<_, Failure>(fired.collect::<Vec<_>>().join(" "))
        }
    };
    let own = &l.own;
    let mut got = Vec::new();
    for (no, m, at) in [
        ("A1", "M1", "14:10"),
        ("A2", "M1", "14:40"),
        ("A3", "M2", "15:05"),
        ("A4", "M1", "15:05"),
        ("A5", "M1", "14:20"),
        ("A6", "M1", "12:30"),
        ("A7", "M1", "13:30"),
    ] {
        got.push(fires(own, no, m, at, "a").await?);
    }
    // Before every reading of an hour logged; a level, again on another field.
    got.push(fires(own, "A8", "M1", "14:05", "a").await?);
    got.push(fires(own, "A4", "M1", "15:05", "x").await?);
    let head = s.head().await.must("reading the head")?;
    let one = |no: &str, at: &str| source_record("reading", no, reading("M3", at, "a"));
    let three = vec![one("C1", "09:10"), one("C3", "10:20"), one("C2", "10:05")];
    let done = ingest(cat, own, "meter", three).await;
    done.must("one delivery")?;
    let delivered = fired(s, head).await?;
    // A predecessor hand `b`'s writer is not answered; one whose `stand` is hidden.
    let grants = json!({"default": "write", "types": {"reading": {"only": {"hand": "b"}}}});
    let b = super::only::holder(s, "b", grants);
    fires(own, "B1", "M4", "10:10", "a").await?;
    got.push(fires(&b, "B2", "M4", "11:05", "b").await?);
    let hide = json!({"field": "hand", "values": ["z"], "fields": {"stand": "none"}});
    let grants = json!({"default": "write", "types": {"reading": {"hide_where": [hide]}}});
    let h = super::only::holder(s, "h", grants);
    fires(own, "H1", "M5", "10:00", "z").await?;
    got.push(fires(&h, "H2", "M5", "10:30", "c").await?);
    got.push(fires(own, "H3", "M5", "10:20", "c").await?);
    // An act takes back the `at` and `meter` another source wrote of Z1: the
    // firings that read them are overruled, and no other.
    fires(own, "Z0", "M9", "09:10", "a").await?;
    let z1 = source_record("reading", "Z1", reading("M9", "10:10", "a"));
    let done = ingest(cat, own, "other", vec![z1]).await;
    done.must("other's reading")?;
    let hand = try_upsert(cat, own, "reading", "Z1", json!({"hand": "x"})).await;
    hand.must("the owner's hand on Z1")?;
    let far = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    let act = s.remove_values(&"other".into(), &far, false, None).await;
    act.must("taking other's reading back")?;
    let filter = FeedFilter {
        kinds: vec!["trigger_fired".to_owned()],
        ..FeedFilter::default()
    };
    let polled = s.feed().poll(Position::GENESIS, &filter, 1024).await;
    let polled = polled.must("polling the feed")?;
    let overruled: Vec<_> = (polled.events.iter())
        .filter(|e| polled.overruled.contains(&e.position))
        .filter_map(|e| match &e.body {
            EventBody::TriggerFired { key, .. } => Some(key.to_string()),
            _ => None,
        })
        .collect();
    let want = [
        "", "", "", "hour", "", "", "hour", "hour", "hour", "", "low", "",
    ];
    let hour = |k: &str| ("hour".to_string(), k.to_string());
    let in_order = delivered == [hour("C3"), hour("C2")];
    ensure!(
        got == want && in_order && overruled == ["Z1", "Z1"],
        "firings by write {got:?}, in one delivery {delivered:?}, overruled {overruled:?}"
    );
    // Refused: matched on a number, over a view's rows.
    let on_number = trigger("t", "reading", "preceding(at, at, stand) is null");
    let a = try_declare(&l, on_number).await?;
    let unlooked = matches!(invalid_case(&a),
        Some(Case::PrecedingUnlooked { field }) if field.as_str() == "stand");
    let field = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    let view = json!({"declare": "type", "name": "hours", "key": ["meter"],
        "fields": [field("meter", "text"), field("n", "number")],
        "view": {"source": "reading", "group_by": ["meter"],
                 "measures": [{"name": "n", "expr": "count()"}]}});
    declare(&l, view).await?;
    l.set("view_kept.hours", json!(true)).await?;
    let on_view = try_declare(&l, trigger("v", "hours", "preceding(n, n, meter) is null")).await?;
    let here = matches!(invalid_case(&on_view), Some(Case::PrecedingHere));
    let number = typed(
        "reading",
        &["no"],
        &fields.replace("meter", "meter:number"),
        json!([]),
    );
    let moved = l.cat.migrate(own, Migration::to(decl(number)?)).await;
    let migrated = matches!(invalid_case(&moved),
        Some(Case::PrecedingUnlooked { field }) if field.as_str() == "meter");
    ensure!(
        unlooked && here && migrated,
        "a series matched on a number is refused `preceding_unlooked`, also by a migration, and \
         a trigger on a view `preceding_here`: {a:?}, {moved:?}, {on_view:?}"
    );
    Ok(())
}

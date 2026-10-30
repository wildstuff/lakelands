//! What a constraint reads that only its write can judge: the write's
//! clock and the record before it in a series
//! (`docs/decisions/059-a-type-declares-its-constraints.md`, extended;
//! lakelands#1552, lakelands#1556). Every write door judges it; no walk
//! judges it again.

use lakelands_api::write::Outcome;

use crate::conformance::fixture::{Answer, ingest, reviewer, source_record};
use crate::conformance::prelude::*;

check!(
    CLOCK_JUDGED_AT_THE_WRITE = a_constraint_compares_the_record_with_the_write_s_clock,
    "059, extended (lakelands#1556): a constraint `datum <= today()` reads the write's clock; a write dated ahead is refused `constraint_broken` at `upsert`, at `ingest`, at a verb's write and at a proposal, and one dated today lands, also at an accept; a write that does not move `datum` is not judged, a migration that adds the constraint over a record dated ahead lists no breach and is lossless, and a declaration that computes the field it reads again is not refused for it; `now()` or `today()` other than as a clock term on one side of a comparison whose other side reads the record, as in `today() > '2027-01-01' or datum <= today()` or `now() > now()`, is refused `clock_not_compared`"
);

check!(
    PRECEDING_JUDGED_AT_THE_WRITE = a_constraint_reads_the_record_before_it_in_its_series,
    "059 and 057, extended (lakelands#1552): `preceding(stand, datum, meter)` reads `stand` of the live record of the same `meter` whose `datum` is the greatest below this one's, null where none is; a reading lower than it is refused `constraint_broken` at `upsert`, at a verb's write, at `ingest`, where the second of one delivery is judged against the first, and at an accept, against the head then; the first reading, a higher one, another meter's and a backdated one higher than the reading before it land; a write that moves nothing it reads is not judged, though the reading before it is now higher; a predecessor its writer is not answered, or whose `datum` or `meter` is hidden from it, is passed over, a field it reads hidden on it makes the write `not_granted`, and a second writer is judged against the first's reading; a series matched on a number is refused `preceding_unlooked`, one ordered by a JSON value `preceding_unordered`, a `preceding` in a `find` `preceding_here`, and a migration that adds the constraint lists no breach"
);

/// Today's date, `YYYY-MM-DD`, and the day after the next, by the wall
/// clock: still ahead if the check runs past midnight.
fn days() -> (String, String) {
    const DAY: i64 = 86_400_000_000;
    let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    let micros = wall.map_or(0, |d| d.as_micros() as i64);
    let day = |n: i64| Timestamp((micros.div_euclid(DAY) + n) * DAY).day();
    (day(0).unwrap_or_default(), day(2).unwrap_or_default())
}

/// The constraint a refusal says is broken, if it says one is.
fn broken(a: &impl Answer) -> Option<String> {
    match reason(a) {
        Some(Refusal::Constraint(said)) => match &said.case {
            Case::ConstraintBroken { constraint, .. } => Some(constraint.to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// The type `ty` keyed by `key`, its fields of `kind`s, which every
/// door writes, its constraints `constraints`.
fn typed(ty: &str, key: &[&str], fields: &[(&str, &str)], constraints: Value) -> Value {
    let fields: Vec<Value> = (fields.iter())
        .map(|(n, k)| json!({"name": n, "type": {"kind": k}, "authority": "shared"}))
        .collect();
    json!({"declare": "type", "name": ty, "key": key, "fields": fields, "constraints": constraints})
}

/// The declaration `json` describes, answered as it came.
async fn try_declare(l: &Lake, json: Value) -> Result<lakelands_api::Result<Outcome>, Failure> {
    let d: Declaration = serde_json::from_value(json)
        .map_err(|e| Failure::new(format!("the check's declaration does not read: {e}")))?;
    Ok(l.cat.declare(&l.own, d).await)
}

/// The type a declaration's JSON describes.
fn decl(mut json: Value) -> Result<TypeDecl, Failure> {
    json.as_object_mut().map(|o| o.remove("declare"));
    serde_json::from_value(json).map_err(|e| Failure::new(format!("the type does not read: {e}")))
}

/// `who` proposes an upsert of `fields` at `key` of `ty`; its proposal.
async fn propose(l: &Lake, ty: &str, key: &str, fields: Value) -> Result<ProposalId, Failure> {
    let w = json!({"write": "upsert", "type": ty, "key": key, "patch": fields});
    let w = serde_json::from_value(w).map_err(|e| Failure::new(e.to_string()))?;
    match l.cat.propose(&l.own, w, "read".into()).await {
        Ok(Outcome::Proposed { proposal, .. }) => Ok(proposal),
        other => Err(Failure::new(format!(
            "a proposal must be proposed: {other:?}"
        ))),
    }
}

/// A verb on `ty` named `verb`, called on `key`.
async fn call(l: &Lake, verb: &str, key: &str) -> lakelands_api::Result<Outcome> {
    let key = RecordKey::from(key);
    let payload = crate::conformance::fixture::patch(json!({}));
    l.cat
        .call(
            &l.own,
            &verb.into(),
            Some(&key),
            payload,
            Default::default(),
        )
        .await
}

async fn a_constraint_compares_the_record_with_the_write_s_clock(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    let (today, later) = days();
    let fields = [("no", "text"), ("datum", "date"), ("note", "text")];
    let ahead = json!([{"name": "not_ahead", "holds": "datum <= today()"}]);
    l.declare_json(typed("reading", &["no"], &fields, ahead.clone()))
        .await?;
    // Refused anywhere but as a clock term compared with the record.
    for holds in [
        "today() > '2027-01-01' or datum <= today()",
        "now() > now()",
    ] {
        let c = json!([{"name": "c", "holds": holds}]);
        let a = try_declare(&l, typed("odd", &["no"], &fields, c)).await?;
        ensure!(
            matches!(invalid_case(&a), Some(Case::ClockNotCompared { .. })),
            "`{holds}` is refused `clock_not_compared`: {a:?}"
        );
    }
    l.declare_as(
        write_verb("ahead", "reading", "datum", &format!("'{later}'")),
        "a verb",
    )
    .await?;
    let soon = try_upsert(cat, &l.own, "reading", "R-1", json!({"datum": later})).await;
    l.upsert("reading", "R-2", json!({"datum": today})).await?;
    let fed = source_record("reading", "R-3", json!({"datum": later}));
    let fed = ingest(cat, &l.own, "meter", vec![fed]).await;
    let called = call(&l, "ahead", "R-2").await;
    let w = json!({"write": "upsert", "type": "reading", "key": "R-4", "patch": {"datum": later}});
    let w = serde_json::from_value(w).map_err(|e| Failure::new(e.to_string()))?;
    let proposed = cat.propose(&l.own, w, "ahead".into()).await;
    let said = [
        &broken(&soon),
        &broken(&fed),
        &broken(&called),
        &broken(&proposed),
    ];
    ensure!(
        said.iter().all(|c| c.as_deref() == Some("not_ahead")),
        "a write dated ahead is refused `not_ahead` at upsert, ingest, a verb and a \
         proposal: {soon:?}, {fed:?}, {called:?}, {proposed:?}"
    );
    let today_proposed = propose(&l, "reading", "R-5", json!({"datum": today})).await?;
    let accepted = cat
        .accept(&reviewer(store.as_ref(), false), &today_proposed)
        .await;
    accepted.must("an accept of a reading dated today")?;
    // A record dated ahead, put there before the constraint: a migration
    // adds it losslessly, and a write that leaves `datum` is not judged.
    let dated = [("no", "text"), ("datum", "date"), ("note", "text")];
    l.declare_json(typed("late", &["no"], &dated, json!([])))
        .await?;
    l.upsert("late", "L-1", json!({"datum": "2999-01-01"}))
        .await?;
    let to = Migration::to(decl(typed("late", &["no"], &dated, ahead.clone()))?);
    let preview = cat
        .migrate(&l.own, to)
        .await
        .must("previewing the migration")?;
    ensure!(
        preview.data.breaches.is_empty() && preview.data.lossless,
        "a migration adding `not_ahead` over a record dated ahead lists no breach: {preview:?}"
    );
    l.migrate(decl(typed("late", &["no"], &dated, ahead))?)
        .await?;
    l.upsert("late", "L-1", json!({"note": "kept"})).await?;
    let moved = try_upsert(cat, &l.own, "late", "L-1", json!({"datum": "2998-01-01"})).await;
    ensure!(
        broken(&moved).as_deref() == Some("not_ahead"),
        "a write that moves `datum` is judged: {moved:?}"
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
    l.declare_json(day("datum")).await?;
    l.upsert("walked", "W-1", json!({"datum": today})).await?;
    let again = try_declare(&l, day("datum + 36500d")).await?;
    again.must("a declaration that computes `due` again past the clock")?;
    Ok(())
}

async fn a_constraint_reads_the_record_before_it_in_its_series(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    let fields = [
        ("meter", "text"),
        ("datum", "date"),
        ("stand", "number"),
        ("hand", "text"),
        ("note", "text"),
    ];
    let rule = "preceding(stand, datum, meter) is null or stand >= preceding(stand, datum, meter)";
    let lower = json!([{"name": "never_lower", "holds": rule}]);
    l.declare_json(typed(
        "reading",
        &["meter", "datum"],
        &fields,
        lower.clone(),
    ))
    .await?;
    // Refused: matched on a number, ordered by JSON, read in a `find`.
    let odd = [("meter", "number"), ("datum", "json"), ("stand", "number")];
    for (holds, matched) in [
        ("preceding(stand, stand, meter) is null", "meter"),
        ("preceding(stand, datum, stand) is null", "stand"),
    ] {
        let c = json!([{"name": "c", "holds": holds}]);
        let a = try_declare(&l, typed("odd", &["stand"], &odd, c)).await?;
        let right = matches!(invalid_case(&a),
            Some(Case::PrecedingUnlooked { field, .. }) if field.as_str() == matched);
        ensure!(right, "`{holds}` matched on a number is refused: {a:?}");
    }
    let json_by = [("meter", "text"), ("datum", "json"), ("stand", "number")];
    let c = json!([{"name": "c", "holds": "preceding(stand, datum, meter) is null"}]);
    let a = try_declare(&l, typed("odd", &["meter"], &json_by, c)).await?;
    ensure!(
        matches!(invalid_case(&a), Some(Case::PrecedingUnordered { field, .. }) if field.as_str() == "datum"),
        "a series ordered by JSON is refused `preceding_unordered`: {a:?}"
    );
    let found = cat
        .find(
            &l.own,
            &"reading".into(),
            find_where("preceding(stand, datum, meter) > 0"),
        )
        .await;
    ensure!(
        matches!(invalid_case(&found), Some(Case::PrecedingHere)),
        "a `preceding` in a `find` is refused `preceding_here`: {found:?}"
    );
    let own = &l.own;
    let put = |m: &str, d: &str, stand: i64| {
        let key = format!("{m}:{d}");
        let fields = json!({"meter": m, "datum": d, "stand": stand, "hand": "a"});
        async move { try_upsert(cat, own, "reading", &key, fields).await }
    };
    put("M1", "2026-01-01", 10)
        .await
        .must("the first reading")?;
    let low = put("M1", "2026-02-01", 5).await;
    put("M1", "2026-02-01", 20).await.must("a higher reading")?;
    put("M2", "2026-03-01", 1).await.must("another meter's")?;
    put("M1", "2026-01-15", 12)
        .await
        .must("a backdated reading higher than the one before it")?;
    let backdated = put("M1", "2026-01-20", 11).await;
    // Put above the reading after it: a write of that one that moves
    // nothing the constraint reads is not judged.
    put("M1", "2026-01-25", 25)
        .await
        .must("a reading above the one after it")?;
    l.upsert("reading", "M1:2026-02-01", json!({"note": "read"}))
        .await?;
    l.declare_as(write_verb("zero", "reading", "stand", "0"), "a verb")
        .await?;
    let called = call(&l, "zero", "M1:2026-02-01").await;
    let one = |d: &str, stand: i64| {
        let fields = json!({"meter": "M3", "datum": d, "stand": stand});
        source_record("reading", &format!("M3:{d}"), fields)
    };
    let fed = ingest(
        cat,
        &l.own,
        "meter",
        vec![one("2026-01-01", 10), one("2026-01-02", 5)],
    )
    .await;
    // Proposed above the reading before it, accepted after a higher one
    // lands between.
    let fields = json!({"meter": "M1", "datum": "2026-03-01", "stand": 25, "hand": "a"});
    let proposal = propose(&l, "reading", "M1:2026-03-01", fields).await?;
    put("M1", "2026-02-15", 30)
        .await
        .must("a reading between")?;
    let accepted = cat
        .accept(&reviewer(store.as_ref(), false), &proposal)
        .await;
    let said = [
        broken(&low),
        broken(&backdated),
        broken(&called),
        broken(&fed),
        broken(&accepted),
    ];
    ensure!(
        said.iter().all(|c| c.as_deref() == Some("never_lower")),
        "a reading below the one before it is refused `never_lower` at upsert, backdated, \
         at a verb, within one delivery and at an accept: {said:?} {low:?}, {backdated:?}, \
         {called:?}, {fed:?}, {accepted:?}"
    );
    // A writer `only` hand `b` is not answered hand `a`'s readings; the
    // owner, after it, is judged against its reading.
    let b = super::only::holder(
        store.as_ref(),
        "b",
        json!({"default": "write", "types": {"reading": {"only": {"hand": "b"}}}}),
    );
    let fields = json!({"meter": "M1", "datum": "2026-04-01", "stand": 40, "hand": "b"});
    let past = try_upsert(cat, &b, "reading", "M1:2026-04-01", fields).await;
    past.must("a reading below one its writer is not answered")?;
    let after = put("M1", "2026-04-02", 35).await;
    ensure!(
        broken(&after).as_deref() == Some("never_lower"),
        "a second writer is judged against the first's reading: {after:?}"
    );
    // A predecessor, hand `b`'s, whose `stand` is hidden from its writer.
    let hide = json!({"field": "hand", "values": ["b"], "fields": {"stand": "none"}});
    let h = super::only::holder(
        store.as_ref(),
        "h",
        json!({"default": "write", "types": {"reading": {"hide_where": [hide]}}}),
    );
    let fields = json!({"meter": "M1", "datum": "2026-05-01", "stand": 99, "hand": "c"});
    let hidden = try_upsert(cat, &h, "reading", "M1:2026-05-01", fields).await;
    ensure!(
        matches!(reason(&hidden), Some(Refusal::NotGranted)),
        "a predecessor whose `stand` is hidden makes the write not granted: {hidden:?}"
    );
    // Twins but for a hidden `datum`, below the write's or above it: each
    // is passed over, and both writes are judged against the reading
    // before them that their writer reads.
    let gauge = [
        ("no", "text"),
        ("meter", "text"),
        ("datum", "date"),
        ("stand", "number"),
        ("hand", "text"),
    ];
    l.declare_json(typed("gauge", &["no"], &gauge, lower.clone()))
        .await?;
    let hide = json!({"field": "hand", "values": ["z"], "fields": {"datum": "none"}});
    let g = super::only::holder(
        store.as_ref(),
        "g",
        json!({"default": "write", "types": {"gauge": {"hide_where": [hide]}}}),
    );
    let mut twins = Vec::new();
    for (m, z) in [("G1", "2026-01-10"), ("G2", "2026-03-10")] {
        let y = json!({"meter": m, "datum": "2026-01-01", "stand": 50, "hand": "a"});
        l.upsert("gauge", &format!("{m}y"), y).await?;
        let z = json!({"meter": m, "datum": z, "stand": 60, "hand": "z"});
        l.upsert("gauge", &format!("{m}z"), z).await?;
        let w = json!({"meter": m, "datum": "2026-02-01", "stand": 10, "hand": "c"});
        twins.push(try_upsert(cat, &g, "gauge", &format!("{m}w"), w).await);
    }
    ensure!(
        twins
            .iter()
            .all(|t| broken(t).as_deref() == Some("never_lower")),
        "a predecessor whose `datum` is hidden is passed over, wherever it lies: {twins:?}"
    );
    // A migration that adds it over a series that breaks it.
    let plain = [("meter", "text"), ("datum", "date"), ("stand", "number")];
    l.declare_json(typed("walked", &["meter", "datum"], &plain, json!([])))
        .await?;
    l.upsert("walked", "M1:2026-01-01", json!({"stand": 10}))
        .await?;
    l.upsert("walked", "M1:2026-01-02", json!({"stand": 5}))
        .await?;
    let to = Migration::to(decl(typed("walked", &["meter", "datum"], &plain, lower))?);
    let preview = cat
        .migrate(&l.own, to)
        .await
        .must("previewing the migration")?;
    ensure!(
        preview.data.breaches.is_empty() && preview.data.lossless,
        "a migration adding `never_lower` over a falling series lists no breach: {preview:?}"
    );
    Ok(())
}

//! The lake's calendar day: `today()` at a write is the day of the lake's
//! zone holding the write's clock, the zone its `zone` setting names in
//! force where the write is made, `UTC` where it names none
//! (`docs/decisions/101-a-lake-keeps-its-calendar-in-one-named-zone.md`;
//! lakelands#1665). Each row moves the write's clock below the store, as
//! the feed's rows do: a `lease_taken` known at the instant wanted, after
//! which every append reads that `known_at`.

use crate::conformance::Lines;
use crate::conformance::fixture::{canonical_line, try_upsert};
use crate::conformance::prelude::*;

check!(
    ZONE_CUTS_TODAY = today_is_the_day_of_the_lake_s_zone,
    "101 (lakelands#1665): with the lake's `zone` `Europe/Berlin` and the write's clock at 2030-03-14T23:30Z, 00:30 of 2030-03-15 in Berlin, a constraint `datum <= today()` admits a reading dated 2030-03-15 and refuses one dated 2030-03-16 `constraint_broken`, and a field computed at the write as `today()` holds 2030-03-15; a `zone` the time-zone database does not name is refused `setting_zone`",
    given
);

check!(
    ZONE_ACROSS_THE_CHANGE = today_follows_the_zone_across_its_clock_change,
    "101 (lakelands#1665): in `Europe/Berlin` on the night its clocks go back, the write's clock at 2030-10-26T22:30Z, 00:30 summer time of 2030-10-27, admits a reading dated 2030-10-27 and refuses one dated 2030-10-28, and at 2030-10-27T22:30Z, 23:30 winter time of the same day, still admits 2030-10-27 and refuses 2030-10-28: the day is the zone's, not a fixed offset's",
    given
);

check!(
    ZONE_IN_FORCE_AT_THE_WRITE = each_write_is_judged_in_the_zone_in_force_where_it_was_made,
    "101 (lakelands#1665): the write's clock at 2030-03-14T23:30Z, a reading dated 2030-03-15 is admitted under `Europe/Berlin`; the `zone` set to `America/New_York` after it, one dated 2030-03-15 is refused and one dated 2030-03-14 admitted; the field computed at the write as `today()` holds 2030-03-15 and 2030-03-14, each its own zone's day, and after a rebuild the lake answers both as before, its chain verified alike",
    given
);

check!(
    ZONE_UNSET_IS_UTC = a_lake_naming_no_zone_reads_its_day_in_utc,
    "101 (lakelands#1665): a lake whose `zone` is not set, or was set and retired, reads `today()` in UTC: the write's clock at 2030-03-14T23:30Z, a reading dated 2030-03-15 is refused and one dated 2030-03-14 admitted, and the field computed at the write as `today()` holds 2030-03-14",
    given
);

check!(
    ZONE_AGING_IS_THE_DATE_HANDED = aging_reads_the_date_handed_west_of_utc_too,
    "101 (lakelands#1665): in a lake whose `zone` is `America/New_York`, `age` handed 2030-03-15 reads `today()` in a rule's `when` as 2030-03-15, the date handed, not cut again in the zone, where it would be 2030-03-14: a rule `due <= today()` moves a bill due 2030-03-15 and leaves one due 2030-03-16, and the report names 2030-03-15",
    given
);

check!(
    ZONE_AT_THE_READ = a_read_cuts_today_in_the_zone_in_force_at_its_position,
    "101 (lakelands#1665): a field computed at the read as `today()`, read `valid_at` 2030-03-14T23:30Z, holds 2030-03-15 as of a position where the lake's `zone` is `Europe/Berlin` and 2030-03-14 at the head after it is set to `America/New_York`; a `find` whose `where` is `datum = today()` answers the reading dated each day at each",
    given
);

/// The probe's instant: 00:30 of 2030-03-15 in Berlin.
const PROBE: &str = "2030-03-14T23:30:00Z";

/// The readings each row writes: a date that may not be ahead of the
/// write's day, and that day, computed at the write.
fn reading() -> Value {
    json!({"declare": "type", "name": "reading", "key": ["no"], "fields": [
        {"name": "no", "type": {"kind": "text"}},
        {"name": "datum", "type": {"kind": "date"}},
        {"name": "day", "type": {"kind": "date"}, "authority": "derived",
         "derived": "today()", "computed_at": "write"},
    ], "constraints": [{"name": "not_ahead", "holds": "datum <= today()"}]})
}

/// The lake opened again for a write, its clock at `at`: a `lease_taken`
/// known at `at` appended below the store, so every append after it reads
/// that `known_at` while the wall clock is behind it.
async fn clocked(store: Store, given: &Given, at: &str) -> Result<Store, Failure> {
    use lakelands_api::engine::OpenMode;
    use lakelands_api::event::EventId;
    let at = Timestamp::parse(at).map_err(|e| Failure::new(format!("{at}: {e}")))?;
    let lake = store.lake().clone();
    let events = log_events(store.as_ref()).await?;
    let taken = |e: &&Event| matches!(e.body, EventBody::LeaseTaken { .. });
    let Some(template) = events.iter().find(taken).cloned() else {
        return Err(Failure::new(
            "the lake's log holds its writer's `lease_taken`",
        ));
    };
    let line: Lines = Box::new(move |(tip, prev), fence| {
        let (position, holder) = (tip.next(), "a writer ahead".into());
        let event = Event {
            position,
            id: EventId(format!("{:032x}", position.0)),
            prev,
            known_at: at,
            fence,
            body: EventBody::LeaseTaken { fence, holder },
            ..template
        };
        (vec![canonical_line(event)], 0)
    });
    let c = (given.continued)(Some(store), lake, line, OpenMode::Write, Seams::default());
    let c = c
        .await
        .map_err(|e| Failure::new(format!("moving the clock to {at} below the store: {e}")))?;
    c.opened
        .map_err(|e| Failure::new(format!("opening the lake for a write at {at}: {e}")))
}

/// The owner's reading `key` dated `datum`: the day it was written on, or
/// the constraint that refused it.
async fn write(l: &Lake, key: &str, datum: &str) -> Result<Result<String, String>, Failure> {
    let a = try_upsert(
        l.cat.as_ref(),
        &l.own,
        "reading",
        key,
        json!({"datum": datum}),
    )
    .await;
    match (&a, reason(&a)) {
        (Ok(_), _) => Ok(Ok(day_of(l, key).await?)),
        (_, Some(Refusal::Constraint(said))) => match &said.case {
            Case::ConstraintBroken { constraint, .. } => Ok(Err(constraint.to_string())),
            other => Err(Failure::new(format!("{key}: refused {other:?}"))),
        },
        _ => Err(Failure::new(format!("{key} dated {datum}: {}", said(&a)))),
    }
}

/// The day the reading `key` holds as written.
async fn day_of(l: &Lake, key: &str) -> Result<String, Failure> {
    let r = l.read("reading", key).await?;
    Ok(r.fields
        .get("day")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned())
}

/// What a write answered, its error as the store says it.
fn said<T: std::fmt::Debug>(a: &Result<T, Error>) -> String {
    match a {
        Ok(v) => format!("{v:?}"),
        Err(e) => e.to_string(),
    }
}

/// Admitted on `day`.
fn on(day: &str) -> Result<String, String> {
    Ok(day.to_owned())
}

/// Refused as ahead of the write's day.
fn ahead() -> Result<String, String> {
    Err("not_ahead".to_owned())
}

async fn today_is_the_day_of_the_lake_s_zone(store: Store, given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    let setting = lakelands_api::seam::SETTING_TYPE;
    let mars = json!({"key": "zone", "value": "Mars/Olympus_Mons"});
    let a = try_upsert(l.cat.as_ref(), &l.own, setting, "zone", mars).await;
    ensure!(
        matches!(invalid_case(&a), Some(Case::SettingZone { .. })),
        "a zone the database does not name is refused `setting_zone`: {}",
        said(&a)
    );
    l.set("zone", json!("Europe/Berlin")).await?;
    l.declare_json(reading()).await?;
    let store = clocked(store, &given, PROBE).await?;
    let l = Lake::of(store.as_ref());
    let (today, tomorrow) = (
        write(&l, "R-1", "2030-03-15").await?,
        write(&l, "R-2", "2030-03-16").await?,
    );
    ensure!(
        today == on("2030-03-15") && tomorrow == ahead(),
        "at 00:30 of 2030-03-15 in Berlin the local today is admitted and written on it, and \
         the local tomorrow refused: {today:?}, {tomorrow:?}"
    );
    Ok(())
}

async fn today_follows_the_zone_across_its_clock_change(store: Store, given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    l.set("zone", json!("Europe/Berlin")).await?;
    l.declare_json(reading()).await?;
    let mut seen = Vec::new();
    for (i, at) in ["2030-10-26T22:30:00Z", "2030-10-27T22:30:00Z"]
        .into_iter()
        .enumerate()
    {
        let store = clocked(store.clone(), &given, at).await?;
        let l = Lake::of(store.as_ref());
        let today = write(&l, &format!("D-{i}"), "2030-10-27").await?;
        let tomorrow = write(&l, &format!("N-{i}"), "2030-10-28").await?;
        seen.push((at, today, tomorrow));
    }
    let held =
        |(_, today, tomorrow): &(&str, _, _)| *today == on("2030-10-27") && *tomorrow == ahead();
    ensure!(
        seen.iter().all(held),
        "on the night Berlin's clocks go back, 00:30 summer time and 23:30 winter time are \
         both 2030-10-27: that day is admitted and the next refused at each: {seen:?}"
    );
    Ok(())
}

async fn each_write_is_judged_in_the_zone_in_force_where_it_was_made(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.set("zone", json!("Europe/Berlin")).await?;
    l.declare_json(reading()).await?;
    let store = clocked(store, &given, PROBE).await?;
    let l = Lake::of(store.as_ref());
    let berlin = write(&l, "R-1", "2030-03-15").await?;
    l.set("zone", json!("America/New_York")).await?;
    let new_york = (
        write(&l, "R-2", "2030-03-15").await?,
        write(&l, "R-3", "2030-03-14").await?,
    );
    ensure!(
        berlin == on("2030-03-15") && new_york == (ahead(), on("2030-03-14")),
        "each write is judged in the zone in force where it is made: 2030-03-15 in Berlin, \
         2030-03-14 in New York: {berlin:?}, {new_york:?}"
    );
    let before = store.verify(None, None).await.must("verifying the chain")?;
    store.rebuild().await.must("rebuilding the projection")?;
    let after = store.verify(None, None);
    let after = after.await.must("verifying the chain after the rebuild")?;
    let days = (day_of(&l, "R-1").await?, day_of(&l, "R-3").await?);
    let absent = l.cat.get(&l.own, &"reading".into(), get_at("R-2")).await;
    ensure!(
        before == after
            && days == ("2030-03-15".into(), "2030-03-14".into())
            && matches!(absent, Err(Error::NotFound { .. })),
        "a rebuild folds each write in the zone in force where it was made, and keeps out the \
         one refused: the chain {before:?} and {after:?}, the days {days:?}, `R-2` {absent:?}"
    );
    Ok(())
}

async fn a_lake_naming_no_zone_reads_its_day_in_utc(store: Store, given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(reading()).await?;
    let store = clocked(store, &given, PROBE).await?;
    let l = Lake::of(store.as_ref());
    let unset = (
        write(&l, "R-1", "2030-03-15").await?,
        write(&l, "R-2", "2030-03-14").await?,
    );
    l.set("zone", json!("Europe/Berlin")).await?;
    let zone = (
        TypeName::from(lakelands_api::seam::SETTING_TYPE),
        RecordKey::from("zone"),
    );
    let retired = l
        .cat
        .retire(&l.own, &zone.0, &zone.1, Default::default())
        .await;
    retired.must("retiring the `zone` setting")?;
    let retired = (
        write(&l, "R-3", "2030-03-15").await?,
        write(&l, "R-4", "2030-03-14").await?,
    );
    let utc = (ahead(), on("2030-03-14"));
    ensure!(
        unset == utc && retired == utc,
        "with no zone, or one retired, the day is UTC's, 2030-03-14: {unset:?}, {retired:?}"
    );
    Ok(())
}

/// A `get` of `key` at the head.
fn get_at(key: &str) -> lakelands_api::Get {
    lakelands_api::Get {
        key: key.into(),
        with_meta: false,
        at: ReadAt::default(),
        params: Default::default(),
    }
}

async fn aging_reads_the_date_handed_west_of_utc_too(store: Store, _given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    l.set("zone", json!("America/New_York")).await?;
    l.declare_json(
        json!({"declare": "type", "name": "bill", "key": ["no"], "fields": [
            {"name": "no", "type": {"kind": "text"}},
            {"name": "due", "type": {"kind": "date"}},
            {"name": "state", "type": {"kind": "text"}},
        ]}),
    )
    .await?;
    for (no, due) in [("B-1", "2030-03-15"), ("B-2", "2030-03-16")] {
        l.upsert("bill", no, json!({"due": due, "state": "open"}))
            .await?;
    }
    let rule = json!({"on": "bill", "field": "state", "from": "open", "to": "due",
                      "when": "due <= today()"});
    let aging_rule = lakelands_api::seam::AGING_RULE_TYPE;
    l.upsert(aging_rule, "falls_due", rule).await?;
    let handed = Timestamp::from_date(2030, 3, 15).expect("a date of the calendar");
    let report = store.age(handed).await.must("aging 2030-03-15")?;
    let mut states = Vec::new();
    for no in ["B-1", "B-2"] {
        let r = l.read("bill", no).await?;
        states.push(r.fields.get("state").cloned().unwrap_or(Value::Null));
    }
    ensure!(
        report.date == handed && report.transitions == 1 && states == [json!("due"), json!("open")],
        "aged for 2030-03-15 in New York, `today()` in the rule is the date handed: the bill \
         due that day moves and the next day's stays: {:?} on {}, moved {}",
        states,
        report.date,
        report.transitions
    );
    Ok(())
}

async fn a_read_cuts_today_in_the_zone_in_force_at_its_position(
    store: Store,
    _given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.set("zone", json!("Europe/Berlin")).await?;
    l.declare_json(
        json!({"declare": "type", "name": "reading", "key": ["no"], "fields": [
            {"name": "no", "type": {"kind": "text"}},
            {"name": "datum", "type": {"kind": "date"}},
            {"name": "day", "type": {"kind": "date"}, "authority": "derived",
             "derived": "today()", "computed_at": "read"},
        ]}),
    )
    .await?;
    for (no, datum) in [("R-14", "2030-03-14"), ("R-15", "2030-03-15")] {
        l.upsert("reading", no, json!({"datum": datum})).await?;
    }
    let berlin = store.head().await.must("the head")?;
    l.set("zone", json!("America/New_York")).await?;
    let probe = Timestamp::parse(PROBE).map_err(|e| Failure::new(format!("{PROBE}: {e}")))?;
    let mut seen = Vec::new();
    for as_of in [Some(berlin), None] {
        let at = ReadAt {
            as_of,
            valid_at: Some(probe),
            clock: None,
        };
        let get = lakelands_api::Get {
            at,
            ..get_at("R-14")
        };
        let got = l.cat.get(&l.own, &"reading".into(), get).await;
        let got = got.must("reading `R-14`")?;
        let day = got.data.fields.get("day").cloned().unwrap_or(Value::Null);
        let find = lakelands_api::Find {
            filter: Some("datum = today()".into()),
            at,
            ..Default::default()
        };
        let found = l.cat.find(&l.own, &"reading".into(), find).await;
        let found = found.must("finding the reading dated today")?;
        let keys: Vec<String> = match &found.data {
            lakelands_api::Found::Records { rows, .. } => {
                rows.iter().map(|r| r.key.to_string()).collect()
            }
            other => return Err(Failure::new(format!("a page of records: {other:?}"))),
        };
        seen.push((day, keys));
    }
    let want = vec![
        (json!("2030-03-15"), vec!["R-15".to_owned()]),
        (json!("2030-03-14"), vec!["R-14".to_owned()]),
    ];
    ensure!(
        seen == want,
        "at 2030-03-14T23:30Z a read cuts the day in the zone in force at its position: \
         Berlin's 2030-03-15 as of the position before the change, New York's 2030-03-14 at \
         the head: {seen:?}"
    );
    Ok(())
}

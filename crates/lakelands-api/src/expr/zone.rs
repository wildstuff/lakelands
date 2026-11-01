//! The local calendar a `date_bucket` unit is cut in when it names a time
//! zone, as in `date_bucket(t, 'day', 'Europe/Berlin')`, and the lake's
//! calendar day, which `today()` and aging read in the lake's `zone`
//! (`docs/decisions/101-a-lake-keeps-its-calendar-in-one-named-zone.md`).
//!
//! A zone is named as the IANA time-zone database names it, and only that
//! database's copy built into Lakelands is read: never the host's, nor a
//! zone the environment names, so a bucket starts at the same instant on
//! every engine and on every host. A local day starts at its local
//! midnight, or where a change of offset first leaves one, so it is 23 or
//! 25 hours long on the days the offset changes.

use std::sync::OnceLock;

use jiff::civil::Date;
use jiff::tz::{TimeZone, TimeZoneDatabase};

// INTERIM: lakelands#1667 — the database is the copy jiff-tzdb 0.1.9
// builds in, pinned exactly; a lake does not record which copy cut its
// kept zoned buckets, so a new copy with new rules would leave kept rows
// cut by the old ones. Recording the copy in the lake, or re-folding kept
// zoned views when it changes, is still to be decided.

/// The built-in database; it keeps every zone it has read.
fn database() -> &'static TimeZoneDatabase {
    static DATABASE: OnceLock<TimeZoneDatabase> = OnceLock::new();
    DATABASE.get_or_init(TimeZoneDatabase::bundled)
}

/// The zone of that name, if the database names it.
fn zone(name: &str) -> Option<TimeZone> {
    database().get(name).ok()
}

/// Whether the database names a zone `name`.
pub fn known(name: &str) -> bool {
    zone(name).is_some()
}

/// The instant, in microseconds since 1970 in UTC, at which the local
/// `unit` of `zone` holding the instant `t` starts: the local day's start,
/// the local Monday's, or the local first of the month, of the quarter's
/// first month, or of January. `None` for a zone or unit it does not know,
/// and for an instant outside the calendar.
pub fn local_start(t: i64, unit: &str, zone_name: &str) -> Option<i64> {
    let tz = zone(zone_name)?;
    let local = jiff::Timestamp::from_microsecond(t)
        .ok()?
        .to_zoned(tz.clone());
    start_of(local.date(), unit, tz)
}

/// The zone a lake reads its calendar day in where it names none.
pub const UTC: &str = "UTC";

/// The day of `zone`'s calendar holding the instant `t`, as a date is
/// read: that date's UTC midnight, in microseconds since 1970. The day
/// starts at local midnight, so it is 23 or 25 hours long where the offset
/// changes. A zone the database does not name, which only a log written
/// under another copy of the database can hold, and an instant outside
/// the calendar are cut in UTC.
pub fn day(t: i64, zone_name: &str) -> i64 {
    const DAY: i64 = 86_400_000_000;
    local_date(t, zone_name).unwrap_or_else(|| t.div_euclid(DAY) * DAY)
}

/// [`day`], where the zone and the instant are known.
fn local_date(t: i64, zone_name: &str) -> Option<i64> {
    let local = jiff::Timestamp::from_microsecond(t)
        .ok()?
        .to_zoned(zone(zone_name)?);
    let midnight = local.date().to_zoned(TimeZone::UTC).ok()?;
    Some(midnight.timestamp().as_microsecond())
}

/// As [`local_start`], for a `date` rather than an instant: the date is
/// that same date in the zone's calendar, not the instant its UTC midnight
/// names, so `2030-07-15` is a day of July 15th in every zone. `t` is the
/// date's UTC midnight, as a date is read.
pub fn date_start(t: i64, unit: &str, zone_name: &str) -> Option<i64> {
    let tz = zone(zone_name)?;
    let utc = jiff::Timestamp::from_microsecond(t)
        .ok()?
        .to_zoned(TimeZone::UTC);
    start_of(utc.date(), unit, tz)
}

/// The instant the local `unit` holding the local `day` starts in `tz`.
fn start_of(day: Date, unit: &str, tz: TimeZone) -> Option<i64> {
    let first = |month: i8| Date::new(day.year(), month, 1).ok();
    let starts: Date = match unit {
        "day" => day,
        "week" => day
            .checked_sub(jiff::Span::new().days(day.weekday().to_monday_zero_offset()))
            .ok()?,
        "month" => day.first_of_month(),
        "quarter" => first((day.month() - 1) / 3 * 3 + 1)?,
        "year" => first(1)?,
        _ => return None,
    };
    let start = starts.to_zoned(tz).ok()?.start_of_day().ok()?;
    Some(start.timestamp().as_microsecond())
}

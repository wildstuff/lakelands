//! The local calendar a `date_bucket` unit is cut in when it names a time
//! zone, as in `date_bucket(t, 'day', 'Europe/Berlin')`.
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

// INTERIM: lakelands#1667 — a lake records the copy that cut its kept
// zoned views, and its writer folds those again on open under another
// ([`copy`]); a derived field a zone cuts is cut again only by a rebuild,
// which cuts it under the binary's copy, so a rebuild under a new copy
// with new rules may answer it otherwise than the fold that first cut it.

/// The name the IANA gives the release of the database built in, such as
/// `2026e`: what a lake records of the copy that cut a kept view's local
/// buckets, so that a writer under another folds the view again (098).
pub fn copy() -> &'static str {
    jiff_tzdb::VERSION.unwrap_or("unnamed")
}

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

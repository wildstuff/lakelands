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
/// `unit` of `zone` holding `t` starts: the local day's start, the local
/// Monday's, or the local first of the month, of the quarter's first
/// month, or of January. `None` for a zone or unit it does not know, and
/// for an instant outside the calendar.
pub fn local_start(t: i64, unit: &str, zone_name: &str) -> Option<i64> {
    let tz = zone(zone_name)?;
    let local = jiff::Timestamp::from_microsecond(t)
        .ok()?
        .to_zoned(tz.clone());
    let day = local.date();
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

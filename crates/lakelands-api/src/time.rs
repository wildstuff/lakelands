//! Wall time and business time.
//!
//! A [`Timestamp`] is a UTC instant with microsecond precision. It is
//! used for `known_at` and `seen_at` (when the lake and its source
//! learned a value) and for `valid_at`, `valid_from` and `valid_to`
//! (when a fact held in the world). None of them orders the log; only a
//! [`Position`](crate::Position) does.
//!
//! On the wire and in the log a timestamp is written in exactly one form,
//! `YYYY-MM-DDTHH:MM:SS.ffffffZ`, so that its bytes are canonical.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A UTC instant, in microseconds since 1970-01-01T00:00:00Z.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

/// Why a string is not a canonical-enough timestamp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimestampError(pub String);

impl fmt::Display for TimestampError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a UTC timestamp: {}", self.0)
    }
}

impl std::error::Error for TimestampError {}

const MICROS_PER_SEC: i64 = 1_000_000;
const SECS_PER_DAY: i64 = 86_400;

// Days since 1970-01-01 for a proleptic Gregorian civil date, and back.
// The algorithm is the well-known era-based conversion; it is exact over
// the whole i64 range this type can reach.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

impl Timestamp {
    /// The civil date, in UTC, the instant falls on: year, month, day.
    pub fn date(self) -> (i64, u32, u32) {
        let (y, m, d) = civil_from_days(self.0.div_euclid(MICROS_PER_SEC * SECS_PER_DAY));
        (y, m as u32, d as u32)
    }

    /// Midnight UTC at the start of a civil date.
    pub fn from_date(year: i64, month: u32, day: u32) -> Option<Timestamp> {
        let (m, d) = (i64::from(month), i64::from(day));
        if !(1..=12).contains(&m) || d < 1 || d > days_in_month(year, m) {
            return None;
        }
        Some(Timestamp(
            days_from_civil(year, m, d) * SECS_PER_DAY * MICROS_PER_SEC,
        ))
    }

    /// The calendar date of an instant at a UTC midnight, `YYYY-MM-DD`:
    /// the form a `date` field keeps. Any other instant has none.
    pub fn day(self) -> Option<String> {
        let (y, m, d) = self.date();
        let midnight = self.0.rem_euclid(MICROS_PER_SEC * SECS_PER_DAY) == 0;
        midnight.then(|| format!("{y:04}-{m:02}-{d:02}"))
    }

    /// Parses `YYYY-MM-DD`, or RFC 3339 with `Z` or a numeric offset and
    /// up to six fractional digits. A date alone means midnight UTC. An
    /// offset that carries the instant out of the years 0000 to 9999 in
    /// UTC is refused: the one form has four digits for the year, and what
    /// is parsed here is written in it and read again.
    pub fn parse(s: &str) -> Result<Timestamp, TimestampError> {
        let t = Timestamp::denoted(s)?;
        (t.in_years())
            .then_some(t)
            .ok_or_else(|| TimestampError(s.to_owned()))
    }

    /// Whether the instant lies in the years 0000 to 9999 in UTC, which
    /// the one form writes. `parse` answers no other; an instant built
    /// from its microseconds, or computed, can be any, and one that is
    /// written is asked this first.
    pub fn in_years(self) -> bool {
        (0..=9999).contains(&self.date().0)
    }

    /// The instant a text denotes, in the forms `parse` reads and in
    /// whatever year its offset leaves it. For a text that is only
    /// compared or computed with, which names its instant wherever that
    /// falls; a value that is written is read by `parse`.
    pub fn denoted(s: &str) -> Result<Timestamp, TimestampError> {
        let err = || TimestampError(s.to_owned());
        let b = s.as_bytes();
        let num = |r: std::ops::Range<usize>| -> Result<i64, TimestampError> {
            let t = s.get(r).ok_or_else(err)?;
            if t.is_empty() || !t.bytes().all(|c| c.is_ascii_digit()) {
                return Err(err());
            }
            t.parse().map_err(|_| err())
        };
        if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
            return Err(err());
        }
        let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
        let date = Timestamp::from_date(y, mo as u32, d as u32).ok_or_else(err)?;
        if b.len() == 10 {
            return Ok(date);
        }
        if !matches!(b[10], b'T' | b't' | b' ') || b.len() < 19 || b[13] != b':' || b[16] != b':' {
            return Err(err());
        }
        let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
        if h > 23 || mi > 59 || se > 59 {
            return Err(err());
        }
        let mut i = 19;
        let mut micros = 0i64;
        if b.get(i) == Some(&b'.') {
            let start = i + 1;
            let mut end = start;
            while end < b.len() && b[end].is_ascii_digit() {
                end += 1;
            }
            let digits = end - start;
            if digits == 0 || digits > 6 {
                return Err(err());
            }
            micros = num(start..end)? * 10i64.pow((6 - digits) as u32);
            i = end;
        }
        let offset_secs = match b.get(i) {
            Some(b'Z' | b'z') if i + 1 == b.len() => 0,
            Some(&sign @ (b'+' | b'-')) if b.len() == i + 6 && b[i + 3] == b':' => {
                let (oh, om) = (num(i + 1..i + 3)?, num(i + 4..i + 6)?);
                if oh > 23 || om > 59 {
                    return Err(err());
                }
                let o = oh * 3600 + om * 60;
                if sign == b'+' { o } else { -o }
            }
            _ => return Err(err()),
        };
        let secs = h * 3600 + mi * 60 + se - offset_secs;
        Ok(Timestamp(date.0 + secs * MICROS_PER_SEC + micros))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secs = self.0.div_euclid(MICROS_PER_SEC);
        let micros = self.0.rem_euclid(MICROS_PER_SEC);
        let (days, sod) = (secs.div_euclid(SECS_PER_DAY), secs.rem_euclid(SECS_PER_DAY));
        let (y, m, d) = civil_from_days(days);
        write!(
            f,
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{micros:06}Z",
            sod / 3600,
            sod % 3600 / 60,
            sod % 60
        )
    }
}

impl FromStr for Timestamp {
    type Err = TimestampError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Timestamp::parse(s)
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Timestamp::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;

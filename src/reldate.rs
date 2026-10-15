//! Relative ("time ago") formatting for the Modified column, Finder/Things
//! style. The relative bands are plain duration math; the calendar bands
//! (weekday, date) format in local time so they agree with the row's local
//! hover timestamp. [`relative_date_in`] takes the timezone explicitly, so
//! tests pin UTC and stay deterministic on any machine.

use chrono::{DateTime, Datelike, TimeZone, Utc};
use std::fmt::Display;
use std::time::{SystemTime, UNIX_EPOCH};

/// Shown when a timestamp lies outside the range chrono can represent (an mtime
/// hundreds of thousands of years away); matches the panel's unknown-mtime dash.
const OUT_OF_RANGE: &str = "–";

/// `t` as a UTC datetime, or `None` when it is outside chrono's range. chrono's
/// `From<SystemTime>` panics there, and a row render must never panic on a
/// hostile mtime.
fn to_utc(t: SystemTime) -> Option<DateTime<Utc>> {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => DateTime::from_timestamp(i64::try_from(d.as_secs()).ok()?, d.subsec_nanos()),
        Err(e) => {
            let d = e.duration();
            let secs = i64::try_from(d.as_secs()).ok()?;
            match d.subsec_nanos() {
                0 => DateTime::from_timestamp(-secs, 0),
                n => {
                    DateTime::from_timestamp(secs.checked_neg()?.checked_sub(1)?, 1_000_000_000 - n)
                }
            }
        }
    }
}

/// A human-friendly modified time relative to `now`.
///
/// Bands, by elapsed time (`now - modified`):
/// - up to a day in the future, or under a minute -> "just now"
///   (further in the future -> the calendar date, below)
/// - under an hour -> "Nm"
/// - under a day -> "Nh"
/// - under two days -> "Yesterday"
/// - under a week and under 7 calendar days back -> the weekday name
///   ("Tuesday"), so it never repeats today's weekday
/// - same calendar year -> "Mon D" ("Jun 5")
/// - older -> "Mon D YYYY" ("May 11 2020")
///
/// Calendar bands use the local timezone.
pub fn relative_date(modified: SystemTime, now: SystemTime) -> String {
    relative_date_in(modified, now, &chrono::Local)
}

/// [`relative_date`] with the calendar bands in timezone `tz`.
pub fn relative_date_in<Tz>(modified: SystemTime, now: SystemTime, tz: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: Display,
{
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    /// How far ahead of `now` a timestamp may sit and still read as the present.
    const FUTURE_SKEW_TOLERANCE: u64 = DAY;

    let elapsed = match now.duration_since(modified) {
        Ok(d) => d,
        Err(e) => {
            // Ahead of `now`. Small skew (NTP, copied metadata) reads oddly as a
            // negative age, so collapse it to the present; a wildly future date
            // is real information, so show the date instead.
            if e.duration().as_secs() <= FUTURE_SKEW_TOLERANCE {
                return "just now".to_string();
            }
            let (Some(modified_utc), Some(now_utc)) = (to_utc(modified), to_utc(now)) else {
                return OUT_OF_RANGE.to_string();
            };
            let (modified_tz, now_tz) = (modified_utc.with_timezone(tz), now_utc.with_timezone(tz));
            return if modified_tz.year() == now_tz.year() {
                modified_tz.format("%b %-d").to_string()
            } else {
                modified_tz.format("%b %-d %Y").to_string()
            };
        }
    };
    let secs = elapsed.as_secs();

    if secs < MINUTE {
        return "just now".to_string();
    }
    if secs < HOUR {
        return format!("{}m", secs / MINUTE);
    }
    if secs < DAY {
        return format!("{}h", secs / HOUR);
    }
    if secs < 2 * DAY {
        return "Yesterday".to_string();
    }

    let (Some(modified_utc), Some(now_utc)) = (to_utc(modified), to_utc(now)) else {
        return OUT_OF_RANGE.to_string();
    };
    let (modified_tz, now_tz) = (modified_utc.with_timezone(tz), now_utc.with_timezone(tz));
    let calendar_days = (now_tz.date_naive() - modified_tz.date_naive()).num_days();
    if secs < WEEK && calendar_days < 7 {
        return modified_tz.format("%A").to_string();
    }

    if modified_tz.year() == now_tz.year() {
        modified_tz.format("%b %-d").to_string()
    } else {
        modified_tz.format("%b %-d %Y").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    /// A `SystemTime` `secs` after the epoch (deterministic, timezone-free).
    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    // 2021-06-15 12:00:00 UTC (a Tuesday), well into the epoch so every band
    // below stays positive.
    const NOW: u64 = 1_623_758_400;

    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    fn rel(modified_secs: u64) -> String {
        relative_date_in(at(modified_secs), at(NOW), &Utc)
    }

    #[test]
    fn future_collapses_to_just_now() {
        assert_eq!(rel(NOW + 500), "just now");
        assert_eq!(rel(NOW + DAY), "just now"); // tolerance boundary, inclusive
    }

    #[test]
    fn far_future_shows_the_date() {
        // Two days ahead is 2021-06-17, same calendar year.
        assert_eq!(rel(NOW + 2 * DAY), "Jun 17");
        // 400 days ahead is 2022-07-20, a different calendar year.
        assert_eq!(rel(NOW + 400 * DAY), "Jul 20 2022");
    }

    #[test]
    fn under_a_minute_is_just_now() {
        assert_eq!(rel(NOW), "just now"); // 0s
        assert_eq!(rel(NOW - 59), "just now"); // 59s
    }

    #[test]
    fn minutes_band() {
        assert_eq!(rel(NOW - MINUTE), "1m"); // exactly 60s
        assert_eq!(rel(NOW - 5 * MINUTE), "5m");
        assert_eq!(rel(NOW - 59 * MINUTE), "59m");
    }

    #[test]
    fn hours_band() {
        assert_eq!(rel(NOW - HOUR), "1h"); // exactly 60m
        assert_eq!(rel(NOW - 3 * HOUR), "3h");
        assert_eq!(rel(NOW - (DAY - 1)), "23h"); // one second under a day
    }

    #[test]
    fn yesterday_band() {
        assert_eq!(rel(NOW - DAY), "Yesterday"); // exactly 24h
        assert_eq!(rel(NOW - (2 * DAY - 1)), "Yesterday"); // one second under 2 days
    }

    #[test]
    fn weekday_band() {
        // Exactly 48h ago is 2021-06-13, a Sunday.
        assert_eq!(rel(NOW - 2 * DAY), "Sunday");
        // 3 days ago is 2021-06-12, a Saturday.
        assert_eq!(rel(NOW - 3 * DAY), "Saturday");
        // 6 days ago is 2021-06-09, a Wednesday.
        assert_eq!(rel(NOW - 6 * DAY), "Wednesday");
        // One second under a week is 2021-06-08, also a Tuesday like today:
        // a weekday name would read as today, so it shows the date.
        assert_eq!(rel(NOW - (7 * DAY - 1)), "Jun 8");
    }

    #[test]
    fn calendar_bands_follow_the_given_timezone() {
        // Saturday 2021-06-12 23:30 UTC is already Sunday at UTC+2.
        let modified = at(NOW - 3 * DAY + 11 * HOUR + 30 * MINUTE);
        let plus_two = chrono::FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(relative_date_in(modified, at(NOW), &Utc), "Saturday");
        assert_eq!(relative_date_in(modified, at(NOW), &plus_two), "Sunday");
        // Near New Year the year suffix follows the zone too:
        // 2020-12-31 23:30 UTC is 2021-01-01 at UTC+2.
        let new_year = at(1_609_457_400);
        assert_eq!(relative_date_in(new_year, at(NOW), &Utc), "Dec 31 2020");
        assert_eq!(relative_date_in(new_year, at(NOW), &plus_two), "Jan 1");
    }

    #[test]
    fn same_year_month_day() {
        // Exactly a week ago is 2021-06-08.
        assert_eq!(rel(NOW - 7 * DAY), "Jun 8");
        // 10 days ago is 2021-06-05 (no leading zero on the day).
        assert_eq!(rel(NOW - 10 * DAY), "Jun 5");
    }

    #[test]
    fn older_includes_year() {
        // 400 days ago is 2020-05-11 12:00 UTC, a different calendar year.
        assert_eq!(rel(NOW - 400 * DAY), "May 11 2020");
    }

    #[test]
    fn out_of_range_mtime_shows_placeholder_instead_of_panicking() {
        // Beyond chrono's year ~262143. Not every platform's SystemTime reaches
        // that far (Windows tops out earlier), in which case there is nothing
        // to guard.
        let Some(far) = UNIX_EPOCH.checked_add(Duration::from_secs(9_000_000_000_000)) else {
            return;
        };
        assert_eq!(relative_date(far, at(NOW)), OUT_OF_RANGE); // far future
        assert_eq!(relative_date(at(NOW), far), OUT_OF_RANGE); // "now" out of range
    }

    #[test]
    fn to_utc_matches_chrono_on_both_sides_of_the_epoch() {
        for t in [
            at(NOW),
            UNIX_EPOCH + Duration::from_millis(1_500),
            UNIX_EPOCH - Duration::from_secs(3),
            UNIX_EPOCH - Duration::from_millis(1_500),
        ] {
            let expected: DateTime<Utc> = t.into();
            assert_eq!(to_utc(t), Some(expected), "{t:?}");
        }
    }
}

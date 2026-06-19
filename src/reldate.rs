//! Relative ("time ago") formatting for the Modified column, Finder/Things
//! style. Pure and deterministic: the relative bands are plain duration math
//! and the calendar bands format in UTC, so a fixed `now` yields fixed output
//! regardless of the machine's timezone (handy for golden tests, and the exact
//! local timestamp stays available as the row's hover tooltip).

use chrono::Datelike;
use std::time::SystemTime;

/// A human-friendly modified time relative to `now`.
///
/// Bands, by elapsed time (`now - modified`):
/// - in the future, or under a minute -> "just now"
/// - under an hour -> "Nm"
/// - under a day -> "Nh"
/// - under two days -> "Yesterday"
/// - under a week -> the weekday name ("Tuesday")
/// - same calendar year -> "Mon D" ("Jun 5")
/// - older -> "Mon D YYYY" ("May 11 2020")
pub fn relative_date(modified: SystemTime, now: SystemTime) -> String {
    // A modified time ahead of `now` (clock skew, copied metadata) reads oddly
    // as a negative age, so collapse it to the present.
    let Ok(elapsed) = now.duration_since(modified) else {
        return "just now".to_string();
    };
    let secs = elapsed.as_secs();

    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;

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

    let modified_utc: chrono::DateTime<chrono::Utc> = modified.into();
    if secs < WEEK {
        return modified_utc.format("%A").to_string();
    }

    let now_utc: chrono::DateTime<chrono::Utc> = now.into();
    if modified_utc.year() == now_utc.year() {
        modified_utc.format("%b %-d").to_string()
    } else {
        modified_utc.format("%b %-d %Y").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    /// A SystemTime `secs` after the epoch (deterministic, timezone-free).
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
        relative_date(at(modified_secs), at(NOW))
    }

    #[test]
    fn future_collapses_to_just_now() {
        assert_eq!(rel(NOW + 500), "just now");
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
        // One second under a week still falls in the weekday band.
        assert_eq!(rel(NOW - (7 * DAY - 1)), "Tuesday");
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
}

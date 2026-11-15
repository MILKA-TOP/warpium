//! The `since` argument: a relative duration (`90s`, `2m`), a timestamp as the tools print them
//! (`11-15 10:23:45.123`, `10:23:45`, ISO 8601), or a mark returned by `clear_logs`.

use std::time::Duration;

use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use warpium_android::logcat::LogTime;

/// Where a query starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Since {
    /// Entries received at or after this sequence number (a mark).
    Seq(u64),
    /// Entries logged at or after this device time.
    Time(LogTime),
}

/// The prefix of mark tokens (`mark:1234`).
pub const MARK_PREFIX: &str = "mark:";

/// Parses `90s`, `500ms`, `2m`, `2min`, `1.5h`.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: f64 = number.parse().ok()?;
    let seconds = match unit.trim().to_ascii_lowercase().as_str() {
        "ms" => number / 1000.0,
        "s" | "sec" | "secs" | "second" | "seconds" => number,
        "m" | "min" | "mins" | "minute" | "minutes" => number * 60.0,
        "h" | "hr" | "hour" | "hours" => number * 3600.0,
        _ => return None,
    };
    (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
}

/// Parses a `since` argument. `now` is the host's time; `device_offset_secs` is how far the
/// device's clock is ahead of it, so relative durations count back from the device's "now".
/// Timestamps without a zone are host-local, as the tools print them.
pub fn parse_since(
    text: &str,
    now: DateTime<Local>,
    device_offset_secs: i64,
) -> Result<Since, String> {
    let text = text.trim();
    if let Some(mark) = text.strip_prefix(MARK_PREFIX) {
        return mark
            .parse()
            .map(Since::Seq)
            .map_err(|_| format!("Invalid mark \"{text}\"."));
    }
    if let Some(duration) = parse_duration(text) {
        let device_now = now.timestamp_nanos_opt().unwrap_or_default() as i128
            + device_offset_secs as i128 * 1_000_000_000;
        let start = device_now - duration.as_nanos() as i128;
        return Ok(Since::Time(LogTime {
            sec: start.div_euclid(1_000_000_000) as i64,
            nsec: start.rem_euclid(1_000_000_000) as u32,
        }));
    }
    parse_timestamp(text, now).map(Since::Time).ok_or_else(|| {
        format!(
            "Can't read since \"{text}\": pass a duration (\"30s\", \"2m\", \"1h\"), a time \
             (\"10:23:45\", \"11-15 10:23:45.123\", \"2026-11-15T10:23:45\") or a mark from \
             clear_logs."
        )
    })
}

fn parse_timestamp(text: &str, now: DateTime<Local>) -> Option<LogTime> {
    if let Ok(time) = DateTime::parse_from_rfc3339(text) {
        return Some(to_log_time(time.timestamp(), time.timestamp_subsec_nanos()));
    }
    let normalized = text.replacen('T', " ", 1);
    let naive = NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%d %H:%M:%S%.f")
        .ok()
        .or_else(|| {
            // Logcat's `MM-DD` form: this year, or last year when that would be in the future
            // (just after New Year).
            let in_year = |year: i32| {
                let with_year = format!("{year}-{normalized}");
                NaiveDateTime::parse_from_str(&with_year, "%Y-%m-%d %H:%M:%S%.f").ok()
            };
            let this_year = in_year(now.year())?;
            if this_year > now.naive_local() + chrono::Duration::days(1) {
                in_year(now.year() - 1)
            } else {
                Some(this_year)
            }
        })
        .or_else(|| {
            let time = NaiveTime::parse_from_str(&normalized, "%H:%M:%S%.f").ok()?;
            let date: NaiveDate = now.date_naive();
            Some(date.and_time(time))
        })?;
    let local = Local.from_local_datetime(&naive).earliest()?;
    Some(to_log_time(
        local.timestamp(),
        local.timestamp_subsec_nanos(),
    ))
}

fn to_log_time(sec: i64, nsec: u32) -> LogTime {
    LogTime { sec, nsec }
}

#[cfg(test)]
#[path = "since_tests.rs"]
mod tests;

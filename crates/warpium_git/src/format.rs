//! Text for dates and authors in the log, blame annotations and tooltips.

use chrono::{Local, TimeZone};

/// `2026-11-19 14:05` in the local time zone.
pub fn local_date_time(unix_seconds: i64) -> String {
    match Local.timestamp_opt(unix_seconds, 0).single() {
        Some(time) => time.format("%Y-%m-%d %H:%M").to_string(),
        None => String::new(),
    }
}

/// `2026-11-19` in the local time zone.
pub fn local_date(unix_seconds: i64) -> String {
    match Local.timestamp_opt(unix_seconds, 0).single() {
        Some(time) => time.format("%Y-%m-%d").to_string(),
        None => String::new(),
    }
}

/// How long before `now` a time was: `just now`, `5 minutes ago`, `yesterday`, `3 months ago`.
pub fn relative_time(unix_seconds: i64, now: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    let elapsed = now - unix_seconds;
    let plural = |count: i64, unit: &str| {
        if count == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{count} {unit}s ago")
        }
    };
    match elapsed {
        i64::MIN..60 => "just now".to_owned(),
        60..HOUR => plural(elapsed / MINUTE, "minute"),
        HOUR..DAY => plural(elapsed / HOUR, "hour"),
        DAY..172_800 => "yesterday".to_owned(),
        172_800..2_592_000 => plural(elapsed / DAY, "day"),
        2_592_000..31_536_000 => plural(elapsed / (30 * DAY), "month"),
        _ => plural(elapsed / (365 * DAY), "year"),
    }
}

/// `name` cut to `max_chars` characters, ending in `…` when cut.
pub fn truncate_chars(name: &str, max_chars: usize) -> String {
    if name.chars().count() <= max_chars {
        return name.to_owned();
    }
    let mut truncated: String = name.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;

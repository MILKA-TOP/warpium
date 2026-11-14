//! What the app tells the user about a check.

use std::time::Duration;

use crate::check::{CheckError, CheckOutcome, UpstreamStatus};

const MAX_INTERVAL_HOURS: u64 = 7 * 24;

/// The scheduled-check interval for the `warpium.upstream_check_interval_hours` setting, clamped
/// to between one hour and a week.
pub fn check_interval(hours: u64) -> Duration {
    Duration::from_secs(hours.clamp(1, MAX_INTERVAL_HOURS) * 60 * 60)
}

/// Whether to show the "new upstream commits" notification: upstream is at least `min_commits`
/// (and at least one) ahead and the user hasn't dismissed this upstream head.
pub fn should_notify(
    status: &UpstreamStatus,
    dismissed_head: Option<&str>,
    min_commits: u64,
) -> bool {
    u64::from(status.ahead_by) >= min_commits.max(1)
        && dismissed_head != Some(status.head_sha.as_str())
}

/// The banner's bold lead. Kept short because the banner shares one line with its three actions
/// and elides from the end.
pub fn banner_heading(status: &UpstreamStatus) -> String {
    format!("{}.", commits_phrase(status.ahead_by))
}

/// The banner's secondary text, elided first when the window is narrow.
pub fn banner_description(status: &UpstreamStatus) -> String {
    match date_part(&status.head_date) {
        Some(date) => format!("Latest: {date}"),
        None => String::new(),
    }
}

/// The result of a manual check, phrased for a toast.
pub fn manual_report(outcome: &CheckOutcome, now: u64) -> String {
    match outcome {
        CheckOutcome::Checked(status) if status.ahead_by == 0 => {
            "Warpium is up to date with upstream Warp.".to_owned()
        }
        CheckOutcome::Checked(status) => {
            format!("{} since this build.", commits_phrase(status.ahead_by))
        }
        CheckOutcome::Failed(CheckError::RateLimited { until })
        | CheckOutcome::Deferred { until } => format!(
            "GitHub's rate limit was reached; try again in {}.",
            wait_phrase(until.saturating_sub(now))
        ),
        CheckOutcome::Failed(err) => format!("Couldn't check for upstream Warp updates: {err}."),
    }
}

fn commits_phrase(ahead_by: u32) -> String {
    match ahead_by {
        1 => "1 new upstream Warp commit".to_owned(),
        n => format!("{n} new upstream Warp commits"),
    }
}

fn wait_phrase(seconds: u64) -> String {
    let minutes = seconds.div_ceil(60).max(1);
    match minutes {
        1 => "1 minute".to_owned(),
        m if m < 60 => format!("{m} minutes"),
        m => {
            let hours = m.div_ceil(60);
            if hours == 1 {
                "1 hour".to_owned()
            } else {
                format!("{hours} hours")
            }
        }
    }
}

/// The `YYYY-MM-DD` part of an ISO 8601 timestamp.
fn date_part(timestamp: &str) -> Option<&str> {
    let date = timestamp.get(..10)?;
    let bytes = date.as_bytes();
    let well_formed = bytes.iter().enumerate().all(|(i, b)| match i {
        4 | 7 => *b == b'-',
        _ => b.is_ascii_digit(),
    });
    well_formed.then_some(date)
}

#[cfg(test)]
#[path = "notice_tests.rs"]
mod tests;

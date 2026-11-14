use std::time::Duration;

use super::*;

const HEAD: &str = "c6f07bb451657e6fbce72c69ed90cfd4f86c2344";

fn status(ahead_by: u32, head_date: &str) -> UpstreamStatus {
    UpstreamStatus {
        base: "20ec697".to_owned(),
        head_sha: HEAD.to_owned(),
        head_date: head_date.to_owned(),
        ahead_by,
    }
}

#[test]
fn notifies_when_upstream_is_ahead_and_not_dismissed() {
    assert!(should_notify(&status(1, ""), None, 1));
}

#[test]
fn does_not_notify_when_up_to_date() {
    assert!(!should_notify(&status(0, ""), None, 1));
}

#[test]
fn does_not_notify_below_the_minimum_commit_count() {
    assert!(!should_notify(&status(4, ""), None, 5));
    assert!(should_notify(&status(5, ""), None, 5));
}

#[test]
fn a_zero_minimum_still_requires_one_commit() {
    assert!(!should_notify(&status(0, ""), None, 0));
}

#[test]
fn does_not_notify_for_the_dismissed_head() {
    assert!(!should_notify(&status(3, ""), Some(HEAD), 1));
}

#[test]
fn notifies_again_once_upstream_moves_past_the_dismissed_head() {
    assert!(should_notify(&status(3, ""), Some("0123456789abcdef"), 1));
}

#[test]
fn check_interval_converts_hours() {
    assert_eq!(check_interval(6), Duration::from_secs(6 * 60 * 60));
}

#[test]
fn check_interval_is_clamped() {
    assert_eq!(check_interval(0), Duration::from_secs(60 * 60));
    assert_eq!(
        check_interval(10_000),
        Duration::from_secs(7 * 24 * 60 * 60)
    );
}

#[test]
fn banner_heading_counts_commits() {
    assert_eq!(
        banner_heading(&status(1, "")),
        "1 new upstream Warp commit."
    );
    assert_eq!(
        banner_heading(&status(29, "")),
        "29 new upstream Warp commits."
    );
}

#[test]
fn banner_description_shows_the_latest_commit_date() {
    assert_eq!(
        banner_description(&status(2, "2026-11-14T07:36:23Z")),
        "Latest: 2026-11-14"
    );
}

#[test]
fn banner_description_is_empty_for_a_malformed_date() {
    assert_eq!(banner_description(&status(2, "yesterday")), "");
}

#[test]
fn manual_report_says_up_to_date() {
    assert_eq!(
        manual_report(&CheckOutcome::Checked(status(0, "")), 0),
        "Warpium is up to date with upstream Warp."
    );
}

#[test]
fn manual_report_counts_new_commits() {
    assert_eq!(
        manual_report(&CheckOutcome::Checked(status(7, "")), 0),
        "7 new upstream Warp commits since this build."
    );
}

#[test]
fn manual_report_gives_the_rate_limit_wait() {
    assert_eq!(
        manual_report(
            &CheckOutcome::Deferred {
                until: 1_000 + 30 * 60
            },
            1_000
        ),
        "GitHub's rate limit was reached; try again in 30 minutes."
    );
    assert_eq!(
        manual_report(
            &CheckOutcome::Failed(CheckError::RateLimited {
                until: 1_000 + 90 * 60
            }),
            1_000
        ),
        "GitHub's rate limit was reached; try again in 2 hours."
    );
}

#[test]
fn manual_report_explains_a_failure() {
    assert_eq!(
        manual_report(&CheckOutcome::Failed(CheckError::BaseNotFound), 0),
        "Couldn't check for upstream Warp updates: upstream Warp does not contain this build's base commit."
    );
}

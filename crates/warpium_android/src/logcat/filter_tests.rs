use std::collections::HashSet;

use super::*;
use crate::logcat::entry::LogTime;

fn entry(pid: i32, level: LogLevel, tag: &str, message: &str) -> LogEntry {
    LogEntry {
        time: LogTime::default(),
        pid,
        tid: pid,
        level,
        tag: tag.into(),
        message: message.into(),
    }
}

#[test]
fn tag_filter_includes_matching_terms_case_insensitively() {
    let filter = TagFilter::parse("okhttp, MyApp");

    assert!(filter.matches("OkHttp"));
    assert!(filter.matches("MyAppViewModel"));
    assert!(!filter.matches("ActivityManager"));
}

#[test]
fn tag_filter_excludes_minus_terms() {
    let filter = TagFilter::parse("-chatty, -EGL");

    assert!(filter.matches("MyApp"));
    assert!(!filter.matches("chatty"));
    assert!(!filter.matches("EGL_emulation"));
}

#[test]
fn tag_filter_exclusion_wins_over_inclusion() {
    let filter = TagFilter::parse("My, -MyNoisy");

    assert!(filter.matches("MyApp"));
    assert!(!filter.matches("MyNoisyTag"));
}

#[test]
fn empty_tag_filter_matches_everything() {
    let filter = TagFilter::parse(" , - ,");

    assert!(filter.is_empty());
    assert!(filter.matches("Anything"));
}

#[test]
fn plain_text_filter_is_case_insensitive_and_literal() {
    let filter = TextFilter::parse("user.id (", false).unwrap();

    assert!(filter.matches(&entry(1, LogLevel::Info, "T", "Loaded USER.ID ( 42 )")));
    assert!(!filter.matches(&entry(1, LogLevel::Info, "T", "Loaded userXid ( 42 )")));
}

#[test]
fn text_filter_also_matches_the_tag() {
    let filter = TextFilter::parse("okhttp", false).unwrap();

    assert!(filter.matches(&entry(1, LogLevel::Info, "OkHttp", "--> GET /")));
}

#[test]
fn regex_text_filter_matches_pattern() {
    let filter = TextFilter::parse(r"took \d+ ms", true).unwrap();

    assert!(filter.matches(&entry(1, LogLevel::Info, "T", "query took 120 ms")));
    assert!(!filter.matches(&entry(1, LogLevel::Info, "T", "query took long")));
}

#[test]
fn invalid_regex_is_an_error() {
    assert!(TextFilter::parse("(unclosed", true).is_err());
}

#[test]
fn log_filter_applies_minimum_level() {
    let filter = LogFilter {
        min_level: LogLevel::Warn,
        ..LogFilter::default()
    };

    assert!(!filter.matches(&entry(1, LogLevel::Info, "T", "m"), None));
    assert!(filter.matches(&entry(1, LogLevel::Warn, "T", "m"), None));
    assert!(filter.matches(&entry(1, LogLevel::Fatal, "T", "m"), None));
}

#[test]
fn log_filter_restricts_to_pids() {
    let pids = HashSet::from([10, 11]);

    assert!(LogFilter::default().matches(&entry(10, LogLevel::Info, "T", "m"), Some(&pids)));
    assert!(!LogFilter::default().matches(&entry(12, LogLevel::Info, "T", "m"), Some(&pids)));
}

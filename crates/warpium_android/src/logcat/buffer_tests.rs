use super::*;
use crate::logcat::entry::{LogLevel, LogTime};
use crate::logcat::filter::TextFilter;

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

fn info(pid: i32, message: &str) -> LogEntry {
    entry(pid, LogLevel::Info, "App", message)
}

fn lines(buffer: &LogBuffer) -> Vec<String> {
    buffer
        .rows(0..buffer.row_count())
        .map(|row| row.line.to_owned())
        .collect()
}

#[test]
fn splits_multi_line_messages_into_rows() {
    let mut buffer = LogBuffer::new(100);

    let outcome = buffer.push_batch(vec![
        info(1, "first\n\tat a.B.c(B.kt:1)\r\nlast"),
        info(1, "next"),
    ]);

    assert_eq!(outcome.added_rows, 4);
    assert_eq!(
        lines(&buffer),
        vec!["first", "\tat a.B.c(B.kt:1)", "last", "next"]
    );
    let first_lines: Vec<bool> = buffer.rows(0..4).map(|row| row.is_first_line).collect();
    assert_eq!(first_lines, vec![true, false, false, true]);
}

#[test]
fn evicts_oldest_entries_beyond_capacity() {
    let mut buffer = LogBuffer::new(2);
    buffer.push_batch(vec![info(1, "a\nb"), info(1, "c")]);

    let outcome = buffer.push_batch(vec![info(1, "d")]);

    assert_eq!(outcome.evicted_rows, 2);
    assert_eq!(buffer.entry_count(), 2);
    assert_eq!(lines(&buffer), vec!["c", "d"]);
}

#[test]
fn filter_change_rebuilds_rows() {
    let mut buffer = LogBuffer::new(100);
    buffer.push_batch(vec![
        entry(1, LogLevel::Debug, "App", "debug"),
        entry(1, LogLevel::Error, "App", "error"),
    ]);

    buffer.set_filter(LogFilter {
        min_level: LogLevel::Warn,
        ..LogFilter::default()
    });

    assert_eq!(lines(&buffer), vec!["error"]);
}

#[test]
fn new_entries_respect_the_filter() {
    let mut buffer = LogBuffer::new(100);
    buffer.set_filter(LogFilter {
        text: TextFilter::parse("keep", false).unwrap(),
        ..LogFilter::default()
    });

    let outcome = buffer.push_batch(vec![info(1, "keep me"), info(1, "drop me")]);

    assert_eq!(outcome.added_rows, 1);
    assert_eq!(lines(&buffer), vec!["keep me"]);
}

#[test]
fn clear_drops_everything_but_keeps_sequence_numbers_increasing() {
    let mut buffer = LogBuffer::new(100);
    buffer.push_batch(vec![info(1, "old")]);
    let old_seq = buffer.row(0).unwrap().seq;

    buffer.clear();
    buffer.push_batch(vec![info(1, "new")]);

    assert_eq!(lines(&buffer), vec!["new"]);
    assert!(buffer.row(0).unwrap().seq > old_seq);
}

#[test]
fn package_filter_follows_activity_manager_process_starts() {
    let mut buffer = LogBuffer::new(100);
    buffer.set_package(Some("com.example".to_owned()));

    buffer.push_batch(vec![
        info(50, "before start"),
        entry(
            1000,
            LogLevel::Info,
            "ActivityManager",
            "Start proc 50:com.example/u0a1 for activity {com.example/.Main}",
        ),
        info(50, "app line"),
        info(60, "other app"),
    ]);

    assert_eq!(lines(&buffer), vec!["app line"]);
    assert_eq!(buffer.running_pid(), Some(50));
}

#[test]
fn package_filter_keeps_logs_across_a_restart() {
    let mut buffer = LogBuffer::new(100);
    buffer.set_package(Some("com.example".to_owned()));
    let start = |pid: i32| {
        entry(
            1000,
            LogLevel::Info,
            "ActivityManager",
            &format!("Start proc {pid}:com.example/u0a1 for activity {{com.example/.Main}}"),
        )
    };

    buffer.push_batch(vec![
        start(50),
        entry(
            50,
            LogLevel::Error,
            "AndroidRuntime",
            "FATAL EXCEPTION: main",
        ),
        entry(
            1000,
            LogLevel::Info,
            "ActivityManager",
            "Process com.example (pid 50) has died: fg TOP",
        ),
    ]);
    let running_after_crash = buffer.running_pid();
    buffer.push_batch(vec![start(70), info(70, "restarted")]);

    assert_eq!(running_after_crash, None);
    assert_eq!(lines(&buffer), vec!["FATAL EXCEPTION: main", "restarted"]);
    assert_eq!(buffer.running_pid(), Some(70));
}

#[test]
fn polled_processes_reveal_earlier_entries() {
    let mut buffer = LogBuffer::new(100);
    buffer.push_batch(vec![info(42, "already running"), info(7, "other")]);
    buffer.set_package(Some("com.example".to_owned()));
    let before = lines(&buffer);

    let changed = buffer.update_package_processes(&[
        (42, "com.example".to_owned()),
        (43, "com.example:remote".to_owned()),
    ]);

    assert!(before.is_empty());
    assert!(changed);
    assert_eq!(lines(&buffer), vec!["already running"]);
    assert_eq!(buffer.running_pid(), Some(42));
    assert!(buffer.package_seen());
}

#[test]
fn polling_the_same_processes_again_changes_nothing() {
    let mut buffer = LogBuffer::new(100);
    buffer.set_package(Some("com.example".to_owned()));
    buffer.update_package_processes(&[(42, "com.example".to_owned())]);

    assert!(!buffer.update_package_processes(&[(42, "com.example".to_owned())]));
}

#[test]
fn changing_the_package_forgets_its_pids() {
    let mut buffer = LogBuffer::new(100);
    buffer.set_package(Some("com.example".to_owned()));
    buffer.update_package_processes(&[(42, "com.example".to_owned())]);
    buffer.push_batch(vec![info(42, "line")]);

    buffer.set_package(Some("com.other".to_owned()));

    assert_eq!(buffer.row_count(), 0);
    assert!(!buffer.package_seen());
}

#[test]
fn row_index_of_finds_the_first_row_of_an_entry() {
    let mut buffer = LogBuffer::new(100);
    buffer.push_batch(vec![info(1, "a\nb"), info(1, "c")]);
    let seq_of_c = buffer.row(2).unwrap().seq;

    assert_eq!(buffer.row_index_of(seq_of_c), Some(2));
    assert_eq!(buffer.row_index_of(seq_of_c + 1), None);
}

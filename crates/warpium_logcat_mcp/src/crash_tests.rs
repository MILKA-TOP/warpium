use warpium_android::logcat::LogLevel;

use super::*;

fn entry(sec: i64, pid: i32, level: LogLevel, tag: &str, message: &str) -> LogEntry {
    LogEntry {
        time: LogTime { sec, nsec: 0 },
        pid,
        tid: pid,
        level,
        tag: tag.into(),
        message: message.into(),
    }
}

#[test]
fn collects_a_java_crash_split_over_several_entries() {
    let entries = vec![
        entry(
            10,
            4242,
            LogLevel::Debug,
            "AndroidRuntime",
            "Shutting down VM",
        ),
        entry(
            10,
            4242,
            LogLevel::Error,
            "AndroidRuntime",
            "FATAL EXCEPTION: main\nProcess: com.example.app, PID: 4242",
        ),
        entry(10, 1000, LogLevel::Info, "Other", "interleaved"),
        entry(
            10,
            4242,
            LogLevel::Error,
            "AndroidRuntime",
            "java.lang.IllegalStateException: boom\n\tat com.example.app.MainActivity.onCreate(MainActivity.kt:42)",
        ),
    ];

    let crashes = extract_crashes(&entries, &HashMap::new());

    assert_eq!(
        crashes,
        vec![Crash {
            kind: CrashKind::Java,
            time: LogTime { sec: 10, nsec: 0 },
            process: Some("com.example.app".to_owned()),
            pid: Some(4242),
            lines: vec![
                "FATAL EXCEPTION: main".to_owned(),
                "Process: com.example.app, PID: 4242".to_owned(),
                "java.lang.IllegalStateException: boom".to_owned(),
                "\tat com.example.app.MainActivity.onCreate(MainActivity.kt:42)".to_owned(),
            ],
            omitted_lines: 0,
        }]
    );
}

#[test]
fn a_late_entry_from_the_same_process_starts_no_continuation() {
    let entries = vec![
        entry(
            10,
            4242,
            LogLevel::Error,
            "AndroidRuntime",
            "FATAL EXCEPTION: main",
        ),
        entry(
            30,
            4242,
            LogLevel::Error,
            "AndroidRuntime",
            "unrelated later line",
        ),
    ];

    let crashes = extract_crashes(&entries, &HashMap::new());

    assert_eq!(crashes[0].lines, vec!["FATAL EXCEPTION: main".to_owned()]);
}

#[test]
fn names_the_process_from_known_pids_when_the_report_does_not() {
    let entries = vec![entry(
        10,
        77,
        LogLevel::Error,
        "AndroidRuntime",
        "FATAL EXCEPTION: worker",
    )];
    let processes = HashMap::from([(77, "com.example.app:sync".to_owned())]);

    let crashes = extract_crashes(&entries, &processes);

    assert_eq!(crashes[0].process.as_deref(), Some("com.example.app:sync"));
}

#[test]
fn reads_anrs_from_activity_manager() {
    let entries = vec![entry(
        10,
        500,
        LogLevel::Error,
        "ActivityManager",
        "ANR in com.example.app (com.example.app/.MainActivity)\nPID: 4242\nReason: Input dispatching timed out",
    )];

    let crashes = extract_crashes(&entries, &HashMap::new());

    assert_eq!(crashes[0].kind, CrashKind::Anr);
    assert_eq!(crashes[0].process.as_deref(), Some("com.example.app"));
    assert_eq!(crashes[0].pid, Some(4242));
    assert_eq!(crashes[0].lines.len(), 3);
}

#[test]
fn a_tombstone_replaces_the_fatal_signal_line_of_the_same_pid() {
    let entries = vec![
        entry(
            10,
            4242,
            LogLevel::Fatal,
            "libc",
            "Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0x0 in tid 4250 (RenderThread), pid 4242 (com.example.app)",
        ),
        entry(
            11,
            900,
            LogLevel::Fatal,
            "DEBUG",
            "*** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***",
        ),
        entry(
            11,
            900,
            LogLevel::Fatal,
            "DEBUG",
            "pid: 4242, tid: 4250, name: RenderThread  >>> com.example.app <<<",
        ),
        entry(
            11,
            900,
            LogLevel::Fatal,
            "DEBUG",
            "      #00 pc 0001 /data/app/lib/libfoo.so",
        ),
    ];

    let crashes = extract_crashes(&entries, &HashMap::new());

    assert_eq!(crashes.len(), 1);
    assert_eq!(crashes[0].kind, CrashKind::Native);
    assert_eq!(crashes[0].process.as_deref(), Some("com.example.app"));
    assert_eq!(crashes[0].pid, Some(4242));
    assert_eq!(crashes[0].lines.len(), 3);
}

#[test]
fn keeps_a_fatal_signal_without_a_tombstone() {
    let entries = vec![entry(
        10,
        4242,
        LogLevel::Fatal,
        "libc",
        "Fatal signal 6 (SIGABRT), code -1 (SI_QUEUE) in tid 4242 (main), pid 4242 (com.example.app)",
    )];

    let crashes = extract_crashes(&entries, &HashMap::new());

    assert_eq!(crashes.len(), 1);
    assert_eq!(crashes[0].process.as_deref(), Some("com.example.app"));
}

#[test]
fn caps_the_lines_of_one_crash() {
    let trace = vec!["\tat a.B.c(B.kt:1)"; MAX_CRASH_LINES + 5].join("\n");
    let entries = vec![entry(
        10,
        1,
        LogLevel::Error,
        "AndroidRuntime",
        &format!("FATAL EXCEPTION: main\n{trace}"),
    )];

    let crashes = extract_crashes(&entries, &HashMap::new());

    assert_eq!(crashes[0].lines.len(), MAX_CRASH_LINES);
    assert_eq!(crashes[0].omitted_lines, 6);
}

#[test]
fn prefers_the_known_name_over_a_truncated_thread_name() {
    let entries = vec![entry(
        10,
        4242,
        LogLevel::Fatal,
        "libc",
        "Fatal signal 11 (SIGSEGV), code 1 in tid 4242 (le.myapplication), pid 4242 (le.myapplication)",
    )];
    let processes = HashMap::from([(4242, "com.example.myapplication".to_owned())]);

    let crashes = extract_crashes(&entries, &processes);

    assert_eq!(
        crashes[0].process.as_deref(),
        Some("com.example.myapplication")
    );
}

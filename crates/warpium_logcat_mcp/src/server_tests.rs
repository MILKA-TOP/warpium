use std::io::Write as _;
use std::net::TcpStream;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use warpium_android::logcat::LogTime;
use warpium_android::logcat::binary::encode_entry;
use warpium_android::test_support::{
    FakeAdbServer, Next, answer_transport, length_prefixed, okay,
};

use super::*;

const SERIAL: &str = "emulator-5554";

enum Live {
    Entries(Vec<LogEntry>),
    /// Ends the current logcat connection, as a device reboot would.
    EndStream,
}

/// A scripted device behind a fake adb server: a logcat backlog, live entries sent while a test
/// runs, and `ps` output.
struct FakeDevice {
    server: FakeAdbServer,
    live: mpsc::Sender<Live>,
    ps: Arc<Mutex<String>>,
}

impl FakeDevice {
    fn start(backlog: Vec<LogEntry>) -> Self {
        let backlog = Arc::new(Mutex::new(backlog));
        let (live, receiver) = mpsc::channel::<Live>();
        let receiver = Arc::new(Mutex::new(receiver));
        let ps = Arc::new(Mutex::new("PID NAME\n".to_owned()));
        let handler_ps = ps.clone();
        let server = FakeAdbServer::start(move |request, stream| {
            if request == "host:devices-l" {
                okay(stream);
                length_prefixed(
                    stream,
                    &format!("{SERIAL}  device product:sdk_gphone64 model:sdk_gphone64 transport_id:1\n"),
                );
                return Next::Close;
            }
            if let Some(next) = answer_transport(request, stream, SERIAL) {
                return next;
            }
            okay(stream);
            if request == "shell:date +%s" {
                let _ = stream.write_all(format!("{}\n", unix_now()).as_bytes());
            } else if request.starts_with("shell:getprop") {
                let _ = stream.write_all(b"34\n14\nGoogle\nPixel 7\n\n\n");
            } else if request.starts_with("shell:ps") {
                let _ = stream.write_all(handler_ps.lock().unwrap().as_bytes());
            } else if request.starts_with("exec:logcat") {
                stream_logcat(stream, &backlog, &receiver);
            }
            Next::Close
        });
        Self { server, live, ps }
    }

    fn server(&self, root: Option<PathBuf>) -> Server {
        Server::new(
            self.server.client(),
            ServerOptions {
                root,
                settle: Duration::from_millis(100),
                settle_max: Duration::from_secs(2),
                process_poll: Duration::from_millis(100),
                ..ServerOptions::default()
            },
        )
    }

    fn send(&self, entries: Vec<LogEntry>) {
        self.live.send(Live::Entries(entries)).unwrap();
    }
}

/// Writes the backlog, then live entries until the test ends or asks to end the stream. Live
/// entries join the backlog, which logcat replays on the next connection.
fn stream_logcat(
    stream: &mut TcpStream,
    backlog: &Mutex<Vec<LogEntry>>,
    receiver: &Mutex<mpsc::Receiver<Live>>,
) {
    for entry in backlog.lock().unwrap().iter() {
        if stream.write_all(&encode(entry)).is_err() {
            return;
        }
    }
    let receiver = receiver.lock().unwrap();
    loop {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(Live::Entries(entries)) => {
                for entry in entries {
                    if stream.write_all(&encode(&entry)).is_err() {
                        return;
                    }
                    backlog.lock().unwrap().push(entry);
                }
            }
            Ok(Live::EndStream) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn encode(entry: &LogEntry) -> Vec<u8> {
    let priority = match entry.level {
        LogLevel::Verbose => 2,
        LogLevel::Debug => 3,
        LogLevel::Info => 4,
        LogLevel::Warn => 5,
        LogLevel::Error => 6,
        LogLevel::Fatal => 7,
    };
    encode_entry(entry, priority)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// An entry logged `ago` seconds before now.
fn entry(ago: i64, pid: i32, level: LogLevel, tag: &str, message: &str) -> LogEntry {
    LogEntry {
        time: LogTime {
            sec: unix_now() - ago,
            nsec: 0,
        },
        pid,
        tid: pid,
        level,
        tag: tag.into(),
        message: message.into(),
    }
}

fn started(ago: i64, pid: i32, process: &str) -> LogEntry {
    entry(
        ago,
        500,
        LogLevel::Info,
        "ActivityManager",
        &format!("Start proc {pid}:{process}/u0a123 for activity {{{process}/.MainActivity}}"),
    )
}

/// Calls a tool and returns its text and whether it was an error.
fn call(server: &mut Server, tool: &str, arguments: Value) -> (String, bool) {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    });
    let response = server.handle_message(&request.to_string()).unwrap();
    let result = &response["result"];
    (
        result["content"][0]["text"].as_str().unwrap().to_owned(),
        result["isError"].as_bool().unwrap_or(false),
    )
}

#[test]
fn lists_devices_with_model_and_api_level() {
    let device = FakeDevice::start(Vec::new());
    let mut server = device.server(None);

    let (text, is_error) = call(&mut server, "list_devices", json!({}));

    assert!(!is_error);
    assert_eq!(
        text,
        "emulator-5554  emulator  online  Pixel 7  API 34 (Android 14)  [default]"
    );
}

#[test]
fn get_logs_requires_a_package_unless_all_packages_is_passed() {
    let device = FakeDevice::start(vec![entry(5, 100, LogLevel::Info, "App", "hello")]);
    let mut server = device.server(None);

    let (refused, refused_is_error) = call(&mut server, "get_logs", json!({}));
    let (all, all_is_error) = call(&mut server, "get_logs", json!({ "all_packages": true }));

    assert!(refused_is_error);
    assert!(refused.contains("all_packages: true"), "{refused}");
    assert!(!all_is_error);
    assert!(all.contains("App: hello"), "{all}");
}

#[test]
fn get_logs_shows_only_the_package_and_applies_filters() {
    let device = FakeDevice::start(vec![
        started(9, 100, "com.example"),
        entry(8, 100, LogLevel::Debug, "Net", "request sent"),
        entry(7, 100, LogLevel::Warn, "Net", "slow response"),
        entry(6, 100, LogLevel::Warn, "Db", "slow query"),
        entry(5, 200, LogLevel::Error, "Other", "another app's secret"),
    ]);
    let mut server = device.server(None);

    let (all_of_app, _) = call(&mut server, "get_logs", json!({ "package": "com.example" }));
    let (filtered, _) = call(
        &mut server,
        "get_logs",
        json!({ "package": "com.example", "min_level": "W", "tag": "-Db", "text": "SLOW" }),
    );

    assert!(all_of_app.starts_with("3 entries from emulator-5554 for com.example"), "{all_of_app}");
    assert!(!all_of_app.contains("secret"), "{all_of_app}");
    assert!(filtered.starts_with("1 entry"), "{filtered}");
    assert!(filtered.contains("W Net: slow response"), "{filtered}");
}

#[test]
fn get_logs_finds_a_package_already_running_through_ps() {
    let device = FakeDevice::start(vec![entry(5, 4242, LogLevel::Info, "App", "running")]);
    *device.ps.lock().unwrap() = "PID NAME\n4242 com.example\n".to_owned();
    let mut server = device.server(None);

    let (text, _) = call(&mut server, "get_logs", json!({ "package": "com.example" }));

    assert!(text.contains("App: running"), "{text}");
}

#[test]
fn get_logs_follows_the_package_across_a_restart() {
    let device = FakeDevice::start(vec![
        started(20, 100, "com.example"),
        entry(19, 100, LogLevel::Info, "App", "first run"),
    ]);
    let mut server = device.server(None);
    call(&mut server, "get_logs", json!({ "package": "com.example" }));

    device.send(vec![
        entry(
            10,
            500,
            LogLevel::Info,
            "ActivityManager",
            "Process com.example (pid 100) has died: fg TOP",
        ),
        // The new process logs before ActivityManager announces it.
        entry(9, 300, LogLevel::Info, "App", "second run"),
        started(9, 300, "com.example"),
    ]);
    std::thread::sleep(Duration::from_millis(300));
    let (text, _) = call(&mut server, "get_logs", json!({ "package": "com.example" }));

    assert!(text.contains("App: first run"), "{text}");
    assert!(text.contains("App: second run"), "{text}");
}

#[test]
fn get_logs_since_a_duration_leaves_out_older_lines() {
    let device = FakeDevice::start(vec![
        started(700, 100, "com.example"),
        entry(600, 100, LogLevel::Info, "App", "ten minutes ago"),
        entry(10, 100, LogLevel::Info, "App", "just now"),
    ]);
    let mut server = device.server(None);

    let (text, _) = call(
        &mut server,
        "get_logs",
        json!({ "package": "com.example", "since": "1m" }),
    );

    assert!(text.contains("just now"), "{text}");
    assert!(!text.contains("ten minutes ago"), "{text}");
}

#[test]
fn clear_logs_sets_a_mark_without_clearing_the_device() {
    let device = FakeDevice::start(vec![
        started(20, 100, "com.example"),
        entry(19, 100, LogLevel::Info, "App", "before the mark"),
    ]);
    let mut server = device.server(None);

    let (mark, _) = call(&mut server, "clear_logs", json!({}));
    device.send(vec![entry(0, 100, LogLevel::Info, "App", "after the mark")]);
    std::thread::sleep(Duration::from_millis(300));
    let (after, _) = call(&mut server, "get_logs", json!({ "package": "com.example" }));
    let (explicit, _) = call(
        &mut server,
        "get_logs",
        json!({ "package": "com.example", "since": "5m" }),
    );

    assert!(mark.starts_with("Mark set: mark:"), "{mark}");
    assert!(after.contains("after the mark"), "{after}");
    assert!(!after.contains("before the mark"), "{after}");
    assert!(explicit.contains("before the mark"), "{explicit}");
    assert!(
        device
            .server
            .requests()
            .iter()
            .all(|request| !request.contains("logcat -c")),
        "the device buffer must not be cleared"
    );
}

#[test]
fn get_logs_returns_the_newest_entries_up_to_the_limit() {
    let mut backlog = vec![started(100, 100, "com.example")];
    backlog.extend((0..30).map(|index| {
        entry(
            60 - index,
            100,
            LogLevel::Info,
            "App",
            &format!("line {index}"),
        )
    }));
    let device = FakeDevice::start(backlog);
    let mut server = device.server(None);

    let (limited, _) = call(
        &mut server,
        "get_logs",
        json!({ "package": "com.example", "limit": 10 }),
    );
    let (everything, _) = call(
        &mut server,
        "get_logs",
        json!({ "package": "com.example", "limit": 100_000 }),
    );

    assert!(limited.starts_with("10 entries"), "{limited}");
    assert!(limited.contains("truncated: true"), "{limited}");
    assert!(limited.contains("line 20") && limited.contains("line 29"), "{limited}");
    assert!(!limited.contains("line 19"), "{limited}");
    assert!(everything.starts_with("30 entries"), "{everything}");
    assert!(everything.contains("truncated: false"), "{everything}");
}

#[test]
fn get_logs_reconnects_without_repeating_the_replayed_backlog() {
    let device = FakeDevice::start(vec![
        started(20, 100, "com.example"),
        entry(19, 100, LogLevel::Info, "App", "once"),
    ]);
    let mut server = device.server(None);
    call(&mut server, "get_logs", json!({ "package": "com.example" }));

    device.live.send(Live::EndStream).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    device.send(vec![entry(0, 100, LogLevel::Info, "App", "after reconnect")]);
    let (text, _) = call(&mut server, "get_logs", json!({ "package": "com.example" }));

    assert_eq!(text.matches("App: once").count(), 1, "{text}");
    assert!(text.contains("after reconnect"), "{text}");
}

#[test]
fn get_crashes_resolves_project_frames_to_files() {
    let project = tempfile::TempDir::new().unwrap();
    let source = project
        .path()
        .join("app/src/main/java/com/example/app/MainActivity.kt");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(
        &source,
        "package com.example.app\n\nclass MainActivity {\n    fun onCreate() {}\n}\n",
    )
    .unwrap();
    let device = FakeDevice::start(vec![
        started(30, 4242, "com.example.app"),
        entry(10, 4242, LogLevel::Info, "App", "not a crash"),
        entry(
            5,
            4242,
            LogLevel::Error,
            "AndroidRuntime",
            "FATAL EXCEPTION: main\nProcess: com.example.app, PID: 4242\n\
             java.lang.IllegalStateException: boom\n\
             \tat com.example.app.MainActivity.onCreate(MainActivity.kt:42)\n\
             \tat android.app.Activity.performCreate(Activity.java:8000)",
        ),
        entry(
            4,
            9999,
            LogLevel::Error,
            "AndroidRuntime",
            "FATAL EXCEPTION: main\nProcess: com.other, PID: 9999",
        ),
    ]);
    let mut server = device.server(Some(project.path().to_path_buf()));

    let (text, is_error) = call(
        &mut server,
        "get_crashes",
        json!({ "package": "com.example.app" }),
    );

    assert!(!is_error, "{text}");
    assert!(text.starts_with("1 crash of com.example.app"), "{text}");
    assert!(
        text.contains(
            "\tat com.example.app.MainActivity.onCreate(MainActivity.kt:42)  -> \
             app/src/main/java/com/example/app/MainActivity.kt:42"
        ),
        "{text}"
    );
    assert!(
        text.contains("\tat android.app.Activity.performCreate(Activity.java:8000)\n")
            || text.ends_with("\tat android.app.Activity.performCreate(Activity.java:8000)"),
        "framework frames stay unresolved: {text}"
    );
    assert!(!text.contains("com.other"), "{text}");
}

#[test]
fn get_crashes_reports_none_for_a_healthy_app() {
    let device = FakeDevice::start(vec![
        started(30, 4242, "com.example.app"),
        entry(10, 4242, LogLevel::Info, "App", "fine"),
    ]);
    let mut server = device.server(None);

    let (text, _) = call(
        &mut server,
        "get_crashes",
        json!({ "package": "com.example.app" }),
    );

    assert_eq!(
        text,
        "No crashes of com.example.app on emulator-5554 in the captured logs."
    );
}

#[test]
fn wait_for_log_returns_the_line_once_it_appears() {
    let device = FakeDevice::start(vec![started(30, 100, "com.example")]);
    let mut server = device.server(None);
    let live = device.live.clone();
    let sender = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        live.send(Live::Entries(vec![
            entry(0, 100, LogLevel::Info, "App", "loading"),
            entry(0, 200, LogLevel::Info, "App", "Ready in another app"),
            entry(0, 100, LogLevel::Info, "App", "Ready"),
        ]))
        .unwrap();
    });

    let (text, is_error) = call(
        &mut server,
        "wait_for_log",
        json!({ "package": "com.example", "text": "ready", "timeout_s": 10 }),
    );
    sender.join().unwrap();

    assert!(!is_error, "{text}");
    assert!(text.starts_with("Found after"), "{text}");
    assert!(text.contains("App: loading\n>> "), "{text}");
    assert!(text.contains("App: Ready\n"), "{text}");
    assert!(!text.contains("another app"), "{text}");
}

#[test]
fn wait_for_log_ignores_lines_from_before_the_call() {
    let device = FakeDevice::start(vec![
        started(30, 100, "com.example"),
        entry(5, 100, LogLevel::Info, "App", "Ready"),
    ]);
    let mut server = device.server(None);
    let started_at = Instant::now();

    let (text, is_error) = call(
        &mut server,
        "wait_for_log",
        json!({ "package": "com.example", "text": "Ready", "timeout_s": 0.5 }),
    );

    assert!(!is_error, "{text}");
    assert!(text.starts_with("timed out: true"), "{text}");
    assert!(started_at.elapsed() < Duration::from_secs(5));
}

#[test]
fn wait_for_log_with_since_accepts_recent_lines() {
    let device = FakeDevice::start(vec![
        started(30, 100, "com.example"),
        entry(5, 100, LogLevel::Info, "App", "Ready"),
    ]);
    let mut server = device.server(None);

    let (text, _) = call(
        &mut server,
        "wait_for_log",
        json!({ "package": "com.example", "text": "Ready", "since": "1m", "timeout_s": 1 }),
    );

    assert!(text.starts_with("Found after"), "{text}");
}

#[test]
fn wait_for_log_needs_something_to_wait_for() {
    let device = FakeDevice::start(Vec::new());
    let mut server = device.server(None);

    let (text, is_error) = call(&mut server, "wait_for_log", json!({ "package": "com.example" }));

    assert!(is_error);
    assert_eq!(text, "Pass text or regex: the line to wait for.");
}

#[test]
fn unknown_device_lists_the_connected_ones() {
    let device = FakeDevice::start(Vec::new());
    let mut server = device.server(None);

    let (text, is_error) = call(
        &mut server,
        "get_logs",
        json!({ "package": "com.example", "device": "R58M" }),
    );

    assert!(is_error);
    assert_eq!(text, "No device R58M. Devices: emulator-5554 (online).");
}

#[test]
fn long_messages_are_cut_at_a_character_boundary() {
    let message = "é".repeat(MAX_MESSAGE_BYTES);

    let cut = truncate(&message, MAX_MESSAGE_BYTES);

    assert!(cut.len() < MAX_MESSAGE_BYTES + 40);
    assert!(cut.ends_with(&format!("… [{} bytes cut]", MAX_MESSAGE_BYTES)));
}

#[test]
fn tools_list_names_every_tool() {
    let device = FakeDevice::start(Vec::new());
    let mut server = device.server(None);

    let response = server
        .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
        .unwrap();

    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["list_devices", "get_logs", "get_crashes", "wait_for_log", "clear_logs"]
    );
}

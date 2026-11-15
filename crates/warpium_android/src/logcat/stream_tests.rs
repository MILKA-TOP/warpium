use std::io::Write as _;
use std::sync::mpsc;

use instant::Instant;

use super::*;
use crate::logcat::binary::encode_entry;
use crate::logcat::entry::{LogLevel, LogTime};
use crate::test_support::{FakeAdbServer, Next, answer_transport, okay};

fn entry(index: usize) -> LogEntry {
    LogEntry {
        time: LogTime {
            sec: 1_700_000_000,
            nsec: 0,
        },
        pid: 100,
        tid: 101,
        level: LogLevel::Debug,
        tag: "Load".into(),
        message: format!("line {index}").into(),
    }
}

/// Collects events until the stream ends.
fn collect(receiver: &mpsc::Receiver<LogcatEvent>) -> (Vec<LogEntry>, Option<String>, usize) {
    let mut entries = Vec::new();
    let mut batches = 0;
    loop {
        match receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("the stream should end")
        {
            LogcatEvent::Entries(batch) => {
                batches += 1;
                entries.extend(batch);
            }
            LogcatEvent::Ended(reason) => return (entries, reason, batches),
        }
    }
}

#[test]
fn logcat_command_requests_binary_buffers() {
    assert_eq!(
        logcat_command(&LogcatOptions { backlog: 100 }),
        "logcat -B -b main -b system -b crash -T 100"
    );
}

#[test]
fn streams_entries_from_exec_service_in_bounded_batches() {
    let count = 10_000;
    let server = FakeAdbServer::start(move |request, stream| {
        if let Some(next) = answer_transport(request, stream, "emulator-5554") {
            return next;
        }
        assert_eq!(request, "exec:logcat -B -b main -b system -b crash -T 5000");
        okay(stream);
        let mut bytes = Vec::new();
        for index in 0..count {
            bytes.extend(encode_entry(&entry(index), 3));
        }
        // Odd-sized writes, so records straddle reads.
        for chunk in bytes.chunks(997) {
            let _ = stream.write_all(chunk);
        }
        Next::Close
    });
    let (sender, receiver) = mpsc::channel();

    let handle = spawn_logcat(
        server.client(),
        "emulator-5554".to_owned(),
        LogcatOptions::default(),
        move |event| {
            let _ = sender.send(event);
        },
    )
    .unwrap();
    let (entries, reason, batches) = collect(&receiver);

    assert_eq!(entries.len(), count);
    assert_eq!(entries[9_999], entry(9_999));
    assert_eq!(reason, None);
    assert!(
        batches >= 3,
        "10k entries should come in batches of at most {MAX_BATCH}"
    );
    handle.stop_and_join();
}

#[test]
fn reports_missing_device() {
    let server = FakeAdbServer::start(|request, stream| {
        answer_transport(request, stream, "emulator-5554").unwrap_or(Next::Close)
    });
    let (sender, receiver) = mpsc::channel();

    let _handle = spawn_logcat(
        server.client(),
        "gone".to_owned(),
        LogcatOptions::default(),
        move |event| {
            let _ = sender.send(event);
        },
    )
    .unwrap();
    let (entries, reason, _) = collect(&receiver);

    assert!(entries.is_empty());
    assert!(reason.is_some_and(|reason| reason.contains("not found")));
}

#[test]
fn dropping_the_handle_stops_a_quiet_stream() {
    let server = FakeAdbServer::start(|request, stream| {
        if let Some(next) = answer_transport(request, stream, "emulator-5554") {
            return next;
        }
        okay(stream);
        // Keep the connection open and silent until the client goes away.
        std::thread::sleep(Duration::from_secs(30));
        Next::Close
    });
    let handle = spawn_logcat(
        server.client(),
        "emulator-5554".to_owned(),
        LogcatOptions::default(),
        |_| {},
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(100));

    let started = Instant::now();
    handle.stop_and_join();

    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn process_poller_reports_changes_only() {
    let polls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server = FakeAdbServer::start(move |request, stream| {
        if let Some(next) = answer_transport(request, stream, "R58M") {
            return next;
        }
        okay(stream);
        let poll = polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let output = if poll < 2 {
            "PID NAME\n42 com.example\n"
        } else {
            "PID NAME\n77 com.example\n"
        };
        let _ = stream.write_all(output.as_bytes());
        Next::Close
    });
    let (sender, receiver) = mpsc::channel();

    let handle = spawn_process_poller(
        server.client(),
        "R58M".to_owned(),
        "com.example".to_owned(),
        Duration::from_millis(1),
        move |processes| {
            let _ = sender.send(processes);
        },
    )
    .unwrap();
    let first = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    let second = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    handle.stop_and_join();

    assert_eq!(first, vec![(42, "com.example".to_owned())]);
    assert_eq!(second, vec![(77, "com.example".to_owned())]);
}

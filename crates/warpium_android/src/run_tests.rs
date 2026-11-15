use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use super::*;
use crate::test_support::{FakeAdbServer, Next, answer_transport, okay};

#[test]
fn parse_update_time_reads_the_value() {
    let output = "    lastUpdateTime=2026-11-15 10:23:45\n";

    assert_eq!(
        parse_update_time(output).as_deref(),
        Some("2026-11-15 10:23:45")
    );
}

#[test]
fn parse_update_time_is_none_when_not_installed() {
    assert_eq!(parse_update_time(""), None);
    assert_eq!(
        parse_update_time("Unable to find package: com.example\n"),
        None
    );
}

#[test]
fn launch_failed_recognizes_errors() {
    assert!(!launch_failed(
        "Starting: Intent { act=android.intent.action.MAIN cmp=com.example/.Main }\n"
    ));
    assert!(launch_failed(
        "Error: Activity class {com.example/.Main} does not exist.\n"
    ));
    assert!(launch_failed(
        "** No activities found to run, monkey aborted.\n"
    ));
}

/// A device where the package's install time changes after `polls_until_installed` reads, and
/// that answers the launch command with `launch_output`.
fn device(polls_until_installed: usize, launch_output: &'static str) -> FakeAdbServer {
    let reads = Arc::new(AtomicUsize::new(0));
    FakeAdbServer::start(move |request, stream| {
        if let Some(next) = answer_transport(request, stream, "emulator-5554") {
            return next;
        }
        okay(stream);
        if request.contains("lastUpdateTime") {
            let read = reads.fetch_add(1, Ordering::SeqCst);
            let time = if read >= polls_until_installed {
                "lastUpdateTime=2026-11-15 10:30:00\n"
            } else {
                "lastUpdateTime=2026-11-15 10:00:00\n"
            };
            let _ = stream.write_all(time.as_bytes());
        } else {
            let _ = stream.write_all(launch_output.as_bytes());
        }
        Next::Close
    })
}

fn run(server: &FakeAdbServer, timeout: Duration) -> LaunchOutcome {
    let (sender, receiver) = mpsc::channel();
    let handle = spawn_launch_after_install(
        server.client(),
        "emulator-5554".to_owned(),
        "com.example".to_owned(),
        timeout,
        Duration::from_millis(1),
        move |outcome| {
            let _ = sender.send(outcome);
        },
    )
    .unwrap();
    let outcome = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    handle.stop_and_join();
    outcome
}

#[test]
fn launches_once_the_install_time_changes() {
    let server = device(3, "Starting: Intent { cmp=com.example/.Main }\n");

    let outcome = run(&server, Duration::from_secs(10));

    assert_eq!(outcome, LaunchOutcome::Launched);
    assert!(
        server
            .requests()
            .last()
            .is_some_and(|request| request.contains("resolve-activity"))
    );
}

#[test]
fn reports_a_failed_launch() {
    let server = device(1, "Error: Activity not started, unable to resolve Intent\n");

    let outcome = run(&server, Duration::from_secs(10));

    assert!(matches!(outcome, LaunchOutcome::Failed(output) if output.starts_with("Error")));
}

#[test]
fn times_out_when_nothing_is_installed() {
    let server = device(usize::MAX, "");

    let outcome = run(&server, Duration::from_millis(30));

    assert_eq!(outcome, LaunchOutcome::TimedOut);
}

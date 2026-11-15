use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use super::*;
use crate::test_support::{FakeAdbServer, Next, answer_transport, fail, okay};

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

/// A device where `package`'s install time changes after `polls_until_installed` reads of it
/// (the first `failed_reads` reads fail), other packages are never installed, and the launch
/// command is answered with `launch_output`.
fn device(
    package: &'static str,
    failed_reads: usize,
    polls_until_installed: usize,
    launch_output: &'static str,
) -> FakeAdbServer {
    let reads = Arc::new(AtomicUsize::new(0));
    FakeAdbServer::start(move |request, stream| {
        if let Some(next) = answer_transport(request, stream, "emulator-5554") {
            return next;
        }
        if request.contains("lastUpdateTime") {
            if !request.contains(&format!("dumpsys package {package} ")) {
                okay(stream);
                return Next::Close;
            }
            let read = reads.fetch_add(1, Ordering::SeqCst);
            if read < failed_reads {
                fail(stream, "device busy");
                return Next::Close;
            }
            okay(stream);
            let time = if read >= failed_reads + polls_until_installed {
                "lastUpdateTime=2026-11-15 10:30:00\n"
            } else {
                "lastUpdateTime=2026-11-15 10:00:00\n"
            };
            let _ = stream.write_all(time.as_bytes());
        } else {
            okay(stream);
            let _ = stream.write_all(launch_output.as_bytes());
        }
        Next::Close
    })
}

fn request(package: &str, metadata: Option<PathBuf>, timeout: Duration) -> LaunchRequest {
    LaunchRequest {
        serial: "emulator-5554".to_owned(),
        package: package.to_owned(),
        metadata,
        timeout,
        poll: Duration::from_millis(1),
    }
}

fn run(server: &FakeAdbServer, request: LaunchRequest) -> LaunchOutcome {
    let (sender, receiver) = mpsc::channel();
    let handle = spawn_launch_after_install(server.client(), request, move |outcome| {
        let _ = sender.send(outcome);
    })
    .unwrap();
    let outcome = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    handle.stop_and_join();
    outcome
}

#[test]
fn launches_once_the_install_time_changes() {
    let server = device(
        "com.example",
        0,
        3,
        "Starting: Intent { cmp=com.example/.Main }\n",
    );

    let outcome = run(
        &server,
        request("com.example", None, Duration::from_secs(10)),
    );

    assert_eq!(outcome, LaunchOutcome::Launched("com.example".to_owned()));
    assert!(
        server
            .requests()
            .last()
            .is_some_and(|request| request.contains("resolve-activity"))
    );
}

#[test]
fn reports_a_failed_launch() {
    let server = device(
        "com.example",
        0,
        1,
        "Error: Activity not started, unable to resolve Intent\n",
    );

    let outcome = run(
        &server,
        request("com.example", None, Duration::from_secs(10)),
    );

    assert!(matches!(outcome, LaunchOutcome::Failed(output) if output.starts_with("Error")));
}

#[test]
fn times_out_when_nothing_is_installed() {
    let server = device("com.example", 0, usize::MAX / 2, "");

    let outcome = run(
        &server,
        request("com.example", None, Duration::from_millis(30)),
    );

    assert_eq!(outcome, LaunchOutcome::TimedOut);
}

#[test]
fn a_failed_baseline_read_is_retried_instead_of_counting_as_not_installed() {
    let server = device("com.example", 2, 3, "Starting: Intent\n");

    let outcome = run(
        &server,
        request("com.example", None, Duration::from_secs(10)),
    );

    assert_eq!(outcome, LaunchOutcome::Launched("com.example".to_owned()));
    let time_reads = server
        .requests()
        .iter()
        .filter(|request| request.contains("lastUpdateTime"))
        .count();
    assert!(time_reads >= 6, "launched after only {time_reads} reads");
}

#[test]
fn follows_the_application_id_the_build_writes() {
    let dir = tempfile::tempdir().unwrap();
    let metadata = dir.path().join("output-metadata.json");
    std::fs::write(&metadata, r#"{"applicationId": "com.example.old"}"#).unwrap();
    let server = device("com.example.real", 0, 50, "Starting: Intent\n");
    let (sender, receiver) = mpsc::channel();
    let handle = spawn_launch_after_install(
        server.client(),
        request(
            "com.example.old",
            Some(metadata.clone()),
            Duration::from_secs(10),
        ),
        move |outcome| {
            let _ = sender.send(outcome);
        },
    )
    .unwrap();

    // Make sure the new file gets a later modification time than the one read at the start.
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(&metadata, r#"{"applicationId": "com.example.real"}"#).unwrap();
    let outcome = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    handle.stop_and_join();

    assert_eq!(
        outcome,
        LaunchOutcome::Launched("com.example.real".to_owned())
    );
}

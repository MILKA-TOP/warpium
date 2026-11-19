use std::{
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Duration,
};

use super::*;
use crate::{
    discovery::{GrpcToken, parse_discovery_file},
    test_support::{FakeEmulator, FakeEmulatorConfig, FakeEmulatorThread},
};

const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

fn entry(fake: &FakeEmulator) -> RunningEmulator {
    parse_discovery_file("pid_1.ini", &fake.discovery_file_contents(5554)).unwrap()
}

fn secured_config() -> FakeEmulatorConfig {
    FakeEmulatorConfig {
        token: Some("secret".to_owned()),
        ..Default::default()
    }
}

fn fast_options() -> SessionOptions {
    SessionOptions {
        keepalive_interval: Duration::from_secs(3600),
        resize_debounce: Duration::from_millis(20),
        restart_delay: Duration::from_millis(20),
    }
}

fn start(emulator: RunningEmulator, options: SessionOptions) -> (Session, Receiver<SessionEvent>) {
    let (sender, events) = mpsc::channel();
    let session = Session::start(emulator, options, move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    (session, events)
}

/// Waits without blocking the runtime the fake serves on.
async fn next_event(events: &Receiver<SessionEvent>) -> SessionEvent {
    tokio::time::timeout(WAIT_TIMEOUT, async {
        loop {
            match events.try_recv() {
                Ok(event) => return event,
                Err(TryRecvError::Empty) => tokio::time::sleep(Duration::from_millis(5)).await,
                Err(TryRecvError::Disconnected) => panic!("the session ended without an event"),
            }
        }
    })
    .await
    .expect("no session event in time")
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT_TIMEOUT, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition not reached in time");
}

async fn connected(fake: &FakeEmulator) -> (Session, Receiver<SessionEvent>) {
    let (session, events) = start(entry(fake), fast_options());
    assert_eq!(next_event(&events).await, SessionEvent::Connected);
    (session, events)
}

#[tokio::test]
async fn connects_with_the_discovered_token() {
    let fake = FakeEmulator::start(secured_config()).await;

    let (_session, events) = start(entry(&fake), fast_options());

    assert_eq!(next_event(&events).await, SessionEvent::Connected);
    assert_eq!(
        fake.recorded().authorization_headers,
        vec![Some("Bearer secret".to_owned())]
    );
}

#[tokio::test]
async fn rejected_token_fails_as_unauthorized() {
    let fake = FakeEmulator::start(secured_config()).await;
    let mut emulator = entry(&fake);
    emulator.grpc_token = Some(GrpcToken::new("stale"));

    let (_session, events) = start(emulator, fast_options());

    assert_eq!(
        next_event(&events).await,
        SessionEvent::Failed(SessionError::Unauthorized)
    );
}

#[tokio::test]
async fn tls_only_endpoint_fails_without_connecting() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let mut emulator = entry(&fake);
    emulator.grpc_uses_tls = true;

    let (_session, events) = start(emulator, fast_options());

    assert_eq!(
        next_event(&events).await,
        SessionEvent::Failed(SessionError::TlsRequired)
    );
    assert!(fake.recorded().authorization_headers.is_empty());
}

#[tokio::test]
async fn closed_port_fails_as_unreachable() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let emulator = entry(&fake);
    fake.stop().await;

    let (_session, events) = start(emulator, fast_options());

    assert_eq!(
        next_event(&events).await,
        SessionEvent::Failed(SessionError::Unreachable)
    );
}

#[tokio::test]
async fn stream_delivers_frames_scaled_to_the_requested_size() {
    let fake = FakeEmulator::start(FakeEmulatorConfig {
        frames_per_stream: 1,
        ..Default::default()
    })
    .await;
    let (session, events) = connected(&fake).await;

    session.stream(540, 1200);

    assert_eq!(next_event(&events).await, SessionEvent::FrameReady);
    let frame = session.take_frame().unwrap();
    assert_eq!((frame.width, frame.height, frame.seq), (540, 1200, 1));
    assert_eq!(session.frames_received(), 1);
}

#[tokio::test]
async fn untaken_frames_are_replaced_by_the_latest() {
    let fake = FakeEmulator::start(FakeEmulatorConfig {
        frames_per_stream: 3,
        ..Default::default()
    })
    .await;
    let (session, events) = connected(&fake).await;

    session.stream(108, 240);
    assert_eq!(next_event(&events).await, SessionEvent::FrameReady);
    wait_until(|| session.frames_received() == 3).await;

    assert_eq!(session.take_frame().unwrap().seq, 3);
    assert_eq!(session.frames_dropped(), 2);
    assert_eq!(session.take_frame(), None);
}

#[tokio::test]
async fn pause_cancels_the_stream_on_the_emulator() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let (session, events) = connected(&fake).await;
    session.stream(108, 240);
    assert_eq!(next_event(&events).await, SessionEvent::FrameReady);
    assert_eq!(fake.active_screenshot_streams(), 1);

    session.pause();

    wait_until(|| fake.active_screenshot_streams() == 0).await;
}

#[tokio::test]
async fn stream_after_pause_starts_a_new_call() {
    let fake = FakeEmulator::start(FakeEmulatorConfig {
        frames_per_stream: 1,
        ..Default::default()
    })
    .await;
    let (session, events) = connected(&fake).await;
    session.stream(108, 240);
    assert_eq!(next_event(&events).await, SessionEvent::FrameReady);
    session.take_frame();
    session.pause();
    wait_until(|| fake.active_screenshot_streams() == 0).await;

    session.stream(108, 240);

    assert_eq!(next_event(&events).await, SessionEvent::FrameReady);
    assert_eq!(session.take_frame().unwrap().seq, 1);
    assert_eq!(fake.active_screenshot_streams(), 1);
}

#[tokio::test]
async fn resize_replaces_the_stream_with_one_at_the_new_size() {
    let fake = FakeEmulator::start(FakeEmulatorConfig {
        frames_per_stream: 1,
        ..Default::default()
    })
    .await;
    let (session, events) = connected(&fake).await;
    session.stream(108, 240);
    assert_eq!(next_event(&events).await, SessionEvent::FrameReady);
    session.take_frame();

    session.stream(54, 120);

    assert_eq!(next_event(&events).await, SessionEvent::FrameReady);
    let frame = session.take_frame().unwrap();
    assert_eq!((frame.width, frame.height), (54, 120));
    wait_until(|| fake.active_screenshot_streams() == 1).await;
}

#[tokio::test]
async fn emulator_exit_while_streaming_fails_as_exited() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let (session, events) = connected(&fake).await;
    session.stream(108, 240);
    assert_eq!(next_event(&events).await, SessionEvent::FrameReady);

    fake.stop().await;

    assert_eq!(
        next_event(&events).await,
        SessionEvent::Failed(SessionError::Exited)
    );
}

#[tokio::test]
async fn dropping_the_session_cancels_the_stream() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let (session, events) = connected(&fake).await;
    session.stream(108, 240);
    assert_eq!(next_event(&events).await, SessionEvent::FrameReady);

    drop(session);

    wait_until(|| fake.active_screenshot_streams() == 0).await;
}

#[tokio::test]
async fn keepalive_runs_only_while_streaming() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let options = SessionOptions {
        keepalive_interval: Duration::from_millis(20),
        ..fast_options()
    };
    let (session, events) = start(entry(&fake), options);
    assert_eq!(next_event(&events).await, SessionEvent::Connected);
    // The connection check is the first `getVmState`.
    let calls_when_connected = fake.recorded().get_vm_state_calls;

    session.stream(108, 240);
    wait_until(|| fake.recorded().get_vm_state_calls >= calls_when_connected + 2).await;
    session.pause();
    wait_until(|| fake.active_screenshot_streams() == 0).await;
    let calls_when_paused = fake.recorded().get_vm_state_calls;
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(calls_when_connected, 1);
    assert_eq!(fake.recorded().get_vm_state_calls, calls_when_paused);
}

#[tokio::test]
async fn fake_emulator_thread_serves_without_a_caller_runtime() {
    let fake = FakeEmulatorThread::spawn(secured_config(), 5560).unwrap();

    let (_session, events) = start(fake.entry().clone(), fast_options());

    assert_eq!(next_event(&events).await, SessionEvent::Connected);
    assert_eq!(fake.entry().adb_serial(), "emulator-5560");
}

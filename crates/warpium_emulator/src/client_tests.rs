use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;

use super::*;
use crate::{
    geometry::DisplaySize,
    proto::{
        Rotation, input_event::Type, keyboard_event::KeyEventType,
        physical_model_value::PhysicalType, rotation::SkinRotation, vm_run_state::RunState,
    },
    test_support::{FAKE_PNG, FakeEmulator, FakeEmulatorConfig, fake_pixel},
};

const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

async fn connect(fake: &FakeEmulator, token: Option<&str>) -> EmulatorClient {
    let token = token.map(GrpcToken::new);
    EmulatorClient::connect_to(
        fake.endpoint(),
        token.as_ref(),
        DEFAULT_MAX_DECODING_MESSAGE_SIZE,
    )
    .await
    .unwrap()
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT_TIMEOUT, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("condition not reached in time");
}

fn secured_config() -> FakeEmulatorConfig {
    FakeEmulatorConfig {
        token: Some("secret".to_owned()),
        ..Default::default()
    }
}

#[tokio::test]
async fn sends_bearer_token_in_authorization_header() {
    let fake = FakeEmulator::start(secured_config()).await;
    let mut client = connect(&fake, Some("secret")).await;

    client.keepalive().await.unwrap();

    assert_eq!(
        fake.recorded().authorization_headers,
        vec![Some("Bearer secret".to_owned())]
    );
}

#[tokio::test]
async fn call_without_token_is_rejected() {
    let fake = FakeEmulator::start(secured_config()).await;
    let mut client = connect(&fake, None).await;

    let result = client.keepalive().await;

    let Err(ClientError::Rpc(status)) = result else {
        panic!("expected an RPC error, got {result:?}");
    };
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}

#[tokio::test]
async fn call_with_wrong_token_is_rejected() {
    let fake = FakeEmulator::start(secured_config()).await;
    let mut client = connect(&fake, Some("guess")).await;

    let result = client.vm_state().await;

    assert!(
        matches!(result, Err(ClientError::Rpc(status)) if status.code() == tonic::Code::Unauthenticated)
    );
}

#[tokio::test]
async fn token_with_invalid_header_characters_is_rejected_before_connecting() {
    let token = GrpcToken::new("bad\ntoken");

    let result =
        EmulatorClient::connect_to("http://127.0.0.1:1".to_owned(), Some(&token), 1024).await;

    assert!(matches!(result, Err(ClientError::InvalidToken)));
}

#[tokio::test]
async fn keepalive_queries_vm_state() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let mut client = connect(&fake, None).await;

    let state = client.vm_state().await.unwrap();
    client.keepalive().await.unwrap();

    assert_eq!(state, RunState::Running);
    assert_eq!(fake.recorded().get_vm_state_calls, 2);
}

#[tokio::test]
async fn keepalive_loop_pings_repeatedly() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let client = connect(&fake, None).await;

    let keepalive = tokio::spawn(run_keepalive(client, Duration::from_millis(5)));
    wait_until(|| fake.recorded().get_vm_state_calls >= 3).await;
    keepalive.abort();

    assert!(fake.recorded().get_vm_state_calls >= 3);
}

#[tokio::test]
async fn keepalive_loop_returns_disconnect_when_emulator_stops() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let client = connect(&fake, None).await;
    fake.stop().await;

    let err = tokio::time::timeout(
        WAIT_TIMEOUT,
        run_keepalive(client, Duration::from_millis(5)),
    )
    .await
    .expect("keepalive should stop after the emulator exits");

    assert!(err.is_disconnect(), "unexpected error: {err:?}");
}

#[tokio::test]
async fn streams_native_size_frame_larger_than_default_grpc_limit() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let mut client = connect(&fake, None).await;

    let mut stream = client.stream_screenshot(0, 0).await.unwrap();
    let frame = stream.next_frame().await.unwrap().unwrap();

    assert_eq!((frame.width, frame.height), (1080, 2400));
    assert_eq!(frame.pixels.len(), 10_368_000);
    assert_eq!(frame.seq, 1);
    let offset = (2 * 1080 + 3) * 4;
    assert_eq!(&frame.pixels[offset..offset + 4], &fake_pixel(3, 2, 1));
}

#[tokio::test]
async fn frame_above_message_limit_fails_the_stream() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let mut client = EmulatorClient::connect_to(fake.endpoint(), None, 1024 * 1024)
        .await
        .unwrap();

    let mut stream = client.stream_screenshot(0, 0).await.unwrap();
    let result = stream.next_frame().await;

    assert!(
        matches!(result, Err(ClientError::Rpc(status)) if status.code() == tonic::Code::OutOfRange)
    );
}

#[tokio::test]
async fn requests_frames_at_panel_size() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let mut client = connect(&fake, None).await;

    let mut stream = client.stream_screenshot(540, 1200).await.unwrap();
    let frame = stream.next_frame().await.unwrap().unwrap();

    assert_eq!((frame.width, frame.height), (540, 1200));
    assert_eq!(frame.rotation, DisplayRotation::Portrait);
}

#[tokio::test]
async fn frame_carries_display_rotation() {
    let fake = FakeEmulator::start(FakeEmulatorConfig {
        display: DisplaySize {
            width: 100,
            height: 200,
        },
        rotation: DisplayRotation::Landscape,
        ..Default::default()
    })
    .await;
    let mut client = connect(&fake, None).await;

    let mut stream = client.stream_screenshot(0, 0).await.unwrap();
    let frame = stream.next_frame().await.unwrap().unwrap();

    assert_eq!((frame.width, frame.height), (200, 100));
    assert_eq!(frame.rotation, DisplayRotation::Landscape);
}

#[tokio::test]
async fn dropping_frame_stream_cancels_it_on_the_emulator() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let mut client = connect(&fake, None).await;
    let mut stream = client.stream_screenshot(54, 120).await.unwrap();
    stream.next_frame().await.unwrap();
    assert_eq!(fake.active_screenshot_streams(), 1);

    drop(stream);

    wait_until(|| fake.active_screenshot_streams() == 0).await;
}

#[tokio::test]
async fn pump_keeps_latest_frame_and_notifies_once_per_take() {
    let fake = FakeEmulator::start(FakeEmulatorConfig {
        frames_per_stream: 3,
        ..Default::default()
    })
    .await;
    let mut client = connect(&fake, None).await;
    let stream = client.stream_screenshot(54, 120).await.unwrap();
    let slot = Arc::new(FrameSlot::new());
    let notifications = Arc::new(AtomicUsize::new(0));

    let pump = tokio::spawn({
        let slot = slot.clone();
        let notifications = notifications.clone();
        async move {
            pump_frames(stream, &slot, || {
                notifications.fetch_add(1, Ordering::SeqCst);
            })
            .await
        }
    });
    wait_until(|| slot.dropped_frames() == 2).await;
    pump.abort();

    assert_eq!(slot.take().map(|frame| frame.seq), Some(3));
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
}

#[test]
fn image_with_wrong_pixel_count_is_malformed() {
    let image = Image {
        format: Some(ImageFormat {
            width: 2,
            height: 2,
            ..Default::default()
        }),
        image: Bytes::from_static(&[0; 15]),
        ..Default::default()
    };

    let result = frame_from_image(image);

    assert!(matches!(
        result,
        Err(ClientError::MalformedFrame {
            width: 2,
            height: 2,
            len: 15
        })
    ));
}

#[test]
fn empty_image_is_display_off_frame() {
    let image = Image {
        format: Some(ImageFormat {
            rotation: Some(Rotation {
                rotation: SkinRotation::ReverseLandscape as i32,
                ..Default::default()
            }),
            ..Default::default()
        }),
        seq: 9,
        ..Default::default()
    };

    let frame = frame_from_image(image).unwrap();

    assert!(frame.is_display_off());
    assert_eq!(frame.rotation, DisplayRotation::ReverseLandscape);
    assert_eq!(frame.seq, 9);
}

#[tokio::test]
async fn input_events_arrive_in_order() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let client = connect(&fake, None).await;
    let input = client.open_input_stream();

    input
        .send_mouse(DisplayPoint { x: 10, y: 20 }, true)
        .unwrap();
    input
        .send_mouse(DisplayPoint { x: 15, y: 25 }, true)
        .unwrap();
    input.send_wheel(0, -120).unwrap();
    input
        .send_mouse(DisplayPoint { x: 15, y: 25 }, false)
        .unwrap();
    input.finish().await.unwrap();

    let events: Vec<Type> = fake
        .recorded()
        .input_events
        .into_iter()
        .filter_map(|event| event.r#type)
        .collect();
    assert_eq!(
        events,
        vec![
            Type::MouseEvent(MouseEvent {
                x: 10,
                y: 20,
                buttons: 1,
                display: 0
            }),
            Type::MouseEvent(MouseEvent {
                x: 15,
                y: 25,
                buttons: 1,
                display: 0
            }),
            Type::WheelEvent(WheelEvent {
                dx: 0,
                dy: -120,
                ..Default::default()
            }),
            Type::MouseEvent(MouseEvent {
                x: 15,
                y: 25,
                buttons: 0,
                display: 0
            }),
        ]
    );
}

#[tokio::test]
async fn input_send_fails_once_emulator_rejects_the_stream() {
    let fake = FakeEmulator::start(secured_config()).await;
    let client = connect(&fake, None).await;
    let input = client.open_input_stream();

    let result = input.finish().await;

    assert!(
        matches!(result, Err(ClientError::Rpc(status)) if status.code() == tonic::Code::Unauthenticated)
    );
}

#[tokio::test]
async fn button_press_sends_w3c_keypress() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let mut client = connect(&fake, None).await;

    client.press_button(DeviceButton::Back).await.unwrap();
    client.press_button(DeviceButton::VolumeUp).await.unwrap();

    let keys: Vec<(String, i32)> = fake
        .recorded()
        .keys
        .into_iter()
        .map(|key| (key.key, key.event_type))
        .collect();
    assert_eq!(
        keys,
        vec![
            ("GoBack".to_owned(), KeyEventType::Keypress as i32),
            ("AudioVolumeUp".to_owned(), KeyEventType::Keypress as i32),
        ]
    );
}

#[tokio::test]
async fn rotation_sets_physical_model_z_angle() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let mut client = connect(&fake, None).await;

    client
        .set_rotation(DisplayRotation::ReverseLandscape)
        .await
        .unwrap();

    let recorded = fake.recorded().physical_models;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].target, PhysicalType::Rotation as i32);
    assert_eq!(
        recorded[0].value.as_ref().map(|value| value.data.clone()),
        Some(vec![0.0, 0.0, -90.0])
    );
}

#[tokio::test]
async fn screenshot_returns_png_bytes() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let mut client = connect(&fake, None).await;

    let png = client.screenshot_png().await.unwrap();

    assert_eq!(png.as_ref(), FAKE_PNG);
}

#[tokio::test]
async fn shutdown_requests_vm_shutdown() {
    let fake = FakeEmulator::start(FakeEmulatorConfig::default()).await;
    let mut client = connect(&fake, None).await;

    client.shutdown().await.unwrap();

    let states: Vec<i32> = fake
        .recorded()
        .vm_states_set
        .into_iter()
        .map(|state| state.state)
        .collect();
    assert_eq!(states, vec![RunState::Shutdown as i32]);
}

#[tokio::test]
async fn connects_to_discovered_emulator_with_its_token() {
    let fake = FakeEmulator::start(secured_config()).await;
    let emulator =
        crate::discovery::parse_discovery_file("pid_1.ini", &fake.discovery_file_contents(5554))
            .unwrap();

    let mut client = EmulatorClient::connect(&emulator).await.unwrap();

    assert_eq!(client.vm_state().await.unwrap(), RunState::Running);
    assert_eq!(emulator.adb_serial(), "emulator-5554");
}

#[tokio::test]
async fn refuses_tls_only_endpoint() {
    let emulator = crate::discovery::parse_discovery_file(
        "pid_1.ini",
        "port.serial=5554\ngrpc.port=8554\ngrpc.server_cert=/certs/emulator.pem\n",
    )
    .unwrap();

    let result = EmulatorClient::connect(&emulator).await;

    assert!(matches!(result, Err(ClientError::TlsRequired)));
}

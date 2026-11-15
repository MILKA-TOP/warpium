use std::io::Write as _;
use std::sync::mpsc;

use super::*;
use crate::test_support::{FakeAdbServer, Next, answer_transport, length_prefixed, okay};

#[test]
fn reports_devices_with_their_properties() {
    let server = FakeAdbServer::start(|request, stream| {
        if request == "host:track-devices-l" {
            okay(stream);
            length_prefixed(
                stream,
                "emulator-5554 device transport_id:1\nR58M unauthorized usb:1-1 transport_id:2\n",
            );
            std::thread::sleep(Duration::from_secs(30));
            return Next::Close;
        }
        if let Some(next) = answer_transport(request, stream, "emulator-5554") {
            return next;
        }
        okay(stream);
        let _ = stream.write_all(b"34\n14\nGoogle\nsdk_gphone64\nPixel_7\n\n");
        Next::Close
    });
    let (sender, receiver) = mpsc::channel();

    let handle = spawn_device_tracker(server.client(), move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    let event = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    handle.stop_and_join();

    let TrackerEvent::Devices(devices) = event else {
        panic!("expected a device list, got {event:?}");
    };
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].display_name(), "Pixel 7");
    assert_eq!(
        devices[0].props.as_ref().and_then(|props| props.api_level),
        Some(34)
    );
    assert_eq!(devices[1].props, None);
}

#[test]
fn reports_an_unreachable_server() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let (sender, receiver) = mpsc::channel();

    let handle = spawn_device_tracker(AdbClient::new(port, None), move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    let event = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    handle.stop_and_join();

    assert!(matches!(event, TrackerEvent::Unavailable(_)));
}

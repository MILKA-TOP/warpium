use std::io::Write as _;

use super::*;
use crate::device::DeviceState;
use crate::test_support::{FakeAdbServer, Next, answer_transport, fail, length_prefixed, okay};

#[test]
fn send_request_prefixes_hex_length() {
    let mut out = Vec::new();

    send_request(&mut out, "host:version").unwrap();

    assert_eq!(out, b"000chost:version");
}

#[test]
fn read_status_returns_fail_message() {
    let mut input: &[u8] = b"FAIL0010device not found";

    let err = read_status(&mut input).unwrap_err();

    assert!(matches!(err, AdbError::Fail(message) if message == "device not found"));
}

#[test]
fn read_status_reports_closed_connection() {
    let mut input: &[u8] = b"OK";

    assert!(matches!(read_status(&mut input), Err(AdbError::Closed)));
}

#[test]
fn version_parses_hex() {
    let server = FakeAdbServer::start(|request, stream| {
        assert_eq!(request, "host:version");
        okay(stream);
        length_prefixed(stream, "0029");
        Next::Close
    });

    assert_eq!(server.client().version().unwrap(), 41);
}

#[test]
fn devices_lists_long_format() {
    let server = FakeAdbServer::start(|_, stream| {
        okay(stream);
        length_prefixed(
            stream,
            "emulator-5554          device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 \
             device:emu64a transport_id:1\nR58M123 unauthorized usb:1-1 transport_id:2\n",
        );
        Next::Close
    });

    let devices = server.client().devices().unwrap();

    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].serial, "emulator-5554");
    assert_eq!(devices[1].state, DeviceState::Unauthorized);
}

#[test]
fn shell_switches_transport_and_normalizes_line_endings() {
    let server = FakeAdbServer::start(|request, stream| {
        if let Some(next) = answer_transport(request, stream, "emulator-5554") {
            return next;
        }
        assert_eq!(request, "shell:getprop ro.build.version.sdk");
        okay(stream);
        let _ = stream.write_all(b"34\r\n");
        Next::Close
    });

    let output = server
        .client()
        .shell("emulator-5554", "getprop ro.build.version.sdk")
        .unwrap();

    assert_eq!(output, "34\n");
    assert_eq!(
        server.requests(),
        vec![
            "host:transport:emulator-5554".to_owned(),
            "shell:getprop ro.build.version.sdk".to_owned()
        ]
    );
}

#[test]
fn shell_reports_unknown_device() {
    let server = FakeAdbServer::start(|request, stream| {
        answer_transport(request, stream, "emulator-5554").unwrap_or(Next::Close)
    });

    let err = server.client().shell("missing", "true").unwrap_err();

    assert!(matches!(err, AdbError::Fail(message) if message.contains("not found")));
}

#[test]
fn track_devices_streams_updates() {
    let server = FakeAdbServer::start(|request, stream| {
        assert_eq!(request, "host:track-devices-l");
        okay(stream);
        length_prefixed(stream, "");
        length_prefixed(stream, "emulator-5554 offline transport_id:1\n");
        length_prefixed(stream, "emulator-5554 device transport_id:1\n");
        Next::Close
    });
    let mut tracking = server.client().track_devices().unwrap();

    let first = tracking.next_list().unwrap();
    let second = tracking.next_list().unwrap();
    let third = tracking.next_list().unwrap();

    assert!(first.is_empty());
    assert_eq!(second[0].state, DeviceState::Offline);
    assert_eq!(third[0].state, DeviceState::Device);
}

#[test]
fn track_devices_falls_back_to_short_form_on_old_servers() {
    let server = FakeAdbServer::start(|request, stream| {
        if request == "host:track-devices-l" {
            fail(stream, "unknown host service");
        } else {
            okay(stream);
            length_prefixed(stream, "0123456789ABCDEF\tdevice\n");
        }
        Next::Close
    });
    let mut tracking = server.client().track_devices().unwrap();

    let devices = tracking.next_list().unwrap();

    assert_eq!(devices[0].serial, "0123456789ABCDEF");
    assert_eq!(
        server.requests(),
        vec![
            "host:track-devices-l".to_owned(),
            "host:track-devices".to_owned()
        ]
    );
}

#[test]
fn unreachable_server_without_adb_path_is_unavailable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let err = AdbClient::new(port, None).version().unwrap_err();

    assert!(matches!(err, AdbError::ServerUnavailable(_)));
}

#[test]
fn shell_quote_leaves_safe_arguments_alone() {
    assert_eq!(shell_quote("com.example.app"), "com.example.app");
}

#[test]
fn shell_quote_quotes_metacharacters() {
    assert_eq!(shell_quote("a b;rm -rf /"), "'a b;rm -rf /'");
    assert_eq!(shell_quote("it's"), r"'it'\''s'");
    assert_eq!(shell_quote(""), "''");
}

use warpium_emulator::discovery::parse_discovery_file;

use super::*;

fn emulator(avd_name: Option<&str>) -> RunningEmulator {
    let avd = avd_name
        .map(|name| format!("avd.name={name}\n"))
        .unwrap_or_default();
    parse_discovery_file(
        "pid_42.ini",
        &format!("port.serial=5554\ngrpc.port=8554\n{avd}"),
    )
    .unwrap()
}

#[test]
fn tab_label_shows_the_avd_name_with_spaces() {
    assert_eq!(
        tab_label(&emulator(Some("Pixel_8_API_35"))),
        "Pixel 8 API 35"
    );
}

#[test]
fn tab_label_falls_back_to_the_serial() {
    assert_eq!(tab_label(&emulator(None)), "emulator-5554");
}

#[test]
fn no_message_while_frames_show() {
    assert_eq!(
        status_message(&ViewerStatus::Showing, &emulator(None)),
        None
    );
}

#[test]
fn connecting_names_the_serial() {
    assert_eq!(
        status_message(&ViewerStatus::Connecting, &emulator(None)),
        Some(("Connecting to emulator-5554…".to_owned(), false))
    );
}

#[test]
fn unreachable_names_the_grpc_port() {
    assert_eq!(
        status_message(
            &ViewerStatus::Failed(SessionError::Unreachable),
            &emulator(None)
        ),
        Some((
            "Couldn't connect to the emulator's gRPC port 8554.".to_owned(),
            true
        ))
    );
}

#[test]
fn rejected_token_is_an_error_that_suggests_a_restart() {
    let message = status_message(
        &ViewerStatus::Failed(SessionError::Unauthorized),
        &emulator(None),
    );

    assert_eq!(
        message,
        Some((
            "The emulator rejected Warpium's access token. Restart the emulator, then retry."
                .to_owned(),
            true
        ))
    );
}

#[test]
fn screen_off_is_not_an_error() {
    assert_eq!(
        status_message(&ViewerStatus::ScreenOff, &emulator(None)),
        Some(("The emulator's screen is off.".to_owned(), false))
    );
}

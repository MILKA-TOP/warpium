use super::*;

#[test]
fn parses_emulator_line() {
    let devices = parse_device_list(
        "emulator-5554          device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 \
         device:emu64a transport_id:1\n",
    );

    assert_eq!(
        devices,
        vec![DeviceEntry {
            serial: "emulator-5554".to_owned(),
            state: DeviceState::Device,
            kind: DeviceKind::Emulator,
            model: Some("sdk_gphone64_arm64".to_owned()),
            product: Some("sdk_gphone64_arm64".to_owned()),
            device: Some("emu64a".to_owned()),
            transport_id: Some(1),
        }]
    );
}

#[test]
fn classifies_usb_and_wifi_devices() {
    let devices = parse_device_list(
        "R58M12345 device usb:1-1 product:beyond1 model:SM_G973F device:beyond1 transport_id:3\n\
         192.168.1.20:5555 device product:panther model:Pixel_7 device:panther transport_id:4\n\
         adb-28291FDH2001SA-vWgJpq._adb-tls-connect._tcp device product:oriole model:Pixel_6 \
         device:oriole transport_id:5\n",
    );

    let kinds: Vec<DeviceKind> = devices.iter().map(|device| device.kind).collect();
    assert_eq!(
        kinds,
        vec![DeviceKind::Usb, DeviceKind::Wifi, DeviceKind::Wifi]
    );
}

#[test]
fn parses_states_without_details() {
    let devices =
        parse_device_list("emulator-5556\toffline\nABC\tunauthorized\nDEF\tauthorizing\n");

    let states: Vec<DeviceState> = devices.into_iter().map(|device| device.state).collect();
    assert_eq!(
        states,
        vec![
            DeviceState::Offline,
            DeviceState::Unauthorized,
            DeviceState::Authorizing
        ]
    );
}

#[test]
fn parses_no_permissions_with_explanation() {
    let devices = parse_device_list(
        "0123456789 no permissions (user in plugdev group; are your udev rules wrong?); see \
         [http://developer.android.com/tools/device.html] usb:1-4 transport_id:7\n",
    );

    assert_eq!(devices[0].state, DeviceState::NoPermissions);
    assert_eq!(devices[0].transport_id, Some(7));
}

#[test]
fn skips_daemon_messages_and_headers() {
    let devices = parse_device_list(
        "* daemon not running; starting now at tcp:5037\n* daemon started successfully\n\
         List of devices attached\n\n",
    );

    assert!(devices.is_empty());
}

#[test]
fn parse_props_reads_lines_in_order() {
    let props = parse_props("34\n14\nGoogle\nPixel 7\n\nPixel_7_API_34\n");

    assert_eq!(
        props,
        DeviceProps {
            api_level: Some(34),
            release: Some("14".to_owned()),
            manufacturer: Some("Google".to_owned()),
            model: Some("Pixel 7".to_owned()),
            avd_name: Some("Pixel_7_API_34".to_owned()),
        }
    );
}

#[test]
fn parse_props_prefers_boot_avd_name() {
    let props = parse_props("34\n14\nGoogle\nsdk_gphone64\nNew_AVD\nOld_AVD\n");

    assert_eq!(props.avd_name.as_deref(), Some("New_AVD"));
}

#[test]
fn parse_props_leaves_missing_values_empty() {
    let props = parse_props("\n\n\n\n\n\n");

    assert_eq!(props, DeviceProps::default());
}

fn device(entry: &str, props: Option<DeviceProps>) -> Device {
    Device {
        entry: parse_device_list(entry).remove(0),
        props,
    }
}

#[test]
fn display_name_uses_avd_name_for_emulators() {
    let device = device(
        "emulator-5554 device model:sdk_gphone64_arm64",
        Some(DeviceProps {
            avd_name: Some("Pixel_7_API_34".to_owned()),
            ..DeviceProps::default()
        }),
    );

    assert_eq!(device.display_name(), "Pixel 7 API 34");
}

#[test]
fn display_name_combines_manufacturer_and_model() {
    let device = device(
        "R58M device model:SM_G973F",
        Some(DeviceProps {
            manufacturer: Some("samsung".to_owned()),
            model: Some("SM-G973F".to_owned()),
            ..DeviceProps::default()
        }),
    );

    assert_eq!(device.display_name(), "Samsung SM-G973F");
}

#[test]
fn display_name_avoids_repeating_the_manufacturer() {
    let device = device(
        "1A2B device model:Pixel_7",
        Some(DeviceProps {
            manufacturer: Some("Google".to_owned()),
            model: Some("Google Pixel 7".to_owned()),
            ..DeviceProps::default()
        }),
    );

    assert_eq!(device.display_name(), "Google Pixel 7");
}

#[test]
fn display_name_falls_back_to_serial() {
    let device = device("R58M unauthorized usb:1-1", None);

    assert_eq!(device.display_name(), "R58M");
}

#[test]
fn detail_shows_connection_version_and_state() {
    let online = device(
        "192.168.1.2:5555 device",
        Some(DeviceProps {
            api_level: Some(34),
            release: Some("14".to_owned()),
            ..DeviceProps::default()
        }),
    );
    let unauthorized = device("R58M unauthorized usb:1-1", None);

    assert_eq!(online.detail(), "Wi-Fi · Android 14 (API 34)");
    assert_eq!(unauthorized.detail(), "USB · unauthorized");
}

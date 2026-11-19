use super::*;

#[test]
fn buttons_map_to_emulator_w3c_key_names() {
    let keys: Vec<&str> = DeviceButton::ALL
        .iter()
        .map(|button| button.w3c_key())
        .collect();

    assert_eq!(
        keys,
        vec![
            "GoBack",
            "GoHome",
            "AppSwitch",
            "Power",
            "AudioVolumeUp",
            "AudioVolumeDown"
        ]
    );
}

#[test]
fn parses_w3c_key_name_back_to_button() {
    assert_eq!(
        DeviceButton::from_w3c_key("AudioVolumeDown"),
        Some(DeviceButton::VolumeDown)
    );
    assert_eq!(DeviceButton::from_w3c_key("Enter"), None);
}

//! Hardware buttons and the W3C key names the emulator's `sendKey` expects for them.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceButton {
    Back,
    Home,
    /// Recent apps ("Overview").
    AppSwitch,
    Power,
    VolumeUp,
    VolumeDown,
}

impl DeviceButton {
    pub const ALL: [DeviceButton; 6] = [
        Self::Back,
        Self::Home,
        Self::AppSwitch,
        Self::Power,
        Self::VolumeUp,
        Self::VolumeDown,
    ];

    pub fn w3c_key(self) -> &'static str {
        match self {
            Self::Back => "GoBack",
            Self::Home => "GoHome",
            Self::AppSwitch => "AppSwitch",
            Self::Power => "Power",
            Self::VolumeUp => "AudioVolumeUp",
            Self::VolumeDown => "AudioVolumeDown",
        }
    }

    pub fn from_w3c_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|button| button.w3c_key() == key)
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;

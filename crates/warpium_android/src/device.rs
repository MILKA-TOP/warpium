//! Devices as the adb server lists them, and the details read from their system properties.

use std::collections::HashMap;

use crate::adb::{AdbClient, AdbError};

/// A device's connection state, as adb reports it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DeviceState {
    /// Online and usable.
    Device,
    Offline,
    /// The user hasn't accepted this computer's RSA key on the device yet.
    Unauthorized,
    Authorizing,
    Connecting,
    /// The OS doesn't let adb open the USB device (udev rules on Linux).
    NoPermissions,
    Bootloader,
    Recovery,
    Sideload,
    Other(String),
}

impl DeviceState {
    fn parse(state: &str) -> Self {
        match state {
            "device" => DeviceState::Device,
            "offline" => DeviceState::Offline,
            "unauthorized" => DeviceState::Unauthorized,
            "authorizing" => DeviceState::Authorizing,
            "connecting" => DeviceState::Connecting,
            "bootloader" => DeviceState::Bootloader,
            "recovery" => DeviceState::Recovery,
            "sideload" => DeviceState::Sideload,
            other => DeviceState::Other(other.to_owned()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            DeviceState::Device => "online",
            DeviceState::Offline => "offline",
            DeviceState::Unauthorized => "unauthorized",
            DeviceState::Authorizing => "authorizing",
            DeviceState::Connecting => "connecting",
            DeviceState::NoPermissions => "no permissions",
            DeviceState::Bootloader => "bootloader",
            DeviceState::Recovery => "recovery",
            DeviceState::Sideload => "sideload",
            DeviceState::Other(state) => state,
        }
    }

    pub fn is_online(&self) -> bool {
        matches!(self, DeviceState::Device)
    }
}

/// How a device is attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    Emulator,
    Usb,
    /// adb over TCP: `adb connect host:port` or wireless debugging (mDNS).
    Wifi,
}

/// One line of `host:devices-l`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEntry {
    pub serial: String,
    pub state: DeviceState,
    pub kind: DeviceKind,
    pub model: Option<String>,
    pub product: Option<String>,
    pub device: Option<String>,
    pub transport_id: Option<u64>,
}

/// Parses the output of `host:devices-l` (or the short `host:devices`).
pub fn parse_device_list(text: &str) -> Vec<DeviceEntry> {
    text.lines().filter_map(parse_device_line).collect()
}

fn parse_device_line(line: &str) -> Option<DeviceEntry> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('*') || line.starts_with("List of devices") {
        return None;
    }
    let serial_end = line.find(char::is_whitespace)?;
    let serial = &line[..serial_end];
    let rest = line[serial_end..].trim_start();
    // "no permissions" has spaces, and is followed by an explanation in parentheses and brackets.
    let (state, details) = if let Some(after) = rest.strip_prefix("no permissions") {
        let details = after
            .rfind(']')
            .or_else(|| after.rfind(')'))
            .map_or(after, |end| &after[end + 1..]);
        (DeviceState::NoPermissions, details)
    } else {
        let state_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        (DeviceState::parse(&rest[..state_end]), &rest[state_end..])
    };
    let mut fields: HashMap<&str, &str> = HashMap::new();
    for token in details.split_whitespace() {
        if let Some((key, value)) = token.split_once(':') {
            fields.insert(key, value);
        }
    }
    let kind = if serial.starts_with("emulator-") {
        DeviceKind::Emulator
    } else if serial.contains("._adb-tls-connect.")
        || serial.contains("._adb._tcp")
        || serial
            .rsplit_once(':')
            .is_some_and(|(_, port)| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()))
    {
        DeviceKind::Wifi
    } else {
        DeviceKind::Usb
    };
    Some(DeviceEntry {
        serial: serial.to_owned(),
        state,
        kind,
        model: fields.get("model").map(|model| model.to_string()),
        product: fields.get("product").map(|product| product.to_string()),
        device: fields.get("device").map(|device| device.to_string()),
        transport_id: fields.get("transport_id").and_then(|id| id.parse().ok()),
    })
}

/// Details of an online device, from its system properties.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceProps {
    pub api_level: Option<u32>,
    pub release: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    /// The AVD an emulator runs.
    pub avd_name: Option<String>,
}

/// The properties [`read_props`] asks for, in the order of [`parse_props`]'s lines.
pub(crate) const PROPS: [&str; 6] = [
    "ro.build.version.sdk",
    "ro.build.version.release",
    "ro.product.manufacturer",
    "ro.product.model",
    "ro.boot.qemu.avd_name",
    "ro.kernel.qemu.avd_name",
];

/// The shell command that prints [`PROPS`], one per line.
pub fn props_command() -> String {
    PROPS
        .iter()
        .map(|prop| format!("getprop {prop}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// Parses the output of [`props_command`].
pub fn parse_props(output: &str) -> DeviceProps {
    let mut lines = output.lines().map(str::trim);
    let mut next = || {
        lines
            .next()
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let api_level = next().and_then(|sdk| sdk.parse().ok());
    let release = next();
    let manufacturer = next();
    let model = next();
    // Newer emulators set `ro.boot.qemu.avd_name`, older ones `ro.kernel.qemu.avd_name`.
    let avd_name = next().or_else(next);
    DeviceProps {
        api_level,
        release,
        manufacturer,
        model,
        avd_name,
    }
}

/// Reads the [`DeviceProps`] of an online device.
pub fn read_props(client: &AdbClient, serial: &str) -> Result<DeviceProps, AdbError> {
    client
        .shell(serial, &props_command())
        .map(|output| parse_props(&output))
}

/// A device with its details, as shown in the device picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub entry: DeviceEntry,
    /// Read once the device is online.
    pub props: Option<DeviceProps>,
}

impl Device {
    pub fn serial(&self) -> &str {
        &self.entry.serial
    }

    /// A human name: the AVD for emulators, else manufacturer and model, else the serial.
    pub fn display_name(&self) -> String {
        let props = self.props.as_ref();
        if let Some(avd) = props.and_then(|props| props.avd_name.as_deref()) {
            return avd.replace('_', " ");
        }
        let model = props.and_then(|props| props.model.clone()).or_else(|| {
            self.entry
                .model
                .as_ref()
                .map(|model| model.replace('_', " "))
        });
        let manufacturer = props.and_then(|props| props.manufacturer.as_deref());
        match (manufacturer, model) {
            (Some(manufacturer), Some(model))
                if !model
                    .to_lowercase()
                    .starts_with(&manufacturer.to_lowercase()) =>
            {
                format!("{} {model}", capitalize(manufacturer))
            }
            (_, Some(model)) => model,
            (_, None) => self.entry.serial.clone(),
        }
    }

    /// The second line in the picker: connection, Android version and state.
    pub fn detail(&self) -> String {
        let kind = match self.entry.kind {
            DeviceKind::Emulator => "Emulator",
            DeviceKind::Usb => "USB",
            DeviceKind::Wifi => "Wi-Fi",
        };
        let mut parts = vec![kind.to_owned()];
        if let Some(props) = &self.props {
            match (&props.release, props.api_level) {
                (Some(release), Some(api)) => parts.push(format!("Android {release} (API {api})")),
                (None, Some(api)) => parts.push(format!("API {api}")),
                (Some(release), None) => parts.push(format!("Android {release}")),
                (None, None) => {}
            }
        }
        if !self.entry.state.is_online() {
            parts.push(self.entry.state.label().to_owned());
        }
        parts.join(" · ")
    }
}

pub(crate) fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
#[path = "device_tests.rs"]
mod tests;

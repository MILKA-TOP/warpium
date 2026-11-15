//! Android Virtual Devices: listing, launching an emulator in its own window and waiting for it
//! to boot.

use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::Duration;

use command::blocking::Command;
use instant::Instant;

use crate::adb::AdbClient;
use crate::device::{DeviceKind, PROPS, parse_props, props_command};
use crate::sdk::property;

/// An AVD, from its `.ini` files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Avd {
    /// The id `emulator -avd` takes.
    pub name: String,
    /// `avd.ini.displayname`, when set.
    pub display_name: Option<String>,
    pub api_level: Option<u32>,
}

impl Avd {
    pub fn label(&self) -> String {
        let name = self
            .display_name
            .clone()
            .unwrap_or_else(|| self.name.replace('_', " "));
        match self.api_level {
            Some(api) => format!("{name} (API {api})"),
            None => name,
        }
    }
}

/// The directories that may hold AVDs, in the emulator's order: `ANDROID_AVD_HOME`,
/// `ANDROID_USER_HOME/avd`, `ANDROID_SDK_HOME/.android/avd` and `~/.android/avd`.
pub fn avd_dirs_from_env() -> Vec<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let mut dirs = Vec::new();
    if let Some(dir) = var("ANDROID_AVD_HOME") {
        dirs.push(dir);
    }
    if let Some(dir) = var("ANDROID_USER_HOME") {
        dirs.push(dir.join("avd"));
    }
    if let Some(dir) = var("ANDROID_SDK_HOME") {
        dirs.push(dir.join(".android").join("avd"));
    }
    if let Some(home) = std::env::home_dir() {
        dirs.push(home.join(".android").join("avd"));
    }
    dirs
}

/// Lists the AVDs in `dirs` by reading `<name>.ini` and `<name>.avd/config.ini`, without starting
/// the emulator binary. Sorted by label; a name found in an earlier directory wins.
pub fn list_avds(dirs: &[PathBuf]) -> Vec<Avd> {
    let mut avds: Vec<Avd> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "ini") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if avds.iter().any(|avd| avd.name == name) {
                continue;
            }
            let Ok(ini) = std::fs::read_to_string(&path) else {
                continue;
            };
            let avd_dir = property(&ini, "path")
                .map(PathBuf::from)
                .filter(|path| path.is_dir())
                .unwrap_or_else(|| dir.join(format!("{name}.avd")));
            let config = std::fs::read_to_string(avd_dir.join("config.ini")).unwrap_or_default();
            let api_level = property(&ini, "target")
                .and_then(|target| api_from_target(&target))
                .or_else(|| {
                    property(&config, "image.sysdir.1").and_then(|dir| api_from_target(&dir))
                });
            avds.push(Avd {
                name: name.to_owned(),
                display_name: property(&config, "avd.ini.displayname")
                    .filter(|name| !name.is_empty()),
                api_level,
            });
        }
    }
    avds.sort_by_key(|avd| avd.label().to_lowercase());
    avds
}

/// The API level in `android-34` or `system-images/android-34/google_apis/arm64-v8a/`.
fn api_from_target(target: &str) -> Option<u32> {
    target.split(['/', '\\']).find_map(|part| {
        let digits = part.strip_prefix("android-")?;
        let end = digits
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(digits.len());
        digits[..end].parse().ok()
    })
}

/// Parses `emulator -list-avds`, dropping the log lines newer emulators print to stdout.
pub fn parse_list_avds(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && line
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        })
        .map(str::to_owned)
        .collect()
}

/// Lists AVDs with `emulator -list-avds`, for setups where the `.ini` files aren't where
/// [`avd_dirs_from_env`] looks. Blocks while the emulator binary runs.
pub fn list_avds_with_emulator(emulator: &Path) -> std::io::Result<Vec<String>> {
    let output = Command::new(emulator)
        .arg("-list-avds")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    Ok(parse_list_avds(&String::from_utf8_lossy(&output.stdout)))
}

/// Starts the emulator for `avd` in its own window. It keeps running after Warpium quits; wait
/// on the returned child (off the UI thread) so it is reaped when it exits.
pub fn launch_avd(emulator: &Path, avd: &str) -> std::io::Result<Child> {
    let mut command = Command::new(emulator);
    command
        .arg("-avd")
        .arg(avd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.spawn()
}

#[derive(Debug, PartialEq, Eq)]
pub enum BootError {
    /// The emulator process exited before the device booted.
    EmulatorExited(String),
    TimedOut,
    Cancelled,
}

impl std::fmt::Display for BootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootError::EmulatorExited(status) => {
                write!(f, "the emulator exited before it booted ({status})")
            }
            BootError::TimedOut => write!(f, "the emulator didn't finish booting in time"),
            BootError::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for BootError {}

/// Waits until an emulator running `avd` is online and reports `sys.boot_completed`, and returns
/// its serial. Polls the adb server every `poll` (a second is fine). `emulator` is the process
/// from [`launch_avd`], if Warpium started it, so an early exit is noticed; `cancelled` stops the
/// wait. Blocks, so run it on a background thread.
pub fn wait_for_avd_boot(
    client: &AdbClient,
    avd: &str,
    mut emulator: Option<&mut Child>,
    timeout: Duration,
    poll: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, BootError> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancelled() {
            return Err(BootError::Cancelled);
        }
        if let Some(child) = emulator.as_deref_mut()
            && let Ok(Some(status)) = child.try_wait()
        {
            return Err(BootError::EmulatorExited(status.to_string()));
        }
        if let Some(serial) = booted_emulator_running(client, avd) {
            return Ok(serial);
        }
        if Instant::now() >= deadline {
            return Err(BootError::TimedOut);
        }
        std::thread::sleep(poll);
    }
}

/// The serial of an online, booted emulator running `avd`.
fn booted_emulator_running(client: &AdbClient, avd: &str) -> Option<String> {
    let devices = client.devices().ok()?;
    devices
        .iter()
        .filter(|entry| entry.kind == DeviceKind::Emulator && entry.state.is_online())
        .find_map(|entry| {
            let command = format!("{};getprop sys.boot_completed", props_command());
            let output = client.shell(&entry.serial, &command).ok()?;
            let props = parse_props(&output);
            let booted = output.lines().nth(PROPS.len()).map(str::trim) == Some("1");
            (props.avd_name.as_deref() == Some(avd) && booted).then(|| entry.serial.clone())
        })
}

/// The serial of an online emulator already running `avd`, booted or not.
pub fn running_emulator_for_avd(client: &AdbClient, avd: &str) -> Option<String> {
    let devices = client.devices().ok()?;
    devices
        .iter()
        .filter(|entry| entry.kind == DeviceKind::Emulator && entry.state.is_online())
        .find_map(|entry| {
            let output = client.shell(&entry.serial, &props_command()).ok()?;
            (parse_props(&output).avd_name.as_deref() == Some(avd)).then(|| entry.serial.clone())
        })
}

#[cfg(test)]
#[path = "avd_tests.rs"]
mod tests;

//! Finds running emulators through the `pid_<pid>.ini` registration files the emulator writes
//! on start (the same catalog Android Studio reads), and maps them to adb serials.

use std::{
    collections::HashMap,
    fmt, fs, io,
    path::{Path, PathBuf},
};

/// Overrides the discovery directory, for tests and debugging.
pub const DISCOVERY_DIR_ENV: &str = "WARPIUM_EMULATOR_DISCOVERY_DIR";

const REGISTRATION_SUBDIR: [&str; 2] = ["avd", "running"];
const FILE_PREFIX: &str = "pid_";
const FILE_SUFFIX: &str = ".ini";
const EMBEDDED_LAUNCH_FLAG: &str = "-qt-hide-window";

/// The static gRPC bearer token from a discovery file. Its `Debug` output is redacted so it can't
/// leak into logs.
#[derive(Clone, PartialEq, Eq)]
pub struct GrpcToken(String);

impl GrpcToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for GrpcToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GrpcToken(<redacted>)")
    }
}

/// A running emulator as described by its discovery file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningEmulator {
    pub pid: u32,
    /// Telnet console port (`port.serial`); the adb serial is `emulator-<console_port>`.
    pub console_port: u16,
    pub adb_port: Option<u16>,
    pub grpc_port: u16,
    /// Absent when the emulator was started with `-grpc <port>` and no token flag, in which case
    /// the endpoint is unauthenticated.
    pub grpc_token: Option<GrpcToken>,
    /// The endpoint requires TLS (`grpc.server_cert`), which this client does not support.
    pub grpc_uses_tls: bool,
    pub avd_name: Option<String>,
    pub avd_dir: Option<PathBuf>,
    pub emulator_version: Option<String>,
    pub command_line: Option<String>,
}

impl RunningEmulator {
    pub fn adb_serial(&self) -> String {
        format!("emulator-{}", self.console_port)
    }

    /// The emulator binds loopback only (IPv4, falling back to IPv6), so `localhost` reaches it
    /// either way.
    pub fn grpc_endpoint(&self) -> String {
        format!("http://localhost:{}", self.grpc_port)
    }

    /// Whether the emulator runs without its own window, as Android Studio and Warpium's
    /// embedded mode launch it.
    pub fn is_headless_window(&self) -> bool {
        self.command_line.as_deref().is_some_and(|cmdline| {
            cmdline
                .split_whitespace()
                .any(|arg| arg == EMBEDDED_LAUNCH_FLAG)
        })
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("not an emulator discovery file name")]
    NotDiscoveryFile,
    #[error("discovery file has no `{0}`")]
    MissingKey(&'static str),
    #[error("discovery file has an invalid `{key}`: {value:?}")]
    InvalidValue { key: &'static str, value: String },
}

/// Extracts the pid from a `pid_<pid>.ini` file name.
pub fn pid_from_file_name(file_name: &str) -> Option<u32> {
    file_name
        .strip_prefix(FILE_PREFIX)?
        .strip_suffix(FILE_SUFFIX)?
        .parse()
        .ok()
}

/// Parses one discovery file. Entries without a gRPC or console port are rejected because
/// Warpium can neither connect to nor identify them; every other key is optional.
pub fn parse_discovery_file(
    file_name: &str,
    contents: &str,
) -> Result<RunningEmulator, ParseError> {
    let pid = pid_from_file_name(file_name).ok_or(ParseError::NotDiscoveryFile)?;
    let properties: HashMap<&str, &str> = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with(';'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect();
    let non_empty = |key: &str| {
        properties
            .get(key)
            .copied()
            .filter(|value| !value.is_empty())
    };
    let port = |key: &'static str| -> Result<Option<u16>, ParseError> {
        non_empty(key)
            .map(|value| {
                value
                    .parse::<u16>()
                    .ok()
                    .filter(|port| *port != 0)
                    .ok_or_else(|| ParseError::InvalidValue {
                        key,
                        value: value.to_owned(),
                    })
            })
            .transpose()
    };

    Ok(RunningEmulator {
        pid,
        console_port: port("port.serial")?.ok_or(ParseError::MissingKey("port.serial"))?,
        adb_port: port("port.adb")?,
        grpc_port: port("grpc.port")?.ok_or(ParseError::MissingKey("grpc.port"))?,
        grpc_token: non_empty("grpc.token").map(GrpcToken::new),
        grpc_uses_tls: non_empty("grpc.server_cert").is_some(),
        avd_name: non_empty("avd.name").map(str::to_owned),
        avd_dir: non_empty("avd.dir").map(PathBuf::from),
        emulator_version: non_empty("emulator.version").map(str::to_owned),
        command_line: non_empty("cmdline").map(str::to_owned),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostOs {
    MacOs,
    Linux,
    Windows,
}

impl HostOs {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// The inputs that decide where the emulator writes its discovery files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryEnv {
    pub os: HostOs,
    pub override_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub xdg_runtime_dir: Option<PathBuf>,
    pub uid: Option<u32>,
    pub local_app_data: Option<PathBuf>,
    pub temp_dir: Option<PathBuf>,
    pub user: Option<String>,
}

impl DiscoveryEnv {
    pub fn current() -> Self {
        let env_path = |key: &str| {
            std::env::var_os(key)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self {
            os: HostOs::current(),
            override_dir: env_path(DISCOVERY_DIR_ENV),
            home: dirs::home_dir(),
            xdg_runtime_dir: env_path("XDG_RUNTIME_DIR"),
            uid: current_uid(),
            local_app_data: env_path("LOCALAPPDATA"),
            temp_dir: Some(std::env::temp_dir()),
            user: std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .ok()
                .filter(|user| !user.is_empty()),
        }
    }
}

/// The directory holding `pid_*.ini` files, following Android Studio's
/// `RunningEmulatorCatalog`: macOS `~/Library/Caches/TemporaryItems`, Windows
/// `%LOCALAPPDATA%/Temp`, Linux the first existing of `$XDG_RUNTIME_DIR`, `/run/user/<uid>`,
/// `~/.android`, falling back to `$TMP/android-<user>`; each followed by `avd/running`.
pub fn discovery_dir(env: &DiscoveryEnv, dir_exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    if let Some(dir) = &env.override_dir {
        return Some(dir.clone());
    }
    let base = match env.os {
        HostOs::MacOs => env.home.as_ref()?.join("Library/Caches/TemporaryItems"),
        HostOs::Windows => env.local_app_data.as_ref()?.join("Temp"),
        HostOs::Linux => {
            let candidates = [
                env.xdg_runtime_dir.clone(),
                env.uid.map(|uid| PathBuf::from(format!("/run/user/{uid}"))),
                env.home.as_ref().map(|home| home.join(".android")),
            ];
            match candidates.into_iter().flatten().find(|dir| dir_exists(dir)) {
                Some(dir) => dir,
                None => env.temp_dir.as_ref()?.join(format!(
                    "android-{}",
                    env.user.as_deref().unwrap_or("unknown")
                )),
            }
        }
    };
    Some(
        REGISTRATION_SUBDIR
            .iter()
            .fold(base, |dir, part| dir.join(part)),
    )
}

/// Reads every discovery file in `dir` and keeps the parseable entries whose process is alive,
/// sorted by console port. A missing directory means no emulator is running.
pub fn scan_dir(dir: &Path, is_alive: impl Fn(u32) -> bool) -> Vec<RunningEmulator> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            log::warn!("[Emulator discovery] Failed to read the discovery directory: {err}");
            return Vec::new();
        }
    };
    let mut emulators: Vec<RunningEmulator> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_name = entry.file_name().into_string().ok()?;
            let pid = pid_from_file_name(&file_name)?;
            if !is_alive(pid) {
                log::debug!("[Emulator discovery] Skipping stale entry pid={pid}");
                return None;
            }
            let contents = fs::read_to_string(entry.path())
                .inspect_err(|err| {
                    log::debug!("[Emulator discovery] Failed to read entry pid={pid}: {err}")
                })
                .ok()?;
            parse_discovery_file(&file_name, &contents)
                .inspect_err(|err| {
                    log::debug!("[Emulator discovery] Ignoring entry pid={pid}: {err}")
                })
                .ok()
        })
        .collect();
    emulators.sort_by_key(|emulator| emulator.console_port);
    emulators
}

/// Lists the emulators running on this machine.
pub fn running_emulators() -> Vec<RunningEmulator> {
    match discovery_dir(&DiscoveryEnv::current(), Path::is_dir) {
        Some(dir) => scan_dir(&dir, process_is_alive),
        None => {
            log::warn!("[Emulator discovery] Could not determine the discovery directory");
            Vec::new()
        }
    }
}

/// Finds the emulator behind an adb serial such as `emulator-5554`.
pub fn find_by_serial<'a>(
    emulators: &'a [RunningEmulator],
    serial: &str,
) -> Option<&'a RunningEmulator> {
    let console_port: u16 = serial.strip_prefix("emulator-")?.parse().ok()?;
    emulators
        .iter()
        .find(|emulator| emulator.console_port == console_port)
}

pub fn find_by_pid(emulators: &[RunningEmulator], pid: u32) -> Option<&RunningEmulator> {
    emulators.iter().find(|emulator| emulator.pid == pid)
}

#[cfg(unix)]
fn current_uid() -> Option<u32> {
    // SAFETY: getuid has no preconditions and cannot fail.
    Some(unsafe { libc::getuid() })
}

#[cfg(not(unix))]
fn current_uid() -> Option<u32> {
    None
}

/// Whether a process with this pid exists.
#[cfg(unix)]
pub fn process_is_alive(pid: u32) -> bool {
    // pid 0 and negative values address process groups rather than one process.
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs only the existence and permission check.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Whether a process with this pid exists.
#[cfg(windows)]
pub fn process_is_alive(pid: u32) -> bool {
    use windows::Win32::{
        Foundation::{CloseHandle, STILL_ACTIVE},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };

    if pid == 0 {
        return false;
    }
    // SAFETY: the handle is checked before use and closed exactly once.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut exit_code = 0u32;
        let alive = GetExitCodeProcess(handle, &mut exit_code).is_ok()
            && exit_code == STILL_ACTIVE.0 as u32;
        let _ = CloseHandle(handle);
        alive
    }
}

#[cfg(not(any(unix, windows)))]
pub fn process_is_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;

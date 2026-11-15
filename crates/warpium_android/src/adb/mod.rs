//! A blocking client for the adb server (`127.0.0.1:5037`), speaking its socket protocol
//! directly instead of running the `adb` binary for every request. Every call does network IO, so
//! call it from a background thread, never the UI thread.

pub mod protocol;

use std::fmt;
use std::io::{self, Read};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use command::blocking::Command;

use crate::device::{DeviceEntry, parse_device_list};
use protocol::{read_length_prefixed, read_status, send_request};

/// The adb server's default port.
pub const DEFAULT_PORT: u16 = 5037;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum AdbError {
    Io(io::Error),
    /// The server answered `FAIL` with this message.
    Fail(String),
    /// The server sent something this client doesn't understand.
    Protocol(String),
    /// The server closed the connection.
    Closed,
    /// No server is listening and none could be started.
    ServerUnavailable(String),
}

impl fmt::Display for AdbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdbError::Io(err) => write!(f, "adb connection error: {err}"),
            AdbError::Fail(message) => write!(f, "adb: {message}"),
            AdbError::Protocol(message) => write!(f, "unexpected adb response: {message}"),
            AdbError::Closed => write!(f, "the adb server closed the connection"),
            AdbError::ServerUnavailable(message) => {
                write!(f, "the adb server isn't running: {message}")
            }
        }
    }
}

impl std::error::Error for AdbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AdbError::Io(err) => Some(err),
            AdbError::Fail(_)
            | AdbError::Protocol(_)
            | AdbError::Closed
            | AdbError::ServerUnavailable(_) => None,
        }
    }
}

impl From<io::Error> for AdbError {
    fn from(err: io::Error) -> Self {
        AdbError::Io(err)
    }
}

/// The adb server port: `ANDROID_ADB_SERVER_PORT` when set, as the `adb` binary does.
pub fn server_port_from_env() -> u16 {
    std::env::var("ANDROID_ADB_SERVER_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// A handle to one adb server. Cheap to clone; each request opens its own connection, as the
/// `adb` binary does.
#[derive(Clone, Debug)]
pub struct AdbClient {
    addr: SocketAddr,
    /// The `adb` binary, used only to start the server when it isn't running.
    adb_path: Option<PathBuf>,
}

impl AdbClient {
    pub fn new(port: u16, adb_path: Option<PathBuf>) -> Self {
        Self {
            addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)),
            adb_path,
        }
    }

    pub fn adb_path(&self) -> Option<&PathBuf> {
        self.adb_path.as_ref()
    }

    fn connect(&self) -> io::Result<TcpStream> {
        let stream = TcpStream::connect_timeout(&self.addr, CONNECT_TIMEOUT)?;
        stream.set_nodelay(true)?;
        Ok(stream)
    }

    /// Connects to the server, starting it with `adb start-server` if nothing is listening.
    fn connect_or_start(&self) -> Result<TcpStream, AdbError> {
        match self.connect() {
            Ok(stream) => Ok(stream),
            Err(first_err) => {
                let Some(adb) = &self.adb_path else {
                    return Err(AdbError::ServerUnavailable(first_err.to_string()));
                };
                log::info!("[Android] Starting the adb server");
                let status = Command::new(adb)
                    .arg("start-server")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map_err(|err| {
                        AdbError::ServerUnavailable(format!("couldn't run adb start-server: {err}"))
                    })?;
                if !status.success() {
                    return Err(AdbError::ServerUnavailable(format!(
                        "adb start-server exited with {status}"
                    )));
                }
                self.connect()
                    .map_err(|err| AdbError::ServerUnavailable(err.to_string()))
            }
        }
    }

    /// Sends a `host:` request and reads its length-prefixed answer.
    pub fn host_query(&self, request: &str) -> Result<String, AdbError> {
        let mut stream = self.connect_or_start()?;
        send_request(&mut stream, request)?;
        read_status(&mut stream)?;
        read_length_prefixed(&mut stream)
    }

    /// The server's protocol version.
    pub fn version(&self) -> Result<u32, AdbError> {
        let hex = self.host_query("host:version")?;
        u32::from_str_radix(hex.trim(), 16)
            .map_err(|_| AdbError::Protocol(format!("invalid version {hex:?}")))
    }

    /// The connected devices, with model and transport details.
    pub fn devices(&self) -> Result<Vec<DeviceEntry>, AdbError> {
        Ok(parse_device_list(&self.host_query("host:devices-l")?))
    }

    /// Starts tracking devices: the server sends the full list now and again on every change.
    pub fn track_devices(&self) -> Result<DeviceTracking, AdbError> {
        let mut stream = self.connect_or_start()?;
        send_request(&mut stream, "host:track-devices-l")?;
        match read_status(&mut stream) {
            Ok(()) => Ok(DeviceTracking { stream }),
            // Servers older than platform-tools 30 only know the short form.
            Err(AdbError::Fail(_)) => {
                let mut stream = self.connect_or_start()?;
                send_request(&mut stream, "host:track-devices")?;
                read_status(&mut stream)?;
                Ok(DeviceTracking { stream })
            }
            Err(err) => Err(err),
        }
    }

    /// Opens `service` (such as `shell:ls` or `exec:logcat -B`) on the device with `serial` and
    /// returns the raw stream of its output.
    pub fn open_service(&self, serial: &str, service: &str) -> Result<TcpStream, AdbError> {
        let mut stream = self.connect_or_start()?;
        send_request(&mut stream, &format!("host:transport:{serial}"))?;
        read_status(&mut stream)?;
        send_request(&mut stream, service)?;
        read_status(&mut stream)?;
        Ok(stream)
    }

    /// Runs a shell command on the device and returns its output, with `\r\n` (from devices that
    /// give `shell:` a terminal) normalized to `\n`.
    pub fn shell(&self, serial: &str, command: &str) -> Result<String, AdbError> {
        let mut stream = self.open_service(serial, &format!("shell:{command}"))?;
        let mut output = Vec::new();
        stream.read_to_end(&mut output)?;
        Ok(String::from_utf8_lossy(&output).replace("\r\n", "\n"))
    }

    /// Runs a command on the device without a terminal, so binary output stays intact (Android 5+).
    pub fn exec(&self, serial: &str, command: &str) -> Result<TcpStream, AdbError> {
        self.open_service(serial, &format!("exec:{command}"))
    }
}

/// An open `host:track-devices` connection.
#[derive(Debug)]
pub struct DeviceTracking {
    stream: TcpStream,
}

impl DeviceTracking {
    /// Blocks until the server sends the next device list.
    pub fn next_list(&mut self) -> Result<Vec<DeviceEntry>, AdbError> {
        Ok(parse_device_list(&read_length_prefixed(&mut self.stream)?))
    }

    /// A handle that can shut the connection down from another thread, unblocking
    /// [`Self::next_list`].
    pub fn shutdown_handle(&self) -> io::Result<TcpStream> {
        self.stream.try_clone()
    }
}

/// Quotes `arg` for the device's `sh`, so a value such as a package name can't add commands.
pub fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/:=@%+,".contains(c))
    {
        return arg.to_owned();
    }
    format!("'{}'", arg.replace('\'', r"'\''"))
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

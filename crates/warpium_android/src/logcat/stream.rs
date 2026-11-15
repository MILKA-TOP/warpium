//! Background threads that stream logcat from a device and poll an app's processes.

use std::io::{self, Read};
use std::time::Duration;

use instant::Instant;

use super::binary::BinaryParser;
use super::entry::LogEntry;
use super::process::{PS_COMMAND, parse_ps};
use crate::adb::AdbClient;
use crate::worker::WorkerHandle;

/// How often parsed entries are handed over at most, which bounds UI updates under a fast stream.
const FLUSH_INTERVAL: Duration = Duration::from_millis(50);
/// Entries handed over at once at most, so a burst (the initial backlog) arrives in pieces.
const MAX_BATCH: usize = 4096;
const READ_BUFFER: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogcatOptions {
    /// Recent entries to start with (`-T`), before following new ones.
    pub backlog: usize,
}

impl Default for LogcatOptions {
    fn default() -> Self {
        Self { backlog: 5000 }
    }
}

/// The device-side command: binary output of the main, system and crash buffers (repeated `-b`,
/// since Android 6 and older take no comma-separated list).
pub fn logcat_command(options: &LogcatOptions) -> String {
    format!(
        "logcat -B -b main -b system -b crash -T {}",
        options.backlog.max(1)
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum LogcatEvent {
    Entries(Vec<LogEntry>),
    /// The stream ended: the device went away, or an error (its message).
    Ended(Option<String>),
}

/// Streams logcat from `serial` on a background thread, calling `sink` (on that thread) with
/// batches of entries. Dropping the handle stops it.
pub fn spawn_logcat(
    client: AdbClient,
    serial: String,
    options: LogcatOptions,
    mut sink: impl FnMut(LogcatEvent) + Send + 'static,
) -> io::Result<WorkerHandle> {
    WorkerHandle::spawn("adb-logcat", move |control| {
        let mut stream = match client.exec(&serial, &logcat_command(&options)) {
            Ok(stream) => stream,
            Err(err) => {
                if !control.is_stopped() {
                    sink(LogcatEvent::Ended(Some(err.to_string())));
                }
                return;
            }
        };
        if !control.set_socket(&stream) {
            return;
        }
        // A short timeout lets a partial batch go out when the stream goes quiet.
        if let Err(err) = stream.set_read_timeout(Some(FLUSH_INTERVAL)) {
            sink(LogcatEvent::Ended(Some(err.to_string())));
            return;
        }
        let mut parser = BinaryParser::new();
        let mut buffer = vec![0u8; READ_BUFFER];
        let mut pending: Vec<LogEntry> = Vec::new();
        let mut last_flush = Instant::now();
        let end = loop {
            match stream.read(&mut buffer) {
                Ok(0) => break None,
                Ok(read) => {
                    if let Err(err) = parser.push(&buffer[..read], &mut pending) {
                        break Some(err.to_string());
                    }
                    if pending.len() >= MAX_BATCH || last_flush.elapsed() >= FLUSH_INTERVAL {
                        flush(&mut pending, &mut last_flush, &mut sink);
                    }
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    if !pending.is_empty() {
                        flush(&mut pending, &mut last_flush, &mut sink);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => break Some(err.to_string()),
            }
            if control.is_stopped() {
                return;
            }
        };
        flush(&mut pending, &mut last_flush, &mut sink);
        if !control.is_stopped() {
            sink(LogcatEvent::Ended(end));
        }
    })
}

fn flush(pending: &mut Vec<LogEntry>, last_flush: &mut Instant, sink: &mut dyn FnMut(LogcatEvent)) {
    *last_flush = Instant::now();
    while !pending.is_empty() {
        let rest = pending.split_off(pending.len().min(MAX_BATCH));
        sink(LogcatEvent::Entries(std::mem::replace(pending, rest)));
    }
}

/// Polls `package`'s processes on `serial` every `interval`, calling `sink` with the
/// `(pid, process)` list whenever it changes (and once at the start). A safety net for pid
/// tracking: ActivityManager's log lines usually announce new processes first.
pub fn spawn_process_poller(
    client: AdbClient,
    serial: String,
    package: String,
    interval: Duration,
    mut sink: impl FnMut(Vec<(i32, String)>) + Send + 'static,
) -> io::Result<WorkerHandle> {
    WorkerHandle::spawn("adb-ps", move |control| {
        let mut last: Option<Vec<(i32, String)>> = None;
        loop {
            match client.shell(&serial, PS_COMMAND) {
                Ok(output) => {
                    let processes = parse_ps(&output, &package);
                    if last.as_ref() != Some(&processes) && !control.is_stopped() {
                        sink(processes.clone());
                        last = Some(processes);
                    }
                }
                Err(err) => log::debug!("[Android] Couldn't list processes: {err}"),
            }
            if !control.sleep(interval) {
                return;
            }
        }
    })
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;

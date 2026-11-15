//! Finding crashes in logcat: Java/Kotlin `FATAL EXCEPTION`s, ANRs and native crashes
//! (tombstones), each with the lines that describe it.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use warpium_android::logcat::{LogEntry, LogTime};

/// Lines kept per crash at most.
pub const MAX_CRASH_LINES: usize = 200;
/// Entries of one crash report further apart than this belong to different reports.
const MAX_GAP_SECS: i64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashKind {
    Java,
    Anr,
    Native,
}

impl CrashKind {
    pub fn label(self) -> &'static str {
        match self {
            CrashKind::Java => "Java/Kotlin crash",
            CrashKind::Anr => "ANR",
            CrashKind::Native => "native crash",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crash {
    pub kind: CrashKind,
    pub time: LogTime,
    /// The crashed process, when the report or the known processes name it.
    pub process: Option<String>,
    pub pid: Option<i32>,
    pub lines: Vec<String>,
    /// Lines beyond [`MAX_CRASH_LINES`] that were left out.
    pub omitted_lines: usize,
}

impl Crash {
    fn new(kind: CrashKind, entry: &LogEntry) -> Self {
        let mut crash = Self {
            kind,
            time: entry.time,
            process: None,
            pid: None,
            lines: Vec::new(),
            omitted_lines: 0,
        };
        crash.push_message(&entry.message);
        crash
    }

    fn push_message(&mut self, message: &str) {
        for line in message.split('\n') {
            let line = line.trim_end_matches('\r');
            if self.lines.len() < MAX_CRASH_LINES {
                self.lines.push(line.to_owned());
            } else {
                self.omitted_lines += 1;
            }
        }
    }
}

static JAVA_PROCESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Process: ([\w.:]+), PID: (\d+)").expect("the process pattern is valid")
});
static ANR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ANR in ([\w.:]+)").expect("the ANR pattern is valid"));
static ANR_PID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^PID: (\d+)").expect("the ANR pid pattern is valid"));
static TOMBSTONE_PROCESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"pid: (\d+), tid: \d+, name: .*>>> ([^ ]+) <<<")
        .expect("the tombstone pattern is valid")
});
static FATAL_SIGNAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Fatal signal .* pid (\d+) \(([^)]+)\)").expect("the signal pattern is valid")
});

/// An open multi-entry report: the logging process and when it last logged a line of it.
struct Open {
    index: usize,
    logger_pid: i32,
    last: LogTime,
}

/// The crashes in `entries` (oldest first), oldest first. `processes` names the pids the
/// reports don't.
pub fn extract_crashes<'a>(
    entries: impl IntoIterator<Item = &'a LogEntry>,
    processes: &HashMap<i32, String>,
) -> Vec<Crash> {
    let mut crashes: Vec<Crash> = Vec::new();
    let mut open_java: Option<Open> = None;
    let mut open_native: Option<Open> = None;
    // `libc` "Fatal signal" lines, dropped later when a tombstone covers the same pid.
    let mut signals: HashSet<usize> = HashSet::new();

    for entry in entries {
        match &*entry.tag {
            "AndroidRuntime" => {
                if entry.message.starts_with("FATAL EXCEPTION") {
                    open_java = Some(Open {
                        index: crashes.len(),
                        logger_pid: entry.pid,
                        last: entry.time,
                    });
                    let mut crash = Crash::new(CrashKind::Java, entry);
                    crash.pid = Some(entry.pid);
                    crashes.push(crash);
                } else if let Some(open) = continues(&mut open_java, entry) {
                    crashes[open].push_message(&entry.message);
                }
            }
            "DEBUG" => {
                if entry.message.starts_with("*** *** ***") {
                    open_native = Some(Open {
                        index: crashes.len(),
                        logger_pid: entry.pid,
                        last: entry.time,
                    });
                    crashes.push(Crash::new(CrashKind::Native, entry));
                } else if let Some(open) = continues(&mut open_native, entry) {
                    crashes[open].push_message(&entry.message);
                }
            }
            "ActivityManager" if entry.message.starts_with("ANR in ") => {
                crashes.push(Crash::new(CrashKind::Anr, entry));
            }
            "libc" if entry.message.starts_with("Fatal signal") => {
                signals.insert(crashes.len());
                let mut crash = Crash::new(CrashKind::Native, entry);
                crash.pid = Some(entry.pid);
                crashes.push(crash);
            }
            _ => {}
        }
    }

    for crash in &mut crashes {
        identify(crash);
        if crash.process.is_none()
            && let Some(pid) = crash.pid
        {
            crash.process = processes.get(&pid).cloned();
        }
    }
    let tombstone_pids: HashSet<i32> = crashes
        .iter()
        .enumerate()
        .filter(|(index, crash)| crash.kind == CrashKind::Native && !signals.contains(index))
        .filter_map(|(_, crash)| crash.pid)
        .collect();
    let mut index = 0;
    crashes.retain(|crash| {
        let keep =
            !signals.contains(&index) || crash.pid.is_none_or(|pid| !tombstone_pids.contains(&pid));
        index += 1;
        keep
    });
    crashes
}

/// The open report `entry` continues: same logging process, soon after its last line.
fn continues(open: &mut Option<Open>, entry: &LogEntry) -> Option<usize> {
    let current = open.as_mut()?;
    if current.logger_pid != entry.pid {
        return None;
    }
    if entry.time.sec - current.last.sec > MAX_GAP_SECS {
        *open = None;
        return None;
    }
    current.last = entry.time;
    Some(current.index)
}

/// Reads the crashed process and pid from the report's own lines.
fn identify(crash: &mut Crash) {
    match crash.kind {
        CrashKind::Java => {
            if let Some(captures) = crash
                .lines
                .iter()
                .find_map(|line| JAVA_PROCESS.captures(line))
            {
                crash.process = Some(captures[1].to_owned());
                crash.pid = captures[2].parse().ok();
            }
        }
        CrashKind::Anr => {
            let first = crash.lines.first().map(String::as_str).unwrap_or_default();
            crash.process = ANR.captures(first).map(|captures| captures[1].to_owned());
            crash.pid = ANR_PID
                .captures(&crash.lines.join("\n"))
                .and_then(|captures| captures[1].parse().ok());
        }
        CrashKind::Native => {
            if let Some(captures) = crash
                .lines
                .iter()
                .find_map(|line| TOMBSTONE_PROCESS.captures(line))
            {
                crash.pid = captures[1].parse().ok();
                crash.process = Some(captures[2].to_owned());
            } else if let Some(captures) = crash
                .lines
                .first()
                .and_then(|line| FATAL_SIGNAL.captures(line))
            {
                crash.pid = captures[1].parse().ok();
                crash.process = Some(captures[2].to_owned());
            }
        }
    }
}

#[cfg(test)]
#[path = "crash_tests.rs"]
mod tests;

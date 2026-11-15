//! Which processes belong to an app package, from `ps`/`pidof` output and ActivityManager's own
//! log lines, so a package filter follows the app across restarts.

/// Whether `process` is `package` or one of its `package:name` subprocesses.
pub fn process_belongs_to(process: &str, package: &str) -> bool {
    match process.strip_prefix(package) {
        Some(rest) => rest.is_empty() || rest.starts_with(':'),
        None => false,
    }
}

/// A process start or death that ActivityManager logged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessEvent {
    Started { pid: i32, process: String },
    Died { pid: i32, process: String },
}

/// Parses ActivityManager messages:
/// - `Start proc 12345:com.example/u0a123 for activity {…}` (Android 7+),
/// - `Start proc com.example for activity …: pid=12345 uid=10061 gids={…}` (older),
/// - `Process com.example (pid 12345) has died…`.
pub fn parse_activity_manager(message: &str) -> Option<ProcessEvent> {
    if let Some(rest) = message.strip_prefix("Start proc ") {
        if let Some((pid, after)) = rest.split_once(':')
            && let Ok(pid) = pid.parse()
        {
            let process = after
                .split(['/', ' '])
                .next()
                .filter(|process| !process.is_empty())?;
            return Some(ProcessEvent::Started {
                pid,
                process: process.to_owned(),
            });
        }
        let process = rest.split(' ').next()?;
        let pid = rest
            .split_once("pid=")?
            .1
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()?;
        return Some(ProcessEvent::Started {
            pid,
            process: process.to_owned(),
        });
    }
    let rest = message.strip_prefix("Process ")?;
    let (process, after) = rest.split_once(" (pid ")?;
    let (pid, after) = after.split_once(')')?;
    if !after.trim_start().starts_with("has died") {
        return None;
    }
    Some(ProcessEvent::Died {
        pid: pid.parse().ok()?,
        process: process.to_owned(),
    })
}

/// The shell command listing processes: toybox `ps -A -o PID,NAME` (Android 8+), then the old
/// toolbox `ps`, which lists everything without arguments. Both run, since toolbox `ps` treats
/// unknown flags as a name filter and still succeeds; on newer devices the plain `ps` lists only
/// the shell's own processes.
pub const PS_COMMAND: &str = "ps -A -o PID,NAME 2>/dev/null; ps 2>/dev/null";

/// Parses `ps` output (any number of sections in either format) into `package`'s processes,
/// without duplicates. Each header line gives the PID column; the name is the last column.
pub fn parse_ps(output: &str, package: &str) -> Vec<(i32, String)> {
    let mut processes: Vec<(i32, String)> = Vec::new();
    let mut pid_column = None;
    for line in output.lines() {
        let columns: Vec<&str> = line.split_whitespace().collect();
        if let Some(column) = columns.iter().position(|column| *column == "PID") {
            pid_column = Some(column);
            continue;
        }
        let Some(pid) = pid_column
            .and_then(|column| columns.get(column))
            .and_then(|pid| pid.parse().ok())
        else {
            continue;
        };
        let Some(name) = columns.last() else {
            continue;
        };
        if process_belongs_to(name, package) && !processes.iter().any(|(known, _)| *known == pid) {
            processes.push((pid, (*name).to_owned()));
        }
    }
    processes
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;

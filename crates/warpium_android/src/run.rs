//! Starting an app once Gradle has installed it: the install is noticed on the device (the
//! package's `lastUpdateTime` changes), so the Gradle build can run visibly in any terminal and
//! shell without Warpium following its output.

use std::io;
use std::time::Duration;

use instant::Instant;

use crate::adb::{AdbClient, shell_quote};
use crate::project::launch_app_command;
use crate::worker::WorkerHandle;

/// The shell command that prints the package's `lastUpdateTime` line (nothing when it isn't
/// installed).
pub fn update_time_command(package: &str) -> String {
    format!(
        "dumpsys package {} | grep -m 1 lastUpdateTime",
        shell_quote(package)
    )
}

/// The `lastUpdateTime` value in [`update_time_command`]'s output, if the package is installed.
pub fn parse_update_time(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("lastUpdateTime="))
        .map(|time| time.trim().to_owned())
        .filter(|time| !time.is_empty())
}

/// Whether `am start`/`monkey` output from [`launch_app_command`] reports a failure.
pub fn launch_failed(output: &str) -> bool {
    output.lines().any(|line| {
        let line = line.trim();
        line.starts_with("Error")
            || line.contains("No activities found")
            || line.contains("monkey aborted")
            || line.contains("Exception")
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum LaunchOutcome {
    Launched,
    /// The install wasn't seen in time (the build failed or is still running).
    TimedOut,
    /// Starting the app failed, with the device's output.
    Failed(String),
}

/// Waits (on a background thread) until `package` is (re)installed on `serial`, then starts its
/// launcher activity and calls `done`. The package's install time is read first, so start this
/// before the install begins. Dropping the handle cancels the wait.
pub fn spawn_launch_after_install(
    client: AdbClient,
    serial: String,
    package: String,
    timeout: Duration,
    poll: Duration,
    done: impl FnOnce(LaunchOutcome) + Send + 'static,
) -> io::Result<WorkerHandle> {
    WorkerHandle::spawn("adb-launch-app", move |control| {
        let read_time = || {
            client
                .shell(&serial, &update_time_command(&package))
                .ok()
                .map(|output| parse_update_time(&output))
        };
        let baseline = read_time().flatten();
        let deadline = Instant::now() + timeout;
        loop {
            if !control.sleep(poll) {
                return;
            }
            // A failed read (the device is busy installing) is retried.
            if let Some(Some(time)) = read_time()
                && baseline.as_ref() != Some(&time)
            {
                break;
            }
            if Instant::now() >= deadline {
                done(LaunchOutcome::TimedOut);
                return;
            }
        }
        if control.is_stopped() {
            return;
        }
        let outcome = match client.shell(&serial, &launch_app_command(&package)) {
            Ok(output) if launch_failed(&output) => LaunchOutcome::Failed(output.trim().to_owned()),
            Ok(_) => LaunchOutcome::Launched,
            Err(err) => LaunchOutcome::Failed(err.to_string()),
        };
        done(outcome);
    })
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;

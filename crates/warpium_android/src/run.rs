//! Starting an app once Gradle has installed it: the install is noticed on the device (the
//! package's `lastUpdateTime` changes), so the Gradle build can run visibly in any terminal and
//! shell without Warpium following its output.

use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use instant::Instant;

use crate::adb::{AdbClient, shell_quote};
use crate::project::{application_id_from_metadata, launch_app_command};
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
    /// The app with this package was started.
    Launched(String),
    /// The install wasn't seen in time (the build failed or is still running).
    TimedOut,
    /// Starting the app failed, with the device's output.
    Failed(String),
}

/// What [`spawn_launch_after_install`] waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchRequest {
    pub serial: String,
    /// The package expected to be installed.
    pub package: String,
    /// The module's `output-metadata.json` for the built variant. When the build rewrites it, its
    /// `applicationId` replaces `package`, which was only a guess from the previous build or the
    /// build script.
    pub metadata: Option<PathBuf>,
    pub timeout: Duration,
    pub poll: Duration,
}

/// Waits (on a background thread) until the requested package is (re)installed, then starts
/// its launcher activity and calls `done`. The install time is read first, so start this before
/// the install begins. Dropping the handle cancels the wait.
pub fn spawn_launch_after_install(
    client: AdbClient,
    request: LaunchRequest,
    done: impl FnOnce(LaunchOutcome) + Send + 'static,
) -> io::Result<WorkerHandle> {
    WorkerHandle::spawn("adb-launch-app", move |control| {
        let LaunchRequest {
            serial,
            mut package,
            metadata,
            timeout,
            poll,
        } = request;
        let deadline = Instant::now() + timeout;
        // `None` when the device didn't answer; `Some(None)` when the package isn't installed.
        let read_time = |package: &str| {
            client
                .shell(&serial, &update_time_command(package))
                .ok()
                .map(|output| parse_update_time(&output))
        };
        let modified = |path: &PathBuf| std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let mut metadata_modified: Option<SystemTime> = metadata.as_ref().and_then(modified);
        let mut baseline = None;
        while baseline.is_none() {
            baseline = read_time(&package);
            if baseline.is_none() && (Instant::now() >= deadline || !control.sleep(poll)) {
                return;
            }
        }
        let mut baseline = baseline.flatten();
        loop {
            if !control.sleep(poll) {
                return;
            }
            if let Some(path) = &metadata {
                let now_modified = modified(path);
                if now_modified.is_some() && now_modified != metadata_modified {
                    metadata_modified = now_modified;
                    if let Some(built) = std::fs::read_to_string(path)
                        .ok()
                        .and_then(|json| application_id_from_metadata(&json))
                        .filter(|built| *built != package)
                    {
                        // The build hasn't installed yet when it rewrites the metadata, so the
                        // new package's current install time is the baseline.
                        package = built;
                        baseline = read_time(&package).flatten();
                        continue;
                    }
                }
            }
            // A failed read (the device is busy installing) is retried.
            if let Some(Some(time)) = read_time(&package)
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
            Ok(_) => LaunchOutcome::Launched(package),
            Err(err) => LaunchOutcome::Failed(err.to_string()),
        };
        done(outcome);
    })
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;

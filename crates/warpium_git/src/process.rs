//! Running git: one-shot commands, and `git log` streamed on a background thread that parses
//! and lays out the graph as it reads, and only reads as far as the UI asked.

use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use command::blocking::Command;
use instant::Instant;

use crate::graph::{GraphBuilder, GraphRow};
use crate::log::{Commit, LogParser, LogQuery};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitError {
    /// git couldn't be started (it isn't installed, or the directory is gone).
    Spawn(String),
    /// git exited with an error.
    Failed { stderr: String },
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(message) => write!(f, "couldn't run git: {message}"),
            Self::Failed { stderr } => {
                let message = stderr.trim();
                let message = message.strip_prefix("fatal: ").unwrap_or(message);
                if message.is_empty() {
                    f.write_str("git failed")
                } else {
                    f.write_str(message)
                }
            }
        }
    }
}

impl std::error::Error for GitError {}

/// `git <args>` in `repo`, never prompting and without optional locks, so running it next to
/// the user's own git commands is safe.
pub fn git_command(repo: &Path, args: &[String]) -> Command {
    let mut command = Command::new("git");
    command
        .args(["-c", "core.quotepath=false", "-c", "color.ui=never", "--no-pager"])
        .args(args)
        .current_dir(repo)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    command
}

/// Runs git to completion and returns its stdout.
pub fn run_git(repo: &Path, args: &[String]) -> Result<Vec<u8>, GitError> {
    let output = git_command(repo, args)
        .output()
        .map_err(|err| GitError::Spawn(err.to_string()))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(GitError::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Runs git with `input` on its stdin and returns its stdout.
pub fn run_git_with_input(repo: &Path, args: &[String], input: &[u8]) -> Result<Vec<u8>, GitError> {
    let mut child = git_command(repo, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| GitError::Spawn(err.to_string()))?;
    let mut stdin = child.stdin.take();
    let input = input.to_vec();
    // Written from another thread: git may fill its stdout pipe before reading all of stdin.
    let writer = thread::Builder::new()
        .name("git-stdin".to_owned())
        .spawn(move || {
            if let Some(stdin) = stdin.as_mut() {
                let _ = stdin.write_all(&input);
            }
        })
        .map_err(|err| GitError::Spawn(err.to_string()))?;
    let output = child
        .wait_with_output()
        .map_err(|err| GitError::Spawn(err.to_string()))?;
    let _ = writer.join();
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(GitError::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// The root of the work tree containing `dir`.
pub fn repo_toplevel(dir: &Path) -> Result<PathBuf, GitError> {
    let output = run_git(dir, &["rev-parse".to_owned(), "--show-toplevel".to_owned()])?;
    let path = String::from_utf8_lossy(&output).trim().to_owned();
    if path.is_empty() {
        return Err(GitError::Failed {
            stderr: "not inside a work tree".to_owned(),
        });
    }
    Ok(PathBuf::from(path))
}

/// `path` relative to the work tree root, with `/` separators, as git names files. Symlinked
/// prefixes (`/var` and `/private/var` on macOS) are resolved first.
pub fn relative_to_repo(toplevel: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(toplevel).ok().map(Path::to_path_buf).or_else(|| {
        let toplevel = toplevel.canonicalize().ok()?;
        let path = path
            .canonicalize()
            .ok()
            .or_else(|| Some(path.parent()?.canonicalize().ok()?.join(path.file_name()?)))?;
        path.strip_prefix(&toplevel).ok().map(Path::to_path_buf)
    })?;
    let parts: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRow {
    pub commit: Commit,
    pub graph: GraphRow,
}

#[derive(Debug)]
pub enum LogEvent {
    Rows(Vec<LogRow>),
    /// The whole history was listed.
    Finished { total: usize },
    Failed(String),
}

/// Rows parsed before they are sent, at most.
const BATCH_ROWS: usize = 2_000;
/// Parsed rows wait at most this long before they are sent.
const BATCH_INTERVAL: Duration = Duration::from_millis(50);
const READ_CHUNK: usize = 64 * 1024;

struct Demand {
    target: usize,
    cancelled: bool,
}

type Shared = Arc<(Mutex<Demand>, Condvar)>;

/// A running `git log`. Rows go to the sink from a background thread until `target` rows were
/// sent; then the thread stops reading, so git blocks on its full pipe until [`Self::request`]
/// asks for more. Dropping it kills git.
pub struct LogStream {
    shared: Shared,
    child: Arc<Mutex<Option<Child>>>,
}

impl LogStream {
    /// Asks for rows until `total` were sent in all.
    pub fn request(&self, total: usize) {
        let (demand, wake) = &*self.shared;
        let mut demand = demand.lock().unwrap_or_else(PoisonError::into_inner);
        if total > demand.target {
            demand.target = total;
            wake.notify_all();
        }
    }
}

impl Drop for LogStream {
    fn drop(&mut self) {
        let (demand, wake) = &*self.shared;
        demand
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cancelled = true;
        wake.notify_all();
        if let Some(child) = self
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            let _ = child.kill();
        }
    }
}

/// Starts `git log` for `query` in `repo`, sending its first `initial` rows (and more on
/// request) to `sink` with their graph rows. Author and text filters list rows without a graph.
pub fn spawn_log(
    repo: &Path,
    query: &LogQuery,
    initial: usize,
    sink: impl FnMut(LogEvent) + Send + 'static,
) -> Result<LogStream, GitError> {
    let mut child = git_command(repo, &query.args())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| GitError::Spawn(err.to_string()))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let shared: Shared = Arc::new((
        Mutex::new(Demand {
            target: initial,
            cancelled: false,
        }),
        Condvar::new(),
    ));
    let child = Arc::new(Mutex::new(Some(child)));
    let with_graph = query.shows_graph();
    let thread_shared = shared.clone();
    let thread_child = child.clone();
    let spawned = thread::Builder::new()
        .name("git-log".to_owned())
        .spawn(move || {
            read_log(
                stdout,
                stderr,
                with_graph,
                thread_shared,
                thread_child,
                Box::new(sink),
            )
        });
    if let Err(err) = spawned {
        if let Some(mut child) = child.lock().unwrap_or_else(PoisonError::into_inner).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        return Err(GitError::Spawn(err.to_string()));
    }
    Ok(LogStream { shared, child })
}

fn read_log(
    stdout: Option<std::process::ChildStdout>,
    stderr: Option<std::process::ChildStderr>,
    with_graph: bool,
    shared: Shared,
    child: Arc<Mutex<Option<Child>>>,
    mut sink: Box<dyn FnMut(LogEvent) + Send>,
) {
    let stderr_reader = stderr.and_then(|mut stderr| {
        thread::Builder::new()
            .name("git-log-stderr".to_owned())
            .spawn(move || {
                let mut text = String::new();
                let _ = stderr.read_to_string(&mut text);
                text
            })
            .ok()
    });
    let mut parser = LogParser::default();
    let mut graph = GraphBuilder::new();
    let mut commits = Vec::new();
    let mut batch: Vec<LogRow> = Vec::new();
    let mut sent = 0;
    let mut last_send = Instant::now();
    let mut buffer = vec![0; READ_CHUNK];
    let mut cancelled = false;
    let to_row = |commit: Commit, graph: &mut GraphBuilder| {
        let graph = if with_graph {
            graph.push(commit.oid, &commit.parents)
        } else {
            GraphRow::single()
        };
        LogRow { commit, graph }
    };

    if let Some(mut stdout) = stdout {
        loop {
            let target = {
                let (demand, wake) = &*shared;
                let mut demand = demand.lock().unwrap_or_else(PoisonError::into_inner);
                while !demand.cancelled && sent >= demand.target {
                    demand = wake.wait(demand).unwrap_or_else(PoisonError::into_inner);
                }
                if demand.cancelled {
                    cancelled = true;
                    break;
                }
                demand.target
            };
            let read = match stdout.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    log::warn!("[Git log] Reading git's output failed: {err}");
                    break;
                }
            };
            parser.feed(&buffer[..read], &mut commits);
            batch.extend(commits.drain(..).map(|commit| to_row(commit, &mut graph)));
            if batch.len() >= BATCH_ROWS
                || sent + batch.len() >= target
                || (!batch.is_empty() && last_send.elapsed() >= BATCH_INTERVAL)
            {
                sent += batch.len();
                sink(LogEvent::Rows(std::mem::take(&mut batch)));
                last_send = Instant::now();
            }
        }
    }

    cancelled |= shared
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .cancelled;
    let status = child
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .map(|mut child| {
            if cancelled {
                let _ = child.kill();
            }
            child.wait()
        });
    let stderr_text = stderr_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    if cancelled {
        return;
    }
    parser.finish(&mut commits);
    batch.extend(commits.drain(..).map(|commit| to_row(commit, &mut graph)));
    if !batch.is_empty() {
        sent += batch.len();
        sink(LogEvent::Rows(batch));
    }
    match status {
        Some(Ok(status)) if status.success() => sink(LogEvent::Finished { total: sent }),
        Some(Ok(_)) => sink(LogEvent::Failed(
            GitError::Failed {
                stderr: stderr_text,
            }
            .to_string(),
        )),
        Some(Err(err)) => sink(LogEvent::Failed(err.to_string())),
        // Killed by `LogStream::drop` after the last read.
        None => {}
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;

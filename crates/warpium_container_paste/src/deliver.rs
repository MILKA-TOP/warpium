//! Copying images into a container or onto an SSH host with the host's CLIs. Every function here
//! blocks; callers run it off the UI thread.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use instant::Instant;

use chrono::Local;

use crate::inspect::{self, ContainerInfo, SelectionHints};
use crate::invocation::{ContainerInvocation, ContainerRef, SshInvocation, Target};
use crate::naming::{self, MAX_IMAGE_BYTES, REMOTE_WRITE_SCRIPT};

/// Folders searched for a CLI after `PATH`: apps started from the Dock get a minimal `PATH`, and
/// Docker Desktop, OrbStack, Rancher Desktop, Podman and Homebrew install their CLIs here.
const EXTRA_BIN_DIRS: &[&str] = &[
    "/usr/local/bin",
    "/opt/homebrew/bin",
    "/opt/local/bin",
    "~/.docker/bin",
    "~/.orbstack/bin",
    "~/.rd/bin",
    "~/.local/bin",
    "/Applications/Docker.app/Contents/Resources/bin",
    "/opt/podman/bin",
    "/usr/bin",
    "/bin",
];

const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const SSH_CONNECT_TIMEOUT_SECS: u32 = 10;

/// Where to look for CLIs and how long to wait for them.
#[derive(Clone, Debug)]
pub struct Tools {
    pub path_var: Option<OsString>,
    pub home: Option<PathBuf>,
    pub timeout: Duration,
}

impl Tools {
    pub fn from_env() -> Self {
        Self {
            path_var: std::env::var_os("PATH"),
            home: std::env::var_os("HOME").map(PathBuf::from),
            timeout: COMMAND_TIMEOUT,
        }
    }

    fn search_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = self
            .path_var
            .as_ref()
            .map(|path| std::env::split_paths(path).collect::<Vec<_>>())
            .unwrap_or_default();
        for dir in EXTRA_BIN_DIRS {
            let dir = match (dir.strip_prefix("~/"), &self.home) {
                (Some(relative), Some(home)) => home.join(relative),
                (Some(_), None) => continue,
                (None, _) => PathBuf::from(dir),
            };
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        dirs
    }

    /// The executable `program` names: a path as is, a bare name looked up in `PATH` and the
    /// usual install folders.
    pub fn find_program(&self, program: &str) -> Option<PathBuf> {
        if program.contains('/') {
            let path = PathBuf::from(program);
            return path.is_file().then_some(path);
        }
        self.search_dirs()
            .into_iter()
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    }

    /// `PATH` for the CLIs this runs, which may start helpers of their own (`ssh` for remote
    /// Docker contexts, credential helpers).
    fn child_path(&self, program: &Path) -> OsString {
        let mut dirs = program
            .parent()
            .map(Path::to_path_buf)
            .into_iter()
            .collect::<Vec<_>>();
        for dir in self.search_dirs() {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        std::env::join_paths(dirs).unwrap_or_default()
    }
}

/// The bytes of one image to paste.
#[derive(Clone, Debug)]
pub enum ImageSource {
    Bytes(Vec<u8>),
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct PasteImage {
    pub source: ImageSource,
    /// The file extension for its format (`png`, `jpg`, ...).
    pub extension: String,
    /// The image's own file name, when it came from a file.
    pub original_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PasteRequest {
    pub target: Target,
    pub hints: OwnedHints,
    pub images: Vec<PasteImage>,
    /// The directory inside the container or on the SSH host.
    pub remote_dir: String,
    pub tools: Tools,
}

#[derive(Clone, Debug, Default)]
pub struct OwnedHints {
    pub cwd: Option<PathBuf>,
    pub hostname: Option<String>,
}

impl OwnedHints {
    fn borrowed(&self) -> SelectionHints<'_> {
        SelectionHints {
            cwd: self.cwd.as_deref(),
            hostname: self.hostname.as_deref(),
        }
    }
}

/// How the files got there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Streamed through `exec -i … sh -c 'cat > …'`.
    Exec,
    /// `cp` into the container.
    Copy,
    /// Written into a bind-mounted host folder.
    Mount,
    /// Streamed through `ssh … 'cat > …'`.
    Ssh,
}

#[derive(Clone, Debug)]
pub struct Pasted {
    /// Each image's path inside the container or on the host, in request order.
    pub paths: Vec<String>,
    pub method: Method,
    /// The container's name or the SSH destination.
    pub destination: String,
}

/// Why a paste failed, worded for a toast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasteError {
    ProgramNotFound(String),
    NoContainer(String),
    Image(String),
    Failed(String),
}

impl PasteError {
    /// A name for the failure that carries no container, host or path names, for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            PasteError::ProgramNotFound(_) => "program not found",
            PasteError::NoContainer(_) => "no matching container",
            PasteError::Image(_) => "unreadable image",
            PasteError::Failed(_) => "copy failed",
        }
    }
}

impl fmt::Display for PasteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PasteError::ProgramNotFound(program) => write!(
                f,
                "`{program}` wasn't found in PATH or the usual install folders (/usr/local/bin, \
                 /opt/homebrew/bin, ~/.docker/bin, ~/.orbstack/bin)."
            ),
            PasteError::NoContainer(message)
            | PasteError::Image(message)
            | PasteError::Failed(message) => f.write_str(message),
        }
    }
}

/// Copies `request.images` to the request's container or SSH host and returns their paths there.
pub fn paste(request: &PasteRequest) -> Result<Pasted, PasteError> {
    if !naming::is_valid_remote_dir(&request.remote_dir) {
        return Err(PasteError::Failed(format!(
            "`{}` can't be used as the paste folder: it must be an absolute path of letters, \
             digits, `.`, `_`, `-` and `/`.",
            request.remote_dir
        )));
    }
    let now = Local::now();
    let mut files = Vec::with_capacity(request.images.len());
    for (seq, image) in request.images.iter().enumerate() {
        let (bytes, extension) = load_image(image)?;
        let name = naming::file_name(image.original_name.as_deref(), &extension, now, seq);
        files.push((name, bytes));
    }
    match &request.target {
        Target::Container(invocation) => paste_into_container(request, invocation, &files),
        Target::Ssh(invocation) => paste_over_ssh(request, invocation, &files),
    }
}

fn load_image(image: &PasteImage) -> Result<(Vec<u8>, String), PasteError> {
    let bytes = match &image.source {
        ImageSource::Bytes(bytes) => bytes.clone(),
        ImageSource::File(path) => {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            let too_large = fs::metadata(path)
                .map_err(|err| PasteError::Image(format!("Couldn't read {name}: {err}")))?
                .len()
                > MAX_IMAGE_BYTES;
            if too_large {
                return Err(PasteError::Image(format!(
                    "{name} is larger than {} MB.",
                    MAX_IMAGE_BYTES / 1024 / 1024
                )));
            }
            fs::read(path)
                .map_err(|err| PasteError::Image(format!("Couldn't read {name}: {err}")))?
        }
    };
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(PasteError::Image(format!(
            "The image is larger than {} MB.",
            MAX_IMAGE_BYTES / 1024 / 1024
        )));
    }
    if naming::needs_png_conversion(&image.extension)
        && let Some(png) = convert_to_png(&bytes, &image.extension)
    {
        return Ok((png, "png".to_owned()));
    }
    Ok((bytes, image.extension.clone()))
}

/// Converts an image the agents can't read (TIFF, BMP, HEIC) to PNG with macOS's `sips`.
#[cfg(target_os = "macos")]
fn convert_to_png(bytes: &[u8], extension: &str) -> Option<Vec<u8>> {
    let dir = tempfile::tempdir().ok()?;
    let input = dir.path().join(format!("in.{extension}"));
    let output = dir.path().join("out.png");
    fs::write(&input, bytes).ok()?;
    let status = command::blocking::Command::new("/usr/bin/sips")
        .args(["-s", "format", "png"])
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .output()
        .ok()?
        .status;
    if !status.success() {
        log::warn!("[Container paste] sips couldn't convert a {extension} image to PNG");
        return None;
    }
    fs::read(&output).ok()
}

#[cfg(not(target_os = "macos"))]
fn convert_to_png(_bytes: &[u8], _extension: &str) -> Option<Vec<u8>> {
    None
}

/// Runs `program args`, feeding it `stdin`, and returns its standard output, or its standard
/// error (or exit status) on failure. Gives up after `tools.timeout`.
fn run(
    tools: &Tools,
    program: &Path,
    args: &[String],
    stdin: Option<&[u8]>,
) -> Result<String, String> {
    let mut command = command::blocking::Command::new(program);
    command
        .args(args)
        .env("PATH", tools.child_path(program))
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let program_name = program.file_name().map_or_else(
        || program.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let mut child = command
        .spawn()
        .map_err(|err| format!("couldn't start {program_name}: {err}"))?;

    let writer = match (stdin, child.stdin.take()) {
        (Some(bytes), Some(mut pipe)) => {
            let bytes = bytes.to_vec();
            Some(std::thread::spawn(move || pipe.write_all(&bytes)))
        }
        _ => None,
    };
    let stdout_reader = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut output = Vec::new();
            let _ = pipe.read_to_end(&mut output);
            output
        })
    });
    let stderr_reader = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut output = Vec::new();
            let _ = pipe.read_to_end(&mut output);
            output
        })
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > tools.timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program_name} didn't finish within {} s",
                    tools.timeout.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(err) => return Err(format!("couldn't wait for {program_name}: {err}")),
        }
    };
    let write_result = writer.map(|writer| writer.join());
    let stdout = stdout_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let stderr = stderr_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        let stderr = stderr.trim();
        return Err(if stderr.is_empty() {
            format!("{program_name} failed ({status})")
        } else {
            last_lines(stderr, 3)
        });
    }
    if let Some(Ok(Err(err))) = write_result {
        return Err(format!("couldn't send the image to {program_name}: {err}"));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

fn last_lines(text: &str, count: usize) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    lines[lines.len().saturating_sub(count)..].join(" ")
}

fn engine_args(invocation: &ContainerInvocation, args: &[&str]) -> Vec<String> {
    invocation
        .global_args
        .iter()
        .cloned()
        .chain(args.iter().map(|arg| (*arg).to_owned()))
        .collect()
}

fn inspect_containers(
    tools: &Tools,
    program: &Path,
    invocation: &ContainerInvocation,
    ids: &[&str],
) -> Result<Vec<ContainerInfo>, String> {
    let mut args = vec!["inspect", "--type", "container"];
    args.extend_from_slice(ids);
    let output = run(tools, program, &engine_args(invocation, &args), None)?;
    inspect::parse_inspect(&output)
}

/// The running container the session is in.
fn resolve_container(
    request: &PasteRequest,
    program: &Path,
    invocation: &ContainerInvocation,
) -> Result<ResolvedContainer, PasteError> {
    let engine = invocation.engine.default_program();
    let describe = || match &invocation.container {
        ContainerRef::Named(name) => format!("container `{name}`"),
        ContainerRef::Image(image) => format!("container of image `{image}`"),
        ContainerRef::ComposeService(service) => {
            format!("container of Compose service `{}`", service.service)
        }
    };
    if let ContainerRef::Named(name) = &invocation.container {
        return Ok(ResolvedContainer {
            id: name.clone(),
            display_name: name.clone(),
            info: None,
        });
    }
    let ids = run(
        &request.tools,
        program,
        &engine_args(invocation, &["ps", "-q", "--no-trunc"]),
        None,
    )
    .map_err(|err| PasteError::Failed(format!("Couldn't list containers with {engine}: {err}")))?;
    let ids = ids.split_whitespace().collect::<Vec<_>>();
    if ids.is_empty() {
        return Err(PasteError::NoContainer(format!(
            "No running {} was found: {engine} has no running containers.",
            describe()
        )));
    }
    let containers = inspect_containers(&request.tools, program, invocation, &ids)
        .map_err(|err| PasteError::Failed(format!("Couldn't inspect containers: {err}")))?;
    let container = inspect::select_container(
        &containers,
        &invocation.container,
        &request.hints.borrowed(),
    )
    .ok_or_else(|| PasteError::NoContainer(format!("No running {} was found.", describe())))?;
    Ok(ResolvedContainer {
        id: container.id.clone(),
        display_name: container.display_name().to_owned(),
        info: Some(container.clone()),
    })
}

struct ResolvedContainer {
    id: String,
    display_name: String,
    info: Option<ContainerInfo>,
}

fn paste_into_container(
    request: &PasteRequest,
    invocation: &ContainerInvocation,
    files: &[(String, Vec<u8>)],
) -> Result<Pasted, PasteError> {
    let tools = &request.tools;
    let program = tools
        .find_program(&invocation.program)
        .ok_or_else(|| PasteError::ProgramNotFound(invocation.program.clone()))?;
    let mut container = resolve_container(request, &program, invocation)?;
    let dir = request.remote_dir.as_str();

    let mut errors = Vec::new();
    let exec = |name: &str, bytes: &[u8]| {
        run(
            tools,
            &program,
            &engine_args(
                invocation,
                &[
                    "exec",
                    "-i",
                    &container.id,
                    "sh",
                    "-c",
                    REMOTE_WRITE_SCRIPT,
                    "warpium-paste",
                    dir,
                    name,
                ],
            ),
            Some(bytes),
        )
    };
    match write_all(files, |name, bytes| exec(name, bytes)) {
        Ok(()) => {
            let cleanup_args = engine_args(
                invocation,
                &[
                    "exec",
                    &container.id,
                    "sh",
                    "-c",
                    &naming::remote_cleanup_script(),
                    "warpium-paste",
                    dir,
                ],
            );
            clean_up_in_background(tools.clone(), program.clone(), cleanup_args);
            return Ok(Pasted {
                paths: files
                    .iter()
                    .map(|(name, _)| format!("{dir}/{name}"))
                    .collect(),
                method: Method::Exec,
                destination: container.display_name,
            });
        }
        Err(err) => errors.push(format!("exec: {err}")),
    }

    match copy_into_container(tools, &program, invocation, &container.id, dir, files) {
        Ok(()) => {
            return Ok(Pasted {
                paths: files
                    .iter()
                    .map(|(name, _)| format!("{dir}/{name}"))
                    .collect(),
                method: Method::Copy,
                destination: container.display_name,
            });
        }
        Err(err) => errors.push(format!("cp: {err}")),
    }

    if container.info.is_none() {
        container.info = inspect_containers(tools, &program, invocation, &[&container.id])
            .ok()
            .and_then(|containers| containers.into_iter().next());
    }
    if let (Some(info), Some(cwd)) = (&container.info, &request.hints.cwd)
        && let Some(paths) = write_into_mount(info, cwd, files)
    {
        return Ok(Pasted {
            paths,
            method: Method::Mount,
            destination: container.display_name,
        });
    }

    Err(PasteError::Failed(format!(
        "Couldn't copy the image into container `{}`. {}",
        container.display_name,
        errors.join("; ")
    )))
}

fn write_all(
    files: &[(String, Vec<u8>)],
    mut write: impl FnMut(&str, &[u8]) -> Result<String, String>,
) -> Result<(), String> {
    for (name, bytes) in files {
        write(name, bytes)?;
    }
    Ok(())
}

/// `cp`s the files into `dir`, creating it if needed. The staging folder's permissions carry
/// over to a newly created `dir`, so both are made readable by every user in the container.
fn copy_into_container(
    tools: &Tools,
    program: &Path,
    invocation: &ContainerInvocation,
    container: &str,
    dir: &str,
    files: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let staging =
        tempfile::tempdir().map_err(|err| format!("couldn't create a temporary folder: {err}"))?;
    for (name, bytes) in files {
        let path = staging.path().join(name);
        fs::write(&path, bytes).map_err(|err| format!("couldn't write a temporary file: {err}"))?;
        set_mode(&path, 0o644);
    }
    set_mode(staging.path(), 0o755);
    let source = format!("{}/.", staging.path().display());
    let destination = format!("{container}:{dir}");
    run(
        tools,
        program,
        &engine_args(invocation, &["cp", &source, &destination]),
        None,
    )
    .map(|_| ())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;

    if let Err(err) = fs::set_permissions(path, fs::Permissions::from_mode(mode)) {
        log::warn!("[Container paste] Couldn't set permissions of a staged file: {err}");
    }
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

/// Writes the files into the project's bind mount, when the session's folder is inside one, and
/// returns their paths in the container.
fn write_into_mount(
    container: &ContainerInfo,
    cwd: &Path,
    files: &[(String, Vec<u8>)],
) -> Option<Vec<String>> {
    let mount = inspect::mount_for_cwd(&container.mounts, cwd)?;
    let dir = Path::new(&mount.source).join(naming::MOUNT_PASTE_DIR);
    if !Path::new(&mount.source).is_dir() || fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        let _ = fs::write(&gitignore, "*\n");
    }
    remove_old_files(&dir);
    let mut paths = Vec::with_capacity(files.len());
    for (name, bytes) in files {
        let path = dir.join(name);
        if let Err(err) = fs::write(&path, bytes) {
            log::warn!("[Container paste] Couldn't write into the bind mount: {err}");
            return None;
        }
        paths.push(inspect::container_path(mount, &path)?);
    }
    Some(paths)
}

/// Removes old pasted files from a host folder (see [`naming::files_to_remove`]).
pub fn remove_old_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let listing = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((name, modified))
        })
        .collect::<Vec<_>>();
    for name in naming::files_to_remove(&listing, SystemTime::now()) {
        let _ = fs::remove_file(dir.join(name));
    }
}

fn clean_up_in_background(tools: Tools, program: PathBuf, args: Vec<String>) {
    let spawned = std::thread::Builder::new()
        .name("warpium-paste-cleanup".to_owned())
        .spawn(move || {
            if let Err(err) = run(&tools, &program, &args, None) {
                log::debug!("[Container paste] Cleanup of old pasted files failed: {err}");
            }
        });
    if let Err(err) = spawned {
        log::debug!("[Container paste] Couldn't start the cleanup thread: {err}");
    }
}

fn ssh_args(invocation: &SshInvocation, remote_command: String) -> Vec<String> {
    let mut args = invocation.options.clone();
    args.extend(
        [
            "-o",
            "BatchMode=yes",
            "-o",
            &format!("ConnectTimeout={SSH_CONNECT_TIMEOUT_SECS}"),
            "-o",
            "RemoteCommand=none",
            "-o",
            "RequestTTY=no",
            "-T",
            "--",
            &invocation.destination,
        ]
        .map(str::to_owned),
    );
    args.push(remote_command);
    args
}

/// Runs `script` with `sh` on the remote host, with `args` as its positional parameters. Every
/// argument is made of characters that need no quoting, but they are single-quoted anyway.
fn remote_sh(script: &str, args: &[&str]) -> String {
    let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let mut command = format!("sh -c {} warpium-paste", quote(script));
    for arg in args {
        command.push(' ');
        command.push_str(&quote(arg));
    }
    command
}

fn paste_over_ssh(
    request: &PasteRequest,
    invocation: &SshInvocation,
    files: &[(String, Vec<u8>)],
) -> Result<Pasted, PasteError> {
    let tools = &request.tools;
    let program = tools
        .find_program(&invocation.program)
        .ok_or_else(|| PasteError::ProgramNotFound(invocation.program.clone()))?;
    let dir = request.remote_dir.as_str();
    write_all(files, |name, bytes| {
        run(
            tools,
            &program,
            &ssh_args(invocation, remote_sh(REMOTE_WRITE_SCRIPT, &[dir, name])),
            Some(bytes),
        )
    })
    .map_err(|err| {
        PasteError::Failed(format!(
            "Couldn't copy the image to {} over SSH (it needs to log in without a password \
             prompt, with a key or a shared connection): {err}",
            invocation.destination
        ))
    })?;
    clean_up_in_background(
        tools.clone(),
        program,
        ssh_args(
            invocation,
            remote_sh(&naming::remote_cleanup_script(), &[dir]),
        ),
    );
    Ok(Pasted {
        paths: files
            .iter()
            .map(|(name, _)| format!("{dir}/{name}"))
            .collect(),
        method: Method::Ssh,
        destination: invocation.destination.clone(),
    })
}

#[cfg(test)]
#[path = "deliver_tests.rs"]
mod tests;

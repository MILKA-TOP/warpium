//! Installs a downloaded, checksum-verified Warpium disk image over the running app bundle.
//!
//! [`prepare`] copies the new bundle out of the read-only mounted image to a hidden sibling of the
//! running bundle (same volume, so the swap is a rename) and verifies it; any failure leaves the
//! running app untouched. [`swap_bundles`] atomically exchanges the two right before the app
//! relaunches, and the old bundle is kept as a backup until the new version has launched.

use command::blocking::Command;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::version::WarpiumVersion;

/// The bundle identifier of every Warpium build (`app/src/bin/oss.rs`, `script/update_plist`).
pub const BUNDLE_ID: &str = "dev.milka.Warpium";

const BACKUP_SUFFIX: &str = "-previous.app";
const STAGING_INFIX: &str = "-update-";

/// The external tools an install uses. [`MacTools`] runs the real macOS commands.
pub trait InstallTools {
    /// Mounts the disk image read-only at `mountpoint`.
    fn attach(&self, dmg: &Path, mountpoint: &Path) -> Result<(), String>;
    fn detach(&self, mountpoint: &Path) -> Result<(), String>;
    /// Copies an app bundle, preserving symlinks, permissions and extended attributes.
    fn copy_bundle(&self, from: &Path, to: &Path) -> Result<(), String>;
    /// Checks that the bundle's code signature is intact.
    fn verify_signature(&self, bundle: &Path) -> Result<(), String>;
    /// Removes the quarantine attribute, so Gatekeeper doesn't block the (not notarized) app.
    fn clear_quarantine(&self, bundle: &Path) -> Result<(), String>;
}

/// [`InstallTools`] backed by `hdiutil`, `ditto`, `codesign` and `xattr`.
pub struct MacTools;

impl InstallTools for MacTools {
    fn attach(&self, dmg: &Path, mountpoint: &Path) -> Result<(), String> {
        run(Command::new("/usr/bin/hdiutil")
            .args(["attach", "-nobrowse", "-noautoopen", "-readonly"])
            .arg("-mountpoint")
            .arg(mountpoint)
            .arg(dmg))
    }

    fn detach(&self, mountpoint: &Path) -> Result<(), String> {
        run(Command::new("/usr/bin/hdiutil")
            .arg("detach")
            .arg(mountpoint)
            .arg("-force"))
    }

    fn copy_bundle(&self, from: &Path, to: &Path) -> Result<(), String> {
        run(Command::new("/usr/bin/ditto").arg(from).arg(to))
    }

    fn verify_signature(&self, bundle: &Path) -> Result<(), String> {
        // Warpium builds are ad-hoc signed, so there is no team identity to require (unlike Warp's
        // updater); this checks that the sealed bundle is intact.
        run(Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep"])
            .arg(bundle))
    }

    fn clear_quarantine(&self, bundle: &Path) -> Result<(), String> {
        run(Command::new("/usr/bin/xattr")
            .args(["-dr", "com.apple.quarantine"])
            .arg(bundle))
    }
}

fn run(command: &mut Command) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|err| format!("couldn't run {:?}: {err}", command.get_program()))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "{:?} failed ({}): {}",
        command.get_program(),
        output.status,
        stderr.trim()
    ))
}

pub struct PrepareRequest<'a> {
    /// The downloaded disk image, already checked against the release's SHA-256.
    pub dmg: &'a Path,
    /// The release being installed; the new bundle must carry its version.
    pub version: &'a WarpiumVersion,
    /// The running app bundle to replace.
    pub bundle: &'a Path,
    /// Where to mount the image; it must not be in use.
    pub mountpoint: &'a Path,
    /// Distinguishes this process's staged copy from another running instance's.
    pub process_id: u32,
}

/// A verified copy of the new app next to the running bundle, ready for [`swap_bundles`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub staged: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("Warpium isn't running from an app bundle ({})", .0.display())]
    NotAnAppBundle(PathBuf),
    #[error(
        "macOS is running Warpium from a temporary read-only copy; move Warpium.app to \
         Applications, open it from there and try again"
    )]
    Translocated,
    #[error("Warpium can't replace itself in {}: the location isn't writable", .0.display())]
    NotWritable(PathBuf),
    #[error("couldn't open the disk image: {0}")]
    Mount(String),
    #[error("the disk image doesn't contain Warpium")]
    AppNotFound,
    #[error("the disk image contains {found}, not Warpium ({BUNDLE_ID})")]
    WrongBundleId { found: String },
    #[error("the disk image contains version {found}, not {expected}")]
    WrongVersion { expected: String, found: String },
    #[error("couldn't read the new app's Info.plist: {0}")]
    InfoPlist(String),
    #[error("the new app is incomplete: {0}")]
    Incomplete(String),
    #[error("the new app's code signature is invalid: {0}")]
    Signature(String),
    #[error("couldn't copy the new app: {0}")]
    Copy(String),
    #[error("couldn't swap in the new app: {0}")]
    Swap(String),
}

impl InstallError {
    /// Whether the app can't be updated where it is, so installing the disk image by hand (or
    /// moving the app) is the way out.
    pub fn is_location_problem(&self) -> bool {
        match self {
            Self::NotAnAppBundle(_) | Self::Translocated | Self::NotWritable(_) => true,
            Self::Mount(_)
            | Self::AppNotFound
            | Self::WrongBundleId { .. }
            | Self::WrongVersion { .. }
            | Self::InfoPlist(_)
            | Self::Incomplete(_)
            | Self::Signature(_)
            | Self::Copy(_)
            | Self::Swap(_) => false,
        }
    }
}

/// Copies the new app out of the disk image to a hidden sibling of the running bundle and
/// verifies it, leaving the running app untouched. Blocking; run it off the main thread.
pub fn prepare(
    request: &PrepareRequest,
    tools: &dyn InstallTools,
) -> Result<Prepared, InstallError> {
    let bundle = request.bundle;
    let (parent, stem) = bundle_location(bundle)?;
    check_writable(parent, bundle)?;

    tools
        .attach(request.dmg, request.mountpoint)
        .map_err(InstallError::Mount)?;
    let mount = MountedImage {
        tools,
        mountpoint: request.mountpoint,
    };
    let source = find_app(mount.mountpoint, request.version)?;

    let mut staged = StagedBundle {
        path: parent.join(format!(".{stem}{STAGING_INFIX}{}.app", request.process_id)),
        armed: true,
    };
    remove_dir_if_exists(&staged.path)
        .map_err(|err| InstallError::Copy(format!("couldn't remove an old staged copy: {err}")))?;
    tools
        .copy_bundle(&source, &staged.path)
        .map_err(InstallError::Copy)?;
    verify_bundle(&staged.path, request.version)?;
    tools
        .verify_signature(&staged.path)
        .map_err(InstallError::Signature)?;
    if let Err(err) = tools.clear_quarantine(&staged.path) {
        log::warn!("[Warpium update] Couldn't clear the quarantine attribute: {err}");
    }
    staged.armed = false;
    Ok(Prepared {
        staged: staged.path.clone(),
    })
}

/// Atomically exchanges the staged copy with the running bundle: afterwards the bundle path holds
/// the new version and `staged` the previous one. Calling it again undoes it.
pub fn swap_bundles(staged: &Path, bundle: &Path) -> Result<(), InstallError> {
    let (parent, _) = bundle_location(bundle)?;
    swap(staged, bundle).map_err(|err| match err.kind() {
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
            InstallError::NotWritable(parent.to_owned())
        }
        _ => InstallError::Swap(err.to_string()),
    })
}

/// Where the previous version is kept after an update, until the new one has launched.
pub fn backup_path(bundle: &Path) -> Option<PathBuf> {
    let (parent, stem) = bundle_location(bundle).ok()?;
    Some(parent.join(format!(".{stem}{BACKUP_SUFFIX}")))
}

/// Renames the previous version (at the staged path after [`swap_bundles`]) to
/// [`backup_path`], replacing an older backup.
pub fn keep_as_backup(previous: &Path, bundle: &Path) -> io::Result<PathBuf> {
    let backup = backup_path(bundle)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not an app bundle"))?;
    replace_dir(previous, &backup)?;
    Ok(backup)
}

/// Staged copies next to `bundle` left by processes that are no longer running (a failed or
/// abandoned update). `is_running` tells whether a process id is alive.
pub fn abandoned_staged_copies(bundle: &Path, is_running: impl Fn(u32) -> bool) -> Vec<PathBuf> {
    let Ok((parent, stem)) = bundle_location(bundle) else {
        return Vec::new();
    };
    let prefix = format!(".{stem}{STAGING_INFIX}");
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut abandoned: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix(&prefix))
                .and_then(|rest| rest.strip_suffix(".app"))
                .and_then(|pid| pid.parse::<u32>().ok())
                .is_some_and(|pid| !is_running(pid))
        })
        .map(|entry| entry.path())
        .collect();
    abandoned.sort();
    abandoned
}

/// Whether a process with this id is alive. Errs on the side of "running" where it can't tell,
/// so nothing that might be in use is deleted.
pub fn process_is_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return true;
        };
        // SAFETY: signal 0 only checks whether the process exists; nothing is sent.
        let result = unsafe { libc::kill(pid, 0) };
        result == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Deletes a backup or staged copy made by this module. Refuses paths that aren't one, so a
/// corrupted path can't delete anything else.
pub fn remove_backup(path: &Path) -> io::Result<()> {
    let is_ours = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with('.')
                && name.ends_with(".app")
                && (name.ends_with(BACKUP_SUFFIX) || name.contains(STAGING_INFIX))
        });
    if !is_ours {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a Warpium update backup",
        ));
    }
    remove_dir_if_exists(path)
}

fn bundle_location(bundle: &Path) -> Result<(&Path, &str), InstallError> {
    let not_a_bundle = || InstallError::NotAnAppBundle(bundle.to_owned());
    if bundle
        .extension()
        .is_none_or(|extension| extension != "app")
    {
        return Err(not_a_bundle());
    }
    let stem = bundle
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(not_a_bundle)?;
    let parent = bundle.parent().ok_or_else(not_a_bundle)?;
    Ok((parent, stem))
}

fn check_writable(parent: &Path, bundle: &Path) -> Result<(), InstallError> {
    // Gatekeeper runs quarantined apps opened from where they were downloaded from a randomized
    // read-only path.
    if bundle
        .components()
        .any(|component| component.as_os_str() == "AppTranslocation")
    {
        return Err(InstallError::Translocated);
    }
    let not_writable = || InstallError::NotWritable(parent.to_owned());
    let bundle_metadata = fs::metadata(bundle).map_err(|_| not_writable())?;
    if bundle_metadata.permissions().readonly() {
        return Err(not_writable());
    }
    let probe = parent.join(format!(".warpium-update-probe-{}", std::process::id()));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|_| not_writable())?;
    if let Err(err) = fs::remove_file(&probe) {
        log::warn!("[Warpium update] Couldn't remove the write probe: {err}");
    }
    Ok(())
}

/// Detaches the image when dropped, whether or not the install succeeded.
struct MountedImage<'a> {
    tools: &'a dyn InstallTools,
    mountpoint: &'a Path,
}

impl Drop for MountedImage<'_> {
    fn drop(&mut self) {
        if let Err(err) = self.tools.detach(self.mountpoint) {
            log::warn!("[Warpium update] Couldn't detach the disk image: {err}");
        }
    }
}

/// Removes the staged copy when dropped while armed, so a failed install leaves nothing behind.
struct StagedBundle {
    path: PathBuf,
    armed: bool,
}

impl Drop for StagedBundle {
    fn drop(&mut self) {
        if self.armed
            && let Err(err) = remove_dir_if_exists(&self.path)
        {
            log::warn!("[Warpium update] Couldn't remove the staged copy: {err}");
        }
    }
}

struct BundleInfo {
    identifier: String,
    short_version: String,
    executable: String,
}

fn read_bundle_info(bundle: &Path) -> Result<BundleInfo, InstallError> {
    let value = plist::Value::from_file(bundle.join("Contents/Info.plist"))
        .map_err(|err| InstallError::InfoPlist(err.to_string()))?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| InstallError::InfoPlist("not a dictionary".to_owned()))?;
    let string = |key: &str| {
        dict.get(key)
            .and_then(plist::Value::as_string)
            .map(str::to_owned)
            .ok_or_else(|| InstallError::InfoPlist(format!("{key} is missing")))
    };
    Ok(BundleInfo {
        identifier: string("CFBundleIdentifier")?,
        short_version: string("CFBundleShortVersionString")?,
        executable: string("CFBundleExecutable")?,
    })
}

fn check_identity(info: &BundleInfo, version: &WarpiumVersion) -> Result<(), InstallError> {
    if info.identifier != BUNDLE_ID {
        return Err(InstallError::WrongBundleId {
            found: info.identifier.clone(),
        });
    }
    let expected = version.plist_version();
    if info.short_version != expected {
        return Err(InstallError::WrongVersion {
            expected,
            found: info.short_version.clone(),
        });
    }
    Ok(())
}

/// The Warpium app bundle at the root of the mounted image.
fn find_app(mountpoint: &Path, version: &WarpiumVersion) -> Result<PathBuf, InstallError> {
    let entries = fs::read_dir(mountpoint).map_err(|err| InstallError::Mount(err.to_string()))?;
    let mut apps: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "app")
                && fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
        })
        .collect();
    apps.sort();

    let mut first_error = None;
    for app in apps {
        match read_bundle_info(&app).and_then(|info| check_identity(&info, version)) {
            Ok(()) => return Ok(app),
            Err(err) => {
                first_error.get_or_insert(err);
            }
        }
    }
    Err(first_error.unwrap_or(InstallError::AppNotFound))
}

fn verify_bundle(bundle: &Path, version: &WarpiumVersion) -> Result<(), InstallError> {
    let info = read_bundle_info(bundle)?;
    check_identity(&info, version)?;
    let executable = bundle.join("Contents/MacOS").join(&info.executable);
    if !executable.is_file() {
        return Err(InstallError::Incomplete(format!(
            "Contents/MacOS/{} is missing",
            info.executable
        )));
    }
    Ok(())
}

fn remove_dir_if_exists(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn replace_dir(from: &Path, to: &Path) -> io::Result<()> {
    remove_dir_if_exists(to)?;
    fs::rename(from, to)
}

/// Atomically exchanges two paths on the same volume.
#[cfg(target_os = "macos")]
fn swap(a: &Path, b: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let a = CString::new(a.as_os_str().as_bytes())?;
    let b = CString::new(b.as_os_str().as_bytes())?;
    // SAFETY: both arguments are valid NUL-terminated C strings that outlive the call.
    let result = unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Atomically exchanges two paths on the same filesystem.
#[cfg(target_os = "linux")]
fn swap(a: &Path, b: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let a = CString::new(a.as_os_str().as_bytes())?;
    let b = CString::new(b.as_os_str().as_bytes())?;
    // SAFETY: both arguments are valid NUL-terminated C strings that outlive the call.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            a.as_ptr(),
            libc::AT_FDCWD,
            b.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn swap(_a: &Path, _b: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "updating in place is only supported on macOS",
    ))
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;

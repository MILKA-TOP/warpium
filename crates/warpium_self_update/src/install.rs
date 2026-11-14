//! Installs a downloaded, checksum-verified Warpium disk image over the running app bundle.
//!
//! The new bundle is copied out of the read-only mounted image to a hidden sibling of the running
//! bundle (same volume, so the swap is a rename), verified, and atomically swapped into place.
//! The old bundle is kept next to it as a backup until the new version has launched. Any failure
//! before the swap leaves the running app untouched.

use command::blocking::Command;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::version::WarpiumVersion;

/// The bundle identifier of every Warpium build (`app/src/bin/oss.rs`, `script/update_plist`).
pub const BUNDLE_ID: &str = "dev.milka.Warpium";

const BACKUP_SUFFIX: &str = "-previous.app";
const STAGING_SUFFIX: &str = "-update.app";

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

pub struct InstallRequest<'a> {
    /// The downloaded disk image, already checked against the release's SHA-256.
    pub dmg: &'a Path,
    /// The release being installed; the new bundle must carry its version.
    pub version: &'a WarpiumVersion,
    /// The running app bundle to replace.
    pub bundle: &'a Path,
    /// Where to mount the image; it must not be in use.
    pub mountpoint: &'a Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The previous app bundle, to delete once the new version has launched.
    pub backup: PathBuf,
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

/// Installs the disk image over the running bundle. Blocking; run it off the main thread.
pub fn install(
    request: &InstallRequest,
    tools: &dyn InstallTools,
) -> Result<Installed, InstallError> {
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
        path: parent.join(format!(".{stem}{STAGING_SUFFIX}")),
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

    swap(&staged.path, bundle).map_err(|err| match err.kind() {
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
            InstallError::NotWritable(parent.to_owned())
        }
        _ => InstallError::Swap(err.to_string()),
    })?;
    // The staged path now holds the previous version.
    staged.armed = false;

    let backup = parent.join(format!(".{stem}{BACKUP_SUFFIX}"));
    let backup = match replace_dir(&staged.path, &backup) {
        Ok(()) => backup,
        Err(err) => {
            log::warn!("[Warpium update] Couldn't rename the previous version's backup: {err}");
            staged.path.clone()
        }
    };
    drop(mount);
    Ok(Installed { backup })
}

/// Deletes a backup made by [`install`]. Refuses paths that aren't one, so a corrupted saved
/// path can't delete anything else.
pub fn remove_backup(path: &Path) -> io::Result<()> {
    let is_backup = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with('.')
                && (name.ends_with(BACKUP_SUFFIX) || name.ends_with(STAGING_SUFFIX))
        });
    if !is_backup {
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

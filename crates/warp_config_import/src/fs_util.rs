use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// Writes `contents` to `path` atomically: the data goes to a temporary file in the same
/// directory, which is then renamed over `path`. Parent directories are created as needed.
///
/// `mode` sets the Unix permission bits of the result; `None` keeps the temporary file's default
/// (owner read/write).
pub fn atomic_write(path: &Path, contents: &[u8], mode: Option<u32>) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::Builder::new()
        .prefix(".warpium-import-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    temp.write_all(contents)?;
    temp.as_file().sync_all()?;
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt as _;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    temp.persist(path).map_err(|err| err.error)?;
    Ok(())
}

/// Copies the existing file at `path` (if any) to `backup_root/relative`, so an overwrite can be
/// undone by hand. Returns the backup path when a copy was made.
pub fn backup_existing(
    path: &Path,
    backup_root: &Path,
    relative: &Path,
) -> io::Result<Option<PathBuf>> {
    if !path.is_file() {
        return Ok(None);
    }
    let backup_path = backup_root.join(relative);
    if let Some(parent) = backup_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(path, &backup_path)?;
    Ok(Some(backup_path))
}

/// Returns the Unix permission bits of `path`, if available.
pub(crate) fn file_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata()
            .ok()
            .map(|meta| meta.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

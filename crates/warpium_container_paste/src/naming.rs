//! Names and formats of pasted files, and when old ones are removed.

use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local};

/// Every pasted file's name starts with this, so cleanup never touches anything else.
pub const FILE_PREFIX: &str = "warpium-";

/// Where pasted files go inside a container or on an SSH host.
pub const DEFAULT_REMOTE_DIR: &str = "/tmp/warpium-paste";

/// Where pasted files go inside a bind-mounted project, relative to the mount's source.
pub const MOUNT_PASTE_DIR: &str = ".warpium/paste";

/// Pasted files older than this are removed on a later paste: long enough for an agent to read a
/// path it was given earlier in the conversation.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// At most this many pasted files are kept in a bind-mounted project.
pub const KEEP_NEWEST: usize = 50;

/// Pasted images larger than this are refused.
pub const MAX_IMAGE_BYTES: u64 = 100 * 1024 * 1024;

/// Clipboard image formats in the order to prefer them when the clipboard holds several
/// renditions of one image (macOS screenshots carry PNG and TIFF), with their file extensions.
/// The agents read PNG, JPEG, GIF and WebP.
const PREFERRED_FORMATS: &[(&str, &str)] = &[
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/jpg", "jpg"),
    ("image/gif", "gif"),
    ("image/webp", "webp"),
    ("image/tiff", "tiff"),
    ("image/bmp", "bmp"),
    ("image/svg+xml", "svg"),
];

/// The index of the rendition to paste among the clipboard's image MIME types, and its extension.
pub fn choose_clipboard_image(mime_types: &[&str]) -> Option<(usize, &'static str)> {
    PREFERRED_FORMATS.iter().find_map(|(mime, extension)| {
        mime_types
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(mime))
            .map(|index| (index, *extension))
    })
}

/// Whether images of this extension need converting to PNG before an agent can read them.
pub fn needs_png_conversion(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "tif" | "tiff" | "bmp" | "heic" | "heif"
    )
}

/// Replaces everything but ASCII letters, digits, `.`, `_` and `-` with `-`, so names need no
/// quoting in any shell.
pub fn sanitize(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for c in name.chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            c
        } else {
            '-'
        };
        if !(c == '-' && sanitized.ends_with('-')) {
            sanitized.push(c);
        }
    }
    sanitized.trim_matches(['-', '.']).to_owned()
}

/// A unique file name for a pasted image: `warpium-<original stem or "paste">-<time>-<seq>.<ext>`.
pub fn file_name(
    original: Option<&str>,
    extension: &str,
    now: DateTime<Local>,
    seq: usize,
) -> String {
    let stem = original
        .map(|name| match name.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => stem,
            _ => name,
        })
        .map(sanitize)
        .filter(|stem| !stem.is_empty())
        .map(|stem| stem.chars().take(40).collect::<String>())
        .unwrap_or_else(|| "paste".to_owned());
    let extension = sanitize(extension).to_ascii_lowercase();
    let extension = if extension.is_empty() {
        "png".to_owned()
    } else {
        extension
    };
    format!(
        "{FILE_PREFIX}{stem}-{}-{}.{extension}",
        now.format("%Y%m%d-%H%M%S-%3f"),
        seq + 1
    )
}

/// Whether `dir` is usable as the remote directory: absolute and made only of characters that
/// need no quoting, since it is passed through a remote shell.
pub fn is_valid_remote_dir(dir: &str) -> bool {
    dir.starts_with('/')
        && dir.len() > 1
        && !dir.split('/').any(|part| part == "..")
        && dir
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
}

/// The pasted files to delete from a directory listing: those older than [`MAX_AGE`] and all but
/// the [`KEEP_NEWEST`] newest. Files without [`FILE_PREFIX`] are never returned.
pub fn files_to_remove(entries: &[(String, SystemTime)], now: SystemTime) -> Vec<String> {
    let mut pasted = entries
        .iter()
        .filter(|(name, _)| name.starts_with(FILE_PREFIX))
        .collect::<Vec<_>>();
    pasted.sort_by(|a, b| b.1.cmp(&a.1));
    pasted
        .into_iter()
        .enumerate()
        .filter(|(index, (_, modified))| {
            *index >= KEEP_NEWEST || now.duration_since(*modified).is_ok_and(|age| age > MAX_AGE)
        })
        .map(|(_, (name, _))| name.clone())
        .collect()
}

/// A POSIX shell script, run as `sh -c <script> warpium-paste <dir>`, that removes pasted files
/// older than [`MAX_AGE`] from `<dir>`, ignoring every error.
pub fn remote_cleanup_script() -> String {
    format!(
        "find \"$1\" -maxdepth 1 -type f -name '{FILE_PREFIX}*' -mmin +{} -exec rm -f {{}} + \
         2>/dev/null; exit 0",
        MAX_AGE.as_secs() / 60
    )
}

/// A POSIX shell script, run as `sh -c <script> warpium-paste <dir> <name>`, that writes its
/// standard input to `<dir>/<name>`, readable by everyone.
pub const REMOTE_WRITE_SCRIPT: &str = "umask 022 && mkdir -p \"$1\" && cat > \"$1/$2\"";

#[cfg(test)]
#[path = "naming_tests.rs"]
mod tests;

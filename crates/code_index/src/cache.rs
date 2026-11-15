use std::path::{Path, PathBuf};

/// Per-user cache directory of the app. Indexes never go inside the project.
const APP_CACHE_DIR: &str = "dev.milka.Warpium";

/// Where the snapshot for `root` lives: `<user cache dir>/dev.milka.Warpium/code-index/<hash of
/// the root path>/index.bin` (`~/Library/Caches/…` on macOS). `None` if the platform has no
/// cache directory.
pub fn default_snapshot_path(root: &Path) -> Option<PathBuf> {
    Some(snapshot_path_in(&dirs::cache_dir()?, root))
}

pub fn snapshot_path_in(cache_dir: &Path, root: &Path) -> PathBuf {
    cache_dir
        .join(APP_CACHE_DIR)
        .join("code-index")
        .join(format!("{:016x}", fnv1a(root.to_string_lossy().as_bytes())))
        .join("index.bin")
}

/// FNV-1a: stable across runs and Rust versions, unlike `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

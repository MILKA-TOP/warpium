use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::fs_util::atomic_write;

/// File name of the manifest inside Warpium's data directory.
pub const MANIFEST_FILE_NAME: &str = ".warp-import-manifest.json";

const MANIFEST_VERSION: u32 = 1;

/// What the last import brought over from Warp: the base of the 3-way sync.
///
/// Every map goes from an item's key to the content hash of the version that was last imported
/// (Warp's and Warpium's copies were identical at that moment).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    /// The channel the last import read from (see `WarpChannel::manifest_name`).
    #[serde(default)]
    pub source_channel: Option<String>,
    /// Files, keyed by `<category>/<relative path>`.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// `settings.toml` entries, keyed by dotted TOML path.
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    /// Private (user defaults) settings, keyed by storage key.
    #[serde(default)]
    pub private_settings: BTreeMap<String, KeyValueBase>,
}

/// The base of a private setting. Warpium re-serializes a value when it stores it, so the stored
/// form can differ from Warp's; each side is compared against its own hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValueBase {
    /// Hash of Warp's value at the last import.
    pub source: String,
    /// Hash of Warpium's stored value right after the last import.
    pub target: String,
}

/// The result of reading the manifest.
#[derive(Debug)]
pub enum ManifestLoad {
    /// Nothing was ever imported: the next run is the initial import.
    Missing,
    Loaded(Manifest),
    /// The manifest exists but can't be used. The run continues with an empty base, but not as
    /// an initial import, so differing items become conflicts instead of being overwritten.
    Unreadable(String),
}

impl ManifestLoad {
    pub fn is_initial_import(&self) -> bool {
        matches!(self, ManifestLoad::Missing)
    }

    pub fn into_manifest(self) -> Manifest {
        match self {
            ManifestLoad::Loaded(manifest) => manifest,
            ManifestLoad::Missing | ManifestLoad::Unreadable(_) => Manifest::default(),
        }
    }
}

impl Manifest {
    pub fn load(path: &Path) -> ManifestLoad {
        let contents = match std::fs::read(path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return ManifestLoad::Missing,
            Err(err) => return ManifestLoad::Unreadable(err.to_string()),
        };
        match serde_json::from_slice::<Manifest>(&contents) {
            Ok(manifest) if manifest.version == MANIFEST_VERSION => ManifestLoad::Loaded(manifest),
            Ok(manifest) => ManifestLoad::Unreadable(format!(
                "unsupported manifest version {}",
                manifest.version
            )),
            Err(err) => ManifestLoad::Unreadable(err.to_string()),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let manifest = Manifest {
            version: MANIFEST_VERSION,
            ..self.clone()
        };
        let json = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
        atomic_write(path, &json, None)
    }
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;

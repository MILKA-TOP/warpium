//! The GitHub Releases API response and the assets an update needs.

use serde::{Deserialize, Serialize};

use crate::version::WarpiumVersion;

/// The repository Warpium releases are published to.
pub const RELEASE_REPO: &str = "MILKA-TOP/warpium";

const API_ROOT: &str = "https://api.github.com";

pub(crate) fn latest_release_api_url() -> String {
    format!("{API_ROOT}/repos/{RELEASE_REPO}/releases/latest")
}

/// The release's page on GitHub, with its notes.
pub fn release_page_url(tag: &str) -> String {
    format!("https://github.com/{RELEASE_REPO}/releases/tag/{tag}")
}

/// A published Warpium release that can be installed.
///
/// Asset names and download URLs are derived from the tag rather than taken from GitHub's
/// response (or from saved state), so a download can only ever come from this repository's
/// releases. Callers download only releases whose [`Release::version`] parses, which limits the
/// tag to the characters of a Warpium version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub tag: String,
    /// Size of the disk image in bytes as reported by GitHub; 0 when unknown.
    pub dmg_size: u64,
}

impl Release {
    /// The parsed version. Always `Some` for a release produced by [`parse_latest_release`];
    /// `None` only for a corrupted saved state.
    pub fn version(&self) -> Option<WarpiumVersion> {
        WarpiumVersion::parse(&self.tag)
    }

    pub fn page_url(&self) -> String {
        release_page_url(&self.tag)
    }

    pub fn dmg_name(&self) -> String {
        dmg_asset_name(&self.tag)
    }

    /// The disk image's `shasum -a 256` file.
    pub fn checksum_name(&self) -> String {
        format!("{}.sha256", self.dmg_name())
    }

    pub fn dmg_url(&self) -> String {
        asset_url(&self.tag, &self.dmg_name())
    }

    pub fn checksum_url(&self) -> String {
        asset_url(&self.tag, &self.checksum_name())
    }
}

fn asset_url(tag: &str, name: &str) -> String {
    format!("https://github.com/{RELEASE_REPO}/releases/download/{tag}/{name}")
}

/// Why a `releases/latest` response doesn't describe an installable release.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReleaseError {
    #[error("unexpected GitHub response ({0})")]
    Parse(String),
    #[error("the latest release is a draft")]
    Draft,
    #[error("the latest release is a prerelease")]
    Prerelease,
    #[error("the latest release tag {0:?} is not a Warpium version")]
    MalformedTag(String),
    #[error("the latest release {tag} has no {name} asset")]
    MissingAsset { tag: String, name: String },
}

impl ReleaseError {
    /// Whether the release is one to skip silently rather than an error: GitHub's "latest"
    /// shouldn't ever be a draft, a prerelease or a non-Warpium tag, but if it is, it is simply
    /// not offered.
    pub fn is_ignored_release(&self) -> bool {
        match self {
            Self::Draft | Self::Prerelease | Self::MalformedTag(_) => true,
            Self::Parse(_) | Self::MissingAsset { .. } => false,
        }
    }
}

#[derive(Deserialize)]
struct ReleaseResponse {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<AssetResponse>,
}

#[derive(Deserialize)]
struct AssetResponse {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    state: Option<String>,
}

/// The arm64 disk image's file name for `tag`, as the release workflow uploads it.
pub fn dmg_asset_name(tag: &str) -> String {
    format!("Warpium-{tag}-arm64.dmg")
}

/// Parses a `GET /repos/{repo}/releases/latest` (or `/releases/tags/{tag}`) response body.
pub fn parse_latest_release(body: &[u8]) -> Result<Release, ReleaseError> {
    let response: ReleaseResponse = serde_json::from_slice(body)
        .map_err(|err| ReleaseError::Parse(format!("release response: {err}")))?;
    if response.draft {
        return Err(ReleaseError::Draft);
    }
    if response.prerelease {
        return Err(ReleaseError::Prerelease);
    }
    let version = WarpiumVersion::parse(&response.tag_name)
        .ok_or_else(|| ReleaseError::MalformedTag(response.tag_name.clone()))?;
    let release = Release {
        tag: version.tag().to_owned(),
        dmg_size: 0,
    };
    let dmg = select_asset(&release.tag, &response.assets, &release.dmg_name())?;
    select_asset(&release.tag, &response.assets, &release.checksum_name())?;
    Ok(Release {
        dmg_size: dmg.size,
        ..release
    })
}

fn select_asset<'a>(
    tag: &str,
    assets: &'a [AssetResponse],
    name: &str,
) -> Result<&'a AssetResponse, ReleaseError> {
    assets
        .iter()
        // Assets still uploading are listed with another state and can't be downloaded yet.
        .find(|asset| {
            asset.name == name
                && asset
                    .state
                    .as_deref()
                    .is_none_or(|state| state == "uploaded")
        })
        .ok_or_else(|| ReleaseError::MissingAsset {
            tag: tag.to_owned(),
            name: name.to_owned(),
        })
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod tests;

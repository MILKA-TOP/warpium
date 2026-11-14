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

/// A downloadable release asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    /// Built from the tag and the name rather than taken from the response, so a download can
    /// only ever come from this repository's releases.
    pub url: String,
    /// Size in bytes as reported by GitHub; 0 when unknown.
    pub size: u64,
}

/// A published Warpium release that can be installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub tag: String,
    pub dmg: ReleaseAsset,
    /// The `shasum -a 256` file for [`Release::dmg`].
    pub checksum: ReleaseAsset,
}

impl Release {
    /// The parsed version. Always `Some` for a release produced by [`parse_latest_release`];
    /// `None` only for a hand-edited saved state.
    pub fn version(&self) -> Option<WarpiumVersion> {
        WarpiumVersion::parse(&self.tag)
    }

    pub fn page_url(&self) -> String {
        release_page_url(&self.tag)
    }
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
    let tag = version.tag().to_owned();

    let dmg_name = dmg_asset_name(&tag);
    let checksum_name = format!("{dmg_name}.sha256");
    let dmg = select_asset(&tag, &response.assets, &dmg_name)?;
    let checksum = select_asset(&tag, &response.assets, &checksum_name)?;
    Ok(Release { tag, dmg, checksum })
}

fn select_asset(
    tag: &str,
    assets: &[AssetResponse],
    name: &str,
) -> Result<ReleaseAsset, ReleaseError> {
    assets
        .iter()
        // Assets still uploading are listed with another state and can't be downloaded yet.
        .find(|asset| asset.name == name && asset.state.as_deref().is_none_or(|s| s == "uploaded"))
        .map(|asset| ReleaseAsset {
            name: asset.name.clone(),
            url: format!(
                "https://github.com/{RELEASE_REPO}/releases/download/{tag}/{}",
                asset.name
            ),
            size: asset.size,
        })
        .ok_or_else(|| ReleaseError::MissingAsset {
            tag: tag.to_owned(),
            name: name.to_owned(),
        })
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod tests;

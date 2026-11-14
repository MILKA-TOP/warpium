//! Warpium: offers and installs new Warpium releases published on GitHub
//! (`MILKA-TOP/warpium`). Official Warp builds are never involved: Warp's own updater stays off
//! for the OSS channel.
//!
//! This crate holds the protocol, the decisions and the install steps; the app supplies the HTTP
//! transport, the clock, persistence, the download and the UI.

mod check;
mod checksum;
mod install;
mod notice;
mod release;
mod version;

pub use check::{CheckError, CheckOutcome, CheckState, next_check_delay, run_check};
pub use checksum::{ChecksumError, Sha256Hasher, parse_checksum_file};
pub use install::{
    BUNDLE_ID, InstallError, InstallRequest, InstallTools, Installed, MacTools, install,
    remove_backup,
};
pub use notice::{
    banner_description, banner_heading, manual_report, progress_percent, update_to_offer,
};
pub use release::{
    RELEASE_REPO, Release, ReleaseAsset, ReleaseError, dmg_asset_name, parse_latest_release,
    release_page_url,
};
pub use version::{WarpiumVersion, resolve_current_version};
pub use warpium_upstream_check::{HttpRequest, HttpResponse, Trigger, check_interval};

#[cfg(test)]
#[path = "oss_autoupdate_guard_tests.rs"]
mod oss_autoupdate_guard_tests;

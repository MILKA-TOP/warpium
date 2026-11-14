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
    BUNDLE_ID, InstallError, InstallTools, MacTools, PrepareRequest, Prepared,
    abandoned_staged_copies, backup_path, keep_as_backup, prepare, process_is_running,
    remove_backup, swap_bundles,
};
pub use notice::{
    FAILED_HEADING, OFFER_HEADING, PREPARING_HEADING, READY_HEADING, downloading_heading,
    in_progress_description, manual_report, offer_description, progress_percent, ready_description,
    release_name, update_to_offer,
};
pub use release::{
    RELEASE_REPO, Release, ReleaseError, dmg_asset_name, parse_latest_release, release_page_url,
};
pub use version::{WarpiumVersion, resolve_current_version};
pub use warpium_upstream_check::{HttpRequest, HttpResponse, Trigger, check_interval};

#[cfg(test)]
#[path = "oss_autoupdate_guard_tests.rs"]
mod oss_autoupdate_guard_tests;

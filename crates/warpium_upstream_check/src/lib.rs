//! Warpium: notices when upstream Warp (`warpdotdev/warp`) has commits the running build lacks.
//!
//! The build embeds its upstream base commit (the merge base of the build commit with upstream
//! `master`). A check asks the GitHub REST API how far upstream `master` is ahead of that commit.
//! This crate holds the protocol and the decisions; the app supplies the HTTP transport, the
//! clock, persistence and the UI.

mod check;
mod github;
mod notice;

pub use check::{
    CheckError, CheckOutcome, CheckState, Comparison, HttpRequest, HttpResponse, Trigger,
    UpstreamHead, UpstreamStatus, backoff_delay, next_check_delay, run_check,
};
pub use github::{
    FORK_REPO, FORK_URL, UPSTREAM_BRANCH, UPSTREAM_REPO, compare_web_url, parse_commit_sha,
    rate_limit_until, resolve_base,
};
pub use notice::{
    banner_description, banner_heading, check_interval, manual_report, should_notify,
};

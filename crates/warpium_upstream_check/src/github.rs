//! GitHub REST API endpoints and response parsing.

use serde::Deserialize;

/// The upstream repository Warpium is forked from.
pub const UPSTREAM_REPO: &str = "warpdotdev/warp";
pub const UPSTREAM_BRANCH: &str = "master";
pub const FORK_REPO: &str = "MILKA-TOP/warpium";
/// The fork's GitHub page, which hosts GitHub's "Sync fork" button.
pub const FORK_URL: &str = "https://github.com/MILKA-TOP/warpium";

const API_ROOT: &str = "https://api.github.com";

/// Returns `value` as a lowercase commit SHA if it is an abbreviated or full hex commit id.
pub fn parse_commit_sha(value: &str) -> Option<String> {
    let value = value.trim();
    let is_sha = (7..=40).contains(&value.len()) && value.chars().all(|c| c.is_ascii_hexdigit());
    is_sha.then(|| value.to_ascii_lowercase())
}

/// Picks the upstream base commit to compare against: a valid runtime override wins over the
/// commit embedded at build time. Returns `None` when neither is a commit SHA.
pub fn resolve_base(runtime_override: Option<&str>, embedded: Option<&str>) -> Option<String> {
    runtime_override
        .and_then(parse_commit_sha)
        .or_else(|| embedded.and_then(parse_commit_sha))
}

/// The web page listing the upstream commits since `base`.
pub fn compare_web_url(base: &str) -> String {
    format!("https://github.com/{UPSTREAM_REPO}/compare/{base}...{UPSTREAM_BRANCH}")
}

pub(crate) fn branch_api_url() -> String {
    format!("{API_ROOT}/repos/{UPSTREAM_REPO}/branches/{UPSTREAM_BRANCH}")
}

/// The compare endpoint for `base...head`. Page 2 of a one-commit page is requested because only
/// page 1 carries the changed-files list, which can run to megabytes; every page carries the
/// counts.
pub(crate) fn compare_api_url(base: &str, head: &str) -> String {
    format!("{API_ROOT}/repos/{UPSTREAM_REPO}/compare/{base}...{head}?per_page=1&page=2")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BranchHead {
    pub sha: String,
    /// ISO 8601 committer date of the head commit.
    pub date: String,
}

#[derive(Deserialize)]
struct BranchResponse {
    commit: BranchCommit,
}

#[derive(Deserialize)]
struct BranchCommit {
    sha: String,
    commit: CommitDetails,
}

#[derive(Deserialize)]
struct CommitDetails {
    committer: Option<Signature>,
    author: Option<Signature>,
}

#[derive(Deserialize)]
struct Signature {
    date: Option<String>,
}

/// Parses a `GET /repos/{repo}/branches/{branch}` response body.
pub(crate) fn parse_branch(body: &[u8]) -> Result<BranchHead, String> {
    let response: BranchResponse =
        serde_json::from_slice(body).map_err(|err| format!("branch response: {err}"))?;
    let sha = parse_commit_sha(&response.commit.sha)
        .ok_or_else(|| "branch response: head is not a commit SHA".to_owned())?;
    let details = response.commit.commit;
    let date = [details.committer, details.author]
        .into_iter()
        .flatten()
        .find_map(|signature| signature.date)
        .unwrap_or_default();
    Ok(BranchHead { sha, date })
}

#[derive(Deserialize)]
struct CompareResponse {
    ahead_by: u32,
}

/// Parses a compare response body into the number of commits the head is ahead of the base.
pub(crate) fn parse_compare(body: &[u8]) -> Result<u32, String> {
    serde_json::from_slice::<CompareResponse>(body)
        .map(|response| response.ahead_by)
        .map_err(|err| format!("compare response: {err}"))
}

/// When a rate-limited request may be retried, as Unix seconds, or `None` when the response is
/// not a rate-limit response.
///
/// GitHub answers 403 or 429 when rate limited and documents `Retry-After` (seconds) and
/// `X-RateLimit-Reset` (Unix seconds) with `X-RateLimit-Remaining: 0` as the signals.
pub(crate) fn rate_limit_until(
    status: u16,
    retry_after: Option<&str>,
    remaining: Option<&str>,
    reset: Option<&str>,
    now: u64,
) -> Option<u64> {
    /// Used when GitHub signals a limit without saying for how long.
    const DEFAULT_WAIT_SECS: u64 = 60 * 60;

    if status != 403 && status != 429 {
        return None;
    }
    let parse = |value: Option<&str>| value.and_then(|value| value.trim().parse::<u64>().ok());
    if let Some(seconds) = parse(retry_after) {
        return Some(now + seconds);
    }
    if parse(remaining) == Some(0) {
        return Some(
            parse(reset)
                .filter(|&reset| reset > now)
                .unwrap_or(now + DEFAULT_WAIT_SECS),
        );
    }
    // A 429 is always a rate limit; a 403 without the headers is an ordinary refusal.
    (status == 429).then_some(now + DEFAULT_WAIT_SECS)
}

#[cfg(test)]
#[path = "github_tests.rs"]
mod tests;

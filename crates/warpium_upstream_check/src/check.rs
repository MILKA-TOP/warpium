//! One upstream check: fetch upstream `master`'s head (conditionally, with its `ETag`), then the
//! number of commits it is ahead of the build's base, with error back-off and rate-limit handling.

use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::github::{
    branch_api_url, compare_api_url, parse_branch, parse_compare, rate_limit_until,
};

/// What started a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The launch-time or periodic check; skipped while backing off after an error.
    Scheduled,
    /// The command-palette check; ignores error back-off but still honors a rate limit.
    Manual,
}

/// A GET request for the app's HTTP client to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    /// Sent as `If-None-Match`.
    pub if_none_match: Option<String>,
}

/// The parts of an HTTP response a check needs. Header values are passed through verbatim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub etag: Option<String>,
    pub retry_after: Option<String>,
    pub rate_limit_remaining: Option<String>,
    pub rate_limit_reset: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamHead {
    pub sha: String,
    /// ISO 8601 committer date.
    pub date: String,
}

/// How far `head_sha` is ahead of `base`. Commits are immutable, so this never goes stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    pub base: String,
    pub head_sha: String,
    pub ahead_by: u32,
}

/// Everything remembered between checks and across launches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckState {
    /// `ETag` of the last successful branch response, valid only together with `head`.
    pub branch_etag: Option<String>,
    pub head: Option<UpstreamHead>,
    pub comparison: Option<Comparison>,
    /// The upstream head whose notification the user dismissed.
    pub dismissed_head: Option<String>,
    /// Unix seconds of the last successful check.
    pub last_success: Option<u64>,
    /// Failed checks since the last success.
    pub consecutive_failures: u32,
    /// Unix seconds before which scheduled checks are skipped after a failure.
    pub retry_at: Option<u64>,
    /// Unix seconds before which no request is sent at all, as GitHub asked.
    pub rate_limited_until: Option<u64>,
}

impl CheckState {
    /// Restores saved state; unreadable state starts over rather than failing.
    pub fn from_json(json: &str) -> Self {
        serde_json::from_str(json).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("check state serializes to JSON")
    }
}

/// The result of a successful check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamStatus {
    pub base: String,
    pub head_sha: String,
    pub head_date: String,
    /// Upstream commits not contained in `base`.
    pub ahead_by: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckError {
    #[error("could not reach GitHub: {0}")]
    Transport(String),
    #[error("GitHub's rate limit was reached")]
    RateLimited { until: u64 },
    #[error("GitHub answered HTTP {status}")]
    Http { status: u16 },
    #[error("upstream Warp does not contain this build's base commit")]
    BaseNotFound,
    #[error("unexpected GitHub response ({0})")]
    Parse(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    Checked(UpstreamStatus),
    Failed(CheckError),
    /// No request was sent because of back-off or a rate limit.
    Deferred {
        until: u64,
    },
}

/// Delay before the next scheduled check after a failure: 15 minutes, doubling per consecutive
/// failure, capped at `max`.
pub fn backoff_delay(consecutive_failures: u32, max: Duration) -> Duration {
    const BASE: Duration = Duration::from_secs(15 * 60);
    let exponent = consecutive_failures.saturating_sub(1).min(16);
    BASE.saturating_mul(1 << exponent).min(max)
}

/// Delay before the next scheduled check: the back-off or rate-limit wait while one is active,
/// otherwise `interval`.
pub fn next_check_delay(state: &CheckState, now: u64, interval: Duration) -> Duration {
    let wait_until = state.retry_at.max(state.rate_limited_until);
    match wait_until {
        Some(until) if until > now => Duration::from_secs(until - now),
        _ => interval,
    }
}

/// Runs one check of upstream `master` against `base` and returns the updated state with the
/// outcome. `fetch` sends a request and returns the response, or a transport error message.
pub async fn run_check<F, Fut>(
    mut state: CheckState,
    base: &str,
    now: u64,
    max_backoff: Duration,
    trigger: Trigger,
    fetch: F,
) -> (CheckState, CheckOutcome)
where
    F: Fn(HttpRequest) -> Fut,
    Fut: Future<Output = Result<HttpResponse, String>>,
{
    let blocked_until = match trigger {
        Trigger::Scheduled => state.retry_at.max(state.rate_limited_until),
        Trigger::Manual => state.rate_limited_until,
    };
    if let Some(until) = blocked_until.filter(|&until| until > now) {
        return (state, CheckOutcome::Deferred { until });
    }

    let outcome = match fetch_status(&mut state, base, now, &fetch).await {
        Ok(status) => {
            state.last_success = Some(now);
            state.consecutive_failures = 0;
            state.retry_at = None;
            state.rate_limited_until = None;
            CheckOutcome::Checked(status)
        }
        Err(err) => {
            state.consecutive_failures = state.consecutive_failures.saturating_add(1);
            let mut retry_at =
                now + backoff_delay(state.consecutive_failures, max_backoff).as_secs();
            if let CheckError::RateLimited { until } = err {
                state.rate_limited_until = Some(until);
                retry_at = retry_at.max(until);
            }
            state.retry_at = Some(retry_at);
            CheckOutcome::Failed(err)
        }
    };
    (state, outcome)
}

async fn fetch_status<F, Fut>(
    state: &mut CheckState,
    base: &str,
    now: u64,
    fetch: &F,
) -> Result<UpstreamStatus, CheckError>
where
    F: Fn(HttpRequest) -> Fut,
    Fut: Future<Output = Result<HttpResponse, String>>,
{
    let head = fetch_head(state, now, fetch).await?;

    let cached = state
        .comparison
        .as_ref()
        .filter(|comparison| comparison.base == base && comparison.head_sha == head.sha)
        .map(|comparison| comparison.ahead_by);
    let ahead_by = match cached {
        Some(ahead_by) => ahead_by,
        None => {
            let request = HttpRequest {
                url: compare_api_url(base, &head.sha),
                if_none_match: None,
            };
            let response = fetch(request).await.map_err(CheckError::Transport)?;
            match response.status {
                200 => parse_compare(&response.body).map_err(CheckError::Parse)?,
                404 => return Err(CheckError::BaseNotFound),
                status => return Err(error_for_status(status, &response, now)),
            }
        }
    };
    state.comparison = Some(Comparison {
        base: base.to_owned(),
        head_sha: head.sha.clone(),
        ahead_by,
    });

    Ok(UpstreamStatus {
        base: base.to_owned(),
        head_sha: head.sha,
        head_date: head.date,
        ahead_by,
    })
}

/// Fetches upstream `master`'s head, revalidating the remembered one with its `ETag`.
async fn fetch_head<F, Fut>(
    state: &mut CheckState,
    now: u64,
    fetch: &F,
) -> Result<UpstreamHead, CheckError>
where
    F: Fn(HttpRequest) -> Fut,
    Fut: Future<Output = Result<HttpResponse, String>>,
{
    let request = HttpRequest {
        url: branch_api_url(),
        if_none_match: state
            .head
            .is_some()
            .then(|| state.branch_etag.clone())
            .flatten(),
    };
    let response = fetch(request).await.map_err(CheckError::Transport)?;
    match response.status {
        200 => {
            let head = parse_branch(&response.body).map_err(CheckError::Parse)?;
            let head = UpstreamHead {
                sha: head.sha,
                date: head.date,
            };
            state.branch_etag = response.etag;
            state.head = Some(head.clone());
            Ok(head)
        }
        304 => match &state.head {
            Some(head) => Ok(head.clone()),
            // Only reachable if the server ignores which ETag was sent; refetch next time.
            None => {
                state.branch_etag = None;
                Err(CheckError::Http { status: 304 })
            }
        },
        status => Err(error_for_status(status, &response, now)),
    }
}

fn error_for_status(status: u16, response: &HttpResponse, now: u64) -> CheckError {
    match rate_limit_until(
        status,
        response.retry_after.as_deref(),
        response.rate_limit_remaining.as_deref(),
        response.rate_limit_reset.as_deref(),
        now,
    ) {
        Some(until) => CheckError::RateLimited { until },
        None => CheckError::Http { status },
    }
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;

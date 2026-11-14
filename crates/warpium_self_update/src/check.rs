//! One update check: fetch the latest Warpium release (conditionally, with its `ETag`), with
//! error back-off and rate-limit handling shared with the upstream check.

use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use warpium_upstream_check::{HttpRequest, HttpResponse, Trigger, backoff_delay, rate_limit_until};

use crate::release::{Release, ReleaseError, latest_release_api_url, parse_latest_release};

/// Everything remembered between checks and across launches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckState {
    /// `ETag` of the last successful response, valid only while `last_success` is set.
    pub etag: Option<String>,
    /// The latest installable release as of the last successful check; `None` when there is
    /// none (no releases, or GitHub's latest is one that is ignored).
    pub latest: Option<Release>,
    /// The release tag the user chose to skip.
    pub skipped_tag: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckError {
    #[error("could not reach GitHub: {0}")]
    Transport(String),
    #[error("GitHub's rate limit was reached")]
    RateLimited { until: u64 },
    #[error("GitHub answered HTTP {status}")]
    Http { status: u16 },
    #[error(transparent)]
    Release(ReleaseError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// The check succeeded; the latest installable release, if there is one.
    Checked(Option<Release>),
    Failed(CheckError),
    /// No request was sent because of back-off or a rate limit.
    Deferred {
        until: u64,
    },
}

/// Delay before the next scheduled check: the back-off or rate-limit wait while one is active,
/// otherwise whatever remains of `interval` since the last successful check (at once when there
/// has been none).
pub fn next_check_delay(state: &CheckState, now: u64, interval: Duration) -> Duration {
    let wait_until = state.retry_at.max(state.rate_limited_until);
    if let Some(until) = wait_until.filter(|&until| until > now) {
        return Duration::from_secs(until - now);
    }
    match state.last_success {
        Some(last_success) => {
            let due = last_success.saturating_add(interval.as_secs());
            Duration::from_secs(due.saturating_sub(now).min(interval.as_secs()))
        }
        None => Duration::ZERO,
    }
}

/// Runs one check for the latest Warpium release and returns the updated state with the outcome.
/// `fetch` sends a request and returns the response, or a transport error message.
pub async fn run_check<F, Fut>(
    mut state: CheckState,
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

    let outcome = match fetch_latest(&mut state, now, &fetch).await {
        Ok(latest) => {
            state.latest = latest.clone();
            state.last_success = Some(now);
            state.consecutive_failures = 0;
            state.retry_at = None;
            state.rate_limited_until = None;
            CheckOutcome::Checked(latest)
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

async fn fetch_latest<F, Fut>(
    state: &mut CheckState,
    now: u64,
    fetch: &F,
) -> Result<Option<Release>, CheckError>
where
    F: Fn(HttpRequest) -> Fut,
    Fut: Future<Output = Result<HttpResponse, String>>,
{
    let request = HttpRequest {
        url: latest_release_api_url(),
        if_none_match: state
            .last_success
            .is_some()
            .then(|| state.etag.clone())
            .flatten(),
    };
    let response = fetch(request).await.map_err(CheckError::Transport)?;
    match response.status {
        200 => {
            let latest = match parse_latest_release(&response.body) {
                Ok(release) => Some(release),
                Err(err) if err.is_ignored_release() => None,
                Err(err) => return Err(CheckError::Release(err)),
            };
            state.etag = response.etag;
            Ok(latest)
        }
        304 if state.last_success.is_some() => Ok(state.latest.clone()),
        // Only reachable if the server ignores which ETag was sent; refetch next time.
        304 => {
            state.etag = None;
            Err(CheckError::Http { status: 304 })
        }
        // The repository has no published release yet.
        404 => {
            state.etag = None;
            Ok(None)
        }
        status => Err(
            match rate_limit_until(
                status,
                response.retry_after.as_deref(),
                response.rate_limit_remaining.as_deref(),
                response.rate_limit_reset.as_deref(),
                now,
            ) {
                Some(until) => CheckError::RateLimited { until },
                None => CheckError::Http { status },
            },
        ),
    }
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;

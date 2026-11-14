use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;

use futures::executor::block_on;
use serde_json::json;

use super::*;

const LATEST_FIXTURE: &str = include_str!("../fixtures/releases_latest.json");
const LATEST_URL: &str = "https://api.github.com/repos/MILKA-TOP/warpium/releases/latest";
const TAG: &str = "v0.2026.06.03.09.49.stable_00.1";
const NOW: u64 = 1_000_000;
const MAX_BACKOFF: Duration = Duration::from_secs(6 * 60 * 60);

/// Replays canned responses in order and records the requests it was sent.
struct FakeGitHub {
    responses: RefCell<VecDeque<Result<HttpResponse, String>>>,
    requests: RefCell<Vec<HttpRequest>>,
}

impl FakeGitHub {
    fn new(responses: Vec<Result<HttpResponse, String>>) -> Self {
        Self {
            responses: RefCell::new(responses.into()),
            requests: RefCell::new(Vec::new()),
        }
    }

    fn run(&self, state: CheckState, trigger: Trigger) -> (CheckState, CheckOutcome) {
        block_on(run_check(state, NOW, MAX_BACKOFF, trigger, |request| {
            self.requests.borrow_mut().push(request);
            let response = self
                .responses
                .borrow_mut()
                .pop_front()
                .expect("unexpected request");
            async move { response }
        }))
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.requests.borrow().clone()
    }
}

fn ok(body: &str, etag: Option<&str>) -> Result<HttpResponse, String> {
    Ok(HttpResponse {
        status: 200,
        etag: etag.map(str::to_owned),
        body: body.as_bytes().to_vec(),
        ..Default::default()
    })
}

fn status(status: u16) -> Result<HttpResponse, String> {
    Ok(HttpResponse {
        status,
        ..Default::default()
    })
}

fn fixture_release() -> Release {
    crate::release::parse_latest_release(LATEST_FIXTURE.as_bytes()).unwrap()
}

fn checked_state() -> CheckState {
    CheckState {
        etag: Some("W/\"etag-1\"".to_owned()),
        latest: Some(fixture_release()),
        last_success: Some(NOW - 60),
        ..Default::default()
    }
}

#[test]
fn first_check_fetches_the_latest_release_unconditionally() {
    let github = FakeGitHub::new(vec![ok(LATEST_FIXTURE, Some("W/\"etag-1\""))]);

    let (state, outcome) = github.run(CheckState::default(), Trigger::Scheduled);

    assert_eq!(outcome, CheckOutcome::Checked(Some(fixture_release())));
    assert_eq!(
        github.requests(),
        vec![HttpRequest {
            url: LATEST_URL.to_owned(),
            if_none_match: None,
        }]
    );
    assert_eq!(state.etag.as_deref(), Some("W/\"etag-1\""));
    assert_eq!(state.latest, Some(fixture_release()));
    assert_eq!(state.last_success, Some(NOW));
}

#[test]
fn revalidates_with_the_etag_and_reuses_the_release_on_304() {
    let github = FakeGitHub::new(vec![status(304)]);

    let (state, outcome) = github.run(checked_state(), Trigger::Scheduled);

    assert_eq!(outcome, CheckOutcome::Checked(Some(fixture_release())));
    assert_eq!(
        github.requests()[0].if_none_match.as_deref(),
        Some("W/\"etag-1\"")
    );
    assert_eq!(state.etag.as_deref(), Some("W/\"etag-1\""));
    assert_eq!(state.last_success, Some(NOW));
}

#[test]
fn an_unexpected_304_without_a_previous_result_is_an_error_and_drops_the_etag() {
    let github = FakeGitHub::new(vec![status(304)]);
    let state = CheckState {
        etag: Some("W/\"stale\"".to_owned()),
        ..Default::default()
    };

    let (state, outcome) = github.run(state, Trigger::Scheduled);

    assert_eq!(
        outcome,
        CheckOutcome::Failed(CheckError::Http { status: 304 })
    );
    assert_eq!(github.requests()[0].if_none_match, None);
    assert_eq!(state.etag, None);
}

#[test]
fn an_ignored_latest_release_is_a_successful_check_with_nothing_to_offer() {
    let body = json!({ "tag_name": TAG, "prerelease": true, "assets": [] }).to_string();
    let github = FakeGitHub::new(vec![ok(&body, Some("W/\"etag-2\""))]);

    let (state, outcome) = github.run(checked_state(), Trigger::Scheduled);

    assert_eq!(outcome, CheckOutcome::Checked(None));
    assert_eq!(state.latest, None);
    assert_eq!(state.etag.as_deref(), Some("W/\"etag-2\""));
}

#[test]
fn no_published_release_is_a_successful_check_with_nothing_to_offer() {
    let github = FakeGitHub::new(vec![status(404)]);

    let (state, outcome) = github.run(CheckState::default(), Trigger::Scheduled);

    assert_eq!(outcome, CheckOutcome::Checked(None));
    assert_eq!(state.last_success, Some(NOW));
}

#[test]
fn a_release_without_its_assets_is_an_error_that_keeps_the_previous_result() {
    let body = json!({ "tag_name": "v0.2026.06.03.09.49.stable_00.2", "assets": [] }).to_string();
    let github = FakeGitHub::new(vec![ok(&body, Some("W/\"etag-2\""))]);

    let (state, outcome) = github.run(checked_state(), Trigger::Scheduled);

    assert!(
        matches!(
            outcome,
            CheckOutcome::Failed(CheckError::Release(ReleaseError::MissingAsset { .. }))
        ),
        "{outcome:?}"
    );
    assert_eq!(state.latest, Some(fixture_release()));
    assert_eq!(state.etag.as_deref(), Some("W/\"etag-1\""));
    assert_eq!(state.consecutive_failures, 1);
}

#[test]
fn a_transport_error_backs_off() {
    let github = FakeGitHub::new(vec![Err("connection refused".to_owned())]);

    let (state, outcome) = github.run(CheckState::default(), Trigger::Scheduled);

    assert_eq!(
        outcome,
        CheckOutcome::Failed(CheckError::Transport("connection refused".to_owned()))
    );
    assert_eq!(state.consecutive_failures, 1);
    assert_eq!(state.retry_at, Some(NOW + 15 * 60));
}

#[test]
fn back_off_doubles_per_consecutive_failure() {
    let github = FakeGitHub::new(vec![status(500)]);
    let state = CheckState {
        consecutive_failures: 2,
        ..Default::default()
    };

    let (state, _) = github.run(state, Trigger::Scheduled);

    assert_eq!(state.consecutive_failures, 3);
    assert_eq!(state.retry_at, Some(NOW + 60 * 60));
}

#[test]
fn a_scheduled_check_is_deferred_while_backing_off() {
    let github = FakeGitHub::new(vec![]);
    let state = CheckState {
        retry_at: Some(NOW + 100),
        ..Default::default()
    };

    let (_, outcome) = github.run(state, Trigger::Scheduled);

    assert_eq!(outcome, CheckOutcome::Deferred { until: NOW + 100 });
    assert_eq!(github.requests(), vec![]);
}

#[test]
fn a_manual_check_ignores_back_off() {
    let github = FakeGitHub::new(vec![ok(LATEST_FIXTURE, None)]);
    let state = CheckState {
        retry_at: Some(NOW + 100),
        consecutive_failures: 3,
        ..Default::default()
    };

    let (state, outcome) = github.run(state, Trigger::Manual);

    assert_eq!(outcome, CheckOutcome::Checked(Some(fixture_release())));
    assert_eq!(state.consecutive_failures, 0);
    assert_eq!(state.retry_at, None);
}

#[test]
fn a_manual_check_still_honors_a_rate_limit() {
    let github = FakeGitHub::new(vec![]);
    let state = CheckState {
        rate_limited_until: Some(NOW + 300),
        ..Default::default()
    };

    let (_, outcome) = github.run(state, Trigger::Manual);

    assert_eq!(outcome, CheckOutcome::Deferred { until: NOW + 300 });
}

#[test]
fn a_rate_limit_response_waits_until_the_reset() {
    let github = FakeGitHub::new(vec![Ok(HttpResponse {
        status: 403,
        rate_limit_remaining: Some("0".to_owned()),
        rate_limit_reset: Some((NOW + 2 * 60 * 60).to_string()),
        ..Default::default()
    })]);

    let (state, outcome) = github.run(CheckState::default(), Trigger::Scheduled);

    assert_eq!(
        outcome,
        CheckOutcome::Failed(CheckError::RateLimited {
            until: NOW + 2 * 60 * 60
        })
    );
    assert_eq!(state.rate_limited_until, Some(NOW + 2 * 60 * 60));
    assert_eq!(state.retry_at, Some(NOW + 2 * 60 * 60));
}

#[test]
fn a_successful_check_keeps_the_skipped_tag() {
    let github = FakeGitHub::new(vec![ok(LATEST_FIXTURE, None)]);
    let state = CheckState {
        skipped_tag: Some(TAG.to_owned()),
        ..Default::default()
    };

    let (state, _) = github.run(state, Trigger::Scheduled);

    assert_eq!(state.skipped_tag.as_deref(), Some(TAG));
}

#[test]
fn the_first_check_is_due_at_once() {
    assert_eq!(
        next_check_delay(&CheckState::default(), NOW, MAX_BACKOFF),
        Duration::ZERO
    );
}

#[test]
fn the_next_check_is_due_an_interval_after_the_last_success() {
    let state = CheckState {
        last_success: Some(NOW - 60 * 60),
        ..Default::default()
    };

    assert_eq!(
        next_check_delay(&state, NOW, MAX_BACKOFF),
        Duration::from_secs(5 * 60 * 60)
    );
}

#[test]
fn an_overdue_check_is_due_at_once() {
    let state = CheckState {
        last_success: Some(NOW - 7 * 60 * 60),
        ..Default::default()
    };

    assert_eq!(next_check_delay(&state, NOW, MAX_BACKOFF), Duration::ZERO);
}

#[test]
fn the_next_check_waits_out_back_off() {
    let state = CheckState {
        last_success: Some(NOW - 7 * 60 * 60),
        retry_at: Some(NOW + 900),
        ..Default::default()
    };

    assert_eq!(
        next_check_delay(&state, NOW, MAX_BACKOFF),
        Duration::from_secs(900)
    );
}

#[test]
fn state_round_trips_through_json() {
    let state = CheckState {
        skipped_tag: Some(TAG.to_owned()),
        ..checked_state()
    };

    assert_eq!(CheckState::from_json(&state.to_json()), state);
}

#[test]
fn unreadable_state_starts_over() {
    assert_eq!(CheckState::from_json("not json"), CheckState::default());
}

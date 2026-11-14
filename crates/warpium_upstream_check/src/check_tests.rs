use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;

use futures::executor::block_on;

use super::*;

const BRANCH_FIXTURE: &str = include_str!("../fixtures/branch_master.json");
const COMPARE_FIXTURE: &str = include_str!("../fixtures/compare_page2.json");
const HEAD: &str = "c6f07bb451657e6fbce72c69ed90cfd4f86c2344";
const BASE: &str = "20ec6971137a5cf06c1860aa1809c19d75de91a0";
const BRANCH_URL: &str = "https://api.github.com/repos/warpdotdev/warp/branches/master";
const COMPARE_URL: &str = "https://api.github.com/repos/warpdotdev/warp/compare/20ec6971137a5cf06c1860aa1809c19d75de91a0...c6f07bb451657e6fbce72c69ed90cfd4f86c2344?per_page=1&page=2";
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
        block_on(run_check(
            state,
            BASE,
            NOW,
            MAX_BACKOFF,
            trigger,
            |request| {
                self.requests.borrow_mut().push(request);
                let response = self
                    .responses
                    .borrow_mut()
                    .pop_front()
                    .expect("unexpected request");
                async move { response }
            },
        ))
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

fn known_head_state() -> CheckState {
    CheckState {
        branch_etag: Some("W/\"etag-1\"".to_owned()),
        head: Some(UpstreamHead {
            sha: HEAD.to_owned(),
            date: "2026-11-14T07:36:23Z".to_owned(),
        }),
        comparison: Some(Comparison {
            base: BASE.to_owned(),
            head_sha: HEAD.to_owned(),
            ahead_by: 29,
        }),
        ..Default::default()
    }
}

#[test]
fn first_check_reports_ahead_count_and_head() {
    let github = FakeGitHub::new(vec![
        ok(BRANCH_FIXTURE, Some("W/\"etag-1\"")),
        ok(COMPARE_FIXTURE, None),
    ]);

    let (_, outcome) = github.run(CheckState::default(), Trigger::Scheduled);

    assert_eq!(
        outcome,
        CheckOutcome::Checked(UpstreamStatus {
            base: BASE.to_owned(),
            head_sha: HEAD.to_owned(),
            head_date: "2026-11-14T07:36:23Z".to_owned(),
            ahead_by: 29,
        })
    );
}

#[test]
fn first_check_sends_no_etag_then_compares_base_with_the_fetched_head() {
    let github = FakeGitHub::new(vec![
        ok(BRANCH_FIXTURE, Some("W/\"etag-1\"")),
        ok(COMPARE_FIXTURE, None),
    ]);

    github.run(CheckState::default(), Trigger::Scheduled);

    assert_eq!(
        github.requests(),
        vec![
            HttpRequest {
                url: BRANCH_URL.to_owned(),
                if_none_match: None,
            },
            HttpRequest {
                url: COMPARE_URL.to_owned(),
                if_none_match: None,
            },
        ]
    );
}

#[test]
fn successful_check_remembers_etag_head_and_comparison() {
    let github = FakeGitHub::new(vec![
        ok(BRANCH_FIXTURE, Some("W/\"etag-1\"")),
        ok(COMPARE_FIXTURE, None),
    ]);

    let (state, _) = github.run(CheckState::default(), Trigger::Scheduled);

    assert_eq!(
        state,
        CheckState {
            last_success: Some(NOW),
            ..known_head_state()
        }
    );
}

#[test]
fn unchanged_head_is_revalidated_with_the_etag_and_not_compared_again() {
    let github = FakeGitHub::new(vec![status(304)]);

    let (_, outcome) = github.run(known_head_state(), Trigger::Scheduled);

    assert_eq!(
        github.requests(),
        vec![HttpRequest {
            url: BRANCH_URL.to_owned(),
            if_none_match: Some("W/\"etag-1\"".to_owned()),
        }]
    );
    assert!(matches!(outcome, CheckOutcome::Checked(status) if status.ahead_by == 29));
}

#[test]
fn a_new_base_is_compared_even_when_the_head_is_unchanged() {
    let state = CheckState {
        comparison: Some(Comparison {
            base: "1111111".to_owned(),
            head_sha: HEAD.to_owned(),
            ahead_by: 3,
        }),
        ..known_head_state()
    };
    let github = FakeGitHub::new(vec![status(304), ok(COMPARE_FIXTURE, None)]);

    let (state, _) = github.run(state, Trigger::Scheduled);

    assert_eq!(github.requests().len(), 2);
    assert_eq!(state.comparison.unwrap().base, BASE);
}

#[test]
fn identical_head_and_base_report_zero_commits() {
    let github = FakeGitHub::new(vec![
        ok(BRANCH_FIXTURE, None),
        ok(
            r#"{"status": "identical", "ahead_by": 0, "behind_by": 0}"#,
            None,
        ),
    ]);

    let (_, outcome) = github.run(CheckState::default(), Trigger::Scheduled);

    assert!(matches!(outcome, CheckOutcome::Checked(status) if status.ahead_by == 0));
}

#[test]
fn unknown_base_commit_is_reported_as_base_not_found() {
    let github = FakeGitHub::new(vec![ok(BRANCH_FIXTURE, None), status(404)]);

    let (_, outcome) = github.run(CheckState::default(), Trigger::Scheduled);

    assert_eq!(outcome, CheckOutcome::Failed(CheckError::BaseNotFound));
}

#[test]
fn transport_error_starts_a_fifteen_minute_backoff() {
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
fn repeated_failures_double_the_backoff() {
    let state = CheckState {
        consecutive_failures: 2,
        ..Default::default()
    };
    let github = FakeGitHub::new(vec![status(502)]);

    let (state, outcome) = github.run(state, Trigger::Scheduled);

    assert_eq!(
        outcome,
        CheckOutcome::Failed(CheckError::Http { status: 502 })
    );
    assert_eq!(state.retry_at, Some(NOW + 60 * 60));
}

#[test]
fn failure_keeps_the_last_known_head_and_dismissal() {
    let state = CheckState {
        dismissed_head: Some(HEAD.to_owned()),
        ..known_head_state()
    };
    let github = FakeGitHub::new(vec![status(500)]);

    let (new_state, _) = github.run(state.clone(), Trigger::Scheduled);

    assert_eq!(new_state.head, state.head);
    assert_eq!(new_state.branch_etag, state.branch_etag);
    assert_eq!(new_state.dismissed_head, Some(HEAD.to_owned()));
}

#[test]
fn unparseable_branch_response_is_a_parse_error() {
    let github = FakeGitHub::new(vec![ok("<html>", None)]);

    let (_, outcome) = github.run(CheckState::default(), Trigger::Scheduled);

    assert!(matches!(
        outcome,
        CheckOutcome::Failed(CheckError::Parse(_))
    ));
}

#[test]
fn rate_limit_waits_until_the_reset_time() {
    let github = FakeGitHub::new(vec![Ok(HttpResponse {
        status: 403,
        rate_limit_remaining: Some("0".to_owned()),
        rate_limit_reset: Some((NOW + 3 * 60 * 60).to_string()),
        ..Default::default()
    })]);

    let (state, outcome) = github.run(CheckState::default(), Trigger::Scheduled);

    assert_eq!(
        outcome,
        CheckOutcome::Failed(CheckError::RateLimited {
            until: NOW + 3 * 60 * 60
        })
    );
    assert_eq!(state.rate_limited_until, Some(NOW + 3 * 60 * 60));
    assert_eq!(state.retry_at, Some(NOW + 3 * 60 * 60));
}

#[test]
fn scheduled_check_is_deferred_during_backoff_without_a_request() {
    let state = CheckState {
        consecutive_failures: 1,
        retry_at: Some(NOW + 60),
        ..Default::default()
    };
    let github = FakeGitHub::new(vec![]);

    let (_, outcome) = github.run(state, Trigger::Scheduled);

    assert_eq!(outcome, CheckOutcome::Deferred { until: NOW + 60 });
    assert!(github.requests().is_empty());
}

#[test]
fn manual_check_ignores_error_backoff() {
    let state = CheckState {
        consecutive_failures: 3,
        retry_at: Some(NOW + 60),
        ..known_head_state()
    };
    let github = FakeGitHub::new(vec![status(304)]);

    let (state, outcome) = github.run(state, Trigger::Manual);

    assert!(matches!(outcome, CheckOutcome::Checked(_)));
    assert_eq!(state.consecutive_failures, 0);
    assert_eq!(state.retry_at, None);
}

#[test]
fn manual_check_honors_a_rate_limit() {
    let state = CheckState {
        rate_limited_until: Some(NOW + 600),
        ..Default::default()
    };
    let github = FakeGitHub::new(vec![]);

    let (_, outcome) = github.run(state, Trigger::Manual);

    assert_eq!(outcome, CheckOutcome::Deferred { until: NOW + 600 });
}

#[test]
fn expired_rate_limit_no_longer_defers() {
    let state = CheckState {
        rate_limited_until: Some(NOW - 1),
        retry_at: Some(NOW - 1),
        ..known_head_state()
    };
    let github = FakeGitHub::new(vec![status(304)]);

    let (state, outcome) = github.run(state, Trigger::Scheduled);

    assert!(matches!(outcome, CheckOutcome::Checked(_)));
    assert_eq!(state.rate_limited_until, None);
}

#[test]
fn etag_is_not_sent_without_a_remembered_head() {
    let state = CheckState {
        branch_etag: Some("W/\"stale\"".to_owned()),
        ..Default::default()
    };
    let github = FakeGitHub::new(vec![ok(BRANCH_FIXTURE, None), ok(COMPARE_FIXTURE, None)]);

    github.run(state, Trigger::Scheduled);

    assert_eq!(github.requests()[0].if_none_match, None);
}

#[test]
fn backoff_delay_doubles_from_fifteen_minutes() {
    assert_eq!(backoff_delay(1, MAX_BACKOFF), Duration::from_secs(15 * 60));
    assert_eq!(backoff_delay(2, MAX_BACKOFF), Duration::from_secs(30 * 60));
    assert_eq!(
        backoff_delay(4, MAX_BACKOFF),
        Duration::from_secs(2 * 60 * 60)
    );
}

#[test]
fn backoff_delay_is_capped() {
    assert_eq!(backoff_delay(10, MAX_BACKOFF), MAX_BACKOFF);
    assert_eq!(backoff_delay(u32::MAX, MAX_BACKOFF), MAX_BACKOFF);
}

#[test]
fn next_check_delay_is_the_interval_after_a_success() {
    let state = CheckState {
        last_success: Some(NOW),
        ..Default::default()
    };

    assert_eq!(next_check_delay(&state, NOW, MAX_BACKOFF), MAX_BACKOFF);
}

#[test]
fn next_check_delay_waits_for_the_later_of_backoff_and_rate_limit() {
    let state = CheckState {
        retry_at: Some(NOW + 100),
        rate_limited_until: Some(NOW + 900),
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
        dismissed_head: Some(HEAD.to_owned()),
        consecutive_failures: 2,
        ..known_head_state()
    };

    assert_eq!(CheckState::from_json(&state.to_json()), state);
}

#[test]
fn unreadable_saved_state_starts_over() {
    assert_eq!(CheckState::from_json("{not json"), CheckState::default());
}

#[test]
fn saved_state_with_missing_fields_keeps_the_rest() {
    let state = CheckState::from_json(r#"{"dismissed_head": "abcdef1"}"#);

    assert_eq!(state.dismissed_head, Some("abcdef1".to_owned()));
    assert_eq!(state.head, None);
}

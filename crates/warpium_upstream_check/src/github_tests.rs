use super::*;

const BRANCH_FIXTURE: &str = include_str!("../fixtures/branch_master.json");
const COMPARE_FIXTURE: &str = include_str!("../fixtures/compare_page2.json");

#[test]
fn parse_commit_sha_accepts_full_and_abbreviated_shas() {
    assert_eq!(
        parse_commit_sha(" C6F07BB451657E6FBCE72C69ED90CFD4F86C2344\n"),
        Some("c6f07bb451657e6fbce72c69ed90cfd4f86c2344".to_owned())
    );
    assert_eq!(parse_commit_sha("c6f07bb"), Some("c6f07bb".to_owned()));
}

#[test]
fn parse_commit_sha_rejects_non_shas() {
    assert_eq!(parse_commit_sha(""), None);
    assert_eq!(parse_commit_sha("c6f07b"), None);
    assert_eq!(parse_commit_sha("master"), None);
    assert_eq!(
        parse_commit_sha("c6f07bb451657e6fbce72c69ed90cfd4f86c23440"),
        None
    );
    assert_eq!(parse_commit_sha("c6f07bb..master"), None);
}

#[test]
fn resolve_base_prefers_a_valid_runtime_override() {
    assert_eq!(
        resolve_base(Some("abcdef1"), Some("1234567")),
        Some("abcdef1".to_owned())
    );
}

#[test]
fn resolve_base_ignores_an_invalid_override() {
    assert_eq!(
        resolve_base(Some("not a sha"), Some("1234567")),
        Some("1234567".to_owned())
    );
}

#[test]
fn resolve_base_is_none_without_a_valid_commit() {
    assert_eq!(resolve_base(None, Some("")), None);
    assert_eq!(resolve_base(None, None), None);
}

#[test]
fn compare_web_url_compares_base_with_upstream_master() {
    assert_eq!(
        compare_web_url("abcdef1"),
        "https://github.com/warpdotdev/warp/compare/abcdef1...master"
    );
}

#[test]
fn compare_api_url_skips_the_files_page() {
    assert_eq!(
        compare_api_url("abcdef1", "1234567"),
        "https://api.github.com/repos/warpdotdev/warp/compare/abcdef1...1234567?per_page=1&page=2"
    );
}

#[test]
fn parse_branch_reads_head_sha_and_committer_date() {
    let head = parse_branch(BRANCH_FIXTURE.as_bytes()).unwrap();

    assert_eq!(
        head,
        UpstreamHead {
            sha: "c6f07bb451657e6fbce72c69ed90cfd4f86c2344".to_owned(),
            date: "2026-11-14T07:36:23Z".to_owned(),
        }
    );
}

#[test]
fn parse_branch_falls_back_to_the_author_date() {
    let body = br#"{"commit": {"sha": "abcdef1", "commit": {"committer": null, "author": {"date": "2026-01-02T03:04:05Z"}}}}"#;

    assert_eq!(parse_branch(body).unwrap().date, "2026-01-02T03:04:05Z");
}

#[test]
fn parse_branch_rejects_an_error_body() {
    let body = br#"{"message": "Branch not found", "status": "404"}"#;

    assert!(parse_branch(body).is_err());
}

#[test]
fn parse_compare_reads_ahead_by() {
    assert_eq!(parse_compare(COMPARE_FIXTURE.as_bytes()), Ok(29));
}

#[test]
fn parse_compare_rejects_truncated_json() {
    assert!(parse_compare(br#"{"ahead_by": "#).is_err());
}

#[test]
fn rate_limit_until_prefers_retry_after() {
    assert_eq!(
        rate_limit_until(429, Some("120"), Some("0"), Some("5000"), 1000),
        Some(1120)
    );
}

#[test]
fn rate_limit_until_uses_the_reset_time_when_exhausted() {
    assert_eq!(
        rate_limit_until(403, None, Some("0"), Some("5000"), 1000),
        Some(5000)
    );
}

#[test]
fn rate_limit_until_waits_an_hour_when_the_reset_time_is_past() {
    assert_eq!(
        rate_limit_until(403, None, Some("0"), Some("10"), 1000),
        Some(4600)
    );
}

#[test]
fn rate_limit_until_treats_a_bare_429_as_a_limit() {
    assert_eq!(rate_limit_until(429, None, None, None, 1000), Some(4600));
}

#[test]
fn rate_limit_until_ignores_a_403_with_quota_left() {
    assert_eq!(
        rate_limit_until(403, None, Some("12"), Some("5000"), 1000),
        None
    );
}

#[test]
fn rate_limit_until_ignores_other_statuses() {
    assert_eq!(
        rate_limit_until(500, Some("60"), Some("0"), None, 1000),
        None
    );
}
